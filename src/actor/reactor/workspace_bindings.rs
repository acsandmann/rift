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
//!
//! When the owning display is not connected the workspace behaves like an
//! unbound one on whatever display the user is on.

use super::{DisplaySelector, Reactor, ScreenInfo};
use crate::common::collections::HashSet;

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
}
