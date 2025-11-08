const URI: &str = "radio://0/60/2M/E7E7E7E7E7";

// Example that prints a list of the param variables
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let link_context = crazyflie_link::LinkContext::new();
    let cf =
        crazyflie_lib::Crazyflie::connect_from_uri(&link_context, URI, crazyflie_lib::NoTocCache)
            .await?;

    println!("{: <30} | {: <6} | {: <6}", "Name", "Access", "Value");
    println!("{0:-<30}-|-{0:-<6}-|-{0:-<6}", "");

    for name in cf.param.names() {
        let value: crazyflie_lib::Value = cf.param.get(&name).await?;
        let writable = if cf.param.is_writable(&name)? {
            "RW"
        } else {
            "RO"
        };

        println!("{: <30} | {: ^6} | {:?}", name, writable, value);
    }

    Ok(())
}
