use std::error::Error;
use std::io;

use rift_client::{RiftMachClient, RiftRequest, RiftResponse};

fn main() -> Result<(), Box<dyn Error>> {
    let client = RiftMachClient::connect()?;
    let response = client.send_request(&RiftRequest::GetWorkspaces { space_id: None })?;

    let data = match response {
        RiftResponse::Success { data } => data,
        RiftResponse::Error { error } => {
            return Err(io::Error::other(format!("Rift returned an error: {error}")).into());
        }
        _ => return Err(io::Error::other("Rift returned an unknown response").into()),
    };

    println!("{}", serde_json::to_string_pretty(&data)?);
    Ok(())
}
