use crazyflie_link::LinkContext;
use crazyflie_lib::Crazyflie;
use tokio::time::{sleep, Duration};

/// Example: Demonstrate hover setpoint control using the Rust API.
///
/// This example ramps up the hover height, hovers in place, then performs yaw maneuvers while maintaining altitude,
/// and finally ramps down the hover height before stopping the motors and notifying the firmware.
///
/// Uses: `Commander::setpoint_hover(vx, vy, yawrate, zdistance)`

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
    sleep(Duration::from_millis(100)).await;

    println!("Starting hover maneuver...");

    // Ramp up hover height
    for y in 0..10 {
        let zdistance = y as f32 / 25.0;
        crazyflie.commander.setpoint_hover(0.0, 0.0, 0.0, zdistance).await?;
        sleep(Duration::from_millis(100)).await;
    }

    // Hover in place
    for _ in 0..20 {
        crazyflie.commander.setpoint_hover(0.0, 0.0, 0.0, 0.4).await?;
        sleep(Duration::from_millis(100)).await;
    }

    // Yaw right while hovering
    for _ in 0..50 {
        crazyflie.commander.setpoint_hover(0.2, 0.0, 72.0, 0.4).await?;
        sleep(Duration::from_millis(100)).await;
    }

    // Yaw left while hovering
    for _ in 0..50 {
        crazyflie.commander.setpoint_hover(0.2, 0.0, -72.0, 0.4).await?;
        sleep(Duration::from_millis(100)).await;
    }

    // Hover in place again
    for _ in 0..20 {
        crazyflie.commander.setpoint_hover(0.0, 0.0, 0.0, 0.4).await?;
        sleep(Duration::from_millis(100)).await;
    }

    // Ramp down hover height
    for y in 0..10 {
        let zdistance = (10 - y) as f32 / 25.0;
        crazyflie.commander.setpoint_hover(0.0, 0.0, 0.0, zdistance).await?;
        sleep(Duration::from_millis(100)).await;
    }

    // Stop the motors
    crazyflie.commander.setpoint_stop().await?;
    // Notify firmware to hand control to high-level commander
    crazyflie.commander.notify_setpoint_stop(0).await?;
    println!("Hover maneuver complete. Motors stopped.");
    Ok(())
}
