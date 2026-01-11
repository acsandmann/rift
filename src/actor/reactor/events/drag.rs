use tracing::trace;

use crate::actor::reactor::{DragState, Reactor};
use crate::common::collections::HashMap;
use crate::layout_engine::LayoutCommand;
use crate::sys::screen::{SpaceId, order_visible_spaces_by_position};

pub struct DragEventHandler;

impl DragEventHandler {
    pub fn handle_mouse_up(reactor: &mut Reactor) {
        let mut need_layout_refresh = false;

        let pending_swap = reactor.get_pending_drag_swap();

        if let Some((dragged_wid, target_wid)) = pending_swap {
            trace!(?dragged_wid, ?target_wid, "Performing deferred swap on MouseUp");

            reactor.drag_manager.skip_layout_for_window = Some(dragged_wid);

            if !reactor.window_manager.windows.contains_key(&dragged_wid)
                || !reactor.window_manager.windows.contains_key(&target_wid)
            {
                trace!(
                    ?dragged_wid,
                    ?target_wid,
                    "Skipping deferred swap; one of the windows no longer exists"
                );
            } else {
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
                let response = reactor.layout_manager.layout_engine.handle_command(
                    swap_space,
                    &visible_spaces,
                    &visible_space_centers,
                    LayoutCommand::SwapWindows(dragged_wid, target_wid),
                );
                reactor.handle_layout_response(response, None);

                need_layout_refresh = true;
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
            let _ = reactor.update_layout_or_warn(false, false);
            if skip_layout_occurred {
                let _ = reactor.update_layout_or_warn(false, false);
            }
        }

        reactor.drag_manager.skip_layout_for_window = None;
    }
}
