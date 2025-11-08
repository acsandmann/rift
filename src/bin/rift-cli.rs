use std::process::{self};

use clap::{Parser, Subcommand};
use rift_wm::actor::reactor;
use rift_wm::ipc::{RiftCommand, RiftMachClient, RiftRequest, RiftResponse};
use rift_wm::layout_engine as layout;
use rift_wm::model::server::{ApplicationData, LayoutStateData, WindowData, WorkspaceData};
use serde_json::Value;

#[derive(Parser)]
#[command(name = "rift-cli")]
#[command(about = "Command-line interface for rift window manager")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Query information from rift
    Query {
        #[command(subcommand)]
        query: QueryCommands,
    },
    /// Execute commands in rift
    Execute {
        #[command(subcommand)]
        command: ExecuteCommands,
    },
    /// Event subscription commands
    Subscribe {
        #[command(subcommand)]
        subscribe: SubscribeCommands,
    },
    /// Manage the launchd service for rift
    Service {
        #[command(subcommand)]
        service: ServiceCommands,
    },
}

#[derive(Subcommand)]
enum ServiceCommands {
    /// Install the per-user launchd service
    Install,
    /// Uninstall the per-user launchd service
    Uninstall,
    /// Start (or bootstrap) the service
    Start,
    /// Stop (or bootout/kill) the service
    Stop,
    /// Restart the service (kickstart -k)
    Restart,
}

#[derive(Subcommand)]
enum QueryCommands {
    /// List all virtual workspaces
    Workspaces,
    /// List windows (optionally filtered by space)
    Windows {
        #[arg(long)]
        space_id: Option<u64>,
    },
    /// List connected displays
    Displays,
    /// Get information about a specific window
    Window { window_id: String },
    /// List running applications
    Applications,
    /// Get layout state for a space
    Layout { space_id: u64 },
    /// Get performance metrics
    Metrics,
}

#[derive(Subcommand)]
enum ExecuteCommands {
    /// Window management commands
    Window {
        #[command(subcommand)]
        window_cmd: WindowCommands,
    },
    /// Virtual workspace commands
    Workspace {
        #[command(subcommand)]
        workspace_cmd: WorkspaceCommands,
    },
    /// Layout commands
    Layout {
        #[command(subcommand)]
        layout_cmd: LayoutCommands,
    },
    /// Configuration management commands
    Config {
        #[command(subcommand)]
        config_cmd: ConfigCommands,
    },
    /// Mission control commands
    MissionControl {
        #[command(subcommand)]
        mission_cmd: MissionControlCommands,
    },
    /// Save current state and exit rift
    SaveAndExit,
    /// Show timing metrics
    ShowTiming,
}

#[derive(Subcommand)]
enum WindowCommands {
    /// Focus the next window
    Next,
    /// Focus the previous window
    Prev,
    /// Move focus in a direction
    Focus {
        direction: String, // up, down, left, right
    },
    /// Toggle window floating state
    ToggleFloat,
    /// Toggle fullscreen mode (fills the whole screen, ignores outer gaps)
    ToggleFullscreen,
    /// Toggle fullscreen within configured outer gaps (respects outer gaps / fills tiling area)
    ToggleFullscreenWithinGaps,
    /// Grow the current window size (increments by ~5%).
    ResizeGrow,
    /// Shrink the current window size (decrements by ~5%).
    ResizeShrink,
    /// Resize the selected window by a fractional amount.
    /// - Pass a signed floating value: positive to grow, negative to shrink.
    /// - The value is a fraction of the current size (e.g. `0.05` = 5%).
    /// Examples:
    ///   rift-cli execute window resize-by --amount 0.05    # grow by 5%
    ///   rift-cli execute window resize-by --amount -0.10   # shrink by 10%
    ResizeBy { amount: f64 },
}

#[derive(Subcommand)]
enum WorkspaceCommands {
    /// Switch to next workspace
    Next { skip_empty: Option<bool> },
    /// Switch to previous workspace
    Prev { skip_empty: Option<bool> },
    /// Switch to specific workspace
    Switch { workspace_id: usize },
    /// Move current window to workspace
    MoveWindow {
        workspace_id: usize,
        window_id: Option<u32>,
    },
    /// Create a new workspace
    Create,
    /// Switch to the last workspace
    Last,
}

#[derive(Subcommand)]
enum LayoutCommands {
    /// Move selection up the tree
    Ascend,
    /// Move selection down the tree
    Descend,
    /// Move the selected node in a direction
    MoveNode { direction: String },
    /// Join the selected window with neighbor in a direction
    JoinWindow { direction: String },
    /// Toggle stacked state for the selected container
    ToggleStack,
    /// Global orientation toggle that works consistently across layout modes (and between splits/stacks)
    ToggleOrientation,
    /// Unjoin previously joined windows
    Unjoin,
    /// Toggle floating on the focused selection (tree focus)
    ToggleFocusFloat,
}

#[derive(Subcommand)]
enum ConfigCommands {
    /// Update animation settings
    SetAnimate {
        value: String,
    },
    SetAnimationDuration {
        value: f64,
    },
    SetAnimationFps {
        value: f64,
    },
    SetAnimationEasing {
        value: String,
    },

    /// Update mouse settings
    SetMouseFollowsFocus {
        value: bool,
    },
    SetMouseHidesOnFocus {
        value: bool,
    },
    SetFocusFollowsMouse {
        value: bool,
    },

    /// Update layout settings
    SetStackOffset {
        value: f64,
    },
    /// Set the default stack orientation behavior. Value should be one of:
    /// "perpendicular", "same", "horizontal", or "vertical"
    SetStackDefaultOrientation {
        value: String,
    },
    SetOuterGaps {
        top: f64,
        left: f64,
        bottom: f64,
        right: f64,
    },
    SetInnerGaps {
        horizontal: f64,
        vertical: f64,
    },

    /// Update workspace settings
    SetWorkspaceNames {
        names: Vec<String>,
    },

    /// Generic set: set an arbitrary config key (dot-separated path) to a JSON value.
    /// Example: rift-cli execute config set --key settings.animate --value true
    Set {
        /// Dot-separated key path (e.g. settings.animate or settings.layout.gaps.outer.top)
        key: String,
        /// Value should be valid JSON (true, 1, "string", {"a":1}), but if it's not valid JSON
        /// it will be treated as a string.
        value: String,
    },

    /// Get current config
    Get,

    /// Save current config to file
    Save,

    /// Reload config from file
    Reload,
}

#[derive(Subcommand)]
enum MissionControlCommands {
    /// Show all workspaces in mission control
    ShowAll,
    /// Show current workspace in mission control
    ShowCurrent,
    /// Dismiss mission control
    Dismiss,
}

#[derive(Subcommand)]
enum SubscribeCommands {
    /// Subscribe to Mach IPC events
    Mach {
        /// Event to subscribe to (workspace_changed, windows_changed, *)
        event: String,
    },
    /// Subscribe to events via CLI command execution
    Cli {
        /// Event to subscribe to (workspace_changed, windows_changed, *)
        #[arg(long)]
        event: String,
        /// Command to execute when event occurs
        #[arg(long)]
        command: String,
        /// Arguments to pass to command (event data will be appended as JSON)
        #[arg(long, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Unsubscribe from Mach IPC events
    UnsubMach {
        /// Event to unsubscribe from
        event: String,
    },
    /// Unsubscribe from CLI events
    UnsubCli {
        /// Event to unsubscribe from
        event: String,
    },
    /// List current CLI subscriptions
    ListCli,
}

fn main() {
    sigpipe::reset();
    let cli = Cli::parse();

    if let Commands::Service { .. } = &cli.command {
        println!(
            "service commands have been moved to the `rift` binary. (ie `rift service install`)"
        );
        process::exit(0);
    }

    let request = match build_request(cli.command) {
        Ok(req) => req,
        Err(e) => {
            eprintln!("Error: {}", e);
            process::exit(1);
        }
    };

    let client = match RiftMachClient::connect() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to connect to rift: {}", e);
            process::exit(1);
        }
    };

    // Send request and handle response.
    match client.send_request(&request) {
        Ok(resp) => match resp {
            RiftResponse::Success { data } => {
                if let Err(e) = handle_success_response(&request, data) {
                    eprintln!("Failed to handle response: {}", e);
                    process::exit(1);
                }
            }
            RiftResponse::Error { error } => {
                match serde_json::to_string_pretty(&error) {
                    Ok(pretty) => eprintln!("{}", pretty),
                    Err(_) => eprintln!("Error: {}", error),
                }
                process::exit(1);
            }
            _ => {
                eprintln!("Received an unknown response shape from rift");
                process::exit(1);
            }
        },
        Err(e) => {
            eprintln!("Communication error: {}", e);
            eprintln!("Hint: ensure the rift service is running (try `rift service start`).");
            process::exit(1);
        }
    }
}

fn build_request(command: Commands) -> Result<RiftRequest, String> {
    match command {
        Commands::Query { query } => build_query_request(query),
        Commands::Execute { command } => build_execute_request(command),
        Commands::Subscribe { subscribe } => build_subscribe_request(subscribe),
        Commands::Service { .. } => Err(
            "Service commands are handled locally and should not be sent to the rift server."
                .to_string(),
        ),
    }
}

fn build_query_request(query: QueryCommands) -> Result<RiftRequest, String> {
    match query {
        QueryCommands::Workspaces => Ok(RiftRequest::GetWorkspaces),
        QueryCommands::Windows { space_id } => Ok(RiftRequest::GetWindows { space_id }),
        QueryCommands::Displays => Ok(RiftRequest::GetDisplays),
        QueryCommands::Window { window_id } => Ok(RiftRequest::GetWindowInfo { window_id }),
        QueryCommands::Applications => Ok(RiftRequest::GetApplications),
        QueryCommands::Layout { space_id } => Ok(RiftRequest::GetLayoutState { space_id }),
        QueryCommands::Metrics => Ok(RiftRequest::GetMetrics),
    }
}

fn build_subscribe_request(sub: SubscribeCommands) -> Result<RiftRequest, String> {
    match sub {
        SubscribeCommands::Mach { event } => Ok(RiftRequest::Subscribe { event }),
        SubscribeCommands::Cli { event, command, args } => {
            Ok(RiftRequest::SubscribeCli { event, command, args })
        }
        SubscribeCommands::UnsubMach { event } => Ok(RiftRequest::Unsubscribe { event }),
        SubscribeCommands::UnsubCli { event } => Ok(RiftRequest::UnsubscribeCli { event }),
        SubscribeCommands::ListCli => Ok(RiftRequest::ListCliSubscriptions),
    }
}

fn build_execute_request(execute: ExecuteCommands) -> Result<RiftRequest, String> {
    let rift_command = match execute {
        ExecuteCommands::Window { window_cmd } => map_window_command(window_cmd)?,
        ExecuteCommands::Workspace { workspace_cmd } => map_workspace_command(workspace_cmd)?,
        ExecuteCommands::Layout { layout_cmd } => map_layout_command(layout_cmd)?,
        ExecuteCommands::Config { config_cmd } => map_config_command(config_cmd)?,
        ExecuteCommands::MissionControl { mission_cmd } => {
            map_mission_control_command(mission_cmd)?
        }
        ExecuteCommands::SaveAndExit => {
            RiftCommand::Reactor(reactor::Command::Reactor(reactor::ReactorCommand::SaveAndExit))
        }
        ExecuteCommands::ShowTiming => RiftCommand::Reactor(reactor::Command::Metrics(
            rift_wm::common::log::MetricsCommand::ShowTiming,
        )),
    };

    if let RiftCommand::Config(rift_wm::common::config::ConfigCommand::GetConfig) = &rift_command {
        return Ok(RiftRequest::GetConfig);
    }

    let maybe_config_json = match &rift_command {
        RiftCommand::Config(cfg_cmd) => match serde_json::to_string(cfg_cmd) {
            Ok(s) => Some(s),
            Err(_) => None,
        },
        _ => None,
    };

    let command_str = serde_json::to_string(&rift_command)
        .map_err(|e| format!("Failed to serialize command: {}", e))?;

    if let Some(cfg_json) = maybe_config_json {
        Ok(RiftRequest::ExecuteCommand {
            command: command_str,
            args: vec!["__apply_config__".to_string(), cfg_json],
        })
    } else {
        Ok(RiftRequest::ExecuteCommand {
            command: command_str,
            args: vec![],
        })
    }
}

fn map_window_command(cmd: WindowCommands) -> Result<RiftCommand, String> {
    use layout::LayoutCommand as LC;
    match cmd {
        WindowCommands::Next => Ok(RiftCommand::Reactor(reactor::Command::Layout(LC::NextWindow))),
        WindowCommands::Prev => Ok(RiftCommand::Reactor(reactor::Command::Layout(LC::PrevWindow))),
        WindowCommands::Focus { direction } => Ok(RiftCommand::Reactor(reactor::Command::Layout(
            LC::MoveFocus(direction.into()),
        ))),
        WindowCommands::ToggleFloat => Ok(RiftCommand::Reactor(reactor::Command::Layout(
            LC::ToggleWindowFloating,
        ))),
        WindowCommands::ToggleFullscreen => Ok(RiftCommand::Reactor(reactor::Command::Layout(
            LC::ToggleFullscreen,
        ))),
        WindowCommands::ToggleFullscreenWithinGaps => Ok(RiftCommand::Reactor(
            reactor::Command::Layout(LC::ToggleFullscreenWithinGaps),
        )),
        WindowCommands::ResizeGrow => Ok(RiftCommand::Reactor(reactor::Command::Layout(
            LC::ResizeWindowGrow,
        ))),
        WindowCommands::ResizeShrink => Ok(RiftCommand::Reactor(reactor::Command::Layout(
            LC::ResizeWindowShrink,
        ))),
        WindowCommands::ResizeBy { amount } => Ok(RiftCommand::Reactor(reactor::Command::Layout(
            LC::ResizeWindowBy { amount },
        ))),
    }
}

fn map_workspace_command(cmd: WorkspaceCommands) -> Result<RiftCommand, String> {
    use layout::LayoutCommand as LC;
    match cmd {
        WorkspaceCommands::Next { skip_empty } => Ok(RiftCommand::Reactor(
            reactor::Command::Layout(LC::NextWorkspace(skip_empty)),
        )),
        WorkspaceCommands::Prev { skip_empty } => Ok(RiftCommand::Reactor(
            reactor::Command::Layout(LC::PrevWorkspace(skip_empty)),
        )),
        WorkspaceCommands::Switch { workspace_id } => Ok(RiftCommand::Reactor(
            reactor::Command::Layout(LC::SwitchToWorkspace(workspace_id)),
        )),
        WorkspaceCommands::MoveWindow { workspace_id, window_id } => Ok(RiftCommand::Reactor(
            reactor::Command::Layout(LC::MoveWindowToWorkspace {
                workspace: workspace_id,
                window_id,
            }),
        )),
        WorkspaceCommands::Create => Ok(RiftCommand::Reactor(reactor::Command::Layout(
            LC::CreateWorkspace,
        ))),
        WorkspaceCommands::Last => Ok(RiftCommand::Reactor(reactor::Command::Layout(
            LC::SwitchToLastWorkspace,
        ))),
    }
}

fn map_layout_command(cmd: LayoutCommands) -> Result<RiftCommand, String> {
    use layout::LayoutCommand as LC;
    match cmd {
        LayoutCommands::Ascend => Ok(RiftCommand::Reactor(reactor::Command::Layout(LC::Ascend))),
        LayoutCommands::Descend => Ok(RiftCommand::Reactor(reactor::Command::Layout(LC::Descend))),
        LayoutCommands::MoveNode { direction } => Ok(RiftCommand::Reactor(
            reactor::Command::Layout(LC::MoveNode(direction.into())),
        )),
        LayoutCommands::JoinWindow { direction } => Ok(RiftCommand::Reactor(
            reactor::Command::Layout(LC::JoinWindow(direction.into())),
        )),
        LayoutCommands::ToggleStack => {
            Ok(RiftCommand::Reactor(reactor::Command::Layout(LC::ToggleStack)))
        }
        LayoutCommands::ToggleOrientation => Ok(RiftCommand::Reactor(reactor::Command::Layout(
            LC::ToggleOrientation,
        ))),
        LayoutCommands::Unjoin => {
            Ok(RiftCommand::Reactor(reactor::Command::Layout(LC::UnjoinWindows)))
        }
        LayoutCommands::ToggleFocusFloat => Ok(RiftCommand::Reactor(reactor::Command::Layout(
            LC::ToggleFocusFloating,
        ))),
    }
}

fn map_config_command(cmd: ConfigCommands) -> Result<RiftCommand, String> {
    use rift_wm::common::config::{AnimationEasing, ConfigCommand};

    let cfg_cmd = match cmd {
        ConfigCommands::SetAnimate { value } => {
            let bool_value = match value.to_lowercase().as_str() {
                "true" | "on" => true,
                "false" | "off" => false,
                _ => return Err(format!("Invalid boolean value: {}. Use true/false", value)),
            };
            ConfigCommand::SetAnimate(bool_value)
        }
        ConfigCommands::SetAnimationDuration { value } => {
            ConfigCommand::SetAnimationDuration(value)
        }
        ConfigCommands::SetAnimationFps { value } => ConfigCommand::SetAnimationFps(value),
        ConfigCommands::SetAnimationEasing { value } => {
            let easing = match value.as_str() {
                "ease_in_out" => AnimationEasing::EaseInOut,
                "linear" => AnimationEasing::Linear,
                "ease_in_sine" => AnimationEasing::EaseInSine,
                "ease_out_sine" => AnimationEasing::EaseOutSine,
                "ease_in_out_sine" => AnimationEasing::EaseInOutSine,
                "ease_in_quad" => AnimationEasing::EaseInQuad,
                "ease_out_quad" => AnimationEasing::EaseOutQuad,
                "ease_in_out_quad" => AnimationEasing::EaseInOutQuad,
                "ease_in_cubic" => AnimationEasing::EaseInCubic,
                "ease_out_cubic" => AnimationEasing::EaseOutCubic,
                "ease_in_out_cubic" => AnimationEasing::EaseInOutCubic,
                "ease_in_quart" => AnimationEasing::EaseInQuart,
                "ease_out_quart" => AnimationEasing::EaseOutQuart,
                "ease_in_out_quart" => AnimationEasing::EaseInOutQuart,
                "ease_in_quint" => AnimationEasing::EaseInQuint,
                "ease_out_quint" => AnimationEasing::EaseOutQuint,
                "ease_in_out_quint" => AnimationEasing::EaseInOutQuint,
                "ease_in_expo" => AnimationEasing::EaseInExpo,
                "ease_out_expo" => AnimationEasing::EaseOutExpo,
                "ease_in_out_expo" => AnimationEasing::EaseInOutExpo,
                "ease_in_circ" => AnimationEasing::EaseInCirc,
                "ease_out_circ" => AnimationEasing::EaseOutCirc,
                "ease_in_out_circ" => AnimationEasing::EaseInOutCirc,
                _ => return Err(format!("Invalid animation easing: {}", value)),
            };
            ConfigCommand::SetAnimationEasing(easing)
        }
        ConfigCommands::SetMouseFollowsFocus { value } => {
            ConfigCommand::SetMouseFollowsFocus(value)
        }
        ConfigCommands::SetMouseHidesOnFocus { value } => {
            ConfigCommand::SetMouseHidesOnFocus(value)
        }
        ConfigCommands::SetFocusFollowsMouse { value } => {
            ConfigCommand::SetFocusFollowsMouse(value)
        }
        ConfigCommands::SetStackOffset { value } => ConfigCommand::SetStackOffset(value),
        ConfigCommands::SetStackDefaultOrientation { value } => {
            let parsed_value: serde_json::Value = serde_json::Value::String(value.clone());
            ConfigCommand::Set {
                key: "settings.layout.stack.default_orientation".to_string(),
                value: parsed_value,
            }
        }
        ConfigCommands::SetOuterGaps { top, left, bottom, right } => {
            ConfigCommand::SetOuterGaps { top, left, bottom, right }
        }
        ConfigCommands::SetInnerGaps { horizontal, vertical } => {
            ConfigCommand::SetInnerGaps { horizontal, vertical }
        }
        ConfigCommands::SetWorkspaceNames { names } => ConfigCommand::SetWorkspaceNames(names),
        ConfigCommands::Set { key, value } => {
            let parsed_value: Value = match serde_json::from_str(&value) {
                Ok(v) => v,
                Err(_) => Value::String(value.clone()),
            };
            ConfigCommand::Set { key, value: parsed_value }
        }
        ConfigCommands::Get => ConfigCommand::GetConfig,
        ConfigCommands::Save => ConfigCommand::SaveConfig,
        ConfigCommands::Reload => ConfigCommand::ReloadConfig,
    };

    Ok(RiftCommand::Config(cfg_cmd))
}

fn map_mission_control_command(cmd: MissionControlCommands) -> Result<RiftCommand, String> {
    match cmd {
        MissionControlCommands::ShowAll => Ok(RiftCommand::Reactor(reactor::Command::Reactor(
            reactor::ReactorCommand::ShowMissionControlAll,
        ))),
        MissionControlCommands::ShowCurrent => Ok(RiftCommand::Reactor(reactor::Command::Reactor(
            reactor::ReactorCommand::ShowMissionControlCurrent,
        ))),
        MissionControlCommands::Dismiss => Ok(RiftCommand::Reactor(reactor::Command::Reactor(
            reactor::ReactorCommand::DismissMissionControl,
        ))),
    }
}

fn handle_success_response(request: &RiftRequest, data: serde_json::Value) -> Result<(), String> {
    match request {
        RiftRequest::GetWorkspaces => {
            let typed: Vec<WorkspaceData> = serde_json::from_value(data)
                .map_err(|e| format!("Deserialization error: {}", e))?;
            println!(
                "{}",
                serde_json::to_string_pretty(&typed).map_err(|e| e.to_string())?
            );
        }
        RiftRequest::GetWindows { .. } => {
            let typed: Vec<WindowData> = serde_json::from_value(data)
                .map_err(|e| format!("Deserialization error: {}", e))?;
            println!(
                "{}",
                serde_json::to_string_pretty(&typed).map_err(|e| e.to_string())?
            );
        }
        RiftRequest::GetWindowInfo { .. } => {
            let typed: WindowData = serde_json::from_value(data)
                .map_err(|e| format!("Deserialization error: {}", e))?;
            println!(
                "{}",
                serde_json::to_string_pretty(&typed).map_err(|e| e.to_string())?
            );
        }
        RiftRequest::GetApplications => {
            let typed: Vec<ApplicationData> = serde_json::from_value(data)
                .map_err(|e| format!("Deserialization error: {}", e))?;
            println!(
                "{}",
                serde_json::to_string_pretty(&typed).map_err(|e| e.to_string())?
            );
        }
        RiftRequest::GetLayoutState { .. } => {
            let typed: LayoutStateData = serde_json::from_value(data)
                .map_err(|e| format!("Deserialization error: {}", e))?;
            println!(
                "{}",
                serde_json::to_string_pretty(&typed).map_err(|e| e.to_string())?
            );
        }
        _ => {
            println!(
                "{}",
                serde_json::to_string_pretty(&data).map_err(|e| e.to_string())?
            );
        }
    }
    Ok(())
}
