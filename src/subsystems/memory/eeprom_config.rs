use crate::{subsystems::memory::memory_types, Error, Result};
use std::{
    convert::{TryFrom, TryInto},
    fmt::{self, Display},
};

use memory_types::{FromMemoryDevice, MemoryDevice, MemoryType};

/// Describes the content of the I2C EEPROM used for configuration on the Crazyflie
/// platform.
#[derive(Debug)]
pub struct EEPROMConfigMemory {
  memory: MemoryDevice,
  /// Version of the EEPROM configuration structure.
  version: u8,
  /// Radio frequency channel (0-125) for wireless communication.
  radio_channel: u8,
  /// Data transmission rate for radio communication.
  radio_speed: RadioSpeed,
  /// Forward/backward balance adjustment in degrees.
  pitch_trim: f32,
  /// Left/right balance adjustment in degrees.
  roll_trim: f32,
  /// 5-byte unique address for the radio module.
  radio_address: [u8; 5],
}


/// Represents the radio speed settings available for the Crazyflie.
#[derive(Debug, Clone)]
pub enum RadioSpeed {
  /// 250 Kbps data rate
  R250Kbps = 0,
  /// 1 Mbps data rate
  R1Mbps = 1,
  /// 2 Mbps data rate
  R2Mbps = 2,
}

impl TryFrom<u8> for RadioSpeed {
  type Error = Error;

  fn try_from(value: u8) -> Result<Self> {
    match value {
      0 => Ok(RadioSpeed::R250Kbps),
      1 => Ok(RadioSpeed::R1Mbps),
      2 => Ok(RadioSpeed::R2Mbps),
      _ => Err(Error::MemoryError(format!("Invalid radio speed value: {}", value))),
    }
  }
}

impl Display for RadioSpeed {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      RadioSpeed::R250Kbps => write!(f, "250 kbps"),
      RadioSpeed::R1Mbps => write!(f, "1 Mbps"),
      RadioSpeed::R2Mbps => write!(f, "2 Mbps"),
    }
  }
}

impl FromMemoryDevice for EEPROMConfigMemory {
    async fn from_memory_device(memory: MemoryDevice) -> Result<Self> {
        if memory.memory_type == MemoryType::EEPROMConfig {
            Ok(EEPROMConfigMemory::new(memory).await?)
        } else {
            Err(Error::MemoryError("Wrong type of memory!".to_owned()))
        }
    }

    async fn initialize_memory_device(memory: MemoryDevice) -> Result<Self> {
      if memory.memory_type == MemoryType::EEPROMConfig {
          Ok(EEPROMConfigMemory::initialize(memory).await?)
      } else {
          Err(Error::MemoryError("Wrong type of memory!".to_owned()))
      }
    }
}

impl EEPROMConfigMemory {
    pub(crate) async fn new(memory: MemoryDevice) -> Result<Self> {
        let data = memory.read(0, 21).await?;
        if data.len() >= 4 && &data[0..4] == b"0xBC" {
            let version = data[4];
            let radio_channel = data[5];
            let radio_speed = RadioSpeed::try_from(data[6])?;
            let pitch_trim = f32::from_le_bytes(data[7..11].try_into().unwrap());
            let roll_trim = f32::from_le_bytes(data[11..15].try_into().unwrap());
            let mut radio_address = [0; 5];
            radio_address.copy_from_slice(&data[15..20]);

            let calculated_checksum = data[..data.len()-1].iter().fold(0u8, |acc, &byte| acc.wrapping_add(byte));
            let stored_checksum = data[data.len()-1];

            if calculated_checksum != stored_checksum {
              return Err(Error::MemoryError("Checksum mismatch in EEPROM config data".to_string()));
            }

            Ok(EEPROMConfigMemory {
                memory,
                version,
                radio_channel,
                radio_speed,
                pitch_trim,
                roll_trim,
                radio_address,
            })
        } else {
            Err(Error::MemoryError(
                "Malformed EEPROM config data".to_string(),
            ))
        }
    }

    pub(crate) async fn initialize(memory: MemoryDevice) -> Result<Self> {
      Ok(EEPROMConfigMemory {
        memory,
        version: 0,
        radio_channel: 80,
        radio_speed: RadioSpeed::R2Mbps,
        pitch_trim: 0.0,
        roll_trim: 0.0,
        radio_address: [0xE7, 0xE7, 0xE7, 0xE7, 0xE7],
      })
    }

    /// Commit the current configuration back to the EEPROM.
    /// This will overwrite the existing configuration.
    /// 
    /// Returns `Ok(())` if the operation is successful, or an `Error` if it fails.
    /// # Errors
    /// This function will return an error if the write operation to the EEPROM fails.
    pub async fn commit(&self) -> Result<()> {
      let mut data = Vec::new();
      data.extend_from_slice(b"0xBC");
      data.push(self.version);
      data.push(self.radio_channel);
      data.push(self.radio_speed.clone() as u8);
      data.extend_from_slice(&self.pitch_trim.to_le_bytes());
      data.extend_from_slice(&self.roll_trim.to_le_bytes());
      data.extend_from_slice(&self.radio_address);

      let checksum = data.iter().fold(0u8, |acc, &byte| acc.wrapping_add(byte));
      data.push(checksum);

      self.memory.write(0, &data).await
    }


  /// Gets the radio frequency channel.
  pub fn get_radio_channel(&self) -> u8 {
    self.radio_channel
  }

  /// Sets the radio frequency channel (0-125).
  pub fn set_radio_channel(&mut self, channel: u8) -> Result<()> {
    if channel > 125 {
      return Err(Error::InvalidParameter("Radio channel must be between 0 and 125".into()));
    }
    self.radio_channel = channel;
    Ok(())
  }

  /// Gets the radio speed.
  pub fn get_radio_speed(&self) -> &RadioSpeed {
    &self.radio_speed
  }

  /// Sets the radio speed.
  pub fn set_radio_speed(&mut self, speed: RadioSpeed) {
    self.radio_speed = speed;
  }

  /// Gets the pitch trim value.
  pub fn get_pitch_trim(&self) -> f32 {
    self.pitch_trim
  }

  /// Sets the pitch trim value.
  pub fn set_pitch_trim(&mut self, trim: f32) {
    self.pitch_trim = trim;
  }

  /// Gets the roll trim value.
  pub fn get_roll_trim(&self) -> f32 {
    self.roll_trim
  }

  /// Sets the roll trim value.
  pub fn set_roll_trim(&mut self, trim: f32) {
    self.roll_trim = trim;
  }

  /// Gets the radio address.
  pub fn get_radio_address(&self) -> &[u8; 5] {
    &self.radio_address
  }

  /// Sets the radio address.
  pub fn set_radio_address(&mut self, address: [u8; 5]) {
    self.radio_address = address;
  }
}