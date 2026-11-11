use objc2_core_foundation::CGRect;
use tracing::debug;

use crate::actor::app::WindowId;
use crate::actor::reactor::events::EventOutcome;
use crate::actor::reactor::managers::DragManager;
use crate::actor::reactor::transaction_manager::TransactionManager;
use crate::actor::reactor::{Quiet, TransactionId, WindowState, utils};
use crate::layout_engine::LayoutEvent;
use crate::model::WindowVisibility;
use crate::sys::app::WindowInfo as Window;
use crate::sys::event::MouseState;
use crate::sys::geometry::{IsWithin, SameAs};
use crate::sys::screen::SpaceId;
use crate::sys::window_server::WindowServerInfo;

#[derive(Debug)]
pub struct WindowCreatedPayload {
    pub window_id: WindowId,
    pub window: Window,
    pub window_server_info: Option<WindowServerInfo>,
}

pub fn handle_window_created(
    state: &mut crate::model::RiftState,
    layout: &mut crate::actor::reactor::managers::LayoutManager,
    payload: WindowCreatedPayload,
) -> anyhow::Result<EventOutcome> {
    let WindowCreatedPayload {
        window_id: wid,
        window,
        window_server_info: ws_info,
    } = payload;
    if let Some(previous) = state.windows.reconcile_ax_identity(wid, window.sys_id) {
        layout.layout_engine.transfer_persistent_window_identity(previous, wid);
    }
    if let Some(info) = ws_info {
        state.windows.clear_window_server_observed(info.id);
        state.windows.track_window_server_info(info);
    }

    let rule_override = state.windows.window(wid).and_then(|window| window.manage_override);
    let mut window_state: WindowState = window.into();
    window_state.manage_override = rule_override;

    state.windows.insert_window(wid, window_state);
    let _ = utils::refresh_heuristic(state, wid);

    // Excluded and claimed windows only need lifecycle notifications; app-rule admission
    // arranges itself.
    let outcome = EventOutcome::window_membership_changed(false, true)
        .with_arrange_passes(state.windows.is_admitted(wid).into());
    Ok(if state.windows.can_reconcile_admission(wid) {
        outcome.with_created_window_finalization(wid)
    } else {
        outcome
    })
}

#[derive(Debug, Clone, Copy)]
pub struct WindowDestroyedPayload {
    pub window: WindowId,
}

pub fn handle_window_destroyed(
    state: &mut crate::model::RiftState,
    transactions: &TransactionManager,
    drag: &mut DragManager,
    payload: WindowDestroyedPayload,
) -> EventOutcome {
    let wid = payload.window;
    if state.windows.suspend_fullscreen_ax_identity(wid) {
        return EventOutcome::no_change().with_layout_event(LayoutEvent::WindowSuspended(wid));
    }
    let Some(record) = state.windows.remove_window(wid) else {
        return EventOutcome::no_change();
    };
    apply_window_retirement(transactions, drag, (wid, record.window_server_id()))
}

pub(crate) fn apply_window_retirement(
    transactions: &TransactionManager,
    drag: &mut DragManager,
    (wid, wsid): (WindowId, Option<crate::sys::window_server::WindowServerId>),
) -> EventOutcome {
    if let Some(wsid) = wsid {
        transactions.remove_for_window(wsid);
    }
    drag.forget_window(wid);
    EventOutcome::window_membership_changed(true, false)
        .with_arrange_passes(0)
        .with_layout_event(LayoutEvent::WindowRemoved(wid))
}

/// Application hiding and AX minimization share one suspension/return lifecycle.
pub fn handle_window_visibility_changed(
    state: &mut crate::model::RiftState,
    wid: WindowId,
    visible: bool,
) -> anyhow::Result<crate::actor::reactor::events::EventOutcome> {
    if !visible {
        if !state.windows.observe_minimized(wid) {
            return Ok(EventOutcome::no_change());
        }
        return Ok(EventOutcome::window_membership_changed(false, false)
            .with_arrange_passes(0)
            .with_layout_event(LayoutEvent::WindowSuspended(wid)));
    }
    let ax_returned = state.windows.validate_ax_identity(wid);
    match state.windows.window_mut(wid) {
        Some(window) => {
            if !window.info.is_minimized && !ax_returned {
                return Ok(crate::actor::reactor::events::EventOutcome::no_change());
            }
            window.info.is_minimized = false;
        }
        None => {
            debug!(
                ?wid,
                "Received WindowDeminiaturized for unknown window - ignoring"
            );
            return Ok(crate::actor::reactor::events::EventOutcome::no_change());
        }
    }
    let _ = utils::refresh_heuristic(state, wid);
    // Minimizing ordered the window out; restoring orders it back in. Without
    // native visibility, inventory treats it as an AX-only fallback and skips
    // it while other windows of the app are visible.
    match state.windows.record(wid).and_then(|record| record.window_server_id()) {
        Some(wsid) => {
            state.windows.mark_window_visible(wsid);
        }
        None => state.windows.set_visibility(wid, WindowVisibility::Visible),
    }

    // Minimized AX snapshots can have stale admission metadata. Refresh them
    // even when the cached snapshot cannot currently be admitted.
    let mut outcome = EventOutcome::no_change().with_window_inventory_request(wid.pid);
    if !state.windows.is_claimed(wid) {
        // Reconcile admission while retaining the existing workspace and layout slot.
        // A claimed window returns on release.
        // WindowAdded restores membership only; the restore transition owns
        // the arrange pass, even if the following inventory is unchanged.
        outcome = outcome
            .with_created_window_finalization(wid)
            .with_arrange_passes(state.windows.is_admitted(wid).into());
    }
    Ok(outcome)
}

#[derive(Debug)]
pub struct WindowFrameChangedPayload {
    pub window: WindowId,
    pub new_frame: CGRect,
    pub mouse_state: Option<MouseState>,
    pub old_space: Option<SpaceId>,
    pub new_space: Option<SpaceId>,
    pub old_space_active: bool,
    pub new_space_active: bool,
    pub active_resize_space: Option<SpaceId>,
    pub pending_target_space: Option<SpaceId>,
    pub assigned_space: Option<SpaceId>,
    pub keep_assigned_for_scrolling: bool,
    pub screens: Vec<(SpaceId, CGRect, Option<String>)>,
}

pub enum FrameChangeDisposition {
    Handled,
    NeedsGeometryAnalysis,
    Reconcile(TransactionId),
}

pub fn classify_window_frame_change(
    state: &mut crate::model::RiftState,
    transactions: &TransactionManager,
    drag: &mut DragManager,
    wid: WindowId,
    new_frame: CGRect,
    last_seen: Option<TransactionId>,
    requested: bool,
    mouse_state: &mut Option<MouseState>,
    mission_control_active: bool,
) -> FrameChangeDisposition {
    let Some(window) = state.windows.window(wid) else {
        query_mouse_for_active_drag(drag, mouse_state);
        return FrameChangeDisposition::Handled;
    };
    let server_id = window.info.sys_id;
    let old_frame = window.info.frame;
    let native_intent = !requested
        && *mouse_state == Some(MouseState::Down)
        && drag.native_geometry_intent(wid, new_frame).is_some();
    let transaction = server_id.and_then(|server| transactions.store.get(&server));
    // Generations fence acknowledgements even after a target has settled.
    if let (Some(record), Some(seen)) = (transaction, last_seen)
        && seen != record.txid
        && !native_intent
        && (requested || (!record.settled && record.target.is_some()))
    {
        query_mouse_for_active_drag(drag, mouse_state);
        return FrameChangeDisposition::Handled;
    }
    if mission_control_active {
        drag.reset();
        if let Some(window) = state.windows.window_mut(wid) {
            window.info.frame = new_frame;
        }
        return FrameChangeDisposition::Handled;
    }
    if native_intent {
        if drag.actor.source().is_none()
            && let Some(server) = server_id
        {
            // Retire queued writes/acknowledgements before the user's first frame.
            transactions.generate_next_txid(server);
        }
        return FrameChangeDisposition::NeedsGeometryAnalysis;
    }
    if let Some(server) = server_id
        && let Some(record) = transaction
        && let Some(target) = record.target
        && (!record.settled || (requested && last_seen.is_some()))
    {
        // Clamp only to known application constraints, never infer a new
        // semantic width from an intermediate or rejected frame.
        let mut accepted = target;
        if let Some(min) = window.info.min_size {
            accepted.size.width = accepted.size.width.max(min.width);
            accepted.size.height = accepted.size.height.max(min.height);
        }
        if let Some(max) = window.info.max_size {
            if max.width > 0.0 {
                accepted.size.width = accepted.size.width.min(max.width);
            }
            if max.height > 0.0 {
                accepted.size.height = accepted.size.height.min(max.height);
            }
        }
        let satisfied = new_frame.is_within(0.5, accepted);
        // After settlement a duplicate terminal report may describe an
        // older sample of the same generation. Repair once without letting
        // late duplicates overwrite the confirmed observation.
        if (!record.settled || satisfied)
            && let Some(window) = state.windows.window_mut(wid)
        {
            window.info.frame = new_frame;
            window.presented_frame = new_frame;
        }
        // A presentation can pass through its destination before motion ends.
        // Its terminal readback, rather than a display-tick observation, retires it.
        if satisfied && (!record.presentation || requested) {
            transactions.store.settle(server, record.txid);
        } else if requested && let Some(txid) = last_seen {
            return FrameChangeDisposition::Reconcile(txid);
        }
        query_mouse_for_active_drag(drag, mouse_state);
        return FrameChangeDisposition::Handled;
    }
    if requested || old_frame.same_as(new_frame) {
        query_mouse_for_active_drag(drag, mouse_state);
        if let Some(window) = state.windows.window_mut(wid) {
            window.info.frame = new_frame;
            window.presented_frame = new_frame;
        }
        return FrameChangeDisposition::Handled;
    }
    FrameChangeDisposition::NeedsGeometryAnalysis
}

fn query_mouse_for_active_drag(drag: &DragManager, mouse_state: &mut Option<MouseState>) {
    if mouse_state.is_none() && drag.actor.is_active() {
        *mouse_state = crate::sys::event::get_mouse_state();
    }
}

pub fn handle_window_frame_changed(
    state: &mut crate::model::RiftState,
    layout: &mut crate::actor::reactor::managers::LayoutManager,
    drag: &mut DragManager,
    payload: WindowFrameChangedPayload,
) -> anyhow::Result<EventOutcome> {
    let WindowFrameChangedPayload {
        window: wid,
        new_frame,
        mouse_state,
        old_space,
        new_space,
        old_space_active,
        new_space_active,
        active_resize_space,
        pending_target_space,
        assigned_space,
        keep_assigned_for_scrolling,
        screens,
    } = payload;
    let mut outcome = EventOutcome::default();
    let Some(window) = state.windows.window(wid) else {
        return Ok(outcome);
    };
    let server_id = window.info.sys_id;
    let old_frame = window.info.frame;

    if old_frame.same_as(new_frame) {
        return Ok(outcome);
    }
    if let Some(window) = state.windows.window_mut(wid) {
        window.presented_frame = new_frame;
        window.info.frame = new_frame;
    }
    if !old_space_active && !new_space_active {
        return Ok(outcome);
    }
    // The external owner has geometry authority; keep only native facts current.
    if state.windows.is_claimed(wid) {
        if old_space != new_space
            && let Some(server) = server_id
        {
            match new_space {
                Some(space) => _ = state.windows.observe_native_space(server, space, true),
                None => state.windows.set_window_server_space(server, None),
            }
        }
        return Ok(EventOutcome::no_change());
    }
    // An adjusted acknowledgement of our own frame write is not a new native drag.
    if matches!(
        drag.actor.kind(),
        Some(crate::actor::drag::DragKind::ModifierMove)
    ) && drag.actor.source().is_some_and(|source| source.window == wid)
    {
        return Ok(EventOutcome::no_change());
    }
    outcome = EventOutcome::layout_changed(false);

    state.windows.clear_floating_display_geometry(wid);
    // External moves as well as resizes are authoritative for floating layouts.
    // Requested frame acknowledgements have already been filtered by the classifier.
    if let Some(space) = assigned_space.or(old_space)
        && Some(space) == new_space
        && let Some(workspace) =
            layout
                .layout_engine
                .workspaces()
                .workspace_for_window(&state.windows, space, wid)
        && layout.layout_engine.is_window_floating(wid)
    {
        layout.layout_engine.store_floating_position(workspace, wid, new_frame);
    }

    let interaction = drag.native_geometry_intent(wid, new_frame);
    let dragging = state.windows.is_admitted(wid)
        && mouse_state == Some(MouseState::Down)
        && interaction.is_some();
    if dragging {
        let old_frame = if drag.actor.source().is_none() {
            drag.native_down.map_or(old_frame, |(_, frame, _)| frame)
        } else {
            old_frame
        };
        let tiled = !layout.layout_engine.is_window_floating(wid);
        let native_resize = interaction == Some(crate::actor::drag::DragKind::NativeResize);
        if !drag.actor.update_native(wid, new_frame, new_space) {
            // The start of a user drag is intent, even when no workspace changes.
            layout.layout_engine.forget_native_layout_for_window(&state.windows, wid);
            let session_id = drag.actor.await_native(wid, native_resize);
            let scene = if tiled && !native_resize {
                new_space.map_or_else(crate::actor::drag::DragScene::default, |space| {
                    crate::actor::reactor::events::drag::build_drag_scene(state, layout, wid, space)
                })
            } else {
                Default::default()
            };
            let _ = drag.actor.resolve_start(
                session_id,
                Some(crate::actor::drag::DragSource {
                    window: wid,
                    origin_frame: old_frame,
                    last_frame: new_frame,
                    origin_space: old_space,
                    current_space: new_space,
                    tiled,
                }),
                scene,
            );
        }
        drag.sync_preview();
        if tiled {
            drag.externally_controlled_window = Some(wid);
        }
        if native_resize && active_resize_space.is_some() {
            outcome = outcome.with_layout_event(LayoutEvent::WindowResized {
                wid,
                old_frame,
                new_frame,
                screens: screens.into(),
            });
        }
    } else {
        if old_space != new_space {
            if pending_target_space.is_some()
                && assigned_space == pending_target_space
                && new_space != pending_target_space
            {
                return Ok(outcome);
            }
            if keep_assigned_for_scrolling {
                return Ok(outcome);
            }
            outcome = outcome.with_layout_event(LayoutEvent::WindowRemovedPreserveFloating(wid));
            if let Some(space) = new_space {
                if let Some(server) = server_id {
                    state.windows.observe_native_space(server, space, true);
                }
                if new_space_active
                    && state.windows.window(wid).is_some_and(WindowState::is_admitted)
                {
                    if let Some(workspace) =
                        layout.layout_engine.workspaces().active_workspace(space)
                    {
                        let _ = layout.layout_engine.workspaces_mut().assign_window_to_workspace(
                            &mut state.windows,
                            space,
                            wid,
                            workspace,
                        );
                    }
                    outcome = outcome.with_layout_event(LayoutEvent::WindowAdded(space, wid));
                }
            } else if let Some(server) = server_id {
                state.windows.set_window_server_space(server, None);
            }
        }
    }

    if handle_mouse_up_if_needed(drag, false, mouse_state) {
        outcome.dispatch_mouse_up = true;
    }
    Ok(outcome)
}

#[derive(Debug)]
pub struct WindowTitleChangedPayload {
    pub window: WindowId,
    pub title: String,
}

pub fn handle_window_title_changed(
    state: &mut crate::model::RiftState,
    payload: WindowTitleChangedPayload,
) -> anyhow::Result<crate::actor::reactor::events::EventOutcome> {
    let WindowTitleChangedPayload { window: wid, title: new_title } = payload;
    if let Some(window) = state.windows.window_mut(wid) {
        let previous_title = window.info.title.clone();
        if previous_title == new_title {
            return Ok(crate::actor::reactor::events::EventOutcome::no_change());
        }
        window.info.title = new_title.clone();
        let mut outcome = crate::actor::reactor::events::EventOutcome::no_change()
            .with_window_title_broadcast(wid, previous_title, new_title);
        outcome.reapply_app_rules.push(wid);
        return Ok(outcome);
    }
    Ok(crate::actor::reactor::events::EventOutcome::no_change())
}

#[derive(Debug, Clone, Copy)]
pub struct MouseMovedPayload {
    pub window: Option<WindowId>,
    pub should_sync: bool,
    pub is_main: bool,
    pub needs_layout_sync: bool,
    pub active_space: Option<SpaceId>,
}

pub fn handle_mouse_moved_over_window(
    apps: &crate::actor::reactor::managers::AppManager,
    payload: MouseMovedPayload,
    mouse_token: &tokio_util::sync::CancellationToken,
) -> anyhow::Result<crate::actor::reactor::events::EventOutcome> {
    let Some(window) = payload.window else {
        return Ok(crate::actor::reactor::events::EventOutcome::default());
    };
    if !payload.should_sync || (payload.is_main && !payload.needs_layout_sync) {
        return Ok(crate::actor::reactor::events::EventOutcome::default());
    }

    let mut outcome = crate::actor::reactor::events::EventOutcome::default();
    if !payload.is_main {
        let mut app_handles = crate::common::collections::HashMap::default();
        if let Some(app) = apps.apps.get(&window.pid) {
            app_handles.insert(window.pid, app.handle.clone());
        }
        outcome =
            outcome.with_raise_request(crate::actor::raise_manager::Event::MouseRaiseRequest(
                crate::actor::raise_manager::RaiseRequest {
                    raise_windows: vec![vec![window]],
                    focus_window: Some((window, None)),
                    app_handles,
                    focus_quiet: Quiet::Yes,
                },
                mouse_token.clone(),
            ));
    }
    if let Some(space) = payload.active_space {
        outcome = outcome.with_layout_event(LayoutEvent::WindowFocused(space, window));
    }
    Ok(outcome)
}
fn handle_mouse_up_if_needed(
    drag: &mut DragManager,
    mission_control_active: bool,
    mouse_state: Option<MouseState>,
) -> bool {
    if mission_control_active {
        drag.reset();
        return false;
    }

    if mouse_state == Some(MouseState::Up) && drag.actor.is_active() {
        return true;
    }
    false
}
