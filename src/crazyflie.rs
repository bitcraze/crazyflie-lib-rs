use crate::subsystems::commander::Commander;
use crate::subsystems::console::Console;
use crate::subsystems::log::Log;
use crate::subsystems::param::Param;

use crate::crtp_utils::CrtpDispatch;
use crate::Executor;
use crate::{Error, Result};
use async_executors::{JoinHandle, LocalSpawnHandleExt, TimerExt};
use flume as channel;
use futures::lock::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;
use std::time::Duration;

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
    /// Console subsystem access
    pub console: Console,
    pub(crate) _executor: Arc<dyn Executor>,
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
        executor: impl Executor,
        link_context: &crazyflie_link::LinkContext,
        uri: &str,
    ) -> Result<Self> {
        let link = link_context.open_link(uri).await?;

        Self::connect_from_link(executor, link).await
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
        executor: impl Executor,
        link: crazyflie_link::Connection,
    ) -> Result<Self> {
        let disconnect = Arc::new(AtomicBool::new(false));
        let executor = Arc::new(executor);

        // Downlink dispatcher
        let link = Arc::new(link);
        let mut dispatcher = CrtpDispatch::new(executor.clone(), link.clone(), disconnect.clone());

        // Uplink queue
        let disconnect_uplink = disconnect.clone();
        let (uplink, rx) = channel::unbounded();
        let executor_uplink = executor.clone();
        let link_uplink = link.clone();
        let uplink_task = executor
            .spawn_handle_local(async move {
                while !disconnect_uplink.load(Relaxed) {
                    match executor_uplink
                        .timeout(Duration::from_millis(100), rx.recv_async())
                        .await
                    {
                        Ok(Ok(pk)) => {
                            if link_uplink.send_packet(pk).await.is_err() {
                                return;
                            }
                        }
                        Err(async_executors::TimeoutError) => (),
                        Ok(Err(flume::RecvError::Disconnected)) => return,
                    }
                }
            })
            .map_err(|e| Error::SystemError(format!("{:?}", e)))?;

        // Create subsystems one by one
        // The future is passed to join!() later down so that all modules initializes at the same time
        // The get_port_receiver calls are guaranteed to work if the same port is not used twice (any way to express that at compile time?)
        let log_downlink = dispatcher.get_port_receiver(5).unwrap();
        let log = Log::new(log_downlink, uplink.clone());

        let param_downlink = dispatcher.get_port_receiver(2).unwrap();
        let param = Param::new(param_downlink, uplink.clone());

        let commander = Commander::new(uplink.clone());

         // Modules that can be initialized synchrnously
         let console_downlink = dispatcher.get_port_receiver(0).unwrap();
         let console = Console::new(executor.clone(), console_downlink).await?;

        // Start the downlink packet dispatcher
        let dispatch_task = dispatcher.run().await?;

        // Initialize all modules in parallel
        let (log, param) = futures::join!(log, param);

        Ok(Crazyflie {
            log: log?,
            param: param?,
            commander,
            console,
            _executor: executor,
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
            uplink_task.await;
        }
        if let Some(dispatch_task) = self.dispatch_task.lock().await.take() {
            dispatch_task.await;
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
