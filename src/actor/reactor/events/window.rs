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
use crate::sys::geometry::SameAs;
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

pub fn frame_change_needs_layout(
    state: &mut crate::model::RiftState,
    transactions: &TransactionManager,
    wid: WindowId,
    new_frame: CGRect,
    last_seen: Option<TransactionId>,
    requested: bool,
    mouse_state: &mut Option<MouseState>,
) -> bool {
    let Some(window) = state.windows.window_mut(wid) else {
        return false;
    };
    let server = window.info.sys_id;
    let record = server.and_then(|server| transactions.store.get(&server));

    if requested {
        if let (Some(seen), Some(record)) = (last_seen, record)
            && seen != record.txid
        {
            return false;
        }
        if let Some(server) = server {
            if let Some(seen) = last_seen {
                transactions.store.acknowledge(server, seen, new_frame);
            } else {
                transactions.clear_target_for_window(server);
            }
        }
        window.frame_monotonic = new_frame;
        return false;
    }

    if window.frame_monotonic.same_as(new_frame) {
        return false;
    }
    if mouse_state.is_none() {
        *mouse_state = crate::sys::event::get_mouse_state();
    }
    if let Some(record) = record
        && let Some(target) = record.target()
    {
        if new_frame.same_as(target) {
            window.frame_monotonic = new_frame;
            return false;
        }
        if *mouse_state != Some(MouseState::Down) && last_seen.is_some() {
            // Native notifications can describe an intermediate AX frame. Only
            // the app's readback completes a write.
            if last_seen == Some(record.txid) {
                window.frame_monotonic = new_frame;
            }
            return false;
        }
    }
    if let Some(server) = server {
        transactions.clear_target_for_window(server);
    }
    true
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
    let old_frame = window.frame_monotonic;

    if !old_space_active && !new_space_active {
        return Ok(outcome);
    }
    if old_frame.same_as(new_frame) {
        return Ok(outcome);
    }
    if let Some(window) = state.windows.window_mut(wid) {
        window.frame_monotonic = new_frame;
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

    if drag.actor.update_native(wid, new_frame, new_space) {
        drag.sync_preview();
        if !old_frame.size.same_as(new_frame.size) && active_resize_space.is_some() {
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
