//! Global workspace mode: workspace numbers shared across displays.
//!
//! Workspaces still belong to one display's native space, but in global mode each
//! also carries a number that is unique across displays (see
//! `WorkspaceStore::number_global_workspaces`). Commands that name a workspace by
//! number resolve it to the display that owns it, then run the ordinary
//! per-display command there.

use tracing::warn;

use super::events::{EventOutcome, command as command_workflow};
use super::{Command, DisplaySelector, Event, Reactor, ReactorCommand, ScreenInfo};
use crate::actor::app::WindowId;
use crate::common::config::WorkspaceSelector;
use crate::layout_engine::LayoutCommand;
use crate::model::VirtualWorkspaceId;
use crate::sys::screen::SpaceId;

struct GlobalWorkspaceTarget {
    screen: ScreenInfo,
    space: SpaceId,
    workspace: VirtualWorkspaceId,
    /// Position among the owning space's workspaces, as per-display commands expect.
    local_index: usize,
}

impl Reactor {
    pub(super) fn global_workspaces_enabled(&self) -> bool {
        let settings = &self.config.virtual_workspaces;
        settings.enabled && settings.global
    }

    /// Numbers any not-yet-numbered workspaces on the displays currently on screen.
    pub(super) fn refresh_global_workspace_numbers(&mut self) {
        let displays: Vec<Option<SpaceId>> = self
            .screens_in_physical_order()
            .into_iter()
            .map(|screen| screen.space.filter(|space| self.is_space_active(*space)))
            .collect();
        let counts = self.config.virtual_workspaces.display_workspace_counts.clone();
        self.layout_manager.layout_engine.number_global_workspaces(&displays, &counts);
    }

    /// Makes `screen` the command context ahead of the native active-display
    /// notification, as `FocusDisplay` does.
    pub(super) fn focus_display_context(&mut self, screen: &ScreenInfo) {
        if crate::sys::screen::set_active_menu_bar_display_uuid(&screen.display_uuid) {
            self.space_state.menu_bar_space = screen.space;
        }
        self.space_state.command_space = screen.space;
    }

    /// Handles layout commands whose meaning changes in global mode. Returns `None`
    /// for commands that keep their per-display behavior.
    pub(super) fn route_global_workspace_command(
        &mut self,
        command: &LayoutCommand,
    ) -> Option<anyhow::Result<EventOutcome>> {
        match command {
            LayoutCommand::SwitchToWorkspace(number) => {
                Some(self.switch_to_global_workspace(*number))
            }
            LayoutCommand::MoveWindowToWorkspace {
                workspace: WorkspaceSelector::Index(number),
                follow,
                window_id,
            } => Some(self.move_window_to_global_workspace(*number, *follow, *window_id)),
            _ => None,
        }
    }

    fn resolve_global_workspace(&mut self, number: usize) -> Option<GlobalWorkspaceTarget> {
        self.refresh_global_workspace_numbers();
        let engine = &self.layout_manager.layout_engine;
        let Some((space, workspace)) = engine.global_workspace(number) else {
            warn!(number, "Global workspace ignored: no workspace has this number");
            return None;
        };
        let Some(screen) = self.space_state.screen_by_space(space).cloned() else {
            warn!(
                number,
                ?space,
                "Global workspace ignored: its space is not on a display"
            );
            return None;
        };
        if !self.is_space_active(space) {
            warn!(
                number,
                ?space,
                "Global workspace ignored: its display space is inactive"
            );
            return None;
        }
        let local_index = engine
            .workspaces()
            .workspace_ids(space)
            .iter()
            .position(|id| *id == workspace)?;
        Some(GlobalWorkspaceTarget {
            screen,
            space,
            workspace,
            local_index,
        })
    }

    fn switch_to_global_workspace(&mut self, number: usize) -> anyhow::Result<EventOutcome> {
        let Some(target) = self.resolve_global_workspace(number) else {
            return Ok(EventOutcome::no_change());
        };
        let command = LayoutCommand::SwitchToWorkspace(target.local_index);
        if self.workspace_command_space() == Some(target.space) {
            return self.run_layout_command(command, Some(target.space));
        }

        let workspaces = self.layout_manager.layout_engine.workspaces();
        if workspaces.active_workspace(target.space) == Some(target.workspace) {
            // Already visible elsewhere: only move focus there, never back-and-forth.
            let selector = DisplaySelector::Uuid(target.screen.display_uuid.clone());
            return self.dispatch_workflow(Event::Command(Command::Reactor(
                ReactorCommand::FocusDisplay(selector),
            )));
        }
        let focus_window =
            workspaces.last_focused_window(target.space, target.workspace).or_else(|| {
                workspaces
                    .workspace_windows(&self.state.windows, target.space, target.workspace)
                    .first()
                    .copied()
            });

        self.focus_display_context(&target.screen);
        let outcome = self.run_layout_command(command, Some(target.space))?;
        Ok(Self::warp_into_display(outcome, &target.screen, focus_window))
    }

    fn move_window_to_global_workspace(
        &mut self,
        number: usize,
        follow: bool,
        window_id: Option<u32>,
    ) -> anyhow::Result<EventOutcome> {
        if self.is_in_drag() {
            warn!("Ignoring move-window-to-workspace while a drag is active");
            return Ok(EventOutcome::no_change());
        }
        let Some(target) = self.resolve_global_workspace(number) else {
            return Ok(EventOutcome::no_change());
        };
        let command_space = self.command_context_space();
        let window = match window_id {
            Some(index) => command_space.and_then(|space| {
                self.layout_manager.layout_engine.workspaces().find_window_by_idx(
                    &self.state.windows,
                    space,
                    index,
                )
            }),
            None => self.layout_manager.layout_engine.focused_window(),
        };
        let Some(window) = window else {
            warn!("Move window to workspace ignored: no target window");
            return Ok(EventOutcome::no_change());
        };

        let source_space = self.assigned_space_for_window_id(window);
        if source_space == Some(target.space) {
            return self.run_layout_command(
                LayoutCommand::MoveWindowToWorkspace {
                    workspace: WorkspaceSelector::Index(target.local_index),
                    follow,
                    window_id,
                },
                Some(target.space),
            );
        }
        let Some(source_space) = source_space.filter(|space| self.is_space_active(*space)) else {
            warn!(
                ?window,
                "Move window to workspace ignored: source space unavailable"
            );
            return Ok(EventOutcome::no_change());
        };
        let Some(window_state) = self.state.windows.window(window) else {
            warn!(?window, "Move window to workspace ignored: unknown window");
            return Ok(EventOutcome::no_change());
        };
        let window_server_id = window_state.info.sys_id;
        let target_frame =
            Self::center_frame_on_screen(window_state.frame_monotonic, target.screen.frame);

        // Retained frames belong to the source space; fence queued presentation work.
        self.cancel_window_presentations(vec![window]);
        if follow {
            self.focus_display_context(&target.screen);
        }
        let outcome = command_workflow::handle_command_reactor_move_window_to_global_workspace(
            &mut self.state,
            &mut self.layout_manager,
            &mut self.workspace_switch_manager,
            command_workflow::MoveWindowToGlobalWorkspacePayload {
                window,
                window_server_id,
                source_space,
                target_space: target.space,
                target_workspace: target.workspace,
                target_screen: target.screen.frame,
                target_frame,
                follow,
            },
        )?;
        Ok(if follow {
            Self::warp_into_display(outcome, &target.screen, Some(window))
        } else {
            outcome
        })
    }

    /// Brings the pointer along to the newly focused display so focus-follows-mouse
    /// does not pull focus back. Windows of a workspace that was hidden are parked
    /// off screen until arranged, so warp to them only after the arrange.
    fn warp_into_display(
        mut outcome: EventOutcome,
        screen: &ScreenInfo,
        focus_window: Option<WindowId>,
    ) -> EventOutcome {
        match focus_window {
            Some(window) => outcome.post_arrange_mouse_warp = Some(window),
            None => {
                let center = objc2_core_foundation::CGPoint::new(
                    screen.frame.origin.x + screen.frame.size.width / 2.0,
                    screen.frame.origin.y + screen.frame.size.height / 2.0,
                );
                outcome = outcome.with_mouse_warp(center);
            }
        }
        outcome
    }

    fn run_layout_command(
        &mut self,
        command: LayoutCommand,
        command_space: Option<SpaceId>,
    ) -> anyhow::Result<EventOutcome> {
        let post_arrange_mouse_warp =
            self.config.settings.mouse_follows_focus.then(|| self.main_window()).flatten();
        let (visible_spaces, visible_space_frames) = self.visible_spaces_for_layout(false);
        command_workflow::handle_command_layout(
            &mut self.state,
            &mut self.layout_manager,
            &mut self.workspace_switch_manager,
            command_workflow::LayoutCommandPayload {
                command,
                command_space,
                visible_spaces,
                visible_space_frames,
                post_arrange_mouse_warp,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};
    use test_log::test;

    use super::super::testing::*;
    use super::*;
    use crate::common::config::VirtualWorkspaceSettings;
    use crate::layout_engine::{Direction, LayoutEvent};
    use crate::sys::window_server::WindowServerId;

    /// Two 1000x1000 displays side by side, each with three workspaces, in global mode.
    fn global_context(
        configure: impl FnOnce(&mut VirtualWorkspaceSettings),
    ) -> (Reactor, SpaceId, SpaceId) {
        crate::sys::screen::TEST_ACTIVE_DISPLAY.with(|display| display.take());
        let mut settings = VirtualWorkspaceSettings {
            default_workspace_count: 3,
            global: true,
            ..VirtualWorkspaceSettings::default()
        };
        configure(&mut settings);
        let mut reactor = test_reactor_with_workspace_settings(&settings);
        reactor.config.virtual_workspaces = settings;
        let (left, right) = (SpaceId::new(1), SpaceId::new(2));
        let frames =
            [0., 1000.].map(|x| CGRect::new(CGPoint::new(x, 0.), CGSize::new(1000., 1000.)));
        reactor.handle_event(space_state_event(frames.to_vec(), vec![Some(left), Some(right)]));
        (reactor, left, right)
    }

    fn layout_command(command: LayoutCommand) -> Event { Event::Command(Command::Layout(command)) }

    /// Adds a focused window to the left display's first workspace.
    fn focused_left_window(reactor: &mut Reactor, left: SpaceId) -> WindowId {
        reactor.add_test_app(1);
        let window = WindowId::new(1, 1);
        let frame = CGRect::new(CGPoint::new(100., 100.), CGSize::new(400., 300.));
        reactor.add_test_window(window, WindowServerId::new(101), Some(left), frame);
        let workspace = reactor.test_workspace(left, 0);
        assert!(reactor.assign_test_window_to_workspace(left, window, workspace));
        reactor.send_layout_event(LayoutEvent::WindowAdded(left, window));
        reactor.layout_manager.layout_engine.commit_workspace_focus(
            &mut reactor.state.windows,
            left,
            Some(window),
        );
        window
    }

    fn active(reactor: &Reactor, space: SpaceId) -> Option<VirtualWorkspaceId> {
        reactor.layout_manager.layout_engine.workspaces().active_workspace(space)
    }

    #[test]
    fn switching_to_a_number_on_another_display_focuses_that_display() {
        let (mut reactor, left, right) = global_context(|_| {});
        let left_active = active(&reactor, left);

        let outcome = reactor
            .dispatch_workflow(layout_command(LayoutCommand::SwitchToWorkspace(4)))
            .unwrap();

        assert_eq!(reactor.raw_command_space(), Some(right));
        crate::sys::screen::TEST_ACTIVE_DISPLAY.with(|display| {
            assert_eq!(display.borrow().as_deref(), Some("test-display-1"));
        });
        assert_eq!(outcome.mouse_warps, vec![CGPoint::new(1500., 500.)]);
        reactor.apply_event_outcome(outcome);
        assert_eq!(active(&reactor, right), Some(reactor.test_workspace(right, 1)));
        assert_eq!(active(&reactor, left), left_active);
    }

    #[test]
    fn switching_to_a_number_on_the_current_display_stays_local() {
        let (mut reactor, left, right) = global_context(|_| {});
        let right_active = active(&reactor, right);

        reactor.handle_event(layout_command(LayoutCommand::SwitchToWorkspace(1)));

        assert_eq!(reactor.raw_command_space(), Some(left));
        assert_eq!(active(&reactor, left), Some(reactor.test_workspace(left, 1)));
        assert_eq!(active(&reactor, right), right_active);
    }

    #[test]
    fn switching_to_a_workspace_visible_elsewhere_only_moves_focus() {
        let (mut reactor, left, right) =
            global_context(|settings| settings.workspace_auto_back_and_forth = true);
        let right_active = active(&reactor, right);
        assert_eq!(right_active, Some(reactor.test_workspace(right, 0)));

        reactor.handle_event(layout_command(LayoutCommand::SwitchToWorkspace(3)));

        assert_eq!(reactor.raw_command_space(), Some(right));
        assert_eq!(active(&reactor, right), right_active);
        assert_eq!(active(&reactor, left), Some(reactor.test_workspace(left, 0)));
    }

    #[test]
    fn uneven_display_counts_shift_the_ranges() {
        let (mut reactor, _left, right) = global_context(|settings| {
            settings.display_workspace_counts = vec![1, 3];
        });

        reactor.handle_event(layout_command(LayoutCommand::SwitchToWorkspace(2)));

        assert_eq!(reactor.raw_command_space(), Some(right));
        assert_eq!(active(&reactor, right), Some(reactor.test_workspace(right, 1)));
    }

    #[test]
    fn numbers_on_an_inactive_display_are_ignored() {
        let (mut reactor, left, right) = global_context(|_| {});
        reactor.space_state.active_spaces.remove(&right);
        reactor.active_spaces.remove(&right);
        let right_active = active(&reactor, right);

        let outcome = reactor
            .dispatch_workflow(layout_command(LayoutCommand::SwitchToWorkspace(4)))
            .unwrap();

        assert!(outcome.mouse_warps.is_empty());
        assert_eq!(reactor.raw_command_space(), Some(left));
        assert_eq!(active(&reactor, right), right_active);
    }

    #[test]
    fn moving_a_window_to_a_number_on_another_display_lands_it_there() {
        for follow in [false, true] {
            let (mut reactor, left, right) = global_context(|_| {});
            let window = focused_left_window(&mut reactor, left);
            let target = reactor.test_workspace(right, 1);

            let outcome = reactor
                .dispatch_workflow(layout_command(LayoutCommand::MoveWindowToWorkspace {
                    workspace: WorkspaceSelector::Index(4),
                    follow,
                    window_id: None,
                }))
                .unwrap();
            assert_eq!(outcome.post_arrange_mouse_warp, follow.then_some(window));
            reactor.apply_event_outcome(outcome);

            assert_eq!(
                reactor.state.windows.workspace_for_window(right, window),
                Some(target)
            );
            let frame = reactor.state.windows.window(window).unwrap().frame_monotonic;
            assert!(
                frame.origin.x >= 1000.,
                "window should be placed on the right display"
            );
            let expected_space = if follow { right } else { left };
            assert_eq!(reactor.raw_command_space(), Some(expected_space));
            assert_eq!(active(&reactor, right) == Some(target), follow);
        }
    }

    #[test]
    fn moved_workspace_keeps_its_number_on_the_new_display() {
        let (mut reactor, left, right) = global_context(|_| {});
        let window = focused_left_window(&mut reactor, left);
        let moved = reactor.test_workspace(left, 0);

        reactor.handle_event(Event::Command(Command::Reactor(
            ReactorCommand::MoveWorkspaceToDisplay {
                selector: DisplaySelector::Direction(Direction::Right),
                wrap_around: false,
            },
        )));

        let engine = &reactor.layout_manager.layout_engine;
        assert_eq!(engine.global_workspace(0), Some((right, moved)));
        assert_eq!(active(&reactor, right), Some(moved));
        assert_eq!(
            reactor.state.windows.workspace_for_window(right, window),
            Some(moved)
        );
        assert_eq!(engine.workspaces().workspace_ids(left).len(), 2);
        assert!(active(&reactor, left).is_some_and(|workspace| workspace != moved));
        assert_eq!(reactor.raw_command_space(), Some(right));

        // Number 0 now follows the workspace to the right display.
        reactor.space_state.command_space = Some(left);
        reactor.handle_event(layout_command(LayoutCommand::SwitchToWorkspace(0)));
        assert_eq!(reactor.raw_command_space(), Some(right));
        assert_eq!(active(&reactor, right), Some(moved));
    }

    #[test]
    fn an_empty_workspace_can_move_to_another_display() {
        let (mut reactor, left, right) = global_context(|_| {});
        let moved = reactor.test_workspace(left, 0);

        reactor.handle_event(Event::Command(Command::Reactor(
            ReactorCommand::MoveWorkspaceToDisplay {
                selector: DisplaySelector::Direction(Direction::Right),
                wrap_around: false,
            },
        )));

        assert_eq!(
            reactor.layout_manager.layout_engine.global_workspace(0),
            Some((right, moved))
        );
        assert_eq!(active(&reactor, right), Some(moved));
        assert_eq!(reactor.raw_command_space(), Some(right));
    }

    #[test]
    fn workspace_queries_report_global_numbers() {
        let (mut reactor, left, right) = global_context(|_| {});
        let numbers = |reactor: &mut Reactor, space| {
            reactor
                .query_workspaces(Some(space))
                .into_iter()
                .map(|workspace| (workspace.index, workspace.global_index))
                .collect::<Vec<_>>()
        };

        assert_eq!(
            numbers(&mut reactor, left),
            [(0, Some(0)), (1, Some(1)), (2, Some(2))]
        );
        assert_eq!(
            numbers(&mut reactor, right),
            [(0, Some(3)), (1, Some(4)), (2, Some(5))]
        );
        reactor.config.virtual_workspaces.global = false;
        assert!(numbers(&mut reactor, right).iter().all(|(_, number)| number.is_none()));
    }

    #[test]
    fn per_display_mode_is_unchanged_when_global_is_off() {
        let (mut reactor, left, right) = global_context(|settings| settings.global = false);
        let right_active = active(&reactor, right);

        reactor.handle_event(layout_command(LayoutCommand::SwitchToWorkspace(4)));

        assert_eq!(reactor.raw_command_space(), Some(left));
        assert_eq!(active(&reactor, left), Some(reactor.test_workspace(left, 0)));
        assert_eq!(active(&reactor, right), right_active);
    }
}
