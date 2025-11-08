use crazyflie_lib::Crazyflie;
use crazyflie_link::LinkContext;
use std::sync::Arc;
use tokio::time::{sleep, Duration};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let context = LinkContext::new();

    println!("First connection ...");
    let cf = Crazyflie::connect_from_uri(
        &context,
        "radio://0/20/2M/E7E7E7E7E7",
        crazyflie_lib::NoTocCache
    )
    .await?;
    let cf = Arc::new(cf);

    let cf_task = cf.clone();
    tokio::spawn(async move {
        let reason = cf_task.wait_disconnect().await;
        println!(
            "Disconnect event detected by parallel task. Disconnect reason: \"{}\"",
            reason
        );
    });

    cf.disconnect().await;

    println!(" --- Disconnected by calling disconnect(), waiting 3 seconds --- ");
    sleep(Duration::from_secs(3)).await;

    println!("Reconnecting ...");

    let cf = Crazyflie::connect_from_uri(
        &context,
        "radio://0/60/2M/E7E7E7E7E7",
        crazyflie_lib::NoTocCache
    )
    .await;

    drop(cf);

    println!(" --- Disconnected by dropping cf, waiting 3 seconds --- ");
    sleep(Duration::from_secs(3)).await;

    println!("Reconnecting ...");

    let _cf = Crazyflie::connect_from_uri(
        &context,
        "radio://0/60/2M/E7E7E7E7E7",
        crazyflie_lib::NoTocCache
    )
    .await;

    Ok(())
}
