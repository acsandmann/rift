use super::Event;
use crate::actor::app::{Quiet, WindowId, pid_t};
use crate::common::collections::HashMap;

#[derive(Default)]
pub(crate) struct MainWindowTracker {
    apps: HashMap<pid_t, AppState>,
    global_frontmost: Option<pid_t>,
    window_server_focus: Option<WindowId>,
    window_server_focus_authoritative: bool,
    /// pid whose WindowServer key window was destroyed before the resolver reported a
    /// successor (native tab switch). Keeps the key-pid gate armed until
    /// `WindowServerFocusChanged` or another pid becomes frontmost.
    key_focus_hold: Option<pid_t>,
}

struct AppState {
    is_frontmost: bool,
    frontmost_is_quiet: Quiet,
    main_window: Option<WindowId>,
}

impl MainWindowTracker {
    #[must_use]
    pub fn handle_event(&mut self, event: &Event) -> Option<WindowId> {
        let (event_pid, quiet_edge) = match event {
            &Event::ApplicationLaunched {
                pid, is_frontmost, main_window, ..
            } => {
                self.apps.insert(
                    pid,
                    AppState {
                        is_frontmost,
                        frontmost_is_quiet: Quiet::No,
                        main_window,
                    },
                );
                (pid, Quiet::No)
            }
            &Event::ApplicationThreadTerminated(pid) => {
                self.apps.remove(&pid);
                if self.window_server_focus.is_some_and(|wid| wid.pid == pid) {
                    self.window_server_focus = None;
                }
                if self.key_focus_hold == Some(pid) {
                    self.key_focus_hold = None;
                }
                return None;
            }
            &Event::WindowDestroyed(wid) => {
                if self.window_server_focus == Some(wid) {
                    self.window_server_focus = None;
                    self.key_focus_hold = Some(wid.pid);
                }
                return None;
            }
            &Event::ApplicationActivated(pid, quiet) => {
                let app = self.apps.get_mut(&pid)?;
                app.is_frontmost = true;
                app.frontmost_is_quiet = quiet;
                (pid, quiet)
            }
            &Event::ApplicationDeactivated(pid) => {
                let app = self.apps.get_mut(&pid)?;
                app.is_frontmost = false;
                return None;
            }
            &Event::ApplicationGloballyActivated(pid) => {
                self.global_frontmost = Some(pid);
                if self.key_focus_hold.is_some_and(|hold| hold != pid) {
                    self.key_focus_hold = None;
                }
                let Some(app) = self.apps.get_mut(&pid) else {
                    return None;
                };
                app.is_frontmost = true;
                (pid, app.frontmost_is_quiet)
            }
            &Event::ApplicationGloballyDeactivated(pid) => {
                if self.global_frontmost == Some(pid) {
                    self.global_frontmost = None;
                }
                if let Some(app) = self.apps.get_mut(&pid) {
                    app.is_frontmost = false;
                }
                return None;
            }
            &Event::ApplicationMainWindowChanged(pid, wid, quiet) => {
                let app = self.apps.get_mut(&pid)?;
                app.main_window = wid;
                (pid, quiet)
            }
            &Event::WindowServerFocusChanged(wid, _) => {
                self.window_server_focus_authoritative = true;
                self.window_server_focus = Some(wid);
                self.key_focus_hold = None;
                return None;
            }
            _ => return None,
        };
        // Once WindowServer focus has produced a result, AX activation/main-window
        // events remain useful as metadata and cold-start fallback only. Letting
        // them emit focus here can replay the previous native focus while the new
        // 808/815 resolution is still in flight.
        if self.window_server_focus_authoritative {
            return None;
        }
        if Some(event_pid) == self.global_frontmost && quiet_edge == Quiet::No {
            if let Some(wid) = self.main_window() {
                return Some(wid);
            }
        }
        None
    }

    pub fn main_window(&self) -> Option<WindowId> {
        let Some(pid) = self.global_frontmost else {
            return None;
        };
        if let Some(window) = self.window_server_focus.filter(|window| window.pid == pid) {
            return Some(window);
        }
        match self.apps.get(&pid) {
            Some(&AppState {
                is_frontmost: true,
                main_window: Some(window),
                ..
            }) => Some(window),
            _ => None,
        }
    }

    pub fn is_globally_frontmost(&self, pid: pid_t) -> bool {
        self.global_frontmost == Some(pid)
    }

    pub fn window_server_focus(&self) -> Option<WindowId> {
        self.window_server_focus
    }

    /// The pid the OS says owns key focus, only while that claim agrees with the
    /// globally frontmost app.
    pub fn key_pid(&self) -> Option<pid_t> {
        let front = self.global_frontmost?;
        let claimed = self.window_server_focus.map(|w| w.pid).or(self.key_focus_hold)?;
        (claimed == front).then_some(front)
    }
}

#[cfg(test)]
mod tests {
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};
    use test_log::test;

    use super::super::testing::{Apps, make_windows, space_state_event};
    use super::super::{Event, Quiet, Reactor, SpaceId, WindowId};
    use super::{AppState, MainWindowTracker};
    use crate::layout_engine::LayoutEngine;

    #[test]
    fn window_server_focus_supersedes_ax_focus_events() {
        let ax_window = WindowId::new(7, 1);
        let server_window = WindowId::new(7, 2);
        let stale_window = WindowId::new(7, 3);
        let mut tracker = MainWindowTracker::default();
        tracker.global_frontmost = Some(7);
        tracker.apps.insert(
            7,
            AppState {
                is_frontmost: true,
                frontmost_is_quiet: Quiet::No,
                main_window: Some(ax_window),
            },
        );

        assert_eq!(tracker.main_window(), Some(ax_window));
        assert_eq!(
            tracker.handle_event(&Event::WindowServerFocusChanged(server_window, SpaceId::new(1),)),
            None
        );
        assert_eq!(tracker.main_window(), Some(server_window));

        assert_eq!(
            tracker.handle_event(&Event::ApplicationMainWindowChanged(
                7,
                Some(stale_window),
                Quiet::No,
            )),
            None,
            "AX must not drive focus after native authority is initialized"
        );
        assert_eq!(tracker.main_window(), Some(server_window));

        let _ = tracker.handle_event(&Event::ApplicationMainWindowChanged(
            7,
            Some(ax_window),
            Quiet::No,
        ));

        let _ = tracker.handle_event(&Event::WindowDestroyed(server_window));
        assert_eq!(tracker.main_window(), Some(ax_window));
    }

    #[test]
    fn it_tracks_frontmost_app_and_main_window_correctly() {
        use Event::*;
        let mut apps = Apps::new();
        let mut reactor = Reactor::new_for_test(LayoutEngine::new(
            &crate::common::config::VirtualWorkspaceSettings::default(),
            &crate::common::config::LayoutSettings::default(),
            None,
        ));
        let space = SpaceId::new(1);
        let screen_frame = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1920., 1080.));
        reactor.handle_event(space_state_event(vec![screen_frame], vec![Some(space)]));
        assert_eq!(None, reactor.main_window());

        reactor.handle_event(ApplicationGloballyActivated(1));
        reactor.handle_events(apps.make_app_with_opts(
            1,
            make_windows(2),
            Some(WindowId::new(1, 1)),
            true,
            true,
        ));
        reactor.handle_events(apps.make_app_with_opts(2, make_windows(2), None, false, true));
        assert_eq!(Some(WindowId::new(1, 1)), reactor.main_window());
        assert_eq!(
            reactor.layout_manager.layout_engine.selected_window(space),
            Some(WindowId::new(1, 1))
        );

        reactor.handle_event(ApplicationGloballyDeactivated(1));
        assert_eq!(None, reactor.main_window());
        reactor.handle_event(ApplicationActivated(2, Quiet::No));
        reactor.handle_event(ApplicationGloballyActivated(2));
        assert_eq!(None, reactor.main_window());
        reactor.handle_event(ApplicationMainWindowChanged(
            2,
            Some(WindowId::new(2, 2)),
            Quiet::No,
        ));
        assert_eq!(Some(WindowId::new(2, 2)), reactor.main_window());
        assert_eq!(
            reactor.layout_manager.layout_engine.selected_window(space),
            Some(WindowId::new(2, 2))
        );
        reactor.handle_event(ApplicationMainWindowChanged(
            1,
            Some(WindowId::new(1, 2)),
            Quiet::No,
        ));
        assert_eq!(Some(WindowId::new(2, 2)), reactor.main_window());
        reactor.handle_event(ApplicationDeactivated(1));
        assert_eq!(Some(WindowId::new(2, 2)), reactor.main_window());
        reactor.handle_event(ApplicationDeactivated(2));
        assert_eq!(None, reactor.main_window());

        reactor.handle_event(ApplicationGloballyActivated(3));
        assert_eq!(None, reactor.main_window());

        reactor.handle_events(apps.make_app_with_opts(
            3,
            make_windows(2),
            Some(WindowId::new(3, 1)),
            true,
            true,
        ));
        assert_eq!(Some(WindowId::new(3, 1)), reactor.main_window());
        assert_eq!(
            reactor.layout_manager.layout_engine.selected_window(space),
            Some(WindowId::new(3, 1))
        );
    }

    #[test]
    fn it_does_not_update_layout_for_quiet_raises() {
        use Event::*;
        let mut apps = Apps::new();
        let mut reactor = Reactor::new_for_test(LayoutEngine::new(
            &crate::common::config::VirtualWorkspaceSettings::default(),
            &crate::common::config::LayoutSettings::default(),
            None,
        ));
        let space = SpaceId::new(1);
        let screen_frame = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1920., 1080.));
        reactor.handle_event(space_state_event(vec![screen_frame], vec![Some(space)]));

        reactor.handle_event(ApplicationGloballyActivated(1));
        reactor.handle_events(apps.make_app_with_opts(
            1,
            make_windows(2),
            Some(WindowId::new(1, 1)),
            true,
            true,
        ));
        reactor.handle_events(apps.make_app_with_opts(2, make_windows(2), None, false, true));
        assert_eq!(Some(WindowId::new(1, 1)), reactor.main_window());
        assert_eq!(
            reactor.layout_manager.layout_engine.selected_window(space),
            Some(WindowId::new(1, 1))
        );

        reactor.handle_event(ApplicationGloballyDeactivated(1));
        assert_eq!(None, reactor.main_window());
        reactor.handle_event(ApplicationGloballyActivated(2));
        reactor.handle_event(ApplicationActivated(2, Quiet::Yes));
        assert_eq!(None, reactor.main_window());
        reactor.handle_event(ApplicationMainWindowChanged(
            2,
            Some(WindowId::new(2, 2)),
            Quiet::Yes,
        ));
        assert_eq!(Some(WindowId::new(2, 2)), reactor.main_window());
        assert_eq!(
            reactor.layout_manager.layout_engine.selected_window(space),
            Some(WindowId::new(1, 1))
        );

        reactor.handle_event(ApplicationActivated(2, Quiet::No));
        assert_eq!(
            reactor.layout_manager.layout_engine.selected_window(space),
            Some(WindowId::new(2, 2))
        );

        reactor.handle_event(ApplicationMainWindowChanged(
            2,
            Some(WindowId::new(2, 1)),
            Quiet::Yes,
        ));
        assert_eq!(Some(WindowId::new(2, 1)), reactor.main_window());
        assert_eq!(
            reactor.layout_manager.layout_engine.selected_window(space),
            Some(WindowId::new(2, 2))
        );

        reactor.handle_event(ApplicationActivated(1, Quiet::Yes));
        reactor.handle_event(ApplicationGloballyActivated(1));
        assert_eq!(Some(WindowId::new(1, 1)), reactor.main_window());
        assert_eq!(
            reactor.layout_manager.layout_engine.selected_window(space),
            Some(WindowId::new(2, 2))
        );

        reactor.handle_event(ApplicationMainWindowChanged(
            1,
            Some(WindowId::new(1, 2)),
            Quiet::No,
        ));
        assert_eq!(Some(WindowId::new(1, 2)), reactor.main_window());
        assert_eq!(
            reactor.layout_manager.layout_engine.selected_window(space),
            Some(WindowId::new(1, 2))
        );
    }

    #[test]
    fn it_selects_main_window_when_space_is_enabled() {
        use Event::*;
        let mut apps = Apps::new();
        let mut reactor = Reactor::new_for_test(LayoutEngine::new(
            &crate::common::config::VirtualWorkspaceSettings::default(),
            &crate::common::config::LayoutSettings::default(),
            None,
        ));
        let pid = 3;
        let windows = make_windows(2);
        let space = SpaceId::new(1);
        let screen_frame = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1920., 1080.));
        reactor.handle_event(space_state_event(vec![screen_frame], vec![Some(space)]));

        reactor.handle_events(apps.make_app_with_opts(
            pid,
            windows,
            Some(WindowId::new(3, 1)),
            false,
            true,
        ));

        reactor.handle_event(space_state_event(vec![screen_frame], vec![None]));
        reactor.handle_event(ApplicationActivated(3, Quiet::No));
        reactor.handle_event(ApplicationGloballyActivated(3));
        reactor.handle_event(WindowsDiscovered {
            pid,
            new: vec![],
            known_visible: vec![WindowId::new(3, 1), WindowId::new(3, 2)],
        });
        assert_eq!(Some(WindowId::new(3, 1)), reactor.main_window());

        reactor.handle_event(space_state_event(vec![screen_frame], vec![Some(space)]));
        assert_eq!(
            reactor.layout_manager.layout_engine.selected_window(space),
            Some(WindowId::new(3, 1))
        );
    }

    #[test]
    fn key_pid_survives_key_window_destroy_until_resolver_or_app_switch() {
        let mut tracker = MainWindowTracker::default();
        tracker.global_frontmost = Some(7);
        // Initial focus via WindowServer
        let w2 = WindowId::new(7, 2);
        let w3 = WindowId::new(7, 3);
        assert_eq!(
            tracker.handle_event(&Event::WindowServerFocusChanged(w2, SpaceId::new(1))),
            None
        );
        assert_eq!(tracker.key_pid(), Some(7));
        assert_eq!(tracker.window_server_focus(), Some(w2));
        // Destroy key window -> hold keeps key_pid armed, window_server_focus cleared
        let _ = tracker.handle_event(&Event::WindowDestroyed(w2));
        assert_eq!(tracker.window_server_focus(), None);
        assert_eq!(
            tracker.key_pid(),
            Some(7),
            "hold should keep key_pid after destroy"
        );
        // Resolver reports successor tab
        assert_eq!(
            tracker.handle_event(&Event::WindowServerFocusChanged(w3, SpaceId::new(1))),
            None
        );
        assert_eq!(tracker.window_server_focus(), Some(w3));
        assert_eq!(tracker.key_pid(), Some(7));
        // Now test branch: after destroy, app switch clears hold
        let mut tracker2 = MainWindowTracker::default();
        tracker2.global_frontmost = Some(7);
        let _ = tracker2.handle_event(&Event::WindowServerFocusChanged(w2, SpaceId::new(1)));
        let _ = tracker2.handle_event(&Event::WindowDestroyed(w2));
        assert_eq!(tracker2.key_pid(), Some(7));
        // Switch to different pid -> disarms
        let _ = tracker2.handle_event(&Event::ApplicationGloballyActivated(8));
        assert_eq!(tracker2.key_pid(), None, "switch to 8 should clear hold");
        // Switching back to 7 still None because hold was cleared (no resolver yet)
        let _ = tracker2.handle_event(&Event::ApplicationGloballyActivated(7));
        assert_eq!(tracker2.key_pid(), None);
        // Duplicate activation for same pid should keep hold
        let mut tracker3 = MainWindowTracker::default();
        tracker3.global_frontmost = Some(7);
        let _ = tracker3.handle_event(&Event::WindowServerFocusChanged(w2, SpaceId::new(1)));
        let _ = tracker3.handle_event(&Event::WindowDestroyed(w2));
        assert_eq!(tracker3.key_pid(), Some(7));
        let _ = tracker3.handle_event(&Event::ApplicationGloballyActivated(7));
        assert_eq!(
            tracker3.key_pid(),
            Some(7),
            "duplicate activation for same pid must not clear hold"
        );
        // WindowServerFocusChanged for different pid while front is 7 -> None due to coherence check
        let mut tracker4 = MainWindowTracker::default();
        tracker4.global_frontmost = Some(7);
        let _ = tracker4.handle_event(&Event::WindowServerFocusChanged(w2, SpaceId::new(1)));
        assert_eq!(tracker4.key_pid(), Some(7));
        let other = WindowId::new(8, 1);
        let _ = tracker4.handle_event(&Event::WindowServerFocusChanged(other, SpaceId::new(1)));
        assert_eq!(
            tracker4.key_pid(),
            None,
            "claimed pid 8 != front 7 should be None"
        );
    }
}
