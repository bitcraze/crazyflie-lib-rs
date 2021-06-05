use async_executors::AsyncStd;
use crazyflie_lib::Crazyflie;
use crazyflie_link::LinkContext;
use std::sync::Arc;

#[async_std::main]
async fn main() {
    let context = LinkContext::new(Arc::new(AsyncStd));
    let cf = Crazyflie::connect_from_uri(&context, "radio://0/60/2M/E7E7E7E7E7").await;

    let block = cf.log.create_block().await.unwrap();
}