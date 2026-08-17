use std::cell::RefCell;
use std::ffi::c_char;
use std::rc::Rc;
use std::sync::mpsc::sync_channel;
use std::time::Duration;

use r#continue::continuation;
use crossbeam_channel::{Sender as ConfigJobSender, TrySendError, bounded};
use serde::Serialize;
use tracing::{error, info, trace};

pub mod cli_exec;
pub mod protocol;
pub mod subscriptions;

pub use protocol::{RiftCommand, RiftRequest, RiftResponse};
pub use rift_client::{ClientError as RiftMachClientError, RiftMachClient, RiftMachSubscription};

use crate::actor::config as config_actor;
use crate::actor::reactor::{self, Event};
use crate::ipc::subscriptions::SharedServerState;
use crate::sys::dispatch::block_on;
use crate::sys::mach::{
    OwnedMachReply, is_mach_server_registered, mach_msg_header_t, mach_server_install,
    send_mach_reply,
};

type ClientPort = u32;

struct ConfigJob {
    request: RiftRequest,
    destination: OwnedMachReply,
}

const CONFIG_QUEUE_CAPACITY: usize = 8;

pub struct InstallRequest {
    config_tx: config_actor::Sender,
    response: std::sync::mpsc::SyncSender<Result<SharedServerState, String>>,
}

impl std::fmt::Debug for InstallRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("InstallRequest(..)")
    }
}

pub fn run_mach_server(
    reactor: reactor::ReactorHandle,
    config_tx: config_actor::Sender,
) -> Result<SharedServerState, String> {
    if is_mach_server_registered() {
        return Err(
            "Another Rift instance is already running; quit it before starting another.".into(),
        );
    }
    let (response, result) = sync_channel(1);
    reactor
        .try_send(Event::InstallIpc(InstallRequest { config_tx, response }))
        .map_err(|_| "Reactor is unavailable".to_string())?;
    result.recv().map_err(|_| "Reactor stopped while installing IPC".to_string())?
}

pub(crate) fn install_mach_server(reactor: Rc<RefCell<reactor::Reactor>>, request: InstallRequest) {
    let InstallRequest { config_tx, response } = request;
    let result = (|| {
        if is_mach_server_registered() {
            return Err(
                "Another Rift instance is already running; quit it before starting another.".into(),
            );
        }

        let server_state: SharedServerState =
            std::sync::Arc::new(crate::ipc::subscriptions::ServerState::new());
        let config_handler = ConfigRequestHandler { config_tx };
        let (config_jobs, jobs) = bounded::<ConfigJob>(CONFIG_QUEUE_CAPACITY);
        // Config has a different state owner and can wait for disk I/O. Keep
        // that exceptional path bounded and off the reactor run loop.
        std::thread::Builder::new()
            .name("ipc-config".into())
            .spawn(move || {
                while let Ok(mut job) = jobs.recv() {
                    let response = config_handler.handle_request(job.request);
                    send_encoded_response(job.destination.header_mut(), &response);
                }
            })
            .map_err(|error| format!("Failed to spawn IPC config worker: {error}"))?;

        let handler = Box::new(IpcRequestHandler::new(
            reactor,
            server_state.clone(),
            config_jobs,
        ));
        let context = Box::into_raw(handler);
        if !unsafe { mach_server_install(context.cast(), handle_mach_request_c) } {
            unsafe { drop(Box::from_raw(context)) };
            return Err("Failed to install Mach IPC on the reactor run loop".into());
        }
        info!("Installed Mach IPC on the reactor run loop");
        Ok(server_state)
    })();
    let _ = response.send(result);
}

struct IpcRequestHandler {
    // CFMachPort invokes this handler on the reactor's own CFRunLoop thread.
    // RefCell enforces local borrow exclusivity without a mutex or state mirror.
    reactor: Rc<RefCell<reactor::Reactor>>,
    server_state: SharedServerState,
    config_jobs: ConfigJobSender<ConfigJob>,
}

impl IpcRequestHandler {
    fn new(
        reactor: Rc<RefCell<reactor::Reactor>>,
        server_state: SharedServerState,
        config_jobs: ConfigJobSender<ConfigJob>,
    ) -> Self {
        Self {
            reactor,
            server_state,
            config_jobs,
        }
    }

    fn handle_message(
        &self,
        payload: &[u8],
        client_port: ClientPort,
        header: &mut mach_msg_header_t,
    ) {
        let message = match std::str::from_utf8(payload) {
            Ok(message) => message,
            Err(error) => {
                error!("Invalid UTF-8 in IPC request: {error}");
                send_error_response(header, "IPC request is not valid UTF-8");
                return;
            }
        };
        let request = match serde_json::from_str(message) {
            Ok(request) => request,
            Err(error) => {
                error!("Failed to parse IPC request: {error}");
                send_error_response(header, &format!("Invalid request format: {error}"));
                return;
            }
        };
        trace!(?request, client_port, "Handling IPC request");

        let response = match request {
            request @ (RiftRequest::GetConfig
            | RiftRequest::ExecuteCommand {
                command: rift_protocol::RiftCommand::Config(_),
            }) => {
                self.dispatch_config_request(request, header);
                return;
            }
            RiftRequest::Subscribe { event } => {
                self.server_state.subscribe_client(client_port, event.to_string());
                encode_success(serde_json::json!({ "subscribed": event.to_string() }))
            }
            RiftRequest::Unsubscribe { event } => {
                self.server_state.unsubscribe_client(client_port, event.to_string());
                encode_success(serde_json::json!({ "unsubscribed": event.to_string() }))
            }
            RiftRequest::SubscribeCli { event, command, args } => {
                self.server_state
                    .subscribe_cli(event.to_string(), command.clone(), args.clone());
                encode_success(serde_json::json!({
                    "cli_subscribed": event.to_string(),
                    "command": command,
                    "args": args
                }))
            }
            RiftRequest::UnsubscribeCli { event } => {
                self.server_state.unsubscribe_cli(event.to_string());
                encode_success(serde_json::json!({ "cli_unsubscribed": event.to_string() }))
            }
            RiftRequest::ListCliSubscriptions => {
                encode_success(self.server_state.list_cli_subscriptions())
            }
            request => match self.reactor.try_borrow_mut() {
                Ok(mut reactor) => encode_reactor_response(&mut reactor, request),
                Err(_) => encode_error(serde_json::json!({ "message": "Reactor is busy" })),
            },
        };
        send_encoded_response(header, &response);
    }

    fn dispatch_config_request(&self, request: RiftRequest, header: &mut mach_msg_header_t) {
        let Some(destination) = (unsafe { OwnedMachReply::retain(header) }) else {
            send_error_response(header, "Failed to retain IPC reply port");
            return;
        };
        let job = ConfigJob { request, destination };
        match self.config_jobs.try_send(job) {
            Ok(()) => {}
            Err(TrySendError::Full(mut job)) => send_error_response(
                job.destination.header_mut(),
                "Too many config requests in flight; try again",
            ),
            Err(TrySendError::Disconnected(mut job)) => send_error_response(
                job.destination.header_mut(),
                "Config request service is unavailable",
            ),
        }
    }
}

fn encode_reactor_response(reactor: &mut reactor::Reactor, request: RiftRequest) -> Vec<u8> {
    match request {
        RiftRequest::GetWorkspaces { space_id } => {
            let workspaces =
                reactor.query_workspaces(space_id.map(crate::sys::screen::SpaceId::new));
            encode_success(
                workspaces
                    .into_iter()
                    .map(rift_protocol::WorkspaceData::from)
                    .collect::<Vec<_>>(),
            )
        }

        RiftRequest::GetDisplays => {
            let displays = reactor.query_displays();
            encode_success(
                displays.into_iter().map(rift_protocol::DisplayData::from).collect::<Vec<_>>(),
            )
        }

        RiftRequest::GetWindows { space_id } => {
            let windows = reactor.query_windows(space_id.map(crate::sys::screen::SpaceId::new));
            encode_success(
                windows.into_iter().map(rift_protocol::WindowData::from).collect::<Vec<_>>(),
            )
        }

        RiftRequest::GetWindowInfo { window_id } => {
            let window_id = crate::actor::app::WindowId::new(window_id.pid, window_id.idx);
            match reactor.query_window_info(window_id) {
                Some(window) => encode_success(rift_protocol::WindowData::from(window)),
                None => encode_error(serde_json::json!({ "message": "Window not found" })),
            }
        }

        RiftRequest::GetLayoutState { space_id, workspace_id } => {
            match reactor.query_layout_state(space_id, workspace_id) {
                Some(layout_state) => encode_success(layout_state),
                None => {
                    encode_error(serde_json::json!({ "message": "Space or workspace not found" }))
                }
            }
        }

        RiftRequest::GetWorkspaceLayouts { space_id, workspace_id } => {
            let workspace_layouts = reactor.query_workspace_layouts(
                space_id.map(crate::sys::screen::SpaceId::new),
                workspace_id,
            );
            encode_success(workspace_layouts)
        }

        RiftRequest::GetApplications => {
            let applications = reactor.query_applications();
            encode_success(applications)
        }

        RiftRequest::GetMetrics => encode_success(reactor.query_metrics()),

        RiftRequest::GetConfig => unreachable!("config requests run on config workers"),

        RiftRequest::ExecuteCommand { command } => match command {
            rift_protocol::RiftCommand::Config(_) => {
                unreachable!("config requests run on config workers")
            }
            rift_protocol::RiftCommand::Layout(command) => {
                handle_reactor_command(reactor, crate::model::reactor::Command::Layout(command))
            }
            rift_protocol::RiftCommand::Metrics(command) => {
                handle_reactor_command(reactor, crate::model::reactor::Command::Metrics(command))
            }
            rift_protocol::RiftCommand::Reactor(command) => {
                handle_reactor_command(reactor, crate::model::reactor::Command::Reactor(command))
            }
        },
        _ => encode_error(serde_json::json!({ "message": "Unsupported request" })),
    }
}

fn handle_reactor_command(
    reactor: &mut reactor::Reactor,
    command: crate::model::reactor::Command,
) -> Vec<u8> {
    reactor.handle_ipc_command(command);
    encode_success("Command executed successfully")
}

#[derive(Clone)]
struct ConfigRequestHandler {
    config_tx: config_actor::Sender,
}

impl ConfigRequestHandler {
    fn forget_sender(event: config_actor::Event) {
        match event {
            config_actor::Event::QueryConfig(response) => std::mem::forget(response),
            config_actor::Event::ApplyConfig { response, .. } => std::mem::forget(response),
        }
    }

    fn perform<T>(
        &self,
        make_event: impl FnOnce(r#continue::Sender<T>) -> config_actor::Event,
    ) -> Result<T, String>
    where
        T: Send + 'static,
    {
        let (response, future) = continuation::<T>();
        let event = make_event(response);
        if let Err(error) = self.config_tx.try_send(event) {
            let message = error.to_string();
            let tokio::sync::mpsc::error::SendError((_span, event)) = error;
            Self::forget_sender(event);
            return Err(format!("Failed to send config query: {message}"));
        }
        block_on(future, Duration::from_secs(5))
            .map_err(|error| format!("Failed to get response: {error}"))
    }

    fn handle_request(&self, request: RiftRequest) -> Vec<u8> {
        match request {
            RiftRequest::GetConfig => match self.perform(config_actor::Event::QueryConfig) {
                Ok(config) => encode_success(config),
                Err(error) => encode_error(serde_json::json!({
                    "message": "Failed to get config response",
                    "details": error,
                })),
            },
            RiftRequest::ExecuteCommand {
                command: rift_protocol::RiftCommand::Config(command),
            } => match self
                .perform(|response| config_actor::Event::ApplyConfig { cmd: command, response })
            {
                Ok(Ok(())) => encode_success("Config applied successfully"),
                Ok(Err(message)) => encode_error(serde_json::json!({ "message": message })),
                Err(error) => encode_error(serde_json::json!({
                    "message": format!("Failed to apply config: {error}"),
                })),
            },
            _ => unreachable!("only config requests are queued to config workers"),
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
    if message.is_null() || len == 0 || original_msg.is_null() {
        error!("Invalid Mach request pointers");
        return;
    }

    let handler = unsafe { &*(context as *const IpcRequestHandler) };
    let message_slice = unsafe { std::slice::from_raw_parts(message as *const u8, len as usize) };

    let trimmed_slice = if let Some(pos) = message_slice.iter().position(|&b| b == 0) {
        &message_slice[..pos]
    } else {
        message_slice
    };

    let client_port = unsafe { (*original_msg).msgh_remote_port };
    handler.handle_message(trimmed_slice, client_port, unsafe { &mut *original_msg });
}

fn send_error_response(header: &mut mach_msg_header_t, message: &str) {
    let response = encode_error(serde_json::json!({ "message": message }));
    send_encoded_response(header, &response);
}

fn encode_success<T: Serialize>(data: T) -> Vec<u8> {
    encode_response(&RiftResponse::Success { data })
}

fn encode_error(error: serde_json::Value) -> Vec<u8> {
    encode_response::<serde_json::Value>(&RiftResponse::Error { error })
}

fn encode_response<T: Serialize>(response: &RiftResponse<T>) -> Vec<u8> {
    let mut response_json = match serde_json::to_vec(response) {
        Ok(response) => response,
        Err(error) => {
            error!("Failed to encode IPC response: {error}");
            br#"{"error":{"message":"Failed to encode IPC response"}}"#.to_vec()
        }
    };

    if response_json.last().copied() != Some(0) {
        response_json.push(0);
    }
    response_json
}

fn send_encoded_response(original_msg: *mut mach_msg_header_t, response_json: &[u8]) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::config::{LayoutSettings, VirtualWorkspaceSettings};
    use crate::layout_engine::LayoutEngine;

    #[test]
    fn reactor_ipc_queries_encode_without_a_response_channel() {
        let mut reactor = reactor::Reactor::new_for_test(LayoutEngine::new(
            &VirtualWorkspaceSettings::default(),
            &LayoutSettings::default(),
            None,
        ));

        let response = encode_reactor_response(&mut reactor, RiftRequest::GetMetrics);
        let response: RiftResponse =
            serde_json::from_slice(response.strip_suffix(&[0]).unwrap()).unwrap();
        let RiftResponse::Success { data } = response else {
            panic!("expected metrics response");
        };
        assert_eq!(data["screens"], 0);
    }
}
