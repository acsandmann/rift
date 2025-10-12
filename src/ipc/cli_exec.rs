use std::process::Command;

use tracing::{debug, error};

use crate::actor::broadcast::BroadcastEvent;
use crate::common::collections::HashMap;
use crate::ipc::subscriptions::CliSubscription;

pub trait CliExecutor: Send + Sync + 'static {
    fn execute(&self, event: &BroadcastEvent, subscription: &CliSubscription);
}

pub struct DefaultCliExecutor;

impl DefaultCliExecutor {
    pub fn new() -> Self { Self {} }
}

impl CliExecutor for DefaultCliExecutor {
    fn execute(&self, event: &BroadcastEvent, subscription: &CliSubscription) {
        let mut env_vars: HashMap<String, String> = HashMap::default();
        match event {
            BroadcastEvent::WorkspaceChanged {
                workspace_id,
                workspace_name,
                space_id,
            } => {
                env_vars.insert("RIFT_EVENT_TYPE".to_string(), "workspace_changed".to_string());
                env_vars.insert("RIFT_WORKSPACE_ID".to_string(), workspace_id.to_string());
                env_vars.insert("RIFT_WORKSPACE_NAME".to_string(), workspace_name.clone());
                env_vars.insert("RIFT_SPACE_ID".to_string(), space_id.to_string());
            }
            BroadcastEvent::WindowsChanged {
                workspace_id,
                workspace_name,
                windows,
            } => {
                env_vars.insert("RIFT_EVENT_TYPE".to_string(), "windows_changed".to_string());
                env_vars.insert("RIFT_WORKSPACE_ID".to_string(), workspace_id.to_string());
                env_vars.insert("RIFT_WORKSPACE_NAME".to_string(), workspace_name.clone());
                env_vars.insert("RIFT_WINDOW_COUNT".to_string(), windows.len().to_string());
                env_vars.insert("RIFT_WINDOWS".to_string(), windows.join(","));
            }
        }

        let event_json = match serde_json::to_string(event) {
            Ok(s) => s,
            Err(e) => {
                error!("Failed to serialize event for CLI executor: {}", e);
                return;
            }
        };
        env_vars.insert("RIFT_EVENT_JSON".to_string(), event_json.clone());

        let command = subscription.command.clone();
        let mut args = subscription.args.clone();
        args.push(event_json.clone());

        let mut shell_cmd = String::new();
        shell_cmd.push_str(&shell_escape::escape(command.into()));
        for arg in &args {
            shell_cmd.push(' ');
            shell_cmd.push_str(&shell_escape::escape(arg.into()));
        }

        std::thread::spawn(move || {
            debug!("Spawning CLI subscription command: {}", shell_cmd);
            let mut cmd = Command::new("sh");
            cmd.arg("-c").arg(shell_cmd).envs(env_vars);
            match cmd.spawn() {
                Ok(mut child) => {
                    if let Err(e) = child.wait() {
                        error!("Failed to wait for CLI subscription command: {}", e);
                    }
                }
                Err(e) => {
                    error!("Failed to spawn CLI subscription command: {}", e);
                }
            }
        });
    }
}

pub fn execute_cli_subscription(event: &BroadcastEvent, subscription: &CliSubscription) {
    let exec = DefaultCliExecutor::new();
    exec.execute(event, subscription);
}
