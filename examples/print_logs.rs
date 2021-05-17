use async_executors::AsyncStd;
use crazyflie_lib::Crazyflie;
use crazyflie_link::LinkContext;
use std::sync::Arc;

#[async_std::main]
async fn main() {
    let context = LinkContext::new(Arc::new(AsyncStd));
    let crazyflie = Crazyflie::connect_from_uri(&context, "radio://0/60/2M/E7E7E7E7E7").await;
    dbg!(&crazyflie);
    crazyflie
        .param
        .set("pid_attitude.yaw_kd", 42f32)
        .await
        .unwrap();
}
