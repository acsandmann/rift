use tracing::{info, warn};

use super::super::*;
use crate::actor::wm_controller::{WmCmd, WmCommand};
use crate::common::config::LayoutMode;
use crate::layout_engine::LayoutCommand;
use crate::model::space_activation::ToggleSpaceContext;

impl Reactor {
    pub(in crate::actor::reactor) fn dispatch_layout_command(
        &mut self,
        changes: &mut Changes,
        cmd: LayoutCommand,
        command_space: Option<SpaceId>,
        swipe: bool,
    ) -> bool {
        let single;
        let multiple;
        let visible_space_frames;
        let visible_spaces = if self.active_spaces.len() <= 1 {
            single = self
                .iter_active_spaces()
                .next()
                .filter(|&space| self.space_state.screen_by_space(space).is_some());
            visible_space_frames = HashMap::default();
            single.as_slice()
        } else {
            (multiple, visible_space_frames) = self.visible_spaces_for_layout();
            &multiple
        };
        let is_move_node = matches!(cmd, LayoutCommand::MoveNode(_));
        let is_resize = matches!(
            cmd,
            LayoutCommand::ResizeWindowGrow(_)
                | LayoutCommand::ResizeWindowShrink(_)
                | LayoutCommand::ResizeWindowBy { .. }
                | LayoutCommand::SwitchPresetColumnWidth { .. }
        );
        let post_arrange_mouse_warp = (is_move_node && self.config.settings.mouse_follows_focus)
            .then(|| self.main_window())
            .flatten();
        let edit_space = match cmd {
            LayoutCommand::MoveStackTab { window, .. } => {
                self.assigned_space_for_window_id(WindowId::new(window.pid, window.idx))
            }
            LayoutCommand::ToggleWindowFloating
            | LayoutCommand::ToggleWindowFloatingWithOptions(_)
            | LayoutCommand::ToggleFullscreen
            | LayoutCommand::ToggleFullscreenWithinGaps => self
                .layout_manager
                .layout_engine
                .focused_window()
                .and_then(|window| self.assigned_space_for_window_id(window))
                .or(command_space),
            _ => command_space,
        };
        let workspace_layout = matches!(cmd, LayoutCommand::SetWorkspaceLayout { .. });
        let workspace_create = matches!(cmd, LayoutCommand::CreateWorkspace);
        info!(?cmd);
        let moves_workspace_membership = matches!(cmd, LayoutCommand::MoveWindowToWorkspace { .. });
        let is_selection_command = matches!(cmd, LayoutCommand::Ascend | LayoutCommand::Descend);
        let is_focus_command = matches!(
            cmd,
            LayoutCommand::NextWindow
                | LayoutCommand::PrevWindow
                | LayoutCommand::MoveFocus(_)
                | LayoutCommand::ToggleFocusFloating
        );
        let (is_virtual_workspace_command, is_workspace_switch, requires_workspace_space) =
            match cmd {
                LayoutCommand::NextWorkspace(_)
                | LayoutCommand::PrevWorkspace(_)
                | LayoutCommand::SwitchToWorkspace(_)
                | LayoutCommand::SwitchToLastWorkspace => (true, true, true),
                LayoutCommand::MoveWindowToWorkspace { follow, .. } => (true, follow, follow),
                LayoutCommand::SetWorkspaceLayout { .. } | LayoutCommand::CreateWorkspace => {
                    (true, false, true)
                }
                _ => (false, false, false),
            };
        if matches!(
            cmd,
            LayoutCommand::JoinWindow(_)
                | LayoutCommand::ToggleStack
                | LayoutCommand::ConsumeOrExpelWindow(_)
        ) && let Some(space) = command_space
            && self.layout_manager.layout_engine.active_layout_mode_at(space)
                == LayoutMode::Floating
        {
            self.store_current_floating_positions(space);
        }
        let workspace_space = if requires_workspace_space {
            if let Some(space) = command_space {
                self.store_current_floating_positions(space);
            }
            command_space
        } else {
            None
        };
        if !is_workspace_switch {
            self.workspace_switch_manager.mark_workspace_switch_inactive();
        }

        let response = if is_virtual_workspace_command {
            command_space
                .map(|space| {
                    self.layout_manager.layout_engine.handle_virtual_workspace_command(
                        &mut self.state.windows,
                        crate::model::virtual_workspace::WorkspaceContext {
                            space,
                            visible: visible_spaces,
                        },
                        &cmd,
                    )
                })
                .unwrap_or_default()
        } else {
            if visible_spaces.is_empty() {
                warn!("Layout command ignored: no active spaces");
                return false;
            }
            self.layout_manager.layout_engine.handle_command(
                &mut self.state.windows,
                command_space,
                visible_spaces,
                &visible_space_frames,
                cmd,
            )
        };

        if is_virtual_workspace_command && !response.changed && response.workspace_space.is_none() {
            if is_workspace_switch {
                self.workspace_switch_manager.mark_workspace_switch_inactive();
                self.workspace_switch_manager.active_workspace_switch = None;
            }
            return false;
        }

        // Only begin the switch lifecycle once a real transition has been produced.
        let destination = response.workspace_space.or(workspace_space);
        if is_workspace_switch
            && response.changed
            && let Some(space) = destination
        {
            self.workspace_switch_manager
                .start_workspace_switch(space, WorkspaceSwitchOrigin::Manual);
        }

        let transitioned = response.changed
            || response.workspace_space.is_some_and(|space| Some(space) != command_space);
        if swipe && transitioned {
            // Retire the old viewport before the workspace switch parks its windows.
            self.finish_gesture(changes, None, true);
        }
        let source = response.layout_space.or(edit_space);
        let target = response.workspace_space.or(response.layout_space).or(edit_space);
        let policy = if is_workspace_switch && response.changed {
            Presentation::WorkspaceSwitch
        } else if is_resize
            && target.is_some_and(|space| {
                self.layout_manager.layout_engine.active_layout_mode_at(space)
                    == LayoutMode::Scrolling
            })
        {
            // The viewport applies sizes up front. Place neighbors on the same
            // sample so they don't leave a gap while easing to their new positions.
            Presentation::Resize
        } else {
            Presentation::Animated
        };
        let focus_space = response
            .focus_window
            .and_then(|window| self.assigned_space_for_window_id(window));
        if is_focus_command {
            if let Some(space) = focus_space.or(edit_space)
                && response.changed
            {
                if self.layout_manager.layout_engine.focus_changes_geometry(space) {
                    changes.arrange(Some(space), Presentation::Animated).layout_changed = true;
                } else {
                    changes.space(space).tabs_changed = true;
                }
            }
        } else if response.changed || (!is_selection_command && !is_virtual_workspace_command) {
            changes.arrange(target, policy);
        }
        if response.changed
            && let Some(space) = target
        {
            if !is_focus_command && !is_selection_command {
                changes.space(space).layout_changed = true;
            }
            if workspace_layout || is_workspace_switch {
                changes.workspace_changed(space);
            } else if workspace_create {
                changes.space(space).workspace_changed = true;
            }
        }
        for &window in &response.transferred_windows {
            if let Some(target) = self.assigned_space_for_window_id(window) {
                if let Some(source) = source {
                    changes.move_between(source, target, policy);
                }
                let source_frame =
                    source.and_then(|space| visible_space_frames.get(&space).copied());
                self.request_window_transfer(changes, window, target, source_frame);
            }
        }
        if moves_workspace_membership && response.changed {
            changes.membership_for(source);
        }
        if let Some(space) = destination
            && is_workspace_switch
            && command_space != Some(space)
            && visible_space_frames.contains_key(&space)
        {
            changes.display_warp = Some(space);
        }
        if is_selection_command && response.changed {
            changes.selection_changed = edit_space;
            changes.overview_changed = true;
        }
        let fallback = (is_workspace_switch && !moves_workspace_membership)
            .then_some(destination)
            .flatten();
        self.reconcile_layout_response(changes, response, fallback);
        if is_move_node {
            changes.post_arrange_mouse_warp = post_arrange_mouse_warp;
            self.follow_focused_window_to_its_display(command_space);
        }
        transitioned
    }

    pub(in crate::actor::reactor) fn apply_config(
        &mut self,
        changes: &mut Changes,
        new_config: Config,
    ) -> anyhow::Result<()> {
        let issues = new_config.validate();
        anyhow::ensure!(
            issues.is_empty(),
            "configuration reload rejected: {}",
            issues.join("; ")
        );
        let migration = self
            .layout_manager
            .layout_engine
            .workspaces()
            .plan_settings(&new_config.virtual_workspaces)
            .map_err(|error| {
                anyhow::anyhow!("configuration reload retained previous workspace graph: {error}")
            })?;
        if self.config.settings.drag_drop != new_config.settings.drag_drop {
            self.cancel_drag(changes);
        }
        let rules_changed =
            self.config.virtual_workspaces.app_rules != new_config.virtual_workspaces.app_rules;
        self.config = new_config;
        self.layout_manager
            .layout_engine
            .set_layout_settings(&self.config.settings.layout);
        self.layout_manager.layout_engine.update_virtual_workspace_settings(
            &self.state.windows,
            &self.config.virtual_workspaces,
            migration,
        );
        if rules_changed {
            let apps: Vec<_> = self
                .app_manager
                .apps
                .iter()
                .map(|(&pid, app)| (pid, app.info.clone()))
                .collect();
            for (pid, info) in apps {
                let windows: Vec<_> = self.state.windows.window_ids_for_pid(pid).collect();
                self.process_windows_for_app_rules(changes, windows, info, false);
            }
        }
        self.drag_manager.update_config(self.config.settings.drag_drop);
        self.update_service_config();
        changes.arrange(None, Presentation::Animated);
        Ok(())
    }

    pub(in crate::actor::reactor) fn toggle_space(&mut self, changes: &mut Changes) {
        let Some(space) = self.active_display_space() else {
            return;
        };
        let display_uuid = self.display_uuid_for_space(space);
        self.space_activation_policy.toggle_space_activated(
            self.activation_cfg(),
            ToggleSpaceContext { space, display_uuid },
        );
        self.recompute_and_set_active_spaces_from_current_screens(changes);
        changes.arrange(Some(space), Presentation::Animated);
        changes.refresh_menu = true;
    }

    pub(in crate::actor::reactor) fn mission_control_command(&mut self, command: WmCmd) {
        if let Some(wm) = &self.communication_manager.wm_sender {
            wm.send(WmEvent::Command(WmCommand::Wm(command)));
        } else if matches!(command, WmCmd::DismissMissionControl) {
            self.set_mission_control_active(false);
        }
    }

    pub(in crate::actor::reactor) fn focus_window(
        &mut self,
        changes: &mut Changes,
        window: WindowId,
        server: Option<WindowServerId>,
        space: Option<SpaceId>,
    ) {
        if self.state.windows.contains_window(window) {
            if let Some(space) = space.filter(|space| self.is_space_active(*space)) {
                self.focus_window_in_space(changes, space, window);
            }
        } else if let Some(server) = server
            && let Err(error) = window_server::make_key_window(window.pid, server)
        {
            warn!(?error, "failed to make key window");
        }
    }

    fn focus_window_in_space(&mut self, changes: &mut Changes, space: SpaceId, window: WindowId) {
        if !self.state.windows.is_admitted(window)
            || self.state.windows.is_temporarily_unavailable(window)
            || self.window_in_non_active_workspace(space, window)
        {
            return;
        }
        self.reconcile_layout_event(changes, LayoutEvent::WindowFocused(space, window));
        self.stage_focus(changes, window);
    }

    pub(in crate::actor::reactor) fn focus_display(
        &mut self,
        changes: &mut Changes,
        selector: DisplaySelector,
        move_mouse: bool,
    ) {
        let Some(screen) = self.screen_for_selector(&selector, None) else {
            return;
        };
        let center = screen.frame.mid();
        let space = screen.space;
        let mut warp = center;
        if let Some(space) = space {
            if !self.is_space_active(space) {
                return;
            }
            if !move_mouse {
                if crate::sys::screen::set_active_menu_bar_display_uuid(&screen.display_uuid) {
                    self.space_state.menu_bar_space = Some(space);
                }
                self.space_state.command_space = Some(space);
            }
            let target = self.last_focused_window_in_space(space).or_else(|| {
                self.layout_manager
                    .layout_engine
                    .workspaces()
                    .windows_in_active_workspace(&self.state.windows, space)
                    .into_iter()
                    .next()
            });
            if let Some(window) = target {
                if !move_mouse {
                    warp = self
                        .state
                        .windows
                        .window(window)
                        .map_or(center, |w| w.frame_monotonic.mid());
                }
                self.focus_window_in_space(changes, space, window);
            }
        }
        changes.mouse_warp = Some(warp);
    }
}
