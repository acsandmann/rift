use std::ffi::{CStr, c_char};
use std::time::Duration;

use r#continue::continuation;
use tracing::{error, info, trace};

pub mod cli_exec;
pub mod protocol;
pub mod subscriptions;

pub use protocol::{RiftCommand, RiftRequest, RiftResponse};

use crate::actor::config as config_actor;
use crate::actor::reactor::{self, Event};
use crate::ipc::subscriptions::SharedServerState;
use crate::sys::dispatch::block_on;
use crate::sys::mach::{
    is_mach_server_registered, mach_msg_header_t, mach_send_request, mach_server_run,
    send_mach_reply,
};

type ClientPort = u32;

pub fn run_mach_server(
    reactor_tx: reactor::Sender,
    config_tx: config_actor::Sender,
) -> Result<SharedServerState, String> {
    if is_mach_server_registered() {
        return Err(
            "Another Rift instance is already running; quit it before starting another.".into(),
        );
    }
    info!("Spawning background Mach server thread and returning SharedServerState");

    let shared_state: SharedServerState = std::sync::Arc::new(parking_lot::RwLock::new(
        crate::ipc::subscriptions::ServerState::new(),
    ));

    let thread_state = shared_state.clone();
    std::thread::spawn(move || {
        let handler = MachHandler::new(reactor_tx, config_tx, thread_state.clone());
        unsafe {
            mach_server_run(Box::into_raw(Box::new(handler)) as *mut _, handle_mach_request_c);
        }
    });

    Ok(shared_state)
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

        let request_json = serde_json::to_vec(request)
            .map_err(|e| format!("Failed to serialize request: {}", e))?;

        let mut response_buf = Vec::with_capacity(256);
        let ok = unsafe {
            mach_send_request(
                request_json.as_ptr() as *const i8,
                request_json.len() as u32,
                &mut response_buf,
            )
        };

        if !ok || response_buf.is_empty() {
            return Err("Failed to send Mach request or no response received".to_string());
        }

        let json_bytes = CStr::from_bytes_until_nul(&response_buf)
            .map_err(|_| {
                "response missing NUL
          terminator"
            })?
            .to_bytes();

        let response: RiftResponse = serde_json::from_slice(json_bytes).map_err(|e| {
            format!(
                "Failed to parse
          response JSON: {}",
                e
            )
        })?;

        Ok(response)
    }
}

struct MachHandler {
    reactor_tx: reactor::Sender,
    config_tx: config_actor::Sender,
    server_state: SharedServerState,
}

impl MachHandler {
    fn new(
        reactor_tx: reactor::Sender,
        config_tx: config_actor::Sender,
        server_state: SharedServerState,
    ) -> Self {
        Self {
            reactor_tx,
            config_tx,
            server_state,
        }
    }

    fn forget_reactor_query_sender(event: Event) {
        match event {
            Event::QueryWorkspaces { response, .. } => std::mem::forget(response),
            Event::QueryWindows { response, .. } => std::mem::forget(response),
            Event::QueryActiveWorkspace { response, .. } => std::mem::forget(response),
            Event::QueryDisplays(response) => std::mem::forget(response),
            Event::QueryWindowInfo { response, .. } => std::mem::forget(response),
            Event::QueryApplications(response) => std::mem::forget(response),
            Event::QueryLayoutState { response, .. } => std::mem::forget(response),
            Event::QueryMetrics(response) => std::mem::forget(response),
            _ => {}
        }
    }

    fn forget_config_query_sender(event: config_actor::Event) {
        match event {
            config_actor::Event::QueryConfig(response) => std::mem::forget(response),
            config_actor::Event::ApplyConfig { response, .. } => std::mem::forget(response),
        }
    }

    fn perform_query<T>(
        &self,
        make_event: impl FnOnce(r#continue::Sender<T>) -> Event,
    ) -> Result<T, String>
    where
        T: Send + 'static,
    {
        let (cont_tx, cont_fut) = continuation::<T>();
        let event = make_event(cont_tx);

        if let Err(e) = self.reactor_tx.try_send(event) {
            let msg = format!("{e}");
            let tokio::sync::mpsc::error::SendError((_span, event)) = e;
            // `continue::Sender` panics on drop if never used.
            Self::forget_reactor_query_sender(event);
            return Err(format!("Failed to send query: {msg}"));
        }

        match block_on(cont_fut, Duration::from_secs(5)) {
            Ok(res) => Ok(res),
            Err(e) => Err(format!("Failed to get response: {}", e)),
        }
    }

    fn perform_config_query<T>(
        &self,
        make_event: impl FnOnce(r#continue::Sender<T>) -> config_actor::Event,
    ) -> Result<T, String>
    where
        T: Send + 'static,
    {
        let (cont_tx, cont_fut) = continuation::<T>();
        let event = make_event(cont_tx);

        if let Err(e) = self.config_tx.try_send(event) {
            let msg = format!("{e}");
            let tokio::sync::mpsc::error::SendError((_span, event)) = e;
            Self::forget_config_query_sender(event);
            return Err(format!("Failed to send config query: {msg}"));
        }

        match block_on(cont_fut, Duration::from_secs(5)) {
            Ok(res) => Ok(res),
            Err(e) => Err(format!("Failed to get response: {}", e)),
        }
    }

    fn handle_request(&self, request: RiftRequest, client_port: ClientPort) -> RiftResponse {
        trace!("Handling request: {:?} from client {}", request, client_port);

        match request {
            RiftRequest::Subscribe { event } => {
                let state = self.server_state.read();
                state.subscribe_client(client_port, event.clone());
                RiftResponse::Success {
                    data: serde_json::json!({ "subscribed": event }),
                }
            }
            RiftRequest::Unsubscribe { event } => {
                let state = self.server_state.read();
                state.unsubscribe_client(client_port, event.clone());
                RiftResponse::Success {
                    data: serde_json::json!({ "unsubscribed": event }),
                }
            }
            RiftRequest::SubscribeCli { event, command, args } => {
                let state = self.server_state.read();
                state.subscribe_cli(event.clone(), command.clone(), args.clone());
                RiftResponse::Success {
                    data: serde_json::json!({
                        "cli_subscribed": event,
                        "command": command,
                        "args": args
                    }),
                }
            }
            RiftRequest::UnsubscribeCli { event } => {
                let state = self.server_state.read();
                state.unsubscribe_cli(event.clone());
                RiftResponse::Success {
                    data: serde_json::json!({ "cli_unsubscribed": event }),
                }
            }
            RiftRequest::ListCliSubscriptions => {
                let state = self.server_state.read();
                let data = state.list_cli_subscriptions();
                RiftResponse::Success { data }
            }

            RiftRequest::GetWorkspaces { space_id } => {
                match self.perform_query(|tx| Event::QueryWorkspaces {
                    space_id: space_id.map(crate::sys::screen::SpaceId::new),
                    response: tx,
                }) {
                    Ok(workspaces) => RiftResponse::Success {
                        data: serde_json::to_value(workspaces).unwrap(),
                    },
                    Err(e) => {
                        error!("{}", e);
                        RiftResponse::Error {
                            error: serde_json::json!({ "message": "Failed to get workspace response", "details": format!("{}", e) }),
                        }
                    }
                }
            }

            RiftRequest::GetDisplays => match self.perform_query(|tx| Event::QueryDisplays(tx)) {
                Ok(displays) => RiftResponse::Success {
                    data: serde_json::to_value(displays).unwrap(),
                },
                Err(e) => {
                    error!("{}", e);
                    RiftResponse::Error {
                        error: serde_json::json!({ "message": "Failed to get displays response", "details": format!("{}", e) }),
                    }
                }
            },

            RiftRequest::GetWindows { space_id } => {
                let space_id = space_id.map(|id| crate::sys::screen::SpaceId::new(id));

                match self.perform_query(|tx| Event::QueryWindows { space_id, response: tx }) {
                    Ok(windows) => RiftResponse::Success {
                        data: serde_json::to_value(windows).unwrap(),
                    },
                    Err(e) => {
                        error!("{}", e);
                        RiftResponse::Error {
                            error: serde_json::json!({ "message": "Failed to get windows response", "details": format!("{}", e) }),
                        }
                    }
                }
            }

            RiftRequest::GetWindowInfo { window_id } => {
                let window_id = match crate::actor::app::WindowId::from_debug_string(&window_id) {
                    Some(wid) => wid,
                    None => {
                        error!("Invalid window_id format: {}", window_id);
                        return RiftResponse::Error {
                            error: serde_json::json!({ "message": "Invalid window_id format", "window_id": window_id }),
                        };
                    }
                };

                match self.perform_query(|tx| Event::QueryWindowInfo { window_id, response: tx }) {
                    Ok(Some(window)) => RiftResponse::Success {
                        data: serde_json::to_value(window).unwrap(),
                    },
                    Ok(None) => RiftResponse::Error {
                        error: serde_json::json!({ "message": "Window not found" }),
                    },
                    Err(e) => {
                        error!("{}", e);
                        RiftResponse::Error {
                            error: serde_json::json!({ "message": "Failed to get window info response", "details": format!("{}", e) }),
                        }
                    }
                }
            }

            RiftRequest::GetLayoutState { space_id } => {
                match self.perform_query(|tx| Event::QueryLayoutState { space_id, response: tx }) {
                    Ok(Some(layout_state)) => RiftResponse::Success {
                        data: serde_json::to_value(layout_state).unwrap(),
                    },
                    Ok(None) => RiftResponse::Error {
                        error: serde_json::json!({ "message": "Space not found or inactive" }),
                    },
                    Err(e) => {
                        error!("{}", e);
                        RiftResponse::Error {
                            error: serde_json::json!({ "message": "Failed to get layout state response", "details": format!("{}", e) }),
                        }
                    }
                }
            }

            RiftRequest::GetApplications => {
                match self.perform_query(|tx| Event::QueryApplications(tx)) {
                    Ok(applications) => RiftResponse::Success {
                        data: serde_json::to_value(applications).unwrap(),
                    },
                    Err(e) => {
                        error!("{}", e);
                        RiftResponse::Error {
                            error: serde_json::json!({ "message": "Failed to get applications response", "details": format!("{}", e) }),
                        }
                    }
                }
            }

            RiftRequest::GetMetrics => match self.perform_query(|tx| Event::QueryMetrics(tx)) {
                Ok(metrics) => RiftResponse::Success { data: metrics },
                Err(e) => {
                    error!("{}", e);
                    RiftResponse::Error {
                        error: serde_json::json!({ "message": "Failed to get metrics response", "details": format!("{}", e) }),
                    }
                }
            },

            RiftRequest::GetConfig => {
                match self.perform_config_query(|tx| config_actor::Event::QueryConfig(tx)) {
                    Ok(config) => match serde_json::to_value(&config) {
                        Ok(value) => RiftResponse::Success { data: value },
                        Err(e) => {
                            error!("Failed to serialize config: {}", e);
                            RiftResponse::Error {
                                error: serde_json::json!({ "message": "Failed to serialize config", "details": format!("{}", e) }),
                            }
                        }
                    },
                    Err(e) => {
                        error!("{}", e);
                        RiftResponse::Error {
                            error: serde_json::json!({ "message": "Failed to get config response", "details": format!("{}", e) }),
                        }
                    }
                }
            }

            RiftRequest::ExecuteCommand { command, args } => {
                match serde_json::from_str::<RiftCommand>(&command) {
                    Ok(RiftCommand::Config(_)) => {
                        if args.len() >= 2 && args[0] == "__apply_config__" {
                            match serde_json::from_str::<crate::common::config::ConfigCommand>(
                                &args[1],
                            ) {
                                Ok(cfg_cmd) => match self.perform_config_query(|tx| {
                                    config_actor::Event::ApplyConfig { cmd: cfg_cmd, response: tx }
                                }) {
                                    Ok(apply_result) => match apply_result {
                                        Ok(()) => RiftResponse::Success {
                                            data: serde_json::json!("Config applied successfully"),
                                        },
                                        Err(msg) => RiftResponse::Error {
                                            error: serde_json::json!({ "message": msg }),
                                        },
                                    },
                                    Err(e) => {
                                        error!("{}", e);
                                        RiftResponse::Error {
                                            error: serde_json::json!({ "message": format!("Failed to apply config: {}", e) }),
                                        }
                                    }
                                },
                                Err(e) => {
                                    error!("Failed to parse config command from args: {}", e);
                                    RiftResponse::Error {
                                        error: serde_json::json!({ "message": format!("Invalid config command in args: {}", e) }),
                                    }
                                }
                            }
                        } else {
                            RiftResponse::Success {
                                data: serde_json::json!("No-op config command"),
                            }
                        }
                    }
                    Ok(RiftCommand::Reactor(reactor_command)) => {
                        let event = Event::Command(reactor_command);

                        if let Err(e) = self.reactor_tx.try_send(event) {
                            error!("Failed to send command to reactor: {}", e);
                            return RiftResponse::Error {
                                error: serde_json::json!({ "message": "Failed to execute command", "details": format!("{}", e) }),
                            };
                        }

                        RiftResponse::Success {
                            data: serde_json::json!("Command executed successfully"),
                        }
                    }
                    Err(e) => {
                        error!("Failed to parse command: {}", e);
                        RiftResponse::Error {
                            error: serde_json::json!({ "message": format!("Invalid command format: {}", e) }),
                        }
                    }
                }
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
    if context.is_null() {
        error!("Invalid context pointer");
        return;
    }
    if message.is_null() || len == 0 {
        return;
    }

    let handler = unsafe { &*(context as *const MachHandler) };
    let message_slice = unsafe { std::slice::from_raw_parts(message as *const u8, len as usize) };

    let trimmed_slice = if let Some(pos) = message_slice.iter().position(|&b| b == 0) {
        &message_slice[..pos]
    } else {
        message_slice
    };

    let message_str = match std::str::from_utf8(trimmed_slice) {
        Ok(s) => s,
        Err(e) => {
            let lossy = String::from_utf8_lossy(trimmed_slice);
            error!(
                "Invalid UTF-8 in message after trimming NULs: {}. Contents (lossy): {}",
                e, lossy
            );
            return;
        }
    };

    let client_port = unsafe { (*original_msg).msgh_remote_port };

    let request: RiftRequest = match serde_json::from_str(message_str) {
        Ok(req) => req,
        Err(e) => {
            error!("Failed to parse request: {}", e);
            let error_response = RiftResponse::Error {
                error: serde_json::json!({ "message": format!("Invalid request format: {}", e) }),
            };
            send_response(original_msg, &error_response);
            return;
        }
    };

    let response = handler.handle_request(request, client_port);
    send_response(original_msg, &response);
}

fn send_response(original_msg: *mut mach_msg_header_t, response: &RiftResponse) {
    let mut response_json = serde_json::to_vec(response).unwrap();

    if response_json.last().copied() != Some(0) {
        response_json.push(0);
    }

    unsafe {
        if !send_mach_reply(
            original_msg,
            response_json.as_ptr() as *mut c_char,
            response_json.len() as u32,
        ) {
            error!(
                "Failed to send mach reply for message id {}",
                if original_msg.is_null() {
                    -1
                } else {
                    (*original_msg).msgh_id
                }
            );
        }
    }
}
