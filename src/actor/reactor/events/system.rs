use tracing::debug;

use crate::actor::app::WindowId;
use crate::actor::raise_manager;
use crate::actor::reactor::{MenuState, Reactor};
use crate::actor::wm_controller::Sender as WmSender;

pub struct SystemEventHandler;

impl SystemEventHandler {
    pub fn handle_menu_opened(reactor: &mut Reactor) {
        debug!("menu opened");
        reactor.menu_manager.menu_state = match reactor.menu_manager.menu_state {
            MenuState::Closed => MenuState::Open(1),
            MenuState::Open(depth) => {
                debug!(
                    depth,
                    "menu already open; ignoring duplicate menu-open notification"
                );
                MenuState::Open(depth)
            }
        };
        reactor.update_focus_follows_mouse_state();
    }

    pub fn handle_menu_closed(reactor: &mut Reactor) {
        match reactor.menu_manager.menu_state {
            MenuState::Closed => {
                debug!("menu closed with zero depth");
                // Reassert the expected focus-follows-mouse state in case we previously
                // got out-of-sync due to missing AX menu notifications.
                reactor.update_focus_follows_mouse_state();
            }
            MenuState::Open(depth) => {
                debug!(depth, "menu closed; clearing menu-open state");
                reactor.menu_manager.menu_state = MenuState::Closed;
                reactor.update_focus_follows_mouse_state();
            }
        }
    }

    pub fn handle_system_woke(reactor: &mut Reactor) {
        let ids: Vec<u32> =
            reactor.window_manager.window_ids.keys().map(|wsid| wsid.as_u32()).collect();
        crate::sys::window_notify::update_window_notifications(&ids);
        reactor.notification_manager.last_sls_notification_ids = ids;
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
