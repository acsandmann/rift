use std::process;

use clap::{Parser, Subcommand};
use rift_wm::actor::reactor;
use rift_wm::layout_engine as layout;
use rift_wm::layout_engine::Direction;
use rift_wm::model::server::{ApplicationData, LayoutStateData, WindowData, WorkspaceData};
use rift_wm::server::{RiftCommand, RiftMachClient, RiftRequest};

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
    /// Toggle fullscreen mode
    ToggleFullscreen,
    /// Grow the current window size
    ResizeGrow,
    /// Shrink the current window size
    ResizeShrink,
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
    MoveWindow { workspace_id: usize },
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
    /// Stack the selected windows
    Stack,
    /// Unstack the selected windows
    Unstack,
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

    /// Get current config
    Get,

    /// Save current config to file
    Save,

    /// Reload config from file
    Reload,
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
    let cli = Cli::parse();

    let request = match build_request(cli.command) {
        Ok(req) => req,
        Err(e) => {
            eprintln!("Error: {}", e);
            process::exit(1);
        }
    };

    let client = match RiftMachClient::connect() {
        Ok(client) => client,
        Err(e) => {
            eprintln!("Failed to connect to rift: {}", e);
            eprintln!("Make sure rift is running with Mach server enabled.");
            process::exit(1);
        }
    };

    let response = client.send_request(&request);

    match response {
        Ok(response) => match response {
            rift_wm::server::RiftResponse::Success { data } => match &request {
                RiftRequest::GetWorkspaces => {
                    let typed: Vec<WorkspaceData> = serde_json::from_value(data).unwrap();
                    println!("{}", serde_json::to_string_pretty(&typed).unwrap());
                }
                RiftRequest::GetWindows { .. } => {
                    let typed: Vec<WindowData> = serde_json::from_value(data).unwrap();
                    println!("{}", serde_json::to_string_pretty(&typed).unwrap());
                }
                RiftRequest::GetWindowInfo { .. } => {
                    let typed: WindowData = serde_json::from_value(data).unwrap();
                    println!("{}", serde_json::to_string_pretty(&typed).unwrap());
                }
                RiftRequest::GetApplications => {
                    let typed: Vec<ApplicationData> = serde_json::from_value(data).unwrap();
                    println!("{}", serde_json::to_string_pretty(&typed).unwrap());
                }
                RiftRequest::GetLayoutState { .. } => {
                    let typed: LayoutStateData = serde_json::from_value(data).unwrap();
                    println!("{}", serde_json::to_string_pretty(&typed).unwrap());
                }
                _ => {
                    println!("{}", serde_json::to_string_pretty(&data).unwrap());
                }
            },
            rift_wm::server::RiftResponse::Error { message } => {
                eprintln!("Error: {}", message);
                process::exit(1);
            }
            _ => {
                eprintln!("Received an unknown response shape from rift");
                process::exit(1);
            }
        },
        Err(e) => {
            eprintln!("Communication error: {}", e);
            process::exit(1);
        }
    }
}

fn build_request(command: Commands) -> Result<RiftRequest, String> {
    match command {
        Commands::Query { query } => match query {
            QueryCommands::Workspaces => Ok(RiftRequest::GetWorkspaces),
            QueryCommands::Windows { space_id } => Ok(RiftRequest::GetWindows { space_id }),
            QueryCommands::Window { window_id } => Ok(RiftRequest::GetWindowInfo { window_id }),
            QueryCommands::Applications => Ok(RiftRequest::GetApplications),
            QueryCommands::Layout { space_id } => Ok(RiftRequest::GetLayoutState { space_id }),
            QueryCommands::Metrics => Ok(RiftRequest::GetMetrics),
        },
        Commands::Execute { command } => {
            let rift_command = match command {
                ExecuteCommands::Window { window_cmd } => match window_cmd {
                    WindowCommands::Next => RiftCommand::Reactor(reactor::Command::Layout(
                        layout::LayoutCommand::NextWindow,
                    )),
                    WindowCommands::Prev => RiftCommand::Reactor(reactor::Command::Layout(
                        layout::LayoutCommand::PrevWindow,
                    )),
                    WindowCommands::Focus { direction } => {
                        let dir = match direction.as_str() {
                            "up" => Direction::Up,
                            "down" => Direction::Down,
                            "left" => Direction::Left,
                            "right" => Direction::Right,
                            _ => {
                                return Err(
                                    "Invalid direction. Use: up, down, left, right".to_string()
                                );
                            }
                        };
                        RiftCommand::Reactor(reactor::Command::Layout(
                            layout::LayoutCommand::MoveFocus(dir),
                        ))
                    }
                    WindowCommands::ToggleFloat => RiftCommand::Reactor(reactor::Command::Layout(
                        layout::LayoutCommand::ToggleWindowFloating,
                    )),
                    WindowCommands::ToggleFullscreen => RiftCommand::Reactor(
                        reactor::Command::Layout(layout::LayoutCommand::ToggleFullscreen),
                    ),
                    WindowCommands::ResizeGrow => RiftCommand::Reactor(reactor::Command::Layout(
                        layout::LayoutCommand::ResizeWindowGrow,
                    )),
                    WindowCommands::ResizeShrink => RiftCommand::Reactor(reactor::Command::Layout(
                        layout::LayoutCommand::ResizeWindowShrink,
                    )),
                },
                ExecuteCommands::Workspace { workspace_cmd } => match workspace_cmd {
                    WorkspaceCommands::Next { skip_empty } => RiftCommand::Reactor(
                        reactor::Command::Layout(layout::LayoutCommand::NextWorkspace(skip_empty)),
                    ),
                    WorkspaceCommands::Prev { skip_empty } => RiftCommand::Reactor(
                        reactor::Command::Layout(layout::LayoutCommand::PrevWorkspace(skip_empty)),
                    ),
                    WorkspaceCommands::Switch { workspace_id } => {
                        RiftCommand::Reactor(reactor::Command::Layout(
                            layout::LayoutCommand::SwitchToWorkspace(workspace_id),
                        ))
                    }
                    WorkspaceCommands::MoveWindow { workspace_id } => {
                        RiftCommand::Reactor(reactor::Command::Layout(
                            layout::LayoutCommand::MoveWindowToWorkspace(workspace_id),
                        ))
                    }
                    WorkspaceCommands::Create => RiftCommand::Reactor(reactor::Command::Layout(
                        layout::LayoutCommand::CreateWorkspace,
                    )),
                    WorkspaceCommands::Last => RiftCommand::Reactor(reactor::Command::Layout(
                        layout::LayoutCommand::SwitchToLastWorkspace,
                    )),
                },
                ExecuteCommands::Layout { layout_cmd } => match layout_cmd {
                    LayoutCommands::Ascend => RiftCommand::Reactor(reactor::Command::Layout(
                        layout::LayoutCommand::Ascend,
                    )),
                    LayoutCommands::Descend => RiftCommand::Reactor(reactor::Command::Layout(
                        layout::LayoutCommand::Descend,
                    )),
                    LayoutCommands::MoveNode { direction } => {
                        let dir = match direction.as_str() {
                            "up" => Direction::Up,
                            "down" => Direction::Down,
                            "left" => Direction::Left,
                            "right" => Direction::Right,
                            _ => {
                                return Err(
                                    "Invalid direction. Use: up, down, left, right".to_string()
                                );
                            }
                        };
                        RiftCommand::Reactor(reactor::Command::Layout(
                            layout::LayoutCommand::MoveNode(dir),
                        ))
                    }
                    LayoutCommands::JoinWindow { direction } => {
                        let dir = match direction.as_str() {
                            "up" => Direction::Up,
                            "down" => Direction::Down,
                            "left" => Direction::Left,
                            "right" => Direction::Right,
                            _ => {
                                return Err(
                                    "Invalid direction. Use: up, down, left, right".to_string()
                                );
                            }
                        };
                        RiftCommand::Reactor(reactor::Command::Layout(
                            layout::LayoutCommand::JoinWindow(dir),
                        ))
                    }
                    LayoutCommands::Stack => RiftCommand::Reactor(reactor::Command::Layout(
                        layout::LayoutCommand::StackWindows,
                    )),
                    LayoutCommands::Unstack => RiftCommand::Reactor(reactor::Command::Layout(
                        layout::LayoutCommand::UnstackWindows,
                    )),
                    LayoutCommands::Unjoin => RiftCommand::Reactor(reactor::Command::Layout(
                        layout::LayoutCommand::UnjoinWindows,
                    )),
                    LayoutCommands::ToggleFocusFloat => RiftCommand::Reactor(
                        reactor::Command::Layout(layout::LayoutCommand::ToggleFocusFloating),
                    ),
                },
                ExecuteCommands::Config { config_cmd } => {
                    use rift_wm::common::config::{AnimationEasing, ConfigCommand};

                    let cmd = match config_cmd {
                        ConfigCommands::SetAnimate { value } => {
                            let bool_value = match value.to_lowercase().as_str() {
                                "true" | "on" => true,
                                "false" | "off" => false,
                                _ => {
                                    return Err(format!(
                                        "Invalid boolean value: {}. Use true/false",
                                        value
                                    ));
                                }
                            };
                            ConfigCommand::SetAnimate(bool_value)
                        }
                        ConfigCommands::SetAnimationDuration { value } => {
                            ConfigCommand::SetAnimationDuration(value)
                        }
                        ConfigCommands::SetAnimationFps { value } => {
                            ConfigCommand::SetAnimationFps(value)
                        }
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
                        ConfigCommands::SetStackOffset { value } => {
                            ConfigCommand::SetStackOffset(value)
                        }
                        ConfigCommands::SetOuterGaps { top, left, bottom, right } => {
                            ConfigCommand::SetOuterGaps { top, left, bottom, right }
                        }
                        ConfigCommands::SetInnerGaps { horizontal, vertical } => {
                            ConfigCommand::SetInnerGaps { horizontal, vertical }
                        }
                        ConfigCommands::SetWorkspaceNames { names } => {
                            ConfigCommand::SetWorkspaceNames(names)
                        }
                        ConfigCommands::Get => {
                            // Use direct query instead of command for Get
                            return Ok(RiftRequest::GetConfig);
                        }
                        ConfigCommands::Save => ConfigCommand::SaveConfig,
                        ConfigCommands::Reload => ConfigCommand::ReloadConfig,
                    };

                    RiftCommand::Reactor(reactor::Command::Config(cmd))
                }
                ExecuteCommands::ShowTiming => RiftCommand::Reactor(reactor::Command::Metrics(
                    rift_wm::common::log::MetricsCommand::ShowTiming,
                )),
            };

            Ok(RiftRequest::ExecuteCommand {
                command: serde_json::to_string(&rift_command).unwrap(),
                args: vec![],
            })
        }
        Commands::Subscribe { subscribe } => match subscribe {
            SubscribeCommands::Mach { event } => Ok(RiftRequest::Subscribe { event }),
            SubscribeCommands::Cli { event, command, args } => {
                Ok(RiftRequest::SubscribeCli { event, command, args })
            }
            SubscribeCommands::UnsubMach { event } => Ok(RiftRequest::Unsubscribe { event }),
            SubscribeCommands::UnsubCli { event } => Ok(RiftRequest::UnsubscribeCli { event }),
            SubscribeCommands::ListCli => Ok(RiftRequest::ListCliSubscriptions),
        },
    }
}
