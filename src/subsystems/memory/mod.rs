//! # Memory subsystem
//!
//! The Crazyflie exposes a memory subsystem that allows to easily read and
//! write various memories in the Crazyflie.
//!
//! During connection the memory subsystem fetches information about all the
//! memories present in the Crazyflie. For interacting with a specific memory
//! it's possible to using a wrapper, there's one for each memory type, or to
//! get raw read and write access to the memory.

use crate::{crtp_utils::WaitForPacket, Error, Result};
use crazyflie_link::Packet;
use flume as channel;
use std::{collections::HashMap, convert::{TryFrom, TryInto}};
use std::sync::Arc;
use tokio::sync::Mutex;

mod memory_types;
mod eeprom_config;
mod raw;
mod ow;

use crate::crazyflie::MEMORY_PORT;

pub use memory_types::*;
pub use eeprom_config::*;
pub use raw::*;
pub use ow::*;

/// # Access to the Crazyflie Memory Subsystem
///
/// This struct provide methods to interact with the memory subsystem. See the
/// [memory module documentation](crate::subsystems::memory) for more context and information.
#[derive(Debug)]
pub struct Memory {
    memories: Vec<MemoryDevice>,
    backends: Vec<Mutex<Option<MemoryBackend>>>,
    memory_read_dispatcher: MemoryDispatcher,
    memory_write_dispatcher: MemoryDispatcher,
}

const INFO_CHANNEL: u8 = 0;
const READ_CHANNEL: u8 = 1;
const WRITE_CHANNEL: u8 = 2;

const _CMD_INFO_VER: u8 = 0;
const CMD_INFO_NBR: u8 = 1;
const CMD_INFO_DETAILS: u8 = 2;

#[derive(Debug)]
struct MemoryDispatcher {
  senders: Arc<Mutex<HashMap<u8, channel::Sender<Packet>>>>,
}

impl MemoryDispatcher {
  fn new(downlink: channel::Receiver<Packet>, channel: u8) -> Self {

    let senders: Arc<Mutex<HashMap<u8, channel::Sender<Packet>>>> = Arc::new(Mutex::new(HashMap::new()));
    let internal_senders = senders.clone();

    tokio::spawn(async move {
      while let Ok(pk) = downlink.recv_async().await {
        if pk.get_channel() == channel {
          let memory_id = pk.get_data()[0];
          if let Some(sender) = internal_senders.lock().await.get(&memory_id) {
            let _ = sender.send_async(pk).await;
          } else {
            println!("Warning: Received memory read response for unknown memory ID {}", memory_id);
          }
        } else {
          println!("Warning: Received packet on unexpected channel {}", pk.get_channel());
        }
      }
    });

    Self {
      senders: senders,
    }
  }

  async fn get_channel(&mut self, memory_id: u8) -> channel::Receiver<Packet> {
    if !self.senders.lock().await.contains_key(&memory_id) {
      let (tx, rx) = channel::unbounded();
      self.senders.lock().await.insert(memory_id, tx);
      rx
    } else {
      panic!("Channel for memory ID {} already exists", memory_id)
    }
  }
}

impl Memory {
    pub(crate) async fn new(
        downlink: channel::Receiver<Packet>,
        uplink: channel::Sender<Packet>,
    ) -> Result<Self> {
        let (info_channel_downlink, read_channel_downlink, write_channel_downlink, _misc_downlink) =
            crate::crtp_utils::crtp_channel_dispatcher(downlink);

        let mut memory = Self {
            memories: Vec::new(),
            backends: Vec::new(),
            memory_read_dispatcher: MemoryDispatcher::new(read_channel_downlink.clone(), READ_CHANNEL),
            memory_write_dispatcher: MemoryDispatcher::new(write_channel_downlink.clone(), WRITE_CHANNEL),
        };

        memory.update_memories(uplink.clone(), info_channel_downlink).await?;

        Ok(memory)
    }

    async fn update_memories(&mut self, uplink: channel::Sender<Packet>, downlink: channel::Receiver<Packet>) -> Result<()> {
      let pk = Packet::new(MEMORY_PORT, INFO_CHANNEL, vec![CMD_INFO_NBR]);
      uplink
          .send_async(pk)
          .await
          .map_err(|_| Error::Disconnected)?;

      let pk = downlink.wait_packet(MEMORY_PORT, INFO_CHANNEL, &[CMD_INFO_NBR]).await?;
      let memory_count = pk.get_data()[1];

      for i in 0..memory_count {
        let pk = Packet::new(MEMORY_PORT, INFO_CHANNEL, vec![CMD_INFO_DETAILS, i]);
        uplink
            .send_async(pk)
            .await
            .map_err(|_| Error::Disconnected)?;

        let pk = downlink.wait_packet(MEMORY_PORT, INFO_CHANNEL, &[CMD_INFO_DETAILS, i]).await?;
        let data = pk.get_data();
        let memory_id = data[1];
        let memory_type = MemoryType::try_from(data[2])?;
        let memory_size = u32::from_le_bytes(data[3..7].try_into()?);

        self.memories.push(MemoryDevice {
          memory_id: memory_id,
          memory_type: memory_type,
          size: memory_size
        });

        self.backends.push(Mutex::new(Some(MemoryBackend {
          memory_id: memory_id,
          memory_type: memory_type,
          uplink: uplink.clone(),
          read_downlink: self.memory_read_dispatcher.get_channel(memory_id).await,
          write_downlink: self.memory_write_dispatcher.get_channel(memory_id).await,
        })));
      }
      Ok(())

    }

    /// Get the list of memories in the Crazyflie, optionally filtered by type.
    /// 
    /// If `memory_type` is None, all memories are returned
    /// If `memory_type` is Some(type), only memories of that type are returned
    /// # Example
    /// ```no_run
    /// let memories = memory.get_memories(Some(MemoryType::OneWire));
    /// ```
    /// # Example
    /// ```no_run
    /// let memories = memory.get_memories(None);
    /// ```
    /// # Returns
    /// A vector of references to MemoryDevice structs
    /// If no memories are found, an empty vector is returned
    pub fn get_memories(&self, memory_type: Option<MemoryType>) -> Vec<&MemoryDevice> {
      match memory_type {
        Some(ty) => self.memories.iter().filter(|m| m.memory_type == ty).collect(),
        None => self.memories.iter().collect(),
      }
    }

    /// Get a specific memory by its ID
    /// 
    /// # Arguments
    /// * `memory` - The MemoryDevice struct representing the memory to get
    /// # Returns
    /// An Option containing a reference to the MemoryDevice struct if found, or None if not found
    pub async fn open_memory<T: FromMemoryBackend>(&self, memory: MemoryDevice) -> Option<Result<T>> {
      let backend = self.backends.get(memory.memory_id as usize)?.lock().await.take()?;
      Some(T::from_memory_backend(backend).await)
    }

    /// Close a memory
    /// 
    /// # Arguments
    /// * `memory_device` - The MemoryDevice struct representing the memory to close
    /// * `backend` - The MemoryBackend to return to the subsystem
    pub async fn close_memory<T: FromMemoryBackend>(&self, device: T) {
      let backend = device.close_memory();
      if let Some(mutex) = self.backends.get(backend.memory_id as usize) {
        let mut guard = mutex.lock().await;
        if guard.is_none() {
          *guard = Some(backend);
        } else {
          println!("Warning: Attempted to close memory ID {} which is already closed", backend.memory_id);
        }
      } else {
        println!("Warning: Attempted to close memory ID {} which does not exist", backend.memory_id);
      }
    }

    /// Get a specific memory by its ID and initialize it according to the defaults. Note that the
    /// values will not be written to the memory by default, the user needs to handle this.
    /// 
    /// # Arguments
    /// * `memory` - The MemoryDevice struct representing the memory to get
    /// # Returns
    /// An Option containing a reference to the MemoryDevice struct if found, or None if not found
    pub async fn initialize_memory<T: FromMemoryBackend>(&self, memory: MemoryDevice) -> Option<Result<T>> {
        let backend = self.backends.get(memory.memory_id as usize)?.lock().await.take()?;
        Some(T::initialize_memory_backend(backend).await)
    }

}