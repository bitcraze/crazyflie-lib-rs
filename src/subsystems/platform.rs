//! # Platform services
//!
//! The platform CRTP port hosts a couple of utility services. This range from fetching the version of the firmware
//! and CRTP protocol, communication with apps using the App layer to setting the continuous wave radio mode for
//! radio testing.

use crate::{Error, Result};
use crazyflie_link::Packet;
use flume::{Receiver, Sender};
use futures::lock::Mutex;

use crate::crazyflie::PLATFORM_PORT;

const _PLATFORM_COMMAND: u8 = 0;
const VERSION_CHANNEL: u8 = 1;
const _APP_CHANNEL: u8 = 2;

const _PLATFORM_SET_CONT_WAVE: u8 = 0;

const VERSION_GET_PROTOCOL: u8 = 0;
const VERSION_GET_FIRMWARE: u8 = 1;
const VERSION_GET_DEVICE_TYPE: u8 = 2;

/// Access to platform services
///
/// See the [platform module documentation](crate::subsystems::platform) for more context and information.
pub struct Platform {
    comm: Mutex<(Sender<Packet>, Receiver<Packet>)>,
}
/// Access to the platform services
impl Platform {
    pub(crate) fn new(uplink: Sender<Packet>, downlink: Receiver<Packet>) -> Self {
        Self {
            comm: Mutex::new((uplink, downlink)),
        }
    }

    /// Fetch the protocol version from Crazyflie
    ///
    /// The protocol version is updated when new message or breaking change are
    /// implemented in the protocol.
    /// see [the crate documentation](crate#compatibility) for more information.
    ///
    /// Compatibility is checked at connection time.
    pub async fn protocol_version(&self) -> Result<u8> {
        let (uplink, downlink) = &*self.comm.lock().await;

        let pk = Packet::new(PLATFORM_PORT, VERSION_CHANNEL, vec![VERSION_GET_PROTOCOL]);
        uplink.send_async(pk).await?;

        let pk = downlink.recv_async().await?;

        if pk.get_data()[0] != VERSION_GET_PROTOCOL {
            return Err(Error::ProtocolError("Wrong version answer".to_owned()));
        }

        Ok(pk.get_data()[1])
    }

    /// Fetch the firmware version
    ///
    /// If this firmware is a stable release, the release name will be returned for example ```2021.06```.
    /// If this firmware is a git build, between releases, the number of commit since the last release will be added
    /// for example ```2021.06 +128```.
    pub async fn firmware_version(&self) -> Result<String> {
        let (uplink, downlink) = &*self.comm.lock().await;

        let pk = Packet::new(PLATFORM_PORT, VERSION_CHANNEL, vec![VERSION_GET_FIRMWARE]);
        uplink.send_async(pk).await?;

        let pk = downlink.recv_async().await?;

        if pk.get_data()[0] != VERSION_GET_FIRMWARE {
            return Err(Error::ProtocolError("Wrong version answer".to_owned()));
        }

        let version = String::from_utf8_lossy(&pk.get_data()[1..]);

        Ok(version.to_string())
    }

    /// Fetch the device type.
    ///
    /// The Crazyflie firmware can run on multiple device. This function returns the name of the device. For example
    /// ```Crazyflie 2.1``` is returned in the case of a Crazyflie 2.1.
    pub async fn device_type_name(&self) -> Result<String> {
        let (uplink, downlink) = &*self.comm.lock().await;

        let pk = Packet::new(
            PLATFORM_PORT,
            VERSION_CHANNEL,
            vec![VERSION_GET_DEVICE_TYPE],
        );
        uplink.send_async(pk).await?;

        let pk = downlink.recv_async().await?;

        if pk.get_data()[0] != VERSION_GET_DEVICE_TYPE {
            return Err(Error::ProtocolError("Wrong device type answer".to_owned()));
        }

        let version = String::from_utf8_lossy(&pk.get_data()[1..]);

        Ok(version.to_string())
    }
}
