# rift-client

`rift-client` is the Rust client for Rift's JSON-over-Mach IPC API. It is meant
for macOS plugins and companion applications that need to query Rift, execute a
command, or subscribe to Rift events without depending on the window manager.

## Query Rift

```rust
use rift_client::RiftMachClient;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = RiftMachClient::connect()?;
    let workspaces = client.get_workspaces(None)?;
    println!("{workspaces:#?}");
    Ok(())
}
```

Run the complete [query example](examples/query.rs) from the Rift repository:

```sh
cargo run -p rift-client --example query
```

## Listen for events

`subscribe` returns a subscription handle whose `recv_event` method blocks until
Rift publishes the next matching, typed `RiftEvent`:

```rust
use rift_client::{EventKind, RiftMachClient};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = RiftMachClient::connect()?;
    let subscription = client.subscribe(EventKind::WorkspaceChanged)?;

    loop {
        let event = subscription.recv_event()?;
        println!("{}", serde_json::to_string_pretty(&event)?);
    }
}
```

Run the complete [event listener example](examples/listen.rs). It listens for
all events by default, or for the event name passed as its first argument:

```sh
cargo run -p rift-client --example listen
cargo run -p rift-client --example listen -- workspace_changed
```

Supported event names are `workspace_changed`, `windows_changed`,
`window_title_changed`, `focused_window_changed`, `stacks_changed`, and
`layout_changed`. Use `*` to listen for all events.

For a more complete example, see the [dimmer example](examples/dimmer.rs),
which dims unfocused windows and updates them as Rift events arrive:

```sh
cargo run -p rift-client --example dimmer
```

Set `RIFT_BS_NAME` to use a non-default Rift bootstrap service (for example,
when running multiple instances).
