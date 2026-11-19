use tracing::trace;

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
    let Some(cancelled) = cancelled else {
        return EventOutcome::no_change();
    };
    let source = cancelled.source;
    match (cancelled.kind, source.tiled) {
        (crate::actor::drag::DragKind::ModifierMove, false)
            if source.last_frame != source.origin_frame =>
        {
            EventOutcome::no_change().with_pre_layout_window_frame_write(
                source.window,
                source.origin_frame,
                true,
            )
        }
        _ => EventOutcome::layout_changed(false),
    }
}

pub fn handle_mouse_up(
    state: &mut RiftState,
    layout: &mut LayoutManager,
    drag: &mut DragManager,
    payload: MouseUpPayload,
) -> anyhow::Result<EventOutcome> {
    let Some(commit) = drag.actor.finish(payload.button) else {
        return Ok(EventOutcome::no_change());
    };
    let mut outcome = EventOutcome::layout_changed(false);
    drag.release_preview();
    let window = commit.source.window;

    if let Some(target) = commit.target
        && payload.final_space == Some(target.intent.space)
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
            outcome.drop_haptic = drag.haptics_enabled;
        }
    }

    if let Some(space) = payload.final_space {
        let engine = &mut layout.layout_engine;
        let floating = engine.is_window_floating(window);
        let source = state.windows.workspace_for_window(window);
        let same_space = source
            .and_then(|workspace| engine.workspaces().workspace_space(workspace))
            == Some(space);
        let destination = if same_space {
            source.filter(|_| floating)
        } else {
            engine.workspaces().active_workspace(space)
        };
        let moved = if let Some(workspace) = destination {
            engine.relocate_window_with_drop(
                &mut state.windows,
                &crate::layout_engine::WorkspaceDropRequest {
                    window,
                    workspace,
                    target: None,
                    frame: floating.then_some(commit.source.last_frame),
                },
            )
        } else if !same_space && source.is_some() {
            engine.detach_window_from_workspace(&mut state.windows, window);
            true
        } else {
            false
        };
        if moved
            && let Some(server_id) = state.windows.window(window).and_then(|win| win.info.sys_id)
        {
            state.windows.observe_native_space(server_id, space, true);
        }
    }

    Ok(outcome)
}
