/// Example demonstrating emergency stop watchdog failsafe behavior
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

    println!("Connected! Demonstrating emergency watchdog...");

    // TODO: show how to check if supervisor has already locked (in log field)

    // Arm, unlock, start motors
    crazyflie.platform.send_arming_request(true).await?;
    sleep(Duration::from_millis(300)).await;
    crazyflie.commander.setpoint_rpyt(0.0, 0.0, 0.0, 0).await?;

    let cf_clone = Arc::clone(&crazyflie);
    let spin_task = tokio::spawn(async move {
        loop {
            if let Err(_) = cf_clone.commander.setpoint_rpyt(0.0, 0.0, 0.0, 8_000).await {
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }
    });
    sleep(Duration::from_millis(300)).await;

    // Activate watchdog
    println!("Activating watchdog (1000ms timeout)...");
    crazyflie.localization.emergency.send_emergency_stop_watchdog().await?;

    print!("Sending periodic messages: ");
    for i in 0..6 {
        print!("{}", i + 1);
        std::io::Write::flush(&mut std::io::stdout()).unwrap();
        crazyflie.localization.emergency.send_emergency_stop_watchdog().await?;
        if i < 5 {
            print!("...");
            std::io::Write::flush(&mut std::io::stdout()).unwrap();
            sleep(Duration::from_millis(800)).await;
        }
    }
    println!();

    println!("STOPPED sending - motors should stop in ~1000ms...");

    // Wait longer than the 1000ms timeout to trigger the watchdog
    sleep(Duration::from_millis(2000)).await;

    println!("Watchdog triggered! Drone is now locked and requires reboot.");

    spin_task.abort();

    Ok(())
}