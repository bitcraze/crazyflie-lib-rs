use std::collections::HashMap;

use crate::{subsystems::memory::{memory_types, MemoryBackend}, Error, Result};
use memory_types::{FromMemoryBackend, MemoryType};

/// Describes the content of a Crazyflie decks 1-wire memory
#[derive(Debug)]
pub struct OwMemory {
    memory: MemoryBackend,
    /// Bitmap of used GPIO pins used for output
    used_pins: u32,
    /// Vendor ID
    vid: u8,
    /// Product ID
    pid: u8,
    /// Key-value pairs of elements stored in the memory
    elements: HashMap<String, String>
  }

  impl OwMemory {
    /// Gets the bitmap of used GPIO pins
    pub fn used_pins(&self) -> u32 {
      self.used_pins
    }

    /// Sets the bitmap of used GPIO pins
    pub fn set_used_pins(&mut self, used_pins: u32) {
      self.used_pins = used_pins;
    }

    /// Gets the vendor ID
    pub fn vid(&self) -> u8 {
      self.vid
    }

    /// Sets the vendor ID
    pub fn set_vid(&mut self, vid: u8) {
      self.vid = vid;
    }

    /// Gets the product ID
    pub fn pid(&self) -> u8 {
      self.pid
    }

    /// Sets the product ID
    pub fn set_pid(&mut self, pid: u8) {
      self.pid = pid;
    }

    /// Gets a reference to the elements map
    pub fn elements(&self) -> &HashMap<String, String> {
      &self.elements
    }

    /// Gets a mutable reference to the elements map
    pub fn elements_mut(&mut self) -> &mut HashMap<String, String> {
      &mut self.elements
    }

    /// Sets the elements map
    pub fn set_elements(&mut self, elements: HashMap<String, String>) {
      self.elements = elements;
    }
}

impl FromMemoryBackend for OwMemory {
    async fn from_memory_backend(memory: MemoryBackend) -> Result<Self> {
        if memory.memory_type == MemoryType::OneWire {
            Ok(OwMemory::new(memory).await?)
        } else {
            Err(Error::MemoryError("Wrong type of memory!".to_owned()))
        }
    }

    async fn initialize_memory_backend(memory: MemoryBackend) -> Result<Self> {
        if memory.memory_type == MemoryType::OneWire {
            Ok(OwMemory::initialize(memory).await?)
        } else {
            Err(Error::MemoryError("Wrong type of memory!".to_owned()))
        }
    }

    fn close_memory(self) -> MemoryBackend {
      self.memory
    }
}

impl OwMemory {
    pub(crate) async fn new(memory: MemoryBackend) -> Result<Self> {
        let header = memory.read(0, 8).await?;

        // Validate header byte
        if header[0] != 0xEB {
          return Err(Error::MemoryError("Invalid OneWire header".to_owned()));
        }

        // Extract fields from header
        let used_pins = u32::from_le_bytes([header[1], header[2], header[3], header[4]]);
        let vid = header[5];
        let pid = header[6];
        let crc_byte = header[7];

        // Calculate CRC32 of bytes 0-6 and check against LSB
        let crc_data = &header[0..7];
        let calculated_crc = crc32fast::hash(crc_data);
        let expected_crc_lsb = (calculated_crc & 0xFF) as u8;

        if crc_byte != expected_crc_lsb {
          return Err(Error::MemoryError("OneWire CRC validation failed".to_owned()));
        }

        let element_header = memory.read(8, 2).await?;
        let version = element_header[0];
        let element_length = element_header[1];

        if version != 0 {
          return Err(Error::MemoryError("Unsupported OneWire version".to_owned()));   
        }

        let elements = memory.read(10, element_length as usize).await?;
        let elements_crc = memory.read(10 + element_length as usize, 1).await?[0];

        let mut data = element_header;
        data.extend_from_slice(&elements);

        let calculated_crc = crc32fast::hash(&data);
        let expected_crc_lsb = (calculated_crc & 0xFF) as u8;

        if elements_crc != expected_crc_lsb {
          return Err(Error::MemoryError("OneWire data CRC validation failed".to_owned()));
        }

        let elements_map = Self::parse_elements(&elements);

        if header.len() == 8 {
            Ok(OwMemory {
              memory,
              used_pins,
              vid,
              pid,
              elements: elements_map
            })
        } else {
            Err(Error::MemoryError("Invalid OneWire memory data".to_owned()))
        }
    }

    pub(crate) async fn initialize(memory: MemoryBackend) -> Result<Self> {
      Ok(OwMemory {
        memory,
        used_pins: 0,
        vid: 0,
        pid: 0,
        elements: HashMap::new()
      })
    }

    fn parse_elements(data: &[u8]) -> HashMap<String, String> {
        let mut elements = HashMap::new();
        let mut offset = 0;

        while offset < data.len() {
            if offset + 1 >= data.len() {
                break;
            }

            let element_id = data[offset];
            let element_length = data[offset + 1];
            offset += 2;

            if offset + element_length as usize > data.len() {
                break;
            }

            let element_data = &data[offset..offset + element_length as usize];
            offset += element_length as usize;

            match element_id {
                1 => { // boardName
                    if let Ok(board_name) = String::from_utf8(element_data.to_vec()) {
                        elements.insert("boardName".to_string(), board_name);
                    }
                },
                2 => { // revision
                    if let Ok(revision) = String::from_utf8(element_data.to_vec()) {
                        elements.insert("revision".to_string(), revision);
                    }
                },
                3 => { // customData
                    let custom_data = hex::encode(element_data);
                    elements.insert("customData".to_string(), custom_data);
                },
                _ => {
                    // Unknown elements are ignored
                }
            }
        }

        elements
    }

}