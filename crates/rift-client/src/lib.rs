//! A small, synchronous Rust client for Rift's Mach IPC API.
//!
//! The client talks to the same bootstrap service as `rift-cli` and includes
//! the public request and response wire types.

#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

#[cfg(not(target_os = "macos"))]
compile_error!("rift-client only supports macOS");

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::mem::{size_of, zeroed};
use std::ptr::copy_nonoverlapping;
use std::thread;
use std::time::Duration;

pub use rift_protocol::*;
use serde::de::DeserializeOwned;
use serde_json::Value;
use thiserror::Error;

const MAX_MESSAGE_SIZE: usize = 262_144;
const DEFAULT_SERVICE_NAME: &str = "git.acsandmann.rift";

type KernReturn = c_int;
type MachPort = u32;
type MachMessageSize = u32;
type MachMessageOption = u32;

const KERN_SUCCESS: KernReturn = 0;
const MACH_SEND_MSG: MachMessageOption = 0x0000_0001;
const MACH_RCV_MSG: MachMessageOption = 0x0000_0002;
const MACH_MSG_TYPE_COPY_SEND: u32 = 19;
const MACH_MSG_TYPE_MAKE_SEND: u32 = 20;
const MACH_PORT_RIGHT_RECEIVE: c_int = 1;
const MACH_PORT_LIMITS_INFO: c_int = 1;
const MACH_PORT_LIMITS_INFO_COUNT: u32 = 1;
const MACH_PORT_QLIMIT_LARGE: u32 = 1024;
const TASK_BOOTSTRAP_PORT: c_int = 4;

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("Rift's Mach service is not registered")]
    ServiceUnavailable,
    #[error("invalid Mach service name")]
    InvalidServiceName,
    #[error("Mach operation {operation} failed with code {code}")]
    Mach {
        operation: &'static str,
        code: KernReturn,
    },
    #[error("IPC payload exceeds the {MAX_MESSAGE_SIZE}-byte limit")]
    MessageTooLarge,
    #[error("Rift returned an empty response")]
    EmptyResponse,
    #[error("{kind} payload is missing its NUL terminator")]
    MissingTerminator { kind: &'static str },
    #[error("failed to encode request: {0}")]
    Encode(#[source] serde_json::Error),
    #[error("failed to decode {kind}: {source}")]
    Decode {
        kind: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("Rift rejected the subscription: {0}")]
    SubscriptionRejected(Value),
    #[error("Rift returned an error: {0}")]
    Server(Value),
    #[error("Rift returned an unknown response shape")]
    UnknownResponse,
}

/// A handle for issuing synchronous requests to the running Rift process.
#[derive(Clone, Copy, Debug, Default)]
pub struct RiftMachClient;

impl RiftMachClient {
    /// Creates a client handle.
    ///
    /// Service discovery happens when a request is sent, allowing callers to
    /// construct the client before Rift has finished starting.
    pub fn connect() -> Result<Self, ClientError> { Ok(Self) }

    /// Returns whether Rift's bootstrap service is currently registered.
    pub fn is_available(&self) -> bool {
        let service_port = service_name()
            .ok()
            .and_then(|name| unsafe { lookup_service(&name).ok() })
            .map(ServicePort::new);
        service_port.is_some()
    }

    /// Sends one request and blocks until Rift responds.
    pub fn send_request(&self, request: &RiftRequest) -> Result<JsonRiftResponse, ClientError> {
        self.send_typed_request(request)
    }

    /// Sends one request and decodes its response payload into caller-provided
    /// types.
    pub fn send_typed_request<T: DeserializeOwned>(
        &self,
        request: &RiftRequest,
    ) -> Result<RiftResponse<T>, ClientError> {
        let request_json = serde_json::to_vec(request).map_err(ClientError::Encode)?;
        let response = unsafe { send_request(&request_json, None)? };
        parse_json_payload(&response, "response")
    }

    /// Lists virtual workspaces, optionally for a specific macOS space.
    pub fn get_workspaces(&self, space_id: Option<u64>) -> Result<Vec<WorkspaceData>, ClientError> {
        self.request(RiftRequest::GetWorkspaces { space_id })
    }

    /// Lists managed windows, optionally filtered by a macOS space.
    pub fn get_windows(&self, space_id: Option<u64>) -> Result<Vec<WindowData>, ClientError> {
        self.request(RiftRequest::GetWindows { space_id })
    }

    /// Lists connected displays.
    pub fn get_displays(&self) -> Result<Vec<DisplayData>, ClientError> {
        self.request(RiftRequest::GetDisplays)
    }

    /// Returns information about a managed window.
    pub fn get_window_info(&self, window_id: WindowId) -> Result<WindowData, ClientError> {
        self.request(RiftRequest::GetWindowInfo { window_id })
    }

    /// Returns the layout state for a macOS space.
    pub fn get_layout_state(&self, space_id: Option<u64>) -> Result<LayoutStateData, ClientError> {
        self.get_workspace_layout_state(space_id, None)
    }

    /// Returns layout state for an optional macOS space and workspace index.
    /// Omitted selectors independently default to the active space/workspace.
    pub fn get_workspace_layout_state(
        &self,
        space_id: Option<u64>,
        workspace_id: Option<usize>,
    ) -> Result<LayoutStateData, ClientError> {
        self.request(RiftRequest::GetLayoutState { space_id, workspace_id })
    }

    /// Returns layout modes for workspaces in a macOS space.
    pub fn get_workspace_layouts(
        &self,
        space_id: Option<u64>,
        workspace_id: Option<usize>,
    ) -> Result<Vec<WorkspaceLayoutData>, ClientError> {
        self.request(RiftRequest::GetWorkspaceLayouts { space_id, workspace_id })
    }

    /// Lists running applications known to Rift.
    pub fn get_applications(&self) -> Result<Vec<ApplicationData>, ClientError> {
        self.request(RiftRequest::GetApplications)
    }

    /// Returns the current metrics payload.
    pub fn get_metrics(&self) -> Result<Value, ClientError> {
        self.request(RiftRequest::GetMetrics)
    }

    /// Returns the current configuration as JSON until the config model is
    /// moved into `rift-protocol`.
    pub fn get_config(&self) -> Result<Value, ClientError> { self.request(RiftRequest::GetConfig) }

    /// Executes a typed Rift command.
    pub fn execute(&self, command: RiftCommand) -> Result<Value, ClientError> {
        self.request(RiftRequest::ExecuteCommand { command })
    }

    /// Executes a typed Rift command.
    pub fn execute_command(&self, command: RiftCommand) -> Result<Value, ClientError> {
        self.execute(command)
    }

    /// Subscribes to an event and returns a handle that blocks for future events.
    pub fn subscribe(&self, event: EventKind) -> Result<RiftMachSubscription, ClientError> {
        let reply_port = ReplyPort::allocate(MACH_PORT_QLIMIT_LARGE)?;
        let request = RiftRequest::Subscribe { event };
        let request_json = serde_json::to_vec(&request).map_err(ClientError::Encode)?;
        let response = unsafe { send_request(&request_json, Some(reply_port.name))? };

        match parse_json_payload::<RiftResponse>(&response, "response")? {
            RiftResponse::Success { .. } => Ok(RiftMachSubscription { reply_port }),
            RiftResponse::Error { error } => Err(ClientError::SubscriptionRejected(error)),
            _ => Err(ClientError::SubscriptionRejected(Value::String(
                "Rift returned an unknown response shape".to_owned(),
            ))),
        }
    }

    fn request<T: DeserializeOwned>(&self, request: RiftRequest) -> Result<T, ClientError> {
        match self.send_typed_request(&request)? {
            RiftResponse::Success { data } => Ok(data),
            RiftResponse::Error { error } => Err(ClientError::Server(error)),
            _ => Err(ClientError::UnknownResponse),
        }
    }
}

/// A live event subscription. Dropping it releases its Mach receive right.
#[derive(Debug)]
pub struct RiftMachSubscription {
    reply_port: ReplyPort,
}

impl RiftMachSubscription {
    /// Blocks until the next event arrives on this subscription.
    pub fn recv_event(&self) -> Result<RiftEvent, ClientError> { self.recv_event_as() }

    /// Blocks until the next event arrives and returns its raw JSON payload.
    ///
    /// This is useful for compatibility with clients that intentionally handle
    /// newer event variants without upgrading their protocol types.
    pub fn recv_event_value(&self) -> Result<Value, ClientError> { self.recv_event_as() }

    /// Blocks until the next event arrives and decodes it into the requested
    /// type.
    pub fn recv_event_as<T: DeserializeOwned>(&self) -> Result<T, ClientError> {
        let payload = unsafe { receive_message(self.reply_port.name)? };
        parse_json_payload(&payload, "event")
    }
}

fn parse_json_payload<T: DeserializeOwned>(
    payload: &[u8],
    kind: &'static str,
) -> Result<T, ClientError> {
    if payload.is_empty() {
        return Err(ClientError::EmptyResponse);
    }
    let bytes = CStr::from_bytes_until_nul(payload)
        .map_err(|_| ClientError::MissingTerminator { kind })?
        .to_bytes();
    serde_json::from_slice(bytes).map_err(|source| ClientError::Decode { kind, source })
}

fn service_name() -> Result<CString, ClientError> {
    let name = std::env::var("RIFT_BS_NAME").unwrap_or_else(|_| DEFAULT_SERVICE_NAME.to_owned());
    CString::new(name).map_err(|_| ClientError::InvalidServiceName)
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct MachMessageHeader {
    bits: u32,
    size: MachMessageSize,
    remote_port: MachPort,
    local_port: MachPort,
    voucher_port: MachPort,
    id: i32,
}

#[repr(C)]
struct InlineMessage {
    header: MachMessageHeader,
    data: [u8; MAX_MESSAGE_SIZE],
}

#[repr(C)]
struct ReceiveBuffer {
    message: InlineMessage,
    trailer: [u8; 512],
}

#[repr(C)]
struct MachPortLimits {
    queue_limit: u32,
}

#[link(name = "System", kind = "framework")]
unsafe extern "C" {
    fn mach_task_self() -> MachPort;
    fn task_get_special_port(task: MachPort, which: c_int, port: *mut MachPort) -> KernReturn;
    fn mach_port_allocate(task: MachPort, right: c_int, name: *mut MachPort) -> KernReturn;
    fn mach_port_insert_right(
        task: MachPort,
        name: MachPort,
        poly: MachPort,
        disposition: c_int,
    ) -> KernReturn;
    fn mach_port_mod_refs(task: MachPort, name: MachPort, right: c_int, delta: c_int)
    -> KernReturn;
    fn mach_port_deallocate(task: MachPort, name: MachPort) -> KernReturn;
    fn mach_port_set_attributes(
        task: MachPort,
        name: MachPort,
        flavor: c_int,
        info: *const c_void,
        count: u32,
    ) -> KernReturn;
    fn mach_msg(
        message: *mut MachMessageHeader,
        option: MachMessageOption,
        send_size: MachMessageSize,
        receive_size: MachMessageSize,
        receive_name: MachPort,
        timeout: u32,
        notify: MachPort,
    ) -> KernReturn;
    fn mach_msg_destroy(message: *mut MachMessageHeader) -> KernReturn;
    fn bootstrap_look_up(
        bootstrap_port: MachPort,
        service_name: *const c_char,
        service_port: *mut MachPort,
    ) -> KernReturn;
}

#[derive(Debug)]
struct ReplyPort {
    name: MachPort,
}

struct ServicePort {
    name: MachPort,
}

impl ServicePort {
    fn new(name: MachPort) -> Self { Self { name } }
}

impl Drop for ServicePort {
    fn drop(&mut self) {
        unsafe {
            let _ = mach_port_deallocate(mach_task_self(), self.name);
        }
    }
}

impl ReplyPort {
    fn allocate(queue_limit: u32) -> Result<Self, ClientError> {
        unsafe {
            let task = mach_task_self();
            let mut name = 0;
            let result = mach_port_allocate(task, MACH_PORT_RIGHT_RECEIVE, &mut name);
            if result != KERN_SUCCESS {
                return Err(ClientError::Mach {
                    operation: "mach_port_allocate",
                    code: result,
                });
            }

            let limits = MachPortLimits { queue_limit };
            let _ = mach_port_set_attributes(
                task,
                name,
                MACH_PORT_LIMITS_INFO,
                &limits as *const _ as *const c_void,
                MACH_PORT_LIMITS_INFO_COUNT,
            );

            let result = mach_port_insert_right(task, name, name, MACH_MSG_TYPE_MAKE_SEND as c_int);
            if result != KERN_SUCCESS {
                let _ = mach_port_mod_refs(task, name, MACH_PORT_RIGHT_RECEIVE, -1);
                let _ = mach_port_deallocate(task, name);
                return Err(ClientError::Mach {
                    operation: "mach_port_insert_right",
                    code: result,
                });
            }
            Ok(Self { name })
        }
    }
}

impl Drop for ReplyPort {
    fn drop(&mut self) {
        unsafe {
            let task = mach_task_self();
            let _ = mach_port_mod_refs(task, self.name, MACH_PORT_RIGHT_RECEIVE, -1);
            let _ = mach_port_deallocate(task, self.name);
        }
    }
}

unsafe fn lookup_service(name: &CStr) -> Result<MachPort, ClientError> {
    let mut bootstrap_port = 0;
    let result = unsafe {
        task_get_special_port(mach_task_self(), TASK_BOOTSTRAP_PORT, &mut bootstrap_port)
    };
    if result != KERN_SUCCESS {
        return Err(ClientError::Mach {
            operation: "task_get_special_port",
            code: result,
        });
    }

    let mut service_port = 0;
    let result = unsafe { bootstrap_look_up(bootstrap_port, name.as_ptr(), &mut service_port) };
    if result != KERN_SUCCESS {
        return Err(ClientError::ServiceUnavailable);
    }
    Ok(service_port)
}

unsafe fn find_service_with_retry() -> Result<ServicePort, ClientError> {
    let name = service_name()?;
    for attempt in 0..5 {
        if let Ok(port) = unsafe { lookup_service(&name) } {
            return Ok(ServicePort::new(port));
        }
        thread::sleep(Duration::from_millis(50 * (1 << attempt)));
    }
    Err(ClientError::ServiceUnavailable)
}

unsafe fn send_request(
    payload: &[u8],
    subscription_port: Option<MachPort>,
) -> Result<Vec<u8>, ClientError> {
    if payload.len() > MAX_MESSAGE_SIZE {
        return Err(ClientError::MessageTooLarge);
    }

    let service_port = unsafe { find_service_with_retry()? };
    let owned_reply_port;
    let reply_port = match subscription_port {
        Some(port) => port,
        None => {
            owned_reply_port = ReplyPort::allocate(1)?;
            owned_reply_port.name
        }
    };

    let aligned_len = (payload.len() + 3) & !3;
    let mut message: InlineMessage = unsafe { zeroed() };
    message.header.remote_port = service_port.name;
    message.header.local_port = reply_port;
    message.header.id = reply_port as i32;
    message.header.bits = message_bits(
        MACH_MSG_TYPE_COPY_SEND,
        if subscription_port.is_some() {
            MACH_MSG_TYPE_COPY_SEND
        } else {
            MACH_MSG_TYPE_MAKE_SEND
        },
    );
    message.header.size = (size_of::<MachMessageHeader>() + aligned_len) as u32;
    unsafe {
        copy_nonoverlapping(payload.as_ptr(), message.data.as_mut_ptr(), payload.len());
    }

    let result = unsafe {
        mach_msg(
            &mut message.header,
            MACH_SEND_MSG,
            message.header.size,
            0,
            0,
            0,
            0,
        )
    };
    if result != KERN_SUCCESS {
        return Err(ClientError::Mach {
            operation: "mach_msg(send)",
            code: result,
        });
    }

    unsafe { receive_message(reply_port) }
}

unsafe fn receive_message(reply_port: MachPort) -> Result<Vec<u8>, ClientError> {
    let mut buffer: ReceiveBuffer = unsafe { zeroed() };
    let result = unsafe {
        mach_msg(
            &mut buffer.message.header,
            MACH_RCV_MSG,
            0,
            size_of::<ReceiveBuffer>() as u32,
            reply_port,
            0,
            0,
        )
    };
    if result != KERN_SUCCESS {
        return Err(ClientError::Mach {
            operation: "mach_msg(receive)",
            code: result,
        });
    }

    let payload_len =
        buffer.message.header.size.saturating_sub(size_of::<MachMessageHeader>() as u32) as usize;
    if payload_len > MAX_MESSAGE_SIZE {
        unsafe { mach_msg_destroy(&mut buffer.message.header) };
        return Err(ClientError::MessageTooLarge);
    }
    let payload = buffer.message.data[..payload_len].to_vec();
    unsafe { mach_msg_destroy(&mut buffer.message.header) };
    Ok(payload)
}

const fn message_bits(remote: u32, local: u32) -> u32 { remote | (local << 8) }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nul_terminated_response_with_alignment_padding() {
        let response: RiftResponse =
            parse_json_payload(b"{\"data\":true}\0\0\0", "response").unwrap();
        assert_eq!(response.into_result(), Ok(Value::Bool(true)));
    }

    #[test]
    fn rejects_payload_without_nul() {
        let result = parse_json_payload::<RiftResponse>(b"{\"data\":true}", "response");
        assert!(matches!(result, Err(ClientError::MissingTerminator { .. })));
    }

    #[test]
    fn request_uses_the_existing_wire_format() {
        let request = RiftRequest::GetWindows { space_id: Some(7) };
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            serde_json::json!({ "get_windows": { "space_id": 7 } })
        );
    }

    #[test]
    fn response_is_untagged() {
        let response = RiftResponse::Success {
            data: serde_json::json!({ "ok": true }),
        };
        assert_eq!(
            serde_json::to_value(response).unwrap(),
            serde_json::json!({ "data": { "ok": true } })
        );
    }
}
