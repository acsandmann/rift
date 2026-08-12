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
use tracing::warn;

use crate::actor;
use crate::actor::reactor;
use crate::actor::spaces::ForwardedSpaceState;
use crate::actor::wm_controller::{self, WmCommand, WmEvent};
use crate::common::collections::HashMap;
use crate::common::config::{Config, HapticPattern, LayoutMode};
use crate::layout_engine::LayoutCommand as LC;
use crate::sys::gesture::{
    self, GesturePayload, ScrollGesturePayload, ScrollTouchFrame, TouchFrame, TouchPath,
};
use crate::sys::haptics;
use crate::sys::screen::SpaceId;
const SCROLL_MOVEMENT_EPSILON: f64 = 0.001;
const SCROLL_EXTRA_ABSOLUTE_EPSILON: f64 = 0.003;
const SCROLL_EXTRA_RELATIVE_THRESHOLD: f64 = 0.35;

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
    previous: Option<ScrollTouchFrame>,
    cohort: [isize; 16],
    cohort_len: usize,
    accum_dx: f64,
    consuming: bool,
}

impl ScrollState {
    #[inline]
    fn reset(&mut self) { *self = Self::default(); }

    #[inline]
    fn finish_contacts(&mut self) {
        self.previous = None;
        self.cohort_len = 0;
        self.consuming = false;
    }

    #[inline]
    fn cancel_contacts(&mut self) {
        self.finish_contacts();
        self.accum_dx = 0.0;
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct PathDelta {
    index: isize,
    dx: f64,
    dy: f64,
}

impl PathDelta {
    #[inline(always)]
    fn magnitude(self) -> f64 { self.dx.abs().max(self.dy.abs()) }
}

#[derive(Default, Debug, Copy, Clone, Eq, PartialEq)]
enum GestureState {
    #[default]
    Idle,
    Armed,
    Committed,
    /// Contact topology changed after acquisition. Do not re-arm until every
    /// finger has lifted, or removing one finger could start a new gesture in
    /// the middle of the same physical interaction.
    Rejected,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ContactDisposition {
    Ended,
    Waiting,
    Ready,
    Rejected,
}

#[inline(always)]
fn classify_contacts(
    phase: &mut GestureState,
    contacts: usize,
    expected: usize,
) -> ContactDisposition {
    if contacts == 0 {
        return ContactDisposition::Ended;
    }

    if contacts != expected {
        // Fewer contacts are normal while the user is placing fingers. Once
        // acquired, or if the count overshoots, a topology change invalidates
        // the rest of this physical session.
        if *phase != GestureState::Idle || contacts > expected {
            *phase = GestureState::Rejected;
        }
        return ContactDisposition::Waiting;
    }

    if *phase == GestureState::Rejected {
        ContactDisposition::Rejected
    } else {
        ContactDisposition::Ready
    }
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
                scroll
                    .as_ref()
                    .is_some_and(|handler| handler.cfg.consume && handler.state.borrow().consuming)
            } else {
                swipe
                    .as_ref()
                    .is_some_and(|handler| handler.cfg.consume && handler.state.borrow().consuming)
            };
            return !consume;
        }

        if !gesture::is_gesture(event_type) {
            return true;
        }

        let consume = if scrolling_mode {
            let payload = gesture::scroll_payload(event);
            scroll.as_ref().is_some_and(|handler| match payload {
                Some(ScrollGesturePayload::Touch(frame)) => self.handle_scroll(handler, frame),
                Some(ScrollGesturePayload::Processed) | None => {
                    handler.cfg.consume && handler.state.borrow().consuming
                }
            })
        } else {
            let payload = gesture::payload(event);
            swipe.as_ref().is_some_and(|handler| match payload {
                Some(GesturePayload::Touch(frame)) => self.handle_swipe(handler, frame),
                Some(GesturePayload::Processed) | None => {
                    handler.cfg.consume && handler.state.borrow().consuming
                }
            })
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

    fn handle_swipe(&self, handler: &SwipeHandler, touches: TouchFrame) -> bool {
        let cfg = &handler.cfg;
        let mut state = handler.state.borrow_mut();

        match classify_contacts(&mut state.phase, touches.contacts, cfg.fingers) {
            ContactDisposition::Ended => {
                let consuming = state.consuming;
                state.reset();
                return cfg.consume && consuming;
            }
            ContactDisposition::Waiting | ContactDisposition::Rejected => {
                return cfg.consume && state.consuming;
            }
            ContactDisposition::Ready => {}
        }

        match state.phase {
            GestureState::Idle => {
                state.start_x = touches.centroid_x;
                state.start_y = touches.centroid_y;
                state.phase = GestureState::Armed;
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
            GestureState::Rejected => {}
        }

        cfg.consume && state.consuming
    }

    fn handle_scroll(&self, handler: &ScrollHandler, touches: ScrollTouchFrame) -> bool {
        let cfg = &handler.cfg;
        let mut state = handler.state.borrow_mut();
        let was_consuming = state.consuming;

        if touches.len == 0 {
            state.phase = GestureState::Idle;
            state.reset();
            return cfg.consume && was_consuming;
        }

        if state.phase == GestureState::Rejected {
            state.previous = Some(touches);
            return false;
        }

        if state.phase == GestureState::Idle && state.previous.is_none() {
            state.accum_dx = 0.0;
        }
        let Some(previous) = state.previous.replace(touches) else {
            return false;
        };
        let mut deltas = [PathDelta::default(); 16];
        let delta_len = collect_path_deltas(&previous, &touches, &mut deltas);

        if state.cohort_len == 0 {
            let selection = select_moving_cohort(&mut deltas[..delta_len], cfg.fingers);
            let Some(selected) = selection else {
                return false;
            };
            if selected == 0 {
                state.phase = GestureState::Rejected;
                state.cancel_contacts();
                return false;
            }
            for (dst, delta) in state.cohort.iter_mut().zip(&deltas[..selected]) {
                *dst = delta.index;
            }
            state.cohort_len = selected;
            state.phase = GestureState::Armed;
        }

        let Some((mut dx, dy, cohort_motion)) = cohort_delta(
            &deltas[..delta_len],
            &state.cohort[..state.cohort_len],
            touches.paths(),
        ) else {
            // The selected fingers lifted while a stationary palm remains.
            // Do not re-arm from a remaining palm until the physical session
            // ends.
            state.phase = GestureState::Rejected;
            state.cancel_contacts();
            return cfg.consume && was_consuming;
        };

        if has_intentional_extra(
            &deltas[..delta_len],
            &state.cohort[..state.cohort_len],
            cohort_motion,
        ) {
            state.phase = GestureState::Rejected;
            state.cancel_contacts();
            return false;
        }

        let horizontal = dx.abs();
        let vertical = dy.abs();
        if state.phase == GestureState::Armed {
            if horizontal <= SCROLL_MOVEMENT_EPSILON && vertical <= SCROLL_MOVEMENT_EPSILON {
                return false;
            }
            if vertical >= horizontal || vertical > cfg.vertical_tolerance {
                state.phase = GestureState::Rejected;
                state.cancel_contacts();
                return false;
            }
            state.phase = GestureState::Committed;
            state.consuming = true;
        }

        if cfg.invert_horizontal {
            dx = -dx;
        }
        state.accum_dx += dx;
        if state.accum_dx.abs() >= cfg.distance_pct {
            let delta = state.accum_dx;
            state.accum_dx = 0.0;
            self.send_layout_command(LC::ScrollStrip { delta });
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
fn find_path(frame: &ScrollTouchFrame, index: isize) -> Option<TouchPath> {
    frame.paths().iter().copied().find(|path| path.index == index)
}

fn collect_path_deltas(
    previous: &ScrollTouchFrame,
    current: &ScrollTouchFrame,
    output: &mut [PathDelta; 16],
) -> usize {
    let mut len = 0;
    for path in current.paths() {
        let Some(old) = find_path(previous, path.index) else {
            continue;
        };
        output[len] = PathDelta {
            index: path.index,
            dx: path.x - old.x,
            dy: path.y - old.y,
        };
        len += 1;
    }
    len
}

/// Sort moving paths by magnitude and select the configured finger cohort.
/// `None` means not enough fingers have moved yet; `Some(0)` means an extra
/// path is moving strongly enough to be intentional rather than a palm.
fn select_moving_cohort(deltas: &mut [PathDelta], expected: usize) -> Option<usize> {
    deltas.sort_unstable_by(|a, b| b.magnitude().total_cmp(&a.magnitude()));
    let moving = deltas
        .iter()
        .take_while(|delta| delta.magnitude() >= SCROLL_MOVEMENT_EPSILON)
        .count();
    if expected == 0 || moving < expected {
        return None;
    }

    if moving > expected {
        let cohort_motion =
            deltas[..expected].iter().map(|delta| delta.magnitude()).sum::<f64>() / expected as f64;
        let extra = deltas[expected].magnitude();
        if extra >= SCROLL_EXTRA_ABSOLUTE_EPSILON
            && extra >= cohort_motion * SCROLL_EXTRA_RELATIVE_THRESHOLD
        {
            return Some(0);
        }
    }
    Some(expected)
}

fn cohort_delta(
    deltas: &[PathDelta],
    cohort: &[isize],
    current_paths: &[TouchPath],
) -> Option<(f64, f64, f64)> {
    if cohort.is_empty() {
        return None;
    }
    let mut dx = 0.0;
    let mut dy = 0.0;
    let mut motion = 0.0;
    for index in cohort {
        // Requiring both entries distinguishes a stationary contact (zero
        // delta, still present) from a lifted contact.
        current_paths.iter().find(|path| path.index == *index)?;
        let delta = deltas.iter().find(|delta| delta.index == *index)?;
        dx += delta.dx;
        dy += delta.dy;
        motion += delta.magnitude();
    }
    let count = cohort.len() as f64;
    Some((dx / count, dy / count, motion / count))
}

fn has_intentional_extra(deltas: &[PathDelta], cohort: &[isize], cohort_motion: f64) -> bool {
    deltas.iter().any(|delta| {
        !cohort.contains(&delta.index)
            && delta.magnitude() >= SCROLL_EXTRA_ABSOLUTE_EPSILON
            && delta.magnitude()
                >= cohort_motion.max(SCROLL_MOVEMENT_EPSILON) * SCROLL_EXTRA_RELATIVE_THRESHOLD
    })
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

#[cfg(test)]
mod tests {
    use super::{
        ContactDisposition, GestureState, PathDelta, classify_contacts, select_moving_cohort,
    };

    #[test]
    fn finger_placement_waits_until_the_configured_count() {
        let mut phase = GestureState::Idle;
        assert_eq!(classify_contacts(&mut phase, 1, 3), ContactDisposition::Waiting);
        assert_eq!(phase, GestureState::Idle);
        assert_eq!(classify_contacts(&mut phase, 2, 3), ContactDisposition::Waiting);
        assert_eq!(classify_contacts(&mut phase, 3, 3), ContactDisposition::Ready);
    }

    #[test]
    fn topology_change_after_acquisition_rejects_until_lift() {
        let mut phase = GestureState::Armed;
        assert_eq!(classify_contacts(&mut phase, 4, 3), ContactDisposition::Waiting);
        assert_eq!(phase, GestureState::Rejected);
        assert_eq!(classify_contacts(&mut phase, 3, 3), ContactDisposition::Rejected);
        assert_eq!(classify_contacts(&mut phase, 0, 3), ContactDisposition::Ended);
    }

    #[test]
    fn overshooting_before_acquisition_cannot_arm_by_removing_a_finger() {
        let mut phase = GestureState::Idle;
        assert_eq!(classify_contacts(&mut phase, 4, 3), ContactDisposition::Waiting);
        assert_eq!(phase, GestureState::Rejected);
        assert_eq!(classify_contacts(&mut phase, 3, 3), ContactDisposition::Rejected);
    }

    #[test]
    fn scrolling_selects_three_movers_and_ignores_stationary_palm() {
        let mut deltas = [
            PathDelta { index: 3, dx: 0.0002, dy: 0.0 },
            PathDelta { index: 2, dx: 0.010, dy: 0.001 },
            PathDelta { index: 6, dx: 0.009, dy: 0.001 },
            PathDelta { index: 9, dx: 0.011, dy: 0.002 },
        ];
        assert_eq!(select_moving_cohort(&mut deltas, 3), Some(3));
        let mut selected = [deltas[0].index, deltas[1].index, deltas[2].index];
        selected.sort_unstable();
        assert_eq!(selected, [2, 6, 9]);
    }

    #[test]
    fn scrolling_rejects_four_intentionally_moving_fingers() {
        let mut deltas = [
            PathDelta { index: 2, dx: 0.010, dy: 0.001 },
            PathDelta { index: 3, dx: 0.008, dy: 0.001 },
            PathDelta { index: 6, dx: 0.009, dy: 0.001 },
            PathDelta { index: 9, dx: 0.011, dy: 0.002 },
        ];
        assert_eq!(select_moving_cohort(&mut deltas, 3), Some(0));
    }
}
