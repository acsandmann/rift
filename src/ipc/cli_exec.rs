use std::convert::Infallible;
use std::ffi::CString;

use nix::unistd::{ForkResult, execvp, fork};
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

        match unsafe { fork() } {
            Ok(ForkResult::Parent { .. }) => {
                debug!("Parent process forked child for CLI subscription");
                return;
            }
            Ok(ForkResult::Child) => {
                debug!("Child executing CLI subscription command: {}", shell_cmd);

                for (k, v) in env_vars {
                    unsafe {
                        std::env::set_var(k, v);
                    }
                }

                let c_shell = CString::new("sh").unwrap();
                let c_flag = CString::new("-c").unwrap();
                let c_cmd = CString::new(shell_cmd).unwrap();
                let exec_args: &[&CString] = &[&c_shell, &c_flag, &c_cmd];

                let result: Result<Infallible, _> = execvp(&c_shell, exec_args);

                error!("execvp failed: {}", result.unwrap_err());

                std::process::exit(1);
            }
            Err(e) => {
                error!("Failed to fork for CLI subscription: {}", e);
            }
        }
    }
}

pub fn execute_cli_subscription(event: &BroadcastEvent, subscription: &CliSubscription) {
    let exec = DefaultCliExecutor::new();
    exec.execute(event, subscription);
}
