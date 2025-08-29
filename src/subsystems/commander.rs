//! # Low level setpoint subsystem
//!
//! This subsystem allows to send low-level setpoint. The setpoints are described as low-level in the sense that they
//! are setting the instant target state. As such they likely need to be send very often to have the crazyflie
//! follow the wanted flight profile.
//!
//! The Crazyflie has a couple of safety mechanisms that one needs to be aware of in order to send setpoints:
//!  - When using the [Commander::setpoint_rpyt()] function, a setpoint with thrust=0 must be sent once to unlock the thrust
//!  - There is a priority for setpoints in the Crazyflie, this allows app and other internal subsystem like the high-level
//!    commander to set setpoints in parallel, only the higher priority setpoint is taken into account.
//!  - In no setpoint are received for 1 seconds, the Crazyflie will reset roll/pitch/yawrate to 0/0/0 and after 2 seconds
//!    will fallback fallback to a lower-priority setpoint which in most case will cut the motors.
//!
//! The following example code would drive the motors in a ramp and then stop:
//! ``` no_run
//! # use tokio::time::{sleep, Duration};
//! # async fn ramp(crazyflie: crazyflie_lib::Crazyflie) -> Result<(), Box<dyn std::error::Error>> {
//! // Unlock the commander
//! crazyflie.commander.setpoint_rpyt(0.0, 0.0, 0.0, 0).await?;
//!
//! // Ramp!
//! for thrust in (0..20_000).step_by(1_000) {
//!     crazyflie.commander.setpoint_rpyt(0.0, 0.0, 0.0, thrust).await?;
//!     sleep(Duration::from_millis(100)).await;
//! }
//!
//! // Stop the motors
//! crazyflie.commander.setpoint_rpyt(0.0, 0.0, 0.0, 0).await?;
//! # Ok(())
//! # }
//! ```

use crazyflie_link::Packet;
use flume::Sender;

use crate::{Error, Result};

use crate::crazyflie::COMMANDER_PORT;
use crate::crazyflie::_GENERIC_SETPOINT_PORT;


// Channels
const RPYT_CHANNEL: u8 = 0;
const _GENERIC_SETPOINT_CHANNEL: u8 = 0;
const _GENERIC_CMD_CHANNEL: u8 = 1;

// Setpoint type identifiers
const TYPE_POSITION: u8 = 7;
const TYPE_VELOCITY_WORLD: u8 = 8;
const TYPE_ZDISTANCE: u8 = 9;
const TYPE_HOVER: u8 = 10;
const TYPE_MANUAL: u8 = 11;
const TYPE_STOP: u8 = 0;
const TYPE_META_COMMAND_NOTIFY_SETPOINT_STOP: u8 = 0;

/// # Low level setpoint subsystem
///
/// This struct implements methods to send low level setpoints to the Crazyflie.
/// See the [commander module documentation](crate::subsystems::commander) for more context and information.
#[derive(Debug)]
pub struct Commander {
    uplink: Sender<Packet>,
}

impl Commander {
    pub(crate) fn new(uplink: Sender<Packet>) -> Self {
        Self { uplink }
    }
}

/// # Legacy RPY+ setpoint
///
/// This setpoint was originally the only one present in the Crazyflie and has been (ab)used to
/// implement the early position control and other assisted and semi-autonomous mode.
impl Commander {
    /// Sends a Roll, Pitch, Yawrate, and Thrust setpoint to the Crazyflie.
    ///
    /// By default, unless modified by [parameters](crate::subsystems::param::Param), the arguments are interpreted as:
    /// * `roll` - Desired roll angle (degrees)
    /// * `pitch` - Desired pitch angle (degrees)
    /// * `yawrate` - Desired yaw rate (degrees/second)
    /// * `thrust` - Thrust as a 16-bit value (0 = 0% thrust, 65535 = 100% thrust)
    ///
    /// Note: Thrust is locked by default for safety. To unlock, send a setpoint with `thrust = 0` once before sending nonzero thrust values.
    ///
    /// Example:
    /// ```no_run
    /// # fn spin(cf: crazyflie_lib::Crazyflie) {
    /// cf.commander.setpoint_rpyt(0.0, 0.0, 0.0, 0);      // Unlocks thrust
    /// cf.commander.setpoint_rpyt(0.0, 0.0, 0.0, 1000);   // Sets thrust to 1000
    /// # }
    /// ```
    pub async fn setpoint_rpyt(&self, roll: f32, pitch: f32, yawrate: f32, thrust: u16) -> Result<()> {
        let mut payload = Vec::new();
        payload.append(&mut roll.to_le_bytes().to_vec());
        payload.append(&mut (-pitch).to_le_bytes().to_vec());  // TODO: pitch is negated in crazyflie-lib-python, confirm whether this is required.
        payload.append(&mut yawrate.to_le_bytes().to_vec());
        payload.append(&mut thrust.to_le_bytes().to_vec());

        let pk = Packet::new(COMMANDER_PORT, RPYT_CHANNEL, payload);

        self.uplink
            .send_async(pk)
            .await
            .map_err(|_| Error::Disconnected)?;

        Ok(())
    }
}

/// # Generic setpoints
///
/// These setpoints are implemented in such a way that they are easy to add in the Crazyflie firmware
/// and in libs like this one. So if you have a use-case not covered by any of the existing setpoint
/// do not hesitate to implement and contribute your dream setpoint :-).
impl Commander {
    /// Sends an absolute position setpoint in world coordinates, with yaw as an absolute orientation.
    ///
    /// # Arguments
    /// * `x` - Target x position (meters, world frame)
    /// * `y` - Target y position (meters, world frame)
    /// * `z` - Target z position (meters, world frame)
    /// * `yaw` - Target yaw angle (degrees, absolute)
    pub async fn setpoint_position(&self, x: f32, y: f32, z: f32, yaw: f32) -> Result<()> {
        let mut payload = Vec::with_capacity(1 + 4 * 4);
        payload.push(TYPE_POSITION);
        payload.extend_from_slice(&x.to_le_bytes());
        payload.extend_from_slice(&y.to_le_bytes());
        payload.extend_from_slice(&z.to_le_bytes());
        payload.extend_from_slice(&yaw.to_le_bytes());
        let pk = Packet::new(_GENERIC_SETPOINT_PORT, _GENERIC_SETPOINT_CHANNEL, payload);
        self.uplink.send_async(pk).await.map_err(|_| Error::Disconnected)?;
        Ok(())
    }

    /// Sends a velocity setpoint in the world frame, with yaw rate control.
    ///
    /// # Arguments
    /// * `vx` - Target velocity in x (meters/second, world frame)
    /// * `vy` - Target velocity in y (meters/second, world frame)
    /// * `vz` - Target velocity in z (meters/second, world frame)
    /// * `yawrate` - Target yaw rate (degrees/second)
    pub async fn setpoint_velocity_world(&self, vx: f32, vy: f32, vz: f32, yawrate: f32) -> Result<()> {
        let mut payload = Vec::with_capacity(1 + 4 * 4);
        payload.push(TYPE_VELOCITY_WORLD);
        payload.extend_from_slice(&vx.to_le_bytes());
        payload.extend_from_slice(&vy.to_le_bytes());
        payload.extend_from_slice(&vz.to_le_bytes());
        payload.extend_from_slice(&yawrate.to_le_bytes());
        let pk = Packet::new(_GENERIC_SETPOINT_PORT, _GENERIC_SETPOINT_CHANNEL, payload);
        self.uplink.send_async(pk).await.map_err(|_| Error::Disconnected)?;
        Ok(())
    }

    /// Sends a setpoint with absolute height (distance to the surface below), roll, pitch, and yaw rate commands.
    ///
    /// # Arguments
    /// * `roll` - Desired roll angle (degrees)
    /// * `pitch` - Desired pitch angle (degrees)
    /// * `yawrate` - Desired yaw rate (degrees/second)
    /// * `zdistance` - Target height above ground (meters)
    pub async fn setpoint_zdistance(&self, roll: f32, pitch: f32, yawrate: f32, zdistance: f32) -> Result<()> {
        let mut payload = Vec::with_capacity(1 + 4 * 4);
        payload.push(TYPE_ZDISTANCE);
        payload.extend_from_slice(&roll.to_le_bytes());
        payload.extend_from_slice(&pitch.to_le_bytes());
        payload.extend_from_slice(&yawrate.to_le_bytes());
        payload.extend_from_slice(&zdistance.to_le_bytes());
        let pk = Packet::new(_GENERIC_SETPOINT_PORT, _GENERIC_SETPOINT_CHANNEL, payload);
        self.uplink.send_async(pk).await.map_err(|_| Error::Disconnected)?;
        Ok(())
    }

    /// Sends a setpoint with absolute height (distance to the surface below), and x/y velocity commands in the body-fixed frame.
    ///
    /// # Arguments
    /// * `vx` - Target velocity in x (meters/second, body frame)
    /// * `vy` - Target velocity in y (meters/second, body frame)
    /// * `yawrate` - Target yaw rate (degrees/second)
    /// * `zdistance` - Target height above ground (meters)
    pub async fn setpoint_hover(&self, vx: f32, vy: f32, yawrate: f32, zdistance: f32) -> Result<()> {
        let mut payload = Vec::with_capacity(1 + 4 * 4);
        payload.push(TYPE_HOVER);
        payload.extend_from_slice(&vx.to_le_bytes());
        payload.extend_from_slice(&vy.to_le_bytes());
        payload.extend_from_slice(&yawrate.to_le_bytes());
        payload.extend_from_slice(&zdistance.to_le_bytes());
        let pk = Packet::new(_GENERIC_SETPOINT_PORT, _GENERIC_SETPOINT_CHANNEL, payload);
        self.uplink.send_async(pk).await.map_err(|_| Error::Disconnected)?;
        Ok(())
    }

    /// Sends a manual control setpoint for roll, pitch, yaw rate, and thrust percentage.
    ///
    /// If `rate` is false, roll and pitch are interpreted as angles (degrees). If `rate` is true, they are interpreted as rates (degrees/second).
    ///
    /// # Arguments
    /// * `roll` - Desired roll (degrees or degrees/second, depending on `rate`)
    /// * `pitch` - Desired pitch (degrees or degrees/second, depending on `rate`)
    /// * `yawrate` - Desired yaw rate (degrees/second)
    /// * `thrust_percentage` - Thrust as a percentage (0 to 100)
    /// * `rate` - If true, use rate mode; if false, use angle mode
    pub async fn setpoint_manual(&self, roll: f32, pitch: f32, yawrate: f32, thrust_percentage: f32, rate: bool) -> Result<()> {
        // Map thrust percentage to Crazyflie thrust value (10001 to 60000)
        let thrust = 10001.0 + 0.01 * thrust_percentage * (60000.0 - 10001.0);
        let thrust_16 = thrust as u16;
        let mut payload = Vec::with_capacity(1 + 4 * 3 + 2 + 1);
        payload.push(TYPE_MANUAL);
        payload.extend_from_slice(&roll.to_le_bytes());
        payload.extend_from_slice(&pitch.to_le_bytes());
        payload.extend_from_slice(&yawrate.to_le_bytes());
        payload.extend_from_slice(&thrust_16.to_le_bytes());
        payload.push(rate as u8);
        let pk = Packet::new(_GENERIC_SETPOINT_PORT, _GENERIC_SETPOINT_CHANNEL, payload);
        self.uplink.send_async(pk).await.map_err(|_| Error::Disconnected)?;
        Ok(())
    }

    /// Sends a STOP setpoint, immediately stopping the motors. The Crazyflie will lose lift and may fall.
    pub async fn setpoint_stop(&self) -> Result<()> {
        let payload = vec![TYPE_STOP];
        let pk = Packet::new(_GENERIC_SETPOINT_PORT, _GENERIC_SETPOINT_CHANNEL, payload);
        self.uplink.send_async(pk).await.map_err(|_| Error::Disconnected)?;
        Ok(())
    }

    /// Lowers the priority of the current setpoint, allowing any new setpoint (from any source) to overwrite it.
    ///
    /// # Arguments
    /// * `remain_valid_milliseconds` - Duration (milliseconds) for which the setpoint remains valid (usually 0)
    pub async fn notify_setpoint_stop(&self, remain_valid_milliseconds: u32) -> Result<()> {
        let mut payload = Vec::with_capacity(1 + 4);
        payload.push(TYPE_META_COMMAND_NOTIFY_SETPOINT_STOP);
        payload.extend_from_slice(&remain_valid_milliseconds.to_le_bytes());
        let pk = Packet::new(_GENERIC_SETPOINT_PORT, _GENERIC_CMD_CHANNEL, payload);
        self.uplink.send_async(pk).await.map_err(|_| Error::Disconnected)?;
        Ok(())
    }
}
