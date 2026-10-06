/// Example demonstrating reading and writing the Lighthouse configuration
///
/// This example:
/// - `read`: reads base station geometry and calibration data from the
///   Crazyflie and prints it as YAML
/// - `write <file>`: loads a YAML file, writes geometry and calibration data
///   to the Crazyflie and persists it to permanent storage
///
/// The YAML file uses the same format as the Python cflib and cfclient.
///
/// Usage:
///   cargo run --example lighthouse_config -- read
///   cargo run --example lighthouse_config -- write lighthouse.yaml
///
/// REQUIREMENTS:
/// - Crazyflie with Lighthouse deck

use crazyflie_lib::Crazyflie;
use crazyflie_lib::crazyflie_link::LinkContext;
use crazyflie_lib::subsystems::memory::{
    LighthouseBsCalibration, LighthouseBsGeometry, LighthouseCalibrationSweep, LighthouseMemory,
    LighthouseWriteReport, MemoryType,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// Lighthouse configuration file
#[derive(Debug, Serialize, Deserialize)]
struct ConfigFile {
    #[serde(default)]
    calibs: BTreeMap<u8, CalibrationEntry>,
    #[serde(default)]
    geos: BTreeMap<u8, GeometryEntry>,
    #[serde(rename = "systemType")]
    system_type: u8,
    #[serde(rename = "type")]
    file_type: String,
    version: String,
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let args: Vec<String> = std::env::args().collect();
    let path = match args.get(1..).unwrap_or_default() {
        [command] if command == "read" => None,
        [command, path] if command == "write" => Some(path.clone()),
        _ => {
            eprintln!("Usage: {} read | write <file.yaml>", args[0]);
            std::process::exit(1);
        }
    };

    let link_context = LinkContext::new();

    // Connect to Crazyflie
    let uri = std::env::var("CFURI").unwrap_or_else(|_| "radio://0/80/2M/E7E7E7E7E7".to_string());
    println!("Connecting to {} ...", uri);

    let crazyflie = Crazyflie::connect_from_uri(&link_context, &uri, crazyflie_lib::NoTocCache).await?;
    println!("Connected!");

    let memories = crazyflie.memory.get_memories(Some(MemoryType::Lighthouse));
    let Some(memory) = memories.first() else {
        println!("No lighthouse memory found. Is the Lighthouse deck attached?");
        crazyflie.disconnect().await;
        return Ok(());
    };

    let lighthouse = match crazyflie.memory.open_memory::<LighthouseMemory>((*memory).clone()).await {
        Some(Ok(lighthouse)) => lighthouse,
        Some(Err(e)) => return Err(format!("Could not access lighthouse memory: {}", e).into()),
        None => return Err("Lighthouse memory not found".into()),
    };

    let result = match path {
        None => read_config(&crazyflie, &lighthouse).await,
        Some(path) => write_config(&crazyflie, &lighthouse, &path).await,
    };

    crazyflie.memory.close_memory(lighthouse).await?;
    crazyflie.disconnect().await;

    result
}

/// Read the configuration from the Crazyflie and print it as YAML
async fn read_config(
    crazyflie: &Crazyflie,
    lighthouse: &LighthouseMemory,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("\nReading geometries...");
    let geometries = lighthouse.read_all_geometries().await?;
    println!("Reading calibrations...");
    let calibrations = lighthouse.read_all_calibrations().await?;
    let system_type: u8 = crazyflie.param.get("lighthouse.systemType").await?;

    let config = ConfigFile {
        calibs: calibrations
            .iter()
            .map(|(&bs_id, calib)| {
                let entry = CalibrationEntry {
                    sweeps: [(&calib.sweeps[0]).into(), (&calib.sweeps[1]).into()],
                    uid: calib.uid,
                };
                (bs_id, entry)
            })
            .collect(),
        geos: geometries
            .iter()
            .map(|(&bs_id, geo)| {
                let entry = GeometryEntry {
                    origin: geo.origin,
                    rotation: geo.rotation_matrix,
                };
                (bs_id, entry)
            })
            .collect(),
        system_type,
        file_type: "lighthouse_system_configuration".to_string(),
        version: "1".to_string(),
    };

    println!("\n{}", serde_yaml::to_string(&config)?);

    Ok(())
}

/// Load a YAML file and write the configuration to the Crazyflie
async fn write_config(
    crazyflie: &Crazyflie,
    lighthouse: &LighthouseMemory,
    path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let config: ConfigFile = serde_yaml::from_str(&std::fs::read_to_string(path)?)?;
    println!("\nLoaded {}", path);

    // Start with empty (invalid) data in all 16 slots, so base stations that are
    // not in the file are cleared on the Crazyflie. Slots above what the
    // Crazyflie supports are rejected by the firmware and skipped.
    let mut geometries: HashMap<u8, LighthouseBsGeometry> = (0..LighthouseMemory::MAX_BASE_STATIONS as u8)
        .map(|bs_id| (bs_id, LighthouseBsGeometry::default()))
        .collect();
    let mut calibrations: HashMap<u8, LighthouseBsCalibration> = (0..LighthouseMemory::MAX_BASE_STATIONS as u8)
        .map(|bs_id| (bs_id, LighthouseBsCalibration::default()))
        .collect();

    for (&bs_id, entry) in &config.geos {
        let geometry = LighthouseBsGeometry {
            origin: entry.origin,
            rotation_matrix: entry.rotation,
            valid: true,
        };
        geometries.insert(bs_id, geometry);
    }
    for (&bs_id, entry) in &config.calibs {
        let calibration = LighthouseBsCalibration {
            sweeps: [(&entry.sweeps[0]).into(), (&entry.sweeps[1]).into()],
            uid: entry.uid,
            valid: true,
        };
        calibrations.insert(bs_id, calibration);
    }

    println!("Writing geometries...");
    let geo_report = lighthouse.write_geometries(&geometries).await?;
    print_report(&geo_report, &config.geos);

    println!("Writing calibrations...");
    let calib_report = lighthouse.write_calibrations(&calibrations).await?;
    print_report(&calib_report, &config.calibs);

    println!("Setting system type to {}...", config.system_type);
    crazyflie.param.set("lighthouse.systemType", config.system_type).await?;

    // Only the written slots are persisted
    println!("Persisting data...");
    let persisted = crazyflie
        .localization
        .lighthouse
        .persist_lighthouse_data(&geo_report.written, &calib_report.written)
        .await?;

    if persisted {
        println!("✓ Configuration written and persisted!");
    } else {
        println!("✗ Persistence failed!");
    }

    Ok(())
}

/// Print which base stations from the file were written, which slots were
/// cleared, and which slots the Crazyflie does not support
fn print_report<T>(report: &LighthouseWriteReport, from_file: &BTreeMap<u8, T>) {
    let (written, cleared): (Vec<u8>, Vec<u8>) = report
        .written
        .iter()
        .partition(|bs_id| from_file.contains_key(bs_id));
    let (missing, unsupported): (Vec<u8>, Vec<u8>) = report
        .rejected
        .iter()
        .partition(|bs_id| from_file.contains_key(bs_id));

    println!("  Written from file: {:?}", written);
    println!("  Cleared: {:?}", cleared);
    println!("  Not supported by the Crazyflie: {:?}", unsupported);
    if !missing.is_empty() {
        println!("  Warning: base stations {:?} from the file are not supported by the Crazyflie", missing);
    }
}
