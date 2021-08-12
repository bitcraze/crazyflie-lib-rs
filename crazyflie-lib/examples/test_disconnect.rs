use async_executors::AsyncStd;
use crazyflie_lib::Crazyflie;
use crazyflie_link::LinkContext;
use std::{sync::Arc, time::Duration};

#[async_std::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let context = LinkContext::new(Arc::new(AsyncStd));

    println!("First connection ...");
    let cf = Crazyflie::connect_from_uri(
        async_executors::AsyncStd,
        &context,
        "radio://0/80/2M/E7E7E7E7E7",
    )
    .await?;

    cf.disconnect().await;

    println!(" --- Disconnected by calling disconnect(), waiting 3 seconds --- ");
    async_std::task::sleep(Duration::from_secs(3)).await;

    println!("Reconnecting ...");

    let cf = Crazyflie::connect_from_uri(
        async_executors::AsyncStd,
        &context,
        "radio://0/80/2M/E7E7E7E7E7",
    )
    .await;

    drop(cf);

    println!(" --- Disconnected by dropping cf, waiting 3 seconds --- ");
    async_std::task::sleep(Duration::from_secs(3)).await;

    println!("Reconnecting ...");

    let _cf = Crazyflie::connect_from_uri(
        async_executors::AsyncStd,
        &context,
        "radio://0/80/2M/E7E7E7E7E7",
    )
    .await;

    Ok(())
}
