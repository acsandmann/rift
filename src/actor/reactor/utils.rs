use objc2_app_kit::NSNormalWindowLevel;
use objc2_core_foundation::CGSize;

use crate::sys::window_server::{WindowServerId, WindowServerInfo, window_is_sticky, window_level};

/// Small WindowServer surfaces used by capture/streaming helpers can expose themselves as
/// otherwise-normal AX windows. They are not useful tiling targets and can disappear without the
/// same lifecycle guarantees as application windows.
const MIN_MANAGEABLE_WINDOW_DIMENSION: f64 = 16.0;

fn is_tiny_window_server_frame(size: CGSize) -> bool {
    let width = size.width.abs();
    let height = size.height.abs();

    if !width.is_finite() || !height.is_finite() {
        return true;
    }

    // A zero-sized frame can be a transient creation snapshot. Keep it eligible so a later
    // discovery pass can reconcile the real frame instead of permanently losing the window.
    width > 0.0
        && height > 0.0
        && (width < MIN_MANAGEABLE_WINDOW_DIMENSION || height < MIN_MANAGEABLE_WINDOW_DIMENSION)
}

/// Computes whether a window is manageable based on its properties and window server information.
///
/// A window is manageable if:
/// - It is not minimized
/// - Its layer is 0 (if info available)
/// - Its nonzero WindowServer dimensions are large enough to represent a real app window
/// - It is not sticky
/// - Its level is normal (if available)
/// - It is AX standard and AX root
pub fn compute_window_manageability(
    window_server_id: Option<WindowServerId>,
    is_minimized: bool,
    is_ax_standard: bool,
    is_ax_root: bool,
    mut window_server_info: impl FnMut(WindowServerId) -> Option<WindowServerInfo>,
) -> bool {
    if is_minimized {
        return false;
    }

    if let Some(wsid) = window_server_id {
        if let Some(info) = window_server_info(wsid) {
            if info.layer != 0 || is_tiny_window_server_frame(info.frame.size) {
                return false;
            }
        }
        if window_is_sticky(wsid) {
            return false;
        }

        if let Some(level) = window_level(wsid.0) {
            if level != NSNormalWindowLevel {
                return false;
            }
        }
    }
    is_ax_standard && is_ax_root
}

#[cfg(test)]
mod tests {
    use objc2_core_foundation::CGSize;

    use super::is_tiny_window_server_frame;

    #[test]
    fn rejects_tiny_capture_helper_frames() {
        assert!(is_tiny_window_server_frame(CGSize::new(40.0, 11.0)));
        assert!(is_tiny_window_server_frame(CGSize::new(11.0, 40.0)));
    }

    #[test]
    fn accepts_ordinary_and_transitional_frames() {
        assert!(!is_tiny_window_server_frame(CGSize::new(800.0, 600.0)));
        assert!(!is_tiny_window_server_frame(CGSize::ZERO));
    }

    #[test]
    fn rejects_invalid_frame_dimensions() {
        assert!(is_tiny_window_server_frame(CGSize::new(f64::NAN, 100.0)));
        assert!(is_tiny_window_server_frame(CGSize::new(100.0, f64::INFINITY)));
    }
}
