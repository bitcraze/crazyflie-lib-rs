use tokio::time::{Duration, timeout};
use futures::StreamExt;

// Example scans for Crazyflies, connect the first one and print the console message line by line.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let link_context = crazyflie_link::LinkContext::new();

    // Scan for Crazyflies on the default address
    let found = link_context.scan([0xE7; 5]).await?;

    if let Some(uri) = found.last() {
        println!("Connecting to {} ...", uri);

        let cf = crazyflie_lib::Crazyflie::connect_from_uri(
            &link_context,
            uri,
            crazyflie_lib::NoTocCache
        )
        .await?;

        let mut console_stream = cf.console.line_stream().await;

        while let Ok(Some(line)) = timeout(Duration::from_secs(10), console_stream.next()).await {
            println!("{}", line);
        }

        cf.disconnect().await;
    } else {
        println!("No Crazyflie found, exiting!");
    }

    Ok(())
}
