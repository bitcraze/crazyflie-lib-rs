use crazyflie_lib::Crazyflie;
use crazyflie_link::LinkContext;
use futures::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let uri = std::env::args()
        .skip_while(|a| a != "--uri")
        .nth(1)
        .unwrap_or_else(|| "radio://0/60/2M/E7E7E7E7E7".to_string());

    let context = LinkContext::new();
    let crazyflie = Crazyflie::connect_from_uri(
        &context,
        &uri,
        crazyflie_lib::NoTocCache
    )
    .await?;

    // Launch a task to watch param changes
    let mut param_watcher = crazyflie.param.watch_change().await?;
    tokio::spawn(async move {
        while let Some((name, value)) = param_watcher.next().await {
            println!(
                " > Param watcher: '{}' updated with value {:?}",
                name, value
            );
        }
    });

    let param_names = crazyflie.param.names();

    println!("{} params variables: ", param_names.len());

    for name in param_names {
        let value: crazyflie_lib::Value = crazyflie.param.get(&name).await?;
        let writable = if crazyflie.param.is_writable(&name)? {
            "RW"
        } else {
            "RO"
        };
        println!("{}\t{}\t{:?}", name, writable, value);
    }

    let val: f32 = crazyflie.param.get("pid_attitude.yaw_kd").await?;
    println!("Param value: {}", val);
    crazyflie.param.set("pid_attitude.yaw_kd", 42f32).await?;

    let val = crazyflie.param.get_lossy("pid_attitude.yaw_kd").await?;
    println!("Param value: {}", val);
    crazyflie
        .param
        .set_lossy("pid_attitude.yaw_kd", 84.0)
        .await?;
    let val = crazyflie.param.get_lossy("pid_attitude.yaw_kd").await?;
    println!("Param value: {}", val);

    println!("Disconnecting...");
    crazyflie.disconnect().await;
    println!("Done.");

    println!("Trying to watch parameter changes...");
    let param_watcher_2 = crazyflie.param.watch_change().await;
    match param_watcher_2 {
        Ok(_) => unreachable!("watch_change() should have returned an error when no Crazyflie is connected."),
        Err(msg) => println!("Watching changes is not possible when no Crazyflie is connected. This is correct! Error message: {msg}")
    }

    Ok(())
}
