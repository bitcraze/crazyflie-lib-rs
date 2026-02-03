//! Trajectory memory for the Crazyflie high level commander
//!
//! This module provides types and functionality for defining and uploading
//! trajectories to the Crazyflie's trajectory memory. Trajectories can be
//! either uncompressed (using `Poly4D`) or compressed (using `CompressedStart`
//! and `CompressedSegment`).

use crate::{Error, Result, subsystems::memory::{MemoryBackend, memory_types}};
use memory_types::{FromMemoryBackend, MemoryType};

/// Encode a spatial coordinate (meters) to millimeters as i16
///
/// Valid range: approximately -32.767 to +32.767 meters
fn encode_spatial(coordinate: f32) -> Result<i16> {
    let scaled = coordinate * 1000.0;
    if scaled < i16::MIN as f32 || scaled > i16::MAX as f32 {
        return Err(Error::InvalidArgument(
            format!("Spatial coordinate {:.3}m out of representable range ({:.3}m to {:.3}m)",
                coordinate, i16::MIN as f32 / 1000.0, i16::MAX as f32 / 1000.0)
        ));
    }
    Ok(scaled as i16)
}

/// Encode a yaw angle (radians) to 1/10th degrees as i16
///
/// Valid range: approximately -573.2 to +573.2 degrees (-10.0 to +10.0 radians)
fn encode_yaw(angle_rad: f32) -> Result<i16> {
    let scaled = angle_rad.to_degrees() * 10.0;
    if scaled < i16::MIN as f32 || scaled > i16::MAX as f32 {
        return Err(Error::InvalidArgument(
            format!("Yaw angle {:.3} rad out of representable range", angle_rad)
        ));
    }
    Ok(scaled as i16)
}

/// A polynomial with up to 8 coefficients
#[derive(Debug, Clone)]
pub struct Poly {
    /// The polynomial coefficients (up to 8)
    pub values: [f32; 8],
}

impl Default for Poly {
    fn default() -> Self {
        Self { values: [0.0; 8] }
    }
}

impl Poly {
    /// Create a new polynomial with the given coefficients
    pub fn new(values: [f32; 8]) -> Self {
        Self { values }
    }

    /// Create a polynomial from a slice of values
    ///
    /// If the slice has fewer than 8 values, the remaining coefficients are set to 0.
    /// If the slice has more than 8 values, only the first 8 are used.
    pub fn from_slice(values: &[f32]) -> Self {
        let mut poly = Self::default();
        let len = values.len().min(8);
        poly.values[..len].copy_from_slice(&values[..len]);
        poly
    }
}

/// A 4D polynomial trajectory segment (uncompressed format)
///
/// This represents a single segment of a trajectory defined by polynomials
/// for x, y, z, and yaw coordinates over a duration of time.
#[derive(Debug, Clone)]
pub struct Poly4D {
    /// Duration of this segment in seconds
    pub duration: f32,
    /// Polynomial for x coordinate
    pub x: Poly,
    /// Polynomial for y coordinate
    pub y: Poly,
    /// Polynomial for z coordinate
    pub z: Poly,
    /// Polynomial for yaw angle
    pub yaw: Poly,
}

impl Poly4D {
    /// Create a new Poly4D trajectory segment
    pub fn new(duration: f32, x: Poly, y: Poly, z: Poly, yaw: Poly) -> Self {
        Self { duration, x, y, z, yaw }
    }

    /// Pack this segment into bytes for transmission
    pub fn pack(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(132); // 8*4*4 + 4 = 132 bytes

        // Pack x coefficients (8 * f32)
        for &v in &self.x.values {
            data.extend_from_slice(&v.to_le_bytes());
        }
        // Pack y coefficients (8 * f32)
        for &v in &self.y.values {
            data.extend_from_slice(&v.to_le_bytes());
        }
        // Pack z coefficients (8 * f32)
        for &v in &self.z.values {
            data.extend_from_slice(&v.to_le_bytes());
        }
        // Pack yaw coefficients (8 * f32)
        for &v in &self.yaw.values {
            data.extend_from_slice(&v.to_le_bytes());
        }
        // Pack duration (f32)
        data.extend_from_slice(&self.duration.to_le_bytes());

        data
    }
}

/// Starting point for a compressed trajectory
///
/// Compressed trajectories begin with a `CompressedStart` that defines
/// the initial position and yaw, followed by `CompressedSegment`s.
#[derive(Debug, Clone)]
pub struct CompressedStart {
    /// X coordinate in meters
    pub x: f32,
    /// Y coordinate in meters
    pub y: f32,
    /// Z coordinate in meters
    pub z: f32,
    /// Yaw angle in radians
    pub yaw: f32,
}

impl CompressedStart {
    /// Create a new compressed start point
    pub fn new(x: f32, y: f32, z: f32, yaw: f32) -> Self {
        Self { x, y, z, yaw }
    }

    /// Pack this start point into bytes for transmission
    ///
    /// Returns an error if any coordinate is out of the representable range.
    pub fn pack(&self) -> Result<Vec<u8>> {
        let mut data = Vec::with_capacity(8);

        data.extend_from_slice(&encode_spatial(self.x)?.to_le_bytes());
        data.extend_from_slice(&encode_spatial(self.y)?.to_le_bytes());
        data.extend_from_slice(&encode_spatial(self.z)?.to_le_bytes());
        data.extend_from_slice(&encode_yaw(self.yaw)?.to_le_bytes());

        Ok(data)
    }
}

/// Type of polynomial element in a compressed segment
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ElementType {
    /// No movement (0 coefficients)
    Constant = 0,
    /// Linear (1 coefficient)
    Linear = 1,
    /// Quadratic (3 coefficients)
    Quadratic = 2,
    /// Full (7 coefficients)
    Full = 3,
}

impl ElementType {
    fn from_len(len: usize) -> Option<Self> {
        match len {
            0 => Some(ElementType::Constant),
            1 => Some(ElementType::Linear),
            3 => Some(ElementType::Quadratic),
            7 => Some(ElementType::Full),
            _ => None,
        }
    }
}

/// A segment in a compressed trajectory
///
/// Compressed segments use variable-length encoding where each axis
/// can have 0, 1, 3, or 7 coefficients depending on the complexity
/// of motion along that axis.
#[derive(Debug, Clone)]
pub struct CompressedSegment {
    duration: f32,
    x: Vec<f32>,
    y: Vec<f32>,
    z: Vec<f32>,
    yaw: Vec<f32>,
}

impl CompressedSegment {
    /// Create a new compressed segment
    ///
    /// # Arguments
    /// * `duration` - Duration of this segment in seconds
    /// * `x` - X polynomial coefficients (must be 0, 1, 3, or 7 elements)
    /// * `y` - Y polynomial coefficients (must be 0, 1, 3, or 7 elements)
    /// * `z` - Z polynomial coefficients (must be 0, 1, 3, or 7 elements)
    /// * `yaw` - Yaw polynomial coefficients (must be 0, 1, 3, or 7 elements)
    ///
    /// # Errors
    /// Returns an error if any element vector has an invalid length
    pub fn new(duration: f32, x: Vec<f32>, y: Vec<f32>, z: Vec<f32>, yaw: Vec<f32>) -> Result<Self> {
        Self::validate(&x)?;
        Self::validate(&y)?;
        Self::validate(&z)?;
        Self::validate(&yaw)?;

        Ok(Self { duration, x, y, z, yaw })
    }

    fn validate(element: &[f32]) -> Result<()> {
        let len = element.len();
        if len != 0 && len != 1 && len != 3 && len != 7 {
            return Err(Error::InvalidArgument(
                "Element length must be 0, 1, 3, or 7".to_owned()
            ));
        }
        Ok(())
    }

    fn encode_type(element: &[f32]) -> u8 {
        // Safe: fields are validated in new()
        ElementType::from_len(element.len()).unwrap() as u8
    }

    fn pack_spatial_element(element: &[f32]) -> Result<Vec<u8>> {
        let mut data = Vec::new();
        for &v in element {
            data.extend_from_slice(&encode_spatial(v)?.to_le_bytes());
        }
        Ok(data)
    }

    fn pack_yaw_element(element: &[f32]) -> Result<Vec<u8>> {
        let mut data = Vec::new();
        for &v in element {
            data.extend_from_slice(&encode_yaw(v)?.to_le_bytes());
        }
        Ok(data)
    }

    /// Pack this segment into bytes for transmission
    ///
    /// Returns an error if any coordinate is out of the representable range.
    pub fn pack(&self) -> Result<Vec<u8>> {
        let element_types = Self::encode_type(&self.x)
            | (Self::encode_type(&self.y) << 2)
            | (Self::encode_type(&self.z) << 4)
            | (Self::encode_type(&self.yaw) << 6);
        let duration_ms = (self.duration * 1000.0) as u16;

        let mut data = Vec::new();

        data.push(element_types);
        data.extend_from_slice(&duration_ms.to_le_bytes());
        data.extend(Self::pack_spatial_element(&self.x)?);
        data.extend(Self::pack_spatial_element(&self.y)?);
        data.extend(Self::pack_spatial_element(&self.z)?);
        data.extend(Self::pack_yaw_element(&self.yaw)?);

        Ok(data)
    }
}

/// Memory interface for trajectories used by the high level commander
///
/// Trajectories can be either uncompressed (using `Poly4D` segments) or
/// compressed (using `CompressedStart` followed by `CompressedSegment`s).
/// Use `write_uncompressed` for Poly4D trajectories and `write_compressed`
/// for compressed trajectories.
#[derive(Debug)]
pub struct TrajectoryMemory {
    memory: MemoryBackend,
}

impl FromMemoryBackend for TrajectoryMemory {
    async fn from_memory_backend(memory: MemoryBackend) -> Result<Self> {
        if memory.memory_type == MemoryType::Trajectory {
            Ok(Self { memory })
        } else {
            Err(Error::MemoryError("Wrong type of memory!".to_owned()))
        }
    }

    async fn initialize_memory_backend(memory: MemoryBackend) -> Result<Self> {
        if memory.memory_type == MemoryType::Trajectory {
            Ok(Self { memory })
        } else {
            Err(Error::MemoryError("Wrong type of memory!".to_owned()))
        }
    }

    fn close_memory(self) -> MemoryBackend {
        self.memory
    }
}

impl TrajectoryMemory {
    /// Write an uncompressed trajectory (Poly4D segments) to the Crazyflie
    ///
    /// # Arguments
    /// * `segments` - A slice of Poly4D trajectory segments
    /// * `start_addr` - The address in trajectory memory to upload to (0 by default)
    ///
    /// # Returns
    /// The number of bytes written
    pub async fn write_uncompressed(
        &self,
        segments: &[Poly4D],
        start_addr: usize,
    ) -> Result<usize> {
        let mut data = Vec::new();
        for segment in segments {
            data.extend(segment.pack());
        }

        self.memory.write::<fn(usize, usize)>(start_addr, &data, None).await?;
        Ok(data.len())
    }

    /// Write an uncompressed trajectory with progress reporting
    ///
    /// # Arguments
    /// * `segments` - A slice of Poly4D trajectory segments
    /// * `start_addr` - The address in trajectory memory to upload to (0 by default)
    /// * `progress_callback` - Called with (bytes_written, total_bytes)
    ///
    /// # Returns
    /// The number of bytes written
    pub async fn write_uncompressed_with_progress<F>(
        &self,
        segments: &[Poly4D],
        start_addr: usize,
        progress_callback: F,
    ) -> Result<usize>
    where
        F: FnMut(usize, usize),
    {
        let mut data = Vec::new();
        for segment in segments {
            data.extend(segment.pack());
        }

        self.memory.write(start_addr, &data, Some(progress_callback)).await?;
        Ok(data.len())
    }

    /// Write a compressed trajectory to the Crazyflie
    ///
    /// Compressed trajectories must start with a `CompressedStart` followed
    /// by zero or more `CompressedSegment`s.
    ///
    /// # Arguments
    /// * `start` - The starting point of the trajectory
    /// * `segments` - A slice of compressed trajectory segments
    /// * `start_addr` - The address in trajectory memory to upload to (0 by default)
    ///
    /// # Returns
    /// The number of bytes written
    pub async fn write_compressed(
        &self,
        start: &CompressedStart,
        segments: &[CompressedSegment],
        start_addr: usize,
    ) -> Result<usize> {
        let mut data = start.pack()?;
        for segment in segments {
            data.extend(segment.pack()?);
        }

        self.memory.write::<fn(usize, usize)>(start_addr, &data, None).await?;
        Ok(data.len())
    }

    /// Write a compressed trajectory with progress reporting
    ///
    /// # Arguments
    /// * `start` - The starting point of the trajectory
    /// * `segments` - A slice of compressed trajectory segments
    /// * `start_addr` - The address in trajectory memory to upload to (0 by default)
    /// * `progress_callback` - Called with (bytes_written, total_bytes)
    ///
    /// # Returns
    /// The number of bytes written
    pub async fn write_compressed_with_progress<F>(
        &self,
        start: &CompressedStart,
        segments: &[CompressedSegment],
        start_addr: usize,
        progress_callback: F,
    ) -> Result<usize>
    where
        F: FnMut(usize, usize),
    {
        let mut data = start.pack()?;
        for segment in segments {
            data.extend(segment.pack()?);
        }

        self.memory.write(start_addr, &data, Some(progress_callback)).await?;
        Ok(data.len())
    }

    /// Write raw packed trajectory data to the Crazyflie
    ///
    /// This is useful when you have pre-packed trajectory data.
    ///
    /// # Arguments
    /// * `data` - The raw trajectory data bytes
    /// * `start_addr` - The address in trajectory memory to upload to (0 by default)
    ///
    /// # Returns
    /// The number of bytes written
    pub async fn write_raw(&self, data: &[u8], start_addr: usize) -> Result<usize> {
        self.memory.write::<fn(usize, usize)>(start_addr, data, None).await?;
        Ok(data.len())
    }
}
