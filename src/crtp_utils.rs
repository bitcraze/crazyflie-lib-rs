//! Various CRTP utils used by the lib
//!
//! These functionalities are currently all private, some might be useful for the user code as well, lets make them
//! public when needed.

use crate::{Error, Result};
use async_trait::async_trait;
use crazyflie_link::Packet;
use flume as channel;
use flume::{Receiver, Sender};
use serde::{Deserialize, Serialize};
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
                let _ = &self;
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
/// Cache format version, included in the cache key.
/// Bump when ParamItemInfo or LogItemInfo serialization changes.
const TOC_CACHE_VERSION: u8 = 1;

pub(crate) async fn fetch_toc<C, T, E>(
    port: u8,
    uplink: channel::Sender<Packet>,
    downlink: channel::Receiver<Packet>,
    toc_cache: C,
) -> Result<std::collections::BTreeMap<String, (u16, T)>>
where
    C: TocCache,
    T: TryFrom<u8, Error = E> + Serialize + for<'de> Deserialize<'de>,
    E: Into<Error>,
{
    let pk = Packet::new(port, 0, vec![TOC_INFO]);
    uplink
        .send_async(pk)
        .await
        .map_err(|_| Error::Disconnected)?;

    let pk = downlink.wait_packet(port, TOC_CHANNEL, &[TOC_INFO]).await?;

    let toc_len = u16::from_le_bytes(pk.get_data()[1..3].try_into()?);
    let toc_crc32 = u32::from_le_bytes(pk.get_data()[3..7].try_into()?);

    let mut toc = std::collections::BTreeMap::new();
    let crc_bytes = toc_crc32.to_le_bytes();
    let cache_key: [u8; 5] = [TOC_CACHE_VERSION, crc_bytes[0], crc_bytes[1], crc_bytes[2], crc_bytes[3]];

    // Check cache first
    if let Some(toc_str) = toc_cache.get_toc(&cache_key) {
        toc = serde_json::from_str(&toc_str).map_err(|e| Error::InvalidParameter(format!("Failed to deserialize TOC cache: {}", e)))?;
        return Ok(toc);
    }

    // Fetch TOC from device
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

    // Store in cache
    let toc_str = serde_json::to_string(&toc).map_err(|e| Error::InvalidParameter(format!("Failed to serialize TOC: {}", e)))?;
    toc_cache.store_toc(&cache_key, &toc_str);

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

/// Null implementation of ToC cache to be used when no caching is needed.
#[derive(Clone)]
pub struct NoTocCache;

impl TocCache for NoTocCache {
    fn get_toc(&self, _key: &[u8]) -> Option<String> {
        None
    }

    fn store_toc(&self, _key: &[u8], _toc: &str) {
        // No-op: this cache doesn't store anything
    }
}

/// A trait for caching Table of Contents (TOC) data.
///
/// This trait provides methods for storing and retrieving TOC information
/// using an opaque byte key. Implementations can use this to avoid
/// re-fetching TOC data when the key matches a cached version.
///
/// The key is constructed by the library and should be treated as an opaque
/// identifier. Implementors are free to encode it in whatever way suits their
/// storage backend (e.g., hex encoding for filenames, raw bytes for in-memory maps).
///
/// # Concurrency
///
/// Both methods take `&self` to allow concurrent reads during parallel TOC fetching
/// (Log and Param subsystems fetch their TOCs simultaneously). Implementations should
/// use interior mutability (e.g., `RwLock`) for thread-safe caching.
///
/// # Example
///
/// ```rust
/// use std::sync::{Arc, RwLock};
/// use std::collections::HashMap;
/// use crazyflie_lib::TocCache;
///
/// #[derive(Clone)]
/// struct InMemoryCache {
///     data: Arc<RwLock<HashMap<Vec<u8>, String>>>,
/// }
///
/// impl TocCache for InMemoryCache {
///     fn get_toc(&self, key: &[u8]) -> Option<String> {
///         self.data.read().ok()?.get(key).cloned()
///     }
///
///     fn store_toc(&self, key: &[u8], toc: &str) {
///         if let Ok(mut lock) = self.data.write() {
///             lock.insert(key.to_vec(), toc.to_string());
///         }
///     }
/// }
/// ```
pub trait TocCache: Clone + Send + Sync + 'static
{
    /// Retrieves a cached TOC string based on the provided key.
    ///
    /// # Arguments
    ///
    /// * `key` - An opaque byte key used to identify the TOC.
    ///
    /// # Returns
    ///
    /// An `Option<String>` containing the cached TOC if it exists, or `None` if not found.
    fn get_toc(&self, key: &[u8]) -> Option<String>;

    /// Stores a TOC string associated with the provided key.
    ///
    /// # Arguments
    ///
    /// * `key` - An opaque byte key used to identify the TOC.
    /// * `toc` - The TOC string to be stored.
    fn store_toc(&self, key: &[u8], toc: &str);
}
