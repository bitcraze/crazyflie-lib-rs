// Test that the Crazyflie object can be sent between threads

use std::thread::spawn;
use crazyflie_lib::Crazyflie;

#[async_std::test]
async fn crazyflie_can_be_sent_to_thread() -> Result<(), Box<dyn std::error::Error>> {
    let link_context = crazyflie_link::LinkContext::new(async_executors::AsyncStd);

    // Scan for Crazyflies on the default address
    let found = link_context.scan([0xE7; 5]).await?;

    if let Some(uri) = found.first() {
        // Connect to the first Crazyflie found
        let cf = crazyflie_lib::Crazyflie::connect_from_uri(
            async_executors::AsyncStd,
            &link_context,
            uri,
        )
        .await?;

        let _ = spawn(move || {
            cf
        }).join().unwrap();
    }
    Ok(())
}