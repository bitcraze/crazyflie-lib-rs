use crate::subsystems::log::Log;
use crate::subsystems::param::Param;
use crate::subsystems::commander::Commander;

use async_executors::{JoinHandle, LocalSpawnHandleExt, TimerExt};
use flume as channel;
use futures::lock::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;
use std::sync::Arc;
use crate::Executor;
use crate::crtp_utils::CrtpDispatch;
use crate::{Error, Result};

/// # The Crazyflie
/// 
/// This struct is one-time use: Creating it will connect to a Crazyflie and once disconnected, either as requested
/// by the lib user or as a result of a connection loss, the object cannot be reconected. A new one need to be created
/// to connect again.
/// 
/// See the [crazyflie-lib crate root documentation](crate) for more context and information.
pub struct Crazyflie {
    pub log: Log,
    pub param: Param,
    pub commander: Commander,
    pub(crate) _executor: Arc<dyn Executor>,
    uplink_task: Mutex<Option<JoinHandle<()>>>,
    dispatch_task: Mutex<Option<JoinHandle<()>>>,
    disconnect: Arc<AtomicBool>,
    link: Arc<crazyflie_link::Connection>,
}

impl Crazyflie {

    /// Open a Crazyflie connection to a given URI
    /// 
    /// 
    pub async fn connect_from_uri(
        executor: impl Executor,
        link_context: &crazyflie_link::LinkContext,
        uri: &str,
    ) -> Result<Self> {
        let link = link_context.open_link(uri).await?;

        Self::connect_from_link(executor, link).await
    }

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

        // Start the downlink packet dispatcher
        let dispatch_task = dispatcher.run().await?;

        // Intitialize all modules in parallel
        let (log, param) = futures::join!(log, param);

        Ok(Crazyflie {
            log: log?,
            param: param?,
            commander,
            _executor: executor,
            uplink_task: Mutex::new(Some(uplink_task)),
            dispatch_task: Mutex::new(Some(dispatch_task)),
            disconnect,
            link,
        })
    }

    pub async fn disconnect(&self) {
        // Set disconnect to true, will make both uplink and dispatcher task quit
        self.disconnect.store(true, Relaxed);

        // Wait for both task to finish
        if self.uplink_task.lock().await.is_some() {
            self.uplink_task.lock().await.take().unwrap().await
        }
        if self.dispatch_task.lock().await.is_some() {
            self.dispatch_task.lock().await.take().unwrap().await
        }

        self.link.close().await;
    }

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
