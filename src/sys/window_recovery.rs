//! Recover parked windows after the daemon exits, including crashes and SIGKILL.
use std::io::{self, Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};

use objc2::MainThreadMarker;
use objc2_core_foundation::{CGPoint, CGRect, CGSize};

use super::app::running_apps;
use super::axuielement::{AX_WINDOW_ROLE, AXUIElement};
use super::screen::ScreenCache;
use crate::model::hidden_window_placement::HiddenWindowPlacement;

/// Install recovery before any actor can park a window. A separate process is
/// necessary: neither destructors nor signal handlers run after SIGKILL.
pub fn start() -> io::Result<Child> {
    let mut child = Command::new(std::env::current_exe()?)
        .arg("--recover-windows")
        .arg(std::process::id().to_string())
        // Terminal signals must reach the daemon without killing its recovery child.
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()?;
    let mut ready = [0];
    child.stdout.take().unwrap().read_exact(&mut ready)?;
    Ok(child)
}

pub fn run(pid: i32) -> io::Result<()> {
    let (exited, exit) = std::sync::mpsc::channel();
    super::dispatch::on_proc_exit(pid, move || {
        let _ = exited.send(());
    });
    // A newline flushes stdout. Keep Rust's default ignored SIGPIPE in this
    // child: a daemon killed during setup can close the pipe before this write.
    let _ = io::stdout().write_all(b"\n");
    // GCD delivers the process-exit event on its own queue. No run loop,
    // periodic wakeups, or window inventories are needed while Rift is alive.
    // The parent may have exited before the dispatch source was registered.
    if std::os::unix::process::parent_id() == pid as u32 {
        exit.recv().map_err(io::Error::other)?;
    }
    objc2::rc::autoreleasepool(|_| recover(MainThreadMarker::new().unwrap()));
    Ok(())
}

fn recover(mtm: MainThreadMarker) {
    let Some((screens, _)) = ScreenCache::new(mtm).refresh() else {
        return;
    };
    let screens: Vec<_> = screens.iter().map(|screen| screen.frame).collect();
    // Bound the cost of an unresponsive application without changing its timeout
    // in the daemon or any other process.
    unsafe { AXUIElement::system_wide().set_messaging_timeout(1.0) };
    let mut index = 0;
    for (pid, _) in running_apps(None) {
        let Ok(windows) = AXUIElement::application(pid).windows() else {
            continue;
        };
        for window in windows {
            let Ok(attributes) = window.window_attributes() else {
                continue;
            };
            if attributes.minimized || attributes.role != AX_WINDOW_ROLE {
                continue;
            }
            let Some(frame) = recovery_frame(attributes.frame, &screens, index) else {
                continue;
            };
            if window.fullscreen().unwrap_or(false) {
                continue;
            }
            if frame.size != attributes.frame.size {
                let _ = window.set_size(frame.size);
            }
            if window.set_position(frame.origin).is_ok() {
                index += 1;
            }
        }
    }
}

fn recovery_frame(window: CGRect, screens: &[CGRect], index: usize) -> Option<CGRect> {
    // A window hidden relative to one display may be visible on another.
    if screens
        .iter()
        .any(|screen| !HiddenWindowPlacement::is_hidden(*screen, window, screens))
    {
        return None;
    }
    let distance = |screen: &CGRect| {
        let dx = (screen.origin.x - window.max().x)
            .max(window.origin.x - screen.max().x)
            .max(0.0);
        let dy = (screen.origin.y - window.max().y)
            .max(window.origin.y - screen.max().y)
            .max(0.0);
        dx * dx + dy * dy
    };
    let screen = screens.iter().min_by(|a, b| distance(a).total_cmp(&distance(b)))?;
    // Keep the entire frame on its nearest display, including large windows.
    // The cascade exposes each title bar instead of restoring an identical pile.
    let x = (index as f64 * 28.0) % (screen.size.width / 3.0).max(1.0);
    let y = (index as f64 * 28.0) % (screen.size.height / 3.0).max(1.0);
    Some(CGRect::new(
        CGPoint::new(screen.origin.x + x, screen.origin.y + y),
        CGSize::new(
            window.size.width.min(screen.size.width - x),
            window.size.height.min(screen.size.height - y),
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::hidden_window_placement::HideCorner;

    fn rect(x: f64, y: f64, w: f64, h: f64) -> CGRect {
        CGRect::new(CGPoint::new(x, y), CGSize::new(w, h))
    }

    #[test]
    fn parked_windows_return_to_their_display_in_a_visible_cascade() {
        let screens = [
            rect(-1200.0, -800.0, 1200.0, 800.0),
            rect(0.0, 0.0, 1600.0, 1000.0),
        ];
        for screen in screens {
            let mut previous = None;
            for index in 0..12 {
                let corner = if index % 2 == 0 {
                    HideCorner::BottomLeft
                } else {
                    HideCorner::BottomRight
                };
                let parked = HiddenWindowPlacement::calculate(
                    screen,
                    rect(0.0, 0.0, 1100.0, 900.0),
                    corner,
                    &screens,
                );
                let restored = recovery_frame(parked, &screens, index)
                    .expect("parked window must be recovered");
                assert!(restored.origin.x >= screen.origin.x && restored.max().x <= screen.max().x);
                assert!(restored.origin.y >= screen.origin.y && restored.max().y <= screen.max().y);
                assert_ne!(previous, Some(restored.origin));
                previous = Some(restored.origin);
            }
        }
    }

    #[test]
    fn recovery_leaves_visible_windows_on_any_display_alone() {
        let screens = [
            rect(0.0, 0.0, 1000.0, 800.0),
            rect(1000.0, 0.0, 1000.0, 800.0),
        ];
        for window in [
            rect(10.0, 20.0, 500.0, 300.0),
            rect(1100.0, 20.0, 500.0, 300.0),
            rect(900.0, 20.0, 500.0, 300.0),
        ] {
            assert_eq!(recovery_frame(window, &screens, 0), None);
        }
        assert_eq!(recovery_frame(rect(0.0, 0.0, 500.0, 300.0), &[], 0), None);
    }
}
