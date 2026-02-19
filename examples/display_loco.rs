use crazyflie_lib::subsystems::memory::{LocoMemory2, MemoryType};

const URI: &str = "radio://0/80/2M/E7E7E7E7E7";

// Example that reads and displays Loco Positioning System anchor data
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let link_context = crazyflie_link::LinkContext::new();
    let cf = crazyflie_lib::Crazyflie::connect_from_uri(&link_context, URI, crazyflie_lib::NoTocCache).await?;

    let memories = cf.memory.get_memories(Some(MemoryType::Loco2));

    if memories.is_empty() {
        println!("No Loco2 memory found. Is the LPS deck attached?");
    } else {
        match cf
            .memory
            .open_memory::<LocoMemory2>(memories[0].clone())
            .await
        {
            Some(Ok(loco)) => {
                let data = loco.read_all().await?;

                println!("Loco Positioning System - Anchor Data:");
                println!("  {:>3}  {:>6}  {:>5}  {}", "ID", "Active", "Valid", "Position (x, y, z)");

                for &id in &data.anchor_ids {
                    let is_active = data.active_anchor_ids.contains(&id);
                    if let Some(anchor) = data.anchors.get(&id) {
                        println!(
                            "  {:>3}  {:>6}  {:>5}  ({:.3}, {:.3}, {:.3})",
                            id,
                            if is_active { "yes" } else { "no" },
                            if anchor.is_valid { "yes" } else { "no" },
                            anchor.position[0],
                            anchor.position[1],
                            anchor.position[2],
                        );
                    }
                }

                cf.memory.close_memory(loco).await?;
            }
            Some(Err(e)) => {
                println!("Could not access Loco2 memory: {}", e);
            }
            None => {
                println!("Loco2 memory not found");
            }
        };
    }

    cf.disconnect().await;

    Ok(())
}
