use std::num::NonZeroU32;
use std::sync::Arc;

use tracing::debug;

use super::reactor::{self, Event};
use crate::actor::app::WindowId;
use crate::actor::reactor::Requested;
use crate::common::collections::{HashMap, HashSet};
use crate::model::tx_store::WindowTxStore;
use crate::sys::screen::SpaceId;
use crate::sys::skylight::{CGSEventType, KnownCGSEvent};
use crate::sys::window_server::{WindowQuery, WindowServerId};
use crate::sys::{event, window_notify};

#[derive(Default)]
pub struct Ignored {
    by_event: HashMap<u32, Arc<HashSet<u32>>>,
}

impl Ignored {
    pub fn empty() -> Self { Self { by_event: HashMap::default() } }

    #[inline]
    pub fn is_ignored(&self, event: CGSEventType, wsid: u32) -> bool {
        self.by_event.get(&event.into()).map_or(false, |set| set.contains(&wsid))
    }

    pub fn with_added(&self, event: CGSEventType, wsid: u32) -> Arc<Ignored> {
        let code = event.into();
        if self.is_ignored(event, wsid) {
            return Arc::new(self.clone());
        }
        let mut next_map = self.by_event.clone();
        let mut next_set = next_map.get(&code).map(|s| (**s).clone()).unwrap_or_default();
        next_set.insert(wsid);
        next_map.insert(code, Arc::new(next_set));
        Arc::new(Ignored { by_event: next_map })
    }

    pub fn with_removed(&self, event: CGSEventType, wsid: u32) -> Arc<Ignored> {
        let code = event.into();
        let Some(set_arc) = self.by_event.get(&code) else {
            return Arc::new(self.clone());
        };
        if !set_arc.contains(&wsid) {
            return Arc::new(self.clone());
        }
        let mut next_map = self.by_event.clone();
        let mut next_set = (**set_arc).clone();
        next_set.remove(&wsid);
        if next_set.is_empty() {
            next_map.remove(&code);
        } else {
            next_map.insert(code, Arc::new(next_set));
        }
        Arc::new(Ignored { by_event: next_map })
    }
}

impl Clone for Ignored {
    fn clone(&self) -> Self {
        Self {
            by_event: self.by_event.clone(),
        }
    }
}

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
    tx_store: Option<WindowTxStore>,
}

impl WindowNotify {
    pub fn new(
        events_tx: reactor::Sender,
        requests_rx: Receiver,
        initial_events: &[CGSEventType],
        tx_store: Option<WindowTxStore>,
    ) -> Self {
        Self {
            events_tx,
            requests_rx: Some(requests_rx),
            subscribed: HashSet::default(),
            initial_events: initial_events.iter().copied().collect(),
            tx_store,
        }
    }

    pub async fn run(mut self) {
        let mut requests_rx = match self.requests_rx.take() {
            Some(rx) => rx,
            None => return,
        };

        for event in self.initial_events.drain(..) {
            match Self::subscribe(event, self.events_tx.clone(), self.tx_store.clone()) {
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
            let _g = span.enter();
            if let Request::Stop = request {
                debug!("received Stop request");
                break;
            }
            self.handle_request(request);
        }

        debug!("WindowNotify actor exiting");
    }

    fn handle_request(&mut self, request: Request) {
        match request {
            Request::Subscribe(event) => {
                if self.subscribed.contains(&event) {
                    debug!("already subscribed to event {}", event);
                    return;
                }
                match Self::subscribe(event, self.events_tx.clone(), self.tx_store.clone()) {
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

    fn subscribe(
        event: CGSEventType,
        events_tx: reactor::Sender,
        tx_store: Option<WindowTxStore>,
    ) -> Result<(), i32> {
        let res = window_notify::init(event);
        if res != 0 {
            return Err(res);
        }

        let mut rx = window_notify::take_receiver(event);

        std::thread::spawn(move || {
            while let Some((_span, evt)) = rx.blocking_recv() {
                if let Some(window_id) = evt.window_id {
                    match event {
                        CGSEventType::Known(KnownCGSEvent::SpaceWindowDestroyed) => {
                            events_tx.send(Event::WindowServerDestroyed(
                                WindowServerId::new(window_id),
                                SpaceId::new(evt.space_id.unwrap()),
                            ))
                        }
                        CGSEventType::Known(KnownCGSEvent::SpaceWindowCreated) => {
                            events_tx.send(Event::WindowServerAppeared(
                                WindowServerId::new(window_id),
                                SpaceId::new(evt.space_id.unwrap()),
                            ))
                        }
                        CGSEventType::Known(KnownCGSEvent::WorkspaceWindowIsViewable)
                        | CGSEventType::Known(KnownCGSEvent::WorkspaceWindowIsNotViewable)
                        | CGSEventType::Known(
                            KnownCGSEvent::WorkspacesWindowDidOrderInOnNonCurrentManagedSpacesOnly,
                        )
                        | CGSEventType::Known(
                            KnownCGSEvent::WorkspacesWindowDidOrderOutOnNonCurrentManagedSpaces,
                        ) => {
                            events_tx
                                .send(Event::ResyncAppForWindow(WindowServerId::new(window_id)));
                        }
                        CGSEventType::Known(KnownCGSEvent::WindowMoved)
                        | CGSEventType::Known(KnownCGSEvent::WindowResized) => {
                            // TODO: suppress move/resize while Mission Control is active
                            let mouse_state = event::get_mouse_state();
                            let wsid = WindowServerId::new(window_id);
                            if let Some(query) = WindowQuery::new(&[wsid]) {
                                if query.advance().is_none() {
                                    continue;
                                }
                                let bounds = query.bounds();
                                let pid = query.pid();
                                if let Some(idx) = NonZeroU32::new(window_id) {
                                    let last_seen = tx_store
                                        .as_ref()
                                        .and_then(|store| store.get(&wsid))
                                        .map(|record| record.txid);
                                    events_tx.send(Event::WindowFrameChanged(
                                        WindowId { idx, pid },
                                        bounds,
                                        last_seen,
                                        Requested(false),
                                        mouse_state,
                                    ));
                                }
                            };
                        }
                        _ => {}
                    }
                }
            }
        });

        Ok(())
    }
}
