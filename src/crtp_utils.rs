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
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Mutex, MutexGuard, OnceLock, Weak};
use std::time::Duration;
use std::{
    convert::{TryFrom, TryInto},
    sync::Arc,
};
use tokio::sync::watch;

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

/// A TOC download that is currently in progress.
///
/// The connection that created the slot is the "leader": it performs the actual
/// download and publishes the serialized TOC on `done`. Connections that find an
/// existing slot for the same key are "followers": they send no packets at all and
/// simply wait for the leader's result.
///
/// The slot removes itself from [`in_flight_map`] when it is dropped, which happens
/// on success, on error, and on cancellation alike.
struct TocFetchSlot {
    key: [u8; 5],
    done: watch::Sender<Option<String>>,
}

impl Drop for TocFetchSlot {
    fn drop(&mut self) {
        let mut map = lock_in_flight();
        // Only remove a dead entry. A live one belongs to a leader that was elected
        // after this slot was already on its way out, and must be left alone.
        if map.get(&self.key).is_some_and(|slot| slot.strong_count() == 0) {
            map.remove(&self.key);
        }
    }
}

/// Role of a connection for a TOC download that is not served from the cache.
enum TocFetch {
    /// No download was in progress: this connection has to perform it.
    Lead(Arc<TocFetchSlot>),
    /// A download is already in progress: wait for its result instead.
    Follow(watch::Receiver<Option<String>>),
}

/// TOC downloads currently in progress, keyed by cache key.
///
/// The map holds [`Weak`] references so that a leader which is dropped mid-download
/// (a failed link, a cancelled connect future) cannot leave a permanently occupied
/// entry behind. Entries are removed when the slot is dropped, so the map is empty
/// whenever no TOC download is running.
fn in_flight_map() -> &'static Mutex<HashMap<[u8; 5], Weak<TocFetchSlot>>> {
    static IN_FLIGHT: OnceLock<Mutex<HashMap<[u8; 5], Weak<TocFetchSlot>>>> = OnceLock::new();
    IN_FLIGHT.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock_in_flight() -> MutexGuard<'static, HashMap<[u8; 5], Weak<TocFetchSlot>>> {
    // The lock is only ever held for a map lookup, never across an await, so a
    // poisoned mutex carries no broken invariant and the map stays usable.
    in_flight_map()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Join the download already in progress for `key`, or become the one that performs it.
fn join_or_lead(key: [u8; 5]) -> TocFetch {
    let mut map = lock_in_flight();

    let running = map.get(&key).and_then(Weak::upgrade);
    if let Some(slot) = running {
        let receiver = slot.done.subscribe();
        // Release the lock before `slot` goes out of scope: dropping the last
        // reference runs `TocFetchSlot::drop`, which takes the same lock.
        drop(map);
        return TocFetch::Follow(receiver);
    }

    let (done, _) = watch::channel(None);
    let slot = Arc::new(TocFetchSlot { key, done });
    map.insert(key, Arc::downgrade(&slot));
    TocFetch::Lead(slot)
}

fn deserialize_toc<T>(toc_str: &str) -> Result<BTreeMap<String, (u16, T)>>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_str(toc_str).map_err(|e| {
        Error::InvalidParameter(format!("Failed to deserialize TOC cache: {}", e))
    })
}

/// Download the full TOC from the Crazyflie, one item at a time.
async fn download_toc<T, E>(
    port: u8,
    uplink: &channel::Sender<Packet>,
    downlink: &channel::Receiver<Packet>,
    toc_len: u16,
) -> Result<BTreeMap<String, (u16, T)>>
where
    T: TryFrom<u8, Error = E>,
    E: Into<Error>,
{
    let mut toc = BTreeMap::new();

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

    let crc_bytes = toc_crc32.to_le_bytes();
    let cache_key: [u8; 5] = [TOC_CACHE_VERSION, crc_bytes[0], crc_bytes[1], crc_bytes[2], crc_bytes[3]];

    // Check cache first
    if let Some(toc_str) = toc_cache.get_toc(&cache_key) {
        return deserialize_toc(&toc_str);
    }

    // Cache miss. Crazyflies running the same firmware share a cache key, so a swarm
    // connecting at once would otherwise download the same TOC once per Crazyflie,
    // all of them competing for the same radio bandwidth. Download it once and share
    // the result instead.
    match join_or_lead(cache_key) {
        TocFetch::Lead(slot) => {
            let toc = download_toc::<T, E>(port, &uplink, &downlink, toc_len).await?;

            let toc_str = serde_json::to_string(&toc).map_err(|e| {
                Error::InvalidParameter(format!("Failed to serialize TOC: {}", e))
            })?;
            toc_cache.store_toc(&cache_key, &toc_str);

            // Hand the result to everyone waiting on this download. Dropping `slot`
            // without sending, on the error paths above, closes the channel and lets
            // the followers fall back to downloading the TOC themselves.
            let _ = slot.done.send(Some(toc_str));

            Ok(toc)
        }
        TocFetch::Follow(mut done) => match done.wait_for(Option::is_some).await {
            Ok(toc_str) => {
                deserialize_toc(toc_str.as_deref().expect("waited for a published TOC"))
            }
            // The leader failed or was cancelled before publishing anything. Nothing
            // is known about why, so just download the TOC over this connection.
            Err(_) => download_toc(port, &uplink, &downlink, toc_len).await,
        },
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The in-flight map is process-wide, so every test uses its own key to stay
    /// independent of the others.
    fn key(id: u8) -> [u8; 5] {
        [TOC_CACHE_VERSION, id, 0, 0, 0]
    }

    #[test]
    fn first_caller_leads_and_the_next_one_follows() {
        let key = key(1);

        let leader = match join_or_lead(key) {
            TocFetch::Lead(slot) => slot,
            TocFetch::Follow(_) => panic!("nothing was in flight, should have become leader"),
        };
        assert!(matches!(join_or_lead(key), TocFetch::Follow(_)));

        drop(leader);
    }

    #[test]
    fn dropping_the_leader_frees_the_key() {
        let key = key(2);

        let leader = join_or_lead(key);
        assert!(matches!(leader, TocFetch::Lead(_)));
        drop(leader);

        assert!(lock_in_flight().get(&key).is_none());
        // A cancelled or failed download must not block later attempts.
        assert!(matches!(join_or_lead(key), TocFetch::Lead(_)));
    }

    #[tokio::test]
    async fn followers_get_the_toc_downloaded_by_the_leader() {
        let key = key(3);

        let TocFetch::Lead(leader) = join_or_lead(key) else {
            panic!("nothing was in flight, should have become leader");
        };
        let TocFetch::Follow(mut follower) = join_or_lead(key) else {
            panic!("a download is in flight, should have become follower");
        };

        let toc = r#"{"example.value":[1,1]}"#;
        let _ = leader.done.send(Some(toc.to_string()));
        drop(leader);

        let received = follower.wait_for(Option::is_some).await;
        assert_eq!(received.unwrap().as_deref(), Some(toc));
    }

    #[tokio::test]
    async fn followers_are_released_when_the_leader_disappears() {
        let key = key(4);

        let TocFetch::Lead(leader) = join_or_lead(key) else {
            panic!("nothing was in flight, should have become leader");
        };
        let TocFetch::Follow(mut follower) = join_or_lead(key) else {
            panic!("a download is in flight, should have become follower");
        };

        // The leader's link died, or its connect future was cancelled.
        drop(leader);

        // The follower must not wait forever: it falls back to its own download.
        assert!(follower.wait_for(Option::is_some).await.is_err());
    }
}
