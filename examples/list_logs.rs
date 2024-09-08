const URI: &str = "radio://0/60/2M/E7E7E7E7E7";

// Example that prints a list of the log variables
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let link_context = crazyflie_link::LinkContext::new();
    let cf =
        crazyflie_lib::Crazyflie::connect_from_uri(&link_context, URI)
            .await?;

    println!("{0: <30} | {1: <5}", "Name", "Type");
    println!("{:-<30}-|-{:-<5}", "", "");

    for name in cf.log.names() {
        let var_type = cf.log.get_type(&name)?;

        println!("{0: <30} | {1: <5?}", name, var_type);
    }

    Ok(())
}
