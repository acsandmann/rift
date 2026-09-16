//! Keyboard, mouse and direct IOHID gesture recognition on one HID input thread.

use std::cell::{Cell, RefCell};
use std::panic::AssertUnwindSafe;
use std::str::FromStr;
use std::time::Duration;

use objc2_core_foundation::{CGPoint, CGRect};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventFlags, CGEventMask, CGEventSource, CGEventSourceStateID,
    CGEventTapLocation as CGTapLoc, CGEventTapOptions as CGTapOpt, CGEventTapProxy, CGEventType,
};
use tracing::{debug, error, trace, warn};

use super::reactor::{self, Event};
use super::stack_line;
use crate::actor;
use crate::actor::spaces::ForwardedSpaceState;
use crate::actor::wm_controller::{self, WmCommand, WmEvent};
use crate::common::collections::{HashMap, HashSet};
use crate::common::config::{Config, HapticPattern, LayoutMode, StackLineHoverMode};
use crate::layout_engine::LayoutCommand as LC;
use crate::sys::event::{self, Hotkey, KeyCode, MouseState};
use crate::sys::gesture::{
    self, GesturePayload, ScrollGesturePayload, ScrollTouchFrame, TouchFrame, TouchPath,
};
use crate::sys::hotkey::{
    Modifiers, is_modifier_key, key_code_from_event, modifier_key_is_active,
    modifiers_from_flags_with_keys,
};
use crate::sys::screen::{CoordinateConverter, SpaceId};
use crate::sys::{haptics, power, window_server};
use crate::ui::stack_line::point_hits_indicator_frame;

const MOUSE_MOVE_MIN_INTERVAL_NS_NORMAL: u64 = 16_000_000; // 16ms ~= 62 Hz
const MOUSE_MOVE_MIN_INTERVAL_NS_LOW_POWER: u64 = 32_000_000; // 32ms ~= 31 Hz

#[derive(Debug)]
pub enum Request {
    Warp(CGPoint),
    HideOnFocus,
    EnforceHidden,
    SpaceStateUpdated(ForwardedSpaceState, CoordinateConverter),
    SetEventProcessing(bool),
    SetFocusFollowsMouseEnabled(bool),
    SetHotkeys(Vec<(String, WmCommand)>),
    KeyboardLayoutChanged,
    ConfigUpdated(Config),
    LayoutModesChanged(Vec<(SpaceId, crate::common::config::LayoutMode)>),
    SetLowPowerMode(bool),
    SetDragActive(bool),
    SetMissionControlActive(bool),
}

pub struct Input {
    events_tx: reactor::Sender,
    requests_rx: Option<Receiver>,
    state: RefCell<State>,
    event_mask: Cell<CGEventMask>,
    mission_control_active: Cell<bool>,
    mouse_move_last_timestamp: Cell<Option<u64>>,
    mouse_move_min_interval_ticks: Cell<u64>,
    mouse_location: Cell<CGPoint>,
    mouse_focus_publisher: reactor::MouseFocusPublisher,
    tap: RefCell<Option<crate::sys::event_tap::EventTap>>,
    tap_generation: Cell<u64>,
    disable_hotkey: RefCell<Option<Hotkey>>,
    hotkey_specs: RefCell<Vec<(String, WmCommand)>>,
    hotkeys: RefCell<HashMap<Hotkey, Vec<WmCommand>>>,
    wm_sender: wm_controller::Sender,
    stack_line_tx: stack_line::Sender,
    mission_control_tx: super::mission_control::Sender,
    stack_line_hit_rects: stack_line::SharedHitRects,
}

impl Drop for Input {
    fn drop(&mut self) {
        // Unregister callbacks before their state is destroyed.
        self.tap.get_mut().take();
    }
}

struct State {
    hide_count: u32,
    mouse_hides_on_focus: bool,
    focus_follows_mouse_config_enabled: bool,
    default_layout_mode: LayoutMode,
    converter: CoordinateConverter,
    screens: Vec<CGRect>,
    event_processing_enabled: bool,
    focus_follows_mouse_enabled: bool,
    stack_line_enabled: bool,
    stack_line_hover_mode: StackLineHoverMode,
    disable_hotkey_active: bool,
    low_power_mode: bool,
    pressed_keys: HashSet<KeyCode>,
    current_flags: CGEventFlags,
    screen_spaces: Vec<(CGRect, SpaceId)>,
    layout_mode_by_space: HashMap<SpaceId, crate::common::config::LayoutMode>,
    last_stack_line_hit: Option<bool>,
    drag_active: bool,
    swipe: Option<SwipeHandler>,
    scroll: Option<ScrollHandler>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            hide_count: 0,
            mouse_hides_on_focus: false,
            focus_follows_mouse_config_enabled: false,
            default_layout_mode: LayoutMode::Traditional,
            converter: CoordinateConverter::default(),
            screens: Vec::new(),
            event_processing_enabled: false,
            focus_follows_mouse_enabled: true,
            stack_line_enabled: false,
            stack_line_hover_mode: StackLineHoverMode::default(),
            disable_hotkey_active: false,
            low_power_mode: power::is_low_power_mode_enabled(),
            pressed_keys: HashSet::with_capacity_and_hasher(256, Default::default()),
            current_flags: CGEventFlags::empty(),
            screen_spaces: Vec::new(),
            layout_mode_by_space: HashMap::default(),
            last_stack_line_hit: None,
            drag_active: false,
            swipe: None,
            scroll: None,
        }
    }
}

pub type Sender = actor::Sender<Request>;
pub type Receiver = actor::Receiver<Request>;

struct CallbackCtx {
    // Input owns the tap; the callback cannot outlive it or leave its thread.
    this: *const Input,
    recovery_tx: tokio::sync::mpsc::UnboundedSender<Recovery>,
    tap_generation: u64,
}

#[derive(Clone, Copy, Debug)]
enum Recovery {
    TapInvalidated(u64),
}

unsafe fn drop_input_ctx(ptr: *mut std::ffi::c_void) {
    unsafe { drop(Box::from_raw(ptr as *mut CallbackCtx)) };
}

impl Input {
    fn desired_event_mask(&self) -> CGEventMask {
        let state = self.state.borrow();
        let disable_hotkey = self.disable_hotkey.borrow();
        let keyed_disable =
            disable_hotkey.as_ref().is_some_and(|key| !is_modifier_key(key.key_code));
        let hotkeys_enabled = !self.hotkeys.borrow().is_empty();
        let mut mask = build_event_mask(
            hotkeys_enabled || keyed_disable || self.mission_control_active.get(),
            hotkeys_enabled || disable_hotkey.is_some(),
            (state.event_processing_enabled
                && (state.stack_line_enabled
                    || state.mouse_hides_on_focus
                    || (state.focus_follows_mouse_config_enabled
                        && state.focus_follows_mouse_enabled)))
                || self.mission_control_active.get(),
            state.event_processing_enabled
                && (state.stack_line_enabled || state.mouse_hides_on_focus),
            // Mouse-up delivery is part of the stable configured mask. Drag
            // start/stop is frequent enough that rebuilding the WindowServer
            // tap costs more than filtering these releases in the callback.
            state.event_processing_enabled,
            keyed_disable,
        );
        if self.mission_control_active.get() {
            mask |= (1u64 << CGEventType::LeftMouseDown.0) | (1u64 << CGEventType::LeftMouseUp.0);
        }
        if state.swipe.is_some() || state.scroll.is_some() {
            mask |= gesture::EVENT_MASK;
        }
        mask
    }

    fn create_tap_with_mask(
        &self,
        mask: CGEventMask,
        recovery_tx: tokio::sync::mpsc::UnboundedSender<Recovery>,
    ) -> Option<crate::sys::event_tap::EventTap> {
        let tap_generation = self.tap_generation.get().wrapping_add(1);
        let ctx = Box::new(CallbackCtx {
            this: self as *const Input,
            recovery_tx,
            tap_generation,
        });
        let ctx_ptr = Box::into_raw(ctx) as *mut std::ffi::c_void;

        let tap = unsafe {
            crate::sys::event_tap::EventTap::new(
                CGTapLoc::HIDEventTap,
                CGTapOpt::Default,
                mask,
                Some(input_callback),
                ctx_ptr,
                Some(drop_input_ctx),
                Some(event_tap_reenabled),
                Some(event_tap_invalidated),
            )
        };

        if tap.is_none() {
            unsafe { drop(Box::from_raw(ctx_ptr as *mut CallbackCtx)) };
        }

        if tap.is_some() {
            self.tap_generation.set(tap_generation);
        }
        tap
    }

    fn rebuild_event_tap_mask_if_needed(
        &self,
        recovery_tx: &tokio::sync::mpsc::UnboundedSender<Recovery>,
    ) {
        let next_mask = self.desired_event_mask();
        if next_mask == self.event_mask.get() && (next_mask == 0 || self.tap.borrow().is_some()) {
            return;
        }

        self.tap.borrow_mut().take();
        if next_mask == 0 {
            self.event_mask.set(0);
            return;
        }
        let Some(new_tap) = self.create_tap_with_mask(next_mask, recovery_tx.clone()) else {
            warn!("Failed to rebuild event tap with updated mask");
            return;
        };

        *self.tap.borrow_mut() = Some(new_tap);
        self.event_mask.set(next_mask);
    }

    fn rebuild_invalidated_event_tap(
        &self,
        generation: u64,
        recovery_tx: &tokio::sync::mpsc::UnboundedSender<Recovery>,
    ) {
        if generation != self.tap_generation.get() {
            debug!(generation, "Ignoring invalidation from a replaced event tap");
            return;
        }

        self.tap.borrow_mut().take();
        self.reconcile_after_tap_reenabled();
        self.rebuild_event_tap_mask_if_needed(recovery_tx);
    }

    pub fn new(
        config: Config,
        events_tx: reactor::Sender,
        requests_rx: Receiver,
        wm_sender: wm_controller::Sender,
        stack_line_tx: stack_line::Sender,
        mission_control_tx: super::mission_control::Sender,
        stack_line_hit_rects: stack_line::SharedHitRects,
    ) -> Self {
        let disable_hotkey = config
            .settings
            .focus_follows_mouse_disable_hotkey
            .clone()
            .and_then(|spec| spec.to_hotkey());
        let mut state = State::default();
        state.mouse_hides_on_focus = config.settings.mouse_hides_on_focus;
        state.focus_follows_mouse_config_enabled = config.settings.focus_follows_mouse;
        state.stack_line_enabled = config.settings.ui.stack_line.enabled;
        state.stack_line_hover_mode = config.settings.ui.stack_line.hover;
        state.default_layout_mode = config.settings.layout.mode;
        state.disable_hotkey_active = disable_hotkey
            .as_ref()
            .map(|target| state.compute_disable_hotkey_active(target))
            .unwrap_or(false);
        (state.swipe, state.scroll) = Self::build_gesture_handlers(&config);
        let mouse_move_min_interval_ticks = mouse_move_sampling_profile(state.low_power_mode);
        Input {
            events_tx,
            requests_rx: Some(requests_rx),
            state: RefCell::new(state),
            event_mask: Cell::new(0),
            mission_control_active: Cell::new(false),
            mouse_move_last_timestamp: Cell::new(None),
            mouse_move_min_interval_ticks: Cell::new(mouse_move_min_interval_ticks),
            mouse_location: Cell::new(CGPoint::new(0.0, 0.0)),
            mouse_focus_publisher: reactor::MouseFocusPublisher::default(),
            tap: RefCell::new(None),
            tap_generation: Cell::new(0),
            disable_hotkey: RefCell::new(disable_hotkey),
            hotkey_specs: RefCell::new(Vec::new()),
            hotkeys: RefCell::new(HashMap::default()),
            wm_sender,
            stack_line_tx,
            mission_control_tx,
            stack_line_hit_rects,
        }
    }

    pub async fn run(mut self) {
        let mut requests_rx = self.requests_rx.take().unwrap();
        let (recovery_tx, mut recovery_rx) = tokio::sync::mpsc::unbounded_channel();

        let this = Box::new(self);

        this.rebuild_event_tap_mask_if_needed(&recovery_tx);

        if this.state.borrow().mouse_hides_on_focus {
            if let Err(e) = window_server::allow_hide_mouse() {
                error!(
                    "Could not enable mouse hiding: {e:?}. \
                    mouse_hides_on_focus will have no effect."
                );
            }
        }

        loop {
            tokio::select! {
                // select evaluates disabled futures too; defer timer creation
                // so healthy taps allocate no timer and schedule no wakeup.
                _ = async { crate::sys::timer::Timer::sleep(Duration::from_secs(1)).await },
                    if this.tap.borrow().is_none() && this.desired_event_mask() != 0 => {
                    this.rebuild_event_tap_mask_if_needed(&recovery_tx);
                    if this.tap.borrow().is_some() { this.reconcile_after_tap_reenabled(); }
                }
                maybe_recovery = recovery_rx.recv() => {
                    let Some(recovery) = maybe_recovery else { break };
                    match recovery {
                        Recovery::TapInvalidated(generation) => {
                            this.rebuild_invalidated_event_tap(generation, &recovery_tx);
                        }
                    }
                }
                maybe_request = requests_rx.recv() => {
                    let Some((span, request)) = maybe_request else { break };
                    let _guard = span.enter();
                    this.on_request(request, &recovery_tx);
                }
            }
        }
    }

    fn on_request(
        &self,
        request: Request,
        recovery_tx: &tokio::sync::mpsc::UnboundedSender<Recovery>,
    ) {
        let mut should_rebuild_mask = false;
        let mut state = self.state.borrow_mut();
        match request {
            Request::SetDragActive(active) => {
                state.drag_active = active;
                // The release may beat the AX notification that identified
                // the drag. Reconcile once without rebuilding the stable tap.
                if active && event::get_mouse_state() == Some(MouseState::Up) {
                    state.drag_active = false;
                    self.events_tx.send(Event::MouseUp);
                }
            }
            Request::SetMissionControlActive(active) => {
                self.mission_control_active.set(active);
                should_rebuild_mask = true;
            }
            Request::Warp(point) => {
                if let Err(e) = event::warp_mouse(point) {
                    warn!("Failed to warp mouse: {e:?}");
                }
                if state.mouse_hides_on_focus && state.hide_count == 0 {
                    debug!("Hiding mouse");
                    state.hide_mouse();
                }
            }
            Request::HideOnFocus => {
                if state.mouse_hides_on_focus && state.hide_count == 0 {
                    debug!("Hiding mouse after window focus changed");
                    state.hide_mouse();
                }
            }
            Request::EnforceHidden => {
                if state.hide_count > 0 {
                    state.hide_mouse();
                }
            }
            Request::SpaceStateUpdated(space_state, converter) => {
                state.screens = space_state.screens.iter().map(|screen| screen.frame).collect();
                state.screen_spaces = space_state
                    .screens
                    .into_iter()
                    .filter_map(|screen| screen.space.map(|space| (screen.frame, space)))
                    .collect();
                state.converter = converter;
            }
            Request::SetEventProcessing(enabled) => {
                state.event_processing_enabled = enabled;
                state.reset(enabled);
                if enabled {
                    self.reset_mouse_move_sample_gate();
                }
                should_rebuild_mask = true;
            }
            Request::SetFocusFollowsMouseEnabled(enabled) => {
                debug!(
                    "focus_follows_mouse temporarily {}",
                    if enabled { "enabled" } else { "disabled" }
                );
                state.focus_follows_mouse_enabled = enabled;
                state.reset(enabled);
                if enabled {
                    self.reset_mouse_move_sample_gate();
                }
                should_rebuild_mask = true;
            }
            Request::SetHotkeys(bindings) => {
                *self.hotkey_specs.borrow_mut() = bindings;
                self.rebuild_hotkeys_for_current_layout();
                should_rebuild_mask = true;
            }
            Request::KeyboardLayoutChanged => {
                self.rebuild_hotkeys_for_current_layout();
                should_rebuild_mask = true;
            }
            Request::ConfigUpdated(new_config) => {
                self.reset_gesture_state(&mut state);
                let (swipe, scroll) = Self::build_gesture_handlers(&new_config);
                state.swipe = swipe;
                state.scroll = scroll;
                let mouse_hides_on_focus = new_config.settings.mouse_hides_on_focus;
                let focus_follows_mouse_config_enabled = new_config.settings.focus_follows_mouse;
                let stack_line_enabled = new_config.settings.ui.stack_line.enabled;
                let stack_line_hover_mode = new_config.settings.ui.stack_line.hover;
                let default_layout_mode = new_config.settings.layout.mode;
                let disable_hotkey = new_config
                    .settings
                    .focus_follows_mouse_disable_hotkey
                    .clone()
                    .and_then(|spec| spec.to_hotkey());
                *self.disable_hotkey.borrow_mut() = disable_hotkey;
                {
                    let prev_mouse_hides_on_focus = state.mouse_hides_on_focus;
                    let prev_focus_follows_mouse_config_enabled =
                        state.focus_follows_mouse_config_enabled;
                    let prev_stack_line_enabled = state.stack_line_enabled;
                    let prev_stack_line_hover_mode = state.stack_line_hover_mode;
                    state.mouse_hides_on_focus = mouse_hides_on_focus;
                    state.focus_follows_mouse_config_enabled = focus_follows_mouse_config_enabled;
                    state.stack_line_enabled = stack_line_enabled;
                    state.stack_line_hover_mode = stack_line_hover_mode;
                    state.default_layout_mode = default_layout_mode;
                    let prev_active = state.disable_hotkey_active;
                    state.disable_hotkey_active = self
                        .disable_hotkey
                        .borrow()
                        .as_ref()
                        .map(|target| state.compute_disable_hotkey_active(target))
                        .unwrap_or(false);
                    if prev_active && !state.disable_hotkey_active {
                        state.reset(true);
                        self.reset_mouse_move_sample_gate();
                    }
                    if prev_focus_follows_mouse_config_enabled
                        != state.focus_follows_mouse_config_enabled
                        || prev_stack_line_enabled != state.stack_line_enabled
                        || prev_stack_line_hover_mode != state.stack_line_hover_mode
                    {
                        state.reset_mouse_sampling();
                        self.reset_mouse_move_sample_gate();
                    }
                    if prev_mouse_hides_on_focus
                        && !state.mouse_hides_on_focus
                        && state.hide_count > 0
                    {
                        debug!("Showing mouse after disabling mouse_hides_on_focus");
                        state.show_mouse();
                    }
                }
                should_rebuild_mask = true;
            }
            Request::LayoutModesChanged(modes) => {
                state.layout_mode_by_space.clear();
                for (space, mode) in modes {
                    state.layout_mode_by_space.insert(space, mode);
                }
                debug!(
                    "Updated layout modes for {} spaces",
                    state.layout_mode_by_space.len()
                );
            }
            Request::SetLowPowerMode(enabled) => {
                if state.low_power_mode != enabled {
                    debug!("low_power_mode changed in event tap: {}", enabled);
                    state.low_power_mode = enabled;
                    state.reset_mouse_sampling();
                    self.mouse_move_min_interval_ticks.set(mouse_move_sampling_profile(enabled));
                    self.reset_mouse_move_sample_gate();
                }
            }
        }
        drop(state);

        if should_rebuild_mask {
            self.rebuild_event_tap_mask_if_needed(recovery_tx);
            // A release can precede the AX drag notification or mask replacement.
            if self.state.borrow().drag_active && event::get_mouse_state() == Some(MouseState::Up) {
                self.state.borrow_mut().drag_active = false;
                self.events_tx.send(Event::MouseUp);
            }
        }
    }

    fn refresh_disable_hotkey_state(&self, state: &mut State) {
        let Some(target) = self.disable_hotkey.borrow().as_ref().cloned() else {
            return;
        };
        let prev_active = state.disable_hotkey_active;
        state.disable_hotkey_active = state.compute_disable_hotkey_active(&target);
        if state.disable_hotkey_active != prev_active {
            if !state.disable_hotkey_active {
                state.reset(true);
                self.reset_mouse_move_sample_gate();
            }
        }
    }

    #[inline]
    fn reset_mouse_move_sample_gate(&self) { self.mouse_move_last_timestamp.set(None); }

    fn reconcile_after_tap_reenabled(&self) {
        let mut state = self.state.borrow_mut();
        self.reset_gesture_state(&mut state);
        let flags = CGEventSource::flags_state(CGEventSourceStateID::HIDSystemState);
        debug!(?flags, "Event tap was re-enabled; reconciling pressed keys");
        state.reconcile_after_event_tap_reenabled(flags);
        drop(state);
        self.refresh_disable_hotkey_state(&mut self.state.borrow_mut());
    }

    fn on_event(&self, event_type: CGEventType, event: &CGEvent) -> bool {
        match event_type {
            ty if ty.0 == gesture::CGS_EVENT_GESTURE || ty.0 == gesture::CGS_EVENT_DOCK_CONTROL => {
                self.on_gesture(ty, event)
            }
            CGEventType::KeyDown | CGEventType::KeyUp | CGEventType::FlagsChanged => {
                if event::is_rift_synthetic_event(event) {
                    return true;
                }
                if event_type == CGEventType::KeyDown && self.mission_control_active.get() {
                    let keycode = CGEvent::integer_value_field(
                        Some(event),
                        CGEventField::KeyboardEventKeycode,
                    ) as u16;
                    if let Some(input) = super::mission_control::Input::from_keycode(
                        keycode,
                        CGEvent::flags(Some(event)),
                    ) {
                        self.mission_control_tx.send(super::mission_control::Event::Input(input));
                        return false;
                    }
                }
                self.handle_keyboard_event(event_type, event, &mut self.state.borrow_mut())
            }
            CGEventType::MouseMoved => self.on_mouse_moved(event, CGEvent::location(Some(event))),
            CGEventType::LeftMouseDown | CGEventType::RightMouseDown => {
                let mut state = self.state.borrow_mut();
                if state.hide_count > 0 {
                    state.show_mouse();
                }
                if self.mission_control_active.get() && event_type == CGEventType::LeftMouseDown {
                    self.mission_control_tx.send(super::mission_control::Event::Input(
                        super::mission_control::Input::Click(CGEvent::location(Some(event))),
                    ));
                    return false;
                }
                if state.stack_line_enabled {
                    let loc = CGEvent::location(Some(event));
                    let hits = self
                        .stack_line_hit_rects
                        .load()
                        .iter()
                        .copied()
                        .any(|frame| point_hits_indicator_frame(loc, frame));
                    if hits && !window_server::is_point_occluded_by_external_window(loc) {
                        let _ = self.stack_line_tx.try_send(stack_line::Event::MouseDown(loc));
                        return false;
                    }
                }
                true
            }
            CGEventType::LeftMouseUp | CGEventType::RightMouseUp => {
                if event_type == CGEventType::LeftMouseUp && self.mission_control_active.get() {
                    return false;
                }
                let mut state = self.state.borrow_mut();
                if state.drag_active {
                    state.drag_active = false;
                    self.events_tx.send(Event::MouseUp);
                }
                true
            }
            _ => true,
        }
    }

    /// Handle mouse moves without running the generic mouse/keyboard path.
    ///
    /// Mouse moves are usually the most frequent events delivered to this tap.
    /// In particular, do not read CGEvent flags for every hardware event: the
    /// keyboard and flags-changed events already maintain modifier state, and
    /// the sampled move path below is sufficient as a recovery check.
    fn on_mouse_moved(&self, event: &CGEvent, loc: CGPoint) -> bool {
        let mut state = self.state.borrow_mut();
        if !state.event_processing_enabled && !self.mission_control_active.get() {
            return true;
        }
        if state.hide_count > 0 {
            state.show_mouse();
        }
        self.mouse_location.set(loc);
        if self.mission_control_active.get() {
            self.mission_control_tx.send(super::mission_control::Event::Input(
                super::mission_control::Input::Move(loc),
            ));
            return false;
        }

        // Recover modifier state at the sampled rate instead of once per raw
        // mouse event. Normal modifier transitions arrive through
        // FlagsChanged; this is only the defensive reconciliation path for
        // events lost while macOS UI interrupts the tap.
        if self.disable_hotkey.borrow().is_some() {
            let flags = CGEvent::flags(Some(event));
            if flags != state.current_flags {
                state.current_flags = flags;
                state.reconcile_modifier_keys();
                self.refresh_disable_hotkey_state(&mut state);
            }
        }

        // Click mode only needs hit-test transitions for cursor feedback.
        // Hover mode forwards samples so the actor can detect segment changes.
        if state.stack_line_enabled {
            let hits = self
                .stack_line_hit_rects
                .load()
                .iter()
                .copied()
                .any(|frame| point_hits_indicator_frame(loc, frame))
                && !window_server::is_point_occluded_by_external_window(loc);
            if (state.stack_line_hover_mode != StackLineHoverMode::Click && hits)
                || state.last_stack_line_hit != Some(hits)
            {
                state.last_stack_line_hit = Some(hits);
                let _ = self.stack_line_tx.try_send(stack_line::Event::MouseMoved {
                    point: loc,
                    hits_indicator: hits,
                });
            }
        }

        // Publish positions only. WindowServer hit testing and focus eligibility
        // belong on the reactor, outside the synchronous input callback.
        if state.focus_follows_mouse_config_enabled
            && state.focus_follows_mouse_enabled
            && !state.disable_hotkey_active
        {
            // Secondary pointer consumers above do not participate in focus
            // suppression or window resolution.
            drop(state);
            self.on_mouse_focus(loc);
        }

        true
    }

    fn on_mouse_focus(&self, loc: CGPoint) {
        _ = self.mouse_focus_publisher.publish(&self.events_tx, loc);
    }

    #[inline]
    fn admit_mouse_move(&self, event: &CGEvent) -> Option<CGPoint> {
        let timestamp = CGEvent::timestamp(Some(event));
        let last_timestamp = self.mouse_move_last_timestamp.get();
        if last_timestamp.is_some_and(|last| {
            timestamp
                .checked_sub(last)
                .is_some_and(|elapsed| elapsed < self.mouse_move_min_interval_ticks.get())
        }) {
            return None;
        }
        self.mouse_move_last_timestamp.set(Some(timestamp));
        Some(CGEvent::location(Some(event)))
    }

    fn handle_keyboard_event(
        &self,
        event_type: CGEventType,
        event: &CGEvent,
        state: &mut State,
    ) -> bool {
        let key_code_opt = key_code_from_event(event);

        // FlagsChanged must be interpreted using the flags from this event,
        // rather than the previous event's modifier state.
        let flags = CGEvent::flags(Some(event));
        state.current_flags = flags;

        if let Some(key_code) = key_code_opt {
            match event_type {
                CGEventType::KeyDown => {
                    if self
                        .disable_hotkey
                        .borrow()
                        .as_ref()
                        .is_some_and(|key| key.key_code == key_code)
                    {
                        state.note_key_down(key_code);
                    }
                }
                CGEventType::KeyUp => state.note_key_up(key_code),
                CGEventType::FlagsChanged => state.note_flags_changed(key_code),
                _ => {}
            }
        }
        self.refresh_disable_hotkey_state(state);

        if event_type == CGEventType::KeyDown {
            if let Some(key_code) = key_code_opt {
                let hotkey = Hotkey::new(
                    modifiers_from_flags_with_keys(state.current_flags, &state.pressed_keys),
                    key_code,
                );
                let bindings = self.hotkeys.borrow();
                if let Some(commands) = bindings.get(&hotkey) {
                    // A held key generates repeated KeyDown events. Hotkeys
                    // are press-triggered, so dispatching those repeats can
                    // execute a command over and over. This is especially
                    // surprising for workspace_auto_back_and_forth, where
                    // each repeat toggles back to the other workspace.
                    let is_repeat = CGEvent::integer_value_field(
                        Some(event),
                        CGEventField::KeyboardEventAutorepeat,
                    ) != 0;
                    if is_repeat {
                        return false;
                    }
                    for cmd in commands {
                        match cmd {
                            WmCommand::ReactorCommand(command) => {
                                self.events_tx.send(Event::Command(command.clone()))
                            }
                            _ => self.wm_sender.send(WmEvent::Command(cmd.clone())),
                        }
                    }
                    return false;
                }
            }
        }

        true
    }

    fn rebuild_hotkeys_for_current_layout(&self) {
        let specs = self.hotkey_specs.borrow();
        let mut map: HashMap<Hotkey, Vec<WmCommand>> = HashMap::default();

        for (spec, command) in specs.iter() {
            let Ok(hotkey) = Hotkey::from_str(spec) else {
                warn!(%spec, "Skipping hotkey that no longer resolves for current keyboard layout");
                continue;
            };

            if hotkey.modifiers.has_generic_modifiers() {
                for expanded_mods in hotkey.modifiers.expand_to_specific() {
                    let expanded_hotkey = Hotkey::new(expanded_mods, hotkey.key_code);
                    let entry = map.entry(expanded_hotkey).or_default();
                    if !entry.contains(command) {
                        entry.push(command.clone());
                    }
                }
            } else {
                let entry = map.entry(hotkey).or_default();
                if !entry.contains(command) {
                    entry.push(command.clone());
                }
            }
        }

        trace!(
            "Updated hotkey bindings for current keyboard layout: {}",
            map.len()
        );
        *self.hotkeys.borrow_mut() = map;
    }
}

unsafe extern "C-unwind" fn input_callback(
    _proxy: CGEventTapProxy,
    event_type: CGEventType,
    event_ref: core::ptr::NonNull<CGEvent>,
    user_info: *mut std::ffi::c_void,
) -> *mut CGEvent {
    if user_info.is_null() {
        return event_ref.as_ptr();
    }
    let ctx = unsafe { &*(user_info as *const CallbackCtx) };
    let event = unsafe { event_ref.as_ref() };

    // Keep rejected high-frequency mouse events out of catch_unwind and the
    // actor/state path entirely. The admission check is scalar-only and has
    // no fallible or panicking operations.
    let this = unsafe { &*ctx.this };
    let mouse_point = if event_type == CGEventType::MouseMoved {
        match this.admit_mouse_move(event) {
            Some(point) => Some(point),
            None => {
                return if this.mission_control_active.get() {
                    core::ptr::null_mut()
                } else {
                    event_ref.as_ptr()
                };
            }
        }
    } else {
        None
    };

    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        if let Some(point) = mouse_point {
            this.on_mouse_moved(event, point)
        } else {
            this.on_event(event_type, event)
        }
    }));

    match result {
        Ok(true) => event_ref.as_ptr(),
        Ok(false) => core::ptr::null_mut(),
        Err(_) => event_ref.as_ptr(),
    }
}

unsafe extern "C-unwind" fn event_tap_reenabled(user_info: *mut std::ffi::c_void) {
    if user_info.is_null() {
        return;
    }
    let ctx = unsafe { &*(user_info as *const CallbackCtx) };
    if std::panic::catch_unwind(AssertUnwindSafe(|| {
        unsafe { &*ctx.this }.reconcile_after_tap_reenabled()
    }))
    .is_err()
    {
        error!("Panic while reconciling input state after event tap recovery");
    }
}

unsafe extern "C-unwind" fn event_tap_invalidated(user_info: *mut std::ffi::c_void) {
    if user_info.is_null() {
        return;
    }
    let ctx = unsafe { &*(user_info as *const CallbackCtx) };
    let _ = ctx.recovery_tx.send(Recovery::TapInvalidated(ctx.tap_generation));
}

impl State {
    fn hide_mouse(&mut self) {
        if let Err(e) = event::hide_mouse() {
            warn!("Failed to hide mouse: {e:?}");
        }
        self.hide_count += 1;
    }

    fn show_mouse(&mut self) {
        while self.hide_count > 0 {
            if let Err(e) = event::show_mouse() {
                warn!("Failed to show mouse: {e:?}");
            }
            self.hide_count -= 1;
        }
    }

    fn layout_mode_at_point(&self, loc: CGPoint) -> Option<crate::common::config::LayoutMode> {
        use crate::sys::geometry::CGRectExt;
        self.screen_spaces
            .iter()
            .find(|(frame, _)| frame.contains(loc))
            .and_then(|(_, space)| self.layout_mode_by_space.get(space).copied())
    }

    fn note_key_down(&mut self, key_code: KeyCode) { self.pressed_keys.insert(key_code); }

    fn note_key_up(&mut self, key_code: KeyCode) { self.pressed_keys.remove(&key_code); }

    fn note_flags_changed(&mut self, key_code: KeyCode) {
        if !is_modifier_key(key_code) {
            return;
        }
        // Use the device-dependent side bit; the family-wide mask cannot
        // distinguish (for example) AltLeft from AltRight.
        if modifier_key_is_active(self.current_flags, key_code) {
            self.pressed_keys.insert(key_code);
        } else {
            self.pressed_keys.remove(&key_code);
        }
    }

    fn reconcile_modifier_keys(&mut self) {
        self.pressed_keys.retain(|key| {
            if is_modifier_key(*key) {
                modifier_key_is_active(self.current_flags, *key)
            } else {
                true // non-modifier keys are not reconciled here
            }
        });
    }

    fn reconcile_after_event_tap_reenabled(&mut self, flags: CGEventFlags) {
        // Any key-up may have occurred while the tap was disabled. Discard the
        // edge-triggered cache and use the authoritative live modifier state.
        self.pressed_keys.clear();
        self.current_flags = flags;
    }

    fn compute_disable_hotkey_active(&self, target: &Hotkey) -> bool {
        let active_mods = modifiers_from_flags_with_keys(self.current_flags, &self.pressed_keys);

        let check_modifier = |left: Modifiers, right: Modifiers| -> bool {
            let target_has_left = target.modifiers.contains(left);
            let target_has_right = target.modifiers.contains(right);
            let active_has_left = active_mods.contains(left);
            let active_has_right = active_mods.contains(right);

            if target_has_left && target_has_right {
                active_has_left || active_has_right
            } else if target_has_left {
                active_has_left
            } else if target_has_right {
                active_has_right
            } else {
                true
            }
        };

        let shift_ok = check_modifier(Modifiers::SHIFT_LEFT, Modifiers::SHIFT_RIGHT);
        let ctrl_ok = check_modifier(Modifiers::CONTROL_LEFT, Modifiers::CONTROL_RIGHT);
        let alt_ok = check_modifier(Modifiers::ALT_LEFT, Modifiers::ALT_RIGHT);
        let meta_ok = check_modifier(Modifiers::META_LEFT, Modifiers::META_RIGHT);

        if !(shift_ok && ctrl_ok && alt_ok && meta_ok) {
            return false;
        }

        self.base_key_active(target.key_code)
    }

    fn base_key_active(&self, key_code: KeyCode) -> bool {
        if is_modifier_key(key_code) {
            modifier_key_is_active(self.current_flags, key_code)
        } else {
            self.pressed_keys.contains(&key_code)
        }
    }

    fn reset(&mut self, enabled: bool) {
        if enabled {
            self.reset_mouse_sampling();
        }
    }

    #[inline]
    fn reset_mouse_sampling(&mut self) { self.last_stack_line_hit = None; }
}

#[inline]
fn mouse_move_sampling_profile(low_power_mode: bool) -> u64 {
    let interval_ns = if low_power_mode {
        MOUSE_MOVE_MIN_INTERVAL_NS_LOW_POWER
    } else {
        MOUSE_MOVE_MIN_INTERVAL_NS_NORMAL
    };
    // HID CGEvent timestamps use Mach ticks, unlike Session timestamps.
    // Convert the interval during configuration, not each hardware event.
    #[repr(C)]
    struct Timebase {
        numer: u32,
        denom: u32,
    }
    unsafe extern "C" {
        fn mach_timebase_info(info: *mut Timebase) -> i32;
    }
    let mut timebase = Timebase { numer: 0, denom: 0 };
    let status = unsafe { mach_timebase_info(&mut timebase) };
    assert!(status == 0 && timebase.numer != 0 && timebase.denom != 0);
    interval_in_ticks(interval_ns, timebase.numer, timebase.denom)
}

fn interval_in_ticks(nanoseconds: u64, numer: u32, denom: u32) -> u64 {
    (nanoseconds * u64::from(denom)).div_ceil(u64::from(numer)).max(1)
}

// AX drag acquisition reads live HID button state. Only an active drag needs release
// events; dragged events add no information. KeyUp releases keyed disable shortcuts.
fn build_event_mask(
    key_down_enabled: bool,
    flags_enabled: bool,
    mouse_move_enabled: bool,
    buttons_enabled: bool,
    release_enabled: bool,
    key_up_enabled: bool,
) -> CGEventMask {
    let mut mask = 0;
    if buttons_enabled {
        for ty in [CGEventType::LeftMouseDown, CGEventType::RightMouseDown] {
            mask |= 1u64 << ty.0;
        }
    }
    if release_enabled {
        mask |= (1u64 << CGEventType::LeftMouseUp.0) | (1u64 << CGEventType::RightMouseUp.0);
    }
    if key_up_enabled {
        mask |= 1u64 << CGEventType::KeyUp.0;
    }
    if mouse_move_enabled {
        mask |= 1u64 << CGEventType::MouseMoved.0;
    }
    if key_down_enabled {
        mask |= 1u64 << CGEventType::KeyDown.0;
    }
    if flags_enabled {
        mask |= 1u64 << CGEventType::FlagsChanged.0;
    }
    mask
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hid_mouse_throttle_uses_mach_ticks_instead_of_nanoseconds() {
        let (input, _, _) = input();
        // This Apple Silicon timebase is 125/3 ns per tick. Sixteen milliseconds
        // is 384,000 ticks; interpreting nanoseconds as ticks would delay FFM.
        let interval = interval_in_ticks(MOUSE_MOVE_MIN_INTERVAL_NS_NORMAL, 125, 3);
        assert_eq!(interval, 384_000);
        assert_eq!(
            interval_in_ticks(MOUSE_MOVE_MIN_INTERVAL_NS_LOW_POWER, 125, 3),
            768_000
        );
        assert_eq!(
            interval_in_ticks(MOUSE_MOVE_MIN_INTERVAL_NS_NORMAL, 1, 1),
            16_000_000
        );
        input.mouse_move_min_interval_ticks.set(interval);
        let event = CGEvent::new_mouse_event(
            None,
            CGEventType::MouseMoved,
            CGPoint::new(20.0, 30.0),
            objc2_core_graphics::CGMouseButton::Left,
        )
        .unwrap();
        let start = 1_000_000_000;
        CGEvent::set_timestamp(Some(&event), start);
        assert_eq!(input.admit_mouse_move(&event), Some(CGPoint::new(20.0, 30.0)));
        CGEvent::set_timestamp(Some(&event), start + interval - 1);
        assert!(input.admit_mouse_move(&event).is_none());
        // Moving elsewhere does not bypass the time-based sample gate.
        CGEvent::set_location(Some(&event), CGPoint::new(150.0, 30.0));
        assert!(input.admit_mouse_move(&event).is_none());
        CGEvent::set_timestamp(Some(&event), start + interval);
        assert_eq!(input.admit_mouse_move(&event), Some(CGPoint::new(150.0, 30.0)));
        // An older timestamp (e.g. switching event sources) resets the gate
        // rather than rejecting hardware input until the old clock catches up.
        CGEvent::set_timestamp(Some(&event), start - interval);
        assert!(input.admit_mouse_move(&event).is_some());
        CGEvent::set_timestamp(Some(&event), start - 1);
        assert!(input.admit_mouse_move(&event).is_none());
        CGEvent::set_timestamp(Some(&event), start);
        assert!(input.admit_mouse_move(&event).is_some());
    }

    #[test]
    fn input_runs_on_cf_run_loop_without_a_tokio_runtime() {
        let (input, _, _) = input();
        // The closed request channel makes the actor exit after polling select.
        // Even disabled select branches used to construct a Tokio sleep here.
        crate::sys::executor::Executor::run(input.run());
    }

    fn input() -> (Input, actor::Receiver<WmEvent>, actor::Receiver<Event>) {
        let (events_tx, events_rx) = actor::channel();
        let (_, requests_rx) = actor::channel();
        let (wm_tx, wm_rx) = actor::channel();
        let (stack_tx, _) = actor::channel();
        let (mc_tx, _) = actor::channel();
        let mut config = Config::default();
        config.settings.gestures.enabled = false;
        config.settings.layout.scrolling.gestures.enabled = false;
        config.settings.focus_follows_mouse = false;
        config.settings.focus_follows_mouse_disable_hotkey = None;
        config.settings.mouse_hides_on_focus = false;
        config.settings.ui.stack_line.enabled = false;
        (
            Input::new(
                config,
                events_tx,
                requests_rx,
                wm_tx,
                stack_tx,
                mc_tx,
                stack_line::new_shared_hit_rects(),
            ),
            wm_rx,
            events_rx,
        )
    }

    #[test]
    fn mask_tracks_enabled_features_without_a_mouse_baseline() {
        let (input, _, _) = input();
        assert_eq!(input.desired_event_mask(), 0);
        input.state.borrow_mut().event_processing_enabled = true;
        let stable_release_mask =
            (1u64 << CGEventType::LeftMouseUp.0) | (1u64 << CGEventType::RightMouseUp.0);
        assert_eq!(input.desired_event_mask(), stable_release_mask);
        input.state.borrow_mut().focus_follows_mouse_config_enabled = true;
        assert_eq!(
            input.desired_event_mask(),
            stable_release_mask | (1u64 << CGEventType::MouseMoved.0)
        );
        let stable_mask = input.desired_event_mask();
        input.state.borrow_mut().drag_active = true;
        assert_eq!(input.desired_event_mask(), stable_mask);
        input.state.borrow_mut().drag_active = false;
        input.state.borrow_mut().focus_follows_mouse_config_enabled = false;
        input.mission_control_active.set(true);
        let mask = input.desired_event_mask();
        assert_ne!(mask & (1u64 << CGEventType::KeyDown.0), 0);
        assert_eq!(mask & (1u64 << CGEventType::KeyUp.0), 0);
        assert_eq!(mask & (1u64 << CGEventType::RightMouseDown.0), 0);
        assert_eq!(mask & (1u64 << CGEventType::LeftMouseDragged.0), 0);
        input.mission_control_active.set(false);
        *input.disable_hotkey.borrow_mut() = Some(Hotkey::new(Modifiers::empty(), KeyCode::KeyA));
        assert_ne!(input.desired_event_mask() & (1u64 << CGEventType::KeyUp.0), 0);
        *input.disable_hotkey.borrow_mut() =
            Some(Hotkey::new(Modifiers::empty(), KeyCode::ShiftLeft));
        assert_eq!(input.desired_event_mask() & (1u64 << CGEventType::KeyUp.0), 0);
    }

    #[test]
    fn hotkeys_suppress_repeats_but_do_not_intercept_rift_synthetic_keys() {
        let (input, mut wm_rx, _) = input();
        input
            .hotkeys
            .borrow_mut()
            .insert(Hotkey::new(Modifiers::empty(), KeyCode::KeyA), vec![
                WmCommand::Wm(wm_controller::WmCmd::ReloadConfig),
            ]);
        let event = CGEvent::new_keyboard_event(None, 0, true).unwrap();
        CGEvent::set_flags(Some(&event), CGEventFlags::empty());
        assert!(!input.on_event(CGEventType::KeyDown, &event));
        assert!(wm_rx.try_recv().unwrap().0.is_none());
        CGEvent::set_integer_value_field(Some(&event), CGEventField::KeyboardEventAutorepeat, 1);
        assert!(!input.on_event(CGEventType::KeyDown, &event));
        assert!(wm_rx.try_recv().is_err());
        CGEvent::set_integer_value_field(
            Some(&event),
            CGEventField::EventSourceUserData,
            0x5249_4654,
        );
        assert!(input.on_event(CGEventType::KeyDown, &event));
        assert!(wm_rx.try_recv().is_err());
    }

    #[test]
    fn releases_only_wake_the_reactor_once_for_an_active_drag() {
        let (input, _, mut events_rx) = input();
        let event = CGEvent::new_mouse_event(
            None,
            CGEventType::LeftMouseUp,
            CGPoint::new(20.0, 30.0),
            objc2_core_graphics::CGMouseButton::Left,
        )
        .unwrap();
        assert!(input.on_event(CGEventType::LeftMouseUp, &event));
        assert!(events_rx.try_recv().is_err());
        input.state.borrow_mut().drag_active = true;
        assert!(input.on_event(CGEventType::LeftMouseUp, &event));
        assert!(matches!(events_rx.try_recv().unwrap().1, Event::MouseUp));
        assert!(input.on_event(CGEventType::LeftMouseUp, &event));
        assert!(events_rx.try_recv().is_err());
    }

    #[test]
    fn workspace_gesture_does_not_discard_mouse_focus() {
        let (input, _, mut events_rx) = input();
        input.state.borrow_mut().event_processing_enabled = true;
        input.state.borrow_mut().focus_follows_mouse_config_enabled = true;
        let mut config = Config::default();
        config.settings.gestures.enabled = true;
        config.settings.gestures.distance_pct = 1.0;
        config.settings.gestures.haptics_enabled = false;
        let (swipe, _) = Input::build_gesture_handlers(&config);
        let mut swipe = swipe.unwrap();
        let contacts = swipe.cfg.fingers;
        for centroid_x in [0.0, 0.1] {
            input.handle_swipe(&mut swipe, TouchFrame {
                contacts,
                centroid_x,
                centroid_y: 0.0,
            });
        }
        assert!(swipe.state.consuming);
        assert!(events_rx.try_recv().is_err());
        input.state.borrow_mut().swipe = Some(swipe);
        let event = CGEvent::new_mouse_event(
            None,
            CGEventType::MouseMoved,
            CGPoint::new(20.0, 30.0),
            objc2_core_graphics::CGMouseButton::Left,
        )
        .unwrap();
        assert!(input.on_mouse_moved(&event, CGPoint::new(20.0, 30.0)));
        assert!(matches!(
            events_rx.try_recv().unwrap().1,
            Event::MouseFocusPending(_)
        ));
    }

    #[test]
    fn layout_mode_at_point_uses_space_mapping() {
        let mut state = State::default();
        let left = CGRect::new(
            CGPoint::new(0.0, 0.0),
            objc2_core_foundation::CGSize::new(100.0, 100.0),
        );
        let right = CGRect::new(
            CGPoint::new(100.0, 0.0),
            objc2_core_foundation::CGSize::new(100.0, 100.0),
        );

        let left_space = SpaceId::new(1);
        let right_space = SpaceId::new(2);
        state.screen_spaces = vec![(left, left_space), (right, right_space)];
        state
            .layout_mode_by_space
            .insert(left_space, crate::common::config::LayoutMode::Traditional);
        state
            .layout_mode_by_space
            .insert(right_space, crate::common::config::LayoutMode::Scrolling);

        assert_eq!(
            state.layout_mode_at_point(CGPoint::new(50.0, 50.0)),
            Some(crate::common::config::LayoutMode::Traditional)
        );
        assert_eq!(
            state.layout_mode_at_point(CGPoint::new(150.0, 50.0)),
            Some(crate::common::config::LayoutMode::Scrolling)
        );
    }

    #[test]
    fn tap_recovery_discards_cached_keys_and_uses_live_flags() {
        let mut state = State::default();
        state.pressed_keys.insert(KeyCode::ShiftLeft);
        state.pressed_keys.insert(KeyCode::KeyA);

        let live_flags = CGEventFlags::MaskShift | CGEventFlags::MaskCommand;
        state.reconcile_after_event_tap_reenabled(live_flags);

        assert!(state.pressed_keys.is_empty());
        assert_eq!(state.current_flags, live_flags);
    }
}

const SCROLL_MOVEMENT_EPSILON: f64 = 0.001;
const SCROLL_EXTRA_ABSOLUTE_EPSILON: f64 = 0.003;
const SCROLL_EXTRA_RELATIVE_THRESHOLD: f64 = 0.35;

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
    state: SwipeState,
}

struct ScrollHandler {
    cfg: ScrollConfig,
    state: ScrollState,
}

impl Input {
    fn build_gesture_handlers(config: &Config) -> (Option<SwipeHandler>, Option<ScrollHandler>) {
        let swipe = SwipeConfig::from_config(config).map(|cfg| SwipeHandler {
            cfg,
            state: SwipeState::default(),
        });
        let scroll = ScrollConfig::from_config(config).map(|cfg| ScrollHandler {
            cfg,
            state: ScrollState::default(),
        });
        (swipe, scroll)
    }

    fn on_gesture(&self, event_type: CGEventType, event: &CGEvent) -> bool {
        let mut state = self.state.borrow_mut();
        if state.scroll.is_none() && state.swipe.is_none() {
            return true;
        }

        // Gesture CGEvents already carry the current pointer location. Avoid
        // creating another CGEvent just to route between displays/layout modes.
        let mode = state
            .layout_mode_at_point(CGEvent::location(Some(event)))
            .unwrap_or(state.default_layout_mode);
        let State { scroll, swipe, .. } = &mut *state;
        let scrolling_mode = matches!(mode, LayoutMode::Scrolling);

        if gesture::is_physical_horizontal_dock_swipe(event_type, event) {
            let consume = if scrolling_mode {
                scroll
                    .as_ref()
                    .is_some_and(|handler| handler.cfg.consume && handler.state.consuming)
            } else {
                swipe
                    .as_ref()
                    .is_some_and(|handler| handler.cfg.consume && handler.state.consuming)
            };
            return !consume;
        }

        if !gesture::is_gesture(event_type) {
            return true;
        }

        let consume = if scrolling_mode {
            if scroll.is_none() {
                return true;
            }
            // Once a scroll gesture is rejected, its paths and coordinates no
            // longer matter. Decode contact presence only until every finger
            // lifts, avoiding the full per-path extraction on each raw frame.
            let payload = if scroll
                .as_ref()
                .is_some_and(|handler| handler.state.phase == GestureState::Rejected)
            {
                gesture::scroll_contact_payload(event)
            } else {
                gesture::scroll_payload(event)
            };
            scroll.as_mut().is_some_and(|handler| match payload {
                Some(ScrollGesturePayload::Touch(frame)) => self.handle_scroll(handler, frame),
                Some(ScrollGesturePayload::Processed) | None => {
                    handler.cfg.consume && handler.state.consuming
                }
            })
        } else {
            if swipe.is_none() {
                return true;
            }
            // Committed and rejected workspace swipes only wait for contact
            // lift. Skip aggregate coordinate reads for the remainder of the
            // physical session.
            let payload = if swipe.as_ref().is_some_and(|handler| {
                matches!(
                    handler.state.phase,
                    GestureState::Committed | GestureState::Rejected
                )
            }) {
                gesture::contact_payload(event)
            } else {
                gesture::payload(event)
            };
            swipe.as_mut().is_some_and(|handler| match payload {
                Some(GesturePayload::Touch(frame)) => self.handle_swipe(handler, frame),
                Some(GesturePayload::Processed) | None => {
                    handler.cfg.consume && handler.state.consuming
                }
            })
        };

        !consume
    }

    fn handle_swipe(&self, handler: &mut SwipeHandler, touches: TouchFrame) -> bool {
        let cfg = &handler.cfg;
        let state = &mut handler.state;

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

    fn handle_scroll(&self, handler: &mut ScrollHandler, touches: ScrollTouchFrame) -> bool {
        let cfg = &handler.cfg;
        let state = &mut handler.state;
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
        self.events_tx.send(Event::Command(reactor::Command::Layout(command)));
    }

    fn reset_gesture_state(&self, state: &mut State) {
        if let Some(handler) = &mut state.swipe {
            handler.state.reset();
        }
        if let Some(handler) = &mut state.scroll {
            handler.state.reset();
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

#[cfg(test)]
mod gesture_tests {
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
