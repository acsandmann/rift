use serde::{Deserialize, Serialize};
use serde_json::Value;

#[non_exhaustive]
#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
pub enum RiftRequest {
    GetWorkspaces {
        space_id: Option<u64>,
    },
    GetDisplays,
    GetWindows {
        space_id: Option<u64>,
    },
    GetWindowInfo {
        window_id: String,
    },
    GetLayoutState {
        space_id: u64,
    },
    GetWorkspaceLayouts {
        space_id: Option<u64>,
        workspace_id: Option<usize>,
    },
    GetApplications,
    GetMetrics,
    GetConfig,
    ExecuteCommand {
        command: String,
        args: Vec<String>,
    },
    Subscribe {
        event: String,
    },
    Unsubscribe {
        event: String,
    },
    SubscribeCli {
        event: String,
        command: String,
        args: Vec<String>,
    },
    UnsubscribeCli {
        event: String,
    },
    ListCliSubscriptions,
}

#[non_exhaustive]
#[derive(Serialize, Deserialize, Debug)]
#[serde(untagged)]
pub enum RiftResponse {
    Success { data: Value },
    Error { error: Value },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum RiftCommand {
    Reactor(crate::actor::reactor::Command),
    Config(crate::common::config::ConfigCommand),
}
