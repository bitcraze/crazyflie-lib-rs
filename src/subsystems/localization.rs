use crazyflie_link::Packet;
use flume::{Receiver, Sender};

use crate::{Error, Result};

use crate::crazyflie::LOCALIZATION_PORT;

// Channels
const _POSITION_CHANNEL: u8 = 0;
const GENERIC_CHANNEL: u8 = 1;

// Generic channel message types
const _RANGE_STREAM_REPORT: u8 = 0;
const _RANGE_STREAM_REPORT_FP16: u8 = 1;
const _LPS_SHORT_LPP_PACKET: u8 = 2;
const EMERGENCY_STOP: u8 = 3;
const EMERGENCY_STOP_WATCHDOG: u8 = 4;
const _COMM_GNSS_NMEA: u8 = 6;
const _COMM_GNSS_PROPRIETARY: u8 = 7;
const _EXT_POSE: u8 = 8;
const _EXT_POSE_PACKED: u8 = 9;
const _LH_ANGLE_STREAM: u8 = 10;
const _LH_PERSIST_DATA: u8 = 11;

pub struct Localization{
    pub emergency: EmergencyControl,
}

impl Localization {
    pub(crate) fn new(uplink: Sender<Packet>, _downlink: Receiver<Packet>) -> Self {
        let emergency = EmergencyControl { uplink: uplink.clone() };
        Self { emergency }
    }
}

/// Emergency control interface
///
/// Provides emergency stop functionality that immediately stops all motors.
/// Note: Emergency stops use the localization CRTP port for historical reasons.
pub struct EmergencyControl {
    uplink: Sender<Packet>,
}

impl EmergencyControl {
    /// Send emergency stop command
    ///
    /// Immediately stops all motors and puts the Crazyflie into a locked state.
    /// The drone will require a reboot before it can fly again.
    pub async fn send_emergency_stop(&self) -> Result<()> {
        let mut payload = Vec::with_capacity(1);
        payload.push(EMERGENCY_STOP);
        let pk = Packet::new(LOCALIZATION_PORT, GENERIC_CHANNEL, payload);
        self.uplink.send_async(pk).await.map_err(|_| Error::Disconnected)?;
        Ok(())
    }

    /// Send emergency stop watchdog
    ///
    /// Activates/resets a watchdog failsafe that will automatically emergency stop
    /// the drone if this message isn't sent every 1000ms. Once activated by the first
    /// call, you must continue sending this periodically forever or the drone will
    /// automatically emergency stop. Use only if you need automatic failsafe behavior.
    pub async fn send_emergency_stop_watchdog(&self) -> Result<()> {
        let mut payload = Vec::with_capacity(1);
        payload.push(EMERGENCY_STOP_WATCHDOG);
        let pk = Packet::new(LOCALIZATION_PORT, GENERIC_CHANNEL, payload);
        self.uplink.send_async(pk).await.map_err(|_| Error::Disconnected)?;
        Ok(())
    }
}