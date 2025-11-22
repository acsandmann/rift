#![allow(non_camel_case_types)]

// based on https://github.com/koekeishiya/yabai/commit/6f9006dd957100ec13096d187a8865e85a164a9b#r148091577
// seems like macOS Sequoia does not send destroyed events from windows that are before the process is created

// https://github.com/asmagill/hs._asm.undocumented.spaces/blob/0b5321fc336f75488fb4bbb524677bb8291050bd/CGSConnection.h#L153
// https://github.com/NUIKit/CGSInternal/blob/c4f6f559d624dc1cfc2bf24c8c19dbf653317fcf/CGSEvent.h#L21

use std::ffi::c_void;

use once_cell::sync::Lazy;
use parking_lot::Mutex;
use tracing::{debug, trace};

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

static EVENT_CHANNELS: Lazy<
    Mutex<HashMap<CGSEventType, (actor::Sender<EventData>, Option<actor::Receiver<EventData>>)>>,
> = Lazy::new(|| Mutex::new(HashMap::default()));

static G_CONNECTION: Lazy<cid_t> = Lazy::new(|| unsafe { SLSMainConnectionID() });

static REGISTERED_EVENTS: Lazy<Mutex<HashSet<CGSEventType>>> =
    Lazy::new(|| Mutex::new(HashSet::default()));

pub fn init(event: CGSEventType) -> i32 {
    {
        let mut registered = REGISTERED_EVENTS.lock();
        if registered.contains(&event) {
            debug!("Event {} already registered, skipping", event);
            return 1;
        }

        {
            let mut channels = EVENT_CHANNELS.lock();
            if !channels.contains_key(&event) {
                let (tx, rx) = actor::channel::<EventData>();
                channels.insert(event, (tx, Some(rx)));
            }
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
        debug!("registered {} (raw={}) callback, res={}", event, raw, res);

        if res == 0 {
            registered.insert(event);
        } else {
            debug!("Failed to register event {} (raw={}), res={}", event, raw, res);
        }
        return res;
    }
}

pub fn take_receiver(event: CGSEventType) -> actor::Receiver<EventData> {
    let mut channels = EVENT_CHANNELS.lock();
    let (_tx, rx_opt) = channels.get_mut(&event).unwrap_or_else(|| {
        panic!(
            "window_notify::take_receiver({}) called for unregistered event",
            event
        )
    });

    rx_opt
        .take()
        .unwrap_or_else(|| panic!("window_notify::take_receiver({}) called more than once", event))
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

extern "C" fn connection_callback(
    event_raw: u32,
    data: *mut c_void,
    len: usize,
    _context: *mut c_void,
    _cid: cid_t,
) {
    let kind = CGSEventType::from(event_raw);

    let payload = unsafe {
        if data.is_null() || len == 0 {
            None
        } else {
            Some(std::slice::from_raw_parts(data as *const u8, len).to_vec())
        }
    };

    let mut window_id = None;
    let mut space_id = None;

    if let Some(bytes) = payload.as_deref() {
        match kind {
            CGSEventType::Known(KnownCGSEvent::SpaceWindowDestroyed)
            | CGSEventType::Known(KnownCGSEvent::SpaceWindowCreated) => {
                if bytes.len() >= std::mem::size_of::<u64>() + std::mem::size_of::<u32>() {
                    let mut sid_bytes = [0u8; std::mem::size_of::<u64>()];
                    sid_bytes.copy_from_slice(&bytes[..std::mem::size_of::<u64>()]);
                    let sid = u64::from_ne_bytes(sid_bytes);

                    let mut wid_bytes = [0u8; std::mem::size_of::<u32>()];
                    wid_bytes.copy_from_slice(
                        &bytes[std::mem::size_of::<u64>()
                            ..std::mem::size_of::<u64>() + std::mem::size_of::<u32>()],
                    );
                    let wid = u32::from_ne_bytes(wid_bytes);

                    space_id = Some(sid);
                    window_id = Some(wid);
                } else {
                    debug!(
                        "Skylight event {} payload too short for space/window ids (len={})",
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
                if bytes.len() >= std::mem::size_of::<u32>() {
                    let mut wid_bytes = [0u8; std::mem::size_of::<u32>()];
                    wid_bytes.copy_from_slice(&bytes[..std::mem::size_of::<u32>()]);
                    let wid = u32::from_ne_bytes(wid_bytes);
                    window_id = Some(wid);
                } else {
                    debug!(
                        "Skylight event {} payload too short for window id (len={})",
                        kind, len
                    );
                }
            }
            _ => {}
        }
    }

    let event_data = EventData {
        event_type: kind,
        window_id,
        space_id,
        payload,
        len,
    };

    trace!("received raw event: {:?}", event_data);

    let channels = EVENT_CHANNELS.lock();
    if let Some((sender, _)) = channels.get(&kind) {
        if let Err(e) = sender.try_send(event_data.clone()) {
            debug!("Failed to send event {}: {}", kind, e);
        } else {
            trace!("Dispatched event {}: {:?}", kind, event_data);
        }
    } else {
        trace!("No channel registered for event {}.", kind);
    }
}
