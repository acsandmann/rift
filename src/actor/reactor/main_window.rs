use super::Event;
use crate::actor::app::{Quiet, WindowId, pid_t};
use crate::common::collections::HashMap;

#[derive(Default)]
pub(crate) struct MainWindowTracker {
    apps: HashMap<pid_t, AppState>,
    global_frontmost: Option<pid_t>,
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
                self.apps.insert(pid, AppState {
                    is_frontmost,
                    frontmost_is_quiet: Quiet::No,
                    main_window,
                });
                (pid, Quiet::No)
            }
            &Event::ApplicationThreadTerminated(pid) => {
                self.apps.remove(&pid);
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
                let Some(app) = self.apps.get(&pid) else { return None };
                (pid, app.frontmost_is_quiet)
            }
            &Event::ApplicationGloballyDeactivated(pid) => {
                if self.global_frontmost == Some(pid) {
                    self.global_frontmost = None;
                }
                return None;
            }
            &Event::ApplicationMainWindowChanged(pid, wid, quiet) => {
                let app = self.apps.get_mut(&pid)?;
                app.main_window = wid;
                (pid, quiet)
            }
            _ => return None,
        };
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
        match self.apps.get(&pid) {
            Some(&AppState {
                is_frontmost: true,
                main_window: Some(window),
                ..
            }) => Some(window),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use objc2_core_foundation::CGRect;
    use test_log::test;

    use super::super::testing::{Apps, make_windows, screen_params_event, screen_space_list};
    use super::super::{Event, Quiet, Reactor, SpaceId, WindowId};
    use crate::layout_engine::LayoutEngine;

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
        reactor.handle_event(screen_params_event(
            vec![CGRect::ZERO],
            vec![Some(space)],
            vec![],
        ));
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
            Some(WindowId::new(2, 2))
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
        reactor.handle_event(screen_params_event(
            vec![CGRect::ZERO],
            vec![Some(space)],
            vec![],
        ));

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
            Some(WindowId::new(2, 2))
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
            Some(WindowId::new(2, 2))
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
        reactor.handle_event(screen_params_event(
            vec![CGRect::ZERO],
            vec![Some(space)],
            vec![],
        ));

        reactor.handle_events(apps.make_app_with_opts(
            pid,
            windows,
            Some(WindowId::new(3, 1)),
            false,
            true,
        ));

        reactor.handle_event(SpaceChanged(screen_space_list(vec![None]), vec![]));
        reactor.handle_event(ApplicationActivated(3, Quiet::No));
        reactor.handle_event(ApplicationGloballyActivated(3));
        reactor.handle_event(WindowsDiscovered {
            pid,
            new: vec![],
            known_visible: vec![WindowId::new(3, 1), WindowId::new(3, 2)],
        });
        assert_eq!(Some(WindowId::new(3, 1)), reactor.main_window());

        reactor.handle_event(SpaceChanged(screen_space_list(vec![Some(space)]), vec![]));
        assert_eq!(
            reactor.layout_manager.layout_engine.selected_window(space),
            Some(WindowId::new(3, 1))
        );
    }
}
