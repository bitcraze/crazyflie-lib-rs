/// Simple example that ramps the motor thrust and then stops them
use crazyflie_lib::Crazyflie;
use crazyflie_link::LinkContext;
use std::time::Duration;

#[async_std::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let context = LinkContext::new(async_executors::AsyncStd);
    let crazyflie = Crazyflie::connect_from_uri(
        async_executors::AsyncStd,
        &context,
        "radio://0/80/2M/E7E7E7E7E7",
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

        async_std::task::sleep(runtime / (steps as u32)).await;
    }

    Ok(())
}
