use tracing::debug;

use crate::actor::app::WindowId;
use crate::actor::raise_manager;
use crate::actor::reactor::MenuState;
use crate::actor::reactor::events::EventOutcome;
use crate::actor::reactor::managers::{CommunicationManager, MenuManager};
use crate::actor::wm_controller::Sender as WmSender;

pub fn handle_menu_opened(
    menu: &mut MenuManager,
    pid: i32,
) -> Result<EventOutcome, crate::model::reactor::ReactorError> {
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
    Ok(EventOutcome::finalized_event(None, false, false, false).with_focus_follows_mouse_refresh())
}

pub fn handle_menu_closed(
    menu: &mut MenuManager,
    pid: i32,
) -> Result<EventOutcome, crate::model::reactor::ReactorError> {
    let refresh = match menu.menu_state {
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
    };
    let outcome = EventOutcome::finalized_event(None, false, false, false);
    if refresh {
        Ok(outcome.with_focus_follows_mouse_refresh())
    } else {
        Ok(outcome)
    }
}

pub fn handle_system_woke() -> Result<EventOutcome, crate::model::reactor::ReactorError> {
    Ok(EventOutcome::window_notification_refresh())
}

#[derive(Debug, Clone, Copy)]
pub struct RaiseCompletedPayload {
    pub window: WindowId,
    pub sequence: u64,
}

pub fn handle_raise_completed(
    payload: RaiseCompletedPayload,
) -> Result<EventOutcome, crate::model::reactor::ReactorError> {
    let RaiseCompletedPayload {
        window: window_id,
        sequence: sequence_id,
    } = payload;
    Ok(EventOutcome::finalized_event(None, false, false, false)
        .with_raise_request(raise_manager::Event::RaiseCompleted { window_id, sequence_id }))
}

pub fn handle_raise_timeout(
    sequence_id: u64,
) -> Result<EventOutcome, crate::model::reactor::ReactorError> {
    Ok(EventOutcome::finalized_event(None, false, false, false)
        .with_raise_request(raise_manager::Event::RaiseTimeout { sequence_id }))
}

pub fn handle_register_wm_sender(
    communication: &mut CommunicationManager,
    sender: WmSender,
) -> Result<EventOutcome, crate::model::reactor::ReactorError> {
    communication.wm_sender = Some(sender);
    Ok(EventOutcome::finalized_event(None, false, false, false))
}
