use std::convert::Infallible;
use std::ffi::{CString, c_void};
use std::os::raw::c_char;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, RawWaker, RawWakerVTable, Waker};
use std::thread;
use std::time::{Duration, Instant};

use r#continue::continuation;
use core_foundation::date::CFAbsoluteTimeGetCurrent;
use core_foundation::runloop::{
    CFRunLoop, CFRunLoopTimer, CFRunLoopTimerContext, CFRunLoopTimerRef, kCFRunLoopDefaultMode,
};
use dispatchr::semaphore::Managed;
use dispatchr::time::Time;
use nix::unistd::{ForkResult, execvp, fork};
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, warn};

use crate::actor::broadcast::BroadcastEvent;
use crate::actor::reactor::{self, Event};
use crate::common::collections::HashMap;
use crate::model::server::WorkspaceQueryResponse;
use crate::sys::mach::{
    mach_msg_header_t, mach_send_message, mach_send_request, mach_server_run, send_mach_reply,
};

#[non_exhaustive]
#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
pub enum RiftRequest {
    GetWorkspaces,
    GetWindows {
        space_id: Option<u64>,
    },
    GetWindowInfo {
        window_id: String,
    },
    GetLayoutState {
        space_id: u64,
    },
    GetApplications,
    GetMetrics,
    GetConfig,
    ExecuteCommand {
        command: String,
        args: Vec<String>,
    },
    Subscribe {
        event: String,
    },
    Unsubscribe {
        event: String,
    },
    SubscribeCli {
        event: String,
        command: String,
        args: Vec<String>,
    },
    UnsubscribeCli {
        event: String,
    },
    ListCliSubscriptions,
}

#[non_exhaustive]
#[derive(Serialize, Deserialize, Debug)]
#[serde(untagged)]
pub enum RiftResponse {
    Success { data: serde_json::Value },
    Error { message: String },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum RiftCommand {
    Reactor(crate::actor::reactor::Command),
}

type ClientPort = u32;

#[derive(Clone, Debug)]
struct CliSubscription {
    command: String,
    args: Vec<String>,
}

static SUBSCRIPTIONS: Mutex<Option<HashMap<ClientPort, Vec<String>>>> = Mutex::new(None);
static CLI_SUBSCRIPTIONS: Mutex<Option<HashMap<String, Vec<CliSubscription>>>> = Mutex::new(None);
static mut MACH_RUNLOOP: OnceLock<Mutex<CFRunLoop>> = OnceLock::new();

pub fn run_mach_server(reactor_tx: reactor::Sender) {
    info!("Starting Mach server with CFRunLoop event handling");

    #[allow(static_mut_refs)]
    let _ = unsafe { MACH_RUNLOOP.set(Mutex::new(CFRunLoop::get_current())) };

    let handler = MachHandler::new(reactor_tx);
    unsafe {
        mach_server_run(Box::into_raw(Box::new(handler)) as *mut _, handle_mach_request_c);
    }
}

pub fn forward_broadcast_event(event: BroadcastEvent) {
    MachHandler::forward_event_to_cli_subscribers(event.clone());
    MachHandler::forward_event_to_subscribers(event);
}

pub struct RiftMachClient {
    connected: bool,
}

impl RiftMachClient {
    pub fn connect() -> Result<Self, String> { Ok(RiftMachClient { connected: true }) }

    pub fn send_request(&self, request: &RiftRequest) -> Result<RiftResponse, String> {
        if !self.connected {
            return Err("Not connected".to_string());
        }

        let request_json = serde_json::to_string(request)
            .map_err(|e| format!("Failed to serialize request: {}", e))?;

        let response_ptr = unsafe {
            let c_request = std::ffi::CString::new(request_json.clone())
                .map_err(|e| format!("Failed to create C string: {}", e))?;

            mach_send_request(
                c_request.as_ptr() as *mut std::os::raw::c_char,
                request_json.len() as u32,
            )
        };

        if response_ptr.is_null() {
            return Err("Failed to send Mach request or no response received".to_string());
        }

        let response_str = unsafe {
            let c_str = std::ffi::CStr::from_ptr(response_ptr);
            c_str.to_string_lossy().to_string()
        };

        let response: RiftResponse = serde_json::from_str(&response_str)
            .map_err(|e| format!("Failed to parse response JSON: {}", e))?;

        Ok(response)
    }
}

struct MachHandler {
    reactor_tx: reactor::Sender,
}

impl MachHandler {
    fn new(reactor_tx: reactor::Sender) -> Self {
        let mut guard = SUBSCRIPTIONS.lock().unwrap();
        if guard.is_none() {
            *guard = Some(HashMap::default());
        }
        drop(guard);

        let mut cli_guard = CLI_SUBSCRIPTIONS.lock().unwrap();
        if cli_guard.is_none() {
            *cli_guard = Some(HashMap::default());
        }
        drop(cli_guard);

        Self { reactor_tx }
    }

    fn forward_event_to_subscribers(event: BroadcastEvent) {
        let event_name = match &event {
            BroadcastEvent::WorkspaceChanged { .. } => "workspace_changed",
            BroadcastEvent::WindowsChanged { .. } => "windows_changed",
        };

        if let Ok(subscriptions_guard) = SUBSCRIPTIONS.lock() {
            if let Some(ref subscriptions) = *subscriptions_guard {
                let subscriptions_clone = subscriptions.clone();
                drop(subscriptions_guard);

                for (client_port, events) in subscriptions_clone {
                    if events.contains(&event_name.to_string()) || events.contains(&"*".to_string())
                    {
                        let event_json = serde_json::to_string(&event).unwrap();

                        #[allow(static_mut_refs)]
                        unsafe {
                            if let Some(runloop_mutex) = MACH_RUNLOOP.get() {
                                match runloop_mutex.lock() {
                                    Ok(runloop) => {
                                        schedule_event_send(&*runloop, client_port, event_json);
                                    }
                                    Err(_) => {
                                        Self::send_event_to_client(client_port, &event_json);
                                    }
                                }
                            } else {
                                Self::send_event_to_client(client_port, &event_json);
                            }
                        }
                    }
                }
            }
        }
    }

    fn forward_event_to_cli_subscribers(event: BroadcastEvent) {
        let event_name = match &event {
            BroadcastEvent::WorkspaceChanged { .. } => "workspace_changed",
            BroadcastEvent::WindowsChanged { .. } => "windows_changed",
        };

        if let Ok(subscriptions_guard) = CLI_SUBSCRIPTIONS.lock() {
            if let Some(ref subscriptions) = *subscriptions_guard {
                let subscriptions_clone = subscriptions.clone();
                drop(subscriptions_guard);

                let mut relevant_subscriptions = Vec::new();
                if let Some(event_subs) = subscriptions_clone.get(event_name) {
                    relevant_subscriptions.extend(event_subs.clone());
                }
                if let Some(wildcard_subs) = subscriptions_clone.get("*") {
                    relevant_subscriptions.extend(wildcard_subs.clone());
                }

                for (_, subscription) in relevant_subscriptions.iter().enumerate() {
                    Self::execute_cli_subscription(&event, &subscription);
                }
            }
        }
    }

    fn execute_cli_subscription(event: &BroadcastEvent, subscription: &CliSubscription) {
        let mut env_vars = HashMap::default();
        match event {
            BroadcastEvent::WorkspaceChanged {
                workspace_id,
                workspace_name,
                space_id,
            } => {
                env_vars.insert("RIFT_EVENT_TYPE".to_string(), "workspace_changed".to_string());
                env_vars.insert("RIFT_WORKSPACE_ID".to_string(), format!("{:?}", workspace_id));
                env_vars.insert("RIFT_WORKSPACE_NAME".to_string(), workspace_name.clone());
                env_vars.insert("RIFT_SPACE_ID".to_string(), space_id.to_string());
            }
            BroadcastEvent::WindowsChanged {
                workspace_id,
                workspace_name,
                windows,
            } => {
                env_vars.insert("RIFT_EVENT_TYPE".to_string(), "windows_changed".to_string());
                env_vars.insert("RIFT_WORKSPACE_ID".to_string(), format!("{:?}", workspace_id));
                env_vars.insert("RIFT_WORKSPACE_NAME".to_string(), workspace_name.clone());
                env_vars.insert("RIFT_WINDOW_COUNT".to_string(), windows.len().to_string());
                env_vars.insert("RIFT_WINDOWS".to_string(), windows.join(","));
            }
        }
        let event_json = serde_json::to_string(event).unwrap();
        env_vars.insert("RIFT_EVENT_JSON".to_string(), event_json.clone());
        let command = subscription.command.clone();
        let mut args = subscription.args.clone();
        args.push(event_json.clone());
        let mut shell_cmd = String::new();
        shell_cmd.push_str(&shell_escape::escape(command.into()));
        for arg in &args {
            shell_cmd.push(' ');
            shell_cmd.push_str(&shell_escape::escape(arg.into()));
        }

        match unsafe { fork() } {
            Ok(ForkResult::Parent { .. }) => {
                debug!("Parent process forked child, returning immediately.");
                return;
            }
            Ok(ForkResult::Child) => {
                debug!("Child process executing command: {}", shell_cmd);

                for (key, value) in env_vars {
                    unsafe { std::env::set_var(key, value) };
                }

                let c_shell = CString::new("sh").unwrap();
                let c_flag = CString::new("-c").unwrap();
                let c_cmd = CString::new(shell_cmd).unwrap();
                let exec_args: &[&CString] = &[&c_shell, &c_flag, &c_cmd];

                let result: Result<Infallible, _> = execvp(&c_shell, exec_args);

                error!("execvp failed: {}", result.unwrap_err());

                std::process::exit(1);
            }
            Err(e) => {
                error!("Failed to fork process: {}", e);
            }
        }
    }

    fn send_event_to_client(client_port: ClientPort, event_json: &str) {
        let c_message = CString::new(event_json).unwrap();
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
                if let Ok(mut subscriptions_guard) = SUBSCRIPTIONS.lock() {
                    if let Some(ref mut subscriptions) = *subscriptions_guard {
                        subscriptions.remove(&client_port);
                    }
                }
            }
        }
    }

    fn handle_request(&self, request: RiftRequest, client_port: ClientPort) -> RiftResponse {
        debug!("Handling request: {:?} from client {}", request, client_port);

        match request {
            RiftRequest::Subscribe { event } => self.handle_subscribe(client_port, event),
            RiftRequest::Unsubscribe { event } => self.handle_unsubscribe(client_port, event),
            RiftRequest::SubscribeCli { event, command, args } => {
                self.handle_cli_subscribe(event, command, args)
            }
            RiftRequest::UnsubscribeCli { event } => self.handle_cli_unsubscribe(event),
            RiftRequest::ListCliSubscriptions => self.handle_list_cli_subscriptions(),
            RiftRequest::GetWorkspaces => {
                let (cont_tx, cont_fut) = continuation::<WorkspaceQueryResponse>();
                let event = Event::QueryWorkspaces(cont_tx);

                if let Err(e) = self.reactor_tx.try_send(event) {
                    error!("Failed to send workspace query: {}", e);
                    return RiftResponse::Error {
                        message: "Failed to query workspaces".to_string(),
                    };
                }

                match block_on_continuation(cont_fut, Duration::from_secs(5)) {
                    Ok(WorkspaceQueryResponse { workspaces }) => RiftResponse::Success {
                        data: serde_json::to_value(workspaces).unwrap(),
                    },
                    Err(e) => {
                        error!("Failed to get workspace response: {}", e);
                        RiftResponse::Error {
                            message: "Failed to get workspace response".to_string(),
                        }
                    }
                }
            }
            RiftRequest::GetWindows { space_id: _ } => {
                let space_id = None;

                let (cont_tx, cont_fut) = continuation::<Vec<crate::model::server::WindowData>>();
                let event = Event::QueryWindows { space_id, response: cont_tx };

                if let Err(e) = self.reactor_tx.try_send(event) {
                    error!("Failed to send windows query: {}", e);
                    return RiftResponse::Error {
                        message: "Failed to query windows".to_string(),
                    };
                }

                match block_on_continuation(cont_fut, Duration::from_secs(5)) {
                    Ok(windows) => RiftResponse::Success {
                        data: serde_json::to_value(windows).unwrap(),
                    },
                    Err(e) => {
                        error!("Failed to get windows response: {}", e);
                        RiftResponse::Error {
                            message: "Failed to get windows response".to_string(),
                        }
                    }
                }
            }
            RiftRequest::GetWindowInfo { window_id } => {
                // Accept either debug string or raw WindowId string; parse via helper
                let window_id = match crate::actor::app::WindowId::from_debug_string(&window_id) {
                    Some(wid) => wid,
                    None => {
                        error!("Invalid window_id format: {}", window_id);
                        return RiftResponse::Error {
                            message: "Invalid window_id format".to_string(),
                        };
                    }
                };

                let (cont_tx, cont_fut) =
                    continuation::<Option<crate::model::server::WindowData>>();
                let event = Event::QueryWindowInfo { window_id, response: cont_tx };

                if let Err(e) = self.reactor_tx.try_send(event) {
                    error!("Failed to send window info query: {}", e);
                    return RiftResponse::Error {
                        message: "Failed to query window info".to_string(),
                    };
                }

                match block_on_continuation(cont_fut, Duration::from_secs(5)) {
                    Ok(Some(window)) => RiftResponse::Success {
                        data: serde_json::to_value(window).unwrap(),
                    },
                    Ok(None) => RiftResponse::Error {
                        message: "Window not found".to_string(),
                    },
                    Err(e) => {
                        error!("Failed to get window info response: {}", e);
                        RiftResponse::Error {
                            message: "Failed to get window info response".to_string(),
                        }
                    }
                }
            }
            RiftRequest::GetLayoutState { space_id } => {
                let (cont_tx, cont_fut) =
                    continuation::<Option<crate::model::server::LayoutStateData>>();
                let event = Event::QueryLayoutState { space_id, response: cont_tx };

                if let Err(e) = self.reactor_tx.try_send(event) {
                    error!("Failed to send layout state query: {}", e);
                    return RiftResponse::Error {
                        message: "Failed to query layout state".to_string(),
                    };
                }

                match block_on_continuation(cont_fut, Duration::from_secs(5)) {
                    Ok(Some(layout_state)) => RiftResponse::Success {
                        data: serde_json::to_value(layout_state).unwrap(),
                    },
                    Ok(None) => RiftResponse::Error {
                        message: "Space not found or inactive".to_string(),
                    },
                    Err(e) => {
                        error!("Failed to get layout state response: {}", e);
                        RiftResponse::Error {
                            message: "Failed to get layout state response".to_string(),
                        }
                    }
                }
            }
            RiftRequest::GetApplications => {
                let (cont_tx, cont_fut) =
                    continuation::<Vec<crate::model::server::ApplicationData>>();
                let event = Event::QueryApplications(cont_tx);

                if let Err(e) = self.reactor_tx.try_send(event) {
                    error!("Failed to send applications query: {}", e);
                    return RiftResponse::Error {
                        message: "Failed to query applications".to_string(),
                    };
                }

                match block_on_continuation(cont_fut, Duration::from_secs(5)) {
                    Ok(applications) => RiftResponse::Success {
                        data: serde_json::to_value(applications).unwrap(),
                    },
                    Err(e) => {
                        error!("Failed to get applications response: {}", e);
                        RiftResponse::Error {
                            message: "Failed to get applications response".to_string(),
                        }
                    }
                }
            }
            RiftRequest::GetMetrics => {
                let (cont_tx, cont_fut) = continuation::<serde_json::Value>();
                let event = Event::QueryMetrics(cont_tx);

                if let Err(e) = self.reactor_tx.try_send(event) {
                    error!("Failed to send metrics query: {}", e);
                    return RiftResponse::Error {
                        message: "Failed to query metrics".to_string(),
                    };
                }

                match block_on_continuation(cont_fut, Duration::from_secs(5)) {
                    Ok(metrics) => RiftResponse::Success { data: metrics },
                    Err(e) => {
                        error!("Failed to get metrics response: {}", e);
                        RiftResponse::Error {
                            message: "Failed to get metrics response".to_string(),
                        }
                    }
                }
            }
            RiftRequest::GetConfig => {
                let (cont_tx, cont_fut) = continuation::<serde_json::Value>();
                let event = Event::QueryConfig(cont_tx);

                if let Err(e) = self.reactor_tx.try_send(event) {
                    error!("Failed to send config query: {}", e);
                    return RiftResponse::Error {
                        message: "Failed to query config".to_string(),
                    };
                }

                match block_on_continuation(cont_fut, Duration::from_secs(5)) {
                    Ok(config) => RiftResponse::Success { data: config },
                    Err(e) => {
                        error!("Failed to get config response: {}", e);
                        RiftResponse::Error {
                            message: "Failed to get config response".to_string(),
                        }
                    }
                }
            }
            RiftRequest::ExecuteCommand { command, args: _ } => {
                match serde_json::from_str::<RiftCommand>(&command) {
                    Ok(RiftCommand::Reactor(reactor_command)) => {
                        let event = Event::Command(reactor_command);

                        if let Err(e) = self.reactor_tx.try_send(event) {
                            error!("Failed to send command to reactor: {}", e);
                            return RiftResponse::Error {
                                message: "Failed to execute command".to_string(),
                            };
                        }

                        RiftResponse::Success {
                            data: serde_json::json!("Command executed successfully"),
                        }
                    }
                    Err(e) => {
                        error!("Failed to parse command: {}", e);
                        RiftResponse::Error {
                            message: format!("Invalid command format: {}", e),
                        }
                    }
                }
            }
        }
    }

    fn handle_subscribe(&self, client_port: ClientPort, event: String) -> RiftResponse {
        info!("Client {} subscribing to event: {}", client_port, event);

        if let Ok(mut subscriptions_guard) = SUBSCRIPTIONS.lock() {
            if let Some(ref mut subscriptions) = *subscriptions_guard {
                let client_events = subscriptions.entry(client_port).or_insert_with(Vec::new);

                if !client_events.contains(&event) {
                    client_events.push(event.clone());
                    info!("Client {} now subscribed to: {:?}", client_port, client_events);
                }
            }
        }

        RiftResponse::Success {
            data: serde_json::json!({ "subscribed": event }),
        }
    }

    fn handle_unsubscribe(&self, client_port: ClientPort, event: String) -> RiftResponse {
        info!("Client {} unsubscribing from event: {}", client_port, event);

        if let Ok(mut subscriptions_guard) = SUBSCRIPTIONS.lock() {
            if let Some(ref mut subscriptions) = *subscriptions_guard {
                if let Some(client_events) = subscriptions.get_mut(&client_port) {
                    client_events.retain(|e| e != &event);
                    if client_events.is_empty() {
                        subscriptions.remove(&client_port);
                    }
                }
            }
        }

        RiftResponse::Success {
            data: serde_json::json!({ "unsubscribed": event }),
        }
    }

    fn handle_cli_subscribe(
        &self,
        event: String,
        command: String,
        args: Vec<String>,
    ) -> RiftResponse {
        info!(
            "CLI subscribing to event '{}' with command: {} {:?}",
            event, command, args
        );

        if let Ok(mut cli_subscriptions_guard) = CLI_SUBSCRIPTIONS.lock() {
            if let Some(ref mut cli_subscriptions) = *cli_subscriptions_guard {
                let event_subscriptions =
                    cli_subscriptions.entry(event.clone()).or_insert_with(Vec::new);

                let subscription = CliSubscription {
                    command: command.clone(),
                    args: args.clone(),
                };

                let is_duplicate = event_subscriptions
                    .iter()
                    .any(|sub| sub.command == command && sub.args == args);

                if !is_duplicate {
                    event_subscriptions.push(subscription);
                    info!(
                        "CLI now subscribed to '{}' with command: {} {:?}",
                        event, command, args
                    );
                } else {
                    info!(
                        "CLI subscription already exists for '{}' with command: {} {:?}",
                        event, command, args
                    );
                }
            }
        }

        RiftResponse::Success {
            data: serde_json::json!({
                "cli_subscribed": event,
                "command": command,
                "args": args
            }),
        }
    }

    fn handle_cli_unsubscribe(&self, event: String) -> RiftResponse {
        info!("CLI unsubscribing from event: {}", event);

        if let Ok(mut cli_subscriptions_guard) = CLI_SUBSCRIPTIONS.lock() {
            if let Some(ref mut cli_subscriptions) = *cli_subscriptions_guard {
                let removed_count = cli_subscriptions.remove(&event).map(|v| v.len()).unwrap_or(0);
                info!(
                    "Removed {} CLI subscriptions for event '{}'",
                    removed_count, event
                );
            }
        }

        RiftResponse::Success {
            data: serde_json::json!({ "cli_unsubscribed": event }),
        }
    }

    fn handle_list_cli_subscriptions(&self) -> RiftResponse {
        info!("Listing CLI subscriptions");

        if let Ok(cli_subscriptions_guard) = CLI_SUBSCRIPTIONS.lock() {
            if let Some(ref cli_subscriptions) = *cli_subscriptions_guard {
                let mut subscription_list = Vec::new();

                for (event, subscriptions) in cli_subscriptions {
                    for subscription in subscriptions {
                        subscription_list.push(serde_json::json!({
                            "event": event,
                            "command": subscription.command,
                            "args": subscription.args
                        }));
                    }
                }

                RiftResponse::Success {
                    data: serde_json::json!({
                        "cli_subscriptions": subscription_list,
                        "total_count": subscription_list.len()
                    }),
                }
            } else {
                RiftResponse::Success {
                    data: serde_json::json!({
                        "cli_subscriptions": [],
                        "total_count": 0
                    }),
                }
            }
        } else {
            RiftResponse::Error {
                message: "Failed to access CLI subscriptions".to_string(),
            }
        }
    }
}

struct EventInfo {
    client_port: ClientPort,
    event_json: String,
}

extern "C" fn perform_send(_timer: CFRunLoopTimerRef, info: *mut c_void) {
    let info = unsafe { Box::from_raw(info as *mut EventInfo) };
    MachHandler::send_event_to_client(info.client_port, &info.event_json);
}

extern "C" fn release_info(info: *const c_void) {
    unsafe {
        drop(Box::from_raw(info as *mut EventInfo));
    }
}

unsafe fn schedule_event_send(runloop: &CFRunLoop, client_port: ClientPort, event_json: String) {
    let info = Box::new(EventInfo { client_port, event_json });

    let mut context = CFRunLoopTimerContext {
        version: 0,
        info: Box::into_raw(info) as *mut _,
        retain: None,
        release: Some(release_info),
        copyDescription: None,
    };

    let timer = CFRunLoopTimer::new(
        unsafe { CFAbsoluteTimeGetCurrent() },
        0.0,
        0,
        0,
        perform_send,
        &mut context,
    );
    runloop.add_timer(&timer, unsafe { kCFRunLoopDefaultMode });
}

fn block_on_continuation<T: Send + 'static>(
    mut fut: r#continue::Future<T>,
    timeout: Duration,
) -> Result<T, String> {
    // Create a RawWaker that unparks this thread when woken.
    struct ThreadWaker {
        thread: thread::Thread,
        signaled: Arc<AtomicBool>,
    }

    unsafe fn clone_raw(data: *const ()) -> RawWaker {
        let tw = &*(data as *const ThreadWaker);
        let cloned = Box::into_raw(Box::new(ThreadWaker {
            thread: tw.thread.clone(),
            signaled: Arc::new(AtomicBool::new(tw.signaled.load(Ordering::SeqCst))),
        }));
        RawWaker::new(cloned as *const (), &VTABLE)
    }
    unsafe fn wake_raw(data: *const ()) {
        let tw = Box::from_raw(data as *mut ThreadWaker);
        tw.signaled.store(true, Ordering::SeqCst);
        tw.thread.unpark();
        // drop tw
    }
    unsafe fn wake_by_ref_raw(data: *const ()) {
        let tw = &*(data as *const ThreadWaker);
        tw.signaled.store(true, Ordering::SeqCst);
        tw.thread.unpark();
    }
    unsafe fn drop_raw(data: *const ()) { let _ = Box::from_raw(data as *mut ThreadWaker); }

    static VTABLE: RawWakerVTable =
        RawWakerVTable::new(clone_raw, wake_raw, wake_by_ref_raw, drop_raw);

    let signaled = Arc::new(AtomicBool::new(false));
    let tw = Box::new(ThreadWaker {
        thread: thread::current(),
        signaled: signaled.clone(),
    });

    let raw = RawWaker::new(Box::into_raw(tw) as *const (), &VTABLE);
    let waker = unsafe { Waker::from_raw(raw) };
    let mut cx = Context::from_waker(&waker);

    let start = Instant::now();
    let deadline = start + timeout;

    loop {
        match Pin::new(&mut fut).poll(&mut cx) {
            std::task::Poll::Ready(val) => return Ok(val),
            std::task::Poll::Pending => {
                let now = Instant::now();
                if now >= deadline {
                    return Err("Timeout".into());
                }
                // Park the thread until woken or timeout for remaining duration
                let remaining = deadline - now;
                thread::park_timeout(remaining);
                // loop and poll again; if woken, poll should progress
            }
        }
    }
}

unsafe extern "C" fn handle_mach_request_c(
    context: *mut std::ffi::c_void,
    message: *mut c_char,
    len: u32,
    original_msg: *mut mach_msg_header_t,
) {
    if context.is_null() || message.is_null() {
        error!("Invalid context or message pointer");
        return;
    }

    let handler = unsafe { &*(context as *const MachHandler) };
    let message_slice = unsafe { std::slice::from_raw_parts(message as *const u8, len as usize) };
    let message_str = match std::str::from_utf8(message_slice) {
        Ok(s) => s,
        Err(e) => {
            error!("Invalid UTF-8 in message: {}", e);
            return;
        }
    };

    debug!("Received message: {}", message_str);

    let client_port = unsafe { (*original_msg).msgh_remote_port };

    let request: RiftRequest = match serde_json::from_str(message_str) {
        Ok(req) => req,
        Err(e) => {
            error!("Failed to parse request: {}", e);
            let error_response = RiftResponse::Error {
                message: format!("Invalid request format: {}", e),
            };
            send_response(original_msg, &error_response);
            return;
        }
    };

    let response = handler.handle_request(request, client_port);
    send_response(original_msg, &response);
}

fn send_response(original_msg: *mut mach_msg_header_t, response: &RiftResponse) {
    let response_json = serde_json::to_string(response).unwrap();
    let c_response = CString::new(response_json).unwrap();

    unsafe {
        send_mach_reply(
            original_msg,
            c_response.as_ptr() as *mut c_char,
            c_response.as_bytes().len() as u32,
        );
    }
}
