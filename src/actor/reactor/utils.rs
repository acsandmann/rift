use objc2_app_kit::NSNormalWindowLevel;

use crate::actor::app::WindowId;
use crate::actor::reactor::managers::LayoutManager;
use crate::model::RiftState;
use crate::sys::app::WindowInfo;
use crate::sys::screen::SpaceId;
use crate::sys::window_server::{WindowServerId, WindowServerInfo, window_is_sticky, window_level};

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
    let manageable = compute_window_info_manageability(&window.info, |wsid| {
        state.windows.get_window_server_info(wsid)
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

/// Computes whether a window is manageable based on its properties and window server information.
///
/// A window is manageable if:
/// - It is not minimized
/// - Its layer/level is normal (if info available), except for known IBKR Desktop AX dialogs
/// - It is not sticky
/// - It is a standard AX window and AX root, except for known IBKR Desktop AX dialogs
pub fn compute_window_info_manageability(
    info: &WindowInfo,
    window_server_info: impl FnMut(WindowServerId) -> Option<WindowServerInfo>,
) -> bool {
    let allow_nonstandard_window = is_ibkr_desktop_dialog(info);
    compute_window_manageability(
        info.sys_id,
        info.is_minimized,
        info.is_ax_window,
        info.is_standard,
        info.is_root,
        allow_nonstandard_window,
        window_server_info,
    )
}

fn compute_window_manageability(
    window_server_id: Option<WindowServerId>,
    is_minimized: bool,
    is_ax_window: bool,
    is_ax_standard: bool,
    is_ax_root: bool,
    allow_nonstandard_window: bool,
    mut window_server_info: impl FnMut(WindowServerId) -> Option<WindowServerInfo>,
) -> bool {
    if is_minimized {
        return false;
    }

    if let Some(wsid) = window_server_id {
        if let Some(info) = window_server_info(wsid) {
            if info.layer != 0 && !allow_nonstandard_window {
                return false;
            }
        }
        if window_is_sticky(wsid) {
            return false;
        }

        if let Some(level) = window_level(wsid.0) {
            if level != NSNormalWindowLevel && !allow_nonstandard_window {
                return false;
            }
        }
    }
    is_ax_window && is_ax_root && (is_ax_standard || allow_nonstandard_window)
}

fn is_ibkr_desktop_dialog(info: &WindowInfo) -> bool {
    if info.ax_role.as_deref() != Some("AXWindow") || info.ax_subrole.as_deref() != Some("AXDialog")
    {
        return false;
    }

    let path_matches = info.path.as_ref().and_then(|path| path.to_str()).is_some_and(|path| {
        let path = path.to_ascii_lowercase();
        path.contains("ibkr%20desktop") || path.contains("ibkr desktop")
    });

    let bundle_matches = info.bundle_id.as_deref().is_some_and(|bundle_id| {
        bundle_id.starts_with("com.install4j.") || bundle_id == "com.azul.zulu.java"
    });

    let max_size = info.max_size.unwrap_or(info.frame.size);
    let min_size = info.min_size.unwrap_or(info.frame.size);
    let looks_like_primary_window = max_size.width >= 1200.0
        || max_size.height >= 900.0
        || min_size.width >= 1000.0
        || info.frame.size.width >= 1200.0
        || info.frame.size.height >= 900.0;

    path_matches && bundle_matches && looks_like_primary_window
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use objc2_core_foundation::{CGPoint, CGRect, CGSize};

    use super::compute_window_info_manageability;
    use crate::sys::app::WindowInfo;

    fn ax_window(subrole: &str, is_standard: bool) -> WindowInfo {
        WindowInfo {
            is_standard,
            is_root: true,
            is_minimized: false,
            is_resizable: true,
            is_ax_window: true,
            title: "Window".into(),
            frame: CGRect::new(CGPoint::ZERO, CGSize::new(800.0, 600.0)),
            min_size: None,
            max_size: None,
            sys_id: None,
            bundle_id: None,
            path: None,
            ax_role: Some("AXWindow".into()),
            ax_subrole: Some(subrole.into()),
        }
    }

    #[test]
    fn standard_ax_window_without_window_server_metadata_is_manageable() {
        let info = ax_window("AXStandardWindow", true);

        assert!(compute_window_info_manageability(&info, |_| None));
    }

    #[test]
    fn nonstandard_qt_tooltip_without_window_server_metadata_is_not_manageable() {
        let mut info = ax_window("AXUnknown", false);
        info.is_resizable = false;
        info.frame.size = CGSize::new(240.0, 32.0);
        info.bundle_id = Some("org.qgis.qgis3".into());

        assert!(!compute_window_info_manageability(&info, |_| None));
    }

    #[test]
    fn known_ibkr_dialog_remains_manageable() {
        let mut info = ax_window("AXDialog", false);
        info.frame.size = CGSize::new(1400.0, 900.0);
        info.bundle_id = Some("com.install4j.runtime.123".into());
        info.path = Some(PathBuf::from("file:///Applications/IBKR%20Desktop.app"));

        assert!(compute_window_info_manageability(&info, |_| None));
    }
}
