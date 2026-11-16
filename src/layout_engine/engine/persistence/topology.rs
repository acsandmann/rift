use super::*;
use crate::model::virtual_workspace::WorkspaceTransferReason;

impl LayoutEngine {
    /// Prove saved native Spaces against the authoritative inventory. A saved SpaceId is a
    /// historical name, so until this has run it is a destination for nothing.
    ///
    /// Each saved Space is one of: still in the inventory (kept, wherever macOS now shows it);
    /// renumbered on a connected display (remapped through the display identity saved in the file);
    /// on a display that is not connected, or that shows no current Space (kept unresolved, and
    /// revisited here on every snapshot); or gone. A gone Space is dropped in per-display mode. In
    /// shared mode its catalog entries survive and move to a live desktop, as they would for a
    /// display departure, keeping their home.
    ///
    /// Normal space switches never reach the body: a new current Space on a display is ordinarily
    /// a distinct layout, not a renamed old Space.
    pub fn resolve_saved_spaces(
        &mut self,
        window_store: &mut WindowStore,
        current_spaces: &[(SpaceId, String, CGSize)],
        expected_display_count: usize,
        inventory: &HashMap<String, Vec<SpaceId>>,
    ) {
        // The first forwarded snapshot can be empty or omit a display, and it disagrees with the
        // inventory while a display borrows a desktop. A display in native fullscreen has no
        // current Space, but its inventory still proves the Spaces it holds.
        if self.persistence.unresolved_spaces.is_empty()
            || expected_display_count == 0
            || inventory.values().filter(|spaces| !spaces.is_empty()).count()
                < expected_display_count
            || !current_spaces.iter().all(|(space, display, _)| {
                inventory.get(display).is_some_and(|spaces| spaces.contains(space))
                    && current_spaces.iter().filter(|(_, other, _)| other == display).count() == 1
                    && current_spaces.iter().filter(|(other, ..)| other == space).count() == 1
            })
        {
            return;
        }
        let saved = std::mem::take(&mut self.persistence.unresolved_spaces);
        let opened = std::mem::take(&mut self.persistence.awaiting_topology);
        let exists = |space: &SpaceId| inventory.values().any(|spaces| spaces.contains(space));

        let proposed: Vec<(SpaceId, SpaceId)> = current_spaces
            .iter()
            .filter_map(|(current, display, _)| {
                let old = *self.display_last_space.get(display)?;
                (old != *current && saved.contains(&old)).then_some((old, *current))
            })
            .collect();
        // Two displays claiming one Space prove nothing. A Space that still exists keeps its name
        // wherever macOS shows it: changing the main display moves desktops, windows and all,
        // between displays without renumbering them. Destinations are current, so they are distinct
        // and, having no saved state, never another remap's source.
        let remaps: Vec<_> = (proposed.iter().copied())
            .filter(|(old, new)| {
                proposed.iter().filter(|(other, _)| other == old).count() == 1
                    && !exists(old)
                    && !saved.contains(new)
            })
            .collect();
        for &(old, new) in &remaps {
            self.remap_space(window_store, old, new);
        }

        let shared = self.workspaces.settings.mode == WorkspaceMode::Global;
        for space in saved {
            if remaps.iter().any(|(old, _)| *old == space) || exists(&space) {
                continue;
            }
            let display = self.display_uuid_for_space(space).map(str::to_owned).or_else(|| {
                self.workspaces
                    .workspaces_on(space)
                    .find_map(|id| self.workspaces[id].preferred_display.clone())
            });
            let live_display = display.as_ref().and_then(|display| {
                current_spaces.iter().find(|(_, current, _)| current == display)
            });
            if shared {
                // A connected display with no current Space may yet show a renumbered successor.
                let connected = display.as_ref().is_some_and(|d| inventory.contains_key(d));
                let fallback = current_spaces.first().filter(|_| !connected);
                let Some(&(target, _, size)) = live_display.or(fallback) else {
                    self.persistence.unresolved_spaces.insert(space);
                    continue;
                };
                for id in self.workspaces.workspace_ids(space) {
                    let _ = self.transfer_workspace(
                        window_store,
                        id,
                        target,
                        size,
                        None,
                        WorkspaceTransferReason::DisplayDeparture,
                    );
                }
            } else if display.is_some() && live_display.is_none() {
                self.persistence.unresolved_spaces.insert(space);
            } else {
                for id in self.workspaces.remove_space(space) {
                    window_store.remap_workspace(id, None);
                    self.forget_workspace(id);
                }
                self.space_display_map.remove(&space);
                self.display_last_space.retain(|_, last| *last != space);
            }
        }
        if opened {
            self.settle_awaited_windows(window_store);
        }
    }
}
