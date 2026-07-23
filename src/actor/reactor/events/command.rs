use tracing::{error, info, warn};

use super::super::ScreenInfo;
use crate::actor::app::{AppThreadHandle, Quiet, WindowId};
use crate::actor::raise_manager;
use crate::actor::reactor::WorkspaceSwitchOrigin;
use crate::actor::reactor::events::EventOutcome;
use crate::actor::reactor::managers::{
    AppManager, DragManager, LayoutManager, WorkspaceSwitchManager,
};
use crate::actor::spaces::ForwardedSpaceState;
use crate::common::collections::HashMap;
use crate::common::config::{self as config, Config};
use crate::common::log::{MetricsCommand, handle_command as handle_metrics_command};
use crate::layout_engine::{EventResponse, LayoutCommand, LayoutEvent};
use crate::model::RiftState;
use crate::model::space_activation::{
    SpaceActivationConfig, SpaceActivationPolicy, ToggleSpaceContext,
};
use crate::sys::screen::SpaceId;
use crate::sys::window_server::WindowServerId;

#[derive(Debug, Clone)]
pub struct LayoutCommandPayload {
    pub command: LayoutCommand,
    pub command_space: Option<SpaceId>,
    pub visible_spaces: Vec<SpaceId>,
    pub visible_space_centers: HashMap<SpaceId, objc2_core_foundation::CGPoint>,
}

pub fn handle_command_layout(
    state: &mut RiftState,
    layout: &mut LayoutManager,
    workspace_switch: &mut WorkspaceSwitchManager,
    payload: LayoutCommandPayload,
) -> anyhow::Result<EventOutcome> {
    let LayoutCommandPayload {
        command: cmd,
        command_space,
        visible_spaces,
        visible_space_centers,
    } = payload;
    info!(?cmd);
    let is_workspace_switch = matches!(
        cmd,
        LayoutCommand::NextWorkspace(_)
            | LayoutCommand::PrevWorkspace(_)
            | LayoutCommand::SwitchToWorkspace(_)
            | LayoutCommand::SwitchToLastWorkspace
    );
    let requires_workspace_space = matches!(
        cmd,
        LayoutCommand::NextWorkspace(_)
            | LayoutCommand::PrevWorkspace(_)
            | LayoutCommand::SwitchToWorkspace(_)
            | LayoutCommand::SetWorkspaceLayout { .. }
            | LayoutCommand::CreateWorkspace
            | LayoutCommand::SwitchToLastWorkspace
    );
    let workspace_space = if requires_workspace_space {
        if let Some(space) = command_space {
            store_current_floating_positions(state, layout, space);
        }
        command_space
    } else {
        None
    };
    if is_workspace_switch {
        workspace_switch.start_workspace_switch(WorkspaceSwitchOrigin::Manual);
    } else {
        workspace_switch.mark_workspace_switch_inactive();
    }

    let response = match &cmd {
        LayoutCommand::NextWorkspace(_)
        | LayoutCommand::PrevWorkspace(_)
        | LayoutCommand::SwitchToWorkspace(_)
        | LayoutCommand::SetWorkspaceLayout { .. }
        | LayoutCommand::CreateWorkspace
        | LayoutCommand::SwitchToLastWorkspace => {
            if let Some(space) = workspace_space {
                layout.layout_engine.handle_virtual_workspace_command(
                    &mut state.windows,
                    space,
                    &cmd,
                )
            } else {
                EventResponse::default()
            }
        }
        LayoutCommand::MoveWindowToWorkspace { .. } => {
            if let Some(space) = command_space {
                layout.layout_engine.handle_virtual_workspace_command(
                    &mut state.windows,
                    space,
                    &cmd,
                )
            } else {
                EventResponse::default()
            }
        }
        _ => {
            if visible_spaces.is_empty() {
                warn!("Layout command ignored: no active spaces");
                return Ok(EventOutcome::finalized_event(None, false, false, false));
            }
            layout.layout_engine.handle_command(
                &mut state.windows,
                command_space,
                &visible_spaces,
                &visible_space_centers,
                cmd,
            )
        }
    };

    Ok(EventOutcome::finalized_event(None, false, false, false)
        .with_layout_response(response, workspace_space))
}

fn current_floating_positions(
    state: &RiftState,
    layout: &LayoutManager,
    space: SpaceId,
) -> Vec<(SpaceId, WindowId, objc2_core_foundation::CGRect)> {
    layout
        .layout_engine
        .windows_in_active_workspace(&state.windows, space)
        .into_iter()
        .filter(|window| layout.layout_engine.is_window_floating(*window))
        .filter_map(|window| {
            state.windows.window(window).map(|state| (space, window, state.frame_monotonic))
        })
        .collect()
}

fn store_current_floating_positions(state: &RiftState, layout: &mut LayoutManager, space: SpaceId) {
    let positions = current_floating_positions(state, layout, space)
        .into_iter()
        .map(|(_, window, frame)| (window, frame))
        .collect::<Vec<_>>();
    if !positions.is_empty() {
        layout.layout_engine.store_floating_window_positions(space, &positions);
    }
}

pub fn handle_command_metrics(cmd: MetricsCommand) -> anyhow::Result<EventOutcome> {
    handle_metrics_command(cmd);
    Ok(EventOutcome::finalized_event(None, false, false, false))
}

pub fn handle_switch_native_space(
    direction: crate::layout_engine::Direction,
) -> anyhow::Result<EventOutcome> {
    Ok(
        EventOutcome::finalized_event(None, false, false, false)
            .with_native_space_switch(direction),
    )
}

pub fn handle_mission_control_command(
    command: crate::actor::wm_controller::WmCmd,
) -> anyhow::Result<EventOutcome> {
    Ok(EventOutcome::finalized_event(None, false, false, false).with_wm_command(command))
}

pub fn handle_close_window(
    window_server_id: Option<WindowServerId>,
) -> anyhow::Result<EventOutcome> {
    Ok(
        EventOutcome::finalized_event(None, false, false, false)
            .with_close_window(window_server_id),
    )
}

pub fn handle_config_updated(
    config: &mut Config,
    layout: &mut LayoutManager,
    state: &RiftState,
    drag: &mut DragManager,
    new_config: Config,
) -> anyhow::Result<EventOutcome> {
    let keys_changed = config.keys != new_config.keys;
    *config = new_config;
    layout.layout_engine.set_layout_settings(&config.settings.layout);

    layout
        .layout_engine
        .update_virtual_workspace_settings(&state.windows, &config.virtual_workspaces);

    drag.update_config(config.settings.window_snapping);

    Ok(EventOutcome::finalized_event(None, false, false, false)
        .with_service_config_update(config.clone(), keys_changed))
}

pub fn handle_command_reactor_debug(
    layout: &LayoutManager,
    topology: &ForwardedSpaceState,
) -> anyhow::Result<EventOutcome> {
    for screen in &topology.screens {
        if let Some(space) = screen.space {
            layout.layout_engine.debug_tree_desc(space, "", true);
        }
    }
    Ok(EventOutcome::finalized_event(None, false, false, false))
}

pub fn handle_command_reactor_serialize(
    serialized: Result<String, serde_json::Error>,
) -> anyhow::Result<EventOutcome> {
    Ok(EventOutcome::finalized_event(None, false, false, false).with_stdout_line(serialized?))
}

pub fn handle_command_reactor_save_and_exit(
    state: &RiftState,
    layout: &mut LayoutManager,
) -> anyhow::Result<EventOutcome> {
    if let Err(e) = save_layout(state, layout, config::restore_file()) {
        error!("Could not save master file: {e}");
        std::process::exit(3);
    }
    std::process::exit(0);
}

fn save_layout(
    state: &RiftState,
    layout: &mut LayoutManager,
    path: std::path::PathBuf,
) -> std::io::Result<()> {
    let floating_positions = layout
        .layout_engine
        .virtual_workspace_manager()
        .initialized_spaces()
        .into_iter()
        .flat_map(|space| current_floating_positions(state, layout, space))
        .collect::<Vec<_>>();
    layout
        .layout_engine
        .save_current_layout(path, &state.windows, &floating_positions)
}

pub fn handle_command_reactor_save_layout(
    state: &RiftState,
    layout: &mut LayoutManager,
    path: std::path::PathBuf,
) -> anyhow::Result<EventOutcome> {
    save_layout(state, layout, path.clone())?;
    info!(path = %path.display(), "Saved layout");
    Ok(EventOutcome::finalized_event(None, false, false, false)
        .with_stdout_line(format!("Saved layout to {}", path.display())))
}

#[derive(Debug, Clone)]
pub struct ToggleSpacePayload {
    pub config: SpaceActivationConfig,
    pub space: Option<SpaceId>,
    pub display_uuid: Option<String>,
}

pub fn handle_command_reactor_toggle_space_activated(
    policy: &mut SpaceActivationPolicy,
    payload: ToggleSpacePayload,
) -> anyhow::Result<EventOutcome> {
    let Some(space) = payload.space else {
        return Ok(EventOutcome::finalized_event(None, false, false, false));
    };
    policy.toggle_space_activated(payload.config, ToggleSpaceContext {
        space,
        display_uuid: payload.display_uuid,
    });
    Ok(EventOutcome::finalized_event(None, false, false, false).with_active_space_recompute())
}

#[derive(Debug, Clone, Copy)]
pub struct FocusWindowPayload {
    pub window_id: WindowId,
    pub window_server_id: Option<WindowServerId>,
    pub resolved_space: Option<SpaceId>,
    pub space_is_active: bool,
}

#[derive(Debug, Clone)]
pub struct DisplayFocusPayload {
    pub screen: Option<ScreenInfo>,
    pub target_is_active: bool,
    pub focus_window: Option<WindowId>,
    /// Center of `focus_window`'s frame, used to warp the cursor onto the
    /// window that receives focus. Falls back to the screen center.
    pub focus_window_center: Option<objc2_core_foundation::CGPoint>,
}

/// Build a raise request that moves real (window server) focus to `window`,
/// mirroring what `handle_command_reactor_focus_window` emits. Without this,
/// display-focus commands only update the layout engine's internal selection
/// and macOS keyboard focus stays on the previous display.
fn display_focus_raise_request(apps: &AppManager, window: WindowId) -> raise_manager::Event {
    let mut app_handles: HashMap<i32, AppThreadHandle> = HashMap::default();
    if let Some(app) = apps.apps.get(&window.pid) {
        app_handles.insert(window.pid, app.handle.clone());
    }
    raise_manager::Event::RaiseRequest(raise_manager::RaiseRequest {
        raise_windows: Vec::new(),
        focus_window: Some((window, None)),
        app_handles,
        focus_quiet: Quiet::No,
    })
}

pub fn handle_move_mouse_to_display(
    apps: &AppManager,
    payload: DisplayFocusPayload,
) -> anyhow::Result<EventOutcome> {
    let Some(screen) = payload.screen else {
        return Ok(EventOutcome::finalized_event(None, false, false, false));
    };
    if !payload.target_is_active {
        warn!(?screen.space, "Move mouse ignored: target display space is inactive");
        return Ok(EventOutcome::finalized_event(None, false, false, false));
    }
    let mut outcome = EventOutcome::finalized_event(None, false, false, false)
        .with_mouse_warp(screen.frame.mid());
    if let (Some(space), Some(window)) = (screen.space, payload.focus_window) {
        outcome = outcome
            .with_layout_event(LayoutEvent::WindowFocused(space, window))
            .with_raise_request(display_focus_raise_request(apps, window));
    }
    Ok(outcome)
}

pub fn handle_focus_display(
    apps: &AppManager,
    payload: DisplayFocusPayload,
) -> anyhow::Result<EventOutcome> {
    let Some(screen) = payload.screen else {
        return Ok(EventOutcome::finalized_event(None, false, false, false));
    };
    if !payload.target_is_active {
        warn!(?screen.space, "Focus display ignored: target display space is inactive");
        return Ok(EventOutcome::finalized_event(None, false, false, false));
    }
    if let (Some(space), Some(window)) = (screen.space, payload.focus_window) {
        return Ok(EventOutcome::finalized_event(None, false, false, false)
            .with_layout_event(LayoutEvent::WindowFocused(space, window))
            .with_raise_request(display_focus_raise_request(apps, window))
            .with_mouse_warp(payload.focus_window_center.unwrap_or_else(|| screen.frame.mid())));
    }
    Ok(
        EventOutcome::finalized_event(None, false, false, false)
            .with_mouse_warp(screen.frame.mid()),
    )
}

pub fn handle_command_reactor_focus_window(
    state: &RiftState,
    apps: &AppManager,
    payload: FocusWindowPayload,
) -> anyhow::Result<EventOutcome> {
    let FocusWindowPayload {
        window_id,
        window_server_id,
        resolved_space,
        space_is_active,
    } = payload;
    let mut outcome = EventOutcome::finalized_event(None, false, false, false);
    if state.windows.window(window_id).is_some() {
        let Some(space) = resolved_space else {
            warn!(?window_id, "Focus window ignored: space unknown");
            return Ok(outcome);
        };
        if !space_is_active {
            warn!(?window_id, ?space, "Focus window ignored: space is inactive");
            return Ok(outcome);
        }
        outcome = outcome.with_layout_event(LayoutEvent::WindowFocused(space, window_id));

        let mut app_handles: HashMap<i32, AppThreadHandle> = HashMap::default();
        if let Some(app) = apps.apps.get(&window_id.pid) {
            app_handles.insert(window_id.pid, app.handle.clone());
        }
        let request = raise_manager::Event::RaiseRequest(raise_manager::RaiseRequest {
            raise_windows: Vec::new(),
            focus_window: Some((window_id, None)),
            app_handles,
            focus_quiet: Quiet::No,
        });
        outcome = outcome.with_raise_request(request);
    } else if let Some(wsid) = window_server_id {
        outcome = outcome.with_make_key_window(window_id.pid, wsid);
    }
    Ok(outcome)
}

#[derive(Debug, Clone, Copy)]
pub struct MoveWindowToDisplayPayload {
    pub window: WindowId,
    pub window_server_id: Option<WindowServerId>,
    pub source_space: SpaceId,
    pub target_space: SpaceId,
    pub target_screen: objc2_core_foundation::CGRect,
    pub target_frame: objc2_core_foundation::CGRect,
}

pub fn handle_command_reactor_move_window_to_display(
    state: &mut RiftState,
    layout: &mut LayoutManager,
    payload: MoveWindowToDisplayPayload,
) -> anyhow::Result<EventOutcome> {
    if let Some(window) = state.windows.window_mut(payload.window) {
        window.frame_monotonic = payload.target_frame;
    } else {
        warn!(window = ?payload.window, "Move window to display ignored: unknown window");
        return Ok(EventOutcome::finalized_event(None, false, false, false));
    }

    let response = layout.layout_engine.move_window_to_space(
        &mut state.windows,
        payload.source_space,
        payload.target_space,
        payload.target_screen.size,
        payload.window,
    );

    if state
        .windows
        .workspace_for_window(payload.target_space, payload.window)
        .is_some()
        && let Some(window_server_id) = payload.window_server_id
    {
        state
            .windows
            .set_window_server_space(window_server_id, Some(payload.target_space));
        state.windows.mark_window_visible(window_server_id);
    }

    Ok(EventOutcome::finalized_event(None, false, false, false)
        .with_layout_response(response, None)
        .with_pre_layout_window_frame_write(payload.window, payload.target_frame, true))
}
