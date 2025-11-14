/// Example demonstrating Lighthouse angle streaming
///
/// This example:
/// 1. Connects to a Crazyflie with a Lighthouse deck
/// 2. Enables lighthouse angle streaming
/// 3. Displays received angle data from visible base stations
///
/// REQUIREMENTS:
/// - Crazyflie with Lighthouse deck
/// - Lighthouse V2 base stations visible to the Crazyflie
/// - Base station calibration data already received by the Crazyflie

use crazyflie_lib::Crazyflie;
use crazyflie_link::LinkContext;
use futures::StreamExt;
use std::collections::HashSet;
use std::time::{Duration, Instant};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let link_context = LinkContext::new();

    // Connect to Crazyflie
    let uri = std::env::var("CFURI").unwrap_or_else(|_| "radio://0/80/2M/E7E7E7E7E7".to_string());
    println!("Connecting to {} ...", uri);

    let crazyflie = Crazyflie::connect_from_uri(&link_context, &uri, crazyflie_lib::NoTocCache).await?;
    println!("Connected!");

    // Set lighthouse system to V2 mode
    println!("\nSetting lighthouse to V2 mode...");
    crazyflie.param.set("lighthouse.systemType", 2u8).await?;
    println!("Lighthouse set to V2 mode");

    // Enable lighthouse angle streaming
    println!("\nEnabling lighthouse angle stream...");
    crazyflie.param.set("locSrv.enLhAngleStream", 1u8).await?;
    println!("Angle streaming enabled");

    println!("\nStreaming lighthouse angles for 10 seconds...");
    println!("Move the Crazyflie around to see different base stations\n");

    let mut angle_stream = crazyflie.localization.lighthouse.angle_stream().await;

    let start = Instant::now();
    let duration = Duration::from_secs(10);
    let mut base_stations_seen = HashSet::new();
    let mut sample_count = 0;

    while start.elapsed() < duration {
        // Use timeout to check elapsed time periodically
        match tokio::time::timeout(Duration::from_millis(1), angle_stream.next()).await {
            Ok(Some(angle_data)) => {
                sample_count += 1;
                base_stations_seen.insert(angle_data.base_station);

                println!(
                    "BS {}: x=[{:6.3}, {:6.3}, {:6.3}, {:6.3}], y=[{:6.3}, {:6.3}, {:6.3}, {:6.3}]",
                    angle_data.base_station,
                    angle_data.angles.x[0],
                    angle_data.angles.x[1],
                    angle_data.angles.x[2],
                    angle_data.angles.x[3],
                    angle_data.angles.y[0],
                    angle_data.angles.y[1],
                    angle_data.angles.y[2],
                    angle_data.angles.y[3],
                );
            }
            Ok(None) => {
                println!("Stream ended unexpectedly");
                break;
            }
            Err(_) => {
                // Timeout - just continue to check elapsed time
            }
        }
    }

    // Disable angle streaming
    println!("\nDisabling angle stream...");
    crazyflie.param.set("locSrv.enLhAngleStream", 0u8).await?;

    println!("\nSummary:");
    println!("  Total samples: {}", sample_count);
    println!("  Base stations seen: {:?}", {
        let mut bs_vec: Vec<_> = base_stations_seen.iter().collect();
        bs_vec.sort();
        bs_vec
    });

    Ok(())
}
