/// Example demonstrating emergency stop
use crazyflie_lib::Crazyflie;
use crazyflie_link::LinkContext;
use std::sync::Arc;
use tokio::time::{sleep, Duration};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let context = LinkContext::new();
    let crazyflie = Arc::new(Crazyflie::connect_from_uri(
        &context,
        "radio://0/80/2M/E7E7E7E7E7",
        crazyflie_lib::NoTocCache
    )
    .await?);

    println!("Connected! Demonstrating emergency stop...");

    // Arm, unlock, start motors
    crazyflie.platform.send_arming_request(true).await?;
    sleep(Duration::from_millis(300)).await;
    crazyflie.commander.setpoint_rpyt(0.0, 0.0, 0.0, 0).await?;

    let cf_clone = Arc::clone(&crazyflie);
    let spin_task = tokio::spawn(async move {
        loop {
            if let Err(_) = cf_clone.commander.setpoint_rpyt(0.0, 0.0, 0.0, 15_000).await {
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }
    });
    sleep(Duration::from_millis(1000)).await;

    println!("Sending emergency stop...");
    crazyflie.localization.emergency.send_emergency_stop().await?;

    // Wait a moment to see the effect
    sleep(Duration::from_millis(500)).await;

    spin_task.abort();

    println!("Emergency stop sent! Drone is now locked and requires reboot.");

    Ok(())
}