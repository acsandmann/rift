use std::ffi::CString;
use std::os::raw::c_char;
use std::sync::Arc;

use dashmap::DashMap;
use dispatchr::queue;
use dispatchr::time::Time;
use parking_lot::{Mutex, RwLock};
use serde_json::Value;
use tracing::{debug, error, info, warn};

use crate::actor::broadcast::BroadcastEvent;
use crate::common::collections::{HashMap, HashSet};
use crate::sys::dispatch::DispatchExt;
use crate::sys::mach::mach_send_message;

pub type ClientPort = u32;

#[derive(Clone, Debug)]
pub struct CliSubscription {
    pub command: String,
    pub args: Vec<String>,
}

pub struct ServerState {
    subscriptions_by_client: DashMap<ClientPort, Vec<String>>,
    subscriptions_by_event: DashMap<String, Vec<ClientPort>>,
    cli_subscriptions: Mutex<HashMap<String, Vec<CliSubscription>>>,
}

pub type SharedServerState = Arc<RwLock<ServerState>>;

impl ServerState {
    pub fn new() -> Self {
        Self {
            subscriptions_by_client: DashMap::new(),
            subscriptions_by_event: DashMap::new(),
            cli_subscriptions: Mutex::new(HashMap::default()),
        }
    }

    pub fn subscribe_client(&self, client_port: ClientPort, event: String) {
        info!("Client {} subscribing to event: {}", client_port, event);
        let mut added = false;
        self.subscriptions_by_client
            .entry(client_port)
            .and_modify(|subs| {
                if !subs.contains(&event) {
                    subs.push(event.clone());
                    added = true;
                }
            })
            .or_insert_with(|| {
                added = true;
                vec![event.clone()]
            });

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
    }

    pub fn unsubscribe_client(&self, client_port: ClientPort, event: String) {
        info!("Client {} unsubscribing from event: {}", client_port, event);
        let mut removed = false;

        if let Some(mut entry) = self.subscriptions_by_client.get_mut(&client_port) {
            entry.retain(|e| e != &event);
            removed = true;
            if entry.is_empty() {
                drop(entry);
                self.subscriptions_by_client.remove(&client_port);
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

        for client_port in targets {
            schedule_event_send(client_port, event_json.clone());
        }
    }

    fn forward_event_to_cli_subscribers(&self, event: BroadcastEvent) {
        let event_name = match &event {
            BroadcastEvent::WorkspaceChanged { .. } => "workspace_changed",
            BroadcastEvent::WindowsChanged { .. } => "windows_changed",
            BroadcastEvent::WindowTitleChanged { .. } => "window_title_changed",
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

    fn send_event_to_client(client_port: ClientPort, event_json: &str) {
        let c_message = CString::new(event_json).unwrap_or_default();
        unsafe {
            let result = mach_send_message(
                client_port,
                c_message.as_ptr() as *mut c_char,
                event_json.len() as u32,
                false,
                None,
            );
            if !result {
                warn!("Failed to send event to client {}", client_port);
            } else {
                debug!("Successfully sent event to client {}", client_port);
            }
        }
    }

    pub fn remove_client(&self, client_port: ClientPort) {
        if let Some((_k, events)) = self.subscriptions_by_client.remove(&client_port) {
            for event in events {
                if let Some(mut entry) = self.subscriptions_by_event.get_mut(&event) {
                    entry.retain(|c| c != &client_port);
                    if entry.is_empty() {
                        drop(entry);
                        self.subscriptions_by_event.remove(&event);
                    }
                }
            }
        }
    }
}

fn schedule_event_send(client_port: ClientPort, event_json: String) {
    match queue::global(dispatchr::QoS::Utility) {
        Some(q) => q.after_f_s(
            Time::new_after(Time::NOW, (0.1 * 1000000.0) as i64),
            (client_port, event_json),
            |(client_port, event_json)| ServerState::send_event_to_client(client_port, &event_json),
        ),
        None => ServerState::send_event_to_client(client_port, &event_json),
    }
}
