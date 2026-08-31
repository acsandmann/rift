//! What `rift status` reports.
//!
//! Three things can independently be wrong: launchd may not be running the
//! agent, the agent may be running but not answering, and the scripting
//! addition inside Dock may be absent, stale or degraded. Each is probed on its
//! own so the output says which one to fix.

use serde::Serialize;
use serde_json::Value;

use crate::ipc::RiftMachClient;
use crate::sys::osax::{SaCommands, handle_sa_command};
use crate::sys::service;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Health {
    /// Working.
    Ok,
    /// Present but not doing its whole job.
    Degraded,
    /// Absent.
    Down,
}

impl Health {
    fn label(self) -> &'static str {
        match self {
            Health::Ok => "ok",
            Health::Degraded => "degraded",
            Health::Down => "down",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub name: &'static str,
    pub health: Health,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub checks: Vec<Check>,
}

impl Report {
    /// Whether the window manager itself is up.
    ///
    /// This is what the exit status reports. A missing scripting addition is
    /// deliberately not a failure: rift runs without it, with fewer features.
    pub fn window_manager_is_up(&self) -> bool {
        self.checks
            .iter()
            .any(|check| check.name == WINDOW_MANAGER && check.health == Health::Ok)
    }

    pub fn render(&self) -> String {
        let width = self.checks.iter().map(|check| check.name.len()).max().unwrap_or(0);
        self.checks
            .iter()
            .map(|check| {
                format!(
                    "{:width$}  {:8}  {}\n",
                    check.name,
                    check.health.label(),
                    check.detail,
                    width = width
                )
            })
            .collect()
    }
}

const WINDOW_MANAGER: &str = "window manager";
const LAUNCHD_SERVICE: &str = "launchd service";
const SCRIPTING_ADDITION: &str = "scripting addition";

/// Probes every component and returns what each one said.
pub fn report() -> Report {
    Report {
        checks: vec![window_manager(), launchd_service(), scripting_addition()],
    }
}

/// Asks the running rift a real question rather than only looking it up.
///
/// A registered Mach service proves a process claimed the name, not that its
/// reactor is still answering, so this round-trips a metrics query and reports
/// what came back.
fn window_manager() -> Check {
    let client = match RiftMachClient::connect() {
        Ok(client) => client,
        Err(error) => {
            return Check {
                name: WINDOW_MANAGER,
                health: Health::Down,
                detail: format!("cannot create a client: {error}"),
            };
        }
    };

    if !client.is_available() {
        return Check {
            name: WINDOW_MANAGER,
            health: Health::Down,
            detail: "not running (its Mach service is not registered)".to_string(),
        };
    }

    match client.get_metrics() {
        Ok(metrics) => Check {
            name: WINDOW_MANAGER,
            health: Health::Ok,
            detail: summarize_metrics(&metrics),
        },
        Err(error) => Check {
            name: WINDOW_MANAGER,
            health: Health::Degraded,
            detail: format!("registered but not answering: {error}"),
        },
    }
}

fn summarize_metrics(metrics: &Value) -> String {
    let count = |key: &str| metrics.get(key).and_then(Value::as_u64);
    match (count("windows_managed"), count("workspaces"), count("screens")) {
        (Some(windows), Some(workspaces), Some(screens)) => {
            format!("running — {windows} windows, {workspaces} workspaces, {screens} screens")
        }
        _ => "running".to_string(),
    }
}

fn launchd_service() -> Check {
    let state = match service::service_state() {
        Ok(state) => state,
        Err(error) => {
            return Check {
                name: LAUNCHD_SERVICE,
                health: Health::Down,
                detail: format!("could not be checked: {error}"),
            };
        }
    };

    match (state.running.as_deref(), state.own_plist_installed) {
        // A Homebrew install runs under Homebrew's label, which `rift service`
        // does not manage. Naming the label is the whole point: it tells you
        // which one `rift service restart` would actually act on.
        (Some(label), _) => Check {
            name: LAUNCHD_SERVICE,
            health: Health::Ok,
            detail: format!("running as {label}"),
        },
        (None, true) => Check {
            name: LAUNCHD_SERVICE,
            health: Health::Degraded,
            detail: "installed but not running (`rift service start`)".to_string(),
        },
        // Running rift by hand is a normal thing to do, so this is not a
        // failure -- only a statement that nothing will restart it.
        (None, false) => Check {
            name: LAUNCHD_SERVICE,
            health: Health::Down,
            detail: "no launchd job; rift is not being kept alive (`rift service install`)"
                .to_string(),
        },
    }
}

fn scripting_addition() -> Check {
    match handle_sa_command(&SaCommands::Status) {
        Ok(detail) => Check {
            name: SCRIPTING_ADDITION,
            health: Health::Ok,
            detail: trim_subject(&detail),
        },
        // Everything rift needs the addition for degrades to a fallback, so a
        // missing or stale payload is never fatal.
        Err(detail) => Check {
            name: SCRIPTING_ADDITION,
            health: Health::Degraded,
            detail: trim_subject(&detail),
        },
    }
}

/// `rift sa status` writes whole sentences because it is its own command. Here
/// the row already says which component is speaking.
fn trim_subject(detail: &str) -> String {
    let trimmed = detail.strip_prefix("scripting addition ").unwrap_or(detail);
    trimmed.strip_prefix("is ").unwrap_or(trimmed).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(name: &'static str, health: Health) -> Check {
        Check {
            name,
            health,
            detail: "detail".to_string(),
        }
    }

    #[test]
    fn exit_status_follows_the_window_manager_alone() {
        let degraded_addition = Report {
            checks: vec![
                check(WINDOW_MANAGER, Health::Ok),
                check(SCRIPTING_ADDITION, Health::Degraded),
            ],
        };
        assert!(degraded_addition.window_manager_is_up());

        let unanswering = Report {
            checks: vec![
                check(WINDOW_MANAGER, Health::Degraded),
                check(SCRIPTING_ADDITION, Health::Ok),
            ],
        };
        assert!(!unanswering.window_manager_is_up());
    }

    #[test]
    fn render_aligns_on_the_longest_name() {
        let rendered = Report {
            checks: vec![
                check(WINDOW_MANAGER, Health::Ok),
                check(SCRIPTING_ADDITION, Health::Down),
            ],
        }
        .render();

        let columns: Vec<usize> =
            rendered.lines().map(|line| line.find("  ").expect("a gap")).collect();
        assert_eq!(columns, vec![WINDOW_MANAGER.len(), SCRIPTING_ADDITION.len()]);
    }

    #[test]
    fn trimming_leaves_the_part_the_row_label_does_not_already_say() {
        assert_eq!(
            trim_subject("scripting addition is loaded and healthy (payload v1.0.0)"),
            "loaded and healthy (payload v1.0.0)"
        );
        assert_eq!(
            trim_subject("scripting addition is NOT loaded (nothing answered)"),
            "NOT loaded (nothing answered)"
        );
        assert_eq!(
            trim_subject("scripting addition v1.0.0 is loaded but could not find dock.spaces"),
            "v1.0.0 is loaded but could not find dock.spaces"
        );
        assert_eq!(
            trim_subject("something else entirely"),
            "something else entirely"
        );
    }

    #[test]
    fn metrics_summary_falls_back_when_fields_are_missing() {
        assert_eq!(
            summarize_metrics(&serde_json::json!({
                "windows_managed": 10,
                "workspaces": 7,
                "screens": 2
            })),
            "running — 10 windows, 7 workspaces, 2 screens"
        );
        assert_eq!(
            summarize_metrics(&serde_json::json!({ "windows_managed": 10 })),
            "running"
        );
    }
}
