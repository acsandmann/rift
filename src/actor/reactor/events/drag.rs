use tracing::trace;

use crate::actor::reactor::{DragState, Reactor};
use crate::layout_engine::LayoutCommand;

pub struct DragEventHandler;

impl DragEventHandler {
    pub fn handle_mouse_up(reactor: &mut Reactor) {
        let mut need_layout_refresh = false;

        let pending_swap = if let DragState::PendingSwap { dragged, target } =
            std::mem::replace(&mut reactor.drag_state, DragState::Inactive)
        {
            Some((dragged, target))
        } else {
            None
        };

        if let Some((dragged_wid, target_wid)) = pending_swap {
            trace!(?dragged_wid, ?target_wid, "Performing deferred swap on MouseUp");

            reactor.skip_layout_for_window = Some(dragged_wid);

            if !reactor.windows.contains_key(&dragged_wid)
                || !reactor.windows.contains_key(&target_wid)
            {
                trace!(
                    ?dragged_wid,
                    ?target_wid,
                    "Skipping deferred swap; one of the windows no longer exists"
                );
            } else {
                let visible_spaces =
                    reactor.screens.iter().flat_map(|s| s.space).collect::<Vec<_>>();

                let swap_space = reactor
                    .windows
                    .get(&dragged_wid)
                    .and_then(|w| reactor.best_space_for_window(&w.frame_monotonic))
                    .or_else(|| {
                        reactor
                            .drag_manager
                            .origin_frame()
                            .and_then(|f| reactor.best_space_for_window(&f))
                    })
                    .or_else(|| reactor.screens.iter().find_map(|s| s.space));
                let response = reactor.layout_engine.handle_command(
                    swap_space,
                    &visible_spaces,
                    LayoutCommand::SwapWindows(dragged_wid, target_wid),
                );
                reactor.handle_layout_response(response);

                need_layout_refresh = true;
            }
        }

        let finalize_needs_layout = reactor.finalize_active_drag();

        reactor.drag_manager.reset();

        if finalize_needs_layout {
            need_layout_refresh = true;
        }

        if need_layout_refresh {
            let _ = reactor.update_layout(false, false);
        }

        reactor.skip_layout_for_window = None;
    }
}
