use crazyflie_link::LinkContext;
use crazyflie_lib::Crazyflie;
use tokio::time::{sleep, Duration};
use std::f32::consts::PI;

/// Commander example that demonstrates control using the high-level commander

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let context = LinkContext::new();
    let crazyflie = Crazyflie::connect_from_uri(
        &context,
        "radio://0/90/2M/F00D2BEFED",
    )
    .await?;

    println!("Taking off...");
    crazyflie.high_level_commander.take_off(0.5, None, 2.0, None).await?;
    sleep(Duration::from_secs(2)).await;

    crazyflie.high_level_commander.go_to(0.0, 0.5, 0.5, 0.0, 2.0, false, false, None).await?;
    sleep(Duration::from_secs(2)).await;

    if let Err(e) = crazyflie.high_level_commander.go_to(-0.25, 0.0, 0.5, 0.0, 2.0, false, false, None).await {
        eprintln!("Go-to command failed: {e}");
    }
    sleep(Duration::from_secs(2)).await;

    println!("Moving in a spiral...");
    if let Err(e) = crazyflie.high_level_commander.spiral(-PI*2.0, 0.5, 0.5, 0.0, 2.0, true, true, None).await {
        eprintln!("Spiral command failed: {e}");
    }
    sleep(Duration::from_secs(2)).await;

    println!("Landing...");
    if let Err(e) = crazyflie.high_level_commander.land(0.0, None, 2.0, None).await {
        eprintln!("Land command failed: {e}");
    }
    sleep(Duration::from_secs(2)).await;

    crazyflie.high_level_commander.stop(None).await?;
    println!("Done");
    Ok(())
}
