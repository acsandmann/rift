use std::io::{self, Write};
use std::path::PathBuf;
use std::process::{self};

use clap::{Parser, Subcommand};
use rift_wm::actor::app::WindowId;
use rift_wm::actor::reactor::{self, DisplaySelector};
use rift_wm::common::config::Config;
use rift_wm::common::config::LayoutMode;
use rift_wm::ipc::{RiftCommand, RiftMachClient, RiftRequest, RiftResponse};
use rift_wm::layout_engine as layout;
use rift_wm::sys::window_server::WindowServerId;
use serde_json::Value;

use rift_wm::cli::rift_cli::*;

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
        Commands::Verify { config_path } => {
            verify_config(&config_path).unwrap_or_else(|e| {
                eprintln!("{}", e);
                println!("{}", e);
                process::exit(1);
            });
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

fn verify_config(config_path: &PathBuf) -> Result<(), String> {
    Config::read(&config_path).map_err(|e| format!("Config verification failed: {}", e))?;
    Ok(())
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
        Commands::Verify { .. } => Err(
            "Config verification is handled locally and should not be sent to the rift server."
                .to_string(),
        ),
    }
}

fn build_query_request(query: QueryCommands) -> Result<RiftRequest, String> {
    match query {
        QueryCommands::Workspaces { space_id } => Ok(RiftRequest::GetWorkspaces { space_id }),
        QueryCommands::Windows { space_id } => Ok(RiftRequest::GetWindows { space_id }),
        QueryCommands::Displays => Ok(RiftRequest::GetDisplays),
        QueryCommands::Window { window_id } => Ok(RiftRequest::GetWindowInfo { window_id }),
        QueryCommands::Applications => Ok(RiftRequest::GetApplications),
        QueryCommands::Layout { space_id } => Ok(RiftRequest::GetLayoutState { space_id }),
        QueryCommands::WorkspaceLayout { space_id, workspace_id } => {
            Ok(RiftRequest::GetWorkspaceLayouts { space_id, workspace_id })
        }
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
        ExecuteCommands::Display { display_cmd } => map_display_command(display_cmd)?,
        ExecuteCommands::SaveAndExit => {
            RiftCommand::Reactor(reactor::Command::Reactor(reactor::ReactorCommand::SaveAndExit))
        }
        ExecuteCommands::Debug => {
            RiftCommand::Reactor(reactor::Command::Reactor(reactor::ReactorCommand::Debug))
        }
        ExecuteCommands::Serialize => {
            RiftCommand::Reactor(reactor::Command::Reactor(reactor::ReactorCommand::Serialize))
        }
        ExecuteCommands::ToggleSpaceActivated => RiftCommand::Reactor(reactor::Command::Reactor(
            reactor::ReactorCommand::ToggleSpaceActivated,
        )),
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
        WindowCommands::Close { window_id } => {
            let wsid = parse_window_server_id(&window_id)?;
            Ok(RiftCommand::Reactor(reactor::Command::Reactor(
                reactor::ReactorCommand::CloseWindow { window_server_id: Some(wsid) },
            )))
        }
        WindowCommands::AddScratchpad => {
            Ok(RiftCommand::Reactor(reactor::Command::Layout(LC::AddScratchpad)))
        }
        WindowCommands::ToggleScratchpad { name } => {
            Ok(RiftCommand::Reactor(reactor::Command::Layout(match name {
                Some(name) => LC::ToggleScratchpadNamed(name),
                None => LC::ToggleScratchpad,
            })))
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

fn parse_window_id(input: &str) -> Result<WindowId, String> {
    WindowId::from_debug_string(input.trim()).ok_or_else(|| {
        format!(
            "Invalid window id '{}'; expected `WindowId {{ pid: 123, idx: 456 }}`",
            input
        )
    })
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
        WorkspaceCommands::SetLayout { workspace_id, mode } => {
            let mode = parse_layout_mode(&mode)?;
            Ok(RiftCommand::Reactor(reactor::Command::Layout(
                LC::SetWorkspaceLayout { workspace: workspace_id, mode },
            )))
        }
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
        LayoutCommands::AdjustMasterRatio { delta } => Ok(RiftCommand::Reactor(
            reactor::Command::Layout(LC::AdjustMasterRatio { delta }),
        )),
        LayoutCommands::AdjustMasterCount { delta } => Ok(RiftCommand::Reactor(
            reactor::Command::Layout(LC::AdjustMasterCount { delta }),
        )),
        LayoutCommands::PromoteToMaster => Ok(RiftCommand::Reactor(reactor::Command::Layout(
            LC::PromoteToMaster,
        ))),
        LayoutCommands::SwapMasterStack => Ok(RiftCommand::Reactor(reactor::Command::Layout(
            LC::SwapMasterStack,
        ))),
        LayoutCommands::SwapWindows { a, b } => Ok(RiftCommand::Reactor(reactor::Command::Layout(
            LC::SwapWindows(parse_window_id(&a)?, parse_window_id(&b)?),
        ))),
        LayoutCommands::ScrollStrip { delta } => {
            Ok(RiftCommand::Reactor(reactor::Command::Layout(LC::ScrollStrip {
                delta,
            })))
        }
        LayoutCommands::SnapStrip => {
            Ok(RiftCommand::Reactor(reactor::Command::Layout(LC::SnapStrip)))
        }
        LayoutCommands::CenterSelection => Ok(RiftCommand::Reactor(reactor::Command::Layout(
            LC::CenterSelection,
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

fn map_display_command(cmd: DisplayCommands) -> Result<RiftCommand, String> {
    match cmd {
        DisplayCommands::Focus { direction, index, uuid } => {
            let selector = build_display_selector(direction, index, uuid)?;
            Ok(RiftCommand::Reactor(reactor::Command::Reactor(
                reactor::ReactorCommand::FocusDisplay(selector),
            )))
        }
        DisplayCommands::MoveMouseToIndex { index } => {
            Ok(RiftCommand::Reactor(reactor::Command::Reactor(
                reactor::ReactorCommand::MoveMouseToDisplay(DisplaySelector::Index(index)),
            )))
        }
        DisplayCommands::MoveMouseToUuid { uuid } => {
            Ok(RiftCommand::Reactor(reactor::Command::Reactor(
                reactor::ReactorCommand::MoveMouseToDisplay(DisplaySelector::Uuid(uuid)),
            )))
        }
        DisplayCommands::MoveWindow {
            direction,
            index,
            uuid,
            window_id,
        } => Ok(RiftCommand::Reactor(reactor::Command::Reactor(
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

fn write_json(value: &Value, pretty: bool) -> Result<(), String> {
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
    let client = RiftMachClient::connect()?;
    let subscription = client.subscribe(event)?;

    loop {
        let event_payload = subscription.recv_event()?;
        // Exit cleanly when output is closed by the consumer.
        if let Err(e) = write_json(&event_payload, pretty) {
            if e.contains("Broken pipe") {
                return Ok(());
            }
            return Err(format!("Failed to write event output: {e}"));
        }
    }
}
