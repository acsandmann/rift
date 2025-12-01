use tracing::{trace, warn};

use crate::actor::reactor::{DragState, Reactor};
use crate::common::collections::HashMap;
use crate::common::config::DragOverlapAction;
use crate::layout_engine::{Direction, LayoutCommand};
use crate::sys::screen::{SpaceId, order_visible_spaces_by_position};

pub struct DragEventHandler;

impl DragEventHandler {
    pub fn handle_mouse_up(reactor: &mut Reactor) {
        let mut need_layout_refresh = false;

        let pending_action = reactor.get_pending_drag_action();

        if let Some((dragged_wid, target_wid, action)) = pending_action {
            trace!(
                ?dragged_wid,
                ?target_wid,
                ?action,
                "Performing deferred drag action on MouseUp"
            );

            reactor.drag_manager.skip_layout_for_window = Some(dragged_wid);

            if !reactor.window_manager.windows.contains_key(&dragged_wid)
                || !reactor.window_manager.windows.contains_key(&target_wid)
            {
                trace!(
                    ?dragged_wid,
                    ?target_wid,
                    "Skipping deferred action; one of the windows no longer exists"
                );
            } else {
                let dragged_frame =
                    reactor.window_manager.windows.get(&dragged_wid).map(|w| w.frame_monotonic);
                let target_frame =
                    reactor.window_manager.windows.get(&target_wid).map(|w| w.frame_monotonic);

                let visible_spaces_input: Vec<(SpaceId, _)> = reactor
                    .space_manager
                    .screens
                    .iter()
                    .filter_map(|screen| {
                        let space = reactor.space_manager.space_for_screen(screen)?;
                        let center = screen.frame.mid();
                        Some((space, center))
                    })
                    .collect();

                let mut visible_space_centers = HashMap::default();
                for (space, center) in &visible_spaces_input {
                    visible_space_centers.insert(*space, *center);
                }

                let visible_spaces =
                    order_visible_spaces_by_position(visible_spaces_input.iter().cloned());

                let swap_space = reactor
                    .window_manager
                    .windows
                    .get(&dragged_wid)
                    .and_then(|w| {
                        reactor.best_space_for_window(&w.frame_monotonic, w.window_server_id)
                    })
                    .or_else(|| {
                        reactor
                            .drag_manager
                            .drag_swap_manager
                            .origin_frame()
                            .and_then(|f| reactor.best_space_for_frame(&f))
                    })
                    .or_else(|| reactor.space_manager.screens.iter().find_map(|s| s.space));
                match action {
                    DragOverlapAction::Swap => {
                        let response = reactor.layout_manager.layout_engine.handle_command(
                            swap_space,
                            &visible_spaces,
                            &visible_space_centers,
                            LayoutCommand::SwapWindows(dragged_wid, target_wid),
                        );
                        reactor.handle_layout_response(response, None);
                        need_layout_refresh = true;
                    }
                    DragOverlapAction::Stack => {
                        let response = reactor.layout_manager.layout_engine.handle_command(
                            swap_space,
                            &visible_spaces,
                            &visible_space_centers,
                            LayoutCommand::StackWindows(dragged_wid, target_wid),
                        );
                        reactor.handle_layout_response(response, None);
                        need_layout_refresh = true;
                    }
                    DragOverlapAction::Move => {
                        if let (Some(space), Some(df), Some(tf)) =
                            (swap_space, dragged_frame, target_frame)
                        {
                            let delta_x = tf.mid().x - df.mid().x;
                            let delta_y = tf.mid().y - df.mid().y;
                            let direction = if delta_x.abs() >= delta_y.abs() {
                                if delta_x >= 0.0 {
                                    Direction::Right
                                } else {
                                    Direction::Left
                                }
                            } else if delta_y >= 0.0 {
                                Direction::Down
                            } else {
                                Direction::Up
                            };

                            if reactor
                                .layout_manager
                                .layout_engine
                                .select_window_in_space(space, dragged_wid)
                            {
                                let response = reactor.layout_manager.layout_engine.handle_command(
                                    Some(space),
                                    &visible_spaces,
                                    &visible_space_centers,
                                    LayoutCommand::MoveNode(direction),
                                );
                                reactor.handle_layout_response(response, None);
                                need_layout_refresh = true;
                            } else {
                                trace!(
                                    ?dragged_wid,
                                    "Skipping move action; could not select dragged window"
                                );
                            }
                        } else {
                            trace!(
                                ?dragged_wid,
                                ?target_wid,
                                "Skipping move action; unable to determine space or frames"
                            );
                        }
                    }
                }
            }
        }

        let finalize_needs_layout = reactor.finalize_active_drag();

        reactor.drag_manager.reset();
        reactor.drag_manager.drag_state = DragState::Inactive;

        if finalize_needs_layout
            || reactor.is_in_drag()
            || reactor.drag_manager.skip_layout_for_window.is_some()
        {
            need_layout_refresh = true;
        }

        if need_layout_refresh {
            let skip_layout_occurred = reactor.drag_manager.skip_layout_for_window.is_some();
            let _ = reactor.update_layout(false, false).unwrap_or_else(|e| {
                warn!("Layout update failed: {}", e);
                false
            });
            if skip_layout_occurred {
                let _ = reactor.update_layout(false, false).unwrap_or_else(|e| {
                    warn!("Layout update failed: {}", e);
                    false
                });
            }
        }

        reactor.drag_manager.skip_layout_for_window = None;
    }
}
