/// Simple example that ramps the motor thrust and then stops them
use crazyflie_lib::Crazyflie;
use crazyflie_link::LinkContext;
use tokio::time::{Duration, sleep};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let context = LinkContext::new();
    let crazyflie = Crazyflie::connect_from_uri(
        &context,
        "radio://0/80/2M/E7E7E7E7E7",
        crazyflie_lib::NoTocCache
    )
    .await?;

    // Unlock thrust
    crazyflie.commander.setpoint_rpyt(0.0, 0.0, 0.0, 0).await?;

    let runtime = Duration::from_secs(5);
    let steps = 100;
    let max_thrust = 10_000u16;

    for thrust in (0..max_thrust).step_by((max_thrust / steps) as usize) {
        crazyflie
            .commander
            .setpoint_rpyt(0.0, 0.0, 0.0, thrust)
            .await?;

        sleep(runtime / (steps as u32)).await;
    }

    Ok(())
}
