//! Lighthouse system configuration file
//!
//! This module reads and writes the YAML file format used to store a lighthouse
//! system configuration (base station geometry, calibration and system type).

use crate::{Error, Result};
use super::{LighthouseBsCalibration, LighthouseBsGeometry, LighthouseCalibrationSweep, LighthouseMemory};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

const FILE_TYPE: &str = "lighthouse_system_configuration";
const FILE_VERSION: &str = "1";

/// System type for Lighthouse V1 base stations
pub const LIGHTHOUSE_SYSTEM_TYPE_V1: u8 = 1;
/// System type for Lighthouse V2 base stations
pub const LIGHTHOUSE_SYSTEM_TYPE_V2: u8 = 2;

/// A lighthouse system configuration, as stored in a configuration file
///
/// Use [`from_yaml`](Self::from_yaml) to load a file and [`to_yaml`](Self::to_yaml)
/// to save one. The geometries and calibrations can be written to the Crazyflie with
/// [`LighthouseMemory::write_geometries`] and [`LighthouseMemory::write_calibrations`].
#[derive(Debug, Clone, PartialEq)]
pub struct LighthouseConfig {
    /// Lighthouse system type ([`LIGHTHOUSE_SYSTEM_TYPE_V1`] or [`LIGHTHOUSE_SYSTEM_TYPE_V2`])
    pub system_type: u8,
    /// Geometry data, mapping base station ID to geometry
    pub geometries: HashMap<u8, LighthouseBsGeometry>,
    /// Calibration data, mapping base station ID to calibration
    pub calibrations: HashMap<u8, LighthouseBsCalibration>,
}

impl LighthouseConfig {
    /// Parse a lighthouse configuration from YAML
    ///
    /// The file must have `type: lighthouse_system_configuration` and `version: '1'`.
    /// `systemType` defaults to [`LIGHTHOUSE_SYSTEM_TYPE_V2`] if missing, and `geos` and
    /// `calibs` default to empty. All geometries and calibrations in the file are marked valid.
    ///
    /// # Errors
    /// Returns [`Error::InvalidArgument`] if the YAML can not be parsed, if the file type or
    /// version is missing or not supported, if the system type is not 1 or 2, or if a base
    /// station ID is out of range (0-15).
    pub fn from_yaml(yaml: &str) -> Result<Self> {
        let file: ConfigFile = serde_yaml_ng::from_str(yaml)
            .map_err(|e| Error::InvalidArgument(format!("Invalid lighthouse config file: {}", e)))?;

        match file.file_type.as_deref() {
            None => return Err(invalid("type field missing")),
            Some(FILE_TYPE) => {}
            Some(other) => return Err(invalid(&format!("unsupported file type '{}'", other))),
        }
        match file.version.as_deref() {
            None => return Err(invalid("version field missing")),
            Some(FILE_VERSION) => {}
            Some(other) => return Err(invalid(&format!("unsupported file version '{}'", other))),
        }

        let system_type = file.system_type.unwrap_or(LIGHTHOUSE_SYSTEM_TYPE_V2);
        if system_type != LIGHTHOUSE_SYSTEM_TYPE_V1 && system_type != LIGHTHOUSE_SYSTEM_TYPE_V2 {
            return Err(invalid(&format!("unsupported system type {}", system_type)));
        }

        for &bs_id in file.geos.keys().chain(file.calibs.keys()) {
            if bs_id as usize >= LighthouseMemory::MAX_BASE_STATIONS {
                return Err(invalid(&format!(
                    "base station ID {} out of range (0-{})",
                    bs_id, LighthouseMemory::MAX_BASE_STATIONS - 1
                )));
            }
        }

        let geometries = file.geos.iter()
            .map(|(&bs_id, geo)| {
                let geometry = LighthouseBsGeometry {
                    origin: geo.origin,
                    rotation_matrix: geo.rotation,
                    valid: true,
                };
                (bs_id, geometry)
            })
            .collect();
        let calibrations = file.calibs.iter()
            .map(|(&bs_id, calib)| {
                let calibration = LighthouseBsCalibration {
                    sweeps: [(&calib.sweeps[0]).into(), (&calib.sweeps[1]).into()],
                    uid: calib.uid,
                    valid: true,
                };
                (bs_id, calibration)
            })
            .collect();

        Ok(Self { system_type, geometries, calibrations })
    }

    /// Serialize the configuration to YAML
    ///
    /// Base stations are written in ascending ID order. Geometries and calibrations
    /// that are not valid are left out, since the file format has no valid flag.
    pub fn to_yaml(&self) -> Result<String> {
        let file = ConfigFile {
            calibs: self.calibrations.iter()
                .filter(|(_, calib)| calib.valid)
                .map(|(&bs_id, calib)| {
                    let entry = CalibrationEntry {
                        sweeps: [(&calib.sweeps[0]).into(), (&calib.sweeps[1]).into()],
                        uid: calib.uid,
                    };
                    (bs_id, entry)
                })
                .collect(),
            geos: self.geometries.iter()
                .filter(|(_, geo)| geo.valid)
                .map(|(&bs_id, geo)| {
                    let entry = GeometryEntry {
                        origin: geo.origin,
                        rotation: geo.rotation_matrix,
                    };
                    (bs_id, entry)
                })
                .collect(),
            system_type: Some(self.system_type),
            file_type: Some(FILE_TYPE.to_owned()),
            version: Some(FILE_VERSION.to_owned()),
        };

        serde_yaml_ng::to_string(&file)
            .map_err(|e| Error::InvalidArgument(format!("Could not serialize lighthouse config: {}", e)))
    }
}

fn invalid(reason: &str) -> Error {
    Error::InvalidArgument(format!("Invalid lighthouse config file: {}", reason))
}

/// The file layout. Fields are in alphabetical order, the same order cflib writes them.
#[derive(Debug, Serialize, Deserialize)]
struct ConfigFile {
    #[serde(default)]
    calibs: BTreeMap<u8, CalibrationEntry>,
    #[serde(default)]
    geos: BTreeMap<u8, GeometryEntry>,
    #[serde(rename = "systemType")]
    system_type: Option<u8>,
    #[serde(rename = "type")]
    file_type: Option<String>,
    version: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct GeometryEntry {
    origin: [f32; 3],
    rotation: [[f32; 3]; 3],
}

#[derive(Debug, Serialize, Deserialize)]
struct CalibrationEntry {
    sweeps: [SweepEntry; 2],
    uid: u32,
}

#[derive(Debug, Serialize, Deserialize)]
struct SweepEntry {
    curve: f32,
    gibmag: f32,
    gibphase: f32,
    ogeemag: f32,
    ogeephase: f32,
    phase: f32,
    tilt: f32,
}

impl From<&LighthouseCalibrationSweep> for SweepEntry {
    fn from(sweep: &LighthouseCalibrationSweep) -> Self {
        Self {
            curve: sweep.curve,
            gibmag: sweep.gibmag,
            gibphase: sweep.gibphase,
            ogeemag: sweep.ogeemag,
            ogeephase: sweep.ogeephase,
            phase: sweep.phase,
            tilt: sweep.tilt,
        }
    }
}

impl From<&SweepEntry> for LighthouseCalibrationSweep {
    fn from(entry: &SweepEntry) -> Self {
        Self {
            phase: entry.phase,
            tilt: entry.tilt,
            curve: entry.curve,
            gibmag: entry.gibmag,
            gibphase: entry.gibphase,
            ogeemag: entry.ogeemag,
            ogeephase: entry.ogeephase,
        }
    }
}
