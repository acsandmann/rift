//! Trackpad workspace gestures via one low-level CGEventTap.
//!
//! Type-29 CGS gesture events retain their backing IOHID digitizer event. Rift
//! reads that contact collection directly and, when it owns the gesture,
//! suppresses the same CGEvent before it reaches applications. No AppKit,
//! NSEvent, NSTouch, or parallel MultitouchSupport stream is involved.

use std::cell::{Cell, RefCell};
use std::panic::AssertUnwindSafe;
use std::rc::Rc;

use objc2_core_foundation::{CGPoint, CGRect};
use objc2_core_graphics::{
    CGEvent, CGEventMask, CGEventTapLocation as CGTapLoc, CGEventTapOptions as CGTapOpt,
    CGEventTapProxy, CGEventType,
};
use tracing::{trace, warn};

use crate::actor;
use crate::actor::reactor;
use crate::actor::spaces::ForwardedSpaceState;
use crate::actor::wm_controller::{self, WmCommand, WmEvent};
use crate::common::collections::HashMap;
use crate::common::config::{Config, HapticPattern, LayoutMode};
use crate::layout_engine::LayoutCommand as LC;
use crate::sys::gesture::{self, EventPhase, TouchFrame};
use crate::sys::haptics;
use crate::sys::screen::SpaceId;

#[derive(Debug)]
pub enum GestureRequest {
    ConfigUpdated(Config),
    LayoutModesChanged(Vec<(SpaceId, LayoutMode)>),
    SpaceStateUpdated(ForwardedSpaceState),
}

pub type Sender = actor::Sender<GestureRequest>;
pub type Receiver = actor::Receiver<GestureRequest>;

pub struct GestureTap {
    config: RefCell<Config>,
    wm_sender: wm_controller::Sender,
    swipe: RefCell<Option<SwipeHandler>>,
    scroll: RefCell<Option<ScrollHandler>>,
    tap: RefCell<Option<crate::sys::event_tap::EventTap>>,
    tap_generation: Cell<u64>,
    screen_spaces: RefCell<Vec<(CGRect, SpaceId)>>,
    layout_mode_by_space: RefCell<HashMap<SpaceId, LayoutMode>>,
    default_layout_mode: RefCell<LayoutMode>,
    requests_rx: Option<Receiver>,
}

#[derive(Debug, Clone)]
struct SwipeConfig {
    consume: bool,
    invert_horizontal: bool,
    vertical_tolerance: f64,
    skip_empty_workspaces: Option<bool>,
    fingers: usize,
    distance_pct: f64,
    haptics_enabled: bool,
    haptic_pattern: HapticPattern,
}

impl SwipeConfig {
    fn from_config(config: &Config) -> Option<Self> {
        let g = &config.settings.gestures;
        g.enabled.then(|| Self {
            consume: g.consume_dock_swipe,
            invert_horizontal: g.invert_horizontal_swipe,
            vertical_tolerance: normalize_tolerance(g.swipe_vertical_tolerance),
            skip_empty_workspaces: g.skip_empty.then_some(true),
            fingers: g.fingers.max(1),
            distance_pct: g.distance_pct.clamp(0.01, 1.0),
            haptics_enabled: g.haptics_enabled,
            haptic_pattern: g.haptic_pattern,
        })
    }
}

#[derive(Default, Debug)]
struct SwipeState {
    phase: GestureState,
    start_x: f64,
    start_y: f64,
    consuming: bool,
}

impl SwipeState {
    #[inline]
    fn reset(&mut self) { *self = Self::default(); }
}

#[derive(Debug, Clone)]
struct ScrollConfig {
    consume: bool,
    invert_horizontal: bool,
    vertical_tolerance: f64,
    fingers: usize,
    distance_pct: f64,
}

impl ScrollConfig {
    fn from_config(config: &Config) -> Option<Self> {
        let g = &config.settings.layout.scrolling.gestures;
        g.enabled.then(|| Self {
            consume: config.settings.gestures.consume_dock_swipe,
            invert_horizontal: g.invert_horizontal,
            vertical_tolerance: normalize_tolerance(g.vertical_tolerance),
            fingers: g.fingers.max(1),
            distance_pct: g.distance_pct.clamp(0.01, 1.0),
        })
    }
}

#[derive(Default, Debug)]
struct ScrollState {
    phase: GestureState,
    last_x: f64,
    last_y: f64,
    accum_dx: f64,
    consuming: bool,
}

impl ScrollState {
    #[inline]
    fn reset(&mut self) { *self = Self::default(); }
}

#[derive(Default, Debug, Copy, Clone, Eq, PartialEq)]
enum GestureState {
    #[default]
    Idle,
    Armed,
    Committed,
}

struct SwipeHandler {
    cfg: SwipeConfig,
    state: RefCell<SwipeState>,
}

struct ScrollHandler {
    cfg: ScrollConfig,
    state: RefCell<ScrollState>,
}

struct CallbackCtx {
    this: Rc<GestureTap>,
    consumes: bool,
    recovery_tx: tokio::sync::mpsc::UnboundedSender<Recovery>,
    generation: u64,
}

#[derive(Clone, Copy, Debug)]
enum Recovery {
    TapInvalidated(u64),
}

unsafe fn drop_gesture_ctx(ptr: *mut std::ffi::c_void) {
    unsafe { drop(Box::from_raw(ptr as *mut CallbackCtx)) };
}

impl GestureTap {
    pub fn new(config: Config, wm_sender: wm_controller::Sender, requests_rx: Receiver) -> Self {
        let default_layout_mode = config.settings.layout.mode;
        let (swipe, scroll) = Self::build_gesture_handlers(&config);
        Self {
            config: RefCell::new(config),
            wm_sender,
            swipe: RefCell::new(swipe),
            scroll: RefCell::new(scroll),
            tap: RefCell::new(None),
            tap_generation: Cell::new(0),
            screen_spaces: RefCell::new(Vec::new()),
            layout_mode_by_space: RefCell::new(HashMap::default()),
            default_layout_mode: RefCell::new(default_layout_mode),
            requests_rx: Some(requests_rx),
        }
    }

    pub async fn run(mut self) {
        let mut requests_rx = self.requests_rx.take().unwrap();
        let (recovery_tx, mut recovery_rx) = tokio::sync::mpsc::unbounded_channel();
        let this = Rc::new(self);

        if this.gesture_handlers_enabled() {
            this.create_and_install_tap(&recovery_tx);
        }

        loop {
            tokio::select! {
                recovery = recovery_rx.recv() => {
                    let Some(Recovery::TapInvalidated(generation)) = recovery else { break };
                    this.rebuild_invalidated_tap(generation, &recovery_tx);
                }
                request = requests_rx.recv() => {
                    let Some((span, request)) = request else { break };
                    let _guard = span.enter();
                    this.on_request(request, &recovery_tx);
                }
            }
        }
    }

    fn on_request(
        self: &Rc<Self>,
        request: GestureRequest,
        recovery_tx: &tokio::sync::mpsc::UnboundedSender<Recovery>,
    ) {
        match request {
            GestureRequest::ConfigUpdated(config) => {
                *self.default_layout_mode.borrow_mut() = config.settings.layout.mode;
                *self.config.borrow_mut() = config;
                self.update_gesture_handlers(recovery_tx);
            }
            GestureRequest::LayoutModesChanged(modes) => {
                let mut map = self.layout_mode_by_space.borrow_mut();
                map.clear();
                map.extend(modes);
            }
            GestureRequest::SpaceStateUpdated(space_state) => {
                *self.screen_spaces.borrow_mut() = space_state
                    .screens
                    .into_iter()
                    .filter_map(|screen| screen.space.map(|space| (screen.frame, space)))
                    .collect();
            }
        }
    }

    fn build_gesture_handlers(config: &Config) -> (Option<SwipeHandler>, Option<ScrollHandler>) {
        let swipe = SwipeConfig::from_config(config).map(|cfg| SwipeHandler {
            cfg,
            state: RefCell::new(SwipeState::default()),
        });
        let scroll = ScrollConfig::from_config(config).map(|cfg| ScrollHandler {
            cfg,
            state: RefCell::new(ScrollState::default()),
        });
        (swipe, scroll)
    }

    fn update_gesture_handlers(
        self: &Rc<Self>,
        recovery_tx: &tokio::sync::mpsc::UnboundedSender<Recovery>,
    ) {
        let was_enabled = self.gesture_handlers_enabled();
        let (swipe, scroll) = Self::build_gesture_handlers(&self.config.borrow());
        *self.swipe.borrow_mut() = swipe;
        *self.scroll.borrow_mut() = scroll;
        let is_enabled = self.gesture_handlers_enabled();

        self.reset_gesture_state();
        if !was_enabled && is_enabled {
            self.create_and_install_tap(recovery_tx);
        } else if was_enabled && !is_enabled {
            *self.tap.borrow_mut() = None;
        }
    }

    fn gesture_handlers_enabled(&self) -> bool {
        self.swipe.borrow().is_some() || self.scroll.borrow().is_some()
    }

    fn create_and_install_tap(
        self: &Rc<Self>,
        recovery_tx: &tokio::sync::mpsc::UnboundedSender<Recovery>,
    ) {
        let generation = self.tap_generation.get().wrapping_add(1);
        let tap = unsafe {
            let ctx = Box::into_raw(Box::new(CallbackCtx {
                this: Rc::clone(self),
                consumes: true,
                recovery_tx: recovery_tx.clone(),
                generation,
            })) as *mut std::ffi::c_void;

            match crate::sys::event_tap::EventTap::new_at_location_with_options_and_recovery_callbacks(
                CGTapLoc::HIDEventTap,
                CGTapOpt::Default,
                gesture_event_mask(),
                Some(gesture_callback),
                ctx,
                Some(drop_gesture_ctx),
                Some(gesture_tap_reenabled),
                Some(gesture_tap_invalidated),
            ) {
                Some(tap) => Some(tap),
                None => {
                    drop(Box::from_raw(ctx as *mut CallbackCtx));
                    let ctx = Box::into_raw(Box::new(CallbackCtx {
                        this: Rc::clone(self),
                        consumes: false,
                        recovery_tx: recovery_tx.clone(),
                        generation,
                    })) as *mut std::ffi::c_void;

                    match crate::sys::event_tap::EventTap::new_at_location_with_options_and_recovery_callbacks(
                        CGTapLoc::HIDEventTap,
                        CGTapOpt::ListenOnly,
                        gesture_event_mask(),
                        Some(gesture_callback),
                        ctx,
                        Some(drop_gesture_ctx),
                        Some(gesture_tap_reenabled),
                        Some(gesture_tap_invalidated),
                    ) {
                        Some(tap) => {
                            warn!(
                                "Falling back to listen-only HID gesture tap; Rift gestures cannot be suppressed"
                            );
                            Some(tap)
                        }
                        None => {
                            drop(Box::from_raw(ctx as *mut CallbackCtx));
                            None
                        }
                    }
                }
            }
        };

        if let Some(tap) = tap {
            self.tap_generation.set(generation);
            *self.tap.borrow_mut() = Some(tap);
        } else {
            warn!("Failed to create gesture event tap");
        }
    }

    fn rebuild_invalidated_tap(
        self: &Rc<Self>,
        generation: u64,
        recovery_tx: &tokio::sync::mpsc::UnboundedSender<Recovery>,
    ) {
        if generation != self.tap_generation.get() || !self.gesture_handlers_enabled() {
            return;
        }

        self.reset_gesture_state();
        self.create_and_install_tap(recovery_tx);
        warn!(generation, "Recreated invalidated gesture event tap");
    }

    fn on_event(&self, event_type: CGEventType, event: &CGEvent) -> bool {
        let scroll = self.scroll.borrow();
        let swipe = self.swipe.borrow();
        if scroll.is_none() && swipe.is_none() {
            return true;
        }

        // Gesture CGEvents already carry the current pointer location. Avoid
        // creating another CGEvent just to route between displays/layout modes.
        let mode = self
            .layout_mode_at_point(CGEvent::location(Some(event)))
            .unwrap_or(*self.default_layout_mode.borrow());
        let scrolling_mode = matches!(mode, LayoutMode::Scrolling);

        if gesture::is_physical_horizontal_dock_swipe(event_type, event) {
            let consume = if scrolling_mode {
                scroll.as_ref().is_some_and(|handler| handler.cfg.consume)
            } else {
                swipe.as_ref().is_some_and(|handler| handler.cfg.consume)
            };
            return !consume;
        }

        if !gesture::is_gesture(event_type) {
            return true;
        }

        let phase = gesture::phase(event);
        let frame = (!phase.terminal()).then(|| gesture::touch_frame(event)).flatten();

        let consume = if scrolling_mode {
            scroll.as_ref().is_some_and(|handler| self.handle_scroll(handler, phase, frame))
        } else {
            swipe.as_ref().is_some_and(|handler| self.handle_swipe(handler, phase, frame))
        };

        !consume
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

    fn handle_swipe(
        &self,
        handler: &SwipeHandler,
        event_phase: EventPhase,
        frame: Option<TouchFrame>,
    ) -> bool {
        let cfg = &handler.cfg;
        let mut state = handler.state.borrow_mut();

        if event_phase.terminal() {
            let consuming = state.consuming;
            state.reset();
            return cfg.consume && consuming;
        }
        if event_phase.began() {
            state.reset();
        }

        // If CoreGraphics ever hands us a transient type-29 event without the
        // HID payload, never leak a gesture Rift has already claimed.
        let Some(touches) = frame else {
            return cfg.consume && state.consuming;
        };

        if touches.contacts != cfg.fingers || touches.contacts == 0 {
            let consuming = state.consuming;
            state.reset();
            return cfg.consume && consuming;
        }

        match state.phase {
            GestureState::Idle => {
                state.start_x = touches.centroid_x;
                state.start_y = touches.centroid_y;
                state.phase = GestureState::Armed;
                trace!(x = state.start_x, y = state.start_y, "Swipe armed");
            }
            GestureState::Armed => {
                let dx = touches.centroid_x - state.start_x;
                let dy = touches.centroid_y - state.start_y;
                let horizontal = dx.abs();
                let vertical = dy.abs();

                if horizontal > vertical && vertical <= cfg.vertical_tolerance {
                    state.consuming = true;
                }

                if horizontal >= cfg.distance_pct && vertical <= cfg.vertical_tolerance {
                    let mut left = dx < 0.0;
                    if cfg.invert_horizontal {
                        left = !left;
                    }

                    if cfg.haptics_enabled {
                        let _ = haptics::perform_haptic(cfg.haptic_pattern);
                    }
                    self.send_layout_command(if left {
                        LC::NextWorkspace(cfg.skip_empty_workspaces)
                    } else {
                        LC::PrevWorkspace(cfg.skip_empty_workspaces)
                    });
                    state.phase = GestureState::Committed;
                }
            }
            GestureState::Committed => {}
        }

        cfg.consume && state.consuming
    }

    fn handle_scroll(
        &self,
        handler: &ScrollHandler,
        event_phase: EventPhase,
        frame: Option<TouchFrame>,
    ) -> bool {
        let cfg = &handler.cfg;
        let mut state = handler.state.borrow_mut();

        if event_phase.terminal() {
            let consuming = state.consuming;
            state.reset();
            return cfg.consume && consuming;
        }
        if event_phase.began() {
            state.reset();
        }

        let Some(touches) = frame else {
            return cfg.consume && state.consuming;
        };

        if touches.contacts != cfg.fingers || touches.contacts == 0 {
            let consuming = state.consuming;
            state.reset();
            return cfg.consume && consuming;
        }

        if state.phase == GestureState::Idle {
            state.last_x = touches.centroid_x;
            state.last_y = touches.centroid_y;
            state.phase = GestureState::Armed;
            trace!(x = state.last_x, y = state.last_y, "Scroll gesture armed");
            return cfg.consume && state.consuming;
        }

        if !touches.all_moved {
            state.last_x = touches.centroid_x;
            state.last_y = touches.centroid_y;
            return cfg.consume && state.consuming;
        }

        let dx = touches.centroid_x - state.last_x;
        let dy = touches.centroid_y - state.last_y;
        state.last_x = touches.centroid_x;
        state.last_y = touches.centroid_y;

        let horizontal = dx.abs();
        let vertical = dy.abs();
        if vertical > cfg.vertical_tolerance || vertical >= horizontal {
            return cfg.consume && state.consuming;
        }

        state.consuming = true;
        state.accum_dx += dx;

        if state.accum_dx.abs() >= cfg.distance_pct {
            let delta = if cfg.invert_horizontal {
                -state.accum_dx
            } else {
                state.accum_dx
            };
            self.send_layout_command(LC::ScrollStrip { delta });
            state.accum_dx = 0.0;
            state.phase = GestureState::Committed;
        }

        cfg.consume && state.consuming
    }

    #[inline]
    fn send_layout_command(&self, command: LC) {
        self.wm_sender.send(WmEvent::Command(WmCommand::ReactorCommand(
            reactor::Command::Layout(command),
        )));
    }

    fn reset_gesture_state(&self) {
        if let Some(handler) = self.swipe.borrow().as_ref() {
            handler.state.borrow_mut().reset();
        }
        if let Some(handler) = self.scroll.borrow().as_ref() {
            handler.state.borrow_mut().reset();
        }
    }
}

#[inline]
fn normalize_tolerance(value: f64) -> f64 {
    if value > 1.0 {
        (value / 100.0).clamp(0.0, 1.0)
    } else {
        value.clamp(0.0, 1.0)
    }
}

#[inline(always)]
fn gesture_event_mask() -> CGEventMask { gesture::EVENT_MASK }

unsafe extern "C-unwind" fn gesture_callback(
    _proxy: CGEventTapProxy,
    event_type: CGEventType,
    event_ref: core::ptr::NonNull<CGEvent>,
    user_info: *mut std::ffi::c_void,
) -> *mut CGEvent {
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let ctx = unsafe { &*(user_info as *const CallbackCtx) };
        let event = unsafe { event_ref.as_ref() };
        (ctx.this.on_event(event_type, event), ctx.consumes)
    }));

    match result {
        Ok((true, _)) | Ok((false, false)) | Err(_) => event_ref.as_ptr(),
        Ok((false, true)) => core::ptr::null_mut(),
    }
}

unsafe extern "C-unwind" fn gesture_tap_reenabled(user_info: *mut std::ffi::c_void) {
    if user_info.is_null() {
        return;
    }
    let ctx = unsafe { &*(user_info as *const CallbackCtx) };
    if std::panic::catch_unwind(AssertUnwindSafe(|| ctx.this.reset_gesture_state())).is_err() {
        warn!("Panic while resetting gesture state after event tap recovery");
    }
}

unsafe extern "C-unwind" fn gesture_tap_invalidated(user_info: *mut std::ffi::c_void) {
    if user_info.is_null() {
        return;
    }
    let ctx = unsafe { &*(user_info as *const CallbackCtx) };
    let _ = ctx.recovery_tx.send(Recovery::TapInvalidated(ctx.generation));
}
