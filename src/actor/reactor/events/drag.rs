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
            && source.origin == crate::actor::drag::DragOrigin::Floating
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
    ) {
        let kind = self.drag_manager.actor.kind();
        let Some(commit) = self.drag_manager.actor.finish(button) else {
            return;
        };
        self.drag_manager.release_preview();
        if self.refreshes_blocked() {
            return;
        }
        let source = commit.source;
        let window = source.window;
        let Some(workspace) = self.state.windows.workspace_for_window(window) else {
            return;
        };
        let Some(owner) = self.assigned_space_for_window_id(window) else {
            return;
        };
        changes.arrange(Some(owner), Presentation::Animated);

        if kind == Some(crate::actor::drag::DragKind::NativeResize) {
            let engine = &mut self.layout_manager.layout_engine;
            if engine.is_window_floating(window) {
                engine.store_floating_position(workspace, window, source.last_frame);
            }
            return;
        }

        let start = self.best_space_for_frame(&source.origin_frame);
        let landing = self.best_space_for_frame(&source.last_frame);
        // The pointer's display is where the drop lands; the frame can still be mostly on the
        // display the drag left. Only a drag with no pointer display falls back to the frame.
        let crossing = match source.current_space {
            Some(space) => Some(space).filter(|space| *space != owner),
            None => landing.filter(|space| Some(*space) != start),
        }
        .filter(|space| self.is_space_active(*space));
        let crossing = crossing.or_else(|| {
            // The last AX frame can lag mouse-up. Use a complete membership sample
            // only when current WindowServer membership corroborates its destination.
            if kind != Some(crate::actor::drag::DragKind::NativeMove)
                || !self.space_state.membership_complete
            {
                return None;
            }
            let wsid = self.state.windows.record(window)?.window_server_id()?;
            let space = *self.space_state.active_window_spaces.get(&wsid)?;
            (Some(space) != start
                && self.is_space_active(space)
                && crate::sys::window_server::window_spaces(wsid).as_slice() == [space])
            .then_some(space)
        });
        let space = crossing.unwrap_or(owner);
        let destination = if space == owner {
            workspace
        } else if let Some(id) = self.ensure_window_move_destination(changes, space) {
            id
        } else {
            self.request_window_transfer(changes, window, owner, None);
            self.request_space_snapshot();
            return;
        };

        // A target on the drop's display: within the owner's layout it is applied here; a
        // window crossing displays carries it into the relocation below.
        let target = commit.target.filter(|target| target.intent.space == space);
        let relocation_target = if space == owner {
            if let Some(target) = target
                && self.state.windows.contains_window(target.intent.window)
            {
                trace!(source=?window, target=?target.intent.window, action=?target.intent.action, "performing window drop");
                if self.layout_manager.layout_engine.apply_window_drop(
                    crate::layout_engine::WindowDropRequest {
                        source: window,
                        target: target.intent.window,
                        space,
                        action: target.intent.action,
                    },
                ) {
                    changes.space(space).layout_changed = true;
                    changes.drop_haptic = self.drag_manager.haptics_enabled;
                }
            }
            None
        } else {
            target.map(|target| (target.intent.window, target.intent.action))
        };

        if crossing.is_some() || landing == Some(space) {
            let floating = self.layout_manager.layout_engine.is_window_floating(window);
            let frame = floating.then(|| {
                self.space_state
                    .screen_by_space(space)
                    .filter(|_| landing != Some(space))
                    .map_or(source.last_frame, |screen| {
                        crate::sys::geometry::frame_on_display(
                            source.last_frame,
                            None,
                            screen.frame,
                            Default::default(),
                        )
                    })
            });
            if self.layout_manager.layout_engine.relocate_window_with_drop(
                &mut self.state.windows,
                &crate::layout_engine::WorkspaceDropRequest {
                    window,
                    workspace: destination,
                    target: relocation_target,
                    frame,
                },
            ) && space != owner
            {
                changes.move_between(owner, space, Presentation::Animated);
            }
        }
        // A completed drag has placement intent, not native membership. Use the
        // transfer fence for late snapshots and any queued modifier-frame writes.
        let owner = self.assigned_space_for_window_id(window).unwrap_or(owner);
        if crossing.is_some() || landing.is_some_and(|space| space != owner) {
            if landing == Some(owner)
                && self.layout_manager.layout_engine.is_window_floating(window)
            {
                self.send_transfer_frame(window, owner, Some(source.last_frame), true, None);
            } else {
                self.request_window_transfer(changes, window, owner, None);
            }
            self.request_space_snapshot();
        }
    }
}
