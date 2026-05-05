use std::time::Duration;
use tokio::time::sleep;
use crazyflie_link::LinkContext;
use crazyflie_lib::Crazyflie;

// Simple example showing how to fly the Crazyflie using supervisor state information.
// Based on its current state, the Crazyflie will arm (if it can be armed), take
// off (if it can fly), and land (if it is flying). Before each action, we call
// the supervisor to check if the Crazyflie is crashed, locked or tumbled.
// Tested with the Flow deck V2 and the Lighthouse positioning system.

async fn safety_check(cf: &Crazyflie) -> Result<(), Box<dyn std::error::Error>> {
    let info = cf.supervisor.read_bitfield().await?;
    
    if info.is_crashed() {
        return Err("Crazyflie crashed!".into());
    }
    if info.is_locked() {
        return Err("Crazyflie locked!".into());
    }
    if info.is_tumbled() {
        return Err("Crazyflie tumbled!".into());
    }
    
    Ok(())
}

async fn run_sequence(cf: &Crazyflie) -> Result<(), Box<dyn std::error::Error>> {
    safety_check(cf).await?;
    
    let info = cf.supervisor.read_bitfield().await?;
    if info.can_be_armed() {
        println!("The Crazyflie can be armed...arming!");
        safety_check(cf).await?;
        cf.supervisor.send_arming_request(true).await?;
        sleep(Duration::from_secs(1)).await;
    }

    safety_check(cf).await?;
    
    let info = cf.supervisor.read_bitfield().await?;
    if info.can_fly() {
        println!("The Crazyflie can fly...taking off!");
        if let Err(e) = cf.high_level_commander.take_off(1.0, None, 2.0, None).await {
            eprintln!("Take-off failed: {e}");
        }
        sleep(Duration::from_secs(3)).await;
    }

    safety_check(cf).await?;
    
    let info = cf.supervisor.read_bitfield().await?;
    if info.is_flying() {
        println!("The Crazyflie is flying...landing!");
        if let Err(e) = cf.high_level_commander.land(0.0, None, 2.0, None).await {
            eprintln!("Landing failed: {e}");
        }
        sleep(Duration::from_secs(3)).await;
    }

    safety_check(cf).await?;
    
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let context = LinkContext::new();
    let cf = Crazyflie::connect_from_uri(
        &context,
        "radio://0/80/2M/E7E7E7E7E7",
        crazyflie_lib::NoTocCache
    )
    .await?;

    sleep(Duration::from_millis(500)).await;

    match run_sequence(&cf).await {
        Ok(_) => println!("Sequence completed successfully!"),
        Err(e) => eprintln!("Safety check failed: {}", e),
    }

    cf.disconnect().await;
    Ok(())
}
