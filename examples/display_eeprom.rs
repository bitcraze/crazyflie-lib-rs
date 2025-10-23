use crazyflie_lib::subsystems::memory::{EEPROMConfigMemory, MemoryType};

const URI: &str = "radio://0/80/2M/E7E7E7E7E7";

// Example that displays the EEPROM config memory contents
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let link_context = crazyflie_link::LinkContext::new();
    let cf = crazyflie_lib::Crazyflie::connect_from_uri(&link_context, URI).await?;

    let memories = cf.memory.get_memories(Some(MemoryType::EEPROMConfig));

    if memories.len() != 1 {
        println!(
            "No EEPROMConfig memory found or more than one ({}), exiting!",
            memories.len()
        );
    } else {
        match cf
            .memory
            .open_memory::<EEPROMConfigMemory>(memories[0].clone())
            .await
        {
            Some(Ok(eeprom)) => {
                println!("EEPROM Config:");
                println!("  Radio Channel: {}", eeprom.get_radio_channel());
                println!("  Radio Speed: {}", eeprom.get_radio_speed());
                println!("  Pitch Trim: {:.4}", eeprom.get_pitch_trim());
                println!("  Roll Trim: {:.4}", eeprom.get_roll_trim());
                println!("  Radio Address: {:02X?}", eeprom.get_radio_address());
            }
            Some(Err(e)) => {
                println!(
                    "Could not access memory ID={} as EEPROMConfig: {}",
                    memories[0].memory_id, e
                );
            }
            None => {
                println!("Memory ID={} not found", memories[0].memory_id);
            }
        };
    }

    Ok(())
}
