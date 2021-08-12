use async_executors::AsyncStd;
use crazyflie_lib::Crazyflie;
use crazyflie_link::LinkContext;
use std::sync::Arc;

#[async_std::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let context = LinkContext::new(Arc::new(AsyncStd));
    let crazyflie =
        Crazyflie::connect_from_uri(AsyncStd, &context, "radio://0/60/2M/E7E7E7E7E7").await?;

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

    Ok(())
}
