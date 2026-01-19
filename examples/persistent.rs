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

    println!("Found {} persistent parameters:\n", persistent_params.len());
    for name in &persistent_params {
        println!("  {}", name);
    }

    Ok(())
}
