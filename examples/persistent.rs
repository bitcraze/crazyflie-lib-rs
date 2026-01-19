/// Persistent parameters example
use crazyflie_lib::{Crazyflie, NoTocCache};
use crazyflie_link::LinkContext;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let uri = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "radio://0/60/2M/E7E7E7E7E7".to_string());

    println!("Connecting to {} ...", uri);
    let context = LinkContext::new();
    let cf = Crazyflie::connect_from_uri(&context, &uri, NoTocCache).await?;
    println!("Connected!\n");

    // Step 1: List all persistent parameters
    println!("=== Persistent Parameters ===");
    let mut persistent_params = Vec::new();
    
    for name in cf.param.names() {
        if cf.param.is_persistent(&name).await? {
            persistent_params.push(name);
        }
    }

    println!("Found {} persistent parameters\n", persistent_params.len());

    // Step 2: Get default values
    // Note: This is redundant with Step 3 (persistent_get_state also returns defaults),
    // but demonstrates the get_default_value() method independently.
    println!("=== Default Values ===\n");
    
    let test_params = vec!["ring.effect", "activeMarker.back", "pm.lowVoltage"];
    
    for name in &test_params {
        match cf.param.get_default_value(name).await {
            Ok(value) => {
                println!("{}: {:?}", name, value);
            }
            Err(e) => {
                println!("{}: ERROR - {:?}", name, e);
            }
        }
    }

    // Step 3: Get persistent state
    println!("\n=== Persistent Parameter States ===\n");
    
    for name in &test_params {
        match cf.param.persistent_get_state(name).await {
            Ok(state) => {
                println!("{}:", name);
                println!("  Default value: {:?}", state.default_value);
                if state.is_stored {
                    println!("  Stored value:  {:?} ✓", state.stored_value.unwrap());
                } else {
                    println!("  Stored: No (using default)");
                }
                println!();
            }
            Err(e) => {
                println!("{}: ERROR - {:?}\n", name, e);
            }
        }
    }

    // Step 4: Store a value to EEPROM
    println!("=== Storing a Parameter ===\n");
    
    let test_param = "ring.effect";
    
    // Get current value
    let current: u8 = cf.param.get(test_param).await?;
    println!("Current value of {}: {}", test_param, current);
    
    // Set a new value
    let new_value = 10u8;
    println!("Setting {} to {}", test_param, new_value);
    cf.param.set(test_param, new_value).await?;
    
    // Store it to EEPROM
    println!("Storing to EEPROM...");
    cf.param.persistent_store(test_param).await?;
    println!("✓ Stored successfully!\n");
    
    // Verify it's now marked as stored
    match cf.param.persistent_get_state(test_param).await {
        Ok(state) => {
            println!("Verification:");
            println!("  Default value: {:?}", state.default_value);
            if state.is_stored {
                println!("  Stored value:  {:?} ✓", state.stored_value.unwrap());
            } else {
                println!("  Stored: No (using default)");
            }
        }
        Err(e) => {
            println!("Verification failed: {:?}", e);
        }
    }

    Ok(())
}
