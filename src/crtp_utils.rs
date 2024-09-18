//! Various CRTP utils used by the lib
//!
//! These functionalities are currently all private, some might be useful for the user code as well, lets make them
//! public when needed.

use crate::{Error, Result};
use async_trait::async_trait;
use crazyflie_link::Packet;
use flume as channel;
use flume::{Receiver, Sender};
use tokio::task::JoinHandle;
use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;
use std::{
    convert::{TryFrom, TryInto},
    sync::Arc,
};

pub struct CrtpDispatch {
    link: Arc<crazyflie_link::Connection>,
    // port_callbacks: [Arc<Mutex<Option<Sender<Packet>>>>; 15]
    port_channels: BTreeMap<u8, Sender<Packet>>,
    disconnect: Arc<AtomicBool>
}

impl CrtpDispatch {
    pub fn new(
        link: Arc<crazyflie_link::Connection>,
        disconnect: Arc<AtomicBool>,
    ) -> Self {
        CrtpDispatch {
            link,
            port_channels: BTreeMap::new(),
            disconnect
        }
    }

    #[allow(clippy::map_entry)]
    pub fn get_port_receiver(&mut self, port: u8) -> Option<Receiver<Packet>> {
        if self.port_channels.contains_key(&port) {
            None
        } else {
            let (tx, rx) = channel::unbounded();
            self.port_channels.insert(port, tx);
            Some(rx)
        }
    }

    pub async fn run(self) -> Result<JoinHandle<()>> {
        let link = self.link.clone();
        Ok(tokio::spawn(async move {
                while !self.disconnect.load(Relaxed) {                  
                    match tokio::time::timeout(Duration::from_millis(200), link.recv_packet())
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
                        Err(_) => continue,
                        Ok(Err(_)) => return, // Other side of the channel disappeared, link closed
                    }
                }
            })
          )
    }
}

#[async_trait]
pub(crate) trait WaitForPacket {
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

pub(crate) async fn fetch_toc<T, E>(
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

    tokio::spawn(async move {
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
