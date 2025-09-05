//! High-level commander example demonstrating one approach to resilient drone control.
//!
//! When controlling real drones, individual command failures should not crash the program,
//! as this would leave the drone flying uncontrolled. This example shows a simple
//! error-handling pattern using explicit variables and `if let Err(e)` blocks.
//!
//! This approach:
//! - Continues the flight sequence if commands fail
//! - Sleeps for the command duration using the same variable
//! - Logs errors without terminating the program
//!
//! Direct calls like `commander.take_off(...).await?` will terminate the program on error.

use crazyflie_link::LinkContext;
use crazyflie_lib::Crazyflie;
use tokio::time::{sleep, Duration};
use std::f32::consts::PI;


#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let context = LinkContext::new();
    let crazyflie = Crazyflie::connect_from_uri(
        &context,
        "radio://0/80/2M/E7E7E7E7E7",
    )
    .await?;

    println!("Taking off...");
    let take_off_duration = 2.0;
    if let Err(e) = crazyflie.high_level_commander.take_off(0.5, None, take_off_duration, None).await {
        eprintln!("Take-off failed: {e}");
    }
    sleep(Duration::from_secs_f32(take_off_duration)).await;

    println!("Going to first position...");
    let go_to_duration = 2.0;
    if let Err(e) = crazyflie.high_level_commander.go_to(0.0, 0.5, 0.5, 0.0, go_to_duration, false, false, None).await {
        eprintln!("Go-to failed: {e}");
    }
    sleep(Duration::from_secs_f32(go_to_duration)).await;

    println!("Going to second position...");
    let go_to_duration2 = 2.0;
    if let Err(e) = crazyflie.high_level_commander.go_to(-0.25, 0.0, 0.5, 0.0, go_to_duration2, false, false, None).await {
        eprintln!("Go-to failed: {e}");
    }
    sleep(Duration::from_secs_f32(go_to_duration2)).await;

    println!("Moving in a spiral...");
    let spiral_duration = 2.0;
    if let Err(e) = crazyflie.high_level_commander.spiral(-PI*2.0, 0.5, 0.5, 0.0, spiral_duration, true, true, None).await {
        eprintln!("Spiral failed: {e}");
    }
    sleep(Duration::from_secs_f32(spiral_duration)).await;

    println!("Landing...");
    let land_duration = 2.0;
    if let Err(e) = crazyflie.high_level_commander.land(0.0, None, land_duration, None).await {
        eprintln!("Landing failed: {e}");
    }
    sleep(Duration::from_secs_f32(land_duration)).await;

    crazyflie.high_level_commander.stop(None).await?;
    println!("Done");
    Ok(())
}
