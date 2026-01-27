//! High-level commander example demonstrating one approach to resilient drone control.
//!
//! When controlling real drones, individual command failures should not crash the program,
//! as this would leave the drone flying uncontrolled. This example shows a simple
//! error-handling pattern using `if let Err(e)` blocks.
//!
//! This approach:
//! - Continues the flight sequence if commands fail
//! - Logs errors without terminating the program
//!
//! Direct calls like `commander.take_off(...).await?` will terminate the program on error.
//!
//! Note: High-level commander methods return immediately after sending the command,
//! so manual sleeps are needed to wait for each movement to complete.

use crazyflie_link::LinkContext;
use crazyflie_lib::Crazyflie;
use std::f32::consts::PI;
use tokio::time::{sleep, Duration};


#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let context = LinkContext::new();
    let crazyflie = Crazyflie::connect_from_uri(
        &context,
        "radio://0/80/2M/E7E7E7E7E7",
        crazyflie_lib::NoTocCache
    )
    .await?;

    println!("Taking off...");
    if let Err(e) = crazyflie.high_level_commander.take_off(0.5, None, 2.0, None).await {
        eprintln!("Take-off failed: {e}");
    }
    sleep(Duration::from_secs_f32(2.0)).await;

    println!("Going to first position...");
    if let Err(e) = crazyflie.high_level_commander.go_to(0.0, 0.5, 0.5, 0.0, 2.0, false, false, None).await {
        eprintln!("Go-to failed: {e}");
    }
    sleep(Duration::from_secs_f32(2.0)).await;

    println!("Going to second position...");
    if let Err(e) = crazyflie.high_level_commander.go_to(-0.25, 0.0, 0.5, 0.0, 2.0, false, false, None).await {
        eprintln!("Go-to failed: {e}");
    }
    sleep(Duration::from_secs_f32(2.0)).await;

    println!("Moving in a spiral...");
    if let Err(e) = crazyflie.high_level_commander.spiral(-PI*2.0, 0.5, 0.5, 0.0, 2.0, true, true, None).await {
        eprintln!("Spiral failed: {e}");
    }
    sleep(Duration::from_secs_f32(2.0)).await;

    println!("Landing...");
    if let Err(e) = crazyflie.high_level_commander.land(0.0, None, 2.0, None).await {
        eprintln!("Landing failed: {e}");
    }
    sleep(Duration::from_secs_f32(2.0)).await;

    crazyflie.high_level_commander.stop(None).await?;
    println!("Done");
    Ok(())
}
