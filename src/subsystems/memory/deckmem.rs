use std::{sync::Arc, time::Duration};

use crate::{
    Error, Result,
    subsystems::memory::{MemoryBackend, memory_types},
};
use memory_types::{FromMemoryBackend, MemoryType};
use tokio::{sync::Mutex, time::sleep};

const DECKMEM_VERSION_REQUIREMENT: u8 = 3;

// Bit field 1 masks (0x0000)
const IS_VALID_MASK: u8 = 0x01;
const IS_STARTED_MASK: u8 = 0x02;
const SUPPORTS_READ_MASK: u8 = 0x04;
const SUPPORTS_WRITE_MASK: u8 = 0x08;
const SUPPORTS_UPGRADE_MASK: u8 = 0x10;
const UPGRADE_REQUIRED_MASK: u8 = 0x20;
const BOOTLOADER_ACTIVE_MASK: u8 = 0x40;

// Bit field 2 masks (0x0001)
const CAN_RESET_TO_FIRMWARE_MASK: u8 = 0x01;
const CAN_RESET_TO_BOOTLOADER_MASK: u8 = 0x02;

const DECKMEM_MAX_SECTIONS: usize = 8;
const DECKMEM_INFO_OFFSET: usize = 1;
const DECKMEM_INFO_SIZE: usize = 0x20;
const DECKMEM_CMD_OFFSET: usize = 0x1000;
const DECKMEM_CMD_SIZE: usize = 0x20;
const DECKMEM_CMD_BITS_OFFSET: usize = 0x4;

const DECKMEM_CMD_RST_TO_FIRMWARE: u8 = 0x01;
const DECKMEM_CMD_RST_TO_BOOTLOADER: u8 = 0x02;

/// Describes the content of a Crazyflie deck memory used to access the deck firmware and bootloaders
#[derive(Debug)]
pub struct DeckMemory {
    /// Thread-safe reference to the underlying memory backend
    memory: Arc<Mutex<MemoryBackend>>,
    /// The memory sections available in the deck memory (each one corresponds to the primary
    /// or secondary memory of a deck)
    sections: Vec<DeckMemorySection>,
}

impl FromMemoryBackend for DeckMemory {
    async fn from_memory_backend(memory: MemoryBackend) -> Result<Self> {
        if memory.memory_type == MemoryType::DeckMemory {
            Ok(DeckMemory::new(memory).await?)
        } else {
            Err(Error::MemoryError("Wrong type of memory!".to_owned()))
        }
    }

    async fn initialize_memory_backend(_memory: MemoryBackend) -> Result<Self> {
        Err(Error::MemoryError(
            "Memory does not support initializing".to_owned(),
        ))
    }

    fn close_memory(mut self) -> MemoryBackend {
        // Drop all sections to release their Arc references
        self.sections.clear();

        // Return backend
        Arc::try_unwrap(self.memory)
            .map_err(|_arc| {
                Error::MemoryError(format!("Multiple references to memory"))
            })
            .map(|mutex| mutex.into_inner())
            .expect("Multiple reference to sections still held")
    }
}

#[derive(Debug)]
/// Represents a memory section for a deck in the Crazyflie system.
///
/// This structure contains information about a deck's memory configuration,
/// including its capabilities (read, write, upgrade) and memory layout (addresses
/// for base, command, and info).
pub struct DeckMemorySection {
    /// Whether the deck supports read operations
    supports_read: bool,
    /// Whether the deck supports write operations
    supports_write: bool,
    /// Whether the deck supports firmware upgrades
    supports_upgrade: bool,
    /// Whether the deck can be reset to run firmware
    can_reset_to_firmware: bool,
    /// Whether the deck can be reset to bootloader mode
    can_reset_to_bootloader: bool,
    /// Optional hash value required for firmware validation
    required_hash: Option<u32>,
    /// Optional expected length of the firmware
    required_length: Option<u32>,
    /// The base memory address for this deck section
    base_address: usize,
    /// The memory address used for sending commands
    command_address: usize,
    /// The memory address containing deck information
    info_address: usize,
    /// The name identifier of the deck
    name: String,
    /// Thread-safe reference to the underlying memory backend
    memory: Arc<Mutex<MemoryBackend>>,
}

impl DeckMemorySection {
    async fn from_bytes(
        memory: Arc<Mutex<MemoryBackend>>,
        info_offset: usize,
        command_address: usize,
    ) -> Result<Option<Self>> {
        let data = memory
            .lock()
            .await
            .read::<fn(usize, usize)>(info_offset, DECKMEM_INFO_SIZE, None)
            .await?;

        // Validate minimum data length for parsing so we don't panic later
        if data.len() < DECKMEM_INFO_SIZE {
            return Ok(None);
        }

        // Only cache data which is not changed between restarts of the Crazyflie

        let bit_field_1 = data[0];
        let is_valid = (bit_field_1 & IS_VALID_MASK) != 0;
        let supports_read = (bit_field_1 & SUPPORTS_READ_MASK) != 0;
        let supports_write = (bit_field_1 & SUPPORTS_WRITE_MASK) != 0;
        let supports_upgrade = (bit_field_1 & SUPPORTS_UPGRADE_MASK) != 0;

        let bit_field_2 = data[1];
        let can_reset_to_firmware = (bit_field_2 & CAN_RESET_TO_FIRMWARE_MASK) != 0;
        let can_reset_to_bootloader = (bit_field_2 & CAN_RESET_TO_BOOTLOADER_MASK) != 0;

        let required_hash = u32::from_le_bytes([data[2], data[3], data[4], data[5]]);
        let required_length = u32::from_le_bytes([data[6], data[7], data[8], data[9]]);
        let base_address = u32::from_le_bytes([data[10], data[11], data[12], data[13]]);

        // Parse name (offset 0x000E / 14, up to 19 bytes including null terminator, zero terminated)
        let name_bytes = &data[14..32];
        let name = name_bytes
            .iter()
            .take_while(|&&b| b != 0)
            .copied()
            .collect::<Vec<u8>>();
        let name = String::from_utf8_lossy(&name).to_string();

        if is_valid {
            Ok(Some(DeckMemorySection {
                supports_read,
                supports_write,
                supports_upgrade,
                can_reset_to_firmware,
                can_reset_to_bootloader,
                required_hash: match required_hash {
                    0 => None,
                    v => Some(v),
                },
                required_length: match required_length {
                    0 => None,
                    v => Some(v),
                },
                base_address: base_address as usize,
                command_address: command_address,
                info_address: info_offset,
                name,
                memory,
            }))
        } else {
            Ok(None)
        }
    }

    async fn read_info_byte(&self) -> Result<u8> {
        let data = self.memory
            .lock()
            .await
            .read::<fn(usize, usize)>(self.info_address, 1, None)
            .await?;
        Ok(data[0])
    }

    /// Returns whether the deck has been started.
    pub async fn is_started(&self) -> Result<bool> {
        let data = self.read_info_byte().await?;
        Ok((data & IS_STARTED_MASK) != 0)
    }

    /// Returns whether this deck supports read operations.
    pub fn supports_read(&self) -> bool {
        self.supports_read
    }

    /// Returns whether this deck supports write operations.
    pub fn supports_write(&self) -> bool {
        self.supports_write
    }

    /// Returns whether this deck supports firmware upgrades.
    pub fn supports_upgrade(&self) -> bool {
        self.supports_upgrade
    }

    /// Returns whether a firmware upgrade is required for this deck.
    pub async fn upgrade_required(&self) -> Result<bool> {
        let data = self.read_info_byte().await?;
        Ok((data & UPGRADE_REQUIRED_MASK) != 0)
    }

    /// Returns whether the bootloader is currently active on this deck.
    pub async fn bootloader_active(&self) -> Result<bool> {
        let data = self.read_info_byte().await?;
        Ok((data & BOOTLOADER_ACTIVE_MASK) != 0)
    }

    /// Returns whether this deck can be reset to firmware mode.
    pub fn can_reset_to_firmware(&self) -> bool {
        self.can_reset_to_firmware
    }

    /// Returns whether this deck can be reset to bootloader mode.
    pub fn can_reset_to_bootloader(&self) -> bool {
        self.can_reset_to_bootloader
    }

    /// Returns the required hash for firmware verification, if any.
    pub fn required_hash(&self) -> Option<u32> {
        self.required_hash
    }

    /// Returns the required firmware length, if any.
    pub fn required_length(&self) -> Option<u32> {
        self.required_length
    }

    /// Returns the name of this memory section.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Reset the MCU connected to this memory section into bootloader mode.
    ///
    /// # Returns
    /// A `Result` indicating success or failure of the reset operation
    /// # Errors
    /// Returns an `Error` if the section does not support resetting to bootloader
    /// or if the reset operation fails
    pub async fn reset_to_bootloader(&self) -> Result<()> {
        if !self.can_reset_to_bootloader {
            return Err(Error::MemoryError(
                "Section cannot reset to bootloader".to_owned(),
            ));
        }

        // Write to specific address to trigger reset
        self.memory
            .lock()
            .await
            .write::<fn(usize, usize)>(self.command_address + DECKMEM_CMD_BITS_OFFSET, &[DECKMEM_CMD_RST_TO_BOOTLOADER], None)
            .await?;

        // Sleep for 10 ms to allow the reset to complete
        sleep(Duration::from_millis(10)).await;

        Ok(())
    }

    /// Reset the MCU connected to this memory section into firmware mode.
    ///
    /// # Returns
    /// A `Result` indicating success or failure of the reset operation
    /// # Errors
    /// Returns an `Error` if the section does not support resetting to firmware
    /// or if the reset operation fails
    pub async fn reset_to_firmware(&self) -> Result<()> {
        if !self.can_reset_to_firmware {
            return Err(Error::MemoryError(
                "Section cannot reset to firmware".to_owned(),
            ));
        }

        // Write to specific address to trigger reset
        self.memory
            .lock()
            .await
            .write::<fn(usize, usize)>(self.command_address + DECKMEM_CMD_BITS_OFFSET, &[DECKMEM_CMD_RST_TO_FIRMWARE], None)
            .await?;

        // Sleep for 10 ms to allow the reset to complete
        sleep(Duration::from_millis(10)).await;

        Ok(())
    }

    /// Write data to the memory section at the specified address.
    ///
    /// # Arguments
    /// * `address` - The address within the memory section to write to.
    /// * `data` - The data to write.
    ///
    /// # Returns
    /// A `Result` indicating success or failure of the write operation.
    /// # Errors
    /// Returns an `Error` if the section does not support writing or if the write operation fails.
    pub async fn write(&self, address: usize, data: &[u8]) -> Result<()> {
        if !self.supports_write {
            return Err(Error::MemoryError(
                "Section does not support write".to_owned(),
            ));
        }

        self.memory
            .lock()
            .await
            .write::<fn(usize, usize)>(self.base_address + address, data, None)
            .await
    }

    /// Write data to the memory section at the specified address with progress reporting.
    ///
    /// # Arguments
    /// * `address` - The address within the memory section to write to.
    /// * `data` - The data to write.
    /// * `progress_callback` - A callback function that takes two usize arguments:
    ///   the number of bytes written so far and the total number of bytes to write.
    ///
    /// # Returns
    /// A `Result` indicating success or failure of the write operation.
    /// # Errors
    /// Returns an `Error` if the section does not support writing or if the write operation fails.
    pub async fn write_with_progress<F>(
        &self,
        address: usize,
        data: &[u8],
        progress_callback: F,
    ) -> Result<()>
    where
        F: FnMut(usize, usize),
    {
        if !self.supports_write {
            return Err(Error::MemoryError(
                "Section does not support write".to_owned(),
            ));
        }

        self.memory
            .lock()
            .await
            .write(self.base_address + address, data, Some(progress_callback))
            .await
    }

    /// Read data from the memory section at the specified address.
    ///
    /// # Arguments
    /// * `address` - The address within the memory section to read from.
    /// * `length` - The number of bytes to read.
    /// # Returns
    /// A `Result` containing a vector of bytes read from the memory section or an `Error` if the operation fails.
    pub async fn read(&self, address: usize, length: usize) -> Result<Vec<u8>> {
        if !self.supports_read {
            return Err(Error::MemoryError(
                "Section does not support read".to_owned(),
            ));
        }

        self
            .memory
            .lock()
            .await
            .read::<fn(usize, usize)>(self.base_address + address, length, None)
            .await
    }

    /// Read data from the memory section at the specified address with progress reporting.
    ///
    /// # Arguments
    /// * `address` - The address within the memory section to read from.
    /// * `length` - The number of bytes to read.
    /// * `progress_callback` - A callback function that takes two usize arguments:
    ///   the number of bytes read so far and the total number of bytes to read.
    /// # Returns
    /// A `Result` containing a vector of bytes read from the memory section or an `Error` if the operation fails.
    pub async fn read_with_progress<F>(
        &self,
        address: usize,
        length: usize,
        progress_callback: F,
    ) -> Result<Vec<u8>>
    where
        F: FnMut(usize, usize),
    {
        if !self.supports_read {
            return Err(Error::MemoryError(
                "Section does not support read".to_owned(),
            ));
        }

        self.memory
            .lock()
            .await
            .read(self.base_address + address, length, Some(progress_callback))
            .await
    }
}

impl DeckMemory {
    pub(crate) async fn new(memory: MemoryBackend) -> Result<Self> {
        let sharable_memory = Arc::new(Mutex::new(memory));

        let info = sharable_memory
            .lock()
            .await
            .read::<fn(usize, usize)>(0, 1, None)
            .await?;

        // Parse version byte
        let version = info[0];
        if version != DECKMEM_VERSION_REQUIREMENT {
            return Err(Error::MemoryError(format!(
                "Unsupported deck memory version: {}",
                version
            )));
        }

        let mut sections: Vec<DeckMemorySection> = Vec::new();
        for i in 0..DECKMEM_MAX_SECTIONS {
            let info_base = DECKMEM_INFO_OFFSET + i * DECKMEM_INFO_SIZE;
            let cmd_base = DECKMEM_CMD_OFFSET + i * DECKMEM_CMD_SIZE;
            if let Some(section) =
                DeckMemorySection::from_bytes(sharable_memory.clone(), info_base, cmd_base).await?
            {
                sections.push(section);
            }
        }

        Ok(DeckMemory {
            memory: sharable_memory,
            sections,
        })
    }

    /// Get all memory sections available in this deck memory.
    /// # Returns
    /// A slice of `DeckMemorySection` representing all available sections.
    pub fn sections(&self) -> &[DeckMemorySection] {
        &self.sections
    }

    /// Get a memory section by name.
    /// # Arguments
    /// * `name` - The name of the memory section to retrieve.
    /// # Returns
    /// An `Option` containing a reference to the `DeckMemorySection` if found, or `None` if not found.
    pub fn section(&self, name: &str) -> Option<&DeckMemorySection> {
        self.sections.iter().find(|s| s.name == name)
    }
}
