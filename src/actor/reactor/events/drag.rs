use tracing::{trace, warn};

use crate::actor::reactor::LayoutEvent;
use crate::actor::reactor::events::EventOutcome;
use crate::actor::reactor::managers::{DragManager, LayoutManager};
use crate::model::RiftState;
use crate::sys::screen::SpaceId;

pub fn build_drag_scene(
    state: &RiftState,
    layout: &LayoutManager,
    source: crate::actor::app::WindowId,
    space: SpaceId,
) -> crate::actor::drag::DragScene {
    crate::actor::drag::DragScene {
        action_override: layout.layout_engine.drop_action_override(space),
        source_neighbors: layout.layout_engine.source_move_neighbors(space, source),
        targets: layout
            .layout_engine
            .drop_scene_windows(space, source)
            .into_iter()
            .filter(|window| *window != source)
            .filter_map(|window| {
                Some(crate::actor::drag::DragSceneTarget {
                    window,
                    space,
                    frame: state.windows.window(window)?.frame_monotonic,
                })
            })
            .collect(),
    }
}

#[derive(Debug, Clone)]
pub struct MouseUpPayload {
    pub button: crate::actor::drag::MouseButton,
    pub final_space: Option<SpaceId>,
}

pub fn handle_cancel(drag: &mut DragManager) -> EventOutcome {
    let cancelled = drag.actor.cancel();
    drag.release_preview();
    drag.externally_controlled_window = None;
    let Some(cancelled) = cancelled else {
        return EventOutcome::no_change();
    };
    let source = cancelled.source;
    match (cancelled.kind, source.tiled) {
        (crate::actor::drag::DragKind::ModifierMove, true) => EventOutcome::layout_changed(false),
        (crate::actor::drag::DragKind::ModifierMove, false)
            if source.last_frame != source.origin_frame =>
        {
            EventOutcome::no_change().with_pre_layout_window_frame_write(
                source.window,
                source.origin_frame,
                true,
            )
        }
        _ => EventOutcome::no_change(),
    }
}

pub fn handle_mouse_up(
    state: &mut RiftState,
    layout: &mut LayoutManager,
    drag: &mut DragManager,
    payload: MouseUpPayload,
) -> anyhow::Result<EventOutcome> {
    let mut outcome = EventOutcome::layout_changed(false);
    let Some(commit) = drag.actor.finish(payload.button) else {
        return Ok(outcome);
    };
    drag.release_preview();
    let window = commit.source.window;
    drag.externally_controlled_window = None;
    let mut needs_layout = commit.source.tiled;

    if let Some(target) = commit.target
        && commit.source.current_space == Some(target.intent.space)
        && state.windows.contains_window(window)
        && state.windows.contains_window(target.intent.window)
    {
        trace!(source=?window, target=?target.intent.window, action=?target.intent.action, "performing window drop");
        if layout.layout_engine.apply_window_drop(crate::layout_engine::WindowDropRequest {
            source: window,
            target: target.intent.window,
            space: target.intent.space,
            action: target.intent.action,
        }) {
            needs_layout = true;
        }
    }

    if commit.source.origin_space != payload.final_space {
        if commit.source.origin_space.is_some() {
            outcome = outcome.with_layout_event(LayoutEvent::WindowRemoved(window));
        }
        if let Some(space) = payload.final_space
            && state.windows.is_admitted(window)
        {
            if let Some(server_id) =
                state.windows.window(window).and_then(|window| window.info.sys_id)
            {
                state.windows.set_window_server_space(server_id, Some(space));
                state.windows.mark_window_visible(server_id);
            }
            if let Some(workspace) = layout.layout_engine.active_workspace(space)
                && !layout.layout_engine.virtual_workspace_manager_mut().assign_window_to_workspace(
                    &mut state.windows,
                    space,
                    window,
                    workspace,
                )
            {
                warn!(?window, ?workspace, "failed to assign dragged window");
            }
            outcome = outcome.with_layout_event(LayoutEvent::WindowAdded(space, window));
        }
        needs_layout = true;
    }

    if let Some(space) = payload.final_space
        && layout.layout_engine.is_window_floating(window)
    {
        if commit.source.origin_space != payload.final_space {
            layout.layout_engine.remove_floating_position(window);
        }
        if let Some(workspace) = layout
            .layout_engine
            .virtual_workspace_manager()
            .workspace_for_window(&state.windows, space, window)
            .or_else(|| layout.layout_engine.active_workspace(space))
        {
            layout.layout_engine.store_floating_position(
                space,
                workspace,
                window,
                commit.source.last_frame,
            );
        }
    }

    Ok(outcome.with_arrange_passes(u8::from(needs_layout)))
}
