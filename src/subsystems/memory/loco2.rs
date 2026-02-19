//! Loco Positioning System v2 memory for anchor position data
//!
//! This module provides types and functionality for reading Loco Positioning
//! System anchor data from the Crazyflie. This includes anchor IDs, active
//! anchor IDs, and anchor position data.

use crate::{Error, Result, subsystems::memory::{MemoryBackend, memory_types}};
use memory_types::{FromMemoryBackend, MemoryType};
use std::collections::HashMap;

const SIZE_FLOAT: usize = std::mem::size_of::<f32>();

const MAX_NR_OF_ANCHORS: usize = 16;
const ID_LIST_LEN: usize = 1 + MAX_NR_OF_ANCHORS;

const ADR_ID_LIST: usize = 0x0000;
const ADR_ACTIVE_ID_LIST: usize = 0x1000;
const ADR_ANCHOR_BASE: usize = 0x2000;

const ANCHOR_PAGE_SIZE: usize = 0x0100;
const ANCHOR_DATA_LEN: usize = 3 * SIZE_FLOAT + 1;

/// Data for a single Loco Positioning anchor
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LocoAnchorData {
    /// 3D position (x, y, z) in meters
    pub position: [f32; 3],
    /// Whether this anchor has valid data
    pub is_valid: bool,
}

impl LocoAnchorData {
    fn from_bytes(data: &[u8]) -> Result<Self> {
        if data.len() < ANCHOR_DATA_LEN {
            return Err(Error::MemoryError(format!(
                "Insufficient data for anchor: expected {} bytes, got {}",
                ANCHOR_DATA_LEN, data.len()
            )));
        }

        let x = f32::from_le_bytes(data[0..4].try_into().unwrap());
        let y = f32::from_le_bytes(data[4..8].try_into().unwrap());
        let z = f32::from_le_bytes(data[8..12].try_into().unwrap());
        let is_valid = data[12] != 0;

        Ok(Self {
            position: [x, y, z],
            is_valid,
        })
    }
}

/// Memory interface for Loco Positioning System v2 data
///
/// Provides methods to read anchor IDs, active anchor IDs, and anchor
/// position data from the Crazyflie's LPS memory.
#[derive(Debug)]
pub struct LocoMemory2 {
    memory: MemoryBackend,
}

impl LocoMemory2 {
    fn from_backend(memory: MemoryBackend) -> Result<Self> {
        if memory.memory_type == MemoryType::Loco2 {
            Ok(Self { memory })
        } else {
            Err(Error::MemoryError(format!(
                "Expected Loco2 memory type, got {:?}",
                memory.memory_type
            )))
        }
    }

    /// Read the list of configured anchor IDs
    ///
    /// Returns a vector of anchor IDs that are configured in the system.
    pub async fn read_id_list(&self) -> Result<Vec<u8>> {
        let data = self.memory.read::<fn(usize, usize)>(ADR_ID_LIST, ID_LIST_LEN, None).await?;
        let count = data[0] as usize;
        if count > MAX_NR_OF_ANCHORS {
            return Err(Error::MemoryError(format!(
                "Anchor count {} exceeds maximum {}", count, MAX_NR_OF_ANCHORS
            )));
        }
        Ok(data[1..1 + count].to_vec())
    }

    /// Read the list of currently active anchor IDs
    ///
    /// Returns a vector of anchor IDs that are currently active.
    pub async fn read_active_id_list(&self) -> Result<Vec<u8>> {
        let data = self.memory.read::<fn(usize, usize)>(ADR_ACTIVE_ID_LIST, ID_LIST_LEN, None).await?;
        let count = data[0] as usize;
        if count > MAX_NR_OF_ANCHORS {
            return Err(Error::MemoryError(format!(
                "Active anchor count {} exceeds maximum {}", count, MAX_NR_OF_ANCHORS
            )));
        }
        Ok(data[1..1 + count].to_vec())
    }

    /// Read position data for a single anchor
    ///
    /// # Arguments
    /// * `anchor_id` - The anchor ID (0-15, as stored in the ID list)
    pub async fn read_anchor_data(&self, anchor_id: u8) -> Result<LocoAnchorData> {
        if anchor_id as usize >= MAX_NR_OF_ANCHORS {
            return Err(Error::MemoryError(format!(
                "Anchor ID {} out of range (0-{})",
                anchor_id,
                MAX_NR_OF_ANCHORS - 1
            )));
        }
        let addr = ADR_ANCHOR_BASE + ANCHOR_PAGE_SIZE * anchor_id as usize;
        let data = self.memory.read::<fn(usize, usize)>(addr, ANCHOR_DATA_LEN, None).await?;
        LocoAnchorData::from_bytes(&data)
    }

    /// Read all anchor data
    ///
    /// Reads the ID list, active ID list, then fetches position data for each
    /// configured anchor. Returns a struct containing all the information.
    pub async fn read_all(&self) -> Result<LocoSystemData> {
        let anchor_ids = self.read_id_list().await?;
        let active_ids = self.read_active_id_list().await?;

        let mut anchors = HashMap::new();
        for &id in &anchor_ids {
            let data = self.read_anchor_data(id).await?;
            anchors.insert(id, data);
        }

        Ok(LocoSystemData {
            anchor_ids,
            active_anchor_ids: active_ids,
            anchors,
        })
    }
}

/// Complete snapshot of the Loco Positioning System state
#[derive(Debug, Clone)]
pub struct LocoSystemData {
    /// List of configured anchor IDs
    pub anchor_ids: Vec<u8>,
    /// List of currently active anchor IDs
    pub active_anchor_ids: Vec<u8>,
    /// Anchor position data, keyed by anchor ID
    pub anchors: HashMap<u8, LocoAnchorData>,
}

impl FromMemoryBackend for LocoMemory2 {
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
