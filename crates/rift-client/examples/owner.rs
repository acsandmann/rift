//! Claim one tracked window until this process exits.
//!
//! cargo run -p rift-client --example owner -- --window-id 12312312
//! cargo run -p rift-client --example owner -- --window-id 1234:1

use std::error::Error;
use std::sync::mpsc;

use rift_client::{RiftMachClient, WindowId};

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some("--window-id") {
        return Err("usage: owner --window-id <window-server-id|pid:idx>".into());
    }
    let raw_id = args.next().ok_or("missing window ID")?;
    if args.next().is_some() {
        return Err("unexpected extra argument".into());
    }

    let client = RiftMachClient::connect()?;
    let window_id = if let Some((pid, idx)) = raw_id.split_once(':') {
        WindowId::new(pid.parse()?, idx.parse()?).ok_or("window idx must be nonzero")?
    } else {
        let server_id: u32 = raw_id.parse()?;
        client
            .get_windows(None)?
            .into_iter()
            .find(|window| window.window_server_id == Some(server_id))
            .ok_or("WindowServer ID not found in the active workspace; use pid:idx instead")?
            .id
    };

    let mut session = client.window_management_session()?;
    session.claim_window(window_id)?;
    println!(
        "Claimed window {}:{}. Press Ctrl-C to release it.",
        window_id.pid, window_id.idx
    );

    let (stop_tx, stop_rx) = mpsc::channel();
    ctrlc::set_handler(move || {
        let _ = stop_tx.send(());
    })?;
    stop_rx.recv()?;
    // Dropping the receive right lets Rift release the claim on Mach dead-name notification.
    drop(session);
    release_result?;
    Ok(())
}
