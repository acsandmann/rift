//! How the reactor follows native display and space topology.
//!
//! The spaces actor forwards a [`ForwardedSpaceState`] whenever displays, their
//! current spaces, or window membership change. This module applies it: it
//! renumbers layouts for spaces macOS recreated, activates the screens' spaces,
//! reconciles each window's workspace with where the window server has it, and
//! returns windows carried off a departed display once it is back. It also
//! answers which space a window or frame is on.
//!
//! Invariants the reconciliation keeps:
//!
//! 1. A native space id can change without its virtual workspaces being lost:
//!    they are renumbered with it ([`Reactor::space_remaps`]).
//! 2. A temporary change of native membership, such as macOS carrying windows off
//!    a departed display, does not change the windows' logical workspace: they keep
//!    their place in their home tree while away.
//! 3. A tracked window has at most one virtual workspace assignment (the window
//!    store holds it).
//! 4. A returning display gets its existing workspaces back rather than new ones.
//! 5. A snapshot with incomplete membership never removes a window or workspace:
//!    if it would invalidate spaces, it waits for a complete one.

use super::*;
use crate::sys::wm_bridge;

impl Reactor {
    // Applying snapshots

    pub(super) fn handle_authoritative_space_snapshot(
        &mut self,
        mut snapshot: ForwardedSpaceState,
    ) -> anyhow::Result<EventOutcome> {
        if let Some(pending) = self.pending_space_change_manager.pending_space_change.take() {
            snapshot = merge_deferred_snapshot(pending, snapshot);
        }
        if self.is_mission_control_active() {
            self.pending_space_change_manager.pending_space_change = Some(snapshot);
            return Ok(EventOutcome::default());
        }
        self.pending_native_returns.retain(|wid, destination| {
            self.state
                .windows
                .window(*wid)
                .and_then(|window| window.info.sys_id)
                .is_some_and(|wsid| snapshot.active_window_spaces.get(&wsid) != Some(destination))
        });
        let space_remaps = self.space_remaps(&snapshot);
        let invalidated_spaces = self.invalidated_spaces(&snapshot, &space_remaps);
        if !snapshot.membership_complete && !invalidated_spaces.is_empty() {
            // Defer geometry and membership together so the old ownership survives
            // an inconclusive query. The spaces actor retries until one is complete.
            self.pending_space_change_manager.pending_space_change = Some(snapshot);
            return Ok(EventOutcome::default());
        }
        if snapshot.membership_complete
            && snapshot.display_set_changed
            && !self.space_state.screens.is_empty()
        {
            let previous = &self.space_state.display_space_ids;
            let candidates: Vec<_> = snapshot
                .display_space_ids
                .values()
                .flatten()
                .copied()
                .filter(|space| !previous.values().any(|spaces| spaces.contains(space)))
                .filter(|space| !snapshot.fullscreen_spaces.contains(space))
                .filter(|space| !space_remaps.iter().any(|(_, new)| new == space))
                .filter(|space| {
                    self.layout_manager.layout_engine.workspaces().workspace_ids(*space).is_empty()
                })
                .collect();
            if !candidates.is_empty()
                && let Some(tx) = &self.spaces_tx
            {
                tx.send(crate::actor::spaces::Event::RetireUnusedDesktops {
                    revision: snapshot.revision,
                    spaces: candidates,
                });
            }
        }
        let display_reconfigured = snapshot.display_set_changed
            || snapshot.should_force_refresh_layout
            || !snapshot.resized_spaces.is_empty();
        if display_reconfigured {
            // A desktop that itself changed display is being recovered by identity
            // reconciliation; its windows must not be pulled back to the reused id.
            self.aftercare_windows = self
                .state
                .windows
                .iter_windows()
                .filter_map(|(wid, _)| {
                    let space = self.assigned_space_for_window_id(wid)?;
                    let display = self
                        .space_state
                        .display_space_ids
                        .iter()
                        .find(|(_, spaces)| spaces.contains(&space))?
                        .0;
                    snapshot
                        .display_space_ids
                        .get(display)
                        .is_some_and(|spaces| spaces.contains(&space))
                        .then_some(wid)
                })
                .collect();
        }
        let spaces: Vec<_> = snapshot.screens.iter().map(|screen| screen.space).collect();
        let topology_changed = snapshot.topology_changed();
        let command_space_only = !topology_changed && self.only_command_space_differs(&snapshot);
        let ForwardedSpaceState {
            screens,
            fullscreen_spaces,
            menu_bar_space,
            command_space,
            display_space_ids,
            display_set_changed,
            should_force_refresh_layout,
            membership_complete,
            resized_spaces,
            topology_window_delta,
            active_window_spaces,
            ..
        } = snapshot;
        let mut outcome = EventOutcome::window_membership_changed(false, true);
        self.space_state.active_window_spaces = active_window_spaces;
        self.space_state.membership_complete = membership_complete;

        self.space_state.fullscreen_spaces = fullscreen_spaces;
        self.space_state.menu_bar_space = menu_bar_space;
        self.space_state.command_space = command_space;
        if command_space_only {
            outcome.arrange.passes = 0;
            self.maybe_send_menu_update();
            return Ok(outcome);
        }
        if display_set_changed {
            if let Some(tx) = &self.animation_tx {
                let _ = tx.send(animation::Message::Displays(
                    screens.iter().map(|s| s.id.as_u32()).collect(),
                ));
            }
            let active_displays: Vec<String> =
                screens.iter().map(|screen| screen.display_uuid.clone()).collect();
            self.layout_manager.layout_engine.prune_display_state(&active_displays);
        }
        self.space_state.display_space_ids = display_space_ids;

        if screens.is_empty() {
            self.refocus_manager.stale_cleanup_state = StaleCleanupState::Suppressed;
            if !self.space_state.screens.is_empty() {
                self.space_state.screens.clear();
                self.expose_all_spaces();
            }
            self.recompute_active_spaces(&[], &[]);
            self.update_complete_window_server_info(Vec::new());
            self.try_apply_pending_space_change();
            return Ok(outcome);
        }

        self.refocus_manager.stale_cleanup_state = StaleCleanupState::Enabled;
        self.space_state.screens = screens;
        if display_reconfigured {
            self.topology_changed_at = Some(Instant::now());
            self.sent_back_after_display_change.clear();
            self.pending_native_returns.clear();
            self.clear_pending_hidden_window_targets();
            self.retarget_drag_off_departed_display();
        }
        for &(old, new) in &space_remaps {
            if let Some(record) = self.moved_desktops.remove(&old) {
                self.moved_desktops.insert(new, record);
            }
            self.layout_manager.layout_engine.remap_space(&mut self.state.windows, old, new);
        }
        self.space_activation_policy.remap_spaces(&space_remaps);
        for screen in &self.space_state.screens {
            let (Some(space), Some(display_uuid)) = (screen.space, screen.display_uuid_opt())
            else {
                continue;
            };
            if self
                .layout_manager
                .layout_engine
                .display_uuid_for_space(space)
                .is_some_and(|previous| previous != display_uuid)
            {
                self.moved_desktops.insert(space, (Instant::now(), None));
            }
            self.layout_manager
                .layout_engine
                .update_space_display(space, Some(display_uuid.to_string()));
        }
        let activation_config = self.activation_cfg();
        self.space_activation_policy
            .on_spaces_updated(activation_config, &self.space_state.screens);
        self.recompute_active_spaces(&spaces, &invalidated_spaces);
        self.return_displaced_windows();
        self.restore_windows_after_fullscreen_exit(&spaces);

        for (space, size) in resized_spaces {
            if !self.is_space_active(space) {
                continue;
            }
            self.layout_manager.layout_engine.workspaces_mut().list_workspaces(space);
            outcome = outcome.with_layout_event(LayoutEvent::SpaceExposed(space, size));
        }
        if let Some(delta) = topology_window_delta {
            if delta.flags.contains(DisplayReconfigFlags::SET_MAIN) {
                self.main_display_changed_at = Some(Instant::now());
            }
            outcome.absorb(self.apply_topology_window_delta(delta));
        }
        self.finalize_space_change(
            &spaces,
            !membership_complete,
            &invalidated_spaces,
            topology_changed,
        );
        self.try_apply_pending_space_change();
        self.rescue_offscreen_floats();
        if should_force_refresh_layout {
            outcome = outcome.with_arrange_passes(1);
        }
        Ok(outcome)
    }

    /// Whether `snapshot` matches the current state except for display focus, so
    /// it needs no reconciliation or layout.
    pub(super) fn only_command_space_differs(&self, snapshot: &ForwardedSpaceState) -> bool {
        let current = &self.space_state;
        let spaces: Vec<_> = snapshot.screens.iter().map(|screen| screen.space).collect();
        let displays: Vec<_> =
            snapshot.screens.iter().map(|screen| screen.display_uuid_owned()).collect();
        let active_spaces: HashSet<SpaceId> = self
            .space_activation_policy
            .compute_active_spaces(self.activation_cfg(), &spaces, &displays)
            .into_iter()
            .flatten()
            .collect();
        current.screens == snapshot.screens
            && current.fullscreen_spaces == snapshot.fullscreen_spaces
            && self.active_spaces == active_spaces
            && current.display_space_ids == snapshot.display_space_ids
            && current.membership_complete == snapshot.membership_complete
            && current.active_window_spaces == snapshot.active_window_spaces
    }

    /// Native spaces a topology change removed from their display, other than ones
    /// renumbered in place. Windows leaving them keep their workspace ordinal.
    pub(super) fn invalidated_spaces(
        &self,
        snapshot: &ForwardedSpaceState,
        space_remaps: &[(SpaceId, SpaceId)],
    ) -> Vec<SpaceId> {
        if !(snapshot.display_set_changed
            || snapshot.should_force_refresh_layout
            || snapshot.topology_window_delta.is_some())
        {
            return Vec::new();
        }
        self.space_state
            .display_space_ids
            .iter()
            .flat_map(|(display, spaces)| {
                let current = snapshot.display_space_ids.get(display);
                spaces
                    .iter()
                    .copied()
                    .filter(move |space| !current.is_some_and(|current| current.contains(space)))
            })
            .filter(|space| !space_remaps.iter().any(|(previous, _)| previous == space))
            .collect()
    }

    /// Native spaces macOS recreated or handed to another display, as `(old, new)` pairs:
    /// the desktop a display was showing is now the new space it shows.
    ///
    /// That is so when the display lists a new space with no workspaces yet, and either
    /// - the old id is gone from every display and the new one was not already listed
    ///   alongside it, as when the display returns or after a reboot, or
    /// - most of the windows Rift had on the old space are on the new one. macOS can
    ///   give the old id to another display and keep its windows here on a desktop it
    ///   mints, as when a monitor arrives as main.
    ///
    /// Anything else is a switch to another desktop, or macOS moving a desktop between
    /// displays, windows and all (as a main display change does), and keeps every
    /// space's identity. The display's last space is persisted, so this also covers a
    /// display returning after a restart. A space given up by one remap can receive
    /// the next display's.
    pub(super) fn space_remaps(&self, snapshot: &ForwardedSpaceState) -> Vec<(SpaceId, SpaceId)> {
        let engine = &self.layout_manager.layout_engine;
        let inventory = &snapshot.display_space_ids;
        let previous_inventory = &self.space_state.display_space_ids;
        let listed = |inventory: &HashMap<String, Vec<SpaceId>>, space: SpaceId| {
            inventory.values().any(|spaces| spaces.contains(&space))
        };
        let windows_followed = |previous: SpaceId, space: SpaceId| {
            if !snapshot.membership_complete {
                return false;
            }
            let (mut here, mut elsewhere) = (0, 0);
            for (wid, window) in self.state.windows.iter_windows() {
                if self.assigned_space_for_window_id(wid) != Some(previous) {
                    continue;
                }
                match window.info.sys_id.and_then(|wsid| snapshot.active_window_spaces.get(&wsid)) {
                    Some(&native) if native == space => here += 1,
                    Some(_) => elsewhere += 1,
                    None => {}
                }
            }
            here > elsewhere
        };
        let mut remaps = self.vanished_desktop_remaps(snapshot);
        // Each pass can free a space a later display's desktop is renumbered to.
        loop {
            let found = remaps.len();
            for screen in &snapshot.screens {
                let Some(display) = screen.display_uuid_opt() else {
                    continue;
                };
                let Some(display_spaces) = inventory.get(display) else {
                    continue;
                };
                // A display showing a fullscreen desktop still has its own desktop behind it.
                let space = screen.space.or_else(|| {
                    let mut desktops = display_spaces
                        .iter()
                        .filter(|space| !snapshot.fullscreen_spaces.contains(space));
                    let desktop = *desktops.next()?;
                    desktops.next().is_none().then_some(desktop)
                });
                let (Some(space), Some(previous)) =
                    (space, engine.last_space_for_display_uuid(display))
                else {
                    continue;
                };
                let free = engine.workspaces().workspace_ids(space).is_empty()
                    || remaps.iter().any(|&(old, _)| old == space);
                let renumbered = previous != space
                    && display_spaces.contains(&space)
                    && free
                    && !remaps.iter().any(|&(old, new)| old == previous || new == space)
                    && ((!listed(inventory, previous)
                        && !(listed(previous_inventory, previous)
                            && listed(previous_inventory, space)))
                        || windows_followed(previous, space));
                if renumbered {
                    remaps.push((previous, space));
                }
            }
            if remaps.len() == found {
                return remaps;
            }
        }
    }

    /// Desktops that are gone from every display, paired with the new desktops holding
    /// their windows (see [`desktop_match`]). This covers every desktop of a returning
    /// display, hidden ones included, which may come back under another display.
    ///
    /// A desktop is only matched once its own display is connected again. Until then
    /// its windows were carried onto the displays that stayed, and that is not where
    /// the desktop went. Only desktops with no workspaces yet can receive a match.
    pub(super) fn vanished_desktop_remaps(
        &self,
        snapshot: &ForwardedSpaceState,
    ) -> Vec<(SpaceId, SpaceId)> {
        use super::desktop_match::{Desktop, match_desktops};

        let engine = &self.layout_manager.layout_engine;
        let inventory = &snapshot.display_space_ids;
        let connected: HashSet<&str> =
            snapshot.screens.iter().map(|screen| screen.display_uuid.as_str()).collect();
        let mut vanished: Vec<(SpaceId, String)> = engine
            .workspaces()
            .layout_spaces()
            .into_iter()
            .filter(|space| !inventory.values().any(|spaces| spaces.contains(space)))
            .filter_map(|space| Some((space, engine.display_uuid_for_space(space)?)))
            .filter(|(_, display)| connected.contains(display.as_str()))
            .collect();
        vanished.sort_unstable();
        let mut fresh: Vec<(SpaceId, &String, usize)> = inventory
            .iter()
            .flat_map(|(display, spaces)| {
                spaces.iter().enumerate().map(move |(index, &space)| (space, display, index))
            })
            .filter(|&(space, ..)| {
                !snapshot.fullscreen_spaces.contains(&space)
                    && engine.workspaces().workspace_ids(space).is_empty()
            })
            .collect();
        fresh.sort_unstable_by(|a, b| a.1.cmp(b.1).then(a.2.cmp(&b.2)).then(a.0.cmp(&b.0)));
        if vanished.is_empty() || fresh.is_empty() {
            return Vec::new();
        }

        // Each window's home: the workspace it is assigned to, or that it was carried off.
        let mut homes: HashMap<SpaceId, Vec<(WindowId, WindowServerId)>> = HashMap::default();
        for (wid, window) in self.state.windows.iter_windows() {
            let Some(wsid) = window.info.sys_id else { continue };
            let home = engine
                .away_home(wid)
                .map(|(space, _)| space)
                .or_else(|| self.assigned_space_for_window_id(wid));
            if let Some(home) = home.filter(|home| vanished.iter().any(|(space, _)| space == home))
            {
                homes.entry(home).or_default().push((wid, wsid));
            }
        }
        let mut now_holding: HashMap<SpaceId, Vec<WindowId>> = HashMap::default();
        for &(wid, wsid) in homes.values().flatten() {
            let native = snapshot
                .active_window_spaces
                .get(&wsid)
                .copied()
                .or_else(|| window_server::window_space(wsid));
            if let Some(native) = native {
                now_holding.entry(native).or_default().push(wid);
            }
        }

        let was: Vec<Desktop> = vanished
            .into_iter()
            .filter_map(|(space, display)| {
                let index = self
                    .space_state
                    .display_space_ids
                    .get(&display)
                    .and_then(|spaces| spaces.iter().position(|listed| *listed == space));
                let windows: Vec<WindowId> =
                    homes.get(&space).into_iter().flatten().map(|(wid, _)| *wid).collect();
                // With neither windows nor a place among its display's desktops there is
                // nothing to recognize it by.
                (!windows.is_empty() || index.is_some()).then(|| Desktop {
                    space,
                    display: Some(display),
                    index: index.unwrap_or(usize::MAX),
                    windows,
                })
            })
            .collect();
        let now: Vec<Desktop> = fresh
            .into_iter()
            .map(|(space, display, index)| Desktop {
                space,
                display: Some(display.clone()),
                index,
                windows: now_holding.remove(&space).unwrap_or_default(),
            })
            .collect();
        let matching = match_desktops(&was, &now);
        // A desktop listed alongside the vanished one is another desktop, as when the
        // vanished one was deleted and macOS shows its neighbour.
        let listed_before = |space: SpaceId| {
            self.space_state
                .display_space_ids
                .values()
                .any(|spaces| spaces.contains(&space))
        };
        let mut remaps: Vec<_> = was
            .iter()
            .filter_map(|desktop| Some((desktop.space, matching.get(desktop.space)?)))
            .filter(|&(old, new)| !(listed_before(old) && listed_before(new)))
            .collect();
        remaps.sort_unstable();
        remaps
    }

    pub(super) fn try_apply_pending_space_change(&mut self) {
        if self.is_mission_control_active() || self.refreshes_blocked() {
            return;
        }
        if let Some(pending) = self.pending_space_change_manager.pending_space_change.take()
            && pending.revision == self.space_state.revision
        {
            // During native Mission Control we must preserve the full forwarded snapshot,
            // not just the raw spaces vector, otherwise command-space and per-display space
            // metadata can remain stale after exit.
            if let Ok(outcome) = self.handle_authoritative_space_snapshot(pending) {
                self.apply_event_outcome(outcome);
            }
        }
    }

    pub(super) fn invalidate_native_topology(&mut self, revision: u64) {
        if revision <= self.space_state.revision {
            return;
        }
        self.space_state.revision = revision;
        self.space_state.authoritative = false;
        self.defer_window_inventory_refresh();
    }

    pub(super) fn refreshes_blocked(&self) -> bool { !self.space_state.authoritative }

    /// Reconciles windows with the applied snapshot, then refocuses and announces
    /// the command display's workspace.
    pub(super) fn finalize_space_change(
        &mut self,
        spaces: &[Option<SpaceId>],
        preserve_missing_assignments: bool,
        invalidated_spaces: &[SpaceId],
        topology_changed: bool,
    ) {
        self.refocus_manager.stale_cleanup_state = if spaces.iter().all(Option::is_none) {
            StaleCleanupState::Suppressed
        } else {
            StaleCleanupState::Enabled
        };
        self.expose_all_spaces();
        if let Some(main_window) = self.main_window()
            && let Some(space) = self.main_window_space()
        {
            self.send_layout_event(LayoutEvent::WindowFocused(space, main_window));
        }
        let active_windows = self.authoritative_active_space_windows();
        self.reconcile_authoritative_active_window_snapshot(
            active_windows,
            preserve_missing_assignments,
            invalidated_spaces,
        );
        if topology_changed {
            self.request_window_inventories();
        } else {
            self.discover_windows_on_active_spaces();
        }

        if let Some(space) = self.workspace_command_space() {
            self.focus_desktop_if_active_workspace_empty(space);
        }
        let Some(space) = self.workspace_command_space().or_else(|| {
            spaces.iter().flatten().copied().find(|space| self.is_space_active(*space))
        }) else {
            return;
        };
        if let Some((workspace_id, workspace_name)) =
            self.layout_manager.layout_engine.ensure_active_workspace_info(space)
        {
            let event = BroadcastEvent::WorkspaceChanged {
                workspace_id: protocol_workspace_id(workspace_id),
                workspace_name,
                space_id: space.get(),
                display_uuid: self.display_uuid_for_space(space),
            };
            self.communication_manager.event_broadcaster.send(event);
        }
    }

    /// A drag over a display that went away has no destination there: dropping into its
    /// desktop would move the window out of sight, into a layout kept for the display's
    /// return. The drag continues over whichever shown display has the window now.
    fn retarget_drag_off_departed_display(&mut self) {
        let Some(source) = self.drag_manager.actor.source() else {
            return;
        };
        if source
            .current_space
            .is_none_or(|space| self.space_state.is_space_on_screen(space))
        {
            return;
        }
        let space = self.best_space_for_frame(&source.last_frame);
        if self.drag_manager.actor.update_current_space(space) {
            self.resolve_drag_preview();
        }
    }

    /// Sends windows carried off a display back to their workspaces on it now that it is here.
    pub(super) fn return_displaced_windows(&mut self) {
        let connected: Vec<_> = self
            .space_state
            .screens
            .iter()
            .flat_map(|screen| self.space_state.display_space_ids.get(&screen.display_uuid))
            .flatten()
            .copied()
            .collect();
        let candidates: Vec<_> = self
            .state
            .windows
            .iter_windows()
            .filter_map(|(wid, window)| {
                let (space, _) = self.layout_manager.layout_engine.away_home(wid)?;
                connected.contains(&space).then_some((wid, window.info.sys_id, space))
            })
            .collect();
        let mut moved = false;
        for (wid, wsid, space) in candidates {
            let engine = &mut self.layout_manager.layout_engine;
            let current =
                self.state.windows.workspace_info_for_window(wid).map(|info| info.workspace_id);
            if current != engine.away_host(wid).map(|(_, workspace)| workspace) {
                // Let the engine discard the old home slot, respecting a placement made
                // while the display was away, before issuing any native move.
                engine.return_displaced_window(&mut self.state.windows, wid);
                continue;
            }
            let issued = wsid.is_some_and(|wsid| wm_bridge::move_windows_to_space(&[wsid], space));
            moved |= issued;
            // A frame write can still return a window to a shown desktop on older systems.
            if !issued && self.space_state.screen_by_space(space).is_none() {
                continue;
            }
            if self
                .layout_manager
                .layout_engine
                .return_displaced_window(&mut self.state.windows, wid)
                && self.is_space_active(space)
                && self.state.windows.is_visible_admitted(wid)
            {
                self.send_layout_event(LayoutEvent::WindowAdded(space, wid));
            }
        }
        if moved {
            self.request_space_snapshot();
        }
    }

    /// Rescue shown floating or disabled-tiling windows after a display change.
    /// Hidden floats keep their saved frame until their workspace is shown.
    pub(super) fn rescue_offscreen_floats(&mut self) -> bool {
        self.moved_desktops.retain(|_, (at, shown)| {
            at.elapsed() < Duration::from_secs(30 * 60)
                && shown.is_none_or(|since| since.elapsed() < Duration::from_secs(3))
        });
        let after_churn = self
            .topology_changed_at
            .is_some_and(|at| at.elapsed() < Duration::from_secs(10));
        if (!after_churn && self.moved_desktops.is_empty())
            || self.is_in_drag()
            || self.is_mission_control_active()
            || self.refreshes_blocked()
        {
            return false;
        }
        for screen in &self.space_state.screens {
            if let Some(space) = screen.space
                && let Some((_, shown)) = self.moved_desktops.get_mut(&space)
            {
                shown.get_or_insert_with(Instant::now);
            }
        }
        let moves: Vec<_> = self
            .state
            .windows
            .iter_windows()
            .filter_map(|(wid, window)| {
                let assigned = self.state.windows.workspace_info_for_window(wid)?;
                let engine = &self.layout_manager.layout_engine;
                let disabled = !self.is_space_active(assigned.space);
                let visible = if disabled {
                    window.is_admitted()
                        && window.info.sys_id.is_some_and(|wsid| {
                            self.space_state.active_window_spaces.get(&wsid)
                                == Some(&assigned.space)
                        })
                } else {
                    self.state.windows.is_visible_admitted(wid)
                };
                if (!engine.is_window_floating(wid) && !disabled)
                    || engine.workspaces().active_workspace(assigned.space)
                        != Some(assigned.workspace_id)
                    || !visible
                {
                    return None;
                }
                let screen = self.space_state.screen_by_space(assigned.space)?.frame;
                let frame = window.frame_monotonic;
                let area = frame.size.width * frame.size.height;
                if area <= 1.0 {
                    return None;
                }
                let overlap = frame.intersection(&screen);
                let moved = self.moved_desktops.contains_key(&assigned.space);
                let overhangs = frame.origin.x < screen.origin.x - 2.
                    || frame.origin.y < screen.origin.y - 2.
                    || frame.max().x > screen.max().x + 2.
                    || frame.max().y > screen.max().y + 2.;
                let mostly_gone =
                    overlap.size.width.max(0.) * overlap.size.height.max(0.) / area < 0.25;
                if !(if moved || (after_churn && disabled) {
                    overhangs
                } else {
                    after_churn && mostly_gone
                }) {
                    return None;
                }
                let size = CGSize::new(
                    frame.size.width.min(screen.size.width),
                    frame.size.height.min(screen.size.height),
                );
                let target = CGRect::new(
                    CGPoint::new(
                        frame.origin.x.clamp(screen.origin.x, screen.max().x - size.width),
                        frame.origin.y.clamp(screen.origin.y, screen.max().y - size.height),
                    ),
                    size,
                );
                Some((wid, window.info.sys_id?, assigned, target))
            })
            .collect();
        let rescued = !moves.is_empty();
        for (wid, wsid, assigned, target) in moves {
            if self.layout_manager.layout_engine.is_window_floating(wid) {
                self.layout_manager.layout_engine.store_floating_position(
                    assigned.space,
                    assigned.workspace_id,
                    wid,
                    target,
                );
            }
            let txid = self.transaction_manager.generate_next_txid(wsid);
            self.transaction_manager.store_txid(wsid, txid, target);
            if let Some(app) = self.app_manager.apps.get(&wid.pid) {
                _ = app.handle.send(Request::set_window_frame(wid, target, txid, true));
            }
        }
        rescued
    }

    /// Takes the frames of windows on shown workspaces from the window server and forgets their
    /// pending writes. A write made during a display change can go unanswered, be adjusted, or be
    /// undone by the window server, and a target left pending makes every later arrange skip the
    /// window as already on its way. Returns whether any window is not where Rift expects it, its
    /// pending target or else its last known frame, so the next arrange has something to write.
    pub(super) fn observe_shown_window_frames(&mut self) -> bool {
        let engine = &self.layout_manager.layout_engine;
        let shown: HashMap<WindowServerId, WindowId> = self
            .space_state
            .iter_known_spaces()
            .filter(|space| self.space_state.screen_by_space(*space).is_some())
            .flat_map(|space| {
                engine.workspaces().windows_in_active_workspace(&self.state.windows, space)
            })
            .filter_map(|wid| Some((self.state.windows.window(wid)?.info.sys_id?, wid)))
            .collect();
        let wsids: Vec<_> = shown.keys().copied().collect();
        let pending: HashMap<WindowServerId, CGRect> = wsids
            .iter()
            .filter_map(|&wsid| Some((wsid, self.transaction_manager.get_target_frame(wsid)?)))
            .collect();
        for wsid in &wsids {
            self.transaction_manager.clear_target_for_window(*wsid);
        }
        let mut drifted = false;
        for info in window_server::get_windows(&wsids) {
            if let Some(window) =
                shown.get(&info.id).and_then(|wid| self.state.windows.window_mut(*wid))
            {
                let expected = pending.get(&info.id).copied().unwrap_or(window.frame_monotonic);
                drifted |= !expected.same_as(info.frame);
                window.frame_monotonic = info.frame;
            }
        }
        drifted
    }

    pub(super) fn expose_all_spaces(&mut self) {
        let spaces: Vec<SpaceId> = self
            .space_state
            .iter_known_spaces()
            .filter(|space| self.is_space_active(*space))
            .collect();
        for space in spaces {
            self.expose_space_if_known(space);
        }
    }

    pub(super) fn expose_space_if_known(&mut self, space: SpaceId) {
        let Some(screen) = self.space_state.screen_by_space(space) else {
            return;
        };
        self.layout_manager.layout_engine.workspaces_mut().list_workspaces(space);
        self.send_layout_event(LayoutEvent::SpaceExposed(space, screen.frame.size));
    }

    // Active spaces

    pub(super) fn set_active_spaces(&mut self, spaces: &[Option<SpaceId>]) {
        self.active_spaces = spaces.iter().flatten().copied().collect();
    }

    pub(super) fn is_space_active(&self, space: SpaceId) -> bool {
        self.active_spaces.contains(&space)
    }

    pub(super) fn iter_active_spaces(&self) -> impl Iterator<Item = SpaceId> + '_ {
        self.active_spaces.iter().copied()
    }

    pub(super) fn active_space_ids(&self) -> Vec<u64> {
        self.active_spaces.iter().map(|space| space.get()).collect()
    }

    pub(super) fn is_window_on_active_space(&self, wid: WindowId) -> bool {
        self.best_space_for_window_id(wid)
            .is_some_and(|space| self.is_space_active(space))
    }

    /// Applies the activation policy to the screens' spaces. Windows leaving an
    /// `invalidated_spaces` space keep their workspace ordinal.
    pub(super) fn recompute_active_spaces(
        &mut self,
        spaces: &[Option<SpaceId>],
        invalidated_spaces: &[SpaceId],
    ) {
        let cfg = self.activation_cfg();
        let displays: Vec<_> = self
            .space_state
            .screens
            .iter()
            .map(|screen| screen.display_uuid_owned())
            .collect();
        let active_spaces =
            self.space_activation_policy.compute_active_spaces(cfg, spaces, &displays);
        let previous_active = self.active_spaces.clone();
        self.set_active_spaces(&active_spaces);
        self.handle_active_space_change(previous_active, invalidated_spaces);
    }

    pub(super) fn recompute_and_set_active_spaces_from_current_screens(&mut self) {
        let spaces: Vec<_> = self.space_state.screens.iter().map(|screen| screen.space).collect();
        self.recompute_active_spaces(&spaces, &[]);
    }

    pub(super) fn handle_active_space_change(
        &mut self,
        previous_active: HashSet<SpaceId>,
        invalidated_spaces: &[SpaceId],
    ) {
        if previous_active == self.active_spaces {
            return;
        }
        let activated: Vec<SpaceId> =
            self.active_spaces.difference(&previous_active).copied().collect();
        for space in &activated {
            self.expose_space_if_known(*space);
        }
        // Deactivation (e.g. a macOS Space switch) keeps assignments: windows without
        // app rules would otherwise be re-assigned to the current workspace.
        let active_windows = self.authoritative_active_space_windows();
        self.reconcile_authoritative_active_window_snapshot(
            active_windows,
            true,
            invalidated_spaces,
        );
        self.discover_windows_on_active_spaces();
        if !activated.is_empty() {
            self.apply_app_rules_for_activated_spaces(&activated);
        }
    }

    /// Admits windows found on spaces that just became active. A window already assigned
    /// there keeps its placement and needs no rules: a native space switch alone does not
    /// rerun them.
    pub(super) fn apply_app_rules_for_activated_spaces(&mut self, activated: &[SpaceId]) {
        let mut windows_by_pid: HashMap<pid_t, Vec<WindowId>> = HashMap::default();
        for (&wsid, space) in &self.space_state.active_window_spaces {
            if activated.contains(space)
                && let Some(wid) = self.state.windows.tracked_window_id(wsid)
                && self.state.windows.window(wid).is_some_and(|w| w.can_reconcile_admission())
                && self.assigned_space_for_window_id(wid) != Some(*space)
            {
                windows_by_pid.entry(wid.pid).or_default().push(wid);
            }
        }
        for (pid, window_ids) in windows_by_pid {
            if let Some(app) = self.app_manager.apps.get(&pid) {
                self.process_windows_for_app_rules(window_ids, app.info.clone(), false);
            }
        }
    }

    // Window membership

    /// Asks for the windows of each app with a window on an active space that was never
    /// discovered, as when it was created while its space was hidden. Activating a space
    /// needs no other inventory. Every app is asked if membership is incomplete. Windows
    /// of apps Rift does not manage, like the wallpaper the window server lists on every
    /// desktop, need no discovery.
    pub(super) fn discover_windows_on_active_spaces(&mut self) {
        if !self.space_state.membership_complete {
            self.request_window_inventories();
            return;
        }
        let mut pids: Vec<pid_t> = self
            .space_state
            .active_window_spaces
            .iter()
            .filter(|&(&wsid, &space)| {
                self.is_space_active(space) && self.state.windows.tracked_window_id(wsid).is_none()
            })
            .filter_map(|(&wsid, _)| {
                let info = self.state.windows.get_window_server_info(wsid);
                Some(info.or_else(|| window_server::get_window(wsid))?.pid)
            })
            .filter(|pid| self.app_manager.apps.contains_key(pid))
            .collect();
        pids.sort_unstable();
        pids.dedup();
        self.request_window_inventories_for(pids);
    }

    pub(super) fn authoritative_active_space_windows(
        &self,
    ) -> Vec<(WindowServerId, Option<SpaceId>)> {
        let mut membership: Vec<_> = self
            .space_state
            .active_window_spaces
            .iter()
            .filter(|(_, space)| self.is_space_active(**space))
            .map(|(&wsid, &space)| (wsid, Some(space)))
            .collect();
        membership.sort_by_key(|(wsid, _)| *wsid);
        membership
    }

    pub(super) fn reconcile_authoritative_active_window_snapshot(
        &mut self,
        active_windows: Vec<(WindowServerId, Option<SpaceId>)>,
        preserve_missing_assignments: bool,
        invalidated_spaces: &[SpaceId],
    ) {
        if active_windows.is_empty() && !self.space_state.membership_complete {
            return;
        }
        let resolved: Vec<_> = active_windows
            .iter()
            .map(|&(wsid, space)| {
                let space = self.resolve_native_space(wsid, space);
                if let Some(space) = space {
                    self.clear_pending_target_if_confirmed_space(wsid, space);
                }
                (wsid, space)
            })
            .collect();
        let missing = self.state.windows.reconcile_native_snapshot(&resolved, &self.active_spaces);
        for wsid in missing {
            let native = self.resolve_native_space(wsid, None);
            if let Some(space) = native.filter(|space| self.is_native_fullscreen_space(*space)) {
                // Gone fullscreen, ahead of the space's own report: it keeps its workspace.
                let home = self
                    .state
                    .windows
                    .tracked_window_id(wsid)
                    .and_then(|wid| self.assigned_space_for_window_id(wid));
                if let Some(wid) =
                    self.state.windows.observe_native_fullscreen(wsid, space, home, None)
                {
                    self.send_layout_event(LayoutEvent::WindowRemovedPreserveFloating(wid));
                }
                continue;
            }
            let inactive_target = native.filter(|space| {
                !self.is_space_active(*space) && window_server::space_is_user(space.get())
            });
            // A hidden workspace is not projected on screen. A missing native window
            // can be entering fullscreen before that desktop is identified; retain its
            // slot until positive membership or the app inventory resolves its lifecycle.
            let hidden_workspace = self.state.windows.tracked_window_id(wsid).is_some_and(|wid| {
                self.state.windows.workspace_info_for_window(wid).is_some_and(|assigned| {
                    self.layout_manager.layout_engine.workspaces().active_workspace(assigned.space)
                        != Some(assigned.workspace_id)
                })
            });
            if let Some((wid, target)) = self.state.windows.reconcile_native_absence(
                wsid,
                &self.active_spaces,
                inactive_target,
                preserve_missing_assignments || hidden_workspace,
            ) {
                if let Some(space) = target {
                    let preserve_ordinal = self
                        .assigned_space_for_window_id(wid)
                        .is_some_and(|assigned| invalidated_spaces.contains(&assigned));
                    self.reassign_window_to_authoritative_space(wid, space, preserve_ordinal);
                } else {
                    self.send_layout_event(LayoutEvent::WindowRemoved(wid));
                }
            }
        }
        self.reconcile_windows_in_authoritative_active_snapshot(
            &active_windows,
            invalidated_spaces,
        );
    }

    pub(super) fn reconcile_windows_in_authoritative_active_snapshot(
        &mut self,
        active_windows: &[(WindowServerId, Option<SpaceId>)],
        invalidated_spaces: &[SpaceId],
    ) {
        if self.refreshes_blocked() {
            self.defer_window_inventory_refresh();
            return;
        }

        let windows: Vec<_> = active_windows
            .iter()
            .filter_map(|&(wsid, observed_space)| {
                let wid = self.state.windows.tracked_window_id(wsid)?;
                let authoritative_space =
                    self.state.windows.window_server_space(wsid).or(observed_space)?;
                Some((wid, authoritative_space))
            })
            .collect();
        for (wid, authoritative_space) in windows {
            let preserve_ordinal = self
                .assigned_space_for_window_id(wid)
                .is_some_and(|space| invalidated_spaces.contains(&space));
            self.reassign_window_to_authoritative_space(wid, authoritative_space, preserve_ordinal);
        }
    }

    #[cfg(test)]
    pub(super) fn reconcile_windows_with_authoritative_spaces(&mut self) {
        let active_windows: Vec<_> = self
            .state
            .windows
            .iter_windows()
            .filter_map(|(wid, state)| {
                let wsid = state.info.sys_id?;
                Some((wsid, self.authoritative_space_for_window_id(wid)))
            })
            .collect();
        self.reconcile_windows_in_authoritative_active_snapshot(&active_windows, &[])
    }

    pub(super) fn apply_topology_window_delta(
        &mut self,
        delta: TopologyWindowDelta,
    ) -> EventOutcome {
        let appeared: HashMap<WindowServerId, SpaceId> = delta.appeared.into_iter().collect();
        let disappeared: HashMap<WindowServerId, SpaceId> = delta.disappeared.into_iter().collect();
        let wsids: HashSet<WindowServerId> =
            appeared.keys().chain(disappeared.keys()).copied().collect();
        let mut outcome = EventOutcome::default();

        for wsid in wsids {
            let appeared_space = appeared.get(&wsid).copied();
            let disappeared_space = disappeared.get(&wsid).copied();
            let authoritative_space = self.resolve_native_space(wsid, appeared_space);
            if let Some(target_space) = authoritative_space {
                self.state.windows.observe_native_space(
                    wsid,
                    target_space,
                    self.is_space_active(target_space),
                );
                if appeared_space == Some(target_space) {
                    self.clear_pending_target_if_confirmed_space(wsid, target_space);
                }
                if let Some(window) = self.state.windows.tracked_window_id(wsid) {
                    outcome =
                        self.reconcile_native_presence(wsid, target_space, window, true, outcome);
                }
            } else if let Some(previous_space) = disappeared_space {
                self.state.windows.observe_native_space(wsid, previous_space, false);
                if let Some(window) = self.state.windows.tracked_window_id(wsid)
                    && self.assigned_space_for_window_id(window) == Some(previous_space)
                    && self.is_space_active(previous_space)
                {
                    outcome = outcome
                        .with_layout_event(LayoutEvent::WindowRemovedPreserveFloating(window));
                }
            }
        }
        outcome
    }

    pub(super) fn reconcile_native_presence(
        &mut self,
        wsid: WindowServerId,
        space: SpaceId,
        mut window: WindowId,
        mut preserve_workspace_ordinal: bool,
        mut outcome: EventOutcome,
    ) -> EventOutcome {
        if let Some(restored) = self.state.windows.restore_native_identity(Some(wsid), window)
            && let Some(owner) = restored.window
        {
            if let Some(previous) = restored.removed_window {
                outcome = outcome.with_layout_event(LayoutEvent::WindowRemoved(previous));
            }
            outcome = outcome.with_window_inventory_request(owner.pid);
            window = owner;
            preserve_workspace_ordinal = false;
        }
        self.reassign_window_to_authoritative_space(window, space, preserve_workspace_ordinal);
        outcome
    }

    pub(super) fn reassign_window_to_authoritative_space(
        &mut self,
        wid: WindowId,
        space: SpaceId,
        preserve_workspace_ordinal: bool,
    ) {
        if !self.state.windows.reconcile_admission(wid) {
            self.send_layout_event(LayoutEvent::WindowRemoved(wid));
            return;
        }
        let assigned = self.state.windows.workspace_info_for_window(wid);
        if assigned.is_some_and(|assigned| assigned.space == space) {
            self.layout_manager.layout_engine.confirm_window_space(wid, space);
            if self.pending_native_returns.get(&wid) == Some(&space) {
                self.pending_native_returns.remove(&wid);
            }
        } else if !self.follow_native_space(wid, assigned, space, preserve_workspace_ordinal) {
            return;
        }
        if self.is_space_active(space) && self.state.windows.is_visible_admitted(wid) {
            self.send_layout_event(LayoutEvent::WindowAdded(space, wid));
        }
    }

    /// Assigns `wid` to `space`, where the window server now has it. Returns false if the
    /// window keeps its assignment.
    pub(super) fn follow_native_space(
        &mut self,
        wid: WindowId,
        assigned: Option<WindowWorkspaceInfo>,
        space: SpaceId,
        preserve_workspace_ordinal: bool,
    ) -> bool {
        let away_home = self.layout_manager.layout_engine.away_home(wid);
        if away_home.is_some()
            && self.space_state.screen_by_space(space).is_none()
            && !self.space_is_listed_on_connected_display(space)
        {
            // A returning desktop's inventory can precede its screen and Space remap.
            // Keep the current host until topology catches up: assigning to a provisional
            // workspace here makes the remap delete that host, which is then mistaken for
            // a user move and discards the preserved home tree. Inactive desktops of a
            // connected display remain valid destinations for an actual native-space move.
            return false;
        }
        // The first check above leaves only a home on a connected display, shown or not.
        if away_home.is_some_and(|(home, _)| home == space)
            && self
                .layout_manager
                .layout_engine
                .return_displaced_window(&mut self.state.windows, wid)
        {
            self.layout_manager.layout_engine.confirm_window_space(wid, space);
            return true;
        }
        if assigned.is_some_and(|assigned| {
            self.layout_manager.layout_engine.is_returning_home(wid, assigned.space)
        }) {
            // Hidden desktops need the same acknowledgement guard as shown desktops.
            return false;
        }
        if let Some(assigned) = assigned
            && self.space_state.screen_by_space(assigned.space).is_some()
        {
            let engine = &self.layout_manager.layout_engine;
            if self.main_display_changed_at.is_some_and(|at| at.elapsed() < MAIN_DISPLAY_CHURN)
                && engine.workspaces().active_workspace(assigned.space)
                    != Some(assigned.workspace_id)
            {
                // Changing the main display moves each desktop to another display, which for a
                // moment can leave a window hidden past a display's edge inside the other
                // display, and so on its desktop, in report after report. Nobody moved it; it is
                // hidden again where it belongs.
                return false;
            }
        }
        if self.pending_native_returns.contains_key(&wid)
            && self.topology_changed_at.is_some_and(|at| at.elapsed() < AFTERCARE)
        {
            return false;
        }
        if let Some(assigned) = assigned
            && away_home.is_none()
            && self.aftercare_windows.contains(&wid)
            && self.topology_changed_at.is_some_and(|at| at.elapsed() < AFTERCARE)
            && self.space_is_listed_on_connected_display(assigned.space)
            && !self.is_native_fullscreen_space(space)
            && self.space_state.display_space_ids.iter().any(|(home_display, spaces)| {
                spaces.contains(&assigned.space)
                    && self.space_state.display_space_ids.iter().any(|(other_display, spaces)| {
                        other_display != home_display && spaces.contains(&space)
                    })
            })
            && !self.is_in_drag()
            && !self.sent_back_after_display_change.contains(&wid)
            && let Some(wsid) = self.state.windows.window(wid).and_then(|w| w.info.sys_id)
            && wm_bridge::move_windows_to_space(&[wsid], assigned.space)
        {
            // The window server goes on moving windows for a few seconds after a display
            // change, putting some where they were the last time these displays were
            // attached. Nobody chose that, so the window is sent back to its desktop. A
            // window moved again is left where it is.
            self.sent_back_after_display_change.insert(wid);
            self.pending_native_returns.insert(wid, assigned.space);
            self.request_space_snapshot();
            debug!(?wid, from = ?space, to = ?assigned.space, "Sent back a window a display change moved");
            return false;
        }
        // A window carried off a departed display stays in its home tree, so the display's
        // layout is intact when it returns.
        let departing = away_home.is_none()
            && assigned.is_some_and(|assigned| self.space_display_departed(assigned.space));
        // A normal native move from a desktop that still exists on a surviving display is
        // the user's placement (for example in Mission Control). Automatic topology moves
        // preserve the workspace ordinal instead, or leave a desktop that is no longer listed.
        let placed_while_away = away_home.is_some()
            && !preserve_workspace_ordinal
            && assigned
                .is_some_and(|assigned| self.space_is_listed_on_connected_display(assigned.space));
        if !departing {
            self.send_layout_event(LayoutEvent::WindowRemovedPreserveFloating(wid));
        }
        let engine = &mut self.layout_manager.layout_engine;
        engine.workspaces_mut().list_workspaces(space);
        let moved = if preserve_workspace_ordinal {
            engine
                .workspaces_mut()
                .assign_window_to_workspace_preserving_ordinal(&mut self.state.windows, space, wid)
                .is_some()
        } else {
            let Some((workspace, _)) = engine.ensure_active_workspace_info(space) else {
                return false;
            };
            engine.workspaces_mut().assign_window_to_workspace(
                &mut self.state.windows,
                space,
                wid,
                workspace,
            )
        };
        let home = away_home.or_else(|| {
            assigned
                .filter(|_| departing)
                .map(|assigned| (assigned.space, assigned.workspace_id))
        });
        if moved {
            if placed_while_away {
                engine.forget_displaced_window(wid, true);
            } else if let Some(home) = home {
                engine.record_displaced_window(&self.state.windows, wid, home);
            }
        }
        moved
    }

    pub(super) fn space_is_listed_on_connected_display(&self, space: SpaceId) -> bool {
        self.space_state.screens.iter().any(|screen| {
            self.space_state
                .display_space_ids
                .get(&screen.display_uuid)
                .is_some_and(|spaces| spaces.contains(&space))
        })
    }

    /// Whether `space` belonged to a display that is no longer connected.
    pub(super) fn space_display_departed(&self, space: SpaceId) -> bool {
        self.layout_manager
            .layout_engine
            .display_uuid_for_space(space)
            .is_some_and(|display| {
                !self.space_state.screens.iter().any(|screen| screen.display_uuid == display)
            })
    }

    pub(super) fn defer_native_space_move(&mut self, wsid: WindowServerId, space: SpaceId) -> bool {
        let Some(wid) = self.state.windows.tracked_window_id(wsid) else {
            return false;
        };
        if self.is_known_fullscreen_window(wsid)
            || self.assigned_space_for_window_id(wid).is_none_or(|assigned| assigned == space)
        {
            return false;
        }
        self.state
            .windows
            .observe_native_space(wsid, space, self.is_space_active(space));
        self.request_space_snapshot();
        true
    }

    pub(super) fn request_space_snapshot(&self) {
        if let Some(tx) = &self.spaces_tx {
            tx.send(crate::actor::spaces::Event::ReconcileWindowSpaces);
        }
    }

    pub(super) fn clear_pending_hidden_window_targets(&self) {
        for (wid, window) in self.state.windows.iter_windows() {
            if self.hidden_assigned_space_for_window_id(wid).is_none() {
                continue;
            }
            if let Some(wsid) = window.info.sys_id {
                self.transaction_manager.clear_target_for_window(wsid);
            }
        }
    }

    pub(super) fn clear_pending_target_if_confirmed_space(
        &self,
        wsid: WindowServerId,
        confirmed_space: SpaceId,
    ) {
        if self.pending_target_space_for_window_server_id(wsid) == Some(confirmed_space) {
            self.transaction_manager.clear_target_for_window(wsid);
        }
    }

    // Space queries

    pub(crate) fn resolve_native_space(
        &self,
        wsid: WindowServerId,
        observation: Option<SpaceId>,
    ) -> Option<SpaceId> {
        let pending = self.pending_target_space_for_window_server_id(wsid);
        let live =
            if observation.is_none() || pending.is_some_and(|target| observation != Some(target)) {
                window_server::window_space(wsid)
            } else {
                None
            };
        let resolved = self.state.windows.resolve_native_space(wsid, observation, pending, live);
        trace!(
            ?wsid,
            ?observation,
            ?pending,
            ?live,
            ?resolved,
            "Resolved native space"
        );
        resolved
    }

    pub(super) fn pending_target_space_for_window_server_id(
        &self,
        wsid: WindowServerId,
    ) -> Option<SpaceId> {
        let wid = self.state.windows.tracked_window_id(wsid)?;
        let target_frame = self.transaction_manager.get_target_frame(wsid)?;
        let assigned_space = self.assigned_space_for_window_id(wid)?;
        let target_space = self
            .hidden_assigned_space(Some(wsid))
            .or_else(|| self.best_space_for_frame(&target_frame))?;
        (target_space == assigned_space).then_some(target_space)
    }

    pub(super) fn assigned_space_for_window_id(&self, wid: WindowId) -> Option<SpaceId> {
        self.state.windows.workspace_info_for_window(wid).map(|info| info.space)
    }

    pub(super) fn current_reported_space_for_window_id(&self, wid: WindowId) -> Option<SpaceId> {
        self.state
            .windows
            .window(wid)
            .and_then(|window| window.info.sys_id)
            .and_then(|wsid| self.resolve_native_space(wsid, None))
    }

    pub(super) fn authoritative_space_for_window_id(&self, wid: WindowId) -> Option<SpaceId> {
        self.current_reported_space_for_window_id(wid)
            .or_else(|| self.assigned_space_for_window_id(wid))
    }

    pub(super) fn best_space_for_window_id(&self, wid: WindowId) -> Option<SpaceId> {
        self.authoritative_space_for_window_id(wid).or_else(|| {
            self.state.windows.window(wid).and_then(|window| {
                self.best_space_for_window(&window.frame_monotonic, window.info.sys_id)
            })
        })
    }

    pub(super) fn best_space_for_window(
        &self,
        frame: &CGRect,
        window_server_id: Option<WindowServerId>,
    ) -> Option<SpaceId> {
        self.space_for_window_observation(frame, window_server_id, || {
            window_server_id.and_then(|wsid| self.resolve_native_space(wsid, None))
        })
    }

    /// The space a frame puts a window on, without asking the window server.
    pub(crate) fn geometry_space_for_window(
        &self,
        frame: &CGRect,
        window_server_id: Option<WindowServerId>,
    ) -> Option<SpaceId> {
        self.space_for_window_observation(frame, window_server_id, || None)
    }

    /// A window's space: `native` membership, else its hidden assignment, else the
    /// screen under its frame. Fullscreen windows have none.
    pub(super) fn space_for_window_observation(
        &self,
        frame: &CGRect,
        wsid: Option<WindowServerId>,
        native: impl FnOnce() -> Option<SpaceId>,
    ) -> Option<SpaceId> {
        if wsid.is_some_and(|wsid| self.is_known_fullscreen_window(wsid)) {
            return None;
        }
        native()
            .or_else(|| self.hidden_assigned_space(wsid))
            .or_else(|| self.best_space_for_frame(frame))
    }

    pub(super) fn best_space_for_frame(&self, frame: &CGRect) -> Option<SpaceId> {
        let center = frame.mid();
        self.screen_for_point(center).and_then(|screen| screen.space).or_else(|| {
            self.space_state
                .screens
                .iter()
                .filter_map(|screen| {
                    let space = screen.space?;
                    let area = screen.frame.intersection(frame).area() as i64;
                    if area > 0 { Some((area, space)) } else { None }
                })
                .max_by_key(|(area, _)| *area)
                .map(|(_, space)| space)
        })
    }

    /// The assigned space of a window parked in an inactive workspace of an active space.
    pub(super) fn hidden_assigned_space(&self, wsid: Option<WindowServerId>) -> Option<SpaceId> {
        let wid = self.state.windows.tracked_window_id(wsid?)?;
        let space = self.assigned_space_for_window_id(wid)?;
        (self.is_space_active(space) && self.window_in_non_active_workspace(space, wid))
            .then_some(space)
    }

    pub(super) fn hidden_assigned_space_for_window_id(&self, wid: WindowId) -> Option<SpaceId> {
        self.hidden_assigned_space(self.state.windows.window(wid)?.info.sys_id)
    }

    pub(super) fn discovery_spaces_for_window(
        &self,
        wid: WindowId,
        native: Option<SpaceId>,
    ) -> (Option<SpaceId>, Option<SpaceId>) {
        let authoritative = native.or_else(|| self.assigned_space_for_window_id(wid));
        let discovery = self.assigned_space_for_window_id(wid).or(authoritative).or_else(|| {
            let window = self.state.windows.window(wid)?;
            self.best_space_for_frame(&window.frame_monotonic).filter(|space| {
                self.is_space_active(*space)
                    || !window.info.sys_id.is_some_and(|wsid| self.is_known_fullscreen_window(wsid))
            })
        });
        // A placeholder assignment supplies ownership but cannot admit an AX window.
        (
            authoritative,
            discovery.filter(|_| self.state.windows.contains_window(wid)),
        )
    }

    pub(super) fn is_known_fullscreen_window(&self, wsid: WindowServerId) -> bool {
        self.state.windows.is_window_server_id_native_fullscreen_suspended(wsid)
    }

    pub(super) fn is_fullscreen_space(&self, space: SpaceId) -> bool {
        self.space_state.fullscreen_spaces.contains(&space)
    }

    /// Like `is_fullscreen_space`, but also asks the window server, which knows a fullscreen
    /// space before the space actor reports it.
    pub(super) fn is_native_fullscreen_space(&self, space: SpaceId) -> bool {
        self.is_fullscreen_space(space) || window_server::space_is_fullscreen(space.get())
    }

    pub(super) fn display_uuid_for_space(&self, space: SpaceId) -> Option<String> {
        self.space_state
            .screen_by_space(space)
            .and_then(|screen| screen.display_uuid_owned())
    }

    #[cfg(test)]
    pub(super) fn raw_spaces_for_current_screens(&self) -> Vec<Option<SpaceId>> {
        self.space_state.screens.iter().map(|s| s.space).collect()
    }

    pub(crate) fn visible_spaces_for_layout(
        &self,
        include_inactive: bool,
    ) -> (Vec<SpaceId>, HashMap<SpaceId, CGRect>) {
        let visible: Vec<(SpaceId, CGRect)> = self
            .space_state
            .screens
            .iter()
            .filter_map(|screen| Some((screen.space?, screen.frame)))
            .filter(|(space, _)| include_inactive || self.is_space_active(*space))
            .collect();
        let ordered = order_visible_spaces_by_position(
            visible.iter().map(|(space, frame)| (*space, frame.mid())),
        );
        (ordered, visible.into_iter().collect())
    }

    pub(super) fn raw_command_space(&self) -> Option<SpaceId> { self.space_state.command_space }

    pub(super) fn active_display_space(&self) -> Option<SpaceId> {
        self.raw_command_space()
            .filter(|space| self.space_state.is_space_on_screen(*space))
            .or_else(|| self.space_state.first_known_space())
    }

    pub(super) fn workspace_command_space(&self) -> Option<SpaceId> {
        self.active_display_space().filter(|space| self.is_space_active(*space))
    }

    pub(super) fn has_user_space_context(&self) -> bool {
        self.raw_command_space().is_some_and(|space| !self.is_fullscreen_space(space))
    }
}

/// Folds a snapshot deferred (during Mission Control or an inconclusive membership
/// query) into a newer one. The same revision keeps the deferred snapshot with the
/// newest display focus; a newer revision keeps its own geometry and membership
/// while inheriting the deferred resizes, change flags, and window delta. Space
/// remaps need no carrying: they are inferred again from the applied state.
fn merge_deferred_snapshot(
    mut pending: ForwardedSpaceState,
    mut snapshot: ForwardedSpaceState,
) -> ForwardedSpaceState {
    if pending.revision == snapshot.revision {
        pending.command_space = snapshot.command_space;
        pending.menu_bar_space = snapshot.menu_bar_space;
        return pending;
    }
    pending
        .resized_spaces
        .retain(|(space, _)| !snapshot.resized_spaces.iter().any(|(new, _)| space == new));
    pending.resized_spaces.append(&mut snapshot.resized_spaces);
    snapshot.resized_spaces = pending.resized_spaces;
    snapshot.should_force_refresh_layout |= pending.should_force_refresh_layout;
    snapshot.display_set_changed |= pending.display_set_changed;
    snapshot.topology_window_delta =
        match (pending.topology_window_delta, snapshot.topology_window_delta) {
            (Some(mut older), Some(newer)) => {
                // The newer delta has the last word on any window it mentions.
                let mentioned: HashSet<WindowServerId> = newer
                    .appeared
                    .iter()
                    .chain(&newer.disappeared)
                    .map(|(wsid, _)| *wsid)
                    .collect();
                older.appeared.retain(|(wsid, _)| !mentioned.contains(wsid));
                older.disappeared.retain(|(wsid, _)| !mentioned.contains(wsid));
                older.appeared.extend(newer.appeared);
                older.disappeared.extend(newer.disappeared);
                older.flags |= newer.flags;
                Some(older)
            }
            (older, newer) => newer.or(older),
        };
    snapshot
}
