use async_executors::AsyncStd;
use crazyflie_lib::{Crazyflie, log::LogPeriod};
use crazyflie_link::LinkContext;
use std::{convert::TryInto, sync::Arc, time::Duration};

#[async_std::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let context = LinkContext::new(Arc::new(AsyncStd));
    let cf = Crazyflie::connect_from_uri(&context, "radio://0/80/2M/E7E7E7E7E7").await;

    let mut block = cf.log.create_block().await.unwrap();

    block.add_variable("stateEstimate.roll").await?;
    block.add_variable("stateEstimate.pitch").await?;
    block.add_variable("stateEstimate.yaw").await?;
    
    let stream = block.start(Duration::from_millis(20).try_into()?).await?;

    for _ in 0..100 {
        let data = stream.next().await?;
        println!("{:?}", data);
    }

    let block = stream.stop().await?;

    println!(" --- Pausing log for 3 secons --- ");
    async_std::task::sleep(Duration::from_secs(3)).await;

    let stream = block.start(LogPeriod::from_millis(10)?).await?;

    for _ in 0..100 {
        let data = stream.next().await?;
        println!("{:?}", data);
    }

    Ok(())
}