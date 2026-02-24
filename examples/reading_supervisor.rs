use std::time::Duration;
use tokio::time::sleep;
use crazyflie_link::LinkContext;
use crazyflie_lib::Crazyflie;

// Example demonstrating how to read the Crazyflie's state through the supervisor.
// Hold the Crazyflie in your hand and tilt it upside down to observe the state changes.
// Once the tilt exceeds ~90°, the can_fly state becomes False and the is_tumbled state
// becomes True.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let context = LinkContext::new();
    let cf = Crazyflie::connect_from_uri(
        &context,
        "radio://0/80/2M/E7E7E7E7E7",
        crazyflie_lib::NoTocCache
    )
    .await?;

    // sleep(Duration::from_millis(500)).await;

    println!("Reading supervisor state:");
    for _ in 0..20 {
        println!("==============================================================================");

        match cf.supervisor.read_bitfield().await {
            Ok(info) => {
                println!("Can fly: {}", info.can_fly());
                println!("Is tumbled: {}", info.is_tumbled());
                println!("Bitfield: 0x{:04x}", info.raw);
                println!("Active states: {:?}", info.active_states());
            }
            Err(e) => {
                eprintln!("Error reading supervisor state: {}", e);
            }
        }

        println!("==============================================================================");
        sleep(Duration::from_millis(500)).await;
    }

    cf.disconnect().await;
    Ok(())
}
