use std::ffi::{CString, c_void};
use std::os::raw::c_char;
use std::sync::Arc;

use objc2_core_foundation::{
    CFAbsoluteTimeGetCurrent, CFRetained, CFRunLoop, CFRunLoopTimer, CFRunLoopTimerContext,
    kCFRunLoopDefaultMode,
};
use parking_lot::{Mutex, RwLock};
use serde_json::Value;
use tracing::{debug, error, info, warn};

use crate::actor::broadcast::BroadcastEvent;
use crate::common::collections::HashMap;
use crate::sys::mach::mach_send_message;

pub type ClientPort = u32;

#[derive(Clone, Debug)]
pub struct CliSubscription {
    pub command: String,
    pub args: Vec<String>,
}

#[derive(Clone)]
struct RunLoopHandle(CFRetained<CFRunLoop>);

unsafe impl Send for RunLoopHandle {}
unsafe impl Sync for RunLoopHandle {}

impl RunLoopHandle {
    fn new(runloop: CFRetained<CFRunLoop>) -> Self { Self(runloop) }

    fn as_ref(&self) -> &CFRunLoop { &self.0 }
}

pub struct ServerState {
    subscriptions: Mutex<HashMap<ClientPort, Vec<String>>>,
    cli_subscriptions: Mutex<HashMap<String, Vec<CliSubscription>>>,
    runloop: Mutex<Option<RunLoopHandle>>,
}

pub type SharedServerState = Arc<RwLock<ServerState>>;

impl ServerState {
    pub fn new() -> Self {
        Self {
            subscriptions: Mutex::new(HashMap::default()),
            cli_subscriptions: Mutex::new(HashMap::default()),
            runloop: Mutex::new(None),
        }
    }

    pub fn set_runloop(&self, rl: Option<CFRetained<CFRunLoop>>) {
        let mut guard = self.runloop.lock();
        *guard = rl.map(RunLoopHandle::new);
    }

    pub fn subscribe_client(&self, client_port: ClientPort, event: String) {
        info!("Client {} subscribing to event: {}", client_port, event);
        let mut guard = self.subscriptions.lock();
        let subs = guard.entry(client_port).or_insert_with(Vec::new);
        if !subs.contains(&event) {
            subs.push(event);
            info!("Client {} now subscribed to: {:?}", client_port, subs);
        }
    }

    pub fn unsubscribe_client(&self, client_port: ClientPort, event: String) {
        info!("Client {} unsubscribing from event: {}", client_port, event);
        let mut guard = self.subscriptions.lock();
        if let Some(events) = guard.get_mut(&client_port) {
            events.retain(|e| e != &event);
            if events.is_empty() {
                guard.remove(&client_port);
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
        };

        let subscriptions_snapshot = {
            let guard = self.subscriptions.lock();
            guard.clone()
        };

        for (client_port, events) in subscriptions_snapshot {
            if events.contains(&event_name.to_string()) || events.contains(&"*".to_string()) {
                let event_json = match serde_json::to_string(&event) {
                    Ok(s) => s,
                    Err(e) => {
                        error!("Failed to serialize broadcast event: {}", e);
                        continue;
                    }
                };

                let maybe_runloop = {
                    let rl_guard = self.runloop.lock();
                    rl_guard.clone()
                };

                if let Some(runloop) = maybe_runloop.as_ref() {
                    schedule_event_send(runloop, client_port, event_json.clone());
                } else {
                    Self::send_event_to_client(client_port, &event_json);
                }
            }
        }
    }

    fn forward_event_to_cli_subscribers(&self, event: BroadcastEvent) {
        let event_name = match &event {
            BroadcastEvent::WorkspaceChanged { .. } => "workspace_changed",
            BroadcastEvent::WindowsChanged { .. } => "windows_changed",
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
            );
            if result.is_null() {
                debug!("Successfully sent event to client {}", client_port);
            } else {
                warn!("Failed to send event to client {}", client_port);
            }
        }
    }

    pub fn remove_client(&self, client_port: ClientPort) {
        let mut guard = self.subscriptions.lock();
        guard.remove(&client_port);
    }
}

struct EventInfo {
    client_port: ClientPort,
    event_json: String,
}

unsafe extern "C-unwind" fn perform_send(_timer: *mut CFRunLoopTimer, info: *mut c_void) {
    if info.is_null() {
        return;
    }

    let info = unsafe { &*(info as *mut EventInfo) };
    ServerState::send_event_to_client(info.client_port, &info.event_json);
}

unsafe extern "C-unwind" fn release_info(info: *const c_void) {
    if info.is_null() {
        return;
    }

    unsafe {
        drop(Box::from_raw(info as *mut EventInfo));
    }
}

fn schedule_event_send(runloop: &RunLoopHandle, client_port: ClientPort, event_json: String) {
    let info = Box::new(EventInfo {
        client_port,
        event_json: event_json.clone(),
    });
    let info_ptr = Box::into_raw(info);

    let mut context = CFRunLoopTimerContext {
        version: 0,
        info: info_ptr.cast(),
        retain: None,
        release: Some(release_info),
        copyDescription: None,
    };

    let timer = unsafe {
        CFRunLoopTimer::new(
            None,
            CFAbsoluteTimeGetCurrent(),
            0.0,
            0,
            0,
            Some(perform_send),
            &mut context,
        )
    };

    let Some(timer) = timer else {
        unsafe {
            drop(Box::from_raw(info_ptr));
        }
        ServerState::send_event_to_client(client_port, &event_json);
        return;
    };

    let Some(mode) = (unsafe { kCFRunLoopDefaultMode }) else {
        drop(timer);
        ServerState::send_event_to_client(client_port, &event_json);
        return;
    };

    runloop.as_ref().add_timer(Some(&timer), Some(mode));
}
