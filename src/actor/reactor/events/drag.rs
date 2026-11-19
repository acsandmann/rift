use tracing::trace;

use super::super::{Changes, Presentation, Reactor};
use crate::actor::reactor::managers::LayoutManager;
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

impl Reactor {
    pub(in crate::actor::reactor) fn cancel_drag(&mut self, changes: &mut Changes) {
        let cancelled = self.drag_manager.actor.cancel();
        self.drag_manager.release_preview();
        let Some(cancelled) = cancelled else { return };
        let source = cancelled.source;
        if cancelled.kind == crate::actor::drag::DragKind::ModifierMove
            && !source.tiled
            && source.last_frame != source.origin_frame
        {
            self.write_window_frame(source.window, source.origin_frame, false, None);
        } else {
            changes.arrange(
                self.assigned_space_for_window_id(source.window).or(source.origin_space),
                Presentation::Animated,
            );
        }
    }

    pub(in crate::actor::reactor) fn finish_drag(
        &mut self,
        changes: &mut Changes,
        button: crate::actor::drag::MouseButton,
        final_space: Option<SpaceId>,
    ) {
        let Some(commit) = self.drag_manager.actor.finish(button) else {
            return;
        };
        let origin = self
            .assigned_space_for_window_id(commit.source.window)
            .or(commit.source.origin_space);
        for space in [origin, final_space].into_iter().flatten() {
            changes.arrange(Some(space), Presentation::Animated);
        }
        let drag = &mut self.drag_manager;
        let state = &mut self.state;
        let layout = &mut self.layout_manager;
        drag.release_preview();
        let window = commit.source.window;

        if let Some(target) = commit.target
            && final_space == Some(target.intent.space)
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
                changes.space(target.intent.space).layout_changed = true;
                changes.drop_haptic = drag.haptics_enabled;
            }
        }

        if let Some(space) = final_space {
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
            if moved {
                changes.membership_for(origin);
                changes.membership_for(Some(space));
            }
            if moved
                && let Some(server_id) =
                    state.windows.window(window).and_then(|win| win.info.sys_id)
            {
                state.windows.observe_native_space(server_id, space, true);
            }
        }
    }
}
