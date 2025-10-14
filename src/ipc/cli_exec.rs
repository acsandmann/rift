use std::collections::HashMap as StdHashMap;
use std::ffi::CString;
use std::os::unix::ffi::OsStringExt;
use std::ptr;

use nix::libc::{
    c_char, pid_t, posix_spawnattr_destroy, posix_spawnattr_init, posix_spawnattr_t, posix_spawnp,
};
use nix::sys::wait::waitpid;
use nix::unistd::{ForkResult, fork, setsid};
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
                env_vars.insert("RIFT_EVENT_TYPE".into(), "workspace_changed".into());
                env_vars.insert("RIFT_WORKSPACE_ID".into(), workspace_id.to_string());
                env_vars.insert("RIFT_WORKSPACE_NAME".into(), workspace_name.clone());
                env_vars.insert("RIFT_SPACE_ID".into(), space_id.to_string());
            }
            BroadcastEvent::WindowsChanged {
                workspace_id,
                workspace_name,
                windows,
            } => {
                env_vars.insert("RIFT_EVENT_TYPE".into(), "windows_changed".into());
                env_vars.insert("RIFT_WORKSPACE_ID".into(), workspace_id.to_string());
                env_vars.insert("RIFT_WORKSPACE_NAME".into(), workspace_name.clone());
                env_vars.insert("RIFT_WINDOW_COUNT".into(), windows.len().to_string());
                env_vars.insert("RIFT_WINDOWS".into(), windows.join(","));
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

        let c_sh = CString::new("/bin/sh").unwrap();
        let c_dash_c = CString::new("-c").unwrap();
        let c_cmd = CString::new(shell_cmd.clone()).unwrap();

        let mut argv_storage = vec![c_sh.clone(), c_dash_c.clone(), c_cmd.clone()];
        let mut argv: Vec<*mut c_char> =
            argv_storage.iter_mut().map(|s| s.as_ptr() as *mut c_char).collect();
        argv.push(ptr::null_mut());

        let mut merged: StdHashMap<Vec<u8>, Vec<u8>> = StdHashMap::new();
        for (k, v) in std::env::vars_os() {
            merged.insert(k.into_vec(), v.into_vec());
        }
        for (k, v) in env_vars {
            merged.insert(k.into_bytes(), v.into_bytes());
        }

        let mut env_storage: Vec<CString> = Vec::with_capacity(merged.len());
        for (k, v) in merged {
            let mut kv = k;
            kv.push(b'=');
            kv.extend_from_slice(&v);
            env_storage.push(CString::new(kv).unwrap());
        }
        let mut envp: Vec<*mut c_char> =
            env_storage.iter_mut().map(|s| s.as_ptr() as *mut c_char).collect();
        envp.push(ptr::null_mut());

        match unsafe { fork() } {
            Ok(ForkResult::Parent { child, .. }) => {
                debug!(
                    "Parent forked child1 (pid {}) for CLI subscription; waiting to reap",
                    child
                );
                if let Err(e) = waitpid(child, None) {
                    error!("Failed to waitpid for child1: {}", e);
                }
                return;
            }
            Ok(ForkResult::Child) => {
                if let Err(e) = setsid() {
                    error!("child1: failed to setsid: {}", e);
                    std::process::exit(1);
                }

                let mut attr: posix_spawnattr_t = unsafe { std::mem::zeroed() };
                let rc_init = unsafe { posix_spawnattr_init(&mut attr) };
                if rc_init != 0 {
                    error!(
                        "posix_spawnattr_init failed: {}",
                        std::io::Error::from_raw_os_error(rc_init)
                    );
                    std::process::exit(1);
                }

                let mut child2_pid: pid_t = 0;
                let rc = unsafe {
                    posix_spawnp(
                        &mut child2_pid as *mut pid_t,
                        c_sh.as_ptr(),
                        ptr::null(),
                        &attr as *const _,
                        argv.as_mut_ptr(),
                        envp.as_mut_ptr(),
                    )
                };
                let _ = unsafe { posix_spawnattr_destroy(&mut attr) };

                if rc != 0 {
                    error!(
                        "posix_spawnp('/bin/sh', ...) failed: {}",
                        std::io::Error::from_raw_os_error(rc)
                    );
                    std::process::exit(1);
                }

                std::process::exit(0);
            }
            Err(e) => error!("Failed to fork for CLI subscription: {}", e),
        }
    }
}

pub fn execute_cli_subscription(event: &BroadcastEvent, subscription: &CliSubscription) {
    let exec = DefaultCliExecutor::new();
    exec.execute(event, subscription);
}
