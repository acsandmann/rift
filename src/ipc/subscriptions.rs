use std::ffi::CString;
use std::os::raw::c_char;
use std::sync::Arc;
use std::thread;

use crossbeam_channel::{Sender, TrySendError, bounded};
use dashmap::DashMap;
use dashmap::mapref::entry::Entry;
use parking_lot::{Mutex, RwLock};
use serde_json::Value;
use tracing::{debug, error, info, warn};

use crate::actor::broadcast::BroadcastEvent;
use crate::common::collections::{HashMap, HashSet};
use crate::sys::mach::{mach_release_send_right, mach_retain_send_right, mach_try_send_message};

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
}

pub type SharedServerState = Arc<RwLock<ServerState>>;

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

        let worker_subscriptions_by_client = Arc::clone(&subscriptions_by_client);
        let worker_subscriptions_by_event = Arc::clone(&subscriptions_by_event);
        thread::spawn(move || {
            Self::run_event_dispatch_worker(
                event_dispatch_rx,
                worker_subscriptions_by_client,
                worker_subscriptions_by_event,
            );
        });

        Self {
            subscriptions_by_client,
            subscriptions_by_event,
            cli_subscriptions,
            event_dispatch_tx,
        }
    }

    pub fn subscribe_client(&self, client_port: ClientPort, event: String) {
        info!("Client {} subscribing to event: {}", client_port, event);
        let mut added = false;
        let mut should_retain_send_right = false;

        match self.subscriptions_by_client.entry(client_port) {
            Entry::Occupied(mut entry) => {
                let subs = entry.get_mut();
                if !subs.contains(&event) {
                    subs.push(event.clone());
                    added = true;
                }
            }
            Entry::Vacant(entry) => {
                added = true;
                should_retain_send_right = true;
                entry.insert(vec![event.clone()]);
            }
        }

        if added {
            if should_retain_send_right {
                let _ = unsafe { mach_retain_send_right(client_port) };
            }
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
            let _ = unsafe { mach_release_send_right(client_port) };
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
        self.forward_event_to_cli_subscribers(event.clone());
        self.forward_event_to_subscribers(event);
    }

    fn forward_event_to_subscribers(&self, event: BroadcastEvent) {
        let event_name = match &event {
            BroadcastEvent::WorkspaceChanged { .. } => "workspace_changed",
            BroadcastEvent::WindowsChanged { .. } => "windows_changed",
            BroadcastEvent::WindowTitleChanged { .. } => "window_title_changed",
            BroadcastEvent::StacksChanged { .. } => "stacks_changed",
        };

        let mut targets: HashSet<ClientPort> = HashSet::default();
        if let Some(clients) = self.subscriptions_by_event.get(event_name) {
            targets.extend(clients.iter().copied());
        }
        if let Some(clients) = self.subscriptions_by_event.get("*") {
            targets.extend(clients.iter().copied());
        }

        if targets.is_empty() {
            return;
        }

        let event_json = match serde_json::to_string(&event) {
            Ok(s) => s,
            Err(e) => {
                error!("Failed to serialize broadcast event: {}", e);
                return;
            }
        };

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

    fn forward_event_to_cli_subscribers(&self, event: BroadcastEvent) {
        let event_name = match &event {
            BroadcastEvent::WorkspaceChanged { .. } => "workspace_changed",
            BroadcastEvent::WindowsChanged { .. } => "windows_changed",
            BroadcastEvent::WindowTitleChanged { .. } => "window_title_changed",
            BroadcastEvent::StacksChanged { .. } => "stacks_changed",
        };

        // Collect relevant subscriptions without full HashMap clone
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

        for subscription in relevant {
            crate::ipc::cli_exec::execute_cli_subscription(&event, &subscription);
        }
    }

    fn send_event_to_client(client_port: ClientPort, c_message: &CString) -> bool {
        let bytes = c_message.as_bytes_with_nul();
        unsafe {
            let result = mach_try_send_message(
                client_port,
                c_message.as_ptr() as *const c_char,
                bytes.len() as u32,
            );
            if !result {
                warn!("Failed to send event to client {}", client_port);
                return false;
            } else {
                debug!("Successfully sent event to client {}", client_port);
                return true;
            }
        }
    }

    pub fn remove_client(&self, client_port: ClientPort) {
        Self::remove_client_from_maps(
            client_port,
            &self.subscriptions_by_client,
            &self.subscriptions_by_event,
        );
    }

    fn run_event_dispatch_worker(
        event_dispatch_rx: crossbeam_channel::Receiver<DispatchBatch>,
        subscriptions_by_client: Arc<DashMap<ClientPort, Vec<String>>>,
        subscriptions_by_event: Arc<DashMap<String, Vec<ClientPort>>>,
    ) {
        while let Ok(batch) = event_dispatch_rx.recv() {
            let c_message = match CString::new(batch.event_json) {
                Ok(message) => message,
                Err(e) => {
                    error!("Failed to encode IPC event payload: {}", e);
                    continue;
                }
            };

            for client_port in batch.targets {
                if !Self::send_event_to_client(client_port, &c_message) {
                    Self::remove_client_from_maps(
                        client_port,
                        &subscriptions_by_client,
                        &subscriptions_by_event,
                    );
                }
            }
        }
    }

    fn remove_client_from_maps(
        client_port: ClientPort,
        subscriptions_by_client: &DashMap<ClientPort, Vec<String>>,
        subscriptions_by_event: &DashMap<String, Vec<ClientPort>>,
    ) {
        if let Some((_k, events)) = subscriptions_by_client.remove(&client_port) {
            for event in events {
                if let Some(mut entry) = subscriptions_by_event.get_mut(&event) {
                    entry.retain(|c| c != &client_port);
                    if entry.is_empty() {
                        drop(entry);
                        subscriptions_by_event.remove(&event);
                    }
                }
            }
            let _ = unsafe { mach_release_send_right(client_port) };
        }
    }
}
