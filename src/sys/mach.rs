// this is a mess and needs a heavy cleanup

#![allow(non_camel_case_types)]
#![allow(dead_code)]
#![allow(improper_ctypes)]
#![allow(unsafe_op_in_unsafe_fn)]
#![allow(non_snake_case)]
#![allow(clippy::missing_safety_doc)]

use core::mem::{size_of, zeroed};
use core::ptr::{copy_nonoverlapping, null, null_mut};
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};

use tracing::{debug, error, info};

const MAX_MESSAGE_SIZE: u32 = 16_384;
const MACH_BS_NAME_FMT_PREFIX: &str = "git.";
static G_NAME: &str = "acsandmann.rift";

fn bs_name() -> CString {
    if let Ok(name) = std::env::var("RIFT_BS_NAME") {
        return CString::new(name).unwrap();
    }
    CString::new(format!("{}{}", MACH_BS_NAME_FMT_PREFIX, G_NAME)).unwrap()
}

pub fn is_mach_server_registered() -> bool {
    let bs_name = bs_name();
    unsafe { mach_get_bs_port(&bs_name) != 0 }
}

type kern_return_t = c_int;
type mach_port_t = u32;
type mach_port_name_t = u32;
type mach_msg_bits_t = u32;
type mach_msg_size_t = u32;
type mach_msg_option_t = u32;
type mach_msg_id_t = i32;

const KERN_SUCCESS: kern_return_t = 0;
const MACH_MSG_SUCCESS: kern_return_t = 0;

const MACH_SEND_MSG: mach_msg_option_t = 0x0000_0001;
const MACH_RCV_MSG: mach_msg_option_t = 0x0000_0002;
const MACH_RCV_TIMEOUT: mach_msg_option_t = 0x0000_0100;

const MACH_MSG_TIMEOUT_NONE: u32 = 0;

const MACH_MSG_TYPE_COPY_SEND: u32 = 19;
const MACH_MSG_TYPE_MOVE_SEND_ONCE: u32 = 18;
const MACH_MSG_TYPE_MAKE_SEND_ONCE: u32 = 21;
const MACH_MSGH_BITS_COMPLEX: u32 = 0x8000_0000;
const MACH_MSG_TYPE_MAKE_SEND: u32 = 20;

const MACH_PORT_RIGHT_RECEIVE: c_int = 1;
const MACH_PORT_LIMITS_INFO: c_int = 1;
const MACH_PORT_LIMITS_INFO_COUNT: u32 = 1;
const MACH_PORT_QLIMIT_LARGE: u32 = 1024;

const TASK_BOOTSTRAP_PORT: c_int = 4;

const BOOTSTRAP_NOT_PRIVILEGED: kern_return_t = 1100;
const BOOTSTRAP_NAME_IN_USE: kern_return_t = 1101;
const BOOTSTRAP_UNKNOWN_SERVICE: kern_return_t = 1102;

#[inline]
const fn MACH_MSGH_BITS(remote: u32, local: u32) -> u32 { remote | (local << 8) }

#[inline]
const fn MACH_MSGH_BITS_REMOTE(bits: u32) -> u32 { bits & 0xff }

#[inline]
const fn MACH_MSGH_BITS_LOCAL(bits: u32) -> u32 { (bits >> 8) & 0xff }

type CFIndex = isize;
type CFAllocatorRef = *const c_void;
type CFStringRef = *const c_void;
type CFMachPortRef = *const c_void;
type CFRunLoopSourceRef = *const c_void;
type CFRunLoopRef = *const c_void;

#[repr(C)]
struct CFMachPortContext {
    version: CFIndex,
    info: *mut c_void,
    retain: Option<extern "C" fn(*const c_void) -> *const c_void>,
    release: Option<extern "C" fn(*const c_void)>,
    #[allow(non_snake_case)]
    copyDescription: Option<extern "C" fn(*const c_void) -> CFStringRef>,
}

type CFMachPortCallBack =
    Option<extern "C" fn(port: CFMachPortRef, msg: *mut c_void, size: CFIndex, info: *mut c_void)>;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFMachPortCreateWithPort(
        allocator: CFAllocatorRef,
        portNum: mach_port_t,
        callout: CFMachPortCallBack,
        context: *const CFMachPortContext,
        shouldFreeInfo: u8,
    ) -> CFMachPortRef;

    fn CFMachPortCreateRunLoopSource(
        allocator: CFAllocatorRef,
        port: CFMachPortRef,
        order: c_int,
    ) -> CFRunLoopSourceRef;

    fn CFRunLoopAddSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);
    fn CFRunLoopGetCurrent() -> CFRunLoopRef;
    fn CFRunLoopGetMain() -> CFRunLoopRef;
    fn CFRunLoopRun();

    fn CFRelease(obj: *const c_void);

    static kCFRunLoopCommonModes: CFStringRef;
    static kCFRunLoopDefaultMode: CFStringRef;
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct mach_msg_header_t {
    pub msgh_bits: mach_msg_bits_t,
    pub msgh_size: mach_msg_size_t,
    pub msgh_remote_port: mach_port_t,
    pub msgh_local_port: mach_port_t,
    pub msgh_voucher_port: mach_port_name_t,
    pub msgh_id: mach_msg_id_t,
}

#[repr(C)]
struct mach_port_limits {
    mpl_qlimit: u32,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct mach_msg_body_t {
    msgh_descriptor_count: u32,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct mach_msg_ool_descriptor_t {
    address: *mut c_void,
    size: u32,
    deallocate: u8, // boolean_t
    copy: u8,       // mach_msg_copy_options_t
    pad1: u32,
    type_: u32, // MACH_MSG_OOL_DESCRIPTOR = 1
}

const MACH_MSG_OOL_DESCRIPTOR: u32 = 1;
const MACH_MSG_VIRTUAL_COPY: u8 = 1;

#[repr(C)]
struct mach_message {
    header: mach_msg_header_t,
    msgh_descriptor_count: u32,
    descriptor: mach_msg_ool_descriptor_t,
}

#[repr(C)]
struct mach_buffer {
    message: simple_message,
    trailer: [u8; 512],
}

#[link(name = "System", kind = "framework")]
unsafe extern "C" {
    fn mach_task_self() -> mach_port_name_t;

    fn task_get_special_port(
        task: mach_port_name_t,
        which: c_int,
        special_port: *mut mach_port_t,
    ) -> kern_return_t;

    fn mach_port_allocate(
        task: mach_port_name_t,
        right: c_int,
        name: *mut mach_port_name_t,
    ) -> kern_return_t;

    fn mach_port_insert_right(
        task: mach_port_name_t,
        name: mach_port_name_t,
        poly: mach_port_t,
        polyPoly: c_int,
    ) -> kern_return_t;

    fn mach_port_mod_refs(
        task: mach_port_name_t,
        name: mach_port_name_t,
        right: c_int,
        delta: c_int,
    ) -> kern_return_t;

    fn mach_port_deallocate(task: mach_port_name_t, name: mach_port_name_t) -> kern_return_t;

    fn mach_port_set_attributes(
        task: mach_port_name_t,
        name: mach_port_name_t,
        flavor: c_int,
        info: *const c_void,
        count: u32,
    ) -> kern_return_t;

    fn mach_port_type(
        task: mach_port_name_t,
        name: mach_port_name_t,
        ptype: *mut u32,
    ) -> kern_return_t;

    fn mach_msg(
        msg: *mut mach_msg_header_t,
        option: mach_msg_option_t,
        send_size: mach_msg_size_t,
        rcv_size: mach_msg_size_t,
        rcv_name: mach_port_name_t,
        timeout: u32,
        notify: mach_port_name_t,
    ) -> kern_return_t;

    fn mach_msg_destroy(msg: *mut mach_msg_header_t) -> kern_return_t;

    fn bootstrap_look_up(
        bp: mach_port_t,
        service_name: *const c_char,
        sp: *mut mach_port_t,
    ) -> kern_return_t;

    fn bootstrap_check_in(
        bp: mach_port_t,
        service_name: *const c_char,
        sp: *mut mach_port_t,
    ) -> kern_return_t;

    fn bootstrap_register(
        bp: mach_port_t,
        service_name: *const c_char,
        sp: mach_port_t,
    ) -> kern_return_t;

    fn bootstrap_register2(
        bp: mach_port_t,
        service_name: *const c_char,
        sp: mach_port_t,
        flags: u64,
    ) -> kern_return_t;
}

#[repr(C)]
struct simple_message {
    header: mach_msg_header_t,
    data: [u8; MAX_MESSAGE_SIZE as usize],
}

unsafe fn mach_get_bs_port(bs_name: &CStr) -> mach_port_t {
    let mut bs_port: mach_port_t = 0;
    if task_get_special_port(mach_task_self(), TASK_BOOTSTRAP_PORT, &mut bs_port) != KERN_SUCCESS {
        error!("mach_get_bs_port: task_get_special_port failed");
        return 0;
    }

    let mut service_port: mach_port_t = 0;
    let result = bootstrap_look_up(bs_port, bs_name.as_ptr(), &mut service_port);
    if result != KERN_SUCCESS {
        if result != BOOTSTRAP_UNKNOWN_SERVICE {
            error!(
                "mach_get_bs_port: bootstrap_look_up failed for {} (kr={})",
                bs_name.to_string_lossy(),
                result
            );
        } else {
            debug!(
                "mach_get_bs_port: {} is not registered yet (kr={})",
                bs_name.to_string_lossy(),
                result
            );
        }
        return 0;
    }
    service_port
}

pub unsafe fn mach_send_message(
    port: mach_port_t,
    message: *const c_char,
    len: u32,
    await_response: bool,
) -> *mut c_char {
    if message.is_null() || port == 0 || len > MAX_MESSAGE_SIZE {
        error!(
            "mach_send_message: invalid input args message={:?} port={} len={}",
            message, port, len
        );
        return null_mut();
    }

    let mut reply_port: mach_port_t = 0;
    let task = mach_task_self();

    if await_response {
        if mach_port_allocate(task, MACH_PORT_RIGHT_RECEIVE, &mut reply_port) != KERN_SUCCESS {
            error!("mach_send_message: mach_port_allocate failed for reply port");
            return null_mut();
        }
        let limits = mach_port_limits { mpl_qlimit: 1 };
        let _ = mach_port_set_attributes(
            task,
            reply_port,
            MACH_PORT_LIMITS_INFO,
            &limits as *const _ as *const c_void,
            MACH_PORT_LIMITS_INFO_COUNT,
        );

        let ir =
            mach_port_insert_right(task, reply_port, reply_port, MACH_MSG_TYPE_MAKE_SEND as c_int);
        if ir != KERN_SUCCESS {
            error!(
                "mach_send_message: mach_port_insert_right failed for reply port (kr={})",
                ir
            );
            let _ = mach_port_mod_refs(task, reply_port, MACH_PORT_RIGHT_RECEIVE, -1);
            let _ = mach_port_deallocate(task, reply_port);
            return null_mut();
        }
    }

    let mut msg: simple_message = zeroed();

    msg.header.msgh_remote_port = port;
    msg.header.msgh_local_port = if await_response { reply_port } else { 0 };
    msg.header.msgh_voucher_port = 0;
    msg.header.msgh_id = if await_response { reply_port as i32 } else { 0 };
    msg.header.msgh_bits = MACH_MSGH_BITS(
        MACH_MSG_TYPE_COPY_SEND,
        if await_response {
            MACH_MSG_TYPE_MAKE_SEND
        } else {
            0
        },
    );

    let aligned_len = (len + 3) & !3;
    msg.header.msgh_size = (size_of::<mach_msg_header_t>() as u32) + aligned_len;
    let mut sm: simple_message = zeroed();
    sm.header = msg.header;
    copy_nonoverlapping(message as *const u8, sm.data.as_mut_ptr(), len as usize);
    if aligned_len > len {
        let pad = (aligned_len - len) as usize;
        core::ptr::write_bytes(sm.data.as_mut_ptr().add(len as usize), 0, pad);
    }

    let send_result = mach_msg(
        &mut sm.header,
        MACH_SEND_MSG,
        sm.header.msgh_size,
        0,
        0,
        MACH_MSG_TIMEOUT_NONE,
        0,
    );

    if send_result != MACH_MSG_SUCCESS {
        error!(
            "mach_send_message: mach_msg send failed (result={} remote_port={} reply_port={})",
            send_result, port, reply_port
        );
        if await_response && reply_port != 0 {
            let _ = mach_port_mod_refs(task, reply_port, MACH_PORT_RIGHT_RECEIVE, -1);
            let _ = mach_port_deallocate(task, reply_port);
        }
        return null_mut();
    }

    if await_response {
        let mut buffer: mach_buffer = zeroed();
        let recv_result = mach_msg(
            &mut buffer.message.header,
            MACH_RCV_MSG,
            0,
            size_of::<mach_buffer>() as u32,
            reply_port,
            MACH_MSG_TIMEOUT_NONE,
            0,
        );

        let _ = mach_port_mod_refs(task, reply_port, MACH_PORT_RIGHT_RECEIVE, -1);
        let _ = mach_port_deallocate(task, reply_port);

        if recv_result != MACH_MSG_SUCCESS {
            error!(
                "mach_send_message: failed to receive response (recv_result={} reply_port={})",
                recv_result, reply_port
            );
            return null_mut();
        }

        let mut rsp_ptr: *mut c_char = null_mut();
        let mut rsp_len: usize = 0;

        let inline_len = buffer
            .message
            .header
            .msgh_size
            .saturating_sub(size_of::<mach_msg_header_t>() as u32)
            as usize;
        if inline_len > 0 {
            let sm_ptr =
                (&mut buffer.message.header) as *mut mach_msg_header_t as *mut simple_message;
            rsp_len = inline_len;
            rsp_ptr = unsafe { (*sm_ptr).data.as_mut_ptr() as *mut c_char };
        }

        let layout = match std::alloc::Layout::array::<c_char>(rsp_len + 1) {
            Ok(l) => l,
            Err(_) => return null_mut(),
        };
        let out = std::alloc::alloc(layout) as *mut c_char;
        if out.is_null() {
            return null_mut();
        }
        if rsp_len > 0 && !rsp_ptr.is_null() {
            copy_nonoverlapping(rsp_ptr as *const u8, out as *mut u8, rsp_len);
        }
        *out.add(rsp_len) = 0;

        mach_msg_destroy(&mut buffer.message.header);
        return out;
    }

    1 as *mut c_char
}

pub unsafe fn mach_send_request(message: *const c_char, len: u32) -> *mut c_char {
    if message.is_null() || len > MAX_MESSAGE_SIZE {
        error!(
            "mach_send_request: invalid args message={:?} len={}",
            message, len
        );
        return null_mut();
    }

    let service_name = bs_name();

    let mut service_port: mach_port_t = 0;
    let mut attempt = 0;
    while attempt < 5 {
        service_port = mach_get_bs_port(&service_name);
        if service_port != 0 {
            break;
        }
        let backoff_ms = 50u64.saturating_mul(1u64 << attempt);
        std::thread::sleep(std::time::Duration::from_millis(backoff_ms));
        attempt += 1;
    }

    if service_port == 0 {
        error!(
            "mach_send_request: mach_get_bs_port returned 0 for {} after {} attempts",
            service_name.to_string_lossy(),
            attempt
        );
        return null_mut();
    }

    mach_send_message(service_port, message, len, true)
}

pub unsafe fn mach_free_response(ptr: *mut c_char, len: u32) {
    if ptr.is_null() {
        return;
    }
    let layout = std::alloc::Layout::array::<c_char>((len as usize) + 1).unwrap();
    std::alloc::dealloc(ptr as *mut u8, layout);
}

pub type mach_handler = unsafe extern "C" fn(
    context: *mut c_void,
    message: *mut c_char,
    len: u32,
    original_msg: *mut mach_msg_header_t,
);

#[repr(C)]
pub struct mach_server {
    is_running: bool,
    task: mach_port_name_t,
    port: mach_port_t,
    bs_port: mach_port_t,
    handler: Option<mach_handler>,
    context: *mut c_void,
}

impl Default for mach_server {
    fn default() -> Self {
        Self {
            is_running: false,
            task: 0,
            port: 0,
            bs_port: 0,
            handler: None,
            context: null_mut(),
        }
    }
}

extern "C" fn mach_message_callback(
    _port: CFMachPortRef,
    message: *mut c_void,
    _size: CFIndex,
    context: *mut c_void,
) {
    unsafe {
        if context.is_null() || message.is_null() {
            return;
        }
        let mach_server = &mut *(context as *mut mach_server);
        let header_val = core::ptr::read_unaligned(message as *const mach_msg_header_t);
        let header_ptr = &header_val as *const mach_msg_header_t as *mut mach_msg_header_t;
        if header_val.msgh_remote_port == 0 {
            return;
        }

        let mut payload_ptr: *mut c_char = null_mut();
        let mut payload_len: u32 = 0;

        if (header_val.msgh_bits & MACH_MSGH_BITS_COMPLEX) != 0 {
            let body_ptr = (message as *const u8).add(size_of::<mach_msg_header_t>())
                as *const mach_msg_body_t;
            let body_val = core::ptr::read_unaligned(body_ptr);
            if body_val.msgh_descriptor_count >= 1 {
                let desc_ptr = ((body_ptr as usize + size_of::<mach_msg_body_t>() + 7) & !7)
                    as *const mach_msg_ool_descriptor_t;
                let desc_val = core::ptr::read_unaligned(desc_ptr);
                payload_ptr = desc_val.address as *mut c_char;
                payload_len = desc_val.size;
                if payload_ptr.is_null() || payload_len == 0 {
                    payload_len =
                        header_val.msgh_size.saturating_sub(size_of::<mach_msg_header_t>() as u32);
                    payload_ptr =
                        (message as *mut u8).add(size_of::<mach_msg_header_t>()) as *mut c_char;
                }
            }
        } else {
            payload_len =
                header_val.msgh_size.saturating_sub(size_of::<mach_msg_header_t>() as u32);
            payload_ptr = (message as *mut u8).add(size_of::<mach_msg_header_t>()) as *mut c_char;
        }

        if let Some(handler) = mach_server.handler {
            handler(mach_server.context, payload_ptr, payload_len, header_ptr);
        }

        let _ = mach_msg_destroy(message as *mut mach_msg_header_t);
    }
}

pub unsafe fn mach_server_begin(
    mach_server: &mut mach_server,
    context: *mut c_void,
    handler: mach_handler,
) -> bool {
    mach_server.task = mach_task_self();

    if task_get_special_port(mach_server.task, TASK_BOOTSTRAP_PORT, &mut mach_server.bs_port)
        != KERN_SUCCESS
    {
        error!("mach_server_begin: task_get_special_port failed");
        return false;
    }

    let service_name = bs_name();

    let ar = mach_port_allocate(mach_server.task, MACH_PORT_RIGHT_RECEIVE, &mut mach_server.port);
    if ar != KERN_SUCCESS {
        error!("mach_server_begin: mach_port_allocate failed (kr={})", ar);
        return false;
    }

    let ir = mach_port_insert_right(
        mach_server.task,
        mach_server.port,
        mach_server.port,
        MACH_MSG_TYPE_MAKE_SEND as c_int,
    );
    if ir != KERN_SUCCESS {
        error!(
            "mach_server_begin: mach_port_insert_right (MAKE_SEND) failed (kr={})",
            ir
        );
        return false;
    }

    let rr = bootstrap_register(mach_server.bs_port, service_name.as_ptr(), mach_server.port);
    if rr != KERN_SUCCESS {
        match rr {
            BOOTSTRAP_NAME_IN_USE => error!(
                "mach_server_begin: bootstrap_register: name in use: {}.",
                service_name.to_string_lossy()
            ),
            BOOTSTRAP_NOT_PRIVILEGED => error!(
                "mach_server_begin: bootstrap_register: not privileged for domain (kr={}).",
                rr
            ),
            _ => error!(
                "mach_server_begin: bootstrap_register failed (kr={}) for {}",
                rr,
                service_name.to_string_lossy()
            ),
        }
        return false;
    }

    let limits = mach_port_limits {
        mpl_qlimit: MACH_PORT_QLIMIT_LARGE,
    };
    let _ = mach_port_set_attributes(
        mach_server.task,
        mach_server.port,
        MACH_PORT_LIMITS_INFO,
        &limits as *const _ as *const c_void,
        MACH_PORT_LIMITS_INFO_COUNT,
    );

    mach_server.handler = Some(handler);
    mach_server.context = context;
    mach_server.is_running = true;

    let cf_context = CFMachPortContext {
        version: 0,
        info: mach_server as *mut _ as *mut c_void,
        retain: None,
        release: None,
        copyDescription: None,
    };
    let cf_mach_port = CFMachPortCreateWithPort(
        null(),
        mach_server.port,
        Some(mach_message_callback),
        &cf_context,
        0,
    );
    if cf_mach_port.is_null() {
        error!(
            "mach_server_begin: CFMachPortCreateWithPort returned null (port={})",
            mach_server.port
        );
        return false;
    }
    let source = CFMachPortCreateRunLoopSource(null(), cf_mach_port, 0);
    if source.is_null() {
        error!("mach_server_begin: CFMachPortCreateRunLoopSource returned null");
        CFRelease(cf_mach_port);
        return false;
    }
    CFRunLoopAddSource(CFRunLoopGetMain(), source, kCFRunLoopDefaultMode);
    CFRelease(source);
    CFRelease(cf_mach_port);

    info!(
        "mach_server_begin: registered '{}' in current bootstrap domain (port={}, bs_port={})",
        bs_name().to_string_lossy(),
        mach_server.port,
        mach_server.bs_port
    );

    true
}

pub unsafe fn send_mach_reply(
    original_msg: *mut mach_msg_header_t,
    response_data: *const c_char,
    response_len: u32,
) -> bool {
    if original_msg.is_null() || response_data.is_null() || response_len > MAX_MESSAGE_SIZE {
        error!(
            "send_mach_reply: invalid args original_msg={:?} response_data={:?} response_len={}",
            original_msg, response_data, response_len
        );
        return false;
    }

    let task = mach_task_self();
    let mut remote_port_type: u32 = 0;
    let mut local_port_type: u32 = 0;

    if (*original_msg).msgh_remote_port != 0 {
        let _ = mach_port_type(task, (*original_msg).msgh_remote_port, &mut remote_port_type);
    }
    if (*original_msg).msgh_local_port != 0 {
        let _ = mach_port_type(task, (*original_msg).msgh_local_port, &mut local_port_type);
    }

    let reply_port = (*original_msg).msgh_remote_port;
    let reply_descriptor = MACH_MSG_TYPE_COPY_SEND;
    if reply_port == 0 {
        error!(
            "send_mach_reply: no available send right (remote_port_type={} local_port_type={} remote_port={} local_port={})",
            remote_port_type,
            local_port_type,
            (*original_msg).msgh_remote_port,
            (*original_msg).msgh_local_port
        );
        return false;
    };

    let mut reply: simple_message = zeroed();

    let aligned_len = (response_len + 3) & !3;
    let total_size = (size_of::<mach_msg_header_t>() as u32) + aligned_len;

    reply.header.msgh_bits = MACH_MSGH_BITS(reply_descriptor as u32, 0);
    reply.header.msgh_size = total_size;
    reply.header.msgh_remote_port = reply_port;
    reply.header.msgh_local_port = 0;
    reply.header.msgh_voucher_port = 0;
    reply.header.msgh_id = (*original_msg).msgh_id;

    copy_nonoverlapping(
        response_data as *const u8,
        reply.data.as_mut_ptr() as *mut u8,
        response_len as usize,
    );
    if aligned_len > response_len {
        let pad = (aligned_len - response_len) as usize;
        let dst = reply.data.as_mut_ptr().add(response_len as usize);
        core::ptr::write_bytes(dst, 0, pad);
    }

    let result = mach_msg(
        &mut reply.header,
        MACH_SEND_MSG,
        reply.header.msgh_size,
        0,
        0,
        MACH_MSG_TIMEOUT_NONE,
        0,
    );

    if result != MACH_MSG_SUCCESS {
        let mut _port_type: u32 = 0;
        let _ = mach_port_type(task, reply_port, &mut _port_type);
        error!(
            "send_mach_reply: mach_msg failed result={} reply_port={} port_type={} descriptor={} remote_port_type={} local_port_type={} original_remote={} original_local={}",
            result,
            reply_port,
            _port_type,
            reply_descriptor,
            remote_port_type,
            local_port_type,
            MACH_MSGH_BITS_REMOTE((*original_msg).msgh_bits),
            MACH_MSGH_BITS_LOCAL((*original_msg).msgh_bits)
        );
        return false;
    }

    true
}

#[allow(static_mut_refs)]
pub unsafe fn mach_server_run(context: *mut c_void, handler: mach_handler) -> bool {
    static mut SERVER: mach_server = mach_server {
        is_running: false,
        task: 0,
        port: 0,
        bs_port: 0,
        handler: None,
        context: null_mut(),
    };

    debug!(
        "mach_server_run: initial state task={} port={} bs_port={} handler_set={} context_ptr={:?}",
        SERVER.task,
        SERVER.port,
        SERVER.bs_port,
        SERVER.handler.is_some(),
        SERVER.context
    );

    if !mach_server_begin(&mut SERVER, context, handler) {
        error!("mach_server_run: mach_server_begin failed, aborting run loop");
        return false;
    }

    debug!(
        "mach_server_run: ports ready (task={}, port={}, bs_port={})",
        SERVER.task, SERVER.port, SERVER.bs_port
    );
    CFRunLoopRun();
    true
}
