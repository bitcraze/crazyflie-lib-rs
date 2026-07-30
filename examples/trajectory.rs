//! Example: upload and run a predefined trajectory via the high-level commander.
use crazyflie_lib::Crazyflie;
use crazyflie_lib::subsystems::memory::{MemoryType, Poly, Poly4D, TrajectoryMemory};
use crazyflie_lib::crazyflie_link::LinkContext;
use tokio::time::{Duration, sleep};

const TRAJECTORY_ID: u8 = 1;
const MEMORY_OFFSET: u32 = 0;
const TIME_SCALE: f32 = 1.0;        // 1.0 = original timing
const EXPECTED_DURATION_S: u64 = 3; // sum of segment durations (3 × 1.0s)

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let context = LinkContext::new();
    let crazyflie = Crazyflie::connect_from_uri(
        &context,
        "radio://0/80/2M/E7E7E7E7E7",
        crazyflie_lib::NoTocCache,
    )
    .await?;

    // Flies a triangle at 0.5 m: (0,0) -> (1,0) -> (0.5,1) -> (0,0)
    let z = Poly::from_slice(&[0.5]);
    let yaw = Poly::from_slice(&[]);

    let x = Poly::from_slice(&[0.0, 1.0]);
    let y = Poly::from_slice(&[]);
    let segment1 = Poly4D::new(1.0, x, y, z.clone(), yaw.clone());

    let x = Poly::from_slice(&[1.0, -0.5]);
    let y = Poly::from_slice(&[0.0, 1.0]);
    let segment2 = Poly4D::new(1.0, x, y, z.clone(), yaw.clone());

    let x = Poly::from_slice(&[0.5, -0.5]);
    let y = Poly::from_slice(&[1.0, -1.0]);
    let segment3 = Poly4D::new(1.0, x, y, z, yaw);

    let segments = vec![segment1, segment2, segment3];

    // Open the trajectory memory and upload the segments.
    let memory_device = crazyflie
        .memory
        .get_memories(Some(MemoryType::Trajectory))
        .pop()
        .cloned()
        .ok_or("No trajectory memory device found.")?;

    let trajectory_memory: TrajectoryMemory =
        crazyflie
            .memory
            .open_memory(memory_device)
            .await
            .ok_or("Trajectory memory not found.")??;

    println!("Uploading trajectory...");
    trajectory_memory
        .write_uncompressed(&segments, MEMORY_OFFSET as usize)
        .await?;

    // Register the uploaded trajectory under an ID the high-level commander can run.
    println!("Defining trajectory...");
    crazyflie
        .high_level_commander
        .define_trajectory(TRAJECTORY_ID, MEMORY_OFFSET, segments.len() as u8, None)
        .await?;

    println!("Taking off...");
    crazyflie
        .high_level_commander
        .take_off(0.5, None, 2.0, None)
        .await?;
    sleep(Duration::from_millis(2100)).await; // wait out the 2.0s take-off

    println!("Starting trajectory...");
    crazyflie
        .high_level_commander
        .start_trajectory(TRAJECTORY_ID, TIME_SCALE, true, false, false, None)
        .await?;
    sleep(Duration::from_secs(EXPECTED_DURATION_S)).await;

    println!("Landing...");
    crazyflie
        .high_level_commander
        .land(0.0, None, 2.0, None)
        .await?;
    sleep(Duration::from_secs(2)).await; // wait out the 2.0s land

    crazyflie.high_level_commander.stop(None).await?; // motors off
    println!("Done");
    Ok(())
}
