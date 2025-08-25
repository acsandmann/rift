#![allow(non_camel_case_types)]

// based on https://github.com/koekeishiya/yabai/commit/6f9006dd957100ec13096d187a8865e85a164a9b#r148091577
// seems like macOS Sequoia does not send destroyed events from windows that are before the process is created

// https://github.com/asmagill/hs._asm.undocumented.spaces/blob/0b5321fc336f75488fb4bbb524677bb8291050bd/CGSConnection.h#L153
// https://github.com/NUIKit/CGSInternal/blob/c4f6f559d624dc1cfc2bf24c8c19dbf653317fcf/CGSEvent.h#L21

use std::ffi::c_void;
use parking_lot::Mutex;

use once_cell::sync::Lazy;
use tracing::{debug, trace};

use super::skylight::{
    SLSMainConnectionID, SLSRegisterConnectionNotifyProc, SLSRequestNotificationsForWindows, cid_t,
};
use crate::actor;
use crate::common::collections::HashMap;
use crate::sys::skylight::CGSEventType;

type Wid = u32;

#[derive(Debug, Clone)]
pub struct EventData {
    pub event_type: CGSEventType,
    pub window_id: Option<Wid>,
}

static EVENT_CHANNELS: Lazy<
    Mutex<HashMap<CGSEventType, (actor::Sender<EventData>, Option<actor::Receiver<EventData>>)>>,
> = Lazy::new(|| Mutex::new(HashMap::default()));

static G_CONNECTION: Lazy<cid_t> = Lazy::new(|| unsafe { SLSMainConnectionID() });

static REGISTERED_EVENTS: Lazy<Mutex<crate::common::collections::HashSet<CGSEventType>>> =
    Lazy::new(|| Mutex::new(crate::common::collections::HashSet::default()));

pub fn init(event: CGSEventType) -> i32 {
    let event = event.into();
    let mut registered = REGISTERED_EVENTS.lock();
    if registered.contains(&event) {
        debug!("Event {} already registered, skipping", event);
        return 1;
    }

    let mut channels = EVENT_CHANNELS.lock();
    if !channels.contains_key(&event) {
        let (tx, rx) = actor::channel::<EventData>();
        channels.insert(event, (tx, Some(rx)));
    }

    unsafe {
        let res = SLSRegisterConnectionNotifyProc(
            *G_CONNECTION,
            connection_callback,
            event,
            std::ptr::null_mut(),
        );
        debug!("registered {} callback, res={}", event, res);

        if res == 0 {
            registered.insert(event);
        } else {
            debug!("Failed to register event {}, res={}", event, res);
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
    event: CGSEventType,
    data: *mut c_void,
    _len: usize,
    _context: *mut c_void,
    _cid: cid_t,
) {
    let event_data = EventData {
        event_type: event,
        window_id: if !data.is_null() {
            Some(unsafe { *(data as *const u32) })
        } else {
            None
        },
    };

    debug!("received: {:?}", event_data);

    let channels = EVENT_CHANNELS.lock();
    if let Some((sender, _)) = channels.get(&event) {
        if let Err(e) = sender.try_send(event_data.clone()) {
            debug!("Failed to send event {}: {}", event, e);
        } else {
            trace!(
                "Sent event {} (callback event {}): {:?}",
                event, event, event_data
            );
        }
    } else {
        trace!("No channel registered for event {}.", event);
    }
}
