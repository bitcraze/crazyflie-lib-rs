use crazyflie_link::LinkContext;
use crazyflie_lib::Crazyflie;
use tokio::time::{sleep, Duration};

/// Example: Fly the Crazyflie to three positions using the high-level Rust API.
///
/// This example demonstrates how to move the Crazyflie to a sequence of three target positions `(x, y, z)` with specified yaw angles.
///
/// Uses: `Commander::setpoint_position(x, y, z, yaw)`

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let context = LinkContext::new();
    let crazyflie = Crazyflie::connect_from_uri(
        &context,
        "radio://0/80/2M/E7E7E7E7E7",
    )
    .await?;

    // Unlock thrust
    crazyflie.commander.setpoint_rpyt(0.0, 0.0, 0.0, 0).await?;

    // Define three target positions within the allowed constraints
    let positions = [
    //  (      x,       y,      z,      yaw),
        ( 0.2f32,  0.0f32, 0.2f32,  0.0f32 ),
        (-0.2f32,  0.2f32, 0.4f32,  22.5f32),
        ( 0.0f32, -0.2f32, 0.2f32, -45.0f32)
    ];

    for (i, &(x, y, z, yaw)) in positions.iter().enumerate() {
        println!("Flying to position {}: x={:.2}, y={:.2}, z={:.2}, yaw={:.1}", i+1, x, y, z, yaw);
        for _ in 0..20 {
            crazyflie.commander.setpoint_position(x, y, z, yaw).await?;
            sleep(Duration::from_millis(100)).await;
        }
    }

    // Stop the motors after the maneuvers
    crazyflie.commander.setpoint_rpyt(0.0, 0.0, 0.0, 0).await?;
    println!("Motors stopped.");
    Ok(())
}
