use tracing::{debug, warn};

use crate::actor::app::{Request, WindowId};
use crate::actor::raise_manager;
use crate::actor::reactor::{MenuState, Reactor};
use crate::actor::wm_controller::Sender as WmSender;
use crate::common::collections::HashMap;

pub struct SystemEventHandler;

impl SystemEventHandler {
    pub fn handle_menu_opened(reactor: &mut Reactor, pid: i32) {
        reactor.menu_manager.menu_state = match reactor.menu_manager.menu_state {
            MenuState::Closed => {
                debug!(pid, "menu opened");
                MenuState::Open(pid)
            }
            MenuState::Open(owner) if owner == pid => {
                debug!(
                    pid,
                    "menu already open for app; ignoring duplicate menu-open notification"
                );
                MenuState::Open(owner)
            }
            MenuState::Open(owner) => {
                debug!(
                    pid,
                    owner,
                    "menu-open owner changed without a close notification; replacing stale state"
                );
                MenuState::Open(pid)
            }
        };
        reactor.update_focus_follows_mouse_state();
    }

    pub fn handle_menu_closed(reactor: &mut Reactor, pid: i32) {
        match reactor.menu_manager.menu_state {
            MenuState::Closed => {
                debug!(pid, "menu closed while no menu was marked open");
                // Reassert the expected focus-follows-mouse state in case we previously
                // got out-of-sync due to missing AX menu notifications.
                reactor.update_focus_follows_mouse_state();
            }
            MenuState::Open(owner) if owner == pid => {
                debug!(pid, "menu closed; clearing menu-open state");
                reactor.menu_manager.menu_state = MenuState::Closed;
                reactor.update_focus_follows_mouse_state();
            }
            MenuState::Open(owner) => {
                debug!(
                    pid,
                    owner, "ignoring menu-closed notification for non-owning app"
                );
            }
        }
    }

    pub fn handle_system_woke(reactor: &mut Reactor) {
        let ids: Vec<u32> =
            reactor.window_manager.window_ids.keys().map(|wsid| wsid.as_u32()).collect();
        crate::sys::window_notify::update_window_notifications(&ids);
        reactor.notification_manager.last_sls_notification_ids = ids;

        // Sleep/wake can interrupt both deferred native-tab destroys and ordinary
        // close/removal cleanup, leaving stale tracked slots in layout for still-running
        // apps like Finder. Re-probe one tracked window per pid so the app actor emits a
        // current visible-window snapshot even when the app itself remains alive.
        let mut probe_windows_by_pid: HashMap<i32, WindowId> = reactor
            .window_manager
            .windows
            .keys()
            .copied()
            .map(|wid| (wid.pid, wid))
            .collect();
        for pending in reactor.pending_native_tab_destroys() {
            probe_windows_by_pid.entry(pending.window_id.pid).or_insert(pending.window_id);
        }

        for window_id in probe_windows_by_pid.into_values() {
            let Some(app) = reactor.app_manager.apps.get(&window_id.pid) else {
                continue;
            };
            if let Err(err) = app.handle.send(Request::WindowMaybeDestroyed(window_id)) {
                warn!(
                    pid = window_id.pid,
                    wid = ?window_id,
                    ?err,
                    "Failed to verify tracked windows after wake"
                );
            }
        }
        reactor.refresh_all_windows_without_pending_refresh();
    }

    pub fn handle_raise_completed(reactor: &mut Reactor, window_id: WindowId, sequence_id: u64) {
        send_raise_event(reactor, raise_manager::Event::RaiseCompleted {
            window_id,
            sequence_id,
        });
    }

    pub fn handle_raise_timeout(reactor: &mut Reactor, sequence_id: u64) {
        send_raise_event(reactor, raise_manager::Event::RaiseTimeout { sequence_id });
    }

    pub fn handle_register_wm_sender(reactor: &mut Reactor, sender: WmSender) {
        reactor.communication_manager.wm_sender = Some(sender);
    }
}

fn send_raise_event(reactor: &mut Reactor, event: raise_manager::Event) {
    _ = reactor.communication_manager.raise_manager_tx.send(event);
}
