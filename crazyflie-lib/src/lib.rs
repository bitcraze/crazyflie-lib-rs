pub mod commander;
mod error;
pub mod log;
pub mod param;
mod value;

// Async executor selection
#[cfg(feature = "async-std")]
pub(crate) use async_std::task::spawn;

#[cfg(feature = "wasm-bindgen-futures")]
use wasm_bindgen_futures::spawn_local as spawn;

pub(crate) use crate::commander::Commander;
pub use crate::error::{Error, Result};
pub(crate) use crate::log::Log;
pub(crate) use crate::param::Param;
pub use crate::value::{Value, ValueType};

use async_executors::{JoinHandle, LocalSpawnHandle, LocalSpawnHandleExt, Timer, TimerExt};
use async_trait::async_trait;
use crazyflie_link::Packet;
use flume as channel;
use flume::{Receiver, Sender};
use futures::lock::Mutex;
use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;
use std::{
    convert::{TryFrom, TryInto},
    sync::Arc,
};
use trait_set::trait_set;

trait_set! {
    pub trait Executor = LocalSpawnHandle<()> + Timer + 'static
}

pub struct Crazyflie {
    pub log: Log,
    pub param: Param,
    pub commander: Commander,
    pub executor: Arc<dyn Executor>,
    uplink_task: Mutex<Option<JoinHandle<()>>>,
    dispatch_task: Mutex<Option<JoinHandle<()>>>,
    disconnect: Arc<AtomicBool>,
    link: Arc<crazyflie_link::Connection>,
}

impl Crazyflie {
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
            executor,
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
        self.link.wait_close().await
    }
}

impl Drop for Crazyflie {
    fn drop(&mut self) {
        self.disconnect.store(true, Relaxed);
    }
}

struct CrtpDispatch {
    link: Arc<crazyflie_link::Connection>,
    // port_callbacks: [Arc<Mutex<Option<Sender<Packet>>>>; 15]
    port_channels: BTreeMap<u8, Sender<Packet>>,
    disconnect: Arc<AtomicBool>,
    executor: Arc<dyn Executor>,
}

impl CrtpDispatch {
    fn new(
        executor: impl Executor,
        link: Arc<crazyflie_link::Connection>,
        disconnect: Arc<AtomicBool>,
    ) -> Self {
        CrtpDispatch {
            link,
            port_channels: BTreeMap::new(),
            disconnect,
            executor: Arc::new(executor),
        }
    }

    #[allow(clippy::map_entry)]
    fn get_port_receiver(&mut self, port: u8) -> Option<Receiver<Packet>> {
        if self.port_channels.contains_key(&port) {
            None
        } else {
            let (tx, rx) = channel::unbounded();
            self.port_channels.insert(port, tx);
            Some(rx)
        }
    }

    async fn run(self) -> Result<JoinHandle<()>> {
        let link = self.link.clone();
        let executor = self.executor.clone();
        executor
            .spawn_handle_local(async move {
                while !self.disconnect.load(Relaxed) {
                    match self
                        .executor
                        .timeout(Duration::from_millis(200), link.recv_packet())
                        .await
                    {
                        Ok(Ok(packet)) => {
                            if packet.get_port() < 16 {
                                let channel = self.port_channels.get(&packet.get_port()); // get(packet.get_port()).lock().await;
                                if let Some(channel) = channel.as_ref() {
                                    let _ = channel.send_async(packet).await;
                                }
                            }
                        }
                        Err(async_executors::TimeoutError) => continue,
                        Ok(Err(_)) => return, // Other side of the channel disapeared, link closed
                    }
                }
            })
            .map_err(|e| Error::SystemError(format!("{:?}", e)))
    }
}

#[async_trait]
trait WaitForPacket {
    async fn wait_packet(&self, port: u8, channel: u8, data_prefix: &[u8]) -> Result<Packet>;
}

#[async_trait]
impl WaitForPacket for channel::Receiver<Packet> {
    async fn wait_packet(&self, port: u8, channel: u8, data_prefix: &[u8]) -> Result<Packet> {
        let mut pk = self.recv_async().await.ok().ok_or(Error::Disconnected)?;

        loop {
            if pk.get_port() == port
                && pk.get_channel() == channel
                && pk.get_data().starts_with(data_prefix)
            {
                break;
            }
            pk = self.recv_async().await.ok().ok_or(Error::Disconnected)?;
        }

        Ok(pk)
    }
}

const TOC_CHANNEL: u8 = 0;
const TOC_GET_ITEM: u8 = 2;
const TOC_INFO: u8 = 3;

async fn fetch_toc<T, E>(
    port: u8,
    uplink: channel::Sender<Packet>,
    downlink: channel::Receiver<Packet>,
) -> Result<std::collections::BTreeMap<String, (u16, T)>>
where
    T: TryFrom<u8, Error = E>,
    E: Into<Error>,
{
    let pk = Packet::new(port, 0, vec![TOC_INFO]);
    uplink
        .send_async(pk)
        .await
        .map_err(|_| Error::Disconnected)?;

    let pk = downlink.wait_packet(port, TOC_CHANNEL, &[TOC_INFO]).await?;

    let toc_len = u16::from_le_bytes(pk.get_data()[1..3].try_into()?);

    let mut toc = std::collections::BTreeMap::new();

    for i in 0..toc_len {
        let pk = Packet::new(
            port,
            0,
            vec![TOC_GET_ITEM, (i & 0x0ff) as u8, (i >> 8) as u8],
        );
        uplink
            .send_async(pk)
            .await
            .map_err(|_| Error::Disconnected)?;

        let pk = downlink.wait_packet(port, 0, &[TOC_GET_ITEM]).await?;

        let mut strings = pk.get_data()[4..].split(|b| *b == 0);
        let group = String::from_utf8_lossy(strings.next().expect("TOC packet format error"));
        let name = String::from_utf8_lossy(strings.next().expect("TOC packet format error"));

        let id = u16::from_le_bytes(pk.get_data()[1..3].try_into()?);
        let item_type = pk.get_data()[3].try_into().map_err(|e: E| e.into())?;
        toc.insert(format!("{}.{}", group, name), (id, item_type));
    }

    Ok(toc)
}

pub fn crtp_channel_dispatcher(
    downlink: channel::Receiver<Packet>,
) -> (
    Receiver<Packet>,
    Receiver<Packet>,
    Receiver<Packet>,
    Receiver<Packet>,
) {
    let (mut senders, mut receivers) = (Vec::new(), Vec::new());

    for _ in 0..4 {
        let (tx, rx) = channel::unbounded();
        senders.push(tx);
        receivers.insert(0, rx);
    }

    spawn(async move {
        while let Ok(pk) = downlink.recv_async().await {
            if pk.get_channel() < 4 {
                let _ = senders[pk.get_channel() as usize].send_async(pk).await;
            }
        }
    });

    // The 4 unwraps are guaranteed to succeed by design (the list is 4 item long)
    (
        receivers.pop().unwrap(),
        receivers.pop().unwrap(),
        receivers.pop().unwrap(),
        receivers.pop().unwrap(),
    )
}
