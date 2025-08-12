// this is a mess and needs a heavy cleanup

#![allow(non_camel_case_types)]
#![allow(dead_code)]
#![allow(improper_ctypes)]
#![allow(unsafe_op_in_unsafe_fn)]
#![allow(non_snake_case)]
#![allow(clippy::missing_safety_doc)]

use core::mem::{size_of, zeroed};
use core::ptr::{copy_nonoverlapping, null, null_mut, write_bytes};
use std::alloc::alloc;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};

const MAX_MESSAGE_SIZE: u32 = 16_384;
const MACH_BS_NAME_FMT_PREFIX: &str = "com.";
static G_NAME: &str = "acsandmann.rift";

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

const MACH_MSG_TIMEOUT_NONE: u32 = 0;

const MACH_MSG_TYPE_MAKE_SEND: u32 = 20;
const MACH_MSG_TYPE_COPY_SEND: u32 = 19;

const MACH_PORT_RIGHT_RECEIVE: c_int = 1;
const MACH_PORT_LIMITS_INFO: c_int = 1;
const MACH_PORT_LIMITS_INFO_COUNT: u32 = 1;
const MACH_PORT_QLIMIT_LARGE: u32 = 1024;

const TASK_BOOTSTRAP_PORT: c_int = 4;

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
        shouldFreeInfo: bool,
    ) -> CFMachPortRef;

    fn CFMachPortCreateRunLoopSource(
        allocator: CFAllocatorRef,
        port: CFMachPortRef,
        order: c_int,
    ) -> CFRunLoopSourceRef;

    fn CFRunLoopAddSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);
    fn CFRunLoopGetMain() -> CFRunLoopRef;
    fn CFRunLoopRun();

    fn CFRelease(obj: *const c_void);

    static kCFRunLoopDefaultMode: CFStringRef;
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct mach_msg_header_t {
    msgh_bits: mach_msg_bits_t,
    msgh_size: mach_msg_size_t,
    pub msgh_remote_port: mach_port_t,
    msgh_local_port: mach_port_t,
    msgh_voucher_port: mach_port_name_t,
    msgh_id: mach_msg_id_t,
}

#[repr(C)]
struct mach_port_limits {
    mpl_qlimit: u32,
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
}

#[repr(C)]
struct simple_message {
    header: mach_msg_header_t,
    data: [u8; MAX_MESSAGE_SIZE as usize],
}

unsafe fn mach_get_bs_port(bs_name: &CStr) -> mach_port_t {
    let mut bs_port: mach_port_t = 0;
    if task_get_special_port(mach_task_self(), TASK_BOOTSTRAP_PORT, &mut bs_port) != KERN_SUCCESS {
        return 0;
    }

    let mut service_port: mach_port_t = 0;
    let result = bootstrap_look_up(bs_port, bs_name.as_ptr(), &mut service_port);
    if result != KERN_SUCCESS {
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
        return null_mut();
    }

    let mut reply_port: mach_port_t = 0;
    let task = mach_task_self();

    if await_response {
        if mach_port_allocate(task, MACH_PORT_RIGHT_RECEIVE, &mut reply_port) != KERN_SUCCESS {
            return null_mut();
        }
        if mach_port_insert_right(task, reply_port, reply_port, MACH_MSG_TYPE_MAKE_SEND as c_int)
            != KERN_SUCCESS
        {
            let _ = mach_port_mod_refs(task, reply_port, MACH_PORT_RIGHT_RECEIVE, -1);
            return null_mut();
        }
    }

    let mut msg: simple_message = zeroed();

    let mut aligned_len = (len + 3) & !3;
    let mut total_size = (size_of::<mach_msg_header_t>() as u32) + aligned_len;

    if total_size < 64 {
        total_size = 64;
        aligned_len = total_size - (size_of::<mach_msg_header_t>() as u32);
    }

    msg.header.msgh_remote_port = port;
    msg.header.msgh_local_port = reply_port;
    msg.header.msgh_size = total_size;
    msg.header.msgh_id = 1234;
    msg.header.msgh_voucher_port = 0;

    if await_response {
        msg.header.msgh_bits = MACH_MSGH_BITS(MACH_MSG_TYPE_COPY_SEND, MACH_MSG_TYPE_MAKE_SEND);
    } else {
        msg.header.msgh_bits = MACH_MSGH_BITS(MACH_MSG_TYPE_COPY_SEND, 0);
    }

    copy_nonoverlapping(
        message as *const u8,
        msg.data.as_mut_ptr() as *mut u8,
        len as usize,
    );
    if aligned_len > len {
        let pad = (aligned_len - len) as usize;
        write_bytes(msg.data.as_mut_ptr().add(len as usize) as *mut c_void, 0, pad);
    }

    let send_result = mach_msg(
        &mut msg.header,
        MACH_SEND_MSG,
        msg.header.msgh_size,
        0,
        0,
        MACH_MSG_TIMEOUT_NONE,
        0,
    );

    if send_result != MACH_MSG_SUCCESS {
        if await_response && reply_port != 0 {
            let _ = mach_port_mod_refs(task, reply_port, MACH_PORT_RIGHT_RECEIVE, -1);
            let _ = mach_port_deallocate(task, reply_port);
        }
        return null_mut();
    }

    if await_response {
        let mut response: simple_message = zeroed();
        let recv_result = mach_msg(
            &mut response.header,
            MACH_RCV_MSG,
            0,
            size_of::<simple_message>() as u32,
            reply_port,
            MACH_MSG_TIMEOUT_NONE,
            0,
        );

        if recv_result != MACH_MSG_SUCCESS {
            let _ = mach_port_mod_refs(task, reply_port, MACH_PORT_RIGHT_RECEIVE, -1);
            let _ = mach_port_deallocate(task, reply_port);
            return null_mut();
        }

        let response_len = response.header.msgh_size - (size_of::<mach_msg_header_t>() as u32);

        let layout = std::alloc::Layout::array::<c_char>((response_len as usize) + 1).unwrap();
        let buf = alloc(layout) as *mut c_char;
        if buf.is_null() {
            let _ = mach_port_mod_refs(task, reply_port, MACH_PORT_RIGHT_RECEIVE, -1);
            let _ = mach_port_deallocate(task, reply_port);
            return null_mut();
        }

        // Copy response payload from Mach message buffer into a new C string buffer
        copy_nonoverlapping(
            response.data.as_ptr() as *const u8,
            buf as *mut u8,
            response_len as usize,
        );
        *buf.add(response_len as usize) = 0;

        let _ = mach_port_mod_refs(task, reply_port, MACH_PORT_RIGHT_RECEIVE, -1);
        let _ = mach_port_deallocate(task, reply_port);

        return buf;
    }

    null_mut()
}

pub unsafe fn mach_send_request(message: *const c_char, len: u32) -> *mut c_char {
    if message.is_null() || len > MAX_MESSAGE_SIZE {
        return null_mut();
    }

    let bs_name = CString::new(format!("{}{}", MACH_BS_NAME_FMT_PREFIX, G_NAME)).unwrap();
    let service_port = mach_get_bs_port(&bs_name);
    if service_port == 0 {
        return null_mut();
    }

    mach_send_message(service_port, message, len, true)
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
        let msg = &mut *(message as *mut simple_message);

        let padded_data_len =
            msg.header.msgh_size.saturating_sub(size_of::<mach_msg_header_t>() as u32);

        let mut actual_data_len = 0u32;
        for i in 0..padded_data_len {
            if msg.data[i as usize] == 0 {
                actual_data_len = i;
                break;
            }
        }
        if actual_data_len == 0 {
            actual_data_len = padded_data_len;
        }

        if let Some(handler) = mach_server.handler {
            handler(
                mach_server.context,
                msg.data.as_mut_ptr() as *mut c_char,
                actual_data_len,
                &mut msg.header as *mut mach_msg_header_t,
            );
        }
    }
}

pub unsafe fn mach_server_begin(
    mach_server: &mut mach_server,
    context: *mut c_void,
    handler: mach_handler,
) -> bool {
    mach_server.task = mach_task_self();

    if mach_port_allocate(mach_server.task, MACH_PORT_RIGHT_RECEIVE, &mut mach_server.port)
        != KERN_SUCCESS
    {
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

    if mach_port_insert_right(
        mach_server.task,
        mach_server.port,
        mach_server.port,
        MACH_MSG_TYPE_MAKE_SEND as c_int,
    ) != KERN_SUCCESS
    {
        return false;
    }

    if task_get_special_port(mach_server.task, TASK_BOOTSTRAP_PORT, &mut mach_server.bs_port)
        != KERN_SUCCESS
    {
        return false;
    }

    let bs_name = CString::new(format!("{}{}", MACH_BS_NAME_FMT_PREFIX, G_NAME)).unwrap();

    // If it exists, check-in to take ownership
    let mut existing_port: mach_port_t = 0;
    if bootstrap_look_up(mach_server.bs_port, bs_name.as_ptr(), &mut existing_port) == KERN_SUCCESS
    {
        let _ = bootstrap_check_in(mach_server.bs_port, bs_name.as_ptr(), &mut mach_server.port);
    }

    let kr = bootstrap_check_in(mach_server.bs_port, bs_name.as_ptr(), &mut mach_server.port);
    if kr != KERN_SUCCESS {
        return false;
    }

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
        false,
    );
    if cf_mach_port.is_null() {
        return false;
    }

    let source = CFMachPortCreateRunLoopSource(null(), cf_mach_port, 0);
    if source.is_null() {
        CFRelease(cf_mach_port);
        return false;
    }

    CFRunLoopAddSource(CFRunLoopGetMain(), source, unsafe { kCFRunLoopDefaultMode });
    CFRelease(source);
    CFRelease(cf_mach_port);

    true
}

pub unsafe fn send_mach_reply(
    original_msg: *mut mach_msg_header_t,
    response_data: *const c_char,
    response_len: u32,
) -> bool {
    if original_msg.is_null() || response_data.is_null() || response_len > MAX_MESSAGE_SIZE {
        return false;
    }

    let original = &*original_msg;

    let reply_port: mach_port_t;

    if original.msgh_remote_port != 0 {
        reply_port = original.msgh_remote_port;
    } else if original.msgh_local_port != 0 {
        reply_port = original.msgh_local_port;
    } else {
        return false;
    }

    let mut reply: simple_message = zeroed();

    let mut aligned_len = (response_len + 3) & !3;
    let mut total_size = (size_of::<mach_msg_header_t>() as u32) + aligned_len;

    if total_size < 64 {
        total_size = 64;
        aligned_len = total_size - (size_of::<mach_msg_header_t>() as u32);
    }

    reply.header.msgh_bits = MACH_MSGH_BITS(MACH_MSG_TYPE_COPY_SEND, 0);
    reply.header.msgh_size = total_size;
    reply.header.msgh_remote_port = reply_port;
    reply.header.msgh_local_port = 0;
    reply.header.msgh_voucher_port = 0;
    reply.header.msgh_id = original.msgh_id;

    // Copy response bytes into the reply message buffer
    copy_nonoverlapping(
        response_data as *const u8,
        reply.data.as_mut_ptr() as *mut u8,
        response_len as usize,
    );
    if aligned_len > response_len {
        let pad = (aligned_len - response_len) as usize;
        write_bytes(
            reply.data.as_mut_ptr().add(response_len as usize) as *mut c_void,
            0,
            pad,
        );
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
        let mut port_type: u32 = 0;
        let _ = mach_port_type(mach_task_self(), reply_port, &mut port_type);
        return false;
    }

    true
}

pub unsafe fn mach_server_run(context: *mut c_void, handler: mach_handler) {
    static mut SERVER: mach_server = mach_server {
        is_running: false,
        task: 0,
        port: 0,
        bs_port: 0,
        handler: None,
        context: null_mut(),
    };

    #[allow(static_mut_refs)]
    if !mach_server_begin(&mut SERVER, context, handler) {
        return;
    }

    CFRunLoopRun();
}
