use tokio::sync::mpsc::error::SendError;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tracing::Span;

pub mod app;
pub mod broadcast;
pub mod menu_bar;
pub mod mouse;
pub mod notification_center;
pub mod raise_manager;
pub mod reactor;
pub mod stack_line;
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
