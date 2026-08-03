pub use rift_client::{RiftRequest, RiftResponse};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum RiftCommand {
    Reactor(crate::actor::reactor::Command),
    Config(crate::common::config::ConfigCommand),
}
