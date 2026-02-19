//! Lighthouse memory for base station geometry and calibration data
//!
//! This module provides types and functionality for reading and writing
//! Lighthouse positioning system configuration to the Crazyflie. This includes
//! base station geometry (position and orientation) and calibration data.

use crate::{Error, Result, subsystems::memory::{MemoryBackend, memory_types}};
use memory_types::{FromMemoryBackend, MemoryType};
use std::collections::HashMap;

// Binary format constants
const SIZE_FLOAT: usize = std::mem::size_of::<f32>();
const SIZE_U32: usize = std::mem::size_of::<u32>();
const SIZE_BOOL: usize = std::mem::size_of::<u8>();
const SIZE_VECTOR: usize = 3 * SIZE_FLOAT;
const NUM_SWEEP_PARAMS: usize = 7;
const NUM_SWEEPS: usize = 2;
const NUM_ROTATION_ROWS: usize = 3;

/// Helper to read a little-endian f32 from a byte slice at a given offset
fn read_f32(data: &[u8], offset: usize) -> Result<f32> {
    data.get(offset..offset + SIZE_FLOAT)
        .and_then(|slice| slice.try_into().ok())
        .map(f32::from_le_bytes)
        .ok_or_else(|| Error::MemoryError(format!(
            "Failed to read f32 at offset {}", offset
        )))
}

/// Helper to read a little-endian u32 from a byte slice at a given offset
fn read_u32(data: &[u8], offset: usize) -> Result<u32> {
    data.get(offset..offset + SIZE_U32)
        .and_then(|slice| slice.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or_else(|| Error::MemoryError(format!(
            "Failed to read u32 at offset {}", offset
        )))
}

/// Calibration data for one sweep of a lighthouse base station
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LighthouseCalibrationSweep {
    /// Phase offset
    pub phase: f32,
    /// Tilt angle
    pub tilt: f32,
    /// Curve compensation
    pub curve: f32,
    /// Gibbs magnitude
    pub gibmag: f32,
    /// Gibbs phase
    pub gibphase: f32,
    /// OGEE magnitude
    pub ogeemag: f32,
    /// OGEE phase
    pub ogeephase: f32,
}

impl LighthouseCalibrationSweep {
    /// Size in bytes when serialized
    pub const SIZE: usize = NUM_SWEEP_PARAMS * SIZE_FLOAT;

    /// Parse sweep calibration data from bytes
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        if data.len() < Self::SIZE {
            return Err(Error::MemoryError(format!(
                "Insufficient data for calibration sweep: expected {} bytes, got {}",
                Self::SIZE, data.len()
            )));
        }

        Ok(Self {
            phase: read_f32(data, 0 * SIZE_FLOAT)?,
            tilt: read_f32(data, 1 * SIZE_FLOAT)?,
            curve: read_f32(data, 2 * SIZE_FLOAT)?,
            gibmag: read_f32(data, 3 * SIZE_FLOAT)?,
            gibphase: read_f32(data, 4 * SIZE_FLOAT)?,
            ogeemag: read_f32(data, 5 * SIZE_FLOAT)?,
            ogeephase: read_f32(data, 6 * SIZE_FLOAT)?,
        })
    }

    /// Serialize sweep calibration data to bytes
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(Self::SIZE);
        data.extend_from_slice(&self.phase.to_le_bytes());
        data.extend_from_slice(&self.tilt.to_le_bytes());
        data.extend_from_slice(&self.curve.to_le_bytes());
        data.extend_from_slice(&self.gibmag.to_le_bytes());
        data.extend_from_slice(&self.gibphase.to_le_bytes());
        data.extend_from_slice(&self.ogeemag.to_le_bytes());
        data.extend_from_slice(&self.ogeephase.to_le_bytes());
        data
    }
}

/// Calibration data for one lighthouse base station
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LighthouseBsCalibration {
    /// Calibration data for both sweeps
    pub sweeps: [LighthouseCalibrationSweep; NUM_SWEEPS],
    /// Unique identifier for this base station
    pub uid: u32,
    /// Whether this calibration data is valid
    pub valid: bool,
}

impl LighthouseBsCalibration {
    /// Size in bytes when serialized
    pub const SIZE: usize = NUM_SWEEPS * LighthouseCalibrationSweep::SIZE + SIZE_U32 + SIZE_BOOL;

    // Offset constants for parsing
    const SWEEP0_OFFSET: usize = 0;
    const SWEEP1_OFFSET: usize = LighthouseCalibrationSweep::SIZE;
    const UID_OFFSET: usize = NUM_SWEEPS * LighthouseCalibrationSweep::SIZE;
    const VALID_OFFSET: usize = Self::UID_OFFSET + SIZE_U32;

    /// Parse calibration data from bytes
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        if data.len() < Self::SIZE {
            return Err(Error::MemoryError(format!(
                "Insufficient data for calibration: expected {} bytes, got {}",
                Self::SIZE, data.len()
            )));
        }

        let sweep0 = LighthouseCalibrationSweep::from_bytes(&data[Self::SWEEP0_OFFSET..Self::SWEEP1_OFFSET])?;
        let sweep1 = LighthouseCalibrationSweep::from_bytes(&data[Self::SWEEP1_OFFSET..Self::UID_OFFSET])?;
        let uid = read_u32(data, Self::UID_OFFSET)?;
        let valid = data.get(Self::VALID_OFFSET).map_or(false, |&b| b != 0);

        Ok(Self {
            sweeps: [sweep0, sweep1],
            uid,
            valid,
        })
    }

    /// Serialize calibration data to bytes
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(Self::SIZE);
        data.extend(self.sweeps[0].to_bytes());
        data.extend(self.sweeps[1].to_bytes());
        data.extend_from_slice(&self.uid.to_le_bytes());
        data.push(u8::from(self.valid));
        data
    }
}

/// Geometry data for one lighthouse base station
#[derive(Debug, Clone, PartialEq)]
pub struct LighthouseBsGeometry {
    /// Origin position of the base station [x, y, z] in meters
    pub origin: [f32; 3],
    /// Rotation matrix of the base station (3x3)
    pub rotation_matrix: [[f32; 3]; 3],
    /// Whether this geometry data is valid
    pub valid: bool,
}

impl Default for LighthouseBsGeometry {
    fn default() -> Self {
        Self {
            origin: [0.0, 0.0, 0.0],
            rotation_matrix: [[0.0; 3]; 3],
            valid: false,
        }
    }
}

impl LighthouseBsGeometry {
    /// Size in bytes when serialized (origin vector + 3 rotation vectors + valid flag)
    pub const SIZE: usize = (1 + NUM_ROTATION_ROWS) * SIZE_VECTOR + SIZE_BOOL;

    // Offset constants for parsing
    const ORIGIN_OFFSET: usize = 0;
    const ROTATION_OFFSET: usize = SIZE_VECTOR;
    const VALID_OFFSET: usize = (1 + NUM_ROTATION_ROWS) * SIZE_VECTOR;

    /// Parse geometry data from bytes
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        if data.len() < Self::SIZE {
            return Err(Error::MemoryError(format!(
                "Insufficient data for geometry: expected {} bytes, got {}",
                Self::SIZE, data.len()
            )));
        }

        let read_vector = |offset: usize| -> Result<[f32; 3]> {
            Ok([
                read_f32(data, offset)?,
                read_f32(data, offset + SIZE_FLOAT)?,
                read_f32(data, offset + 2 * SIZE_FLOAT)?,
            ])
        };

        let origin = read_vector(Self::ORIGIN_OFFSET)?;
        let rotation_matrix = [
            read_vector(Self::ROTATION_OFFSET)?,
            read_vector(Self::ROTATION_OFFSET + SIZE_VECTOR)?,
            read_vector(Self::ROTATION_OFFSET + 2 * SIZE_VECTOR)?,
        ];
        let valid = data.get(Self::VALID_OFFSET).map_or(false, |&b| b != 0);

        Ok(Self {
            origin,
            rotation_matrix,
            valid,
        })
    }

    /// Serialize geometry data to bytes
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(Self::SIZE);

        // Write origin
        for &v in &self.origin {
            data.extend_from_slice(&v.to_le_bytes());
        }

        // Write rotation matrix rows
        for row in &self.rotation_matrix {
            for &v in row {
                data.extend_from_slice(&v.to_le_bytes());
            }
        }

        // Write valid flag
        data.push(u8::from(self.valid));

        data
    }
}

/// Memory interface for lighthouse configuration data
///
/// This provides methods to read and write lighthouse base station
/// geometry and calibration data to the Crazyflie.
#[derive(Debug)]
pub struct LighthouseMemory {
    memory: MemoryBackend,
}

impl LighthouseMemory {
    /// Start address for geometry data
    pub const GEO_START_ADDR: usize = 0x00;
    /// Start address for calibration data
    pub const CALIB_START_ADDR: usize = 0x1000;
    /// Size of one page (each base station uses one page)
    pub const PAGE_SIZE: usize = 0x100;
    /// Maximum number of base stations supported
    pub const MAX_BASE_STATIONS: usize = 16;

    /// Validate that a base station ID is within the valid range
    fn validate_bs_id(bs_id: u8) -> Result<()> {
        if bs_id as usize >= Self::MAX_BASE_STATIONS {
            return Err(Error::InvalidArgument(format!(
                "Base station ID {} out of range (0-{})",
                bs_id, Self::MAX_BASE_STATIONS - 1
            )));
        }
        Ok(())
    }

    /// Create a LighthouseMemory from a MemoryBackend, validating the memory type
    fn from_backend(memory: MemoryBackend) -> Result<Self> {
        if memory.memory_type == MemoryType::Lighthouse {
            Ok(Self { memory })
        } else {
            Err(Error::MemoryError(format!(
                "Expected Lighthouse memory type, got {:?}",
                memory.memory_type
            )))
        }
    }

    /// Read geometry data for a specific base station
    ///
    /// # Arguments
    /// * `bs_id` - Base station ID (0-15)
    ///
    /// # Returns
    /// The geometry data, or an error if the read failed
    pub async fn read_geometry(&self, bs_id: u8) -> Result<LighthouseBsGeometry> {
        Self::validate_bs_id(bs_id)?;

        let addr = Self::GEO_START_ADDR + (bs_id as usize) * Self::PAGE_SIZE;
        let data = self.memory.read::<fn(usize, usize)>(addr, LighthouseBsGeometry::SIZE, None).await?;
        LighthouseBsGeometry::from_bytes(&data)
    }

    /// Write geometry data for a specific base station
    ///
    /// # Arguments
    /// * `bs_id` - Base station ID (0-15)
    /// * `geometry` - The geometry data to write
    pub async fn write_geometry(&self, bs_id: u8, geometry: &LighthouseBsGeometry) -> Result<()> {
        Self::validate_bs_id(bs_id)?;

        let addr = Self::GEO_START_ADDR + (bs_id as usize) * Self::PAGE_SIZE;
        let data = geometry.to_bytes();
        self.memory.write::<fn(usize, usize)>(addr, &data, None).await
    }

    /// Read calibration data for a specific base station
    ///
    /// # Arguments
    /// * `bs_id` - Base station ID (0-15)
    ///
    /// # Returns
    /// The calibration data, or an error if the read failed
    pub async fn read_calibration(&self, bs_id: u8) -> Result<LighthouseBsCalibration> {
        Self::validate_bs_id(bs_id)?;

        let addr = Self::CALIB_START_ADDR + (bs_id as usize) * Self::PAGE_SIZE;
        let data = self.memory.read::<fn(usize, usize)>(addr, LighthouseBsCalibration::SIZE, None).await?;
        LighthouseBsCalibration::from_bytes(&data)
    }

    /// Write calibration data for a specific base station
    ///
    /// # Arguments
    /// * `bs_id` - Base station ID (0-15)
    /// * `calibration` - The calibration data to write
    pub async fn write_calibration(&self, bs_id: u8, calibration: &LighthouseBsCalibration) -> Result<()> {
        Self::validate_bs_id(bs_id)?;

        let addr = Self::CALIB_START_ADDR + (bs_id as usize) * Self::PAGE_SIZE;
        let data = calibration.to_bytes();
        self.memory.write::<fn(usize, usize)>(addr, &data, None).await
    }

    /// Read all geometry data from the Crazyflie
    ///
    /// Attempts to read geometry for all base stations (0-15). Only base stations
    /// with valid data are included in the result.
    ///
    /// # Returns
    /// A HashMap mapping base station ID to geometry data
    pub async fn read_all_geometries(&self) -> Result<HashMap<u8, LighthouseBsGeometry>> {
        self.read_all_geometries_with_progress(|_, _| {}).await
    }

    /// Read all geometry data with progress reporting
    ///
    /// # Arguments
    /// * `progress_callback` - Called with (completed_count, total_count) after each read
    pub async fn read_all_geometries_with_progress<F>(&self, mut progress_callback: F) -> Result<HashMap<u8, LighthouseBsGeometry>>
    where
        F: FnMut(usize, usize),
    {
        let mut result = HashMap::new();

        for bs_id in 0..Self::MAX_BASE_STATIONS as u8 {
            match self.read_geometry(bs_id).await {
                Ok(geo) => {
                    if geo.valid {
                        result.insert(bs_id, geo);
                    }
                }
                Err(Error::MemoryError(_)) => {
                    // Base station not supported by firmware, skip it
                }
                Err(e) => return Err(e),
            }
            progress_callback(bs_id as usize + 1, Self::MAX_BASE_STATIONS);
        }

        Ok(result)
    }

    /// Read all calibration data from the Crazyflie
    ///
    /// Attempts to read calibration for all base stations (0-15). Only base stations
    /// with valid data are included in the result.
    ///
    /// # Returns
    /// A HashMap mapping base station ID to calibration data
    pub async fn read_all_calibrations(&self) -> Result<HashMap<u8, LighthouseBsCalibration>> {
        self.read_all_calibrations_with_progress(|_, _| {}).await
    }

    /// Read all calibration data with progress reporting
    ///
    /// # Arguments
    /// * `progress_callback` - Called with (completed_count, total_count) after each read
    pub async fn read_all_calibrations_with_progress<F>(&self, mut progress_callback: F) -> Result<HashMap<u8, LighthouseBsCalibration>>
    where
        F: FnMut(usize, usize),
    {
        let mut result = HashMap::new();

        for bs_id in 0..Self::MAX_BASE_STATIONS as u8 {
            match self.read_calibration(bs_id).await {
                Ok(calib) => {
                    if calib.valid {
                        result.insert(bs_id, calib);
                    }
                }
                Err(Error::MemoryError(_)) => {
                    // Base station not supported by firmware, skip it
                }
                Err(e) => return Err(e),
            }
            progress_callback(bs_id as usize + 1, Self::MAX_BASE_STATIONS);
        }

        Ok(result)
    }

    /// Write geometry data for multiple base stations
    ///
    /// # Arguments
    /// * `geometries` - A HashMap mapping base station ID to geometry data
    pub async fn write_geometries(&self, geometries: &HashMap<u8, LighthouseBsGeometry>) -> Result<()> {
        self.write_geometries_with_progress(geometries, |_, _| {}).await
    }

    /// Write geometry data for multiple base stations with progress reporting
    ///
    /// # Arguments
    /// * `geometries` - A HashMap mapping base station ID to geometry data
    /// * `progress_callback` - Called with (completed_count, total_count) after each write
    pub async fn write_geometries_with_progress<F>(
        &self,
        geometries: &HashMap<u8, LighthouseBsGeometry>,
        mut progress_callback: F,
    ) -> Result<()>
    where
        F: FnMut(usize, usize),
    {
        let total = geometries.len();
        let mut completed = 0;

        for (&bs_id, geometry) in geometries {
            self.write_geometry(bs_id, geometry).await?;
            completed += 1;
            progress_callback(completed, total);
        }

        Ok(())
    }

    /// Write calibration data for multiple base stations
    ///
    /// # Arguments
    /// * `calibrations` - A HashMap mapping base station ID to calibration data
    pub async fn write_calibrations(&self, calibrations: &HashMap<u8, LighthouseBsCalibration>) -> Result<()> {
        self.write_calibrations_with_progress(calibrations, |_, _| {}).await
    }

    /// Write calibration data for multiple base stations with progress reporting
    ///
    /// # Arguments
    /// * `calibrations` - A HashMap mapping base station ID to calibration data
    /// * `progress_callback` - Called with (completed_count, total_count) after each write
    pub async fn write_calibrations_with_progress<F>(
        &self,
        calibrations: &HashMap<u8, LighthouseBsCalibration>,
        mut progress_callback: F,
    ) -> Result<()>
    where
        F: FnMut(usize, usize),
    {
        let total = calibrations.len();
        let mut completed = 0;

        for (&bs_id, calibration) in calibrations {
            self.write_calibration(bs_id, calibration).await?;
            completed += 1;
            progress_callback(completed, total);
        }

        Ok(())
    }
}

impl FromMemoryBackend for LighthouseMemory {
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
