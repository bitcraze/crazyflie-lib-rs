//! LED driver memory for the Crazyflie LED ring
//!
//! This module provides types and functionality for controlling the LEDs in
//! the Crazyflie LED ring deck by writing RGB values to the LED driver memory.

use crate::{Error, Result, subsystems::memory::{MemoryBackend, memory_types}};
use memory_types::{FromMemoryBackend, MemoryType};

const NUM_LEDS: usize = 12;
const LED_DATA_SIZE: usize = NUM_LEDS * 2; // RGB565, 2 bytes per LED

/// Represents a single LED with RGB color and intensity
#[derive(Debug, Clone, Copy)]
pub struct Led {
    /// Red component (0-255)
    pub r: u8,
    /// Green component (0-255)
    pub g: u8,
    /// Blue component (0-255)
    pub b: u8,
    /// Intensity percentage (0-100)
    pub intensity: u8,
}

impl Default for Led {
    fn default() -> Self {
        Self {
            r: 0,
            g: 0,
            b: 0,
            intensity: 100,
        }
    }
}

impl Led {
    /// Set the R/G/B and optionally intensity in one call
    pub fn set(&mut self, r: u8, g: u8, b: u8, intensity: Option<u8>) {
        self.r = r;
        self.g = g;
        self.b = b;
        if let Some(i) = intensity {
            self.intensity = i;
        }
    }

    fn to_rgb565(&self) -> u16 {
        let intensity = self.intensity as u32;
        let r5 = ((((self.r as u32) * 249 + 1014) >> 11) & 0x1F) * intensity / 100;
        let g6 = ((((self.g as u32) * 253 + 505) >> 10) & 0x3F) * intensity / 100;
        let b5 = ((((self.b as u32) * 249 + 1014) >> 11) & 0x1F) * intensity / 100;
        ((r5 << 11) | (g6 << 5) | b5) as u16
    }
}

/// Memory interface for the Crazyflie LED ring
///
/// Provides methods to control the 12 LEDs in the Crazyflie LED ring by writing
/// RGB values to the LED driver memory. Colors are compressed to RGB565 format
/// with intensity applied before writing.
#[derive(Debug)]
pub struct LedDriverMemory {
    /// The 12 LEDs in the ring
    pub leds: [Led; NUM_LEDS],
    memory: MemoryBackend,
}

impl LedDriverMemory {
    fn from_backend(memory: MemoryBackend) -> Result<Self> {
        if memory.memory_type == MemoryType::DriverLed {
            Ok(Self {
                leds: [Led::default(); NUM_LEDS],
                memory,
            })
        } else {
            Err(Error::MemoryError(format!(
                "Expected DriverLed memory type, got {:?}",
                memory.memory_type
            )))
        }
    }

    /// Write the current LED values to the Crazyflie LED ring
    ///
    /// Converts each LED's RGB values to RGB565 format with intensity applied,
    /// and writes the 24-byte result to address 0x00 of the LED driver memory.
    pub async fn write_leds(&self) -> Result<()> {
        let mut data = Vec::with_capacity(LED_DATA_SIZE);
        for led in &self.leds {
            let rgb565 = led.to_rgb565();
            data.push((rgb565 >> 8) as u8);
            data.push((rgb565 & 0xFF) as u8);
        }
        self.memory.write::<fn(usize, usize)>(0x00, &data, None).await
    }
}

impl FromMemoryBackend for LedDriverMemory {
    async fn from_memory_backend(memory: MemoryBackend) -> Result<Self> {
        Self::from_backend(memory)
    }

    async fn initialize_memory_backend(memory: MemoryBackend) -> Result<Self> {
        Self::from_backend(memory)
    }

    fn close_memory(self) -> MemoryBackend {
        self.memory
    }
}
