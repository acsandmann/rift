use std::cell::RefCell;
use std::mem::replace;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use core_foundation::runloop::{CFRunLoop, kCFRunLoopCommonModes};
use core_graphics::event::{
    CGEvent, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
};
use objc2_core_foundation::{CGPoint, CGRect};
use objc2_foundation::{MainThreadMarker, NSInteger};
use tracing::{Span, debug, error, trace, warn};

use super::reactor::{self, Event};
use crate::common::config::Config;
use crate::sys::event;
use crate::sys::geometry::{CGRectExt, ToICrate};
use crate::sys::screen::CoordinateConverter;
use crate::sys::window_server::{self, WindowServerId, get_window};

#[derive(Debug)]
pub enum Request {
    Warp(CGPoint),
    EnforceHidden,
    ScreenParametersChanged(Vec<CGRect>, CoordinateConverter),
    SetEventProcessing(bool),
}

pub struct Mouse {
    config: Arc<Config>,
    events_tx: reactor::Sender,
    requests_rx: Option<Receiver>,
    state: RefCell<State>,
}

struct State {
    hidden: bool,
    above_window: Option<WindowServerId>,
    above_window_level: NSWindowLevel,
    converter: CoordinateConverter,
    screens: Vec<CGRect>,
    event_processing_enabled: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            hidden: false,
            above_window: None,
            above_window_level: NSWindowLevel::MIN,
            converter: CoordinateConverter::default(),
            screens: Vec::new(),
            event_processing_enabled: false,
        }
    }
}

pub type Sender = tokio::sync::mpsc::UnboundedSender<(Span, Request)>;
pub type Receiver = tokio::sync::mpsc::UnboundedReceiver<(Span, Request)>;

pub fn channel() -> (Sender, Receiver) { tokio::sync::mpsc::unbounded_channel() }

impl Mouse {
    pub fn new(config: Arc<Config>, events_tx: reactor::Sender, requests_rx: Receiver) -> Self {
        Mouse {
            config,
            events_tx,
            requests_rx: Some(requests_rx),
            state: RefCell::new(State::default()),
        }
    }

    pub async fn run(mut self) {
        let mut requests_rx = self.requests_rx.take().unwrap();

        let this = Rc::new(self);
        let this_ = Rc::clone(&this);
        let current = CFRunLoop::get_current();
        let mtm = MainThreadMarker::new().unwrap();
        let tap = CGEventTap::new(
            CGEventTapLocation::Session,
            CGEventTapPlacement::HeadInsertEventTap,
            CGEventTapOptions::ListenOnly,
            vec![
                CGEventType::LeftMouseDown,
                CGEventType::LeftMouseUp,
                CGEventType::RightMouseDown,
                CGEventType::RightMouseUp,
                CGEventType::MouseMoved,
                CGEventType::LeftMouseDragged,
                CGEventType::RightMouseDragged,
            ],
            move |_, event_type, event| {
                this_.on_event(event_type, event, mtm);
                None
            },
        )
        .expect("Could not create event tap");

        let loop_source = tap.mach_port().create_runloop_source(0).unwrap();
        current.add_source(&loop_source, unsafe { kCFRunLoopCommonModes });

        tap.enable();

        if this.config.settings.mouse_hides_on_focus {
            if let Err(e) = window_server::allow_hide_mouse() {
                error!(
                    "Could not enable mouse hiding: {e:?}. \
                    mouse_hides_on_focus will have no effect."
                );
            }
        }

        while let Some((_span, request)) = requests_rx.recv().await {
            this.on_request(request);
        }
    }

    fn on_request(self: &Rc<Self>, request: Request) {
        let mut state = self.state.borrow_mut();
        match request {
            Request::Warp(point) => {
                if let Err(e) = event::warp_mouse(point) {
                    warn!("Failed to warp mouse: {e:?}");
                }
                if self.config.settings.mouse_hides_on_focus && !state.hidden {
                    debug!("Hiding mouse");
                    if let Err(e) = event::hide_mouse() {
                        warn!("Failed to hide mouse: {e:?}");
                    }
                    state.hidden = true;
                }
            }
            Request::EnforceHidden => {
                if state.hidden {
                    if let Err(e) = event::hide_mouse() {
                        warn!("Failed to hide mouse: {e:?}");
                    }
                }
            }
            Request::ScreenParametersChanged(frames, converter) => {
                state.screens = frames;
                state.converter = converter;
            }
            Request::SetEventProcessing(enabled) => {
                state.event_processing_enabled = enabled;
            }
        }
    }

    fn on_event(self: &Rc<Self>, event_type: CGEventType, event: &CGEvent, mtm: MainThreadMarker) {
        let mut state = self.state.borrow_mut();

        if !state.event_processing_enabled {
            trace!("Mouse event processing disabled, ignoring {:?}", event_type);
            return;
        }

        if state.hidden {
            debug!("Showing mouse");
            if let Err(e) = event::show_mouse() {
                warn!("Failed to show mouse: {e:?}");
            }
            state.hidden = false;
        }
        match event_type {
            CGEventType::LeftMouseUp => {
                _ = self.events_tx.send((Span::current().clone(), Event::MouseUp));
            }
            CGEventType::MouseMoved if self.config.settings.focus_follows_mouse => {
                let loc = event.location();
                trace!("Mouse moved {loc:?}");
                if let Some(wsid) = state.track_mouse_move(loc.to_icrate(), mtm) {
                    _ = self.events_tx.send((Span::current(), Event::MouseMovedOverWindow(wsid)));
                }
            }
            _ => (),
        }
    }
}

impl State {
    fn track_mouse_move(&mut self, loc: CGPoint, mtm: MainThreadMarker) -> Option<WindowServerId> {
        let new_window = trace_misc("get_window_at_point", || {
            window_server::get_window_at_point(loc, self.converter, mtm)
        });
        if self.above_window == new_window {
            return None;
        }
        debug!("Mouse is now above window {new_window:?} at {loc:?}");

        // There is a gap between the menu bar and the actual menu pop-ups when
        // a menu is opened. When the mouse goes over this gap, the system
        // reports it to be over whatever window happens to be below the menu
        // bar and behind the pop-up. Ignore anything in this gap so we don't
        // dismiss the pop-up. Strangely, it only seems to happen when the mouse
        // travels down from the menu bar and not when it travels back up.
        // First observed on 13.5.2.
        if self.above_window_level == NSMainMenuWindowLevel {
            const WITHIN: f64 = 1.0;
            for screen in &self.screens {
                if screen.contains(CGPoint::new(loc.x, loc.y + WITHIN))
                    && loc.y < screen.min().y + WITHIN
                {
                    return None;
                }
            }
        }

        let old_window = replace(&mut self.above_window, new_window);
        let new_window_level = new_window
            .and_then(|id| trace_misc("get_window", || get_window(id)))
            .map(|info| info.layer as NSWindowLevel)
            .unwrap_or(NSWindowLevel::MIN);
        let old_window_level = replace(&mut self.above_window_level, new_window_level);
        debug!(?old_window, ?old_window_level, ?new_window, ?new_window_level);

        if old_window_level >= NSPopUpMenuWindowLevel {
            return None;
        }

        if !(0..NSPopUpMenuWindowLevel).contains(&new_window_level) {
            return None;
        }

        new_window
    }
}

fn trace_misc<T>(desc: &str, f: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let out = f();
    let end = Instant::now();
    trace!(time = ?(end - start), "{desc}");
    out
}

/// https://developer.apple.com/documentation/appkit/nswindowlevel?language=objc
pub type NSWindowLevel = NSInteger;
#[allow(non_upper_case_globals)]
pub const NSMainMenuWindowLevel: NSWindowLevel = 24;
#[allow(non_upper_case_globals)]
pub const NSPopUpMenuWindowLevel: NSWindowLevel = 101;
