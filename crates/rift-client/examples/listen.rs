use std::error::Error;

use rift_client::RiftMachClient;

fn main() -> Result<(), Box<dyn Error>> {
    // Pass a specific event on the command line, or listen to every event by default.
    let event = std::env::args().nth(1).unwrap_or_else(|| "*".to_owned());
    let client = RiftMachClient::connect()?;
    let subscription = client.subscribe(&event)?;

    eprintln!("Listening for Rift event '{event}'. Press Ctrl-C to stop.");
    loop {
        let payload = subscription.recv_event()?;
        println!("{}", serde_json::to_string_pretty(&payload)?);
    }
}
