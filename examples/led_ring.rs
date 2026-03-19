use crazyflie_lib::subsystems::memory::{LedDriverMemory, MemoryType};

const URI: &str = "radio://0/80/2M/E7E7E7E7E7";

// Example that cycles through colors on the Crazyflie LED ring
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let link_context = crazyflie_link::LinkContext::new();
    let cf = crazyflie_lib::Crazyflie::connect_from_uri(&link_context, URI, crazyflie_lib::NoTocCache).await?;

    // Switch to the virtual memory effect so the LED ring reads from LED driver memory
    cf.param.set("ring.effect", 13u8).await?;

    let memories = cf.memory.get_memories(Some(MemoryType::DriverLed));

    if memories.is_empty() {
        println!("No LED driver memory found. Is the LED ring deck attached?");
    } else {
        match cf
            .memory
            .open_memory::<LedDriverMemory>(memories[0].clone())
            .await
        {
            Some(Ok(mut leds)) => {
                // Red
                println!("Setting all LEDs to red...");
                for led in &mut leds.leds {
                    led.set(255, 0, 0, None);
                }
                leds.write_leds().await?;
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;

                // Green
                println!("Setting all LEDs to green...");
                for led in &mut leds.leds {
                    led.set(0, 255, 0, None);
                }
                leds.write_leds().await?;
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;

                // Blue
                println!("Setting all LEDs to blue...");
                for led in &mut leds.leds {
                    led.set(0, 0, 255, None);
                }
                leds.write_leds().await?;
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;

                // Off
                println!("Turning off all LEDs...");
                for led in &mut leds.leds {
                    led.set(0, 0, 0, None);
                }
                leds.write_leds().await?;

                cf.memory.close_memory(leds).await?;
            }
            Some(Err(e)) => {
                println!("Could not access LED driver memory: {}", e);
            }
            None => {
                println!("LED driver memory not found");
            }
        };
    }

    cf.disconnect().await;

    Ok(())
}
