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

    println!("Available consoles:");
    for path in crazyflie.console.list().await? {
        println!("  {path}");
    }

    let mut console = crazyflie.console.open(&source_path).await?;

    println!("Showing {source_path}; press Ctrl-C to stop");
    let mut lines = console.lines();
    loop {
        tokio::select! {
            line = lines.next() => match line {
                Some(line) => println!("{line}"),
                None => break,
            },
            _ = tokio::signal::ctrl_c() => break,
        }
    }
    drop(lines);

    console.close().await?;
    crazyflie.disconnect().await;
    Ok(())
}
