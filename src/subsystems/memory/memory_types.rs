use crate::{crtp_utils::WaitForPacket, Error, Result};
use crazyflie_link::Packet;
use flume as channel;
use std::convert::{TryFrom, TryInto};

use crate::crazyflie::MEMORY_PORT;

const READ_CHANNEL: u8 = 1;
const WRITE_CHANNEL: u8 = 2;

const MEM_MAX_READ_REQUEST_SIZE: usize = 20;

/// Description of a memory in the Crazyflie
#[derive(Debug, Clone)]
pub struct MemoryDevice {
    /// Unique identifier for this memory subsystem (used when reading/writing/querying)
    pub memory_id: u8,
    /// Type of memory
    pub memory_type: MemoryType,
    /// Size of the memory in bytes
    pub size: u32,

    pub(crate) uplink: channel::Sender<Packet>,
    //TODO: Lock this!
    pub(crate) read_downlink: channel::Receiver<Packet>,
    pub(crate) write_downlink: channel::Receiver<Packet>,
}

impl MemoryDevice {
    pub(crate) async fn read(&self, address: usize, length: usize) -> Result<Vec<u8>> {
        // memory::ReadMemory::new(self.uplink.clone(), self.read_downlink.clone(), memory_id, offset, length)
        let mut data = vec![0; length];
        let mut current_address = address;
        while current_address < address + length {
            let to_read = std::cmp::min(
                MEM_MAX_READ_REQUEST_SIZE,
                (address + length) - current_address,
            );
            // dbg!(&to_read);
            let mut request_data = Vec::new();
            request_data.extend_from_slice(&self.memory_id.to_le_bytes());
            request_data.extend_from_slice(&(current_address as u32).to_le_bytes());
            request_data.extend_from_slice(&(to_read as u8).to_le_bytes());
            let pk = Packet::new(MEMORY_PORT, READ_CHANNEL, request_data.clone());
            // dbg!(&pk);
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
                // dbg!(&pk);
                let pk_data = pk.get_data();
                let read_address = u32::from_le_bytes(pk_data[1..5].try_into().unwrap());
                // println!("Length: {}, address: {}, read: {}", pk_data.len(), read_address, pk_data.len() - 6);
                // println!("Stats: {}", pk_data[4]);
                if pk_data.len() >= 5 && read_address == current_address as u32 {
                    let read_data = &pk_data[6..];
                    let start = (current_address - address) as usize;
                    let end = start + read_data.len();
                    data[start..end].copy_from_slice(read_data);
                } else {
                    println!("Warning: Malformed memory read response");
                }
            } else {
                println!("Warning: Failed to read memory");
            }

            current_address += to_read;
        }

        Ok(data)
    }

    pub(crate) async fn write(&self, address: usize, data: &[u8]) -> Result<()> {
        let mut current_address = address;
        let length = data.len();
        while current_address < address + length {
            let to_write = std::cmp::min(
                MEM_MAX_READ_REQUEST_SIZE,
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

            let pk = self
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
pub trait FromMemoryDevice: Sized {
    /// Create a memory-specific type from a `MemoryDevice`. When created the
    /// memory is automatically read to populate the fields of the type.
    /// 
    /// # Arguments
    /// * `memory` - The `MemoryDevice` to read from
    /// # Returns
    /// A `Result` containing the constructed type or an `Error` if the operation fails
    async fn from_memory_device(memory: MemoryDevice) -> Result<Self>;

    async fn initialize_memory_device(memory: MemoryDevice) -> Result<Self>;
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
            MemoryType::DeckMultiranger => "Deck Multiranger",
            MemoryType::DeckPaa3905 => "Deck PAA3905",
            MemoryType::UNKNOWN => "Unknown",
        };
        write!(f, "{}", name)
    }
}
