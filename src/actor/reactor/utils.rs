use objc2_app_kit::NSNormalWindowLevel;

use crate::actor::app::WindowId;
use crate::actor::reactor::managers::LayoutManager;
use crate::model::RiftState;
use crate::sys::screen::SpaceId;
use crate::sys::window_server::{window_is_sticky, window_level};

pub(crate) struct AdmissionTransition {
    pub(crate) was_admitted: bool,
    pub(crate) is_admitted: bool,
}

pub(crate) fn refresh_heuristic(
    state: &mut RiftState,
    wid: WindowId,
) -> Option<AdmissionTransition> {
    let window = state.windows.window(wid)?;
    let was_admitted = window.is_admitted();
    let server_id = window.info.sys_id;
    let manageable = !window.info.is_minimized
        && window.info.is_standard
        && window.info.is_root
        && server_id.is_none_or(|wsid| {
            !state.windows.get_window_server_info(wsid).is_some_and(|info| info.layer != 0)
                && !window_is_sticky(wsid)
                && window_level(wsid.0).is_none_or(|level| level == NSNormalWindowLevel)
        });
    let window = state.windows.window_mut(wid)?;
    window.is_manageable = manageable;
    Some(AdmissionTransition {
        was_admitted,
        is_admitted: window.is_admitted(),
    })
}

pub(crate) fn rejection_needs_removal(
    state: &RiftState,
    layout: &LayoutManager,
    wid: WindowId,
    space: SpaceId,
) -> bool {
    let engine = &layout.layout_engine;
    engine
        .virtual_workspace_manager()
        .workspace_for_window(&state.windows, space, wid)
        .is_some()
        || engine.is_window_floating(wid)
}

pub(crate) fn clear_rule_admission(state: &mut RiftState, wid: WindowId) {
    if let Some(window) = state.windows.window_mut(wid) {
        window.manage_override = None;
    }
}
