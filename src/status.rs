//! What `rift status` reports.
//!
//! Launchd may not be running the agent, or the agent may be running but not
//! answering. Each is probed independently so the output says which one to fix.

use serde::Serialize;
use serde_json::Value;

use crate::ipc::RiftMachClient;
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
    /// This is what the exit status reports.
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

/// Probes every component and returns what each one said.
pub fn report() -> Report {
    Report {
        checks: vec![window_manager(), launchd_service()],
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
            detail: summarize_metrics(&metrics, env!("CARGO_PKG_VERSION")),
        },
        Err(error) => Check {
            name: WINDOW_MANAGER,
            health: Health::Degraded,
            detail: format!("registered but not answering: {error}"),
        },
    }
}

/// Names the version that is running, and says so when it is not the one
/// asking: after an upgrade the new binary answers `rift status` while the old
/// one keeps running until the service restarts, and nothing else tells them
/// apart.
fn summarize_metrics(metrics: &Value, own_version: &str) -> String {
    let count = |key: &str| metrics.get(key).and_then(Value::as_u64);
    let mut summary = match metrics.get("version").and_then(Value::as_str) {
        Some(version) => format!("running {version}"),
        None => "running".to_string(),
    };
    if let (Some(windows), Some(workspaces), Some(screens)) =
        (count("windows_managed"), count("workspaces"), count("screens"))
    {
        summary.push_str(&format!(
            " — {windows} windows, {workspaces} workspaces, {screens} screens"
        ));
    }
    // A rift from before the version was reported is necessarily older than
    // this binary, so its silence is a mismatch too.
    let running = metrics.get("version").and_then(Value::as_str);
    if running != Some(own_version) {
        summary.push_str(&format!("; restart rift to run {own_version}"));
    }
    summary
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
        let running_without_launchd = Report {
            checks: vec![check(WINDOW_MANAGER, Health::Ok), check(LAUNCHD_SERVICE, Health::Down)],
        };
        assert!(running_without_launchd.window_manager_is_up());

        let unanswering = Report {
            checks: vec![
                check(WINDOW_MANAGER, Health::Degraded),
                check(LAUNCHD_SERVICE, Health::Ok),
            ],
        };
        assert!(!unanswering.window_manager_is_up());
    }

    #[test]
    fn render_aligns_on_the_longest_name() {
        let rendered = Report {
            checks: vec![
                check(WINDOW_MANAGER, Health::Ok),
                check(LAUNCHD_SERVICE, Health::Down),
            ],
        }
        .render();

        let columns: Vec<usize> =
            rendered.lines().map(|line| line.find("  ").expect("a gap")).collect();
        assert_eq!(columns, vec![WINDOW_MANAGER.len(), WINDOW_MANAGER.len()]);
    }

    #[test]
    fn metrics_summary_falls_back_when_fields_are_missing() {
        assert_eq!(
            summarize_metrics(
                &serde_json::json!({
                    "version": "1.0.0",
                    "windows_managed": 10,
                    "workspaces": 7,
                    "screens": 2
                }),
                "1.0.0"
            ),
            "running 1.0.0 — 10 windows, 7 workspaces, 2 screens"
        );
        assert_eq!(
            summarize_metrics(
                &serde_json::json!({ "version": "1.0.0", "windows_managed": 10 }),
                "1.0.0"
            ),
            "running 1.0.0"
        );
    }

    #[test]
    fn metrics_summary_names_the_binary_when_the_running_rift_is_another() {
        assert_eq!(
            summarize_metrics(&serde_json::json!({ "version": "1.0.0" }), "1.1.0"),
            "running 1.0.0; restart rift to run 1.1.0"
        );
        // A rift too old to report a version at all.
        assert_eq!(
            summarize_metrics(
                &serde_json::json!({ "windows_managed": 10, "workspaces": 7, "screens": 2 }),
                "1.1.0"
            ),
            "running — 10 windows, 7 workspaces, 2 screens; restart rift to run 1.1.0"
        );
    }
}
