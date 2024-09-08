use crazyflie_lib::{subsystems::log::LogPeriod, Crazyflie};
use crazyflie_link::LinkContext;
use std::convert::TryInto;
use tokio::time::{Duration, sleep};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let context = LinkContext::new();
    let cf = Crazyflie::connect_from_uri(
        &context,
        "radio://0/80/2M/E7E7E7E7E7",
    )
    .await?;

    let mut block = cf.log.create_block().await?;

    block.add_variable("stateEstimate.roll").await?;
    block.add_variable("stateEstimate.pitch").await?;
    block.add_variable("stateEstimate.yaw").await?;

    let stream = block.start(Duration::from_millis(20).try_into()?).await?;

    for _ in 0..100 {
        let data = stream.next().await?;
        println!("{:?}", data);
    }

    let block = stream.stop().await?;

    println!(" --- Pausing log for 3 seconds --- ");
    sleep(Duration::from_secs(3)).await;

    let stream = block.start(LogPeriod::from_millis(10)?).await?;

    for _ in 0..100 {
        let data = stream.next().await?;
        println!("{:?}", data);
    }

    Ok(())
}
