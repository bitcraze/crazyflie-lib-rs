use std::sync::Arc;

use async_executors::AsyncStd;

const URI: &str = "radio://0/60/2M/E7E7E7E7E7";

// Example that prints a list of the param variables
#[async_std::main]
async fn main() {
    let link_context = crazyflie_link::LinkContext::new(Arc::new(AsyncStd));
    let cf = crazyflie_lib::Crazyflie::connect_from_uri(&link_context, URI).await;

    println!("{: <30} | {: <6} | {: <6}", "Name", "Access", "Value");
    println!("{0:-<30}-|-{0:-<6}-|-{0:-<6}", "");

    for name in cf.param.names() {
        let value: crazyflie_lib::Value = cf.param.get(&name).await.unwrap();
        let writable = if cf.param.is_writable(&name).unwrap() {
            "RW"
        } else {
            "RO"
        };

        println!("{: <30} | {: ^6} | {:?}", name, writable, value);
    }
}