use tokio::sync::mpsc::error::SendError;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tracing::Span;

pub mod app;
pub mod broadcast;
pub mod config;
pub mod config_watcher;
pub mod drag_swap;
pub mod event_tap;
pub mod menu_bar;
pub mod mission_control;
pub mod mission_control_observer;
pub mod notification_center;
pub mod process;
pub mod raise_manager;
pub mod reactor;
pub mod stack_line;
pub mod window_notify;
pub mod wm_controller;

pub struct Sender<Event>(UnboundedSender<(Span, Event)>);
pub type Receiver<Event> = UnboundedReceiver<(Span, Event)>;

pub fn channel<Event>() -> (Sender<Event>, Receiver<Event>) {
    let (tx, rx) = unbounded_channel();
    (Sender(tx), rx)
}

impl<Event> Sender<Event> {
    pub fn send(&self, event: Event) {
        // Most of the time we can ignore send errors, they just indicate the
        // app is shutting down.
        _ = self.try_send(event)
    }

    pub fn try_send(&self, event: Event) -> Result<(), SendError<(Span, Event)>> {
        self.0.send((Span::current(), event))
    }
}

impl<Event> Clone for Sender<Event> {
    fn clone(&self) -> Self { Self(self.0.clone()) }
}

impl<Event> std::fmt::Debug for Sender<Event> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("actor::Sender(...)")
    }
}
