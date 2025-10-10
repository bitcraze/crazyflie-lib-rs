/// Example demonstrating Lighthouse geometry persistence
///
/// This example shows how to persist lighthouse geometry and calibration data
/// to the Crazyflie's permanent storage.
///
/// IMPORTANT: This example assumes geometry/calibration data has already been
/// written to the Crazyflie's RAM via the memory subsystem (not shown here).
///
/// In a real scenario, you would:
/// 1. Estimate or load geometry data
/// 2. Write it to RAM via memory subsystem
/// 3. Use this persist function to save to permanent storage
///
/// REQUIREMENTS:
/// - Crazyflie with Lighthouse deck
/// - Geometry/calibration data already in RAM

use crazyflie_lib::Crazyflie;
use crazyflie_link::LinkContext;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let link_context = LinkContext::new();

    // Connect to Crazyflie
    let uri = std::env::var("CFURI").unwrap_or_else(|_| "radio://0/80/2M/E7E7E7E7E7".to_string());
    println!("Connecting to {} ...", uri);

    let crazyflie = Crazyflie::connect_from_uri(&link_context, &uri).await?;
    println!("Connected!");

    println!("\nThis example demonstrates the persist API.");
    println!("NOTE: Geometry/calibration data must already be in RAM (via memory subsystem)");

    // Example: Persist geometry for base stations 0 and 1, calibration for base station 0
    let geo_base_stations = vec![0, 1];  // Persist geometry for BS 0 and 1
    let calib_base_stations = vec![0];    // Persist calibration for BS 0

    println!(
        "\nPersisting geometry for base stations: {:?}",
        geo_base_stations
    );
    println!(
        "Persisting calibration for base stations: {:?}",
        calib_base_stations
    );

    // Send persist command
    crazyflie
        .localization
        .lighthouse
        .send_lh_persist_data_packet(&geo_base_stations, &calib_base_stations)
        .await?;

    println!("Persist command sent, waiting for confirmation...");

    // Wait for confirmation (with 5 second timeout)
    match crazyflie
        .localization
        .lighthouse
        .wait_persist_confirmation()
        .await
    {
        Ok(true) => {
            println!("✓ Data persisted successfully!");
        }
        Ok(false) => {
            println!("✗ Persistence failed!");
        }
        Err(e) => {
            println!("✗ Error waiting for confirmation: {}", e);
        }
    }

    println!("\nExample complete!");

    Ok(())
}
