use std::convert::TryInto;

use crazyflie_lib::Crazyflie;
use futures::{SinkExt, StreamExt};

// Example of how to use the appchannel API
// The appchannel is a bidirectional communication channel intended to be used
// to communicate between a program on the ground and an app in the Crazyflie
//
// This allows to start communicating without implementing custom CRTP packet
//
// This example is communicating with the Crazyflie's App channel test example:
// https://github.com/bitcraze/crazyflie-firmware/tree/master/examples/app_appchannel_test
//
// The example sends 3 floats, the Crazyflie sums the 3 float and return the sum
// as one float.
//
// Like most of the other examples, it scans for Crazyflies and connect the fist one

#[async_std::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let link_context = crazyflie_link::LinkContext::new(async_executors::AsyncStd);

    // Scan for Crazyflies on the default address
    println!("Scanning for Crazyflie.");
    let found = link_context.scan([0xE7; 5]).await?;

    if let Some(uri) = found.first() {
        println!("Connecting to {}", &uri);
        let cf = Crazyflie::connect_from_uri(async_executors::AsyncStd, &link_context, uri).await?;

        let (mut tx, mut rx) = cf.platform.get_app_channel().await.unwrap();

        // 0+0 = 0
        let _ = tx.send([0; 12].into()).await;
        assert_eq!(rx.next().await, Some([0; 4].into()));
        println!("0+0+0 = 0");

        // 1+2+3 = 6
        let (a, b, c) = (1.0f32, 2.0f32, 3.0f32);
        let mut pk = Vec::new();
        pk.append(&mut a.to_le_bytes().to_vec());
        pk.append(&mut b.to_le_bytes().to_vec());
        pk.append(&mut c.to_le_bytes().to_vec());
        let _ = tx.send(pk.try_into().unwrap()).await;
        assert_eq!(rx.next().await, Some((a + b + c).to_le_bytes().into()));
        println!("{}+{}+{} = {}", a, b, c, a + b + c);
    } else {
        println!("Could not find any Crazyflie to connect!");
    }

    Ok(())
}
