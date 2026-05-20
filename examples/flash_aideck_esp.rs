use std::{env, fs, process, time::Duration};

use crazyflie_lib::subsystems::memory::{DeckMemory, MemoryType};
use tokio::time::{Instant, sleep};

const BOOTLOADER_READY_TIMEOUT: Duration = Duration::from_secs(5);
const BOOTLOADER_POLL_INTERVAL: Duration = Duration::from_millis(100);

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let uri = args.next().unwrap_or_else(|| {
        eprintln!("usage: flash_aideck_esp <radio-uri> <firmware.bin> [section-name]");
        eprintln!("  default section-name: bcAI:esp");
        process::exit(1);
    });
    let path = args.next().unwrap_or_else(|| {
        eprintln!("usage: flash_aideck_esp <radio-uri> <firmware.bin> [section-name]");
        process::exit(1);
    });
    let section_name = args.next().unwrap_or_else(|| "bcAI:esp".to_owned());

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

    let available: Vec<&str> = deck_memory
        .sections()
        .iter()
        .map(|s| s.name())
        .collect();
    println!("Available deck memory sections: {:?}", available);

    let section = deck_memory.section(&section_name).ok_or_else(|| {
        format!(
            "section '{}' not found; available sections: {:?}",
            section_name, available
        )
    })?;

    if !section.supports_upgrade() {
        return Err(format!(
            "section '{}' does not support firmware upgrade",
            section_name
        )
        .into());
    }

    if !section.can_reset_to_bootloader() {
        return Err(format!(
            "section '{}' cannot reset to bootloader",
            section_name
        )
        .into());
    }

    if !section.bootloader_active().await? {
        println!("Resetting deck to bootloader mode...");
        section.reset_to_bootloader().await?;

        let deadline = Instant::now() + BOOTLOADER_READY_TIMEOUT;
        loop {
            if section.bootloader_active().await? {
                break;
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "section '{}' did not enter bootloader within {:?}",
                    section_name, BOOTLOADER_READY_TIMEOUT
                )
                .into());
            }
            sleep(BOOTLOADER_POLL_INTERVAL).await;
        }
    } else {
        println!("Deck already in bootloader mode.");
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
