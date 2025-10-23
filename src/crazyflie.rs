use crate::subsystems::commander::Commander;
use crate::subsystems::high_level_commander::HighLevelCommander;
use crate::subsystems::console::Console;
use crate::subsystems::localization::Localization;
use crate::subsystems::log::Log;
use crate::subsystems::param::Param;

use crate::crtp_utils::CrtpDispatch;
use crate::subsystems::platform::Platform;
use crate::{Error, Result};
use crate::SUPPORTED_PROTOCOL_VERSION;
use flume as channel;
use futures::lock::Mutex;
use tokio::task::JoinHandle;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;
use std::time::Duration;

// CRTP ports
pub(crate) const CONSOLE_PORT: u8 = 0;
pub(crate) const PARAM_PORT: u8 = 2;
pub(crate) const COMMANDER_PORT: u8 = 3;
pub(crate) const _MEMORY_PORT: u8 = 4;
pub(crate) const LOG_PORT: u8 = 5;
pub(crate) const LOCALIZATION_PORT: u8 = 6;
pub(crate) const GENERIC_SETPOINT_PORT: u8 = 7;
pub(crate) const HL_COMMANDER_PORT: u8 = 8;
pub(crate) const PLATFORM_PORT: u8 = 13;
pub(crate) const _LINK_PORT: u8 = 15;

/// # The Crazyflie
///
/// This struct is one-time use: Creating it will connect to a Crazyflie and once disconnected, either as requested
/// by the lib user or as a result of a connection loss, the object cannot be reconnected. A new one need to be created
/// to connect again.
///
/// See the [crazyflie-lib crate root documentation](crate) for more context and information.
pub struct Crazyflie {
    /// Log subsystem access
    pub log: Log,
    /// Parameter subsystem access
    pub param: Param,
    /// Commander/setpoint subsystem access
    pub commander: Commander,
    /// High-level commander subsystem access
    pub high_level_commander: HighLevelCommander,
    /// Console subsystem access
    pub console: Console,
    /// Localization services
    pub localization: Localization,
    /// Platform services
    pub platform: Platform,
    uplink_task: Mutex<Option<JoinHandle<()>>>,
    dispatch_task: Mutex<Option<JoinHandle<()>>>,
    disconnect: Arc<AtomicBool>,
    link: Arc<crazyflie_link::Connection>,
}

impl Crazyflie {
    /// Open a Crazyflie connection to a given URI
    ///
    /// This function opens a link to the given URI and calls [Crazyflie::connect_from_link()] to connect the Crazyflie.
    ///
    /// The executor argument should be an async executor from the crate `async_executors`. See example in the
    /// [crate root documentation](crate).
    ///
    /// An error is returned either if the link cannot be opened or if the Crazyflie connection fails.
    pub async fn connect_from_uri(
        link_context: &crazyflie_link::LinkContext,
        uri: &str,
    ) -> Result<Self> {
        let link = link_context.open_link(uri).await?;

        Self::connect_from_link(link).await
    }

    /// Connect a Crazyflie using an existing link
    ///
    /// Connect a Crazyflie using an existing connected link.
    ///
    /// The executor argument should be an async executor from the crate `async_executors`. See example in the
    /// [crate root documentation](crate).
    ///
    /// This function will return an error if anything goes wrong in the connection process.
    pub async fn connect_from_link(
        link: crazyflie_link::Connection,
    ) -> Result<Self> {
        let disconnect = Arc::new(AtomicBool::new(false));

        // Downlink dispatcher
        let link = Arc::new(link);
        let mut dispatcher = CrtpDispatch::new(link.clone(), disconnect.clone());

        // Uplink queue
        let disconnect_uplink = disconnect.clone();
        let (uplink, rx) = channel::unbounded();
        let link_uplink = link.clone();
        let uplink_task = tokio::spawn(async move {
                while !disconnect_uplink.load(Relaxed) {
                    match tokio::time::timeout(
                          Duration::from_millis(100), rx.recv_async()
                        ).await
                    {
                        Ok(Ok(pk)) => {
                            if link_uplink.send_packet(pk).await.is_err() {
                                return;
                            }
                        }
                        Err(_) => (),
                        Ok(Err(flume::RecvError::Disconnected)) => return,
                    }
                }
            });

        // Downlink dispatch
        let platform_downlink = dispatcher.get_port_receiver(PLATFORM_PORT).unwrap();
        let log_downlink = dispatcher.get_port_receiver(LOG_PORT).unwrap();
        let param_downlink = dispatcher.get_port_receiver(PARAM_PORT).unwrap();
        let console_downlink = dispatcher.get_port_receiver(CONSOLE_PORT).unwrap();
        let localization_downlink = dispatcher.get_port_receiver(LOCALIZATION_PORT).unwrap();

        // Start the downlink packet dispatcher
        let dispatch_task = dispatcher.run().await?;

        // Start with the platform subsystem to get and test the Crazyflie's protocol version
        let platform = Platform::new(uplink.clone(), platform_downlink);

        let protocol_version = platform.protocol_version().await?;

        if !(SUPPORTED_PROTOCOL_VERSION..=(SUPPORTED_PROTOCOL_VERSION + 1))
            .contains(&protocol_version)
        {
            return Err(Error::ProtocolVersionNotSupported);
        }

        // Create subsystems one by one
        // The future is passed to join!() later down so that all modules initializes at the same time
        // The get_port_receiver calls are guaranteed to work if the same port is not used twice (any way to express that at compile time?)
        let log_future = Log::new(log_downlink, uplink.clone());
        let param_future = Param::new(param_downlink, uplink.clone());

        let commander = Commander::new(uplink.clone());
        let high_level_commander = HighLevelCommander::new(uplink.clone());
        let console = Console::new(console_downlink).await?;
        let localization = Localization::new(uplink.clone(), localization_downlink);

        // Initialize async modules in parallel
        let (log, param) = futures::join!(log_future, param_future);

        Ok(Crazyflie {
            log: log?,
            param: param?,
            commander,
            high_level_commander,
            console,
            localization,
            platform,
            uplink_task: Mutex::new(Some(uplink_task)),
            dispatch_task: Mutex::new(Some(dispatch_task)),
            disconnect,
            link,
        })
    }

    /// Disconnect the Crazyflie
    ///
    /// The Connection can be ended in two ways: either by dropping the [Crazyflie] object or by calling this
    /// disconnect() function. Once this function return, the Crazyflie is fully disconnected.
    ///
    /// Once disconnected, any methods that uses the communication to the Crazyflie will return the error
    /// [Error::Disconnected]
    pub async fn disconnect(&self) {
        // Set disconnect to true, will make both uplink and dispatcher task quit
        self.disconnect.store(true, Relaxed);

        // Wait for both task to finish
        if let Some(uplink_task) = self.uplink_task.lock().await.take() {
            uplink_task.await.expect("Uplink task failed");
        }
        if let Some(dispatch_task) = self.dispatch_task.lock().await.take() {
            dispatch_task.await.expect("Dispatcher task failed");
        }

        self.link.close().await;
    }

    /// Wait for the Crazyflie to be disconnected
    ///
    /// This function waits for the Crazyflie link to close and for the Crazyflie to fully disconnect. It returns
    /// a string describing the reason for the disconnection.
    ///
    /// One intended use if to call and block on this function from an async task to detect a disconnection and, for
    /// example, update the state of a GUI.
    pub async fn wait_disconnect(&self) -> String {
        let reason = self.link.wait_close().await;

        self.disconnect().await;

        reason
    }
}

impl Drop for Crazyflie {
    fn drop(&mut self) {
        self.disconnect.store(true, Relaxed);
    }
}
