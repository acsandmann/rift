#![allow(non_camel_case_types)]

// based on https://github.com/koekeishiya/yabai/commit/6f9006dd957100ec13096d187a8865e85a164a9b#r148091577
// seems like macOS Sequoia does not send destroyed events from windows that are before the process is created

// https://github.com/asmagill/hs._asm.undocumented.spaces/blob/0b5321fc336f75488fb4bbb524677bb8291050bd/CGSConnection.h#L153
// https://github.com/NUIKit/CGSInternal/blob/c4f6f559d624dc1cfc2bf24c8c19dbf653317fcf/CGSEvent.h#L21

use std::ffi::c_void;

use once_cell::sync::Lazy;
use parking_lot::{Mutex, RwLock};
use tracing::{debug, trace, warn};

use super::skylight::{
    CGSEventType, SLSMainConnectionID, SLSRegisterConnectionNotifyProc,
    SLSRequestNotificationsForWindows, cid_t,
};
use crate::actor;
use crate::common::collections::{HashMap, HashSet};
use crate::sys::skylight::KnownCGSEvent;

type Wid = u32;
type Sid = u64;

#[derive(Debug, Clone)]
pub struct EventData {
    pub event_type: CGSEventType,
    pub window_id: Option<Wid>,
    pub space_id: Option<Sid>,
    pub payload: Option<Vec<u8>>,
    pub len: usize,
}

static EVENT_SENDERS: Lazy<RwLock<HashMap<CGSEventType, actor::Sender<EventData>>>> =
    Lazy::new(|| RwLock::new(HashMap::default()));

static EVENT_RECEIVERS: Lazy<Mutex<HashMap<CGSEventType, Option<actor::Receiver<EventData>>>>> =
    Lazy::new(|| Mutex::new(HashMap::default()));

static G_CONNECTION: Lazy<cid_t> = Lazy::new(|| unsafe { SLSMainConnectionID() });

static REGISTERED_EVENTS: Lazy<Mutex<HashSet<CGSEventType>>> =
    Lazy::new(|| Mutex::new(HashSet::default()));

pub fn init(event: CGSEventType) -> i32 {
    if REGISTERED_EVENTS.lock().contains(&event) {
        debug!("Event {} already registered, skipping", event);
        return 1;
    }

    let mut senders = EVENT_SENDERS.write();
    if !senders.contains_key(&event) {
        let (tx, rx) = actor::channel::<EventData>();
        senders.insert(event, tx);

        let mut receivers = EVENT_RECEIVERS.lock();
        receivers.insert(event, Some(rx));
    }

    let raw: u32 = event.into();
    let res = unsafe {
        SLSRegisterConnectionNotifyProc(
            *G_CONNECTION,
            connection_callback,
            raw,
            std::ptr::null_mut(),
        )
    };

    if res == 0 {
        let mut registered = REGISTERED_EVENTS.lock();
        registered.insert(event);
        debug!("registered {} (raw={}) callback, res={}", event, raw, res);
    } else {
        warn!("failed to register event {} (raw={}), res={}", event, raw, res);
    }

    res
}

pub fn take_receiver(event: CGSEventType) -> actor::Receiver<EventData> {
    if let Some(rx) = EVENT_RECEIVERS.lock().get_mut(&event)
        && let Some(rxo) = rx.take()
    {
        rxo
    } else {
        panic!("window_notify::take_receiver({}) failed", event)
    }
}

pub fn update_window_notifications(window_ids: &[u32]) {
    unsafe {
        let _ = SLSRequestNotificationsForWindows(
            *G_CONNECTION,
            window_ids.as_ptr(),
            window_ids.len() as i32,
        );
    }
}

#[inline(always)]
fn read<T: Copy + Sized + 'static>(bytes: &[u8], off: usize) -> Option<T> {
    let n = std::mem::size_of::<T>();
    if bytes.len() < off + n {
        return None;
    }
    let mut buf = [0u8; 32];
    assert!(n <= buf.len(), "read_ne: type too large");
    buf[..n].copy_from_slice(&bytes[off..off + n]);
    Some(unsafe { std::ptr::read_unaligned(buf.as_ptr() as *const T) })
}

extern "C" fn connection_callback(
    event_raw: u32,
    data: *mut c_void,
    len: usize,
    _context: *mut c_void,
    _cid: cid_t,
) {
    let kind = CGSEventType::from(event_raw);

    let sender = {
        let senders = EVENT_SENDERS.read();
        senders.get(&kind).cloned()
    };
    let Some(sender) = sender else {
        return;
    };

    let bytes = if data.is_null() || len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(data as *const u8, len) }
    };

    let mut window_id = None;
    let mut space_id = None;

    match kind {
        CGSEventType::Known(KnownCGSEvent::SpaceWindowDestroyed)
        | CGSEventType::Known(KnownCGSEvent::SpaceWindowCreated) => {
            if let Some(sid) = read::<u64>(bytes, 0) {
                if let Some(wid) = read::<u32>(bytes, std::mem::size_of::<u64>()) {
                    space_id = Some(sid);
                    window_id = Some(wid);
                } else {
                    warn!(
                        "Skylight event {} payload too short for window id (len={})",
                        kind, len
                    );
                }
            } else {
                warn!(
                    "Skylight event {} payload too short for space id (len={})",
                    kind, len
                );
            }
        }

        CGSEventType::Known(KnownCGSEvent::WindowClosed)
        | CGSEventType::Known(KnownCGSEvent::WindowMoved)
        | CGSEventType::Known(KnownCGSEvent::WindowResized)
        | CGSEventType::Known(KnownCGSEvent::WindowReordered)
        | CGSEventType::Known(KnownCGSEvent::WindowLevelChanged)
        | CGSEventType::Known(KnownCGSEvent::WindowUnhidden)
        | CGSEventType::Known(KnownCGSEvent::WindowHidden) => {
            if let Some(wid) = read::<u32>(bytes, 0) {
                window_id = Some(wid);
            } else {
                warn!(
                    "Skylight event {} payload too short for window id (len={})",
                    kind, len
                );
            }
        }

        _ => {}
    }

    let payload = if bytes.is_empty() {
        None
    } else {
        Some(bytes.to_vec())
    };

    let event_data = EventData {
        event_type: kind,
        window_id,
        space_id,
        payload,
        len,
    };

    trace!("received raw event: {:?}", event_data);

    if let Err(e) = sender.try_send(event_data) {
        debug!("Failed to send event {}: {}", kind, e);
    }
}
