use tracing::{error, info};

use crate::actor::app::{AppThreadHandle, WindowId};
use crate::actor::raise_manager;
use crate::actor::reactor::{Reactor, WorkspaceSwitchState};
use crate::actor::stack_line::Event as StackLineEvent;
use crate::actor::wm_controller::WmEvent;
use crate::common::collections::HashMap;
use crate::common::config::{self as config, Config};
use crate::common::log::{MetricsCommand, handle_command};
use crate::layout_engine::{EventResponse, LayoutCommand, LayoutEvent};
use crate::sys::window_server::{self as window_server, WindowServerId};

pub struct CommandEventHandler;

impl CommandEventHandler {
    pub fn handle_command_layout(reactor: &mut Reactor, cmd: LayoutCommand) {
        info!(?cmd);
        let visible_spaces =
            reactor.screens.iter().flat_map(|screen| screen.space).collect::<Vec<_>>();

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
            reactor.workspace_switch_generation =
                reactor.workspace_switch_generation.wrapping_add(1);
            reactor.active_workspace_switch = Some(reactor.workspace_switch_generation);
        }

        let response = match &cmd {
            LayoutCommand::NextWorkspace(_)
            | LayoutCommand::PrevWorkspace(_)
            | LayoutCommand::SwitchToWorkspace(_)
            | LayoutCommand::MoveWindowToWorkspace(_)
            | LayoutCommand::CreateWorkspace
            | LayoutCommand::SwitchToLastWorkspace => {
                if let Some(space) = reactor.workspace_command_space() {
                    reactor.layout_engine.handle_virtual_workspace_command(space, &cmd)
                } else {
                    EventResponse::default()
                }
            }
            _ => reactor.layout_engine.handle_command(
                reactor.workspace_command_space(),
                &visible_spaces,
                cmd,
            ),
        };

        reactor.workspace_switch_state = if is_workspace_switch {
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
        let old_keys = reactor.config.keys.clone();

        reactor.config = new_cfg;
        reactor.layout_engine.set_layout_settings(&reactor.config.settings.layout);
        let _ = reactor.drag_manager.update_config(reactor.config.settings.window_snapping);

        if let Some(tx) = &reactor.stack_line_tx {
            let _ = tx.try_send(StackLineEvent::ConfigUpdated(reactor.config.clone()));
        }

        let _ = reactor.update_layout(false, true);

        if old_keys != reactor.config.keys {
            if let Some(wm) = &reactor.wm_sender {
                let _ = wm.send(WmEvent::ConfigUpdated(reactor.config.clone()));
            }
        }
    }

    pub fn handle_command_reactor_debug(reactor: &mut Reactor) {
        for screen in &reactor.screens {
            if let Some(space) = screen.space {
                reactor.layout_engine.debug_tree_desc(space, "", true);
            }
        }
    }

    pub fn handle_command_reactor_serialize(reactor: &mut Reactor) {
        if let Ok(state) = reactor.serialize_state() {
            println!("{}", state);
        }
    }

    pub fn handle_command_reactor_save_and_exit(reactor: &mut Reactor) {
        match reactor.layout_engine.save(config::restore_file()) {
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
        if reactor.windows.contains_key(&window_id) {
            if let Some(space) = reactor
                .windows
                .get(&window_id)
                .and_then(|w| reactor.best_space_for_window(&w.frame_monotonic))
            {
                reactor.send_layout_event(LayoutEvent::WindowFocused(space, window_id));
            }

            let mut app_handles: HashMap<i32, AppThreadHandle> = HashMap::default();
            if let Some(app) = reactor.apps.get(&window_id.pid) {
                app_handles.insert(window_id.pid, app.handle.clone());
            }
            let request = raise_manager::Event::RaiseRequest(raise_manager::RaiseRequest {
                raise_windows: Vec::new(),
                focus_window: Some((window_id, None)),
                app_handles,
            });
            let _ = reactor.raise_manager_tx.try_send(request);
        } else if let Some(wsid) = window_server_id {
            let _ = window_server::make_key_window(window_id.pid, wsid);
        }
    }

    pub fn handle_command_reactor_set_mission_control_active(reactor: &mut Reactor, active: bool) {
        reactor.set_mission_control_active(active);
    }
}
