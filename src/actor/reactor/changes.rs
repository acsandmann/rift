use std::time::Instant;

use objc2_core_foundation::{CGPoint, CGRect};

use super::{WindowId, pid_t, raise_manager};
use crate::sys::screen::SpaceId;

// Stronger requests take precedence for the same Space. A workspace switch
// still applies full frames where sizes differ.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum Presentation {
    Animated,
    Resize,
    WorkspaceSwitch,
    Instant,
    Force,
}

#[derive(Clone, Copy, Default)]
pub(super) struct SpaceChange {
    pub(super) presentation: Option<Presentation>,
    pub(super) layout_changed: bool,
    pub(super) windows_changed: bool,
    pub(super) workspace_changed: bool,
    pub(super) tabs_changed: bool,
    pub(super) during_drag: bool,
}

/// Work that depends on the final reconciled model for one Reactor operation.
#[derive(Default)]
pub(super) struct Changes {
    all: SpaceChange,
    inline: [Option<(SpaceId, SpaceChange)>; 2],
    extra: Vec<(SpaceId, SpaceChange)>,
    pub(super) refresh_notifications: bool,
    pub(super) refresh_ffm: bool,
    pub(super) refresh_menu: bool,
    pub(super) overview_changed: bool,
    pub(super) overview_activation:
        Option<(crate::model::VirtualWorkspaceId, Option<WindowId>, Instant)>,
    pub(super) selection_changed: Option<SpaceId>,
    pub(super) post_arrange_mouse_warp: Option<WindowId>,
    pub(super) drop_haptic: bool,
    pub(super) inventory_all: bool,
    pub(super) flush_inventory: bool,
    pub(super) inventory: Option<pid_t>,
    pub(super) extra_inventory: Vec<pid_t>,
    pub(super) title_changed: Option<(WindowId, String, String)>,
    tiled_transfer: Option<(WindowId, SpaceId, Option<CGRect>)>,
    extra_transfers: Vec<(WindowId, SpaceId, Option<CGRect>)>,
    pub(super) desktop_focus: Option<SpaceId>,
    pub(super) fallback_focus: Option<(SpaceId, Option<WindowId>)>,
    pub(super) focus: Option<raise_manager::Event>,
    pub(super) mouse_warp: Option<CGPoint>,
    pub(super) display_warp: Option<SpaceId>,
    pub(super) focus_warp: Option<WindowId>,
}

impl Changes {
    pub(super) fn spaces(&self) -> impl Iterator<Item = &(SpaceId, SpaceChange)> {
        self.inline.iter().flatten().chain(&self.extra)
    }

    pub(super) fn for_space(&self, space: SpaceId) -> SpaceChange {
        let local = self
            .spaces()
            .find(|(id, _)| *id == space)
            .map_or_else(SpaceChange::default, |(_, local)| *local);
        SpaceChange {
            presentation: local.presentation.max(self.all.presentation),
            layout_changed: local.layout_changed || self.all.layout_changed,
            windows_changed: local.windows_changed || self.all.windows_changed,
            ..local
        }
    }

    pub(super) fn defer_drag_layouts(
        &mut self,
        mut defer: impl FnMut(Option<SpaceId>, Presentation, bool),
    ) {
        if let Some(policy) = self.all.presentation.take() {
            defer(None, policy, self.all.layout_changed);
        }
        for (space, change) in self.inline.iter_mut().flatten().chain(&mut self.extra) {
            if !change.during_drag
                && let Some(policy) = change.presentation.take()
            {
                defer(Some(*space), policy, change.layout_changed);
            }
        }
    }

    pub(super) fn has_publications(&self) -> bool {
        self.all.windows_changed
            || self
                .spaces()
                .any(|(_, change)| change.windows_changed || change.workspace_changed)
    }

    pub(super) fn has_layouts(&self) -> bool {
        self.all.presentation.is_some()
            || self.spaces().any(|(_, change)| change.presentation.is_some())
    }

    pub(super) fn space(&mut self, space: SpaceId) -> &mut SpaceChange {
        for slot in &mut self.inline {
            if slot.is_none() || slot.as_ref().is_some_and(|(id, _)| *id == space) {
                return &mut slot.get_or_insert((space, SpaceChange::default())).1;
            }
        }
        let index = self.extra.iter().position(|(id, _)| *id == space).unwrap_or_else(|| {
            self.extra.push((space, SpaceChange::default()));
            self.extra.len() - 1
        });
        &mut self.extra[index].1
    }

    pub(super) fn arrange(
        &mut self,
        space: Option<SpaceId>,
        presentation: Presentation,
    ) -> &mut SpaceChange {
        let change = self.scope(space);
        change.presentation = change.presentation.max(Some(presentation));
        change
    }

    fn scope(&mut self, space: Option<SpaceId>) -> &mut SpaceChange {
        match space {
            Some(space) => self.space(space),
            None => &mut self.all,
        }
    }

    pub(super) fn membership_for(&mut self, space: Option<SpaceId>) {
        let change = self.scope(space);
        change.windows_changed = true;
        change.layout_changed = true;
        self.overview_changed = true;
    }

    pub(super) fn move_between(&mut self, source: SpaceId, target: SpaceId, policy: Presentation) {
        self.arrange(Some(source), Presentation::Animated);
        self.arrange(Some(target), policy);
        self.membership_for(Some(source));
        self.membership_for(Some(target));
    }

    pub(super) fn workspace_changed(&mut self, space: SpaceId) {
        self.space(space).workspace_changed = true;
        self.membership_for(Some(space));
    }

    pub(super) fn inventory(&mut self, pid: pid_t) {
        if *self.inventory.get_or_insert(pid) != pid && !self.extra_inventory.contains(&pid) {
            self.extra_inventory.push(pid);
        }
    }

    pub(super) fn transfer_tile(
        &mut self,
        window: WindowId,
        target: SpaceId,
        fallback: Option<CGRect>,
    ) {
        let transfer = (window, target, fallback);
        if let Some(previous) = self
            .tiled_transfer
            .iter_mut()
            .chain(&mut self.extra_transfers)
            .find(|previous| previous.0 == window)
        {
            *previous = transfer;
        } else if self.tiled_transfer.is_none() {
            self.tiled_transfer = Some(transfer);
        } else {
            self.extra_transfers.push(transfer);
        }
    }

    pub(super) fn tiled_transfers(
        &self,
    ) -> impl Iterator<Item = (WindowId, SpaceId, Option<CGRect>)> + '_ {
        self.tiled_transfer.into_iter().chain(self.extra_transfers.iter().copied())
    }
}
