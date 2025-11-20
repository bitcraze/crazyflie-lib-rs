use crate::{subsystems::memory::{memory_types, MemoryBackend}, Result};

use memory_types::{FromMemoryBackend};

/// This is used to get raw memory access to any memory device.
pub struct RawMemory {
    memory: MemoryBackend,
}

impl FromMemoryBackend for RawMemory {
    async fn from_memory_backend(memory: MemoryBackend) -> Result<Self> {
        Ok(Self { memory })
    }

    async fn initialize_memory_backend(memory: MemoryBackend) -> Result<Self> {
        Ok(Self { memory })
    }

    fn close_memory(self) -> MemoryBackend {
      self.memory
    }
}

impl RawMemory {
    /// Read raw data from the memory device at the specified address and length.
    /// 
    /// # Arguments
    /// * `address` - The starting address to read from
    /// * `length` - The number of bytes to read
    /// # Returns
    /// A `Result` containing a vector of bytes read from the memory or an `Error
    /// if the operation fails
    pub async fn read(&self, address: usize, length: usize) -> Result<Vec<u8>> {
        self.memory.read::<fn(usize, usize)>(address, length, None).await
    }

    /// Read raw data from the memory device at the specified address and length and
    /// report progress via the provided callback.
    /// 
    /// # Arguments
    /// * `address` - The starting address to read from
    /// * `length` - The number of bytes to read
    /// * `progress_callback` - A callback function that takes two usize arguments:
    ///   the number of bytes read so far and the total number of bytes to read.
    /// # Returns
    /// A `Result` containing a vector of bytes read from the memory or an `Error
    /// if the operation fails
    pub async fn read_with_progress<F>(&self, address: usize, length: usize, progress_callback: F) -> Result<Vec<u8>>
    where
        F: FnMut(usize, usize),
    {
        self.memory.read(address, length, Some(progress_callback)).await
    }

    /// Write raw data to the memory device at the specified address.
    /// 
    /// # Arguments
    /// * `address` - The starting address to write to
    /// * `data` - A slice of bytes to write to the memory
    /// # Returns
    /// A `Result` indicating success or failure of the write operation
    pub async fn write(&self, address: usize, data: &[u8]) -> Result<()> {
        self.memory.write::<fn(usize, usize)>(address, data, None).await
    }

    /// Write raw data to the memory device at the specified address and report progress
    /// via the provided callback.
    /// 
    /// # Arguments
    /// * `address` - The starting address to write to
    /// * `data` - A slice of bytes to write to the memory
    /// * `progress_callback` - A callback function that takes two usize arguments:
    ///   the number of bytes written so far and the total number of bytes to write.
    /// # Returns
    /// A `Result` indicating success or failure of the write operation
    pub async fn write_with_progress<F>(&self, address: usize, data: &[u8], progress_callback: F) -> Result<()>
    where
        F: FnMut(usize, usize),
    {
        self.memory.write(address, data, Some(progress_callback)).await
    }
}