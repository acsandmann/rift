//! Raw trackpad gesture handling via MultitouchSupport.
//!
//! Gesture recognition is driven entirely by raw contact frames. A small CG
//! event tap exists separately only to suppress macOS Dock swipes when enabled.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use objc2_core_foundation::{CGPoint, CGRect};
use objc2_core_graphics::CGEvent;
use tracing::{trace, warn};

use crate::actor;
use crate::actor::reactor;
use crate::actor::spaces::ForwardedSpaceState;
use crate::actor::wm_controller::{self, WmCommand, WmEvent};
use crate::common::collections::HashMap;
use crate::common::config::{Config, HapticPattern, LayoutMode};
use crate::layout_engine::LayoutCommand as LC;
use crate::sys::event_tap::DockSwipeSuppressor;
use crate::sys::haptics;
use crate::sys::multitouch::{TouchSnapshot, TouchTracker};
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
    multitouch: Option<TouchTracker>,
    route: Cell<Option<GestureRoute>>,
    dock_tap: RefCell<Option<DockSwipeSuppressor>>,
    dock_tap_generation: Cell<u64>,
    screen_spaces: RefCell<Vec<(CGRect, SpaceId)>>,
    layout_mode_by_space: RefCell<HashMap<SpaceId, LayoutMode>>,
    default_layout_mode: RefCell<LayoutMode>,
    requests_rx: Option<Receiver>,
}

#[derive(Debug, Clone)]
struct SwipeConfig {
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
    phase: GesturePhase,
    start_x: f64,
    start_y: f64,
}

impl SwipeState {
    fn reset(&mut self) { *self = Self::default(); }
}

#[derive(Debug, Clone)]
struct ScrollConfig {
    invert_horizontal: bool,
    vertical_tolerance: f64,
    fingers: usize,
    distance_pct: f64,
}

impl ScrollConfig {
    fn from_config(config: &Config) -> Option<Self> {
        let g = &config.settings.layout.scrolling.gestures;
        g.enabled.then(|| Self {
            invert_horizontal: g.invert_horizontal,
            vertical_tolerance: normalize_tolerance(g.vertical_tolerance),
            fingers: g.fingers.max(1),
            distance_pct: g.distance_pct.clamp(0.01, 1.0),
        })
    }
}

#[derive(Default, Debug)]
struct ScrollState {
    phase: GesturePhase,
    last_x: f64,
    last_y: f64,
    accum_dx: f64,
}

impl ScrollState {
    fn reset(&mut self) { *self = Self::default(); }
}

#[derive(Default, Debug, Copy, Clone, Eq, PartialEq)]
enum GesturePhase {
    #[default]
    Idle,
    Armed,
    Committed,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
enum GestureRoute {
    Swipe,
    Scroll,
}

struct SwipeHandler {
    cfg: SwipeConfig,
    state: RefCell<SwipeState>,
}

struct ScrollHandler {
    cfg: ScrollConfig,
    state: RefCell<ScrollState>,
}

struct DockTapCtx {
    recovery_tx: tokio::sync::mpsc::UnboundedSender<Recovery>,
    generation: u64,
}

#[derive(Clone, Copy, Debug)]
enum Recovery {
    DockTapInvalidated(u64),
}

unsafe fn drop_dock_tap_ctx(ptr: *mut std::ffi::c_void) {
    unsafe { drop(Box::from_raw(ptr as *mut DockTapCtx)) };
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
            multitouch: None,
            route: Cell::new(None),
            dock_tap: RefCell::new(None),
            dock_tap_generation: Cell::new(0),
            screen_spaces: RefCell::new(Vec::new()),
            layout_mode_by_space: RefCell::new(HashMap::default()),
            default_layout_mode: RefCell::new(default_layout_mode),
            requests_rx: Some(requests_rx),
        }
    }

    pub async fn run(mut self) {
        let mut requests_rx = self.requests_rx.take().unwrap();
        let (touch_tx, mut touch_rx) = tokio::sync::mpsc::channel(1);
        self.multitouch = match TouchTracker::start(touch_tx) {
            Ok(tracker) => {
                trace!(devices = tracker.device_count(), "Started raw multitouch input");
                Some(tracker)
            }
            Err(err) => {
                warn!(?err, "Failed to start raw MultitouchSupport input");
                None
            }
        };

        let (recovery_tx, mut recovery_rx) = tokio::sync::mpsc::unbounded_channel();
        let this = Rc::new(self);
        this.sync_input_state(&recovery_tx);

        loop {
            tokio::select! {
                touch = touch_rx.recv(), if this.multitouch.is_some() => {
                    if touch.is_some() {
                        this.on_touch_frame();
                    }
                }
                recovery = recovery_rx.recv() => {
                    let Some(Recovery::DockTapInvalidated(generation)) = recovery else { break };
                    this.rebuild_dock_tap(generation, &recovery_tx);
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
                self.update_gesture_handlers();
                self.sync_input_state(recovery_tx);
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

    fn update_gesture_handlers(&self) {
        let (swipe, scroll) = Self::build_gesture_handlers(&self.config.borrow());
        *self.swipe.borrow_mut() = swipe;
        *self.scroll.borrow_mut() = scroll;
        self.reset_gesture_state();
    }

    fn sync_input_state(
        self: &Rc<Self>,
        recovery_tx: &tokio::sync::mpsc::UnboundedSender<Recovery>,
    ) {
        let recognition_enabled = self.multitouch.is_some() && self.gesture_handlers_enabled();
        if let Some(multitouch) = &self.multitouch {
            multitouch.set_enabled(recognition_enabled);
        }
        if !recognition_enabled {
            self.reset_gesture_state();
        }

        let suppress = recognition_enabled && self.config.borrow().settings.gestures.consume_dock_swipe;
        if suppress && self.dock_tap.borrow().is_none() {
            self.create_dock_tap(recovery_tx);
        } else if !suppress {
            *self.dock_tap.borrow_mut() = None;
        }
    }

    fn gesture_handlers_enabled(&self) -> bool {
        self.swipe.borrow().is_some() || self.scroll.borrow().is_some()
    }

    fn create_dock_tap(
        &self,
        recovery_tx: &tokio::sync::mpsc::UnboundedSender<Recovery>,
    ) {
        let generation = self.dock_tap_generation.get().wrapping_add(1);
        let ctx = Box::into_raw(Box::new(DockTapCtx {
            recovery_tx: recovery_tx.clone(),
            generation,
        })) as *mut std::ffi::c_void;

        let tap = unsafe {
            DockSwipeSuppressor::new(ctx, Some(drop_dock_tap_ctx), Some(dock_tap_invalidated))
        };
        if let Some(tap) = tap {
            self.dock_tap_generation.set(generation);
            *self.dock_tap.borrow_mut() = Some(tap);
        } else {
            unsafe { drop_dock_tap_ctx(ctx) };
            warn!("Failed to create Dock swipe suppression tap; macOS gestures will pass through");
        }
    }

    fn rebuild_dock_tap(
        &self,
        generation: u64,
        recovery_tx: &tokio::sync::mpsc::UnboundedSender<Recovery>,
    ) {
        if generation != self.dock_tap_generation.get()
            || !self.gesture_handlers_enabled()
            || !self.config.borrow().settings.gestures.consume_dock_swipe
        {
            return;
        }

        *self.dock_tap.borrow_mut() = None;
        self.create_dock_tap(recovery_tx);
        warn!(generation, "Recreated invalidated Dock swipe suppression tap");
    }

    fn on_touch_frame(&self) {
        let Some(multitouch) = &self.multitouch else { return };
        if !self.gesture_handlers_enabled() {
            return;
        }

        let touches = multitouch.snapshot();
        if let Some(route) = self.route.get() {
            let active = match route {
                GestureRoute::Swipe => self
                    .swipe
                    .borrow()
                    .as_ref()
                    .is_some_and(|handler| self.handle_swipe(handler, touches)),
                GestureRoute::Scroll => self
                    .scroll
                    .borrow()
                    .as_ref()
                    .is_some_and(|handler| self.handle_scroll(handler, touches)),
            };
            if !active {
                self.route.set(None);
            }
            return;
        }

        if touches.contacts == 0 || !self.is_candidate_finger_count(touches.contacts) {
            return;
        }

        let mode = current_cursor_location()
            .and_then(|point| self.layout_mode_at_point(point))
            .unwrap_or(*self.default_layout_mode.borrow());
        let route = if matches!(mode, LayoutMode::Scrolling) && self.scroll.borrow().is_some() {
            GestureRoute::Scroll
        } else if self.swipe.borrow().is_some() {
            GestureRoute::Swipe
        } else {
            return;
        };

        let matches = match route {
            GestureRoute::Swipe => self
                .swipe
                .borrow()
                .as_ref()
                .is_some_and(|handler| touches.contacts == handler.cfg.fingers),
            GestureRoute::Scroll => self
                .scroll
                .borrow()
                .as_ref()
                .is_some_and(|handler| touches.contacts == handler.cfg.fingers),
        };
        if !matches {
            return;
        }

        self.route.set(Some(route));
        match route {
            GestureRoute::Swipe => {
                if let Some(handler) = self.swipe.borrow().as_ref() {
                    self.handle_swipe(handler, touches);
                }
            }
            GestureRoute::Scroll => {
                if let Some(handler) = self.scroll.borrow().as_ref() {
                    self.handle_scroll(handler, touches);
                }
            }
        }
    }

    fn is_candidate_finger_count(&self, contacts: usize) -> bool {
        self.swipe
            .borrow()
            .as_ref()
            .is_some_and(|handler| handler.cfg.fingers == contacts)
            || self
                .scroll
                .borrow()
                .as_ref()
                .is_some_and(|handler| handler.cfg.fingers == contacts)
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

    fn handle_swipe(&self, handler: &SwipeHandler, touches: TouchSnapshot) -> bool {
        let cfg = &handler.cfg;
        let mut state = handler.state.borrow_mut();
        if touches.contacts != cfg.fingers || touches.contacts == 0 {
            state.reset();
            return false;
        }

        match state.phase {
            GesturePhase::Idle => {
                state.start_x = touches.centroid_x;
                state.start_y = touches.centroid_y;
                state.phase = GesturePhase::Armed;
                trace!(x = state.start_x, y = state.start_y, "Swipe armed");
            }
            GesturePhase::Armed => {
                let dx = touches.centroid_x - state.start_x;
                let dy = touches.centroid_y - state.start_y;
                if dx.abs() >= cfg.distance_pct && dy.abs() <= cfg.vertical_tolerance {
                    let mut left = dx < 0.0;
                    if cfg.invert_horizontal {
                        left = !left;
                    }
                    let command = if left {
                        LC::NextWorkspace(cfg.skip_empty_workspaces)
                    } else {
                        LC::PrevWorkspace(cfg.skip_empty_workspaces)
                    };

                    if cfg.haptics_enabled {
                        let _ = haptics::perform_haptic(cfg.haptic_pattern);
                    }
                    self.send_layout_command(command);
                    state.phase = GesturePhase::Committed;
                }
            }
            GesturePhase::Committed => {}
        }
        true
    }

    fn handle_scroll(&self, handler: &ScrollHandler, touches: TouchSnapshot) -> bool {
        let cfg = &handler.cfg;
        let mut state = handler.state.borrow_mut();
        if touches.contacts != cfg.fingers || touches.contacts == 0 {
            state.reset();
            return false;
        }

        if state.phase == GesturePhase::Idle {
            state.last_x = touches.centroid_x;
            state.last_y = touches.centroid_y;
            state.phase = GesturePhase::Armed;
            trace!(x = state.last_x, y = state.last_y, "Scroll gesture armed");
            return true;
        }

        if !touches.all_moved {
            if state.phase == GesturePhase::Armed {
                state.last_x = touches.centroid_x;
                state.last_y = touches.centroid_y;
            }
            return true;
        }

        let dx = touches.centroid_x - state.last_x;
        let dy = touches.centroid_y - state.last_y;
        state.last_x = touches.centroid_x;
        state.last_y = touches.centroid_y;

        if dy.abs() > cfg.vertical_tolerance || dy.abs() >= dx.abs() {
            return true;
        }

        state.accum_dx += dx;
        if state.accum_dx.abs() >= cfg.distance_pct {
            let delta = if cfg.invert_horizontal {
                -state.accum_dx
            } else {
                state.accum_dx
            };
            self.send_layout_command(LC::ScrollStrip { delta });
            state.accum_dx = 0.0;
            state.phase = GesturePhase::Committed;
        }
        true
    }

    #[inline]
    fn send_layout_command(&self, command: LC) {
        self.wm_sender.send(WmEvent::Command(WmCommand::ReactorCommand(
            reactor::Command::Layout(command),
        )));
    }

    fn reset_gesture_state(&self) {
        self.route.set(None);
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

#[inline]
fn current_cursor_location() -> Option<CGPoint> {
    let event = CGEvent::new(None)?;
    Some(CGEvent::location(Some(&event)))
}

unsafe extern "C-unwind" fn dock_tap_invalidated(user_info: *mut std::ffi::c_void) {
    if user_info.is_null() {
        return;
    }
    let ctx = unsafe { &*(user_info as *const DockTapCtx) };
    let _ = ctx
        .recovery_tx
        .send(Recovery::DockTapInvalidated(ctx.generation));
}
