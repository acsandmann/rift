//! Gesture handling via a dedicated ListenOnly CGEventTap.
//!
//! This actor runs on the main thread and handles trackpad swipe/scroll
//! gestures for workspace switching. It uses a ListenOnly tap which is more
//! resilient than a Default tap (macOS never disables listen-only taps via
//! timeout).

use std::cell::RefCell;
use std::panic::AssertUnwindSafe;
use std::rc::Rc;

use objc2::exception;
use objc2_app_kit::{NSEvent, NSEventPhase, NSEventType, NSTouchPhase, NSTouchType};
use objc2_core_foundation::{CGPoint, CGRect};
use objc2_core_graphics::{
    CGEvent, CGEventMask, CGEventTapProxy, CGEventType,
};
use tracing::trace;

use crate::actor;
use crate::actor::wm_controller::{self, WmCommand, WmEvent};
use crate::actor::reactor;
use crate::common::collections::HashMap;
use crate::common::config::{Config, HapticPattern, LayoutMode};
use crate::layout_engine::LayoutCommand as LC;
use crate::sys::screen::SpaceId;
use crate::sys::{haptics};

#[derive(Debug)]
pub enum GestureRequest {
    ConfigUpdated(Config),
    LayoutModesChanged(Vec<(SpaceId, LayoutMode)>),
    ScreenParametersChanged(Vec<(CGRect, Option<SpaceId>)>),
    SpaceChanged(Vec<Option<SpaceId>>),
}

pub type Sender = actor::Sender<GestureRequest>;
pub type Receiver = actor::Receiver<GestureRequest>;

pub struct GestureTap {
    config: RefCell<Config>,
    wm_sender: wm_controller::Sender,
    swipe: RefCell<Option<SwipeHandler>>,
    scroll: RefCell<Option<ScrollHandler>>,
    tap: RefCell<Option<crate::sys::event_tap::EventTap>>,
    screen_spaces: RefCell<Vec<(CGRect, SpaceId)>>,
    layout_mode_by_space: RefCell<HashMap<SpaceId, LayoutMode>>,
    default_layout_mode: RefCell<LayoutMode>,
    requests_rx: Option<Receiver>,
}

#[derive(Debug, Clone)]
struct SwipeConfig {
    enabled: bool,
    invert_horizontal: bool,
    vertical_tolerance: f64,
    skip_empty_workspaces: Option<bool>,
    fingers: usize,
    distance_pct: f64,
    haptics_enabled: bool,
    haptic_pattern: HapticPattern,
}

impl SwipeConfig {
    fn from_config(config: &Config) -> Self {
        let g = &config.settings.gestures;
        let vt_norm = if g.swipe_vertical_tolerance > 1.0 && g.swipe_vertical_tolerance <= 100.0 {
            (g.swipe_vertical_tolerance / 100.0).clamp(0.0, 1.0)
        } else if g.swipe_vertical_tolerance > 100.0 {
            1.0
        } else {
            g.swipe_vertical_tolerance.max(0.0).min(1.0)
        };
        SwipeConfig {
            enabled: g.enabled,
            invert_horizontal: g.invert_horizontal_swipe,
            vertical_tolerance: vt_norm,
            skip_empty_workspaces: if g.skip_empty { Some(true) } else { None },
            fingers: g.fingers.max(1),
            distance_pct: g.distance_pct.clamp(0.01, 1.0),
            haptics_enabled: g.haptics_enabled,
            haptic_pattern: g.haptic_pattern,
        }
    }
}

#[derive(Default, Debug)]
struct SwipeState {
    phase: GesturePhase,
    start_x: f64,
    start_y: f64,
}

impl SwipeState {
    fn reset(&mut self) {
        self.phase = GesturePhase::Idle;
        self.start_x = 0.0;
        self.start_y = 0.0;
    }
}

#[derive(Default, Debug, Copy, Clone, Eq, PartialEq)]
enum GesturePhase {
    #[default]
    Idle,
    Armed,
    Committed,
}

struct SwipeHandler {
    cfg: SwipeConfig,
    state: RefCell<SwipeState>,
}

#[derive(Debug, Clone)]
struct ScrollConfig {
    enabled: bool,
    invert_horizontal: bool,
    vertical_tolerance: f64,
    fingers: usize,
    distance_pct: f64,
}

impl ScrollConfig {
    fn from_config(config: &Config) -> Self {
        let g = &config.settings.layout.scrolling.gestures;
        let vt_norm = if g.vertical_tolerance > 1.0 && g.vertical_tolerance <= 100.0 {
            (g.vertical_tolerance / 100.0).clamp(0.0, 1.0)
        } else if g.vertical_tolerance > 100.0 {
            1.0
        } else {
            g.vertical_tolerance.max(0.0).min(1.0)
        };
        ScrollConfig {
            enabled: g.enabled,
            invert_horizontal: g.invert_horizontal,
            vertical_tolerance: vt_norm,
            fingers: g.fingers.max(1),
            distance_pct: g.distance_pct.clamp(0.01, 1.0),
        }
    }
}

#[derive(Default, Debug)]
struct ScrollState {
    phase: GesturePhase,
    start_x: f64,
    start_y: f64,
    last_x: f64,
    last_y: f64,
    accum_dx: f64,
}

impl ScrollState {
    fn reset(&mut self) {
        self.phase = GesturePhase::Idle;
        self.start_x = 0.0;
        self.start_y = 0.0;
        self.last_x = 0.0;
        self.last_y = 0.0;
        self.accum_dx = 0.0;
    }
}

struct ScrollHandler {
    cfg: ScrollConfig,
    state: RefCell<ScrollState>,
}

struct CallbackCtx {
    this: Rc<GestureTap>,
}

unsafe fn drop_gesture_ctx(ptr: *mut std::ffi::c_void) {
    unsafe { drop(Box::from_raw(ptr as *mut CallbackCtx)) };
}

impl GestureTap {
    pub fn new(
        config: Config,
        wm_sender: wm_controller::Sender,
        requests_rx: Receiver,
    ) -> Self {
        let default_layout_mode = config.settings.layout.mode;
        let (swipe, scroll) = Self::build_gesture_handlers(&config);
        GestureTap {
            config: RefCell::new(config),
            wm_sender,
            swipe: RefCell::new(swipe),
            scroll: RefCell::new(scroll),
            tap: RefCell::new(None),
            screen_spaces: RefCell::new(Vec::new()),
            layout_mode_by_space: RefCell::new(HashMap::default()),
            default_layout_mode: RefCell::new(default_layout_mode),
            requests_rx: Some(requests_rx),
        }
    }

    pub async fn run(mut self) {
        let mut requests_rx = self.requests_rx.take().unwrap();

        let this = Rc::new(self);

        if this.gesture_handlers_enabled() {
            this.create_and_install_tap();
        }

        while let Some((span, request)) = requests_rx.recv().await {
            let _guard = span.enter();
            this.on_request(request);
        }
    }

    fn on_request(self: &Rc<Self>, request: GestureRequest) {
        match request {
            GestureRequest::ConfigUpdated(new_config) => {
                *self.default_layout_mode.borrow_mut() = new_config.settings.layout.mode;
                *self.config.borrow_mut() = new_config;
                self.update_gesture_handlers();
            }
            GestureRequest::LayoutModesChanged(modes) => {
                let mut map = self.layout_mode_by_space.borrow_mut();
                map.clear();
                for (space, mode) in modes {
                    map.insert(space, mode);
                }
            }
            GestureRequest::ScreenParametersChanged(screens_with_spaces) => {
                *self.screen_spaces.borrow_mut() = screens_with_spaces
                    .into_iter()
                    .filter_map(|(frame, maybe_space)| maybe_space.map(|space| (frame, space)))
                    .collect();
            }
            GestureRequest::SpaceChanged(spaces) => {
                let screens: Vec<CGRect> = self
                    .screen_spaces
                    .borrow()
                    .iter()
                    .map(|(frame, _)| *frame)
                    .collect();
                *self.screen_spaces.borrow_mut() = screens
                    .into_iter()
                    .zip(spaces.into_iter())
                    .filter_map(|(frame, maybe_space)| maybe_space.map(|space| (frame, space)))
                    .collect();
            }
        }
    }

    fn build_gesture_handlers(config: &Config) -> (Option<SwipeHandler>, Option<ScrollHandler>) {
        let swipe_cfg = SwipeConfig::from_config(config);
        let swipe = if swipe_cfg.enabled {
            Some(SwipeHandler {
                cfg: swipe_cfg,
                state: RefCell::new(SwipeState::default()),
            })
        } else {
            None
        };

        let scroll_cfg = ScrollConfig::from_config(config);
        let scroll = if scroll_cfg.enabled {
            Some(ScrollHandler {
                cfg: scroll_cfg,
                state: RefCell::new(ScrollState::default()),
            })
        } else {
            None
        };

        (swipe, scroll)
    }

    fn update_gesture_handlers(self: &Rc<Self>) {
        let config = self.config.borrow();
        let (swipe, scroll) = Self::build_gesture_handlers(&config);
        let was_enabled = self.gesture_handlers_enabled();
        *self.swipe.borrow_mut() = swipe;
        *self.scroll.borrow_mut() = scroll;
        let is_enabled = self.gesture_handlers_enabled();

        if !was_enabled && is_enabled {
            self.create_and_install_tap();
        } else if was_enabled && !is_enabled {
            *self.tap.borrow_mut() = None;
        }
    }

    fn gesture_handlers_enabled(&self) -> bool {
        self.swipe.borrow().is_some() || self.scroll.borrow().is_some()
    }

    fn create_and_install_tap(self: &Rc<Self>) {
        let mask = gesture_event_mask();
        let ctx = Box::new(CallbackCtx { this: Rc::clone(self) });
        let ctx_ptr = Box::into_raw(ctx) as *mut std::ffi::c_void;

        let tap = unsafe {
            crate::sys::event_tap::EventTap::new_listen_only(
                mask,
                Some(gesture_callback),
                ctx_ptr,
                Some(drop_gesture_ctx),
            )
        };

        if let Some(tap) = tap {
            *self.tap.borrow_mut() = Some(tap);
        } else {
            unsafe { drop(Box::from_raw(ctx_ptr as *mut CallbackCtx)) };
            tracing::warn!("Failed to create gesture ListenOnly tap");
        }
    }

    fn on_event(self: &Rc<Self>, event_type: CGEventType, event: &CGEvent) {
        if event_type.0 != NSEventType::Gesture.0 as u32 {
            return;
        }

        let scroll_handler = self.scroll.borrow();
        let swipe_handler = self.swipe.borrow();
        if scroll_handler.is_none() && swipe_handler.is_none() {
            return;
        }

        if let Some(nsevent) = NSEvent::eventWithCGEvent(event)
            && nsevent.r#type() == NSEventType::Gesture
        {
            let cursor = CGEvent::location(Some(event));
            let mode = self
                .layout_mode_at_point(cursor)
                .unwrap_or(*self.default_layout_mode.borrow());
            let is_scrolling_mode = matches!(mode, LayoutMode::Scrolling);
            if is_scrolling_mode && let Some(handler) = scroll_handler.as_ref() {
                self.handle_scroll_gesture_event(handler, &nsevent);
            } else if let Some(handler) = swipe_handler.as_ref() {
                self.handle_gesture_event(handler, &nsevent);
            }
        }
    }

    fn layout_mode_at_point(&self, loc: CGPoint) -> Option<LayoutMode> {
        let screen_spaces = self.screen_spaces.borrow();
        let layout_modes = self.layout_mode_by_space.borrow();
        screen_spaces
            .iter()
            .find(|(frame, _)| {
                loc.x >= frame.origin.x
                    && loc.x < frame.origin.x + frame.size.width
                    && loc.y >= frame.origin.y
                    && loc.y < frame.origin.y + frame.size.height
            })
            .and_then(|(_, space)| layout_modes.get(space).copied())
    }

    fn handle_gesture_event(&self, handler: &SwipeHandler, nsevent: &NSEvent) {
        let cfg = &handler.cfg;
        let state = &handler.state;

        let mut st = state.borrow_mut();

        let phase = nsevent.phase();
        if matches!(
            phase,
            NSEventPhase::Ended | NSEventPhase::Cancelled | NSEventPhase::Began
        ) {
            st.reset();
            return;
        }

        let touches = nsevent.allTouches();
        let mut sum_x = 0.0f64;
        let mut sum_y = 0.0f64;
        let mut touch_count = 0usize;
        let mut active_count = 0usize;
        let mut too_many_touches = false;

        for t in touches.iter() {
            let phase = t.phase();
            if phase.contains(NSTouchPhase::Stationary) {
                continue;
            }

            let ended =
                phase.contains(NSTouchPhase::Ended) || phase.contains(NSTouchPhase::Cancelled);

            touch_count += 1;
            if touch_count > cfg.fingers {
                too_many_touches = true;
                break;
            }

            if !ended && let Some((x, y)) = touch_normalized_position(&t) {
                sum_x += x;
                sum_y += y;
                active_count += 1;
            }
        }

        if too_many_touches || touch_count != cfg.fingers || active_count == 0 {
            st.reset();
            return;
        }

        let avg_x = sum_x / active_count as f64;
        let avg_y = sum_y / active_count as f64;

        match st.phase {
            GesturePhase::Idle => {
                st.start_x = avg_x;
                st.start_y = avg_y;
                st.phase = GesturePhase::Armed;
                trace!(
                    "swipe armed: start_x={:.3} start_y={:.3}",
                    st.start_x, st.start_y
                );
            }
            GesturePhase::Armed => {
                let dx = avg_x - st.start_x;
                let dy = avg_y - st.start_y;
                let horizontal = dx.abs();
                let vertical = dy.abs();

                if horizontal >= cfg.distance_pct && vertical <= cfg.vertical_tolerance {
                    let mut dir_left = dx < 0.0;
                    if cfg.invert_horizontal {
                        dir_left = !dir_left;
                    }
                    let cmd = if dir_left {
                        LC::NextWorkspace(cfg.skip_empty_workspaces)
                    } else {
                        LC::PrevWorkspace(cfg.skip_empty_workspaces)
                    };

                    if cfg.haptics_enabled {
                        let _ = haptics::perform_haptic(cfg.haptic_pattern);
                    }
                    self.wm_sender.send(WmEvent::Command(WmCommand::ReactorCommand(
                        reactor::Command::Layout(cmd),
                    )));
                    st.phase = GesturePhase::Committed;
                }
            }
            GesturePhase::Committed => {
                if active_count == 0 {
                    st.reset();
                }
            }
        }
    }

    fn handle_scroll_gesture_event(&self, handler: &ScrollHandler, nsevent: &NSEvent) {
        let cfg = &handler.cfg;
        let state = &handler.state;

        let mut st = state.borrow_mut();

        let phase = nsevent.phase();
        if matches!(
            phase,
            NSEventPhase::Ended | NSEventPhase::Cancelled | NSEventPhase::Began
        ) {
            st.reset();
            return;
        }

        let touches = nsevent.allTouches();
        let mut sum_x = 0.0f64;
        let mut sum_y = 0.0f64;
        let mut touch_count = 0usize;
        let mut active_count = 0usize;
        let mut too_many_touches = false;
        let mut all_moved = true;

        for t in touches.iter() {
            let phase = t.phase();
            if phase.contains(NSTouchPhase::Stationary) {
                all_moved = false;
                continue;
            }

            if !phase.contains(NSTouchPhase::Moved) {
                all_moved = false;
            }

            let ended =
                phase.contains(NSTouchPhase::Ended) || phase.contains(NSTouchPhase::Cancelled);

            touch_count += 1;
            if touch_count > cfg.fingers {
                too_many_touches = true;
                break;
            }

            if !ended && let Some((x, y)) = touch_normalized_position(&t) {
                sum_x += x;
                sum_y += y;
                active_count += 1;
            }
        }

        if too_many_touches || touch_count != cfg.fingers || active_count == 0 {
            st.reset();
            return;
        }

        let avg_x = sum_x / active_count as f64;
        let avg_y = sum_y / active_count as f64;

        match st.phase {
            GesturePhase::Idle => {
                st.start_x = avg_x;
                st.start_y = avg_y;
                st.last_x = avg_x;
                st.last_y = avg_y;
                st.accum_dx = 0.0;
                st.phase = GesturePhase::Armed;
                trace!(
                    "scroll armed: start_x={:.3} start_y={:.3}",
                    st.start_x, st.start_y
                );
            }
            GesturePhase::Armed => {
                if !all_moved {
                    st.last_x = avg_x;
                    st.last_y = avg_y;
                    return;
                }

                let dx = avg_x - st.last_x;
                let dy = avg_y - st.last_y;
                let horizontal = dx.abs();
                let vertical = dy.abs();

                st.last_x = avg_x;
                st.last_y = avg_y;

                if vertical > cfg.vertical_tolerance || vertical >= horizontal {
                    return;
                }

                st.accum_dx += dx;
                let step = cfg.distance_pct;
                if st.accum_dx.abs() >= step {
                    let delta = if cfg.invert_horizontal {
                        -st.accum_dx
                    } else {
                        st.accum_dx
                    };
                    let cmd = LC::ScrollStrip { delta };

                    self.wm_sender.send(WmEvent::Command(WmCommand::ReactorCommand(
                        reactor::Command::Layout(cmd),
                    )));

                    st.accum_dx = 0.0;
                    st.phase = GesturePhase::Committed;
                }
            }
            GesturePhase::Committed => {
                if active_count == 0 {
                    st.reset();
                } else if all_moved {
                    let dx = avg_x - st.last_x;
                    let dy = avg_y - st.last_y;
                    let horizontal = dx.abs();
                    let vertical = dy.abs();
                    st.last_x = avg_x;
                    st.last_y = avg_y;
                    if vertical > cfg.vertical_tolerance || vertical >= horizontal {
                        return;
                    }
                    st.accum_dx += dx;
                    let step = cfg.distance_pct;
                    if st.accum_dx.abs() >= step {
                        let delta = if cfg.invert_horizontal {
                            -st.accum_dx
                        } else {
                            st.accum_dx
                        };
                        let cmd = LC::ScrollStrip { delta };

                        self.wm_sender.send(WmEvent::Command(WmCommand::ReactorCommand(
                            reactor::Command::Layout(cmd),
                        )));

                        st.accum_dx = 0.0;
                    }
                }
            }
        }
    }
}

fn gesture_event_mask() -> CGEventMask {
    1u64 << (NSEventType::Gesture.0 as u64)
}

#[inline]
fn touch_normalized_position(touch: &objc2_app_kit::NSTouch) -> Option<(f64, f64)> {
    if touch.r#type() != NSTouchType::Indirect || touch.isResting() {
        return None;
    }

    let position = std::panic::catch_unwind(AssertUnwindSafe(|| {
        exception::catch(AssertUnwindSafe(|| touch.normalizedPosition())).ok()
    }))
    .ok()
    .flatten()?;
    let x = position.x.clamp(0.0, 1.0) as f64;
    let y = position.y.clamp(0.0, 1.0) as f64;
    Some((x, y))
}

unsafe extern "C-unwind" fn gesture_callback(
    _proxy: CGEventTapProxy,
    event_type: CGEventType,
    event_ref: core::ptr::NonNull<CGEvent>,
    user_info: *mut std::ffi::c_void,
) -> *mut CGEvent {
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let ctx = unsafe { &*(user_info as *const CallbackCtx) };
        let event = unsafe { event_ref.as_ref() };
        ctx.this.on_event(event_type, event);
    }));

    if let Err(_) = result {
        // Gesture callback panicked; pass event through.
    }

    // ListenOnly taps must always return the event pointer.
    event_ref.as_ptr()
}
