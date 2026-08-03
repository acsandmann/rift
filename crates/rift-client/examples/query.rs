use std::error::Error;

use rift_client::RiftMachClient;

fn main() -> Result<(), Box<dyn Error>> {
    let client = RiftMachClient::connect()?;
    let data = client.get_workspaces(None)?;

    println!("{}", serde_json::to_string_pretty(&data)?);
    Ok(())
}
