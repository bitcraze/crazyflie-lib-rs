use crazyflie_lib::subsystems::console::ConsoleHistory;
use futures::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args().skip(1);
    let uri = arguments
        .next()
        .unwrap_or_else(|| "radio://0/22/2M/E7E7E7E7E7".to_owned());
    let source_path = arguments.next().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "usage: sourced_console [URI] SOURCE_PATH",
        )
    })?;

    let link_context = crazyflie_lib::crazyflie_link::LinkContext::new();
    let crazyflie =
        crazyflie_lib::Crazyflie::connect_from_uri(&link_context, &uri, crazyflie_lib::NoTocCache)
            .await?;

    let catalog = crazyflie.console.catalog().await?;
    println!(
        "Discovered {} sourced consoles (CRC: {:?})",
        catalog.len(),
        catalog.crc32()
    );
    for source in &catalog {
        println!("  {}: {}", source.id().get(), source.path());
    }

    let source = catalog.find(&source_path).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("Console source {source_path:?} was not found"),
        )
    })?;
    let selector = source.selector();
    let mut lines = source.line_stream(ConsoleHistory::Replay).await;
    crazyflie.console.enable(selector).await?;

    println!("Showing {source_path}; press Ctrl-C to stop");
    loop {
        tokio::select! {
            line = lines.next() => match line {
                Some(line) => println!("{line}"),
                None => break,
            },
            _ = tokio::signal::ctrl_c() => break,
        }
    }

    crazyflie.console.disable(selector).await?;
    crazyflie.disconnect().await;
    Ok(())
}
