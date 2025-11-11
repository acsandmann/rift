use tracing::{error, info, warn};

use crate::actor::app::{AppThreadHandle, WindowId};
use crate::actor::raise_manager;
use crate::actor::reactor::{Reactor, WorkspaceSwitchState};
use crate::actor::stack_line::Event as StackLineEvent;
use crate::actor::wm_controller::WmEvent;
use crate::common::collections::HashMap;
use crate::common::config::{self as config, Config};
use crate::common::log::{MetricsCommand, handle_command};
use crate::layout_engine::{EventResponse, LayoutCommand, LayoutEvent};
use crate::sys::screen::order_visible_spaces_by_position;
use crate::sys::window_server::{self as window_server, WindowServerId};

pub struct CommandEventHandler;

impl CommandEventHandler {
    pub fn handle_command_layout(reactor: &mut Reactor, cmd: LayoutCommand) {
        info!(?cmd);
        let visible_spaces = order_visible_spaces_by_position(
            reactor.space_manager.screens.iter().filter_map(|screen| {
                let space = screen.space?;
                let center = screen.frame.mid();
                Some((space, center))
            }),
        );

        let is_workspace_switch = matches!(
            cmd,
            LayoutCommand::NextWorkspace(_)
                | LayoutCommand::PrevWorkspace(_)
                | LayoutCommand::SwitchToWorkspace(_)
                | LayoutCommand::SwitchToLastWorkspace
        );
        if is_workspace_switch {
            if let Some(space) = reactor.workspace_command_space() {
                reactor.store_current_floating_positions(space);
            }
            reactor.workspace_switch_manager.workspace_switch_generation =
                reactor.workspace_switch_manager.workspace_switch_generation.wrapping_add(1);
            reactor.workspace_switch_manager.active_workspace_switch =
                Some(reactor.workspace_switch_manager.workspace_switch_generation);
        }

        let response = match &cmd {
            LayoutCommand::NextWorkspace(_)
            | LayoutCommand::PrevWorkspace(_)
            | LayoutCommand::SwitchToWorkspace(_)
            | LayoutCommand::MoveWindowToWorkspace { .. }
            | LayoutCommand::CreateWorkspace
            | LayoutCommand::SwitchToLastWorkspace => {
                if let Some(space) = reactor.workspace_command_space() {
                    reactor
                        .layout_manager
                        .layout_engine
                        .handle_virtual_workspace_command(space, &cmd)
                } else {
                    EventResponse::default()
                }
            }
            _ => reactor.layout_manager.layout_engine.handle_command(
                reactor.workspace_command_space(),
                &visible_spaces,
                cmd,
            ),
        };

        reactor.workspace_switch_manager.workspace_switch_state = if is_workspace_switch {
            WorkspaceSwitchState::Active
        } else {
            WorkspaceSwitchState::Inactive
        };
        reactor.handle_layout_response(response);
    }

    pub fn handle_command_metrics(_reactor: &mut Reactor, cmd: MetricsCommand) {
        handle_command(cmd);
    }

    pub fn handle_config_updated(reactor: &mut Reactor, new_cfg: Config) {
        let old_keys = reactor.config_manager.config.keys.clone();

        reactor.config_manager.config = new_cfg;
        reactor
            .layout_manager
            .layout_engine
            .set_layout_settings(&reactor.config_manager.config.settings.layout);

        reactor
            .layout_manager
            .layout_engine
            .update_virtual_workspace_settings(&reactor.config_manager.config.virtual_workspaces);

        reactor
            .drag_manager
            .update_config(reactor.config_manager.config.settings.window_snapping);

        if let Some(tx) = &reactor.communication_manager.stack_line_tx {
            if let Err(e) = tx.try_send(StackLineEvent::ConfigUpdated(
                reactor.config_manager.config.clone(),
            )) {
                warn!("Failed to send config update to stack line: {}", e);
            }
        }

        let _ = reactor.update_layout(false, true).unwrap_or_else(|e| {
            warn!("Layout update failed: {}", e);
            false
        });

        if old_keys != reactor.config_manager.config.keys {
            if let Some(wm) = &reactor.communication_manager.wm_sender {
                wm.send(WmEvent::ConfigUpdated(reactor.config_manager.config.clone()));
            }
        }
    }

    pub fn handle_command_reactor_debug(reactor: &mut Reactor) {
        for screen in &reactor.space_manager.screens {
            if let Some(space) = screen.space {
                reactor.layout_manager.layout_engine.debug_tree_desc(space, "", true);
            }
        }
    }

    pub fn handle_command_reactor_serialize(reactor: &mut Reactor) {
        if let Ok(state) = reactor.serialize_state() {
            println!("{}", state);
        }
    }

    pub fn handle_command_reactor_save_and_exit(reactor: &mut Reactor) {
        match reactor.layout_manager.layout_engine.save(config::restore_file()) {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                error!("Could not save layout: {e}");
                std::process::exit(3);
            }
        }
    }

    pub fn handle_command_reactor_switch_space(
        _reactor: &mut Reactor,
        dir: crate::layout_engine::Direction,
    ) {
        unsafe { window_server::switch_space(dir) }
    }

    pub fn handle_command_reactor_focus_window(
        reactor: &mut Reactor,
        window_id: WindowId,
        window_server_id: Option<WindowServerId>,
    ) {
        if reactor.window_manager.windows.contains_key(&window_id) {
            if let Some(space) =
                reactor.window_manager.windows.get(&window_id).and_then(|w| {
                    reactor.best_space_for_window(&w.frame_monotonic, w.window_server_id)
                })
            {
                reactor.send_layout_event(LayoutEvent::WindowFocused(space, window_id));
            }

            let mut app_handles: HashMap<i32, AppThreadHandle> = HashMap::default();
            if let Some(app) = reactor.app_manager.apps.get(&window_id.pid) {
                app_handles.insert(window_id.pid, app.handle.clone());
            }
            let request = raise_manager::Event::RaiseRequest(raise_manager::RaiseRequest {
                raise_windows: Vec::new(),
                focus_window: Some((window_id, None)),
                app_handles,
            });
            if let Err(e) = reactor.communication_manager.raise_manager_tx.try_send(request) {
                warn!("Failed to send raise request: {}", e);
            }
        } else if let Some(wsid) = window_server_id {
            if let Err(e) = window_server::make_key_window(window_id.pid, wsid) {
                warn!("Failed to make key window: {:?}", e);
            }
        }
    }

    pub fn handle_command_reactor_show_mission_control_all(reactor: &mut Reactor) {
        if let Some(wm) = reactor.communication_manager.wm_sender.as_ref() {
            let _ = wm.send(crate::actor::wm_controller::WmEvent::Command(
                crate::actor::wm_controller::WmCommand::Wm(
                    crate::actor::wm_controller::WmCmd::ShowMissionControlAll,
                ),
            ));
        }
    }

    pub fn handle_command_reactor_show_mission_control_current(reactor: &mut Reactor) {
        if let Some(wm) = reactor.communication_manager.wm_sender.as_ref() {
            let _ = wm.send(crate::actor::wm_controller::WmEvent::Command(
                crate::actor::wm_controller::WmCommand::Wm(
                    crate::actor::wm_controller::WmCmd::ShowMissionControlCurrent,
                ),
            ));
        }
    }

    pub fn handle_command_reactor_dismiss_mission_control(reactor: &mut Reactor) {
        if let Some(wm) = reactor.communication_manager.wm_sender.as_ref() {
            let _ = wm.send(crate::actor::wm_controller::WmEvent::Command(
                crate::actor::wm_controller::WmCommand::Wm(
                    crate::actor::wm_controller::WmCmd::ShowMissionControlAll,
                ),
            ));
        } else {
            reactor.set_mission_control_active(false);
        }
    }
}
