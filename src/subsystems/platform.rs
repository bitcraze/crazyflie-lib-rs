//! # Platform services
//!
//! The platform CRTP port hosts a couple of utility services. This range from fetching the version of the firmware
//! and CRTP protocol, communication with apps using the App layer to setting the continuous wave radio mode for
//! radio testing.

use std::convert::TryFrom;

use crate::{crtp_utils::crtp_channel_dispatcher, Error, Result};
use crazyflie_link::Packet;
use flume::{Receiver, Sender};
use futures::{lock::Mutex, stream, Sink, SinkExt, Stream, StreamExt};

use crate::crazyflie::PLATFORM_PORT;

const PLATFORM_COMMAND: u8 = 0;
const VERSION_CHANNEL: u8 = 1;
const APP_CHANNEL: u8 = 2;

const PLATFORM_SET_CONT_WAVE: u8 = 0;
const PLATFORM_REQUEST_ARMING: u8 = 1;
const PLATFORM_REQUEST_CRASH_RECOVERY: u8 = 2;

const VERSION_GET_PROTOCOL: u8 = 0;
const VERSION_GET_FIRMWARE: u8 = 1;
const VERSION_GET_DEVICE_TYPE: u8 = 2;

/// Maximum packet size that can be transmitted in an app channel packet.
pub const APPCHANNEL_MTU: usize = 31;

/// Access to platform services
///
/// See the [platform module documentation](crate::subsystems::platform) for more context and information.
pub struct Platform {
    version_comm: Mutex<(Sender<Packet>, Receiver<Packet>)>,
    appchannel_comm: Mutex<Option<(Sender<Packet>, Receiver<Packet>)>>,
    uplink: Sender<Packet>,
}
/// Access to the platform services
impl Platform {
    pub(crate) fn new(uplink: Sender<Packet>, downlink: Receiver<Packet>) -> Self {
        let (_, version_downlink, appchannel_downlink, _) = crtp_channel_dispatcher(downlink);

        Self {
            version_comm: Mutex::new((uplink.clone(), version_downlink)),
            appchannel_comm: Mutex::new(Some((uplink.clone(), appchannel_downlink))),
            uplink,
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
        let (uplink, downlink) = &*self.version_comm.lock().await;

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
        let (uplink, downlink) = &*self.version_comm.lock().await;

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
        let (uplink, downlink) = &*self.version_comm.lock().await;

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

    /// Get sender and receiver to the app channel
    ///
    /// This function returns the transmit and receive channel to and from
    /// the app channel. The channel accepts and generates [AppChannelPacket]
    /// which guarantees that the packet length is correct. the From trait is
    /// implemented to all possible ```[u8; n]``` and TryFrom to Vec<u8> for
    /// [AppChannelPacket].
    pub async fn get_app_channel(
        &self,
    ) -> Option<(
        impl Sink<AppChannelPacket>,
        impl Stream<Item = AppChannelPacket>,
    )> {
        if let Some((tx, rx)) = self.appchannel_comm.lock().await.take() {
            // let all_rx = ;

            let app_tx = Box::pin(tx.into_sink().with_flat_map(|app_pk: AppChannelPacket| {
                stream::once(async { Ok(Packet::new(PLATFORM_PORT, APP_CHANNEL, app_pk.0)) })
            }));

            let app_rx = rx
                .into_stream()
                .map(|pk: Packet| AppChannelPacket(pk.get_data().to_vec()))
                .boxed();

            Some((app_tx, app_rx))
        } else {
            None
        }
    }

    /// Set radio in continious wave mode
    ///
    /// If activate is set to true, the Crazyflie's radio will transmit a continious wave at the current channel
    /// frequency. This will be active until the Crazyflie is reset or this function is called with activate to false.
    ///
    /// Setting continious wave will:
    ///  - Disconnect the radio link. So this function should practically only be used when connected over USB
    ///  - Jam any radio running on the same frequency, this includes Wifi and Bluetooth
    ///
    /// As such, this shall only be used for test purpose in a controlled environment.
    pub async fn set_cont_wave(&self, activate: bool) -> Result<()> {
        let command = if activate { 1 } else { 0 };
        self.uplink
            .send_async(Packet::new(
                PLATFORM_PORT,
                PLATFORM_COMMAND,
                vec![PLATFORM_SET_CONT_WAVE, command],
            ))
            .await?;
        Ok(())
    }

    /// Send system arm/disarm request
    ///
    /// Arms or disarms the Crazyflie's safety systems. When disarmed, the motors
    /// will not spin even if thrust commands are sent.
    ///
    /// # Arguments
    /// * `do_arm` - true to arm, false to disarm
    pub async fn send_arming_request(&self, do_arm: bool) -> Result<()> {
        let command = if do_arm { 1 } else { 0 };
        self.uplink
            .send_async(Packet::new(
                PLATFORM_PORT,
                PLATFORM_COMMAND,
                vec![PLATFORM_REQUEST_ARMING, command],
            ))
            .await?;
        Ok(())
    }

    /// Send crash recovery request
    ///
    /// Requests recovery from a crash state detected by the Crazyflie.
    pub async fn send_crash_recovery_request(&self) -> Result<()> {
        self.uplink
            .send_async(Packet::new(
                PLATFORM_PORT,
                PLATFORM_COMMAND,
                vec![PLATFORM_REQUEST_CRASH_RECOVERY],
            ))
            .await?;
        Ok(())
    }
}

/// # App channel packet
///
/// This object wraps a Vec<u8> but can only be created for byte array of length
/// <= [APPCHANNEL_MTU].
///
/// The [TryFrom] trait is implemented for ```Vec<u8>``` and ```&[u8]```. The
/// From trait is implemented for fixed size array with compatible length. These
/// traits are teh expected way to build a packet:
///
/// ```
/// # use std::convert::TryInto;
/// # use crazyflie_lib::subsystems::platform::AppChannelPacket;
/// let a: AppChannelPacket = [1,2,3].into();
/// let b: AppChannelPacket = vec![1,2,3].try_into().unwrap();
/// ```
///
/// And it protects agains building bad packets:
/// ``` should_panic
/// # use std::convert::TryInto;
/// # use crazyflie_lib::subsystems::platform::AppChannelPacket;
/// // This will panic!
/// let bad: AppChannelPacket = vec![0; 64].try_into().unwrap();
/// ```
///
/// The traits also allows to go the other way:
/// ```
/// # use crazyflie_lib::subsystems::platform::AppChannelPacket;
/// let pk: AppChannelPacket = [1,2,3].into();
/// let data: Vec<u8> = pk.into();
/// assert_eq!(data, vec![1,2,3]);
/// ```
#[derive(Debug, PartialEq, Eq)]
pub struct AppChannelPacket(Vec<u8>);

impl TryFrom<Vec<u8>> for AppChannelPacket {
    type Error = Error;

    fn try_from(value: Vec<u8>) -> Result<Self> {
        if value.len() <= APPCHANNEL_MTU {
            Ok(AppChannelPacket(value))
        } else {
            Err(Error::AppchannelPacketTooLarge)
        }
    }
}

impl TryFrom<&[u8]> for AppChannelPacket {
    type Error = Error;

    fn try_from(value: &[u8]) -> Result<Self> {
        if value.len() <= APPCHANNEL_MTU {
            Ok(AppChannelPacket(value.to_vec()))
        } else {
            Err(Error::AppchannelPacketTooLarge)
        }
    }
}

impl From<AppChannelPacket> for Vec<u8> {
    fn from(pk: AppChannelPacket) -> Self {
        pk.0
    }
}

// Implement useful From<> for fixed size array
// This would be much better as a contrained const generic but
// it does not seems to be possible at the moment
macro_rules! from_impl {
    ($n:expr) => {
        impl From<[u8; $n]> for AppChannelPacket {
            fn from(v: [u8; $n]) -> Self {
                AppChannelPacket(v.to_vec())
            }
        }
    };
}

from_impl!(0);
from_impl!(1);
from_impl!(2);
from_impl!(3);
from_impl!(4);
from_impl!(5);
from_impl!(6);
from_impl!(7);
from_impl!(8);
from_impl!(9);
from_impl!(10);
from_impl!(11);
from_impl!(12);
from_impl!(13);
from_impl!(14);
from_impl!(15);
from_impl!(16);
from_impl!(17);
from_impl!(18);
from_impl!(19);
from_impl!(20);
from_impl!(21);
from_impl!(22);
from_impl!(23);
from_impl!(24);
from_impl!(25);
from_impl!(26);
from_impl!(27);
from_impl!(28);
from_impl!(29);
from_impl!(30);
