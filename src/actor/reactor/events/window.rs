use objc2_core_foundation::CGRect;
use tracing::{debug, trace};

use crate::actor::app::{Request, WindowId};
use crate::actor::reactor::events::EventOutcome;
use crate::actor::reactor::managers::DragManager;
use crate::actor::reactor::transaction_manager::TransactionManager;
use crate::actor::reactor::{DragState, Quiet, Requested, TransactionId, WindowState, utils};
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
    transactions: &TransactionManager,
    payload: WindowCreatedPayload,
) -> anyhow::Result<EventOutcome> {
    let WindowCreatedPayload {
        window_id: wid,
        window,
        window_server_info: ws_info,
    } = payload;
    if let Some(wsid) = window.sys_id {
        state.windows.track_window_server_id(wsid, wid);
        state.windows.clear_window_server_observed(wsid);
    }
    if let Some(info) = ws_info {
        state.windows.clear_window_server_observed(info.id);
        state.windows.track_window_server_info(info);
    }

    let mut window_state: WindowState = window.into();
    let is_manageable = utils::compute_window_manageability(
        window_state.info.sys_id,
        window_state.info.is_minimized,
        window_state.info.is_standard,
        window_state.info.is_root,
        |wsid| state.windows.get_window_server_info(wsid),
    );
    window_state.is_manageable = is_manageable;
    if let Some(wsid) = window_state.info.sys_id {
        transactions.store_txid(
            wsid,
            transactions.get_last_sent_txid(wsid),
            window_state.frame_monotonic,
        );
    }

    state.windows.insert_window(wid, window_state);

    let outcome = EventOutcome::finalized_event(None, false, false, true);
    Ok(if is_manageable {
        outcome.with_created_window_finalization(wid)
    } else {
        outcome
    })
}

#[derive(Debug, Clone, Copy)]
pub struct WindowDestroyedPayload {
    pub window: WindowId,
}

#[derive(Debug, Clone, Copy)]
pub struct WindowInvalidatedPayload {
    pub window: WindowId,
}

pub fn handle_window_invalidated(
    state: &mut crate::model::RiftState,
    transactions: &TransactionManager,
    drag: &mut DragManager,
    payload: WindowInvalidatedPayload,
) -> anyhow::Result<EventOutcome> {
    let wid = payload.window;
    let window_server_id = state.windows.record(wid).and_then(|record| record.window_server_id());
    let Some(window_server_id) = window_server_id else {
        return handle_window_destroyed(state, transactions, drag, WindowDestroyedPayload {
            window: wid,
        });
    };

    // AX identity and native-window identity have different lifetimes. Detach the dead AX
    // state so it cannot remain in layouts or queries as a ghost, but retain its assignment
    // off-index until the same WindowServer identity is rediscovered. A later AX loss with no
    // native peer, or an authoritative WindowServer disappearance, takes the permanent-removal
    // path below.
    transactions.remove_for_window(window_server_id);
    state.windows.detach_window_state(wid);
    debug!(
        ?wid,
        ?window_server_id,
        "Detached invalid AX identity for later reacquisition"
    );

    if let DragState::PendingSwap { session, target } = &drag.drag_state
        && (session.window == wid || *target == wid)
    {
        drag.drag_state = DragState::Inactive;
    }
    if drag.dragged() == Some(wid) || drag.last_target() == Some(wid) {
        drag.reset();
        drag.drag_state = DragState::Inactive;
    }
    if drag.skip_layout_for_window == Some(wid) {
        drag.skip_layout_for_window = None;
    }

    Ok(EventOutcome::finalized_event(None, false, true, true)
        .with_layout_event(LayoutEvent::WindowRemovedPreserveFloating(wid))
        .with_app_request(wid.pid, Request::GetVisibleWindows))
}

pub fn handle_window_destroyed(
    state: &mut crate::model::RiftState,
    transactions: &TransactionManager,
    drag: &mut DragManager,
    payload: WindowDestroyedPayload,
) -> anyhow::Result<EventOutcome> {
    let wid = payload.window;
    let window_server_id = match state.windows.record(wid) {
        Some(record) => record.window_server_id(),
        None => return Ok(EventOutcome::finalized_event(None, false, false, false)),
    };

    if let Some(ws_id) = window_server_id {
        transactions.remove_for_window(ws_id);
        state.windows.remove_window_server_state(ws_id);
    } else {
        debug!(?wid, "Received WindowDestroyed for unknown window - ignoring");
    }
    state.windows.remove_window(wid);

    if let DragState::PendingSwap { session, target } = &drag.drag_state {
        if session.window == wid || *target == wid {
            trace!(
                ?wid,
                "Clearing pending drag swap because a participant window was destroyed"
            );
            drag.drag_state = DragState::Inactive;
        }
    }

    let dragged_window = drag.dragged();
    let last_target = drag.last_target();
    if dragged_window == Some(wid) || last_target == Some(wid) {
        drag.reset();
        if dragged_window == Some(wid) {
            drag.drag_state = DragState::Inactive;
        }
    }

    if drag.skip_layout_for_window == Some(wid) {
        drag.skip_layout_for_window = None;
    }
    Ok(EventOutcome::finalized_event(None, false, true, false)
        .with_layout_event(LayoutEvent::WindowRemoved(wid)))
}

pub fn handle_window_minimized(
    state: &mut crate::model::RiftState,
    wid: WindowId,
) -> anyhow::Result<crate::actor::reactor::events::EventOutcome> {
    let server_id = if let Some(window) = state.windows.window_mut(wid) {
        if window.info.is_minimized {
            return Ok(crate::actor::reactor::events::EventOutcome::finalized_event(
                None, false, false, false,
            ));
        }
        window.info.is_minimized = true;
        window.is_manageable = false;
        window.info.sys_id
    } else {
        debug!(?wid, "Received WindowMinimized for unknown window - ignoring");
        return Ok(crate::actor::reactor::events::EventOutcome::finalized_event(
            None, false, false, false,
        ));
    };
    if let Some(ws_id) = server_id {
        state.windows.mark_window_hidden(ws_id);
    }
    state.windows.set_visibility(wid, WindowVisibility::Minimized);
    Ok(
        crate::actor::reactor::events::EventOutcome::finalized_event(None, false, false, false)
            .with_layout_event(LayoutEvent::WindowRemoved(wid)),
    )
}

#[derive(Debug, Clone, Copy)]
pub struct WindowDeminiaturizedPayload {
    pub window: WindowId,
    pub active_space: Option<SpaceId>,
}

pub fn handle_window_deminiaturized(
    state: &mut crate::model::RiftState,
    payload: WindowDeminiaturizedPayload,
) -> anyhow::Result<crate::actor::reactor::events::EventOutcome> {
    let WindowDeminiaturizedPayload { window: wid, active_space } = payload;
    let (server_id, is_ax_standard, is_ax_root) = match state.windows.window_mut(wid) {
        Some(window) => {
            if !window.info.is_minimized {
                return Ok(crate::actor::reactor::events::EventOutcome::finalized_event(
                    None, false, false, false,
                ));
            }
            window.info.is_minimized = false;
            (window.info.sys_id, window.info.is_standard, window.info.is_root)
        }
        None => {
            debug!(
                ?wid,
                "Received WindowDeminiaturized for unknown window - ignoring"
            );
            return Ok(crate::actor::reactor::events::EventOutcome::finalized_event(
                None, false, false, false,
            ));
        }
    };
    let is_manageable =
        utils::compute_window_manageability(server_id, false, is_ax_standard, is_ax_root, |wsid| {
            state.windows.get_window_server_info(wsid)
        });
    if let Some(window) = state.windows.window_mut(wid) {
        window.is_manageable = is_manageable;
    }
    state.windows.set_visibility(wid, WindowVisibility::Visible);

    let mut outcome =
        crate::actor::reactor::events::EventOutcome::finalized_event(None, false, false, false);
    if is_manageable && let Some(space) = active_space {
        outcome = outcome.with_layout_event(LayoutEvent::WindowAdded(space, wid));
    }
    Ok(outcome)
}

#[derive(Debug)]
pub struct WindowFrameChangedPayload {
    pub window: WindowId,
    pub new_frame: CGRect,
    pub last_seen: Option<TransactionId>,
    pub requested: Requested,
    pub mouse_state: Option<MouseState>,
    pub mission_control_active: bool,
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

pub fn handle_window_frame_changed(
    state: &mut crate::model::RiftState,
    layout: &mut crate::actor::reactor::managers::LayoutManager,
    transactions: &TransactionManager,
    drag: &mut DragManager,
    payload: WindowFrameChangedPayload,
) -> anyhow::Result<EventOutcome> {
    let WindowFrameChangedPayload {
        window: wid,
        new_frame,
        last_seen,
        requested,
        mouse_state,
        mission_control_active,
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
    let mut outcome = EventOutcome::finalized_event(None, false, false, false);
    let Some(window) = state.windows.window(wid) else {
        return Ok(outcome);
    };
    let server_id = window.info.sys_id;
    let old_frame = window.frame_monotonic;

    if mission_control_active {
        drag.reset();
        drag.drag_state = DragState::Inactive;
        drag.skip_layout_for_window = None;
        return Ok(outcome);
    }

    let pending_target = server_id
        .and_then(|server| transactions.get_target_frame(server).map(|frame| (server, frame)));
    let last_sent = server_id
        .map(|server| transactions.get_last_sent_txid(server))
        .unwrap_or_default();
    let mut has_pending = pending_target.is_some();
    let mut triggered_by_rift = has_pending && last_seen.is_some_and(|seen| seen == last_sent);
    if mouse_state == Some(MouseState::Down) && triggered_by_rift {
        if let Some((server, _)) = pending_target {
            transactions.clear_target_for_window(server);
        }
        has_pending = false;
        triggered_by_rift = false;
    }
    if has_pending && last_seen.is_some_and(|seen| seen != last_sent) {
        return Ok(outcome);
    }
    if triggered_by_rift {
        if let Some((server, target)) = pending_target {
            if new_frame.same_as(target) {
                transactions.clear_target_for_window(server);
                if let Some(window) = state.windows.window_mut(wid) {
                    window.frame_monotonic = new_frame;
                }
            }
        }
        return Ok(outcome);
    }
    if requested.0 {
        if let Some(window) = state.windows.window_mut(wid) {
            window.frame_monotonic = new_frame;
        }
        if let Some(server) = server_id {
            transactions.clear_target_for_window(server);
        }
        return Ok(outcome);
    }
    if !old_space_active && !new_space_active {
        return Ok(outcome);
    }
    if old_frame.same_as(new_frame) {
        return Ok(outcome);
    }
    if let Some(window) = state.windows.window_mut(wid) {
        window.frame_monotonic = new_frame;
    }

    let dragging = mouse_state == Some(MouseState::Down)
        || matches!(
            drag.drag_state,
            DragState::Active { .. } | DragState::PendingSwap { .. }
        );
    if dragging {
        let needs_session = !matches!(
            &drag.drag_state,
            DragState::Active { session } | DragState::PendingSwap { session, .. }
                if session.window == wid
        );
        if needs_session {
            drag.drag_state = DragState::Active {
                session: crate::actor::reactor::DragSession {
                    window: wid,
                    last_frame: old_frame,
                    origin_space: old_space,
                    settled_space: old_space,
                    layout_dirty: false,
                },
            };
        }
        if let DragState::Active { session } = &mut drag.drag_state {
            session.last_frame = new_frame;
            session.layout_dirty = true;
            if session.settled_space != new_space {
                session.settled_space = new_space;
            }
        }
        drag.skip_layout_for_window = Some(wid);
        if !old_frame.size.same_as(new_frame.size) {
            if active_resize_space.is_some() {
                outcome = outcome.with_layout_event(LayoutEvent::WindowResized {
                    wid,
                    old_frame,
                    new_frame,
                    screens,
                });
            }
        } else {
            outcome = outcome.with_drag_swap_evaluation(wid, new_frame);
        }
    } else {
        drag.skip_layout_for_window = Some(wid);
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
                    state.windows.set_window_server_space(server, Some(space));
                    state.windows.mark_window_visible(server);
                }
                if new_space_active {
                    if let Some(workspace) = layout.layout_engine.active_workspace(space) {
                        let _ = layout
                            .layout_engine
                            .virtual_workspace_manager_mut()
                            .assign_window_to_workspace(&mut state.windows, space, wid, workspace);
                    }
                    outcome = outcome.with_layout_event(LayoutEvent::WindowAdded(space, wid));
                }
            } else if let Some(server) = server_id {
                state.windows.set_window_server_space(server, None);
            }
            outcome = outcome.with_arrange_passes(2);
        } else if !old_frame.size.same_as(new_frame.size) && old_space_active {
            // Apps implementing non-native fullscreen (Firefox with
            // full-screen-api.macos-native-full-screen=false, etc.) resize
            // their own window to cover the screen with no mouse involved.
            // Folding that transient frame into the layout rewrites the
            // split ratios and wrecks the workspace arrangement. Like
            // AeroSpace, never incorporate a programmatic full-screen-sized
            // resize: leave the tree untouched while it covers the screen,
            // and re-assert the stored layout once it shrinks back.
            let covers = |frame: &objc2_core_foundation::CGRect| {
                screens.iter().any(|(_, screen_frame, _)| {
                    frame.origin.x <= screen_frame.origin.x + 1.0
                        && frame.origin.y <= screen_frame.origin.y + 1.0
                        && frame.size.width >= screen_frame.size.width - 2.0
                        && frame.size.height >= screen_frame.size.height - 2.0
                })
            };
            if covers(&new_frame) {
                // Entering self-fullscreen: keep layout state pristine.
            } else if covers(&old_frame) {
                // Leaving self-fullscreen: snap the window back into its
                // slot instead of deriving new ratios from the restored frame.
                outcome = outcome.with_arrange_passes(1);
            } else {
                outcome.arrange.is_resize = true;
                outcome = outcome.with_layout_event(LayoutEvent::WindowResized {
                    wid,
                    old_frame,
                    new_frame,
                    screens,
                });
            }
        }
    }

    if handle_mouse_up_if_needed(drag, false, mouse_state) {
        outcome = outcome.with_mouse_up_dispatch();
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
            return Ok(crate::actor::reactor::events::EventOutcome::finalized_event(
                None, false, false, false,
            ));
        }
        window.info.title = new_title.clone();
        return Ok(crate::actor::reactor::events::EventOutcome::finalized_event(
            None, false, false, false,
        )
        .with_app_rule_reapply(wid)
        .with_window_title_broadcast(wid, previous_title, new_title));
    }
    Ok(crate::actor::reactor::events::EventOutcome::finalized_event(
        None, false, false, false,
    ))
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
        outcome = outcome.with_raise_request(crate::actor::raise_manager::Event::RaiseRequest(
            crate::actor::raise_manager::RaiseRequest {
                raise_windows: vec![vec![window]],
                focus_window: Some((window, None)),
                app_handles,
                focus_quiet: Quiet::No,
            },
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
        drag.drag_state = DragState::Inactive;
        drag.skip_layout_for_window = None;
        return false;
    }

    if mouse_state == Some(MouseState::Up)
        && (matches!(
            drag.drag_state,
            DragState::Active { .. } | DragState::PendingSwap { .. }
        ) || drag.skip_layout_for_window.is_some())
    {
        return true;
    }
    false
}
