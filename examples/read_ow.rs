use crazyflie_lib::subsystems::memory::{MemoryType, RawMemory};

const URI: &str = "radio://0/80/2M/E7E7E7E7E7";

// Example that prints the raw content of the 1-wire
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let link_context = crazyflie_link::LinkContext::new();
    let cf = crazyflie_lib::Crazyflie::connect_from_uri(&link_context, URI).await?;

    let memories = cf.memory.get_memories(Some(MemoryType::OneWire));

    if memories.is_empty() {
        println!("No OneWire memory found, exiting!");
    } else {
        for mem in memories {
            match cf
                .memory
                .open_memory::<RawMemory>(mem.clone())
                .await
            {
                Some(Ok(m)) => {
                    let data = m.read(0, 112).await?;

                    for (i, byte) in data.iter().enumerate() {
                        if i % 16 == 0 {
                            print!("\n{:08x}: ", i);
                        }
                        print!("{:02x} ", byte);
                    }
                    println!();
                }
                Some(Err(e)) => {
                    println!(
                        "Could not access memory ID={} as raw memory: {}",
                        mem.memory_id, e
                    );
                }
                None => {
                    println!("Memory ID={} not found", mem.memory_id);
                }
            }
        }
    }

    Ok(())
}
