use tracing::{debug, trace};

use super::reactor::{self, Event};
use crate::common::collections::HashSet;
use crate::sys::skylight::CGSEventType;
use crate::sys::window_notify::{self};
use crate::sys::window_server::WindowServerId;

#[derive(Debug)]
pub enum Request {
    Subscribe(CGSEventType),
    UpdateWindowNotifications(Vec<u32>),
    Stop,
}

pub type Sender = crate::actor::Sender<Request>;
pub type Receiver = crate::actor::Receiver<Request>;

pub struct WindowNotify {
    events_tx: reactor::Sender,
    requests_rx: Option<Receiver>,
    subscribed: HashSet<CGSEventType>,
    initial_events: Vec<CGSEventType>,
}

impl WindowNotify {
    pub fn new(
        events_tx: reactor::Sender,
        requests_rx: Receiver,
        initial_events: &[CGSEventType],
    ) -> Self {
        Self {
            events_tx,
            requests_rx: Some(requests_rx),
            subscribed: HashSet::default(),
            initial_events: initial_events.into_iter().copied().collect(),
        }
    }

    pub async fn run(mut self) {
        let mut requests_rx = match self.requests_rx.take() {
            Some(rx) => rx,
            None => return,
        };

        for event in self.initial_events.drain(..) {
            match Self::subscribe(event, self.events_tx.clone()) {
                Ok(()) => {
                    self.subscribed.insert(event);
                    debug!("initial subscription succeeded for event {}", event);
                }
                Err(code) => {
                    debug!("initial subscribe for {} failed (res={})", event, code);
                }
            }
        }

        while let Some((span, request)) = requests_rx.recv().await {
            let _ = span.enter();
            if let Request::Stop = request {
                debug!("received Stop request");
                break;
            }

            self.handle_request(request);
        }

        debug!("exiting run loop");
    }

    fn handle_request(&mut self, request: Request) {
        match request {
            Request::Subscribe(event) => {
                if self.subscribed.contains(&event) {
                    debug!("already subscribed to event {}", event);
                    return;
                }

                match Self::subscribe(event, self.events_tx.clone()) {
                    Ok(()) => {
                        self.subscribed.insert(event);
                        debug!("subscribed to event {}", event);
                    }
                    Err(code) => {
                        debug!("failed to register event {} (res={})", event, code);
                    }
                }
            }

            Request::UpdateWindowNotifications(window_ids) => {
                window_notify::update_window_notifications(&window_ids);
            }

            Request::Stop => {}
        }
    }

    fn subscribe(event: CGSEventType, events_tx: reactor::Sender) -> Result<(), i32> {
        let res = window_notify::init(event);
        if res != 0 {
            return Err(res);
        }

        let mut rx = window_notify::take_receiver(event);

        std::thread::spawn(move || {
            loop {
                match rx.blocking_recv() {
                    Some(evt) => {
                        if let Some(window_id) = evt.1.window_id {
                            match event {
                                CGSEventType::ServerWindowDidTerminate
                                | CGSEventType::WindowDestroyed => events_tx.send(
                                    Event::WindowServerDestroyed(WindowServerId::new(window_id)),
                                ),
                                CGSEventType::ServerWindowDidCreate => events_tx.send(
                                    Event::WindowServerAppeared(WindowServerId::new(window_id)),
                                ),
                                // TODO: handle WindowCreated and confirm when it is triggered
                                CGSEventType::WindowCreated
                                | CGSEventType::WindowMoved
                                | CGSEventType::WindowResized => {}
                            }
                        } else {
                            trace!("event had no window id: {:?}", evt);
                        }
                    }
                    None => break,
                }
            }
        });

        Ok(())
    }
}
