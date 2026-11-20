use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use objc2_core_foundation::CGRect;
use rift_protocol::StackInfo;
use tracing::trace;

use super::changes::SpaceChange;
use super::replay::Record;
use super::{AppState, Changes, Event, Presentation, WorkspaceSwitchOrigin};
use crate::actor;
use crate::actor::app::{AppThreadHandle, WindowId, WindowInventoryToken, pid_t};
use crate::actor::reactor::Reactor;
use crate::actor::reactor::animation::AnimationManager;
use crate::actor::spaces::ForwardedSpaceState;
use crate::actor::{input, menu_bar, raise_manager, stack_line, window_notify, wm_controller};
use crate::common::collections::{HashMap, HashSet};
use crate::common::config::{DragDropSettings, LayoutMode};
use crate::layout_engine::LayoutEngine;
use crate::layout_engine::engine::GroupContainerInfo;
use crate::model::broadcast::{BroadcastEvent, BroadcastSender, protocol_workspace_id};
use crate::sys::geometry::Round;
use crate::sys::screen::SpaceId;

/// Manages application state and rules
pub struct AppManager {
    pub apps: HashMap<pid_t, AppState>,
}

impl AppManager {
    pub fn new() -> Self { AppManager { apps: HashMap::default() } }

    pub fn reject_duplicate(&self, pid: pid_t, handle: &AppThreadHandle) -> bool {
        let Some(existing) = self.apps.get(&pid) else {
            return false;
        };
        tracing::error!(pid, "Duplicate app actor registration; retaining original actor");
        if !existing.handle.same_actor(handle) {
            _ = handle.send(crate::actor::app::Request::Terminate);
        }
        true
    }
}

/// Manages drag operations and window swapping
pub struct DragManager {
    pub actor: crate::actor::drag::DragActor,
    pub native_motion_active: Arc<AtomicBool>,
    pub(super) preview: Option<crate::ui::drag_preview::DragPreview>,
    pub(super) preview_enabled: bool,
    pub(super) haptics_enabled: bool,
    // Layouts suppressed across native drag events resume when the drag ends.
    pub(super) deferred_layouts: Vec<(Option<SpaceId>, Presentation, bool)>,
}

impl DragManager {
    pub(super) fn defer_layout(
        &mut self,
        space: Option<SpaceId>,
        policy: Presentation,
        changed: bool,
    ) {
        if let Some((_, current, layout_changed)) =
            self.deferred_layouts.iter_mut().find(|(id, ..)| *id == space)
        {
            *current = (*current).max(policy);
            *layout_changed |= changed;
        } else {
            self.deferred_layouts.push((space, policy, changed));
        }
    }

    pub fn sync_motion_gate(&self) {
        let active = self.actor.drop_enabled()
            && self.actor.kind() == Some(crate::actor::drag::DragKind::NativeMove);
        if self.native_motion_active.load(Ordering::Relaxed) != active {
            self.native_motion_active.store(active, Ordering::Release);
        }
    }

    /// A window leaving Rift's control can no longer be dragged by it.
    pub fn forget_window(&mut self, wid: WindowId) {
        self.actor.window_removed(wid);
        self.sync_motion_gate();
        self.sync_preview();
    }

    pub fn reset(&mut self) {
        self.actor.cancel();
        self.sync_motion_gate();
        self.release_preview();
    }

    pub fn update_config(&mut self, config: DragDropSettings) {
        self.haptics_enabled = config.haptics_enabled;
        self.actor.update_config(config);
        self.preview_enabled = config.enabled && config.preview;
        if !config.enabled || !config.preview {
            self.release_preview();
        }
        self.sync_preview();
    }

    pub fn sync_preview(&mut self) {
        if cfg!(test) {
            return;
        }
        let Some(target) = self.actor.preview_target().filter(|_| self.preview_enabled) else {
            self.hide_preview();
            return;
        };
        let result = if let Some(preview) = &mut self.preview {
            preview.show(target)
        } else {
            crate::ui::drag_preview::DragPreview::new(target).and_then(|mut preview| {
                preview.show(target)?;
                self.preview = Some(preview);
                Ok(())
            })
        };
        if let Err(error) = result {
            tracing::warn!(?error, "failed to update drag preview");
            self.release_preview();
        }
    }

    pub fn hide_preview(&mut self) {
        if let Some(preview) = &mut self.preview {
            preview.hide();
        }
    }

    pub fn release_preview(&mut self) { drop(self.preview.take()); }
}

/// Manages window notifications
pub struct NotificationManager {
    pub last_sls_notification_ids: Vec<u32>,
    pub last_layout_modes_by_space: HashMap<SpaceId, crate::common::config::LayoutMode>,
    pub _window_notify_tx: Option<window_notify::Sender>,
}

/// Manages menu state and interactions
pub struct MenuManager {
    pub menu_state: super::MenuState,
    pub menu_tx: Option<menu_bar::Sender>,
    pub last_projection_signature: Option<u64>,
}

/// Manages Mission Control state
pub struct MissionControlManager {
    pub mission_control_state: super::MissionControlState,
}

/// Owns ordering and coalescing for asynchronous AX window inventories.
pub struct WindowInventoryManager {
    pub next_request_id: u64,
    pub in_flight: HashMap<pid_t, WindowInventoryToken>,
    pub pending: HashSet<pid_t>,
    pub refocus_after_refresh: HashMap<pid_t, WindowId>,
}

/// Manages workspace switching state
pub struct WorkspaceSwitchManager {
    pub active_workspace_switch: Option<SpaceId>,
    pub pending_workspace_switch_origin: Option<WorkspaceSwitchOrigin>,
}

impl WorkspaceSwitchManager {
    pub fn start_workspace_switch(&mut self, space: SpaceId, origin: WorkspaceSwitchOrigin) {
        self.active_workspace_switch = Some(space);
        self.pending_workspace_switch_origin = Some(origin);
    }

    pub fn manual_switch_in_progress(&self) -> bool {
        self.pending_workspace_switch_origin == Some(WorkspaceSwitchOrigin::Manual)
    }

    pub fn mark_workspace_switch_inactive(&mut self) {
        self.pending_workspace_switch_origin = None;
    }
}

/// Manages refocus and cleanup state
pub struct RefocusManager {
    pub stale_cleanup_state: super::StaleCleanupState,
    pub refocus_state: super::RefocusState,
}

/// Manages communication channels to other actors
pub struct CommunicationManager {
    pub input_tx: Option<input::Sender>,
    pub stack_line_tx: Option<stack_line::Sender>,
    pub raise_manager_tx: raise_manager::Sender,
    pub event_broadcaster: BroadcastSender,
    pub wm_sender: Option<wm_controller::Sender>,
    pub events_tx: Option<actor::Sender<Event>>,
}

/// Manages recording state
pub struct RecordingManager {
    pub record: Record,
}

/// Manages layout engine state
pub struct LayoutManager {
    pub layout_engine: LayoutEngine,
}

pub(super) use crate::model::hidden_window_placement::ScrollingDisplayBounds;

struct CalculatedLayout {
    space: SpaceId,
    presentation: Presentation,
    bounds: Option<ScrollingDisplayBounds>,
    frames: Vec<(WindowId, CGRect)>,
    groups: Vec<GroupContainerInfo>,
    change: SpaceChange,
    placed: usize,
}

impl LayoutManager {
    fn publish_stack_tabs(
        reactor: &Reactor,
        tx: &stack_line::Sender,
        space: SpaceId,
        group_infos: &[GroupContainerInfo],
        active_workspace_for_space_has_fullscreen: bool,
        reorder: bool,
    ) {
        let groups: Vec<stack_line::GroupInfo> = group_infos
            .iter()
            .map(|g| stack_line::GroupInfo {
                node_id: g.node_id,
                space_id: space,
                frame: g.frame,
                horizontal: g.container_kind != crate::layout_engine::LayoutKind::VerticalStack,
                selected_index: g.selected_index,
                tabs: g
                    .window_ids
                    .iter()
                    .map(|&window_id| {
                        let info = reactor.state.windows.window(window_id).map(|w| &w.info);
                        stack_line::TabInfo {
                            window_id,
                            title: info.map(|i| i.title.clone()).unwrap_or_default(),
                            bundle_id: info.and_then(|i| i.bundle_id.clone()),
                            window_server_id: reactor
                                .state
                                .windows
                                .record(window_id)
                                .and_then(|record| record.window_server_id()),
                        }
                    })
                    .collect(),
            })
            .collect();
        trace!(?space, count = groups.len(), "Publishing stack tabs");
        if let Err(e) = tx.try_send(stack_line::Event::GroupsUpdated {
            active_space_ids: reactor.iter_active_spaces().collect(),
            space_id: space,
            groups,
            active_workspace_for_space_has_fullscreen,
            reorder,
        }) {
            tracing::warn!("Failed to send groups update to stack_line: {}", e);
        }
    }

    pub(super) fn refresh_stack_tabs(reactor: &mut Reactor, space: SpaceId) {
        if !reactor.config.settings.ui.stack_line.enabled || !reactor.is_space_active(space) {
            return;
        }
        let Some(tx) = &reactor.communication_manager.stack_line_tx else {
            return;
        };
        let Some(screen) = reactor.space_state.screen_by_space(space) else {
            return;
        };
        let gaps = reactor
            .config
            .settings
            .layout
            .gaps
            .effective_for_display(screen.display_uuid_opt());
        let groups = reactor.layout_manager.layout_engine.collect_group_containers(
            space,
            screen.frame,
            &gaps,
            reactor.config.settings.ui.stack_line.reserve(),
        );
        let fullscreen = reactor
            .layout_manager
            .layout_engine
            .active_workspace_for_space_has_fullscreen(space);
        Self::publish_stack_tabs(reactor, tx, space, &groups, fullscreen, true);
    }

    pub(super) fn commit(reactor: &mut Reactor, changes: &mut Changes) -> bool {
        if reactor.refreshes_blocked() || reactor.is_mission_control_active() {
            return false;
        }
        let publish_stacks = !reactor.communication_manager.event_broadcaster.is_closed();
        let tabs_enabled = reactor.config.settings.ui.stack_line.enabled;
        let collect_groups = publish_stacks
            || (tabs_enabled && reactor.communication_manager.stack_line_tx.is_some());
        let screens = &reactor.space_state.screens;
        let inline_frames;
        let extra_frames;
        let all_screen_frames = match screens.as_slice() {
            [] => &[],
            [screen] => {
                inline_frames = [screen.frame, CGRect::ZERO];
                &inline_frames[..1]
            }
            [left, right] => {
                inline_frames = [left.frame, right.frame];
                &inline_frames[..]
            }
            _ => {
                extra_frames = screens.iter().map(|screen| screen.frame).collect::<Vec<_>>();
                &extra_frames[..]
            }
        };
        let has_windows = reactor.state.windows.tracked_window_count() != 0;
        let mut any_frame_changed = false;
        let skip_wid = reactor.drag_manager.actor.source().map(|source| source.window);
        let mut inline = [None, None];
        let mut rest = Vec::new();

        for screen in screens {
            let (Some(space), frame) = (screen.space, screen.frame) else {
                continue;
            };
            let change = changes.for_space(space);
            let Some(mut presentation) = change.presentation else {
                continue;
            };
            if !reactor.is_space_active(space) {
                continue;
            }
            if reactor.workspace_switch_manager.active_workspace_switch == Some(space)
                && matches!(presentation, Presentation::Animated | Presentation::Resize)
            {
                presentation = Presentation::Instant;
            }
            if reactor.overview_focus.is_some_and(|(covered, _, started)| {
                covered == space && started.elapsed() < std::time::Duration::from_secs(1)
            }) {
                presentation = presentation.max(Presentation::Instant);
            }
            let mut update = CalculatedLayout {
                space,
                presentation,
                bounds: None,
                frames: Vec::new(),
                groups: Vec::new(),
                change,
                placed: 0,
            };
            if has_windows {
                let display_uuid = screen.display_uuid_opt();
                let gaps = reactor.config.settings.layout.gaps.effective_for_display(display_uuid);
                // Accepted topology owns display information. Geometry uses it without
                // refreshing the engine's display mapping on every frame commit.
                #[cfg(test)]
                reactor.layout_calculations.push(space);
                trace!(?space, ?presentation, "Calculating reactor layout");
                let mut layout = reactor.layout_manager.layout_engine.calculate_layout_with_groups(
                    &reactor.state.windows,
                    space,
                    frame,
                    &gaps,
                    reactor.config.settings.ui.stack_line.reserve(),
                    |wid| reactor.state.windows.window(wid).map(|w| w.frame_monotonic),
                    all_screen_frames,
                    collect_groups.then_some(&mut update.groups),
                    matches!(presentation, Presentation::Instant | Presentation::Force),
                );
                let engine = &reactor.layout_manager.layout_engine;
                let bounds = (engine.active_layout_mode_at(space) == LayoutMode::Scrolling)
                    .then(|| {
                        ScrollingDisplayBounds::for_screen(frame, all_screen_frames.iter().copied())
                    })
                    .flatten();
                if let Some(bounds) = bounds
                    && let Some(workspace) = engine.workspaces().active_workspace(space)
                {
                    for (window, frame) in &mut layout {
                        if reactor.state.windows.workspace_for_window(*window) == Some(workspace)
                            && !engine.is_window_floating(*window)
                        {
                            *frame = bounds.apply(*frame);
                        }
                    }
                }
                update.frames = layout;
                update.bounds = bounds;
            }
            if let Some(slot) = inline.iter_mut().find(|slot| slot.is_none()) {
                *slot = Some(update);
            } else {
                rest.push(update);
            }
        }

        // All spaces have sampled the same reconciled frames. Install native
        // placements before delivering any ordinary layout, including on the source.
        for (window, target, fallback) in changes.tiled_transfers() {
            let Some(update) = inline
                .iter_mut()
                .flatten()
                .chain(&mut rest)
                .find(|layout| layout.space == target)
            else {
                continue;
            };
            if !reactor
                .state
                .windows
                .record(window)
                .and_then(|record| record.pending_transfer())
                .is_some_and(|operation| operation.requested_space == target)
                || reactor.assigned_space_for_window_id(window) != Some(target)
            {
                continue;
            }
            // An intervening switch may have parked this window. Place it on
            // its physical display first, then apply its separate parking frame.
            let active = reactor
                .layout_manager
                .layout_engine
                .workspaces()
                .is_window_in_active_workspace(&reactor.state.windows, target, window);
            let tile = active
                .then(|| update.frames.iter().position(|(wid, _)| *wid == window))
                .flatten();
            let frame = tile.map(|idx| update.frames[idx].1.round()).or(fallback);
            let bounds = tile.and(update.bounds);
            if reactor.send_transfer_frame(window, target, frame, true, bounds) {
                any_frame_changed = true;
                if let Some(idx) = tile {
                    // Keep the complete projection for IPC while excluding the
                    // already submitted tile even from a forced frame pass.
                    update.frames.swap(update.placed, idx);
                    update.placed += 1;
                }
            }
        }
        for update in inline.iter_mut().flatten().chain(&mut rest) {
            if update.frames.is_empty() {
                continue;
            }
            let changed = AnimationManager::apply_layout(
                reactor,
                update.space,
                &update.frames[update.placed..],
                update.presentation,
                skip_wid,
                update.bounds,
            );
            any_frame_changed |= changed;
            update.change.layout_changed |= changed && update.presentation != Presentation::Force;
        }

        let active_space = publish_stacks.then(|| reactor.workspace_command_space()).flatten();
        for CalculatedLayout {
            space,
            frames: layout,
            groups,
            change,
            ..
        } in inline.into_iter().flatten().chain(rest)
        {
            reactor.publish_space_changes(space, change);
            let stack_line =
                reactor.communication_manager.stack_line_tx.as_ref().filter(|_| tabs_enabled);
            let stack_has_fullscreen = (stack_line.is_some() || !groups.is_empty())
                && reactor
                    .layout_manager
                    .layout_engine
                    .active_workspace_for_space_has_fullscreen(space);
            let active_workspace_for_space_has_fullscreen =
                active_space == Some(space) && stack_has_fullscreen;
            if let Some(tx) = stack_line {
                Self::publish_stack_tabs(reactor, tx, space, &groups, stack_has_fullscreen, false);
            }

            if !groups.is_empty()
                && !reactor.communication_manager.event_broadcaster.is_closed()
                && let Some(workspace_id) =
                    reactor.layout_manager.layout_engine.workspaces().active_workspace(space)
            {
                let workspace_index =
                    reactor.layout_manager.layout_engine.workspaces().active_workspace_idx(space);
                let workspace_name = reactor
                    .layout_manager
                    .layout_engine
                    .workspace_name(space, workspace_id)
                    .unwrap_or_else(|| format!("Workspace {:?}", workspace_id));

                let stacks: Vec<StackInfo> = groups
                    .iter()
                    .map(|g| {
                        // Clients have always received vertical stacks bottom to top.
                        let mut windows: Vec<_> =
                            g.window_ids.iter().map(WindowId::to_debug_string).collect();
                        let mut selected_index = g.selected_index;
                        if g.container_kind == crate::layout_engine::LayoutKind::VerticalStack {
                            windows.reverse();
                            selected_index =
                                windows.len().saturating_sub(1).saturating_sub(selected_index);
                        }
                        StackInfo {
                            total_count: g.total_count,
                            container_kind: g.container_kind,
                            selected_index,
                            windows,
                        }
                    })
                    .collect();

                let event = BroadcastEvent::StacksChanged {
                    workspace_id: protocol_workspace_id(workspace_id),
                    workspace_index,
                    workspace_name,
                    stacks,
                    active_workspace_has_fullscreen: active_workspace_for_space_has_fullscreen,
                    space_id: space.get(),
                    display_uuid: reactor.display_uuid_for_space(space),
                };
                reactor.publish(event);
            }

            if change.layout_changed {
                reactor.broadcast_layout_state_changed(
                    space,
                    rift_protocol::EventKind::LayoutChanged,
                    Some(&layout),
                );
            }
            if changes.selection_changed == Some(space) {
                reactor.broadcast_layout_state_changed(
                    space,
                    rift_protocol::EventKind::SelectionChanged,
                    Some(&layout),
                );
                changes.selection_changed = None;
            }
            if let Some(window) = changes.focus_warp
                && changes.display_warp.is_none()
                && reactor.layout_manager.layout_engine.active_layout_mode_at(space)
                    == LayoutMode::Scrolling
                && let Some((_, frame)) = layout.iter().find(|(wid, _)| *wid == window)
            {
                changes.focus_warp = None;
                if reactor.screen_for_point(frame.mid()).is_some() {
                    reactor.warp_mouse(frame.mid());
                }
            }
        }

        any_frame_changed
    }
}

/// Manages pending space changes
pub struct PendingSpaceChangeManager {
    pub pending_space_change: Option<ForwardedSpaceState>,
}
