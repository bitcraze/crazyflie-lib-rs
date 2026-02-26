// Example: Connect to a Crazyflie and print link statistics every second.
//
// Shows ping latency, link quality, packet rates, and radio metrics.

use tokio::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let link_context = crazyflie_link::LinkContext::new();

    let uri = "radio://0/80/2M/E7E7E7E7E7";
    println!("Connecting to {} ...", uri);

    let cf = crazyflie_lib::Crazyflie::connect_from_uri(
        &link_context,
        &uri,
        crazyflie_lib::NoTocCache,
    )
    .await?;

    println!("Connected!\n");

    // Print statistics every second for 10 seconds
    for i in 1..=10 {
        // Measure latency with an explicit ping
        let ping_result = cf.link_service.ping().await;

        let stats = cf.link_service.get_statistics().await;

        println!("--- Sample {} ---", i);

        match ping_result {
            Ok(rtt) => println!("  Ping RTT:        {:.1} ms", rtt),
            Err(e) => println!("  Ping RTT:        failed ({})", e),
        }

        match stats.link_quality {
            Some(quality) => println!("  Link quality:    {:.1}%", quality * 100.0),
            None => println!("  Link quality:    N/A (USB connection)"),
        }

        match stats.uplink_rate {
            Some(rate) => println!("  Uplink rate:     {:.0} pkt/s (data)", rate),
            None => println!("  Uplink rate:     N/A"),
        }

        match stats.downlink_rate {
            Some(rate) => println!("  Downlink rate:   {:.0} pkt/s", rate),
            None => println!("  Downlink rate:   N/A"),
        }

        match stats.radio_send_rate {
            Some(rate) => println!("  Radio send rate: {:.0} pkt/s (data + null)", rate),
            None => println!("  Radio send rate: N/A"),
        }

        match stats.avg_retries {
            Some(retries) => println!("  Avg retries:     {:.2}", retries),
            None => println!("  Avg retries:     N/A"),
        }

        match stats.power_detector_rate {
            Some(rate) => println!("  Power detector:  {:.1}%", rate * 100.0),
            None => println!("  Power detector:  N/A"),
        }

        match stats.rssi {
            Some(rssi) => println!("  RSSI:            {:.0} dBm", rssi),
            None => println!("  RSSI:            N/A"),
        }

        println!();
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    // Bandwidth tests
    println!("--- Bandwidth tests (5 seconds each) ---\n");

    println!("Testing uplink (sink)...");
    match cf.link_service.test_uplink_bandwidth(Duration::from_secs(5)).await {
        Ok(bps) => println!("  Uplink:   {:.1} kB/s", bps / 1024.0),
        Err(e) => println!("  Uplink:   failed ({})", e),
    }

    println!("Testing downlink (source)...");
    match cf.link_service.test_downlink_bandwidth(Duration::from_secs(5)).await {
        Ok(bps) => println!("  Downlink: {:.1} kB/s", bps / 1024.0),
        Err(e) => println!("  Downlink: failed ({})", e),
    }

    println!("Testing echo (round-trip)...");
    match cf.link_service.test_echo_bandwidth(Duration::from_secs(5)).await {
        Ok(result) => {
            println!("  Uplink:   {:.1} kB/s", result.uplink_bytes_per_sec / 1024.0);
            println!("  Downlink: {:.1} kB/s", result.downlink_bytes_per_sec / 1024.0);
            println!("  Packets:  {:.0} pkt/s", result.packets_per_sec);
        }
        Err(e) => println!("  Echo:     failed ({})", e),
    }

    println!();
    cf.disconnect().await;

    Ok(())
}
