use tracing::debug;

use crate::actor::reactor::MenuState;
use crate::actor::reactor::managers::MenuManager;

pub fn handle_menu_opened(menu: &mut MenuManager, pid: i32) {
    menu.menu_state = match menu.menu_state {
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
}

pub fn handle_menu_closed(menu: &mut MenuManager, pid: i32) -> bool {
    match menu.menu_state {
        MenuState::Closed => {
            debug!(pid, "menu closed while no menu was marked open");
            true
        }
        MenuState::Open(owner) if owner == pid => {
            debug!(pid, "menu closed; clearing menu-open state");
            menu.menu_state = MenuState::Closed;
            true
        }
        MenuState::Open(owner) => {
            debug!(
                pid,
                owner, "ignoring menu-closed notification for non-owning app"
            );
            false
        }
    }
}
