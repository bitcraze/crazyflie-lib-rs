//! Trajectory memory for the Crazyflie high level commander
//!
//! This module provides types and functionality for defining and uploading
//! trajectories to the Crazyflie's trajectory memory. Trajectories can be
//! either uncompressed (using `Poly4D`) or compressed (using `CompressedStart`
//! and `CompressedSegment`).

use crate::{Error, Result, subsystems::memory::{MemoryBackend, memory_types}};
use memory_types::{FromMemoryBackend, MemoryType};

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

    /// Create a new Poly4D with default (zero) polynomials
    pub fn with_duration(duration: f32) -> Self {
        Self {
            duration,
            x: Poly::default(),
            y: Poly::default(),
            z: Poly::default(),
            yaw: Poly::default(),
        }
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

    /// Encode a spatial coordinate (meters) to millimeters as i16
    fn encode_spatial(coordinate: f32) -> i16 {
        (coordinate * 1000.0) as i16
    }

    /// Encode a yaw angle (radians) to 1/10th degrees as i16
    fn encode_yaw(angle_rad: f32) -> i16 {
        (angle_rad.to_degrees() * 10.0) as i16
    }

    /// Pack this start point into bytes for transmission
    pub fn pack(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(8);

        data.extend_from_slice(&Self::encode_spatial(self.x).to_le_bytes());
        data.extend_from_slice(&Self::encode_spatial(self.y).to_le_bytes());
        data.extend_from_slice(&Self::encode_spatial(self.z).to_le_bytes());
        data.extend_from_slice(&Self::encode_yaw(self.yaw).to_le_bytes());

        data
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
    /// Duration of this segment in seconds
    pub duration: f32,
    /// X polynomial coefficients (0, 1, 3, or 7 elements)
    pub x: Vec<f32>,
    /// Y polynomial coefficients (0, 1, 3, or 7 elements)
    pub y: Vec<f32>,
    /// Z polynomial coefficients (0, 1, 3, or 7 elements)
    pub z: Vec<f32>,
    /// Yaw polynomial coefficients (0, 1, 3, or 7 elements)
    pub yaw: Vec<f32>,
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
            return Err(Error::TrajectoryError(
                "Element length must be 0, 1, 3, or 7".to_owned()
            ));
        }
        Ok(())
    }

    fn encode_type(element: &[f32]) -> u8 {
        ElementType::from_len(element.len()).unwrap() as u8
    }

    /// Encode a spatial coordinate (meters) to millimeters as i16
    fn encode_spatial(coordinate: f32) -> i16 {
        (coordinate * 1000.0) as i16
    }

    /// Encode a yaw angle (radians) to 1/10th degrees as i16
    fn encode_yaw(angle_rad: f32) -> i16 {
        (angle_rad.to_degrees() * 10.0) as i16
    }

    fn pack_spatial_element(element: &[f32]) -> Vec<u8> {
        let mut data = Vec::new();
        for &v in element {
            data.extend_from_slice(&Self::encode_spatial(v).to_le_bytes());
        }
        data
    }

    fn pack_yaw_element(element: &[f32]) -> Vec<u8> {
        let mut data = Vec::new();
        for &v in element {
            data.extend_from_slice(&Self::encode_yaw(v).to_le_bytes());
        }
        data
    }

    /// Pack this segment into bytes for transmission
    pub fn pack(&self) -> Vec<u8> {
        let element_types = (Self::encode_type(&self.x) << 0)
            | (Self::encode_type(&self.y) << 2)
            | (Self::encode_type(&self.z) << 4)
            | (Self::encode_type(&self.yaw) << 6);
        let duration_ms = (self.duration * 1000.0) as u16;

        let mut data = Vec::new();

        data.push(element_types);
        data.extend_from_slice(&duration_ms.to_le_bytes());
        data.extend(Self::pack_spatial_element(&self.x));
        data.extend(Self::pack_spatial_element(&self.y));
        data.extend(Self::pack_spatial_element(&self.z));
        data.extend(Self::pack_yaw_element(&self.yaw));

        data
    }
}

/// A trajectory element that can be packed for transmission
pub trait TrajectoryElement {
    /// Pack this element into bytes for transmission
    fn pack(&self) -> Vec<u8>;
}

impl TrajectoryElement for Poly4D {
    fn pack(&self) -> Vec<u8> {
        self.pack()
    }
}

impl TrajectoryElement for CompressedStart {
    fn pack(&self) -> Vec<u8> {
        self.pack()
    }
}

impl TrajectoryElement for CompressedSegment {
    fn pack(&self) -> Vec<u8> {
        self.pack()
    }
}

/// Memory interface for trajectories used by the high level commander
///
/// Trajectories can be either uncompressed (using `Poly4D` segments) or
/// compressed (using `CompressedStart` followed by `CompressedSegment`s).
/// It is not possible to mix uncompressed and compressed elements in the
/// same trajectory.
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
    /// Write trajectory data to the Crazyflie
    ///
    /// # Arguments
    /// * `trajectory` - A slice of trajectory elements to write
    /// * `start_addr` - The address in trajectory memory to upload to (0 by default)
    ///
    /// # Returns
    /// The number of bytes written
    pub async fn write_trajectory<T: TrajectoryElement>(
        &self,
        trajectory: &[T],
        start_addr: usize,
    ) -> Result<usize> {
        let mut data = Vec::new();
        for element in trajectory {
            data.extend(element.pack());
        }

        self.memory.write::<fn(usize, usize)>(start_addr, &data, None).await?;
        Ok(data.len())
    }

    /// Write trajectory data to the Crazyflie with progress reporting
    ///
    /// # Arguments
    /// * `trajectory` - A slice of trajectory elements to write
    /// * `start_addr` - The address in trajectory memory to upload to (0 by default)
    /// * `progress_callback` - A callback function that takes two usize arguments:
    ///   the number of bytes written so far and the total number of bytes to write
    ///
    /// # Returns
    /// The number of bytes written
    pub async fn write_trajectory_with_progress<T, F>(
        &self,
        trajectory: &[T],
        start_addr: usize,
        progress_callback: F,
    ) -> Result<usize>
    where
        T: TrajectoryElement,
        F: FnMut(usize, usize),
    {
        let mut data = Vec::new();
        for element in trajectory {
            data.extend(element.pack());
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
