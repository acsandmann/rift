//! Workspace display bindings.
//!
//! `workspace_rules` may bind a workspace to a display. Every native space still
//! owns the full workspace list, so a binding is enforced by routing rather than
//! by a different topology. While the owning display is connected, a bound
//! workspace is only ever shown there:
//!
//! - Each display starts on a workspace bound to it, or an unbound one.
//! - Cycling, relative window moves and back-and-forth on a display skip the
//!   workspaces bound to other displays.
//! - `switch_to_workspace`, or picking the workspace in the overview, focuses
//!   the owning display and switches there; `move_window_to_workspace` sends
//!   the window to the owning display's copy of the workspace, and
//!   `move_workspace_to_display` will not take a bound workspace off its display.
//!
//! When the owning display is not connected the workspace behaves like an
//! unbound one on whatever display the user is on.

use super::{DisplaySelector, EventOutcome, Reactor, ScreenInfo};
use crate::actor::app::WindowId;
use crate::actor::reactor::events::command as command_workflow;
use crate::common::collections::HashSet;
use crate::layout_engine::LayoutCommand;
use crate::model::VirtualWorkspaceId;
use crate::sys::screen::SpaceId;

/// Order screens by physical arrangement: left to right, then top to bottom.
///
/// `DisplaySelector::Index` counts displays in this order.
pub(crate) fn physical_order(screens: &[ScreenInfo]) -> Vec<&ScreenInfo> {
    let mut ordered: Vec<&ScreenInfo> = screens.iter().collect();
    ordered.sort_by(|a, b| {
        a.frame
            .origin
            .x
            .total_cmp(&b.frame.origin.x)
            .then_with(|| a.frame.origin.y.total_cmp(&b.frame.origin.y))
    });
    ordered
}

/// The screen among `screens` that a workspace binding names.
pub(crate) fn bound_screen<'a>(
    screens: &'a [ScreenInfo],
    binding: &DisplaySelector,
) -> Option<&'a ScreenInfo> {
    match binding {
        DisplaySelector::Uuid(uuid) => screens.iter().find(|screen| screen.display_uuid == *uuid),
        DisplaySelector::Index(index) => physical_order(screens).get(*index).copied(),
        DisplaySelector::Direction(_) => None,
    }
}

fn same_screen(a: &ScreenInfo, b: &ScreenInfo) -> bool {
    a.id == b.id && a.display_uuid == b.display_uuid
}

impl Reactor {
    pub(crate) fn has_workspace_display_bindings(&self) -> bool {
        self.config.virtual_workspaces.has_display_bindings()
    }

    /// Tell the workspace manager, for every display in `screens`, which
    /// workspace it starts on and which workspaces are bound to another one.
    ///
    /// Runs on each forwarded snapshot before any new native space gets its
    /// workspaces, so windows found on a display land in one of its own, and on
    /// config reload.
    pub(crate) fn refresh_display_bindings(&mut self, screens: &[ScreenInfo]) {
        if !self.has_workspace_display_bindings() {
            self.layout_manager.layout_engine.workspaces_mut().set_display_bindings([]);
            return;
        }
        let settings = &self.config.virtual_workspaces;
        let count = settings.default_workspace_count.max(1);
        let default = settings.default_workspace;
        // Per workspace: `None` when unbound, else its owner if connected.
        let owners: Vec<Option<Option<&ScreenInfo>>> = (0..count)
            .map(|index| {
                settings
                    .display_binding_for_workspace(index)
                    .map(|binding| bound_screen(screens, binding))
            })
            .collect();
        let bindings: Vec<_> = screens
            .iter()
            .filter_map(|screen| {
                let owned_here =
                    |owner: &Option<Option<&ScreenInfo>>| matches!(owner, Some(Some(owner)) if same_screen(owner, screen));
                let foreign: HashSet<usize> = (0..count)
                    .filter(|index| matches!(owners[*index], Some(Some(_))) && !owned_here(&owners[*index]))
                    .collect();
                // default_workspace if it may live here, else the first workspace
                // bound here, else the first unbound one, else one not foreign.
                let start = (default < count && !foreign.contains(&default))
                    .then_some(default)
                    .or_else(|| owners.iter().position(owned_here))
                    .or_else(|| owners.iter().position(Option::is_none))
                    .or_else(|| (0..count).find(|index| !foreign.contains(index)));
                Some((screen.space?, start, foreign))
            })
            .collect();
        self.layout_manager
            .layout_engine
            .workspaces_mut()
            .set_display_bindings(bindings);
    }

    /// The active native space of the display workspace `index` is bound to, if
    /// that display is connected.
    fn bound_space_for_workspace_index(&self, index: usize) -> Option<SpaceId> {
        let binding = self.config.virtual_workspaces.display_binding_for_workspace(index)?;
        let screen = bound_screen(&self.space_state.screens, binding)?;
        screen.space.filter(|space| self.is_space_active(*space))
    }

    fn workspace_ordinal(&self, space: SpaceId, workspace: VirtualWorkspaceId) -> Option<usize> {
        let workspaces = self.layout_manager.layout_engine.workspaces();
        workspaces.workspace_ids(space).iter().position(|id| *id == workspace)
    }

    /// Redirect commands that show, or move a window to, a workspace bound to
    /// another display. `None` means the command takes the regular path.
    pub(crate) fn route_bound_workspace_command(
        &mut self,
        command: &LayoutCommand,
    ) -> Option<anyhow::Result<EventOutcome>> {
        if !self.has_workspace_display_bindings() {
            return None;
        }
        match command {
            LayoutCommand::SwitchToWorkspace(index) => {
                let owner = self.bound_space_for_workspace_index(*index)?;
                (self.command_context_space() != Some(owner))
                    .then(|| self.switch_to_bound_workspace(owner, *index))
            }
            LayoutCommand::MoveWindowToWorkspace { workspace, follow, window_id } => {
                let window = self.resolve_command_window(*window_id)?;
                let source = self
                    .assigned_space_for_window_id(window)
                    .filter(|space| self.is_space_active(*space))?;
                let workspaces = self.layout_manager.layout_engine.workspaces();
                let target = workspaces.resolve_workspace(source, workspace)?;
                let index = self.workspace_ordinal(source, target)?;
                let owner =
                    self.bound_space_for_workspace_index(index).filter(|owner| *owner != source)?;
                Some(self.move_window_to_bound_workspace(window, source, owner, index, *follow))
            }
            _ => None,
        }
    }

    /// Picking a display's copy of a workspace bound elsewhere in the overview
    /// switches to it on its owner instead.
    pub(crate) fn route_overview_workspace_selection(
        &mut self,
        space: SpaceId,
        index: usize,
    ) -> Option<anyhow::Result<EventOutcome>> {
        let owner = self.bound_space_for_workspace_index(index).filter(|owner| *owner != space)?;
        Some(self.switch_to_bound_workspace(owner, index))
    }

    /// Whether moving the workspace `source` shows to `target` would take a bound
    /// workspace off its display.
    pub(crate) fn bound_workspace_blocks_display_move(
        &self,
        source: SpaceId,
        target: SpaceId,
    ) -> bool {
        let active = self.layout_manager.layout_engine.workspaces().active_workspace(source);
        active
            .and_then(|active| self.workspace_ordinal(source, active))
            .and_then(|index| self.bound_space_for_workspace_index(index))
            .is_some_and(|owner| owner != target)
    }

    /// Resolve the window a workspace command acts on. A window named by id may
    /// live on any display, not just the one holding the command context.
    fn resolve_command_window(&self, window_id: Option<u32>) -> Option<WindowId> {
        let Some(idx) = window_id else {
            return self.layout_manager.layout_engine.focused_window();
        };
        let workspaces = self.layout_manager.layout_engine.workspaces();
        self.command_context_space()
            .into_iter()
            .chain(self.iter_active_spaces())
            .find_map(|space| workspaces.find_window_by_idx(&self.state.windows, space, idx))
    }

    /// Run a layout command through the regular command path on `space`.
    fn dispatch_layout_command_on(
        &mut self,
        command: LayoutCommand,
        space: SpaceId,
    ) -> anyhow::Result<EventOutcome> {
        let post_arrange_mouse_warp =
            self.config.settings.mouse_follows_focus.then(|| self.main_window()).flatten();
        let (visible_spaces, visible_space_centers) = self.visible_spaces_for_layout(false);
        command_workflow::handle_command_layout(
            &mut self.state,
            &mut self.layout_manager,
            &mut self.workspace_switch_manager,
            command_workflow::LayoutCommandPayload {
                command,
                command_space: Some(space),
                visible_spaces,
                visible_space_centers,
                post_arrange_mouse_warp,
            },
        )
    }

    /// Make `screen` the command and menu-bar context, as `focus_display` does.
    fn adopt_display_context(&mut self, screen: &ScreenInfo) {
        if crate::sys::screen::set_active_menu_bar_display_uuid(&screen.display_uuid) {
            self.space_state.menu_bar_space = screen.space;
        }
        self.space_state.command_space = screen.space;
    }

    fn switch_to_bound_workspace(
        &mut self,
        owner: SpaceId,
        index: usize,
    ) -> anyhow::Result<EventOutcome> {
        let Some(screen) = self.space_state.screen_by_space(owner).cloned() else {
            return Ok(EventOutcome::no_change());
        };
        let workspaces = self.layout_manager.layout_engine.workspaces_mut();
        if workspaces.workspace_id_at(owner, Some(index)) == workspaces.active_workspace(owner) {
            // Already showing there: go there, as focus_display does.
            return self.focus_display_by_selector(&DisplaySelector::Uuid(screen.display_uuid));
        }
        self.adopt_display_context(&screen);
        let outcome =
            self.dispatch_layout_command_on(LayoutCommand::SwitchToWorkspace(index), owner)?;
        // An empty workspace has nothing to focus; warp the cursor so the user
        // still lands on the display they asked for.
        let focuses = outcome
            .layout_responses
            .iter()
            .any(|(response, _)| response.focus_window.is_some());
        Ok(if focuses {
            outcome
        } else {
            outcome.with_mouse_warp(screen.frame.mid())
        })
    }

    fn move_window_to_bound_workspace(
        &mut self,
        window: WindowId,
        source: SpaceId,
        owner: SpaceId,
        index: usize,
        follow: bool,
    ) -> anyhow::Result<EventOutcome> {
        let Some(screen) = self.space_state.screen_by_space(owner).cloned() else {
            return Ok(EventOutcome::no_change());
        };
        let Some(state) = self.state.windows.window(window).filter(|_| !self.is_in_drag()) else {
            return Ok(EventOutcome::no_change());
        };
        let (window_server_id, frame) = (state.info.sys_id, state.frame_monotonic);
        let target_workspace = self
            .layout_manager
            .layout_engine
            .workspaces_mut()
            .workspace_id_at(owner, Some(index));
        let outcome = command_workflow::handle_command_reactor_move_window_to_display(
            &mut self.state,
            &mut self.layout_manager,
            command_workflow::MoveWindowToDisplayPayload {
                window,
                window_server_id,
                source_space: source,
                target_space: owner,
                target_screen: screen.frame,
                target_frame: Self::center_frame_on_screen(frame, screen.frame),
                target_workspace,
                follow,
            },
        )?;
        self.note_display_move_in_flight(window, owner);
        if follow {
            self.workspace_switch_manager
                .start_workspace_switch(super::WorkspaceSwitchOrigin::Manual);
            self.adopt_display_context(&screen);
        }
        Ok(outcome)
    }
}
