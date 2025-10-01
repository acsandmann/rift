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
    _len: usize,
    _context: *mut c_void,
    _cid: cid_t,
) {
    let kind = CGSEventType::from(event_raw);

    let event_data: EventData = unsafe {
        if data.is_null() {
            EventData {
                event_type: kind,
                window_id: None,
                space_id: None,
            }
        } else {
            match kind {
                CGSEventType::Known(KnownCGSEvent::WindowDestroyed)
                | CGSEventType::Known(KnownCGSEvent::WindowCreated) => {
                    let sid = std::ptr::read_unaligned(data as *const u64);
                    let wid = std::ptr::read_unaligned(
                        (data as *const u8).add(std::mem::size_of::<u64>()) as *const u32,
                    );
                    EventData {
                        event_type: kind,
                        window_id: Some(wid),
                        space_id: Some(sid),
                    }
                }
                CGSEventType::Known(KnownCGSEvent::MissionControlEntered) => EventData {
                    event_type: kind,
                    window_id: None,
                    space_id: None,
                },
                _ => {
                    // TODO: this isnt really safe
                    let wid = std::ptr::read_unaligned(data as *const u32);
                    EventData {
                        event_type: kind,
                        window_id: Some(wid),
                        space_id: None,
                    }
                }
            }
        }
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
