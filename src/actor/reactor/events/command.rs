use tracing::{error, info, warn};

use super::super::Screen;
use crate::actor::app::{AppThreadHandle, Quiet, WindowId};
use crate::actor::reactor::transaction_manager::TransactionId;
use crate::actor::reactor::{DisplaySelector, Reactor, WorkspaceSwitchOrigin};
use crate::actor::stack_line::Event as StackLineEvent;
use crate::actor::wm_controller::WmEvent;
use crate::actor::{menu_bar, raise_manager};
use crate::common::collections::HashMap;
use crate::common::config::{self as config, Config};
use crate::common::log::{MetricsCommand, handle_command};
use crate::layout_engine::{EventResponse, LayoutCommand, LayoutEvent};
use crate::sys::screen::{SpaceId, order_visible_spaces_by_position};
use crate::sys::window_server::{self as window_server, WindowServerId};

pub struct CommandEventHandler;

impl CommandEventHandler {
    pub fn handle_command_layout(reactor: &mut Reactor, cmd: LayoutCommand) {
        info!(?cmd);
        let visible_spaces_input: Vec<(SpaceId, _)> = reactor
            .space_manager
            .screens
            .iter()
            .filter_map(|screen| {
                let space = screen.space?;
                if !reactor.is_space_active(space) {
                    return None;
                }
                let center = screen.frame.mid();
                Some((space, center))
            })
            .collect();

        if visible_spaces_input.is_empty() {
            warn!("Layout command ignored: no active spaces");
            return;
        }

        let mut visible_space_centers = HashMap::default();
        for (space, center) in &visible_spaces_input {
            visible_space_centers.insert(*space, *center);
        }

        let visible_spaces = order_visible_spaces_by_position(visible_spaces_input.iter().cloned());

        let is_workspace_switch = matches!(
            cmd,
            LayoutCommand::NextWorkspace(_)
                | LayoutCommand::PrevWorkspace(_)
                | LayoutCommand::SwitchToWorkspace(_)
                | LayoutCommand::SwitchToLastWorkspace
        );
        let workspace_space = if is_workspace_switch {
            let space = reactor.workspace_command_space();
            if let Some(space) = space {
                reactor.store_current_floating_positions(space);
            }
            space
        } else {
            None
        };
        if is_workspace_switch {
            reactor
                .workspace_switch_manager
                .start_workspace_switch(WorkspaceSwitchOrigin::Manual);
        } else {
            reactor.workspace_switch_manager.mark_workspace_switch_inactive();
        }

        let response = match &cmd {
            LayoutCommand::NextWorkspace(_)
            | LayoutCommand::PrevWorkspace(_)
            | LayoutCommand::SwitchToWorkspace(_)
            | LayoutCommand::CreateWorkspace
            | LayoutCommand::SwitchToLastWorkspace => {
                if let Some(space) = workspace_space {
                    reactor
                        .layout_manager
                        .layout_engine
                        .handle_virtual_workspace_command(space, &cmd)
                } else {
                    EventResponse::default()
                }
            }
            LayoutCommand::MoveWindowToWorkspace { .. } => {
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
                &visible_space_centers,
                cmd,
            ),
        };

        reactor.handle_layout_response(response, workspace_space);
    }

    pub fn handle_command_metrics(_reactor: &mut Reactor, cmd: MetricsCommand) {
        handle_command(cmd);
    }

    pub fn handle_config_updated(reactor: &mut Reactor, new_cfg: Config) {
        let old_keys = reactor.config.keys.clone();

        reactor.config = new_cfg;
        reactor
            .layout_manager
            .layout_engine
            .set_layout_settings(&reactor.config.settings.layout);

        reactor
            .layout_manager
            .layout_engine
            .update_virtual_workspace_settings(&reactor.config.virtual_workspaces);

        reactor.drag_manager.update_config(reactor.config.settings.window_snapping);

        if let Some(tx) = &reactor.communication_manager.stack_line_tx {
            if let Err(e) = tx.try_send(StackLineEvent::ConfigUpdated(reactor.config.clone())) {
                warn!("Failed to send config update to stack line: {}", e);
            }
        }

        if let Some(tx) = &reactor.menu_manager.menu_tx {
            if let Err(e) = tx.try_send(menu_bar::Event::ConfigUpdated(reactor.config.clone())) {
                warn!("Failed to send config update to menu bar: {}", e);
            }
        }

        let _ = reactor.update_layout_or_warn(false, true);

        if old_keys != reactor.config.keys {
            if let Some(wm) = &reactor.communication_manager.wm_sender {
                wm.send(WmEvent::ConfigUpdated(reactor.config.clone()));
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
            let Some(space) = reactor.window_manager.windows.get(&window_id).and_then(|w| {
                reactor.best_space_for_window(&w.frame_monotonic, w.window_server_id)
            }) else {
                warn!(?window_id, "Focus window ignored: space unknown");
                return;
            };
            if !reactor.is_space_active(space) {
                warn!(?window_id, ?space, "Focus window ignored: space is inactive");
                return;
            }
            reactor.send_layout_event(LayoutEvent::WindowFocused(space, window_id));

            let mut app_handles: HashMap<i32, AppThreadHandle> = HashMap::default();
            if let Some(app) = reactor.app_manager.apps.get(&window_id.pid) {
                app_handles.insert(window_id.pid, app.handle.clone());
            }
            let request = raise_manager::Event::RaiseRequest(raise_manager::RaiseRequest {
                raise_windows: Vec::new(),
                focus_window: Some((window_id, None)),
                app_handles,
                focus_quiet: Quiet::No,
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

    fn focus_first_window_on_screen(reactor: &mut Reactor, screen: &Screen) -> bool {
        if let Some(space) = screen.space {
            if !reactor.is_space_active(space) {
                return false;
            }
            let focus_target = reactor.last_focused_window_in_space(space).or_else(|| {
                reactor
                    .layout_manager
                    .layout_engine
                    .windows_in_active_workspace(space)
                    .into_iter()
                    .next()
            });
            if let Some(window_id) = focus_target {
                reactor.send_layout_event(LayoutEvent::WindowFocused(space, window_id));
                return true;
            }
        }
        false
    }

    pub fn handle_command_reactor_move_mouse_to_display(
        reactor: &mut Reactor,
        selector: &DisplaySelector,
    ) {
        let target_screen = reactor.screen_for_selector(selector, None).cloned();

        if let Some(screen) = target_screen {
            if let Some(space) = screen.space {
                if !reactor.is_space_active(space) {
                    warn!(
                        ?selector,
                        ?space,
                        "Move mouse ignored: target display space is inactive"
                    );
                    return;
                }
            }
            let center = screen.frame.mid();
            if let Some(event_tap_tx) = reactor.communication_manager.event_tap_tx.as_ref() {
                event_tap_tx.send(crate::actor::event_tap::Request::Warp(center));
            }
            let _ = Self::focus_first_window_on_screen(reactor, &screen);
        }
    }

    pub fn handle_command_reactor_focus_display(reactor: &mut Reactor, selector: &DisplaySelector) {
        let screen = match reactor.screen_for_selector(selector, None).cloned() {
            Some(s) => s,
            None => return,
        };
        if let Some(space) = screen.space {
            if !reactor.is_space_active(space) {
                warn!(
                    ?selector,
                    ?space,
                    "Focus display ignored: target display space is inactive"
                );
                return;
            }
        }

        if Self::focus_first_window_on_screen(reactor, &screen) {
            return;
        }

        if let Some(event_tap_tx) = reactor.communication_manager.event_tap_tx.as_ref() {
            event_tap_tx.send(crate::actor::event_tap::Request::Warp(screen.frame.mid()));
        }
    }

    pub fn handle_command_reactor_move_window_to_display(
        reactor: &mut Reactor,
        selector: &DisplaySelector,
        window_idx: Option<u32>,
    ) {
        if reactor.is_in_drag() {
            warn!("Ignoring move-window-to-display while a drag is active");
            return;
        }

        let resolved_window = {
            let vwm = reactor.layout_manager.layout_engine.virtual_workspace_manager();
            match window_idx {
                Some(idx) => {
                    if let Some(space) = reactor.workspace_command_space() {
                        vwm.find_window_by_idx(space, idx).or_else(|| {
                            reactor
                                .iter_active_spaces()
                                .find_map(|sp| vwm.find_window_by_idx(sp, idx))
                        })
                    } else {
                        reactor.iter_active_spaces().find_map(|sp| vwm.find_window_by_idx(sp, idx))
                    }
                }
                None => reactor.main_window().or_else(|| reactor.window_id_under_cursor()).or_else(
                    || {
                        reactor
                            .workspace_command_space()
                            .and_then(|space| vwm.find_window_by_idx(space, 0))
                    },
                ),
            }
        };

        let Some(window_id) = resolved_window else {
            warn!("Move window to display ignored because no target window was resolved");
            return;
        };

        let (window_server_id, window_frame) = match reactor.window_manager.windows.get(&window_id)
        {
            Some(state) => (state.window_server_id, state.frame_monotonic),
            None => {
                warn!(?window_id, "Move window to display ignored: unknown window");
                return;
            }
        };

        let Some(source_space) = reactor.best_space_for_window(&window_frame, window_server_id)
        else {
            warn!(
                ?window_id,
                "Move window to display ignored: source space unknown"
            );
            return;
        };
        if !reactor.is_space_active(source_space) {
            warn!(
                ?window_id,
                ?source_space,
                "Move window to display ignored: source space is inactive"
            );
            return;
        }

        let origin_screen = reactor.space_manager.screen_by_space(source_space);

        let origin_point =
            origin_screen.map(|s| s.frame.mid()).or_else(|| reactor.current_screen_center());
        let target_screen = reactor.screen_for_selector(selector, origin_point).cloned();

        let Some(target_screen) = target_screen else {
            warn!(
                ?selector,
                "Move window to display ignored: target display not found"
            );
            return;
        };
        if let Some(space) = target_screen.space {
            if !reactor.is_space_active(space) {
                warn!(
                    ?selector,
                    ?space,
                    "Move window to display ignored: target display space is inactive"
                );
                return;
            }
        }

        let Some(target_space) = target_screen.space else {
            warn!(
                uuid = ?target_screen.display_uuid,
                "Move window to display ignored: display has no active space"
            );
            return;
        };

        if target_space == source_space {
            return;
        }

        let mut target_frame = window_frame;
        let size = window_frame.size;
        let dest_rect = target_screen.frame;
        let mut origin = dest_rect.mid();
        origin.x -= size.width / 2.0;
        origin.y -= size.height / 2.0;
        let min = dest_rect.min();
        let max = dest_rect.max();
        origin.x = origin.x.max(min.x).min(max.x - size.width);
        origin.y = origin.y.max(min.y).min(max.y - size.height);
        target_frame.origin = origin;

        if let Some(app) = reactor.app_manager.apps.get(&window_id.pid) {
            if let Some(wsid) = window_server_id {
                let txid = reactor.transaction_manager.generate_next_txid(wsid);
                reactor.transaction_manager.set_last_sent_txid(wsid, txid);
                let _ = app.handle.send(crate::actor::app::Request::SetWindowFrame(
                    window_id,
                    target_frame,
                    txid,
                    true,
                ));
            } else {
                let txid = TransactionId::default();
                let _ = app.handle.send(crate::actor::app::Request::SetWindowFrame(
                    window_id,
                    target_frame,
                    txid,
                    true,
                ));
            }
        }

        if let Some(state) = reactor.window_manager.windows.get_mut(&window_id) {
            state.frame_monotonic = target_frame;
        }

        let response = reactor.layout_manager.layout_engine.move_window_to_space(
            source_space,
            target_space,
            target_screen.frame.size,
            window_id,
        );

        reactor.handle_layout_response(response, None);

        let _ = reactor.update_layout_or_warn(false, false);
    }

    pub fn handle_command_reactor_close_window(
        reactor: &mut Reactor,
        window_server_id: Option<WindowServerId>,
    ) {
        let target = window_server_id
            .and_then(|wsid| reactor.window_manager.window_ids.get(&wsid).copied())
            .or_else(|| reactor.main_window());
        if let Some(wid) = target {
            reactor.request_close_window(wid);
        } else {
            warn!("Close window command ignored because no window is tracked");
        }
    }
}
