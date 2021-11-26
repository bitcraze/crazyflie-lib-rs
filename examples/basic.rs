// Example scans for Crazyflies, connect the first one and print the log and param variables TOC.
#[async_std::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let link_context = crazyflie_link::LinkContext::new(async_executors::AsyncStd);

    // Scann for Crazyflies on the default address
    let found = link_context.scan([0xE7; 5]).await?;

    if let Some(uri) = found.first() {
        println!("Connecting to {} ...", uri);

        let cf = crazyflie_lib::Crazyflie::connect_from_uri(
            async_executors::AsyncStd,
            &link_context,
            uri,
        )
        .await?;

        println!("Connected!");

        let firmware_version = cf.platform.firmware_version().await?;
        let protocol_version = cf.platform.protocol_version().await?;
        println!("Firmware version:     {} (protocol {})", firmware_version, protocol_version);

        let device_type = cf.platform.device_type_name().await?;
        println!("Device type:          {}", device_type);

        println!("Number of params var: {}", cf.param.names().len());
        println!("Number of log var:    {}", cf.log.names().len());

        cf.disconnect().await;
    } else {
        println!("No Crazyflie found, exiting!");
    }

    Ok(())
}
