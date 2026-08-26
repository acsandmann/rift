use std::io::{self, Write};
use std::path::PathBuf;
use std::process::{self};

use clap::{Args, Parser, Subcommand, ValueEnum};
use rift_protocol::{EventKind, RiftRequest, RiftResponse};
use rift_wm::actor::app::WindowId as InternalWindowId;
use rift_wm::actor::reactor::{self, DisplaySelector};
use rift_wm::common::config::{LayoutMode, WorkspaceSelector};
use rift_wm::ipc::RiftMachClient;
use rift_wm::layout_engine as layout;
use rift_wm::sys::window_server::WindowServerId;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

#[derive(Parser)]
#[command(name = "rift-cli")]
#[command(about = "Command-line interface for rift window manager")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

enum CliCommand {
    Reactor(reactor::Command),
    Config(rift_wm::common::config::ConfigCommand),
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
    /// List virtual workspaces (optionally for a specific MacOS space)
    Workspaces {
        #[arg(long)]
        space_id: Option<u64>,
    },
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
    /// Get layout state and normalized container tree for a space
    Layout {
        /// macOS space ID; defaults to the active display space
        #[arg(long)]
        space_id: Option<u64>,
        /// Virtual workspace index; defaults to the active workspace
        #[arg(long)]
        workspace_id: Option<usize>,
    },
    /// Get workspace layout-engine mode(s)
    WorkspaceLayout {
        #[arg(long)]
        space_id: Option<u64>,
        #[arg(long)]
        workspace_id: Option<usize>,
    },
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
    /// Display/mouse commands
    Display {
        #[command(subcommand)]
        display_cmd: DisplayCommands,
    },
    /// macOS space commands (Mission Control spaces, not virtual workspaces)
    Space {
        #[command(subcommand)]
        space_cmd: SpaceCommands,
    },
    /// Save the master file and exit Rift
    SaveAndExit,
    /// Save Rift's current layout state without exiting
    ///
    /// Use --master instead of PATH to update Rift's master file.
    SaveLayout {
        #[command(flatten)]
        file: LayoutFileSelection,
    },
    /// Restore a layout file to the current workspace or macOS Space
    ///
    /// Use --master instead of PATH to load Rift's master file.
    LoadLayout {
        #[command(flatten)]
        file: LayoutFileSelection,
        /// Restore one workspace or all saved workspaces for the current macOS Space.
        #[arg(long, value_enum, default_value_t = CliRestoreScope::Workspace)]
        scope: CliRestoreScope,
    },
    /// Print layout tree debugging output in the running rift instance
    Debug,
    /// Serialize and print runtime state
    Serialize,
    /// this command is deprecated, use `rift-cli execute space toggle-activated`
    #[deprecated]
    ToggleSpaceActivated,
    /// Show timing metrics
    ShowTiming,
}

#[derive(Subcommand)]
enum WindowCommands {
    /// Focus the next window
    Next,
    /// Focus the previous window
    Prev,
    /// Focus a window by direction or by a specific window ID
    Focus {
        /// Direction to focus (left, right, up, down)
        direction: Option<String>,
        /// Rift window ID as JSON (`{"pid":123,"idx":456}`) or debug text
        #[arg(long, conflicts_with = "direction")]
        window_id: Option<String>,
        /// Optional macOS window server ID for the target window
        #[arg(long, requires = "window_id")]
        window_server_id: Option<String>,
    },
    /// Toggle window floating state
    ToggleFloat,
    /// Toggle fullscreen mode (fills the whole screen, ignores outer gaps)
    ToggleFullscreen,
    /// Toggle fullscreen within configured outer gaps (respects outer gaps / fills tiling area)
    ToggleFullscreenWithinGaps,
    /// Grow the current window size (increments by ~5%).
    ResizeGrow {
        /// Axis to resize; smart chooses the nearest applicable split.
        #[arg(long, value_enum, default_value_t = CliResizeOrientation::Horizontal)]
        orientation: CliResizeOrientation,
    },
    /// Shrink the current window size (decrements by ~5%).
    ResizeShrink {
        /// Axis to resize; smart chooses the nearest applicable split.
        #[arg(long, value_enum, default_value_t = CliResizeOrientation::Horizontal)]
        orientation: CliResizeOrientation,
    },
    /// Resize the selected window by a fractional amount.
    /// - Pass a signed floating value: positive to grow, negative to shrink.
    /// - The value is a fraction of the current size (e.g. `0.05` = 5%).
    /// Examples:
    ///   rift-cli execute window resize-by --amount 0.05    # grow by 5%
    ///   rift-cli execute window resize-by --amount -0.10   # shrink by 10%
    ResizeBy { amount: f64 },
    /// Close a window as if Command-W was pressed
    Close {
        /// Optional window server ID; defaults to the focused window
        #[arg(long, visible_alias = "window-server-id")]
        window_id: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliRestoreScope {
    Workspace,
    Space,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliResizeOrientation {
    Horizontal,
    Vertical,
    Smart,
}

impl From<CliResizeOrientation> for rift_wm::layout_engine::ResizeOrientation {
    fn from(value: CliResizeOrientation) -> Self {
        match value {
            CliResizeOrientation::Horizontal => Self::Horizontal,
            CliResizeOrientation::Vertical => Self::Vertical,
            CliResizeOrientation::Smart => Self::Smart,
        }
    }
}

#[derive(Debug, Clone, Args)]
#[group(required = true, multiple = false)]
struct LayoutFileSelection {
    /// Layout file path.
    #[arg(value_name = "PATH")]
    path: Option<PathBuf>,
    /// Use Rift's master file (~/.rift/layout.ron).
    #[arg(long)]
    master: bool,
}

#[derive(Subcommand)]
enum SpaceCommands {
    /// Toggle whether rift manages the current macOS space
    ToggleActivated,
    /// Switch to an adjacent macOS space (Mission Control spaces, not virtual workspaces)
    Switch {
        /// Direction to switch (left, right, up, down)
        direction: String,
    },
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
        /// Switch to the destination workspace after moving the window.
        #[arg(long)]
        follow: bool,
        window_id: Option<u32>,
    },
    /// Create a new workspace
    Create,
    /// Switch to the last workspace
    Last,
    /// Set layout mode for a workspace (or active workspace when omitted)
    SetLayout {
        /// Workspace index (0-based). Defaults to active workspace if omitted.
        #[arg(long)]
        workspace_id: Option<usize>,
        /// Layout mode: traditional, bsp, stack, master_stack, scrolling
        mode: String,
    },
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
    /// Join with a neighbor, or unjoin when the selected window is already joined
    ConsumeOrExpelWindow { direction: String },
    /// Toggle stacked state for the selected container
    ToggleStack,
    /// Global orientation toggle that works consistently across layout modes (and between splits/stacks)
    ToggleOrientation,
    /// Unjoin previously joined windows
    Unjoin,
    /// Toggle floating on the focused selection (tree focus)
    ToggleFocusFloat,
    /// Adjust master ratio by a delta (master/stack layout only)
    AdjustMasterRatio { delta: f64 },
    /// Adjust master count by a delta (master/stack layout only)
    AdjustMasterCount { delta: i32 },
    /// Promote the selected window into the master area (master/stack layout only)
    PromoteToMaster,
    /// Swap the first master with the first stack window (master/stack layout only)
    SwapMasterStack,
    /// Swap two windows by window id (`WindowId { pid: ..., idx: ... }`)
    SwapWindows { a: String, b: String },
    /// Scroll the strip by a normalized delta (scrolling layout only)
    ScrollStrip { delta: f64 },
    /// Snap the strip to the nearest column boundary (scrolling layout only)
    SnapStrip,
    /// Toggle centering of the selected column in scrolling layout.
    /// If invoked again on the same selection, centering is removed.
    CenterSelection,
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
enum DisplayCommands {
    /// Focus a display by direction, index, or UUID.
    Focus {
        /// Direction relative to the current display (left, right, up, down).
        #[arg(long)]
        direction: Option<String>,
        /// Display index (0-based).
        #[arg(long)]
        index: Option<usize>,
        /// Display UUID.
        #[arg(long)]
        uuid: Option<String>,
    },
    /// Move mouse cursor to a display by index (0-based)
    MoveMouseToIndex {
        /// Display index (0-based)
        index: usize,
    },
    /// Move mouse cursor to a display by UUID
    MoveMouseToUuid {
        /// Display UUID
        uuid: String,
    },
    /// Move a window to a display by direction, index, or UUID.
    MoveWindow {
        /// Direction relative to the window's current display (left, right, up, down).
        #[arg(long)]
        direction: Option<String>,
        /// Display index (0-based).
        #[arg(long)]
        index: Option<usize>,
        /// Display UUID.
        #[arg(long)]
        uuid: Option<String>,
        /// Optional window id (window idx); defaults to the focused window if omitted.
        #[arg(long)]
        window_id: Option<u32>,
    },
}

#[derive(Subcommand)]
enum SubscribeCommands {
    /// Subscribe to Mach IPC events
    Mach {
        /// Event to subscribe to (workspace_changed, windows_changed, window_title_changed, focused_window_changed, stacks_changed, *)
        event: String,
    },
    /// Subscribe to events via CLI command execution
    Cli {
        /// Event to subscribe to (workspace_changed, windows_changed, window_title_changed, focused_window_changed, stacks_changed, *)
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

    let request = match cli.command {
        Commands::Service { .. } => {
            println!(
                "service commands have been moved to the `rift` binary. (ie `rift service install`)"
            );
            process::exit(0);
        }
        Commands::Subscribe {
            subscribe: SubscribeCommands::Mach { event },
        } => {
            if let Err(e) = run_mach_subscription(event) {
                eprintln!("Communication error: {}", e);
                eprintln!("Hint: ensure the rift service is running (try `rift service start`).");
                process::exit(1);
            }
            process::exit(0);
        }
        command => match build_request(command) {
            Ok(req) => req,
            Err(e) => {
                eprintln!("Error: {}", e);
                process::exit(1);
            }
        },
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
                if let Err(e) = write_json(
                    &data,
                    std::env::var("RIFT_CLI_PRETTY").map(|v| v != "0").unwrap_or(false),
                ) {
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
        QueryCommands::Workspaces { space_id } => Ok(RiftRequest::GetWorkspaces { space_id }),
        QueryCommands::Windows { space_id } => Ok(RiftRequest::GetWindows { space_id }),
        QueryCommands::Displays => Ok(RiftRequest::GetDisplays),
        QueryCommands::Window { window_id } => {
            let window_id = protocol_window_id(&parse_window_id(&window_id)?)?;
            Ok(RiftRequest::GetWindowInfo { window_id })
        }
        QueryCommands::Applications => Ok(RiftRequest::GetApplications),
        QueryCommands::Layout { space_id, workspace_id } => {
            Ok(RiftRequest::GetLayoutState { space_id, workspace_id })
        }
        QueryCommands::WorkspaceLayout { space_id, workspace_id } => {
            Ok(RiftRequest::GetWorkspaceLayouts { space_id, workspace_id })
        }
        QueryCommands::Metrics => Ok(RiftRequest::GetMetrics),
    }
}

fn build_subscribe_request(sub: SubscribeCommands) -> Result<RiftRequest, String> {
    match sub {
        SubscribeCommands::Mach { event } => Ok(RiftRequest::Subscribe {
            event: parse_event_kind(&event)?,
        }),
        SubscribeCommands::Cli { event, command, args } => Ok(RiftRequest::SubscribeCli {
            event: parse_event_kind(&event)?,
            command,
            args,
        }),
        SubscribeCommands::UnsubMach { event } => Ok(RiftRequest::Unsubscribe {
            event: parse_event_kind(&event)?,
        }),
        SubscribeCommands::UnsubCli { event } => Ok(RiftRequest::UnsubscribeCli {
            event: parse_event_kind(&event)?,
        }),
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
        ExecuteCommands::Display { display_cmd } => map_display_command(display_cmd)?,
        ExecuteCommands::Space { space_cmd } => map_space_command(space_cmd)?,
        ExecuteCommands::SaveAndExit => {
            CliCommand::Reactor(reactor::Command::Reactor(reactor::ReactorCommand::SaveAndExit))
        }
        ExecuteCommands::SaveLayout { file } => {
            let path = if file.master {
                rift_wm::common::config::restore_file()
            } else {
                absolute_layout_path(file.path.expect("clap requires either PATH or --master"))?
            };
            CliCommand::Reactor(reactor::Command::Reactor(reactor::ReactorCommand::SaveLayout {
                path,
            }))
        }
        ExecuteCommands::LoadLayout { file, scope } => {
            let (path, source) = if file.master {
                (
                    rift_wm::common::config::restore_file(),
                    layout::RestoreSource::CurrentSpace,
                )
            } else {
                (
                    absolute_layout_path(
                        file.path.expect("clap requires either PATH or --master"),
                    )?,
                    layout::RestoreSource::SavedActiveSpace,
                )
            };
            layout::LayoutEngine::load(path.clone()).map_err(|error| {
                format!("could not load layout file at {}: {error}", path.display())
            })?;
            let scope = match scope {
                CliRestoreScope::Workspace => layout::RestoreScope::Workspace,
                CliRestoreScope::Space => layout::RestoreScope::Space,
            };
            CliCommand::Reactor(reactor::Command::Reactor(
                reactor::ReactorCommand::RestoreLayout { path, scope, source },
            ))
        }
        ExecuteCommands::Debug => {
            CliCommand::Reactor(reactor::Command::Reactor(reactor::ReactorCommand::Debug))
        }
        ExecuteCommands::Serialize => {
            CliCommand::Reactor(reactor::Command::Reactor(reactor::ReactorCommand::Serialize))
        }
        #[allow(deprecated)]
        ExecuteCommands::ToggleSpaceActivated => {
            eprintln!("this command is deprecated, use rift-cli execute space toggle-activated");
            CliCommand::Reactor(reactor::Command::Reactor(
                reactor::ReactorCommand::ToggleSpaceActivated,
            ))
        }
        ExecuteCommands::ShowTiming => CliCommand::Reactor(reactor::Command::Metrics(
            rift_wm::common::log::MetricsCommand::ShowTiming,
        )),
    };

    if let CliCommand::Config(rift_wm::common::config::ConfigCommand::GetConfig) = &rift_command {
        return Ok(RiftRequest::GetConfig);
    }

    let command = into_protocol_command(rift_command)?;
    Ok(RiftRequest::ExecuteCommand { command })
}

fn into_protocol_command(command: CliCommand) -> Result<rift_protocol::RiftCommand, String> {
    match command {
        CliCommand::Config(command) => {
            Ok(rift_protocol::RiftCommand::Config(decode_protocol(command)?))
        }
        CliCommand::Reactor(reactor::Command::Layout(command)) => {
            Ok(rift_protocol::RiftCommand::Layout(decode_protocol(command)?))
        }
        CliCommand::Reactor(reactor::Command::Metrics(command)) => {
            Ok(rift_protocol::RiftCommand::Metrics(decode_protocol(command)?))
        }
        CliCommand::Reactor(reactor::Command::Reactor(command)) => {
            Ok(rift_protocol::RiftCommand::Reactor(decode_protocol(command)?))
        }
    }
}

fn decode_protocol<T, U>(value: T) -> Result<U, String>
where
    T: Serialize,
    U: DeserializeOwned, {
    serde_json::from_value(serde_json::to_value(value).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())
}

fn absolute_layout_path(path: PathBuf) -> Result<PathBuf, String> {
    if path.is_absolute() {
        Ok(path)
    } else {
        std::env::current_dir()
            .map(|current_dir| current_dir.join(path))
            .map_err(|error| format!("could not resolve layout path: {error}"))
    }
}

fn map_window_command(cmd: WindowCommands) -> Result<CliCommand, String> {
    use layout::LayoutCommand as LC;
    match cmd {
        WindowCommands::Next => Ok(CliCommand::Reactor(reactor::Command::Layout(LC::NextWindow))),
        WindowCommands::Prev => Ok(CliCommand::Reactor(reactor::Command::Layout(LC::PrevWindow))),
        WindowCommands::Focus {
            direction,
            window_id,
            window_server_id,
        } => match (direction, window_id) {
            (Some(direction), None) => Ok(CliCommand::Reactor(reactor::Command::Layout(
                LC::MoveFocus(parse_focus_direction(&direction)?),
            ))),
            (None, Some(window_id)) => Ok(CliCommand::Reactor(reactor::Command::Reactor(
                reactor::ReactorCommand::FocusWindow {
                    window_id: parse_window_id(&window_id)?.into(),
                    window_server_id: window_server_id
                        .as_deref()
                        .map(parse_window_server_id)
                        .map(|result| result.map(|id| id.as_u32()))
                        .transpose()?,
                },
            ))),
            (None, None) => Err("window focus requires a direction or --window-id".to_string()),
            (Some(_), Some(_)) => {
                Err("window focus accepts either a direction or --window-id, not both".to_string())
            }
        },
        WindowCommands::ToggleFloat => Ok(CliCommand::Reactor(reactor::Command::Layout(
            LC::ToggleWindowFloating,
        ))),
        WindowCommands::ToggleFullscreen => Ok(CliCommand::Reactor(reactor::Command::Layout(
            LC::ToggleFullscreen,
        ))),
        WindowCommands::ToggleFullscreenWithinGaps => Ok(CliCommand::Reactor(
            reactor::Command::Layout(LC::ToggleFullscreenWithinGaps),
        )),
        WindowCommands::ResizeGrow { orientation } => Ok(CliCommand::Reactor(
            reactor::Command::Layout(LC::ResizeWindowGrow(orientation.into())),
        )),
        WindowCommands::ResizeShrink { orientation } => Ok(CliCommand::Reactor(
            reactor::Command::Layout(LC::ResizeWindowShrink(orientation.into())),
        )),
        WindowCommands::ResizeBy { amount } => Ok(CliCommand::Reactor(reactor::Command::Layout(
            LC::ResizeWindowBy { amount },
        ))),
        WindowCommands::Close { window_id } => {
            let window_server_id = window_id.as_deref().map(parse_window_server_id).transpose()?;
            Ok(CliCommand::Reactor(reactor::Command::Reactor(
                reactor::ReactorCommand::CloseWindow {
                    window_server_id: window_server_id.map(|id| id.as_u32()),
                },
            )))
        }
    }
}

fn parse_window_server_id(input: &str) -> Result<WindowServerId, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("window_server_id cannot be empty".to_string());
    }

    let value = if trimmed.starts_with("0x") {
        u32::from_str_radix(trimmed.trim_start_matches("0x"), 16)
            .map_err(|_| format!("Invalid hexadecimal window server id: {}", trimmed))?
    } else {
        trimmed.parse().map_err(|_| format!("Invalid window server id: {}", trimmed))?
    };
    Ok(WindowServerId::new(value))
}

fn parse_window_id(input: &str) -> Result<InternalWindowId, String> {
    let input = input.trim();
    if let Ok(window_id) = serde_json::from_str(input) {
        return Ok(window_id);
    }
    if let Some(window_id) = InternalWindowId::from_debug_string(input) {
        return Ok(window_id);
    }

    Err(format!(
        "Invalid window id '{}'; expected `{{\"pid\":123,\"idx\":456}}` or `WindowId {{ pid: 123, idx: 456 }}`",
        input
    ))
}

fn protocol_window_id(window_id: &InternalWindowId) -> Result<rift_protocol::WindowId, String> {
    rift_protocol::WindowId::new(window_id.pid, window_id.idx.get())
        .ok_or_else(|| "window id index must be non-zero".to_string())
}

fn parse_event_kind(input: &str) -> Result<EventKind, String> {
    match input.trim().to_ascii_lowercase().as_str() {
        "workspace_changed" => Ok(EventKind::WorkspaceChanged),
        "windows_changed" => Ok(EventKind::WindowsChanged),
        "window_title_changed" => Ok(EventKind::WindowTitleChanged),
        "focused_window_changed" => Ok(EventKind::FocusedWindowChanged),
        "stacks_changed" => Ok(EventKind::StacksChanged),
        "layout_changed" => Ok(EventKind::LayoutChanged),
        "*" => Ok(EventKind::All),
        other => Err(format!(
            "Invalid event '{}'; expected workspace_changed, windows_changed, window_title_changed, focused_window_changed, stacks_changed, layout_changed, or *",
            other
        )),
    }
}

fn parse_layout_mode(value: &str) -> Result<LayoutMode, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "traditional" => Ok(LayoutMode::Traditional),
        "bsp" => Ok(LayoutMode::Bsp),
        "stack" => Ok(LayoutMode::Stack),
        "master_stack" => Ok(LayoutMode::MasterStack),
        "scrolling" => Ok(LayoutMode::Scrolling),
        other => Err(format!(
            "Invalid layout mode '{}'; must be traditional, bsp, stack, master_stack, or scrolling",
            other
        )),
    }
}

fn map_workspace_command(cmd: WorkspaceCommands) -> Result<CliCommand, String> {
    use layout::LayoutCommand as LC;
    match cmd {
        WorkspaceCommands::Next { skip_empty } => Ok(CliCommand::Reactor(
            reactor::Command::Layout(LC::NextWorkspace(skip_empty)),
        )),
        WorkspaceCommands::Prev { skip_empty } => Ok(CliCommand::Reactor(
            reactor::Command::Layout(LC::PrevWorkspace(skip_empty)),
        )),
        WorkspaceCommands::Switch { workspace_id } => Ok(CliCommand::Reactor(
            reactor::Command::Layout(LC::SwitchToWorkspace(workspace_id)),
        )),
        WorkspaceCommands::MoveWindow {
            workspace_id,
            follow,
            window_id,
        } => Ok(CliCommand::Reactor(reactor::Command::Layout(
            LC::MoveWindowToWorkspace {
                workspace: WorkspaceSelector::Index(workspace_id),
                follow,
                window_id,
            },
        ))),
        WorkspaceCommands::Create => Ok(CliCommand::Reactor(reactor::Command::Layout(
            LC::CreateWorkspace,
        ))),
        WorkspaceCommands::Last => Ok(CliCommand::Reactor(reactor::Command::Layout(
            LC::SwitchToLastWorkspace,
        ))),
        WorkspaceCommands::SetLayout { workspace_id, mode } => {
            let mode = parse_layout_mode(&mode)?;
            Ok(CliCommand::Reactor(reactor::Command::Layout(
                LC::SetWorkspaceLayout { workspace: workspace_id, mode },
            )))
        }
    }
}

fn map_layout_command(cmd: LayoutCommands) -> Result<CliCommand, String> {
    use layout::LayoutCommand as LC;
    match cmd {
        LayoutCommands::Ascend => Ok(CliCommand::Reactor(reactor::Command::Layout(LC::Ascend))),
        LayoutCommands::Descend => Ok(CliCommand::Reactor(reactor::Command::Layout(LC::Descend))),
        LayoutCommands::MoveNode { direction } => Ok(CliCommand::Reactor(
            reactor::Command::Layout(LC::MoveNode(direction.into())),
        )),
        LayoutCommands::JoinWindow { direction } => Ok(CliCommand::Reactor(
            reactor::Command::Layout(LC::JoinWindow(direction.into())),
        )),
        LayoutCommands::ConsumeOrExpelWindow { direction } => Ok(CliCommand::Reactor(
            reactor::Command::Layout(LC::ConsumeOrExpelWindow(direction.into())),
        )),
        LayoutCommands::ToggleStack => {
            Ok(CliCommand::Reactor(reactor::Command::Layout(LC::ToggleStack)))
        }
        LayoutCommands::ToggleOrientation => Ok(CliCommand::Reactor(reactor::Command::Layout(
            LC::ToggleOrientation,
        ))),
        LayoutCommands::Unjoin => {
            Ok(CliCommand::Reactor(reactor::Command::Layout(LC::UnjoinWindows)))
        }
        LayoutCommands::ToggleFocusFloat => Ok(CliCommand::Reactor(reactor::Command::Layout(
            LC::ToggleFocusFloating,
        ))),
        LayoutCommands::AdjustMasterRatio { delta } => Ok(CliCommand::Reactor(
            reactor::Command::Layout(LC::AdjustMasterRatio(delta)),
        )),
        LayoutCommands::AdjustMasterCount { delta } => Ok(CliCommand::Reactor(
            reactor::Command::Layout(LC::AdjustMasterCount { delta }),
        )),
        LayoutCommands::PromoteToMaster => Ok(CliCommand::Reactor(reactor::Command::Layout(
            LC::PromoteToMaster,
        ))),
        LayoutCommands::SwapMasterStack => Ok(CliCommand::Reactor(reactor::Command::Layout(
            LC::SwapMasterStack,
        ))),
        LayoutCommands::SwapWindows { a, b } => Ok(CliCommand::Reactor(reactor::Command::Layout(
            LC::SwapWindows(parse_window_id(&a)?.into(), parse_window_id(&b)?.into()),
        ))),
        LayoutCommands::ScrollStrip { delta } => {
            Ok(CliCommand::Reactor(reactor::Command::Layout(LC::ScrollStrip {
                delta,
            })))
        }
        LayoutCommands::SnapStrip => {
            Ok(CliCommand::Reactor(reactor::Command::Layout(LC::SnapStrip)))
        }
        LayoutCommands::CenterSelection => Ok(CliCommand::Reactor(reactor::Command::Layout(
            LC::CenterSelection,
        ))),
    }
}

fn map_config_command(cmd: ConfigCommands) -> Result<CliCommand, String> {
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

    Ok(CliCommand::Config(cfg_cmd))
}

fn map_mission_control_command(cmd: MissionControlCommands) -> Result<CliCommand, String> {
    match cmd {
        MissionControlCommands::ShowAll => Ok(CliCommand::Reactor(reactor::Command::Reactor(
            reactor::ReactorCommand::ShowMissionControlAll,
        ))),
        MissionControlCommands::ShowCurrent => Ok(CliCommand::Reactor(reactor::Command::Reactor(
            reactor::ReactorCommand::ShowMissionControlCurrent,
        ))),
        MissionControlCommands::Dismiss => Ok(CliCommand::Reactor(reactor::Command::Reactor(
            reactor::ReactorCommand::DismissMissionControl,
        ))),
    }
}

fn map_space_command(cmd: SpaceCommands) -> Result<CliCommand, String> {
    let command = match cmd {
        SpaceCommands::ToggleActivated => reactor::ReactorCommand::ToggleSpaceActivated,
        SpaceCommands::Switch { direction } => {
            reactor::ReactorCommand::SwitchSpace(parse_focus_direction(&direction)?)
        }
    };

    Ok(CliCommand::Reactor(reactor::Command::Reactor(command)))
}

fn map_display_command(cmd: DisplayCommands) -> Result<CliCommand, String> {
    match cmd {
        DisplayCommands::Focus { direction, index, uuid } => {
            let selector = build_display_selector(direction, index, uuid)?;
            Ok(CliCommand::Reactor(reactor::Command::Reactor(
                reactor::ReactorCommand::FocusDisplay(selector),
            )))
        }
        DisplayCommands::MoveMouseToIndex { index } => {
            Ok(CliCommand::Reactor(reactor::Command::Reactor(
                reactor::ReactorCommand::MoveMouseToDisplay(DisplaySelector::Index(index)),
            )))
        }
        DisplayCommands::MoveMouseToUuid { uuid } => {
            Ok(CliCommand::Reactor(reactor::Command::Reactor(
                reactor::ReactorCommand::MoveMouseToDisplay(DisplaySelector::Uuid(uuid)),
            )))
        }
        DisplayCommands::MoveWindow {
            direction,
            index,
            uuid,
            window_id,
        } => Ok(CliCommand::Reactor(reactor::Command::Reactor(
            reactor::ReactorCommand::MoveWindowToDisplay {
                selector: build_display_selector(direction, index, uuid)?,
                window_id,
            },
        ))),
    }
}

fn build_display_selector(
    direction: Option<String>,
    index: Option<usize>,
    uuid: Option<String>,
) -> Result<DisplaySelector, String> {
    let provided =
        direction.is_some() as usize + index.is_some() as usize + uuid.is_some() as usize;
    if provided != 1 {
        return Err(
            "display selection requires exactly one of --direction, --index, or --uuid".to_string(),
        );
    }

    if let Some(direction) = direction {
        let parsed_direction = parse_focus_direction(&direction)?;
        Ok(DisplaySelector::Direction(parsed_direction))
    } else if let Some(index) = index {
        Ok(DisplaySelector::Index(index))
    } else if let Some(uuid) = uuid {
        Ok(DisplaySelector::Uuid(uuid))
    } else {
        unreachable!("At least one selector value is guaranteed to be provided")
    }
}

fn parse_focus_direction(value: &str) -> Result<layout::Direction, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "left" => Ok(layout::Direction::Left),
        "right" => Ok(layout::Direction::Right),
        "up" => Ok(layout::Direction::Up),
        "down" => Ok(layout::Direction::Down),
        other => Err(format!(
            "Invalid focus direction '{}'; must be left, right, up, or down",
            other
        )),
    }
}

fn write_json<T: Serialize>(value: &T, pretty: bool) -> Result<(), String> {
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    let mut writer = io::BufWriter::new(&mut handle);

    if pretty {
        serde_json::to_writer_pretty(&mut writer, value).map_err(|e| e.to_string())?;
    } else {
        serde_json::to_writer(&mut writer, value).map_err(|e| e.to_string())?;
    }
    writer.write_all(b"\n").map_err(|e| e.to_string())?;
    writer.flush().map_err(|e| e.to_string())
}

fn run_mach_subscription(event: String) -> Result<(), String> {
    let pretty = std::env::var("RIFT_CLI_PRETTY").map(|v| v != "0").unwrap_or(false);
    let client = RiftMachClient::connect().map_err(|e| e.to_string())?;
    let event_kind = parse_event_kind(&event)?;
    let subscription = client.subscribe(event_kind).map_err(|e| e.to_string())?;

    loop {
        let event_payload = subscription.recv_event().map_err(|e| e.to_string())?;
        // Exit cleanly when output is closed by the consumer.
        if let Err(e) = write_json(&event_payload, pretty) {
            if e.contains("Broken pipe") {
                return Ok(());
            }
            return Err(format!("Failed to write event output: {e}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execute_requests_use_typed_protocol_commands() {
        let request = build_execute_request(ExecuteCommands::Window {
            window_cmd: WindowCommands::Next,
        })
        .unwrap();

        assert_eq!(
            serde_json::to_value(request).unwrap(),
            serde_json::json!({
                "execute_command": { "command": { "layout": "next_window" } }
            })
        );
    }

    #[test]
    fn config_commands_are_not_embedded_as_json_strings() {
        let request = build_execute_request(ExecuteCommands::Config {
            config_cmd: ConfigCommands::SetAnimate { value: "true".into() },
        })
        .unwrap();

        assert_eq!(
            serde_json::to_value(request).unwrap(),
            serde_json::json!({
                "execute_command": { "command": { "config": { "set_animate": true } } }
            })
        );
    }
}
