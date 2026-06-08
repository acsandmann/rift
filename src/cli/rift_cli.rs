use clap::{Parser, Subcommand};
#[derive(Parser)]
#[command(name = "rift-cli")]
#[command(about = "Command-line interface for rift window manager")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
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
    Verify {
        config_path: std::path::PathBuf,
    },
}

#[derive(Subcommand)]
pub enum ServiceCommands {
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
pub enum QueryCommands {
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
    /// Get layout state for a space
    Layout { space_id: u64 },
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
pub enum ExecuteCommands {
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
    /// Save current state and exit rift
    SaveAndExit,
    /// Print layout tree debugging output in the running rift instance
    Debug,
    /// Serialize and print runtime state
    Serialize,
    /// Toggle whether the current space is managed by rift
    ToggleSpaceActivated,
    /// Show timing metrics
    ShowTiming,
}

#[derive(Subcommand)]
pub enum WindowCommands {
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
    ResizeBy {
        amount: f64,
    },
    /// Close a window by window server identifier
    Close {
        /// Window Id (window server id or idx from window id)
        #[arg(long)]
        window_id: String,
    },
    AddScratchpad,
    ToggleScratchpad {
        /// Name of the scratchpad (optional, defaults to "default")
        #[arg(long)]
        name: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum WorkspaceCommands {
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
pub enum LayoutCommands {
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
pub enum ConfigCommands {
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
        #[arg(action = clap::ArgAction::Set)]
        value: bool,
    },
    SetMouseHidesOnFocus {
        #[arg(action = clap::ArgAction::Set)]
        value: bool,
    },
    SetFocusFollowsMouse {
        #[arg(action = clap::ArgAction::Set)]
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
pub enum MissionControlCommands {
    /// Show all workspaces in mission control
    ShowAll,
    /// Show current workspace in mission control
    ShowCurrent,
    /// Dismiss mission control
    Dismiss,
}

#[derive(Subcommand)]
pub enum DisplayCommands {
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
pub enum SubscribeCommands {
    /// Subscribe to Mach IPC events
    Mach {
        /// Event to subscribe to (workspace_changed, windows_changed, window_title_changed, stacks_changed, *)
        event: String,
    },
    /// Subscribe to events via CLI command execution
    Cli {
        /// Event to subscribe to (workspace_changed, windows_changed, window_title_changed, stacks_changed, *)
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
