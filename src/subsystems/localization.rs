//! # Localization subsystem
//!
//! This subsystem provides access to the Crazyflie's localization services including
//! external position/pose streaming, lighthouse positioning
//! system data, and Loco Positioning System (UWB) communication.
//!
//! ## External Position and Pose
//!
//! Send position data from external tracking systems (motion capture, etc.) to the
//! Crazyflie's onboard state estimator:
//! ```no_run
//! # async fn external_pos(crazyflie: &crazyflie_lib::Crazyflie) -> crazyflie_lib::Result<()> {
//! // Send position update
//! crazyflie.localization.external_pose
//!     .send_external_position([1.0, 2.0, 0.5]).await?;
//!
//! // Or send full pose with orientation
//! crazyflie.localization.external_pose
//!     .send_external_pose([1.0, 2.0, 0.5], [0.0, 0.0, 0.0, 1.0]).await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Lighthouse Positioning
//!
//! Access lighthouse sweep angle data for position estimation and base station calibration:
//! ```no_run
//! use futures::StreamExt;
//!
//! # async fn lighthouse(crazyflie: &crazyflie_lib::Crazyflie) -> crazyflie_lib::Result<()> {
//! // Enable angle streaming
//! crazyflie.param.set("locSrv.enLhAngleStream", 1u8).await?;
//!
//! let mut angle_stream = crazyflie.localization.lighthouse.angle_stream().await;
//! while let Some(data) = angle_stream.next().await {
//!     println!("Base station {}: x={:?}, y={:?}",
//!         data.base_station, data.angles.x, data.angles.y);
//! }
//! # Ok(())
//! # }
//! ```
//!
//! ## Loco Positioning System
//!
//! Send Loco Positioning Protocol (LPP) packets to ultra-wide-band positioning nodes:
//! ```no_run
//! # async fn loco_pos(crazyflie: &crazyflie_lib::Crazyflie) -> crazyflie_lib::Result<()> {
//! // Send LPP packet to node 5
//! let lpp_data = vec![0x01, 0x02, 0x03];
//! crazyflie.localization.loco_positioning
//!     .send_short_lpp_packet(5, &lpp_data).await?;
//! # Ok(())
//! # }
//! ```

use crazyflie_link::Packet;
use flume::{Receiver, Sender};
use async_broadcast::{broadcast, Receiver as BroadcastReceiver};
use futures::Stream;
use half::f16;

use crate::{Error, Result};

use crate::crazyflie::{LOCALIZATION_PORT, SUPERVISOR_PORT};
use crate::subsystems::supervisor::{
    CMD_EMERGENCY_STOP, CMD_EMERGENCY_STOP_WATCHDOG, SUPERVISOR_CH_COMMAND,
};

// Channels
const POSITION_CHANNEL: u8 = 0;
const GENERIC_CHANNEL: u8 = 1;

// Generic channel message types
const _RANGE_STREAM_REPORT: u8 = 0;
const _RANGE_STREAM_REPORT_FP16: u8 = 1;
const LPS_SHORT_LPP_PACKET: u8 = 2;
const _EMERGENCY_STOP: u8 = 3;
const _EMERGENCY_STOP_WATCHDOG: u8 = 4;
const _COMM_GNSS_NMEA: u8 = 6;
const _COMM_GNSS_PROPRIETARY: u8 = 7;
const EXT_POSE: u8 = 8;
const _EXT_POSE_PACKED: u8 = 9;
const LH_ANGLE_STREAM: u8 = 10;
const LH_PERSIST_DATA: u8 = 11;
const LH_MATCHED_ANGLE_STREAM: u8 = 12;

/// Lighthouse angle sweep data
#[derive(Debug, Clone)]
pub struct LighthouseAngleData {
    /// Base station ID
    pub base_station: u8,
    /// Angle measurements
    pub angles: LighthouseAngles,
}

/// Lighthouse sweep angle data from the matched angle stream
///
/// The Crazyflie sends one packet per base station. Packets that were sampled
/// at the same time share a `group_id`, and `bs_count` tells how many packets
/// belong to the group.
#[derive(Debug, Clone)]
pub struct LighthouseMatchedAngleData {
    /// Base station ID
    pub base_station: u8,
    /// Angle measurements
    pub angles: LighthouseAngles,
    /// Group ID (0-15, wraps around)
    pub group_id: u8,
    /// Number of base stations in the group
    pub bs_count: u8,
}

/// Lighthouse sweep angles for all 4 sensors
#[derive(Debug, Clone)]
pub struct LighthouseAngles {
    /// Horizontal angles for 4 sensors (rad)
    pub x: [f32; 4],
    /// Vertical angles for 4 sensors (rad)
    pub y: [f32; 4],
}

/// Localization subsystem
///
/// Provides access to localization services including emergency stop,
/// external position/pose streaming, and lighthouse positioning system.
pub struct Localization{
    /// Emergency stop controls
    pub emergency: EmergencyControl,
    /// External position and pose streaming
    pub external_pose: ExternalPose,
    /// Lighthouse positioning system
    pub lighthouse: Lighthouse,
    /// Loco Positioning System (UWB)
    pub loco_positioning: LocoPositioning,
}

impl Localization {
    pub(crate) fn new(uplink: Sender<Packet>, downlink: Receiver<Packet>) -> Self {
        let emergency = EmergencyControl { uplink: uplink.clone() };
        let external_pose = ExternalPose { uplink: uplink.clone() };

        let (mut angle_broadcast, angle_receiver) = broadcast(100);
        let (mut matched_angle_broadcast, matched_angle_receiver) = broadcast(100);
        let (mut persist_broadcast, persist_receiver) = broadcast(10);

        // Enable overflow mode so old messages are dropped instead of blocking
        angle_broadcast.set_overflow(true);
        matched_angle_broadcast.set_overflow(true);
        persist_broadcast.set_overflow(true);

        // Spawn background task to process incoming localization packets
        tokio::spawn(async move {
            while let Ok(pk) = downlink.recv_async().await {
                if pk.get_channel() != GENERIC_CHANNEL || pk.get_data().is_empty() {
                    continue;
                }

                let packet_type = pk.get_data()[0];
                let data = &pk.get_data()[1..];

                match packet_type {
                    LH_ANGLE_STREAM => {
                        if let Ok(angle_data) = decode_lh_angle(data) {
                            let _ = angle_broadcast.broadcast(angle_data).await;
                        }
                    }
                    LH_MATCHED_ANGLE_STREAM => {
                        if let Ok(angle_data) = decode_matched_lh_angle(data) {
                            let _ = matched_angle_broadcast.broadcast(angle_data).await;
                        }
                    }
                    LH_PERSIST_DATA if !data.is_empty() => {
                        let success = data[0] != 0;
                        let _ = persist_broadcast.broadcast(success).await;
                    }
                    _ => {} // Ignore unknown packet types
                }
            }
        });

        let lighthouse = Lighthouse {
            uplink: uplink.clone(),
            angle_stream_receiver: angle_receiver,
            matched_angle_stream_receiver: matched_angle_receiver,
            persist_receiver,
        };

        let loco_positioning = LocoPositioning { uplink: uplink.clone() };

        Self { emergency, external_pose, lighthouse, loco_positioning }
    }
}

/// Decode lighthouse angle stream packet
///
/// Packet format (from Python): '<Bfhhhfhhh'
/// - B: base station ID
/// - f: x[0] angle (float32)
/// - h: x[1] diff (int16 fp16)
/// - h: x[2] diff (int16 fp16)
/// - h: x[3] diff (int16 fp16)
/// - f: y[0] angle (float32)
/// - h: y[1] diff (int16 fp16)
/// - h: y[2] diff (int16 fp16)
/// - h: y[3] diff (int16 fp16)
fn decode_lh_angle(data: &[u8]) -> Result<LighthouseAngleData> {
    if data.len() < 21 {
        return Err(Error::ProtocolError("LH_ANGLE_STREAM packet too short".to_owned()));
    }

    Ok(LighthouseAngleData {
        base_station: data[0],
        angles: decode_lh_angles(&data[1..21]),
    })
}

/// Decode lighthouse matched angle stream packet
///
/// Packet format: '<BfhhhfhhhB'
/// - Same as [decode_lh_angle], followed by
/// - B: group ID (upper 4 bits) and base station count (lower 4 bits)
fn decode_matched_lh_angle(data: &[u8]) -> Result<LighthouseMatchedAngleData> {
    if data.len() < 22 {
        return Err(Error::ProtocolError("LH_MATCHED_ANGLE_STREAM packet too short".to_owned()));
    }

    Ok(LighthouseMatchedAngleData {
        base_station: data[0],
        angles: decode_lh_angles(&data[1..21]),
        group_id: data[21] >> 4,
        bs_count: data[21] & 0x0F,
    })
}

/// Decode the 20 bytes of sweep angles shared by the angle stream packets
fn decode_lh_angles(data: &[u8]) -> LighthouseAngles {
    // Each sweep is the first sensor's angle as f32, followed by the
    // differences to the other 3 sensors as int16 fp16
    let decode_sweep = |d: &[u8]| -> [f32; 4] {
        let first = f32::from_le_bytes([d[0], d[1], d[2], d[3]]);
        let other = |offset: usize| {
            first - f16::from_bits(u16::from_le_bytes([d[offset], d[offset + 1]])).to_f32()
        };
        [first, other(4), other(6), other(8)]
    };

    LighthouseAngles {
        x: decode_sweep(&data[0..10]),
        y: decode_sweep(&data[10..20]),
    }
}

/// Emergency control interface
///
/// Provides emergency stop functionality that immediately stops all motors.
pub struct EmergencyControl {
    uplink: Sender<Packet>,
}

impl EmergencyControl {
    /// Send emergency stop command
    ///
    /// Immediately stops all motors and puts the Crazyflie into a locked state.
    /// The drone will require a reboot before it can fly again.
    #[deprecated(since = "0.8.1", note = "Use [`Supervisor::send_emergency_stop`](crate::subsystems::supervisor::Supervisor::send_emergency_stop) instead")]
    pub async fn send_emergency_stop(&self) -> Result<()> {
        // Route to supervisor port for compatibility
        let pk = Packet::new(SUPERVISOR_PORT, SUPERVISOR_CH_COMMAND, vec![CMD_EMERGENCY_STOP]);
        self.uplink.send_async(pk).await.map_err(|_| Error::Disconnected)?;
        Ok(())
    }

    /// Send emergency stop watchdog
    ///
    /// Activates/resets a watchdog failsafe that will automatically emergency stop
    /// the drone if this message isn't sent every 1000ms. Once activated by the first
    /// call, you must continue sending this periodically forever or the drone will
    /// automatically emergency stop. Use only if you need automatic failsafe behavior.
    #[deprecated(since = "0.8.1", note = "Use [`Supervisor::send_emergency_stop_watchdog`](crate::subsystems::supervisor::Supervisor::send_emergency_stop_watchdog) instead")]
    pub async fn send_emergency_stop_watchdog(&self) -> Result<()> {
        // Route to supervisor port for compatibility
        let pk = Packet::new(
            SUPERVISOR_PORT,
            SUPERVISOR_CH_COMMAND,
            vec![CMD_EMERGENCY_STOP_WATCHDOG],
        );
        self.uplink.send_async(pk).await.map_err(|_| Error::Disconnected)?;
        Ok(())
    }
}

/// External pose interface
///
/// Provides functionality to send external position and pose data from motion 
/// capture systems or other external tracking sources to the Crazyflie's 
/// onboard state estimator.
pub struct ExternalPose {
    uplink: Sender<Packet>,
}

impl ExternalPose {
    /// Send external position (x, y, z) to the Crazyflie
    ///
    /// Updates the Crazyflie's position estimate with 3D position data.
    ///
    /// # Arguments
    /// * `pos` - Position array [x, y, z] in meters
    pub async fn send_external_position(&self, pos: [f32; 3]) -> Result<()> {
        let mut payload = Vec::with_capacity(3 * 4);
        payload.extend_from_slice(&pos[0].to_le_bytes());
        payload.extend_from_slice(&pos[1].to_le_bytes());
        payload.extend_from_slice(&pos[2].to_le_bytes());

        let pk = Packet::new(LOCALIZATION_PORT, POSITION_CHANNEL, payload);
        self.uplink.send_async(pk).await.map_err(|_| Error::Disconnected)?;
        Ok(())
    }

    /// Send external pose (position + quaternion) to the Crazyflie
    ///
    /// Updates the Crazyflie's position estimate with full 6DOF pose data.
    /// Includes both position and orientation.
    ///
    /// # Arguments
    /// * `pos` - Position array [x, y, z] in meters
    /// * `quat` - Quaternion array [qx, qy, qz, qw]
    pub async fn send_external_pose(&self, pos: [f32; 3], quat: [f32; 4]) -> Result<()> {
        let mut payload = Vec::with_capacity(1 + 7 * 4);
        payload.push(EXT_POSE);
        payload.extend_from_slice(&pos[0].to_le_bytes());
        payload.extend_from_slice(&pos[1].to_le_bytes());
        payload.extend_from_slice(&pos[2].to_le_bytes());
        payload.extend_from_slice(&quat[0].to_le_bytes());
        payload.extend_from_slice(&quat[1].to_le_bytes());
        payload.extend_from_slice(&quat[2].to_le_bytes());
        payload.extend_from_slice(&quat[3].to_le_bytes());

        let pk = Packet::new(LOCALIZATION_PORT, GENERIC_CHANNEL, payload);
        self.uplink.send_async(pk).await.map_err(|_| Error::Disconnected)?;
        Ok(())
    }
}

/// Loco Positioning System (UWB) interface
///
/// Provides functionality to send Loco Positioning Protocol (LPP) packets
/// to ultra-wide-band positioning nodes.
pub struct LocoPositioning {
    uplink: Sender<Packet>,
}

impl LocoPositioning {
    /// Send Loco Positioning Protocol (LPP) packet to a specific destination
    ///
    /// # Arguments
    /// * `dest_id` - Destination node ID
    /// * `data` - LPP packet payload
    pub async fn send_short_lpp_packet(&self, dest_id: u8, data: &[u8]) -> Result<()> {
        let mut payload = Vec::with_capacity(2 + data.len());
        payload.push(LPS_SHORT_LPP_PACKET);
        payload.push(dest_id);
        payload.extend_from_slice(data);

        let pk = Packet::new(LOCALIZATION_PORT, GENERIC_CHANNEL, payload);
        self.uplink.send_async(pk).await.map_err(|_| Error::Disconnected)?;
        Ok(())
    }
}

/// Lighthouse positioning system interface
///
/// Provides functionality to receive lighthouse sweep angle data and manage
/// lighthouse base station configuration persistence.
pub struct Lighthouse {
    uplink: Sender<Packet>,
    angle_stream_receiver: BroadcastReceiver<LighthouseAngleData>,
    matched_angle_stream_receiver: BroadcastReceiver<LighthouseMatchedAngleData>,
    persist_receiver: BroadcastReceiver<bool>,
}

impl Lighthouse {
    /// Get a stream of lighthouse angle measurements
    ///
    /// Returns a Stream that yields [LighthouseAngleData] whenever lighthouse
    /// sweep angle data is received from the Crazyflie. This is typically used
    /// for lighthouse base station calibration and geometry estimation.
    ///
    /// To enable the angle stream, set the parameter `locSrv.enLhAngleStream` to 1
    /// on the Crazyflie.
    ///
    /// # Example
    /// ```no_run
    /// # use crazyflie_lib::Crazyflie;
    /// # use futures::StreamExt;
    /// # async fn example(crazyflie: &Crazyflie) -> Result<(), Box<dyn std::error::Error>> {
    /// // Enable angle streaming
    /// crazyflie.param.set("locSrv.enLhAngleStream", 1u8).await?;
    ///
    /// let mut angle_stream = crazyflie.localization.lighthouse.angle_stream().await;
    /// while let Some(data) = angle_stream.next().await {
    ///     println!("Base station {}: x={:?}, y={:?}",
    ///         data.base_station, data.angles.x, data.angles.y);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn angle_stream(&self) -> impl Stream<Item = LighthouseAngleData> + use<> {
        self.angle_stream_receiver.clone()
    }

    /// Get a stream of matched lighthouse angle measurements
    ///
    /// Returns a Stream that yields [LighthouseMatchedAngleData] whenever matched
    /// lighthouse sweep angle data is received from the Crazyflie. Unlike
    /// [angle_stream](Self::angle_stream), packets are sent in groups of samples
    /// from several base stations taken at about the same time, which is what
    /// geometry estimation needs.
    ///
    /// The stream is controlled by these parameters on the Crazyflie:
    /// - `locSrv.enLhMtchStm`: number of groups to send (0 = off, 255 = continuous)
    /// - `locSrv.minBsLhMtchStm`: minimum number of base stations in a group
    /// - `locSrv.maxTimeLhMtchStm`: maximum time span of a group (ms)
    ///
    /// # Example
    /// ```no_run
    /// # use crazyflie_lib::Crazyflie;
    /// # use futures::StreamExt;
    /// # async fn example(crazyflie: &Crazyflie) -> Result<(), Box<dyn std::error::Error>> {
    /// let mut angle_stream = crazyflie.localization.lighthouse.matched_angle_stream().await;
    ///
    /// // Send groups with at least 2 base stations continuously
    /// crazyflie.param.set("locSrv.minBsLhMtchStm", 2u8).await?;
    /// crazyflie.param.set("locSrv.enLhMtchStm", 255u8).await?;
    ///
    /// while let Some(data) = angle_stream.next().await {
    ///     println!("Group {} ({} base stations), base station {}: x={:?}, y={:?}",
    ///         data.group_id, data.bs_count, data.base_station, data.angles.x, data.angles.y);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn matched_angle_stream(&self) -> impl Stream<Item = LighthouseMatchedAngleData> + use<> {
        self.matched_angle_stream_receiver.clone()
    }

    /// Persist lighthouse geometry and calibration data to permanent storage
    ///
    /// Sends a command to persist lighthouse geometry and/or calibration data
    /// to permanent storage in the Crazyflie, then waits for confirmation.
    /// The geometry and calibration data must have been previously written to
    /// RAM via the memory subsystem.
    ///
    /// # Arguments
    /// * `geo_list` - List of base station IDs (0-15) for which to persist geometry data
    /// * `calib_list` - List of base station IDs (0-15) for which to persist calibration data
    ///
    /// # Returns
    /// * `Ok(true)` if data was successfully persisted
    /// * `Ok(false)` if persistence failed
    /// * `Err` if there was a communication error or timeout (5 seconds)
    ///
    /// # Example
    /// ```no_run
    /// # use crazyflie_lib::Crazyflie;
    /// # async fn example(crazyflie: &Crazyflie) -> crazyflie_lib::Result<()> {
    /// // Persist geometry for base stations 0 and 1, calibration for base station 0
    /// let success = crazyflie.localization.lighthouse
    ///     .persist_lighthouse_data(&[0, 1], &[0]).await?;
    ///
    /// if success {
    ///     println!("Data persisted successfully");
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn persist_lighthouse_data(&self, geo_list: &[u8], calib_list: &[u8]) -> Result<bool> {
        self.send_lh_persist_data_packet(geo_list, calib_list).await?;
        self.wait_persist_confirmation().await
    }

    /// Wait for lighthouse persistence confirmation
    ///
    /// After sending geometry or calibration data to be persisted (via
    /// send_lh_persist_data_packet), this function waits for and returns
    /// the confirmation from the Crazyflie.
    ///
    /// Returns `Ok(true)` if data was successfully persisted, `Ok(false)` if
    /// persistence failed, or an error if no confirmation is received within
    /// the timeout.
    async fn wait_persist_confirmation(&self) -> Result<bool> {
        let mut receiver = self.persist_receiver.clone();
        match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            receiver.recv()
        ).await {
            Ok(Ok(success)) => Ok(success),
            Ok(Err(_)) => Err(Error::Disconnected),
            Err(_) => Err(Error::Timeout),
        }
    }

    /// Send lighthouse persist data packet
    ///
    /// Sends a command to persist lighthouse geometry and/or calibration data
    /// to permanent storage in the Crazyflie. The geometry and calibration data
    /// must have been previously written to RAM via the memory subsystem.
    ///
    /// # Arguments
    /// * `geo_list` - List of base station IDs (0-15) for which to persist geometry data
    /// * `calib_list` - List of base station IDs (0-15) for which to persist calibration data
    ///
    /// Use [wait_persist_confirmation] to wait for the result.
   async fn send_lh_persist_data_packet(&self, geo_list: &[u8], calib_list: &[u8]) -> Result<()> {
        // Validate base station IDs
        const MAX_BS_NR: u8 = 15;
        for &bs in geo_list {
            if bs > MAX_BS_NR {
                return Err(Error::ProtocolError(format!(
                    "Invalid geometry base station ID: {} (max: {})", bs, MAX_BS_NR
                )));
            }
        }
        for &bs in calib_list {
            if bs > MAX_BS_NR {
                return Err(Error::ProtocolError(format!(
                    "Invalid calibration base station ID: {} (max: {})", bs, MAX_BS_NR
                )));
            }
        }

        // Build bitmasks
        let mut mask_geo: u16 = 0;
        let mut mask_calib: u16 = 0;

        for &bs in geo_list {
            mask_geo |= 1 << bs;
        }
        for &bs in calib_list {
            mask_calib |= 1 << bs;
        }

        // Build packet
        let mut payload = Vec::with_capacity(5);
        payload.push(LH_PERSIST_DATA);
        payload.extend_from_slice(&mask_geo.to_le_bytes());
        payload.extend_from_slice(&mask_calib.to_le_bytes());

        let pk = Packet::new(LOCALIZATION_PORT, GENERIC_CHANNEL, payload);
        self.uplink.send_async(pk).await.map_err(|_| Error::Disconnected)?;
        Ok(())
    }
}