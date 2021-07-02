
use flume::Sender;
use crazyflie_link::Packet;

use crate::{Result, Error};

const RPYT_PORT:u8 = 3;
const GENERIC_PORT:u8 = 7;

const RPYT_CHANNEL:u8 = 0;

const GENERIC_SETPOINT_CHANNEL:u8 = 0;
const GENERIC_CMD_CHANNEL:u8 = 1;

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
    /// When not modified by [parameters](crate::param::Param), the meaning of the argument are:
    ///  - **Roll/Pitch** are in degree and represent the absolute angle
    ///  - **Yaw** is in degree per seconds and represents the rotation rate
    ///  - **Thrust** is a 16 bit value where 0 maps to 0% thrust and 65535 to 100% thrust
    ///
    /// The thrust is blocked by defaut. The setpoint needs to be set once with thrust = 0 to unlock
    /// the thrust for example:
    /// ``` no_run
    /// # fn spin(cf: crate::Crazyflie) {
    /// cf.commander.setpoint_rpyt(0, 0, 0, 0);      // Unlocks the thrust
    /// cf.commander.setpoint_rpyt(0, 0, 0, 1000);   // Thrust set to 1000
    /// # }
    /// ```
    pub async fn setpoint_rpyt(&self, roll: f32, pitch: f32, yaw: f32, thrust: u16) -> Result<()>{
        let mut payload = Vec::new();
        payload.append(&mut roll.to_le_bytes().to_vec());
        payload.append(&mut pitch.to_le_bytes().to_vec());
        payload.append(&mut yaw.to_le_bytes().to_vec());
        payload.append(&mut thrust.to_le_bytes().to_vec());

        let pk = Packet::new(RPYT_PORT, RPYT_CHANNEL, payload);

        self.uplink.send_async(pk).await.map_err(|_| Error::Disconnected)?;

        Ok(())
    }
}

/// # Generic setpoints
///
/// These setpoints are implemented in such a way that they are easy to add in the Crazyflie firmware
/// and in libs like this one. So if you have a use-case not covered by any of the existing setpoint
/// do not hesitate to implement and contribute your dream setpoint :-).
impl Commander {
}