use std::ffi::{CString, c_char};
use std::time::Duration;

use r#continue::continuation;
use core_foundation::runloop::CFRunLoop;
use tracing::{debug, error, info};

pub mod cli_exec;
pub mod protocol;
pub mod subscriptions;

pub use protocol::{RiftCommand, RiftRequest, RiftResponse};

use crate::actor::reactor::{self, Event};
use crate::ipc::subscriptions::SharedServerState;
use crate::model::server::WorkspaceQueryResponse;
use crate::sys::dispatch::block_on;
use crate::sys::mach::{mach_msg_header_t, mach_send_request, mach_server_run, send_mach_reply};

type ClientPort = u32;

pub fn run_mach_server(reactor_tx: reactor::Sender) -> SharedServerState {
    info!("Spawning background Mach server thread and returning SharedServerState");

    let shared_state: SharedServerState = std::sync::Arc::new(parking_lot::RwLock::new(
        crate::ipc::subscriptions::ServerState::new(),
    ));

    let thread_state = shared_state.clone();
    std::thread::spawn(move || {
        let s = thread_state.write();
        s.set_runloop(Some(CFRunLoop::get_current()));

        let handler = MachHandler::new(reactor_tx, thread_state.clone());
        unsafe {
            mach_server_run(Box::into_raw(Box::new(handler)) as *mut _, handle_mach_request_c);
        }
    });

    shared_state
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
    server_state: SharedServerState,
}

impl MachHandler {
    fn new(reactor_tx: reactor::Sender, server_state: SharedServerState) -> Self {
        Self { reactor_tx, server_state }
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
            return Err(format!("Failed to send query: {}", e));
        }

        match block_on(cont_fut, Duration::from_secs(5)) {
            Ok(res) => Ok(res),
            Err(e) => Err(format!("Failed to get response: {}", e)),
        }
    }

    fn handle_request(&self, request: RiftRequest, client_port: ClientPort) -> RiftResponse {
        debug!("Handling request: {:?} from client {}", request, client_port);

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

            RiftRequest::GetWorkspaces => match self.perform_query(|tx| Event::QueryWorkspaces(tx))
            {
                Ok(WorkspaceQueryResponse { workspaces }) => RiftResponse::Success {
                    data: serde_json::to_value(workspaces).unwrap(),
                },
                Err(e) => {
                    error!("{}", e);
                    RiftResponse::Error {
                        error: serde_json::json!({ "message": "Failed to get workspace response", "details": format!("{}", e) }),
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

            RiftRequest::GetConfig => match self.perform_query(|tx| Event::QueryConfig(tx)) {
                Ok(config) => RiftResponse::Success { data: config },
                Err(e) => {
                    error!("{}", e);
                    RiftResponse::Error {
                        error: serde_json::json!({ "message": "Failed to get config response", "details": format!("{}", e) }),
                    }
                }
            },

            RiftRequest::ExecuteCommand { command, args } => {
                match serde_json::from_str::<RiftCommand>(&command) {
                    Ok(RiftCommand::Reactor(reactor_command)) => {
                        if args.len() >= 2 && args[0] == "__apply_config__" {
                            match serde_json::from_str::<crate::common::config::ConfigCommand>(
                                &args[1],
                            ) {
                                Ok(cfg_cmd) => {
                                    match self.perform_query(|tx| Event::ApplyConfig {
                                        cmd: cfg_cmd,
                                        response: tx,
                                    }) {
                                        Ok(apply_result) => match apply_result {
                                            Ok(()) => RiftResponse::Success {
                                                data: serde_json::json!(
                                                    "Config applied successfully"
                                                ),
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
                                    }
                                }
                                Err(e) => {
                                    error!("Failed to parse config command from args: {}", e);
                                    RiftResponse::Error {
                                        error: serde_json::json!({ "message": format!("Invalid config command in args: {}", e) }),
                                    }
                                }
                            }
                        } else {
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
