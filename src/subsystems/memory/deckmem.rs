use std::{sync::Arc, time::Duration};

use crate::{
    Error, Result,
    subsystems::memory::{MemoryBackend, memory_types},
};
use memory_types::{FromMemoryBackend, MemoryType};
use tokio::{sync::Mutex, time::sleep};

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

    fn close_memory(self) -> MemoryBackend {
        // Drop all sections and return the backend

        Arc::try_unwrap(self.memory)
            .expect("Multiple references to memory")
            .into_inner()
    }
}

#[derive(Debug, Clone)]
/// Represents a memory section for a deck in the Crazyflie system.
///
/// This structure contains information about a deck's memory configuration,
/// including its capabilities (read, write, upgrade), state (bootloader active,
/// upgrade required), and memory layout (addresses for base, command, and info).
pub struct DeckMemorySection {
    /// Indicates whether the deck has been started
    is_started: bool,
    /// Whether the the deck supports read operations
    supports_read: bool,
    /// Whether the the deck supports write operations
    supports_write: bool,
    /// Whether the the deck supports firmware upgrades
    supports_upgrade: bool,
    /// Whether a firmware upgrade is required for this deck
    upgrade_required: bool,
    /// Whether the deck is currently running in bootloader mode
    bootloader_active: bool,
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
            .read::<fn(usize, usize)>(info_offset, 0x20, None)
            .await?;

        // Validate minimum data length for parsing
        if data.len() < 0x20 {
            println!(
                "DeckMemorySection: Insufficient data length: {}",
                data.len()
            );
            return Ok(None);
        }

        // Parse bit field 1 (0x0000)
        let bit_field_1 = data[0];
        let is_valid = (bit_field_1 & 0x01) != 0;
        let is_started = (bit_field_1 & 0x02) != 0;
        let supports_read = (bit_field_1 & 0x04) != 0;
        let supports_write = (bit_field_1 & 0x08) != 0;
        let supports_upgrade = (bit_field_1 & 0x10) != 0;
        let upgrade_required = (bit_field_1 & 0x20) != 0;
        let bootloader_active = (bit_field_1 & 0x40) != 0;

        // Parse bit field 2 (0x0001)
        let bit_field_2 = data[1];
        let can_reset_to_firmware = (bit_field_2 & 0x01) != 0;
        let can_reset_to_bootloader = (bit_field_2 & 0x02) != 0;

        // Parse required hash (0x0002, uint32)
        let required_hash = u32::from_le_bytes([data[2], data[3], data[4], data[5]]);

        // Parse required length (0x0006, uint32)
        let required_length = u32::from_le_bytes([data[6], data[7], data[8], data[9]]);

        // Parse base address (0x000A, uint32)
        let base_address = u32::from_le_bytes([data[10], data[11], data[12], data[13]]);

        // Parse name (0x000E, 18 bytes max, zero terminated)
        let name_bytes = &data[14..33.min(data.len())];
        let name = name_bytes
            .iter()
            .take_while(|&&b| b != 0)
            .copied()
            .collect::<Vec<u8>>();
        let name = String::from_utf8_lossy(&name).to_string();

        if is_valid {
            Ok(Some(DeckMemorySection {
                is_started,
                supports_read,
                supports_write,
                supports_upgrade,
                upgrade_required,
                bootloader_active,
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

    async fn update_status_bits(&mut self) -> Result<()> {
        let data = self
            .memory
            .lock()
            .await
            .read::<fn(usize, usize)>(self.info_address, 2, None)
            .await?;
        if data.len() < 1 {
            return Err(Error::MemoryError("Failed to read status bits".to_owned()));
        }

        // Parse bit field 1 (0x0000)
        let bit_field_1 = data[0];
        self.bootloader_active = (bit_field_1 & 0x40) != 0;

        // Parse bit field 2 (0x0001)
        let bit_field_2 = data[1];
        self.can_reset_to_firmware = (bit_field_2 & 0x01) != 0;
        self.can_reset_to_bootloader = (bit_field_2 & 0x02) != 0;

        Ok(())
    }

    /// Returns whether the deck has been started.
    pub fn is_started(&self) -> bool {
        self.is_started
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
    pub fn upgrade_required(&self) -> bool {
        self.upgrade_required
    }

    /// Returns whether the bootloader is currently active on this deck.
    pub fn bootloader_active(&self) -> bool {
        self.bootloader_active
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
    pub async fn reset_to_bootloader(&mut self) -> Result<()> {
        if !self.can_reset_to_bootloader {
            return Err(Error::MemoryError(
                "Section cannot reset to bootloader".to_owned(),
            ));
        }

        // Write to specific address to trigger reset
        self.memory
            .lock()
            .await
            .write::<fn(usize, usize)>(self.command_address + 4, &[0x02u8], None)
            .await?;

        // Sleep for 10 ms to allow the reset to complete
        sleep(Duration::from_millis(10)).await;

        self.update_status_bits().await
    }

    /// Reset the MCU connected to this memory section into firmware mode.
    ///
    /// # Returns
    /// A `Result` indicating success or failure of the reset operation
    /// # Errors
    /// Returns an `Error` if the section does not support resetting to firmware
    /// or if the reset operation fails
    pub async fn reset_to_firmware(&mut self) -> Result<()> {
        if !self.can_reset_to_firmware {
            return Err(Error::MemoryError(
                "Section cannot reset to firmware".to_owned(),
            ));
        }

        // Write to specific address to trigger reset
        self.memory
            .lock()
            .await
            .write::<fn(usize, usize)>(self.command_address + 4, &[0x01u8], None)
            .await?;

        // Sleep for 10 ms to allow the reset to complete
        sleep(Duration::from_millis(10)).await;

        self.update_status_bits().await
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
    pub async fn write(&self, address: usize, data: &Vec<u8>) -> Result<()> {
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
        data: &Vec<u8>,
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
        if !self.supports_write {
            return Err(Error::MemoryError(
                "Section does not support write".to_owned(),
            ));
        }

        return self
            .memory
            .lock()
            .await
            .read::<fn(usize, usize)>(self.base_address + address, length, None)
            .await;
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
        if version != 3 {
            return Err(Error::MemoryError(format!(
                "Unsupported deck memory version: {}",
                version
            )));
        }

        let mut sections: Vec<DeckMemorySection> = Vec::new();
        for i in 0..8 {
            let info_base = 1 + i * 0x20;
            let cmd_base = 0x1000 + i * 0x10;
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
    /// A a vector of `DeckMemorySection` representing all available sections.
    pub fn sections(&self) -> Vec<DeckMemorySection> {
        self.sections.clone()
    }

    /// Get a memory section by name.
    ///  /// # Arguments
    /// * `name` - The name of the memory section to retrieve.
    /// # Returns
    /// An `Option` containing a reference to the `DeckMemorySection` if found, or `None` if not found.
    pub fn section(&self, name: &str) -> Option<&DeckMemorySection> {
        self.sections.iter().find(|s| s.name == name)
    }
}
