use std::ffi::CString;
use std::os::raw::c_char;
use std::sync::Arc;
use std::thread;

use crossbeam_channel::{Sender, TrySendError, bounded};
use dashmap::DashMap;
use dashmap::mapref::entry::Entry;
use parking_lot::Mutex;
use serde_json::Value;
use tracing::{debug, error, info, warn};

use crate::common::collections::{HashMap, HashSet};
use crate::model::broadcast::BroadcastEvent;
use crate::sys::mach::{
    mach_release_send_right, mach_retain_send_right, mach_try_send_message,
    mach_unwatch_send_right, mach_watch_send_right,
};

pub type ClientPort = u32;

#[derive(Clone, Debug)]
pub struct CliSubscription {
    pub command: String,
    pub args: Vec<String>,
}

pub struct ServerState {
    subscriptions_by_client: Arc<DashMap<ClientPort, Vec<String>>>,
    subscriptions_by_event: Arc<DashMap<String, Vec<ClientPort>>>,
    cli_subscriptions: Arc<Mutex<HashMap<String, Vec<CliSubscription>>>>,
    event_dispatch_tx: Sender<DispatchBatch>,
    /// Independent reasons to keep each client's send right and its one dead-name watch.
    leases: Mutex<HashMap<ClientPort, u8>>,
}

/// A use of a client port that must end when the client's receive right dies.
#[derive(Clone, Copy)]
#[repr(u8)]
pub enum PortLease {
    Subscriptions = 1,
    Claims = 2,
}

/// Subscription state is internally synchronized by `DashMap`, `Mutex`, and
/// the dispatch channel. An outer lock would serialize otherwise independent
/// subscription and publication operations without protecting any additional
/// invariant.
pub type SharedServerState = Arc<ServerState>;

const EVENT_DISPATCH_QUEUE_CAPACITY: usize = 4096;

struct DispatchBatch {
    event_json: String,
    targets: Vec<ClientPort>,
}

impl ServerState {
    pub fn new() -> Self {
        let subscriptions_by_client = Arc::new(DashMap::new());
        let subscriptions_by_event = Arc::new(DashMap::new());
        let cli_subscriptions = Arc::new(Mutex::new(HashMap::default()));
        let (event_dispatch_tx, event_dispatch_rx) = bounded(EVENT_DISPATCH_QUEUE_CAPACITY);
        thread::spawn(move || Self::run_event_dispatch_worker(event_dispatch_rx));

        Self {
            subscriptions_by_client,
            subscriptions_by_event,
            cli_subscriptions,
            event_dispatch_tx,
            leases: Mutex::default(),
        }
    }

    /// Hold or drop one lease. The first lease retains the send right and requests the
    /// port's dead-name notification; the last one cancels the request and releases it.
    pub fn set_lease(&self, port: ClientPort, lease: PortLease, held: bool) -> bool {
        let mut leases = self.leases.lock();
        let old = leases.get(&port).copied().unwrap_or(0);
        let new = if held {
            old | lease as u8
        } else {
            old & !(lease as u8)
        };
        if old == 0 && new != 0 {
            if !unsafe { mach_retain_send_right(port) } {
                return false;
            }
            if !unsafe { mach_watch_send_right(port) } {
                let _ = unsafe { mach_release_send_right(port) };
                return false;
            }
        } else if old != 0 && new == 0 {
            unsafe { mach_unwatch_send_right(port) };
            let _ = unsafe { mach_release_send_right(port) };
        }
        if new == 0 {
            leases.remove(&port);
        } else {
            leases.insert(port, new);
        }
        true
    }

    pub fn subscribe_client(&self, client_port: ClientPort, event: String) -> bool {
        info!("Client {} subscribing to event: {}", client_port, event);
        let mut added = false;

        match self.subscriptions_by_client.entry(client_port) {
            Entry::Occupied(mut entry) => {
                let subs = entry.get_mut();
                if !subs.contains(&event) {
                    subs.push(event.clone());
                    added = true;
                }
            }
            Entry::Vacant(entry) => {
                if !self.set_lease(client_port, PortLease::Subscriptions, true) {
                    warn!("Failed to watch client {} for disconnection", client_port);
                    return false;
                }
                added = true;
                entry.insert(vec![event.clone()]);
            }
        }

        if added {
            self.subscriptions_by_event
                .entry(event.clone())
                .and_modify(|clients| {
                    if !clients.contains(&client_port) {
                        clients.push(client_port);
                    }
                })
                .or_insert_with(|| vec![client_port]);
            info!("Client {} now subscribed to '{}'", client_port, event);
        }
        true
    }

    pub fn unsubscribe_client(&self, client_port: ClientPort, event: String) {
        info!("Client {} unsubscribing from event: {}", client_port, event);
        let mut removed = false;
        let mut removed_client_entry = false;

        if let Some(mut entry) = self.subscriptions_by_client.get_mut(&client_port) {
            let old_len = entry.len();
            entry.retain(|e| e != &event);
            removed = entry.len() != old_len;
            if entry.is_empty() {
                drop(entry);
                self.subscriptions_by_client.remove(&client_port);
                removed_client_entry = true;
            }
        }

        if removed {
            if let Some(mut entry) = self.subscriptions_by_event.get_mut(&event) {
                entry.retain(|c| c != &client_port);
                if entry.is_empty() {
                    drop(entry);
                    self.subscriptions_by_event.remove(&event);
                }
            }
        }

        if removed_client_entry {
            self.set_lease(client_port, PortLease::Subscriptions, false);
        }
    }

    pub fn subscribe_cli(&self, event: String, command: String, args: Vec<String>) {
        info!(
            "CLI subscribing to event '{}' with command: {} {:?}",
            event, command, args
        );

        let subscription = CliSubscription { command, args };

        let mut guard = self.cli_subscriptions.lock();
        let list = guard.entry(event.clone()).or_insert_with(Vec::new);
        let is_duplicate = list
            .iter()
            .any(|s| s.command == subscription.command && s.args == subscription.args);
        if !is_duplicate {
            list.push(subscription);
            info!("CLI now subscribed to '{}'", event);
        } else {
            info!("Duplicate CLI subscription ignored for '{}'", event);
        }
    }

    pub fn unsubscribe_cli(&self, event: String) {
        info!("CLI unsubscribing from event: {}", event);
        let mut guard = self.cli_subscriptions.lock();
        let removed = guard.remove(&event).map(|v| v.len()).unwrap_or(0);
        info!("Removed {} CLI subscriptions for event '{}'", removed, event);
    }

    pub fn list_cli_subscriptions(&self) -> Value {
        let guard = self.cli_subscriptions.lock();
        let mut subscription_list: Vec<Value> = Vec::new();
        for (event, subs) in guard.iter() {
            for s in subs {
                subscription_list.push(serde_json::json!({
                    "event": event,
                    "command": s.command,
                    "args": s.args,
                }));
            }
        }
        serde_json::json!({
            "cli_subscriptions": subscription_list,
            "total_count": subscription_list.len()
        })
    }

    pub fn publish(&self, event: BroadcastEvent) {
        let event_name = event.kind().as_str();
        let mut targets: HashSet<ClientPort> = HashSet::default();
        if let Some(clients) = self.subscriptions_by_event.get(event_name) {
            targets.extend(clients.iter().copied());
        }
        if let Some(clients) = self.subscriptions_by_event.get("*") {
            targets.extend(clients.iter().copied());
        }
        // Snapshot registrations before executing commands; never hold subscription
        // locks across serialization or process creation.
        let mut relevant: Vec<CliSubscription> = Vec::new();
        {
            let guard = self.cli_subscriptions.lock();
            if let Some(list) = guard.get(event_name) {
                relevant.extend(list.iter().cloned());
            }
            if let Some(list) = guard.get("*") {
                relevant.extend(list.iter().cloned());
            }
        }
        if targets.is_empty() && relevant.is_empty() {
            return;
        }
        let event_json = match serde_json::to_string(&event) {
            Ok(s) => s,
            Err(e) => {
                error!("Failed to serialize broadcast event: {}", e);
                return;
            }
        };
        for subscription in relevant {
            crate::ipc::cli_exec::execute_serialized_cli_subscription(
                &event,
                &event_json,
                &subscription,
            );
        }
        if targets.is_empty() {
            return;
        }

        let batch = DispatchBatch {
            event_json,
            targets: targets.into_iter().collect(),
        };

        if let Err(err) = self.event_dispatch_tx.try_send(batch) {
            match err {
                TrySendError::Full(_) => {
                    warn!(
                        "Dropping IPC event batch: dispatch queue full (capacity={})",
                        EVENT_DISPATCH_QUEUE_CAPACITY
                    );
                }
                TrySendError::Disconnected(_) => {
                    error!("Dropping IPC event batch: dispatch worker channel disconnected");
                }
            }
        }
    }

    fn send_event_to_client(client_port: ClientPort, c_message: &CString) {
        let bytes = c_message.as_bytes_with_nul();
        let sent = unsafe {
            mach_try_send_message(
                client_port,
                c_message.as_ptr() as *const c_char,
                bytes.len() as u32,
            )
        };
        if sent {
            debug!("Successfully sent event to client {}", client_port);
        } else {
            warn!("Failed to send event to client {}", client_port);
        }
    }

    /// The client's port died: drop its subscriptions and their lease.
    pub fn remove_client(&self, client_port: ClientPort) {
        if let Some((_, events)) = self.subscriptions_by_client.remove(&client_port) {
            for event in events {
                if let Some(mut entry) = self.subscriptions_by_event.get_mut(&event) {
                    entry.retain(|c| c != &client_port);
                    if entry.is_empty() {
                        drop(entry);
                        self.subscriptions_by_event.remove(&event);
                    }
                }
            }
            self.set_lease(client_port, PortLease::Subscriptions, false);
        }
    }

    fn run_event_dispatch_worker(event_dispatch_rx: crossbeam_channel::Receiver<DispatchBatch>) {
        while let Ok(batch) = event_dispatch_rx.recv() {
            let c_message = match CString::new(batch.event_json) {
                Ok(message) => message,
                Err(e) => {
                    error!("Failed to encode IPC event payload: {}", e);
                    continue;
                }
            };

            // A full or slow receive queue is not a disconnect; only the
            // dead-name notification ends a client's subscriptions.
            for client_port in batch.targets {
                Self::send_event_to_client(client_port, &c_message);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::mach::{mach_allocate_reply_port, test_support};

    const DEAD_NAME: i32 = 72;

    #[test]
    fn subscriptions_and_claims_share_one_dead_name_watch() {
        let notify = unsafe { mach_allocate_reply_port() }.unwrap();
        test_support::set_notify_port(notify);
        let state = ServerState::new();
        let notices = || unsafe { test_support::drain(notify) };
        let subscribed = |claims: bool| {
            let client = unsafe { mach_allocate_reply_port() }.unwrap();
            assert!(state.subscribe_client(client, "a".into()));
            assert!(state.subscribe_client(client, "b".into()));
            assert!(state.set_lease(client, PortLease::Claims, claims));
            assert!(state.set_lease(client, PortLease::Claims, claims));
            client
        };
        // Once every lease is gone, dropping the test's own reference frees the name.
        let freed = |client| unsafe {
            assert!(mach_release_send_right(client));
            test_support::is_unused(client)
        };

        // Dropping every subscription keeps the claims' watch.
        let client = subscribed(true);
        state.unsubscribe_client(client, "a".into());
        state.unsubscribe_client(client, "b".into());
        unsafe { test_support::destroy_receive_right(client) };
        assert_eq!(notices(), [(DEAD_NAME, client)]);
        state.remove_client(client);
        state.set_lease(client, PortLease::Claims, false);
        assert!(freed(client));

        // Death while used both ways: one notice; claims outlive subscriptions.
        let client = subscribed(true);
        unsafe { test_support::destroy_receive_right(client) };
        assert_eq!(notices(), [(DEAD_NAME, client)]);
        state.remove_client(client);
        assert!(state.leases.lock().contains_key(&client));
        assert!(!unsafe { test_support::is_unused(client) });
        state.set_lease(client, PortLease::Claims, false);
        assert!(freed(client));

        // A full receive queue is not a disconnect: the dispatch worker keeps both leases.
        let client = subscribed(true);
        let event = CString::new("{}").unwrap();
        while unsafe { mach_try_send_message(client, event.as_ptr(), 3) } {}
        let (batches, worker) = bounded(1);
        let batch = DispatchBatch {
            event_json: "{}".into(),
            targets: vec![client],
        };
        batches.send(batch).unwrap();
        drop(batches);
        ServerState::run_event_dispatch_worker(worker);
        assert!(state.subscriptions_by_client.contains_key(&client));
        assert_eq!(state.leases.lock()[&client], 3);
        assert!(notices().is_empty());
        state.remove_client(client);
        state.set_lease(client, PortLease::Claims, false);
        unsafe { test_support::destroy_receive_right(client) };
        assert!(freed(client));

        // With no lease left the watch is cancelled.
        let client = subscribed(false);
        state.unsubscribe_client(client, "a".into());
        state.unsubscribe_client(client, "b".into());
        unsafe { test_support::destroy_receive_right(client) };
        // Only the cancelled requests' send-once notices remain.
        assert!(notices().iter().all(|(id, _)| *id != DEAD_NAME));
        assert!(state.leases.lock().is_empty());
        assert!(freed(client));
        test_support::set_notify_port(0);
    }
}
