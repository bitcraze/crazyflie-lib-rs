use std::{env, fs, process};

use crazyflie_lib::subsystems::memory::{DeckMemory, MemoryType};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let uri = args.next().unwrap_or_else(|| {
        eprintln!("usage: flash_aideck_esp <radio-uri> <firmware.bin> [section-name]");
        eprintln!("  default section-name: esp");
        process::exit(1);
    });
    let path = args.next().unwrap_or_else(|| {
        eprintln!("usage: flash_aideck_esp <radio-uri> <firmware.bin> [section-name]");
        process::exit(1);
    });
    let section_name = args.next().unwrap_or_else(|| "esp".to_owned());

    let firmware = fs::read(&path)?;
    let size = firmware.len() as u32;
    println!("Loaded {} bytes from {}", size, path);

    let link_context = crazyflie_link::LinkContext::new();
    let cf = crazyflie_lib::Crazyflie::connect_from_uri(
        &link_context,
        &uri,
        crazyflie_lib::NoTocCache,
    )
    .await?;

    let memories = cf.memory.get_memories(Some(MemoryType::DeckMemory));
    let device = memories
        .into_iter()
        .next()
        .ok_or("no DeckMemory found on this Crazyflie")?
        .clone();

    let deck_memory = cf
        .memory
        .open_memory::<DeckMemory>(device)
        .await
        .ok_or("DeckMemory could not be opened")??;

    let section = deck_memory
        .section(&section_name)
        .ok_or_else(|| format!("section '{}' not found", section_name))?;

    if !section.supports_upgrade() {
        return Err(format!(
            "section '{}' does not support firmware upgrade",
            section_name
        )
        .into());
    }

    println!("Flashing {} bytes to section '{}'", size, section.name());

    section
        .flash_firmware_with_progress(&firmware, |done, total| {
            print!("\r  {}/{} bytes ({}%)", done, total, done * 100 / total);
            use std::io::Write;
            let _ = std::io::stdout().flush();
        })
        .await?;
    println!();

    println!("Flash complete. Resetting deck to firmware mode...");
    section.reset_to_firmware().await?;
    println!("Done.");

    Ok(())
}
