//! # Supervisor subsystem
//!
//! The supervisor monitors the Crazyflie's system state and safety conditions. It manages:
//! - System readiness (can be armed, is armed, can fly)
//! - Flight state (is flying, is tumbled)
//! - Safety state (is crashed, is locked)
//! - High level commander state
//!
//! ## Reading System State
//!
//! Query the current system state using the supervisor info bitfield:
//! ``` no_run
//! # async fn read_state(crazyflie: &crazyflie_lib::Crazyflie) -> crazyflie_lib::Result<()> {
//! // Read the supervisor bitfield
//! let info = crazyflie.supervisor.read_bitfield().await?;
//! 
//! // Check specific state flags
//! if info.can_be_armed() {
//!     println!("System can be armed");
//! }
//! if info.can_fly() {
//!     println!("System is ready to fly");
//! }
//! if info.is_flying() {
//!     println!("System is flying");
//! }
//! # Ok(())
//! # }
//! ```
//!
//! ## Sending System Commands
//!
//! The supervisor also allows sending arming and crash recovery commands:
//! ``` no_run
//! # async fn send_commands(crazyflie: &crazyflie_lib::Crazyflie) -> crazyflie_lib::Result<()> {
//! // Arm the system
//! crazyflie.supervisor.send_arming_request(true).await?;
//!
//! // Disarm the system
//! crazyflie.supervisor.send_arming_request(false).await?;
//!
//! // Request crash recovery
//! crazyflie.supervisor.send_crash_recovery_request().await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Available State Properties
//!
//! - `can_be_armed()` - System can be armed and will accept an arming command
//! - `is_armed()` - System is currently armed
//! - `is_auto_armed()` - System is configured to automatically arm
//! - `can_fly()` - System is ready to fly
//! - `is_flying()` - System is actively flying
//! - `is_tumbled()` - System is upside down
//! - `is_locked()` - System is in locked state and must be restarted
//! - `is_crashed()` - System has crashed
//! - `hl_control_active()` - High level commander is actively flying the drone
//! - `hl_traj_finished()` - High level commander trajectory has finished
//! - `hl_control_disabled()` - High level commander is disabled

use crate::crtp_utils::crtp_channel_dispatcher;
use crate::{Error, Result};
use crate::crazyflie::SUPERVISOR_PORT;
use crazyflie_link::Packet;
use flume::{Receiver, Sender};
use futures::lock::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::time::{timeout, Duration};

// Channels
const SUPERVISOR_CH_INFO: u8 = 0;
const SUPERVISOR_CH_COMMAND: u8 = 1;

// Commands
const CMD_GET_STATE_BITFIELD: u8 = 0x0C;
const CMD_ARM_SYSTEM: u8 = 0x01;
const CMD_RECOVER_SYSTEM: u8 = 0x02;

// Bit positions
const BIT_CAN_BE_ARMED: u8 = 0;
const BIT_IS_ARMED: u8 = 1;
const BIT_IS_AUTO_ARMED: u8 = 2;
const BIT_CAN_FLY: u8 = 3;
const BIT_IS_FLYING: u8 = 4;
const BIT_IS_TUMBLED: u8 = 5;
const BIT_IS_LOCKED: u8 = 6;
const BIT_IS_CRASHED: u8 = 7;
const BIT_HL_CONTROL_ACTIVE: u8 = 8;
const BIT_HL_TRAJ_FINISHED: u8 = 9;
const BIT_HL_CONTROL_DISABLED: u8 = 10;

/// Supervisor info bitfield
///
/// Contains the decoded state of the supervisor system. Use the various
/// methods to query specific state flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupervisorInfo {
    /// Raw bitfield value
    pub raw: u16,
}

impl SupervisorInfo {
    /// Create from raw bitfield value
    pub fn from_bits(bits: u16) -> Self {
        Self { raw: bits }
    }

    /// System can be armed - the system can be armed and will accept an arming command
    pub fn can_be_armed(&self) -> bool {
        (self.raw >> BIT_CAN_BE_ARMED) & 0x01 != 0
    }

    /// System is armed
    pub fn is_armed(&self) -> bool {
        (self.raw >> BIT_IS_ARMED) & 0x01 != 0
    }

    /// System is configured to automatically arm
    pub fn is_auto_armed(&self) -> bool {
        (self.raw >> BIT_IS_AUTO_ARMED) & 0x01 != 0
    }

    /// The Crazyflie is ready to fly
    pub fn can_fly(&self) -> bool {
        (self.raw >> BIT_CAN_FLY) & 0x01 != 0
    }

    /// The Crazyflie is flying
    pub fn is_flying(&self) -> bool {
        (self.raw >> BIT_IS_FLYING) & 0x01 != 0
    }

    /// The Crazyflie is tumbled (upside down)
    pub fn is_tumbled(&self) -> bool {
        (self.raw >> BIT_IS_TUMBLED) & 0x01 != 0
    }

    /// The Crazyflie is in the locked state and must be restarted
    pub fn is_locked(&self) -> bool {
        (self.raw >> BIT_IS_LOCKED) & 0x01 != 0
    }

    /// The Crazyflie has crashed
    pub fn is_crashed(&self) -> bool {
        (self.raw >> BIT_IS_CRASHED) & 0x01 != 0
    }

    /// High level commander is actively flying the drone
    pub fn hl_control_active(&self) -> bool {
        (self.raw >> BIT_HL_CONTROL_ACTIVE) & 0x01 != 0
    }

    /// High level commander trajectory has finished
    pub fn hl_traj_finished(&self) -> bool {
        (self.raw >> BIT_HL_TRAJ_FINISHED) & 0x01 != 0
    }

    /// High level commander is disabled and not producing setpoints
    pub fn hl_control_disabled(&self) -> bool {
        (self.raw >> BIT_HL_CONTROL_DISABLED) & 0x01 != 0
    }

    /// Get list of all active state names
    pub fn active_states(&self) -> Vec<&'static str> {
        let states = [
            ("Can be armed", self.can_be_armed()),
            ("Is armed", self.is_armed()),
            ("Is auto armed", self.is_auto_armed()),
            ("Can fly", self.can_fly()),
            ("Is flying", self.is_flying()),
            ("Is tumbled", self.is_tumbled()),
            ("Is locked", self.is_locked()),
            ("Is crashed", self.is_crashed()),
            ("HL control active", self.hl_control_active()),
            ("HL trajectory finished", self.hl_traj_finished()),
            ("HL control disabled", self.hl_control_disabled()),
        ];

        states
            .iter()
            .filter_map(|(name, active)| if *active { Some(*name) } else { None })
            .collect()
    }
}

/// # Access to the supervisor subsystem
///
/// The supervisor monitors system state and provides information about
/// the flight readiness and safety status of the Crazyflie.
///
/// See the [supervisor module documentation](crate::subsystems::supervisor) for more context and information.
pub struct Supervisor {
    uplink: Sender<Packet>,
    info_downlink: Mutex<Receiver<Packet>>,
    cache_timeout_ms: u64,
    last_fetch_time: std::sync::Mutex<u64>,
    cached_bitfield: std::sync::Mutex<Option<u16>>,
}

impl Supervisor {
    pub(crate) fn new(uplink: Sender<Packet>, downlink: Receiver<Packet>) -> Self {
        let (info_downlink, _cmd_downlink, _misc1, _misc2) = crtp_channel_dispatcher(downlink);
        Self {
            uplink,
            info_downlink: Mutex::new(info_downlink),
            cache_timeout_ms: 100,
            last_fetch_time: std::sync::Mutex::new(0),
            cached_bitfield: std::sync::Mutex::new(None),
        }
    }

    /// Read the supervisor bitfield
    ///
    /// Requests the current supervisor bitfield from the Crazyflie and returns it decoded.
    /// Uses time-based caching to avoid sending packages too frequently.
    ///
    /// # Example
    /// ```no_run
    /// # async fn example(crazyflie: &crazyflie_lib::Crazyflie) -> crazyflie_lib::Result<()> {
    /// let info = crazyflie.supervisor.read_bitfield().await?;
    /// println!("Can fly: {}", info.can_fly());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn read_bitfield(&self) -> Result<SupervisorInfo> {
        let now = Self::current_time_ms();
        let last_fetch = self.last_fetch_time.lock().unwrap();
        let cached = self.cached_bitfield.lock().unwrap();

        // Return cached value if it's recent enough
        if let Some(bitfield) = *cached {
            if now - *last_fetch < self.cache_timeout_ms {
                return Ok(SupervisorInfo::from_bits(bitfield));
            }
        }

        drop(last_fetch);
        drop(cached);

        // Send request
        let pk = Packet::new(
            SUPERVISOR_PORT,
            SUPERVISOR_CH_INFO,
            vec![CMD_GET_STATE_BITFIELD],
        );
        self.uplink.send_async(pk).await.map_err(|_| Error::Disconnected)?;

        let bitfield = self.wait_for_bitfield().await?;

        let mut last_fetch = self.last_fetch_time.lock().unwrap();
        let mut cached = self.cached_bitfield.lock().unwrap();
        *last_fetch = now;
        *cached = Some(bitfield);

        Ok(SupervisorInfo::from_bits(bitfield))
    }

    async fn wait_for_bitfield(&self) -> Result<u16> {
        let downlink = self.info_downlink.lock().await;
        loop {
            let packet = timeout(Duration::from_millis(1000), downlink.recv_async())
                .await
                .map_err(|_| Error::Timeout)??;

            if packet.get_port() != SUPERVISOR_PORT || packet.get_channel() != SUPERVISOR_CH_INFO {
                continue;
            }

            let data = packet.get_data();
            if data.len() < 3 {
                continue;
            }

            let cmd = data[0];
            if cmd != CMD_GET_STATE_BITFIELD && cmd != (CMD_GET_STATE_BITFIELD | 0x80) {
                continue;
            }

            let bitfield = u16::from_le_bytes([data[1], data[2]]);
            return Ok(bitfield);
        }
    }

    fn current_time_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    /// Send system arm/disarm request
    ///
    /// Arms or disarms the Crazyflie's motors. When disarmed, the motors
    /// will not spin even if thrust commands are sent.
    ///
    /// # Arguments
    /// * `do_arm` - true to arm, false to disarm
    ///
    /// # Example
    /// ```no_run
    /// # async fn example(crazyflie: &crazyflie_lib::Crazyflie) -> crazyflie_lib::Result<()> {
    /// // Arm the system
    /// crazyflie.supervisor.send_arming_request(true).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn send_arming_request(&self, do_arm: bool) -> Result<()> {
        let command = if do_arm { 1u8 } else { 0u8 };
        let pk = Packet::new(
            SUPERVISOR_PORT,
            SUPERVISOR_CH_COMMAND,
            vec![CMD_ARM_SYSTEM, command],
        );
        self.uplink.send_async(pk).await.map_err(|_| Error::Disconnected)?;
        Ok(())
    }

    /// Send crash recovery request
    ///
    /// Requests recovery from a crashed state detected by the Crazyflie.
    ///
    /// # Example
    /// ```no_run
    /// # async fn example(crazyflie: &crazyflie_lib::Crazyflie) -> crazyflie_lib::Result<()> {
    /// crazyflie.supervisor.send_crash_recovery_request().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn send_crash_recovery_request(&self) -> Result<()> {
        let pk = Packet::new(
            SUPERVISOR_PORT,
            SUPERVISOR_CH_COMMAND,
            vec![CMD_RECOVER_SYSTEM],
        );
        self.uplink.send_async(pk).await.map_err(|_| Error::Disconnected)?;
        Ok(())
    }
}
