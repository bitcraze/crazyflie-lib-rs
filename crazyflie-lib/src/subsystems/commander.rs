//! # Low level setpoint subsystem
//! 
//! This subsytem allows to send low-level setpoints. The setpoints are described as low-level in the sense that they
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
//! # use std::time::Duration;
//! # async fn ramp(crazyflie: crazyflie_lib::Crazyflie) -> Result<(), Box<dyn std::error::Error>> {
//! // Unlock the commander
//! crazyflie.commander.setpoint_rpyt(0.0, 0.0, 0.0, 0).await?;
//! 
//! // Ramp!
//! for thrust in (0..20_000).step_by(1_000) {
//!     crazyflie.commander.setpoint_rpyt(0.0, 0.0, 0.0, thrust).await?;
//!     async_std::task::sleep(Duration::from_millis(100)).await;
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

const RPYT_PORT: u8 = 3;
const _GENERIC_PORT: u8 = 7;

const RPYT_CHANNEL: u8 = 0;

const _GENERIC_SETPOINT_CHANNEL: u8 = 0;
const _GENERIC_CMD_CHANNEL: u8 = 1;

/// # Low level setpoint subsystem
/// 
/// This struct implements methods to send low level setpoints to the Crazyflie.
/// See the [commander module documentation](crate::subsystems::commander) for more context and information.
#[derive(Debug)]
pub struct Commander {
    uplink: Sender<Packet>,
}

impl Commander {
    pub(crate) fn new(uplink: Sender<Packet>) -> Commander {
        Commander { uplink }
    }
}

/// # Legay RPY+ setpoint
///
/// This setpoint was originaly the only one present in the Crazyflie and has been (ab)used to
/// implement the early position control and other assisted and semi-autonomous mode.
impl Commander {
    /// Set the Roll Pitch Yaw Thrust setpoint
    ///
    /// When not modified by [parameters](crate::subsystems::param::Param), the meaning of the argument are:
    ///  - **Roll/Pitch** are in degree and represent the absolute angle
    ///  - **Yaw** is in degree per seconds and represents the rotation rate
    ///  - **Thrust** is a 16 bit value where 0 maps to 0% thrust and 65535 to 100% thrust
    ///
    /// The thrust is blocked by defaut. The setpoint needs to be set once with thrust = 0 to unlock
    /// the thrust for example:
    /// ``` no_run
    /// # fn spin(cf: crazyflie_lib::Crazyflie) {
    /// cf.commander.setpoint_rpyt(0.0, 0.0, 0.0, 0);      // Unlocks the thrust
    /// cf.commander.setpoint_rpyt(0.0, 0.0, 0.0, 1000);   // Thrust set to 1000
    /// # }
    /// ```
    pub async fn setpoint_rpyt(&self, roll: f32, pitch: f32, yaw: f32, thrust: u16) -> Result<()> {
        let mut payload = Vec::new();
        payload.append(&mut roll.to_le_bytes().to_vec());
        payload.append(&mut pitch.to_le_bytes().to_vec());
        payload.append(&mut yaw.to_le_bytes().to_vec());
        payload.append(&mut thrust.to_le_bytes().to_vec());

        let pk = Packet::new(RPYT_PORT, RPYT_CHANNEL, payload);

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
impl Commander {}
