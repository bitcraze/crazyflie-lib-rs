/// Commander example that demonstrates starting a predefined trajectory.
///
/// NOTE: This assumes a trajectory has already been uploaded to the Crazyflie's
/// trajectory memory at `MEMORY_OFFSET` with `PIECE_COUNT` pieces.
/// The Rust memory upload subsystem is not implemented yet.
///
/// Unlike other high-level commander methods, `start_trajectory` is non-blocking
/// because the trajectory duration is determined by the uploaded data, which the
/// library doesn't currently have access to. Users must manually sleep for the expected duration.


use crazyflie_link::LinkContext;
use crazyflie_lib::Crazyflie;
use tokio::time::{sleep, Duration};


const TRAJECTORY_ID: u8 = 1;
const MEMORY_OFFSET: u32 = 0;
const PIECE_COUNT: u8 = 10;        // adjust to your uploaded trajectory
const TIME_SCALE: f32 = 1.0;       // 1.0 = original timing
const EXPECTED_DURATION_S: u64 = 6; // rough duration of the uploaded trajectory

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let context = LinkContext::new();
    let crazyflie = Crazyflie::connect_from_uri(
        &context,
        "radio://0/80/2M/E7E7E7E7E7",
    )
    .await?;

    // Bind trajectory ID to memory (assumes data is already uploaded there).
    println!("Defining trajectory...");
    if let Err(e) = crazyflie
        .high_level_commander
        .define_trajectory(TRAJECTORY_ID, MEMORY_OFFSET, PIECE_COUNT, None)
        .await
    {
        eprintln!("Define trajectory failed: {e}");
    }
    sleep(Duration::from_secs(1)).await;

    println!("Taking off...");
    crazyflie.high_level_commander.take_off(0.8, None, 2.0, None).await?;

    println!("Starting trajectory...");
    if let Err(e) = crazyflie
        .high_level_commander
        .start_trajectory(TRAJECTORY_ID, TIME_SCALE, true, false, false, None)
        .await
    {
        eprintln!("Start trajectory failed: {e}");
    }
    sleep(Duration::from_secs(EXPECTED_DURATION_S)).await;

    println!("Landing...");
    if let Err(e) = crazyflie.high_level_commander.land(0.0, None, 2.0, None).await {
        eprintln!("Land command failed: {e}");
    }

    crazyflie.high_level_commander.stop(None).await?;
    println!("Done");
    Ok(())
}
