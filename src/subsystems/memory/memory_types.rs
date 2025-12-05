use crate::{crtp_utils::WaitForPacket, Error, Result};
use crazyflie_link::Packet;
use flume as channel;
use std::convert::{TryFrom, TryInto};

use crate::crazyflie::MEMORY_PORT;

const READ_CHANNEL: u8 = 1;
const WRITE_CHANNEL: u8 = 2;

const MEM_MAX_REQUEST_SIZE: usize = 24;


/// Description of a memory in the Crazyflie
#[derive(Debug)]
pub struct MemoryBackend {
    /// Unique identifier for this memory subsystem (used when reading/writing/querying)
    pub memory_id: u8,
    /// Type of memory
    pub memory_type: MemoryType,

    pub(crate) uplink: channel::Sender<Packet>,
    pub(crate) read_downlink: channel::Receiver<Packet>,
    pub(crate) write_downlink: channel::Receiver<Packet>,
}
/// Description of a memory in the Crazyflie
#[derive(Debug, Clone)]
pub struct MemoryDevice {
    /// Unique identifier for this memory subsystem (used when reading/writing/querying)
    pub memory_id: u8,
    /// Type of memory
    pub memory_type: MemoryType,
    /// Size of the memory in bytes
    pub size: u32
}

impl MemoryBackend {
    pub(crate) async fn read<F>(&self, address: usize, length: usize, mut progress_callback: Option<F>) -> Result<Vec<u8>>
    where
        F: FnMut(usize, usize),
    {
        let mut data = vec![0; length];
        let mut current_address = address;
        while current_address < address + length {
            let to_read = std::cmp::min(
                MEM_MAX_REQUEST_SIZE,
                (address + length) - current_address,
            );
            let mut request_data = Vec::new();
            request_data.extend_from_slice(&self.memory_id.to_le_bytes());
            request_data.extend_from_slice(&(current_address as u32).to_le_bytes());
            request_data.extend_from_slice(&(to_read as u8).to_le_bytes());
            let pk = Packet::new(MEMORY_PORT, READ_CHANNEL, request_data.clone());
            self.uplink
                .send_async(pk)
                .await
                .map_err(|_| Error::Disconnected)
                .ok();

            let pk = self
                .read_downlink
                .wait_packet(MEMORY_PORT, READ_CHANNEL, &request_data[0..5])
                .await;
            if let Ok(pk) = pk {
                let pk_data = pk.get_data();

                if pk_data.len() < 6 {
                    return Err(Error::MemoryError("Malformed memory read response".into()));
                }

                let read_address = u32::from_le_bytes(pk_data[1..5].try_into().unwrap());
                let status = pk_data[5];

                if pk_data.len() >= 6 && read_address == current_address as u32 && status == 0 {
                    let read_data = &pk_data[6..];
                    let start = (current_address - address) as usize;
                    let end = start + read_data.len();
                    data[start..end].copy_from_slice(read_data);
                } else if status != 0 {
                    return Err(Error::MemoryError(format!("Memory read returned error status ({}) @ {}", status, current_address)));
                } else {
                    return Err(Error::MemoryError("Malformed memory read response".into()));
                }
            } else {
                return Err(Error::MemoryError("Failed to read memory".into()));
            }

            current_address += to_read;

            if let Some(ref mut callback) = progress_callback {
                callback(current_address - address, length);
            }
            
        }

        Ok(data)
    }

    pub(crate) async fn write<F>(&self, address: usize, data: &[u8], mut progress_callback: Option<F>) -> Result<()>
    where
        F: FnMut(usize, usize),
    {
        let mut current_address = address;
        let length = data.len();
        while current_address < address + length {
            let to_write = std::cmp::min(
                MEM_MAX_REQUEST_SIZE,
                (address + length) - current_address,
            );
            let start = (current_address - address) as usize;
            let end = start + to_write;
            let mut request_data = Vec::new();
            request_data.extend_from_slice(&self.memory_id.to_le_bytes());
            request_data.extend_from_slice(&(current_address as u32).to_le_bytes());
            request_data.extend_from_slice(&data[start..end]);
            let pk = Packet::new(MEMORY_PORT, WRITE_CHANNEL, request_data.clone());

            self.uplink
                .send_async(pk)
                .await
                .map_err(|_| Error::Disconnected)
                .ok();
            current_address += to_write;

            if let Some(ref mut callback) = progress_callback {
                callback(current_address - address, length);
            }

            let _pk = self
                .write_downlink
                .wait_packet(MEMORY_PORT, WRITE_CHANNEL, &request_data[0..5])
                .await;
        }
        Ok(())
    }
}

/// A trait for types that can be constructed from a memory device.
///
/// This trait provides an asynchronous interface for creating instances of types
/// from a `MemoryDevice`. Implementors should define how to read and parse data
/// from the memory device to construct their specific type.
///
/// # Errors
///
/// Implementations should return an error if:
/// - The memory device cannot be accessed
/// - The data format is invalid or corrupted
/// - Required data is missing from the memory device
pub trait FromMemoryBackend: Sized {
    /// Create a memory-specific type from a `MemoryBackend`. When created the
    /// memory is automatically read to populate the fields of the type.
    /// 
    /// # Arguments
    /// * `memory` - The `MemoryBackend` to read from
    /// # Returns
    /// A `Result` containing the constructed type or an `Error` if the operation fails
    fn from_memory_backend(memory: MemoryBackend) -> impl std::future::Future<Output = Result<Self>> + Send;

    /// Get a specific memory by its ID and initialize it according to the defaults. Note that the
    /// values will not be written to the memory by default, the user needs to handle this.
    /// 
    /// # Arguments
    /// * `memory` - The MemoryDevice struct representing the memory to get
    /// # Returns
    /// An Option containing a reference to the MemoryDevice struct if found, or None if not found
    fn initialize_memory_backend(memory: MemoryBackend) -> impl std::future::Future<Output = Result<Self>> + Send;

    /// Close the memory and return the backend to the subsystem
    ///
    /// # Arguments
    /// * `memory_device` - The MemoryDevice struct representing the memory to close
    /// * `backend` - The MemoryBackend to return to the subsystem
    fn close_memory(self) -> MemoryBackend;
}

/// The memory types supported by the Crazyflie
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryType {
    /// The I2C EEPROM configuration memory on the Crazyflie
    EEPROMConfig = 0x00,
    /// 1-Wire memory type on the decks
    OneWire = 0x01,
    /// Driver LED memory type
    DriverLed = 0x10,
    /// Loco memory type
    Loco = 0x11,
    /// Trajectory memory type
    Trajectory = 0x12,
    /// Loco2 memory type
    Loco2 = 0x13,
    /// Lighthouse memory type
    Lighthouse = 0x14,
    /// Memory tester type
    MemoryTester = 0x15,
    /// Driver LED timing type
    DriverLedTiming = 0x17,
    /// Application memory type
    App = 0x18,
    /// Deck memory type
    DeckMemory = 0x19,
    /// Deck Ctrl memory type
    DeckCtrlDFU = 0x20,
    /// Deck multi-ranger memory type
    DeckMultiranger = 0x1A,
    /// Deck PAA3905 memory type
    DeckPaa3905 = 0x1B,
    /// Unknown memory type (defaults to this if an unrecognized type is encountered)
    UNKNOWN = 0xFF,
}

impl TryFrom<u8> for MemoryType {
    type Error = Error;

    fn try_from(value: u8) -> Result<Self> {
        match value {
            0x00 => Ok(MemoryType::EEPROMConfig),
            0x01 => Ok(MemoryType::OneWire),
            0x10 => Ok(MemoryType::DriverLed),
            0x11 => Ok(MemoryType::Loco),
            0x12 => Ok(MemoryType::Trajectory),
            0x13 => Ok(MemoryType::Loco2),
            0x14 => Ok(MemoryType::Lighthouse),
            0x15 => Ok(MemoryType::MemoryTester),
            0x17 => Ok(MemoryType::DriverLedTiming),
            0x18 => Ok(MemoryType::App),
            0x19 => Ok(MemoryType::DeckMemory),
            0x20 => Ok(MemoryType::DeckCtrlDFU),
            0x1A => Ok(MemoryType::DeckMultiranger),
            0x1B => Ok(MemoryType::DeckPaa3905),
            _ => Ok(MemoryType::UNKNOWN),
        }
    }
}

impl std::fmt::Display for MemoryType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            MemoryType::EEPROMConfig => "EEPROM config",
            MemoryType::OneWire => "1-Wire",
            MemoryType::DriverLed => "Driver LED",
            MemoryType::Loco => "Loco",
            MemoryType::Trajectory => "Trajectory",
            MemoryType::Loco2 => "Loco2",
            MemoryType::Lighthouse => "Lighthouse",
            MemoryType::MemoryTester => "Memory Tester",
            MemoryType::DriverLedTiming => "Driver LED Timing",
            MemoryType::App => "Application",
            MemoryType::DeckMemory => "Deck Memory",
            MemoryType::DeckCtrlDFU => "Deck Ctrl DFU",
            MemoryType::DeckMultiranger => "Deck Multiranger",
            MemoryType::DeckPaa3905 => "Deck PAA3905",
            MemoryType::UNKNOWN => "Unknown",
        };
        write!(f, "{}", name)
    }
}
