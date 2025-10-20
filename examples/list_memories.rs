const URI: &str = "radio://0/80/2M/E7E7E7E7E7";

// Example that prints a list of the memories
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let link_context = crazyflie_link::LinkContext::new();
    let cf = crazyflie_lib::Crazyflie::connect_from_uri(&link_context, URI).await?;

    println!("{: <2} | {: <20} | {: <6}", "ID", "Name", "Size");
    println!("{0:-<2}-|-{0:-<20}-|-{0:-<6}-", "");

    let memory = cf.memory.get_memories(None);

    for mem in memory {
      println!("{: <2} | {: <20} | {: <6}", mem.memory_id, format!("{:.20}", mem.memory_type), mem.size);
    }

    Ok(())
}
