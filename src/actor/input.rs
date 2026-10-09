//! Keyboard, mouse and native gesture arbitration on one HID input thread.

use std::cell::{Cell, RefCell};
use std::panic::AssertUnwindSafe;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use objc2_core_foundation::{CGPoint, CGRect};
use objc2_core_graphics::{
    CGDisplayBounds, CGEvent, CGEventField, CGEventFlags, CGEventMask, CGEventSource,
    CGEventSourceStateID, CGEventTapLocation as CGTapLoc, CGEventTapOptions as CGTapOpt,
    CGEventTapProxy, CGEventType,
};
use tracing::{debug, error, trace, warn};

use super::reactor::{self, Event};
use super::stack_line;
use crate::actor;
use crate::actor::spaces::ForwardedSpaceState;
use crate::actor::wm_controller::{self, WmCommand, WmEvent};
use crate::common::collections::{HashMap, HashSet};
use crate::common::config::{
    BindingModeSpecs, Config, DragDropSettings, HorizontalMouseWarp, LayoutMode, MouseAction,
    MouseModifier, StackLineHoverMode,
};
use crate::sys::event::{self, Hotkey, KeyCode};
use crate::sys::hotkey::{
    Modifiers, is_modifier_key, key_code_from_event, modifier_key_is_active,
    modifiers_from_flags_with_keys,
};
use crate::sys::screen::{CoordinateConverter, SpaceId};
use crate::sys::{gesture, power, window_server};
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
    EnableHotkeys,
    SetBindingMode(String),
    KeyboardLayoutChanged,
    ConfigUpdated(Config),
    LayoutModesChanged(Vec<(SpaceId, crate::common::config::LayoutMode)>),
    SetLowPowerMode(bool),
    SetMissionControlActive(bool),
    ReleaseMissionControl,
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
    horizontal_mouse_warp: Cell<Option<HorizontalMouseWarp>>,
    warp_screens: RefCell<Vec<CGRect>>,
    mouse_focus_publisher: reactor::MouseFocusPublisher,
    drag_motion_publisher: crate::actor::drag::DragMotionPublisher,
    native_motion_active: Arc<AtomicBool>,
    gesture_control: super::gesture::Control,
    gesture_filter: RefCell<gesture::Filter>,
    tap: RefCell<Option<crate::sys::event_tap::EventTap>>,
    tap_generation: Cell<u64>,
    disable_hotkey: RefCell<Option<Hotkey>>,
    binding_mode_specs: RefCell<BindingModeSpecs>,
    hotkeys: RefCell<Vec<HashMap<Hotkey, Vec<WmCommand>>>>,
    mode_indices: RefCell<HashMap<String, usize>>,
    active_mode: Cell<usize>,
    hotkeys_active: Cell<bool>,
    wm_sender: wm_controller::Sender,
    stack_line_tx: stack_line::Sender,
    mission_control_tx: RefCell<Option<super::mission_control::Sender>>,
    stack_line_hit_rects: stack_line::SharedHitRects,
}

impl Drop for Input {
    fn drop(&mut self) {
        // Unregister callbacks before their state is destroyed.
        self.gesture_control.stop(&self.events_tx);
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
    mouse_features_enabled: bool,
    mouse_settings: DragDropSettings,
    captured_button: Option<crate::actor::drag::MouseButton>,
    gesture_settings: super::gesture::Settings,
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
            mouse_features_enabled: false,
            mouse_settings: DragDropSettings::default(),
            captured_button: None,
            gesture_settings: super::gesture::Settings::new(&Config::default()),
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
    NativeGestureHeld,
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
        let hotkeys_enabled = self.hotkeys.borrow().iter().any(|map| !map.is_empty());
        let mut mask = build_event_mask(
            hotkeys_enabled || keyed_disable || self.mission_control_active.get(),
            hotkeys_enabled || disable_hotkey.is_some(),
            (state.event_processing_enabled
                && (state.stack_line_enabled
                    || state.mouse_hides_on_focus
                    || self.horizontal_mouse_warp.get().is_some()
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
            mask |= (1u64 << CGEventType::LeftMouseDown.0)
                | (1u64 << CGEventType::LeftMouseUp.0)
                | (1u64 << CGEventType::LeftMouseDragged.0)
                | (1u64 << CGEventType::RightMouseDown.0)
                | (1u64 << CGEventType::RightMouseDragged.0)
                | (1u64 << CGEventType::RightMouseUp.0)
                | (1u64 << CGEventType::ScrollWheel.0);
        }
        if state.event_processing_enabled && state.mouse_features_enabled {
            if state.mouse_settings.action1 != MouseAction::None {
                mask |= (1u64 << CGEventType::LeftMouseDown.0)
                    | (1u64 << CGEventType::LeftMouseDragged.0);
            }
            if state.mouse_settings.action2 != MouseAction::None {
                mask |= (1u64 << CGEventType::RightMouseDown.0)
                    | (1u64 << CGEventType::RightMouseDragged.0);
            }
        }
        if state.event_processing_enabled && self.horizontal_mouse_warp.get().is_some() {
            mask |= (1u64 << CGEventType::LeftMouseDragged.0)
                | (1u64 << CGEventType::RightMouseDragged.0);
        }
        if state.gesture_settings.enabled() {
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

        self.reset_gestures();
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
        mission_control_tx: Option<super::mission_control::Sender>,
        stack_line_hit_rects: stack_line::SharedHitRects,
        native_motion_active: Arc<AtomicBool>,
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
        state.mouse_features_enabled = config.settings.drag_drop.enabled;
        state.mouse_settings = config.settings.drag_drop;
        state.disable_hotkey_active = disable_hotkey
            .as_ref()
            .map(|target| state.compute_disable_hotkey_active(target))
            .unwrap_or(false);
        state.gesture_settings = super::gesture::Settings::new(&config);
        let gesture_control = super::gesture::Control::new(&config);
        let mouse_move_min_interval_ticks = mouse_move_sampling_profile(state.low_power_mode);
        let input = Input {
            events_tx,
            requests_rx: Some(requests_rx),
            state: RefCell::new(state),
            event_mask: Cell::new(0),
            mission_control_active: Cell::new(false),
            mouse_move_last_timestamp: Cell::new(None),
            mouse_move_min_interval_ticks: Cell::new(mouse_move_min_interval_ticks),
            mouse_location: Cell::new(CGPoint::new(0.0, 0.0)),
            horizontal_mouse_warp: Cell::new(config.settings.horizontal_mouse_warp),
            warp_screens: RefCell::new(Vec::new()),
            mouse_focus_publisher: reactor::MouseFocusPublisher::default(),
            drag_motion_publisher: crate::actor::drag::DragMotionPublisher::default(),
            native_motion_active,
            gesture_control,
            gesture_filter: RefCell::new(gesture::Filter::default()),
            tap: RefCell::new(None),
            tap_generation: Cell::new(0),
            disable_hotkey: RefCell::new(disable_hotkey),
            binding_mode_specs: RefCell::new(Vec::new()),
            hotkeys: RefCell::new(Vec::new()),
            mode_indices: RefCell::new(HashMap::default()),
            active_mode: Cell::new(0),
            hotkeys_active: Cell::new(false),
            wm_sender,
            stack_line_tx,
            mission_control_tx: RefCell::new(mission_control_tx),
            stack_line_hit_rects,
        };
        input.install_binding_specs(config.binding_mode_specs);
        input
    }

    pub async fn run(mut self) {
        let mut requests_rx = self.requests_rx.take().unwrap();
        let (recovery_tx, mut recovery_rx) = tokio::sync::mpsc::unbounded_channel();

        let this = Box::new(self);

        this.rebuild_event_tap_mask_if_needed(&recovery_tx);
        let _gesture_monitor = this.gesture_control.start(this.events_tx.clone());

        if this.state.borrow().mouse_hides_on_focus {
            if let Err(e) = window_server::allow_hide_mouse() {
                error!(
                    "Could not enable mouse hiding: {e:?}. \
                    mouse_hides_on_focus will have no effect."
                );
            }
        }

        loop {
            let hold_deadline = this.gesture_filter.borrow().hold_deadline();
            tokio::select! {
                _ = async { crate::sys::timer::Timer::sleep(hold_deadline.unwrap().saturating_duration_since(std::time::Instant::now())).await }, if hold_deadline.is_some() => {
                    let owner = this.gesture_control.ownership_guard();
                    this.gesture_filter.borrow_mut().release_expired(*owner);
                }

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
                        Recovery::NativeGestureHeld => {}
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
        let reset_gestures = match &request {
            Request::SpaceStateUpdated(snapshot, _) => !snapshot
                .screens
                .iter()
                .filter_map(|s| s.space.map(|space| (s.frame, space)))
                .eq(self.state.borrow().screen_spaces.iter().copied()),
            Request::LayoutModesChanged(modes) => {
                let state = self.state.borrow();
                modes.len() != state.layout_mode_by_space.len()
                    || modes
                        .iter()
                        .any(|(space, mode)| state.layout_mode_by_space.get(space) != Some(mode))
            }
            Request::SetEventProcessing(enabled) => {
                *enabled != self.state.borrow().event_processing_enabled
            }
            Request::SetMissionControlActive(active) => {
                *active != self.mission_control_active.get()
            }
            Request::ConfigUpdated(_) | Request::ReleaseMissionControl => true,
            _ => false,
        };
        if reset_gestures {
            self.reset_gestures();
        }
        let configure_gestures =
            reset_gestures || matches!(&request, Request::SpaceStateUpdated(..));
        let mut should_rebuild_mask = false;
        let mut state = self.state.borrow_mut();
        match request {
            Request::ReleaseMissionControl => {
                self.mission_control_tx.borrow_mut().take();
                self.mission_control_active.set(false);
                should_rebuild_mask = true;
            }
            Request::SetMissionControlActive(active) => {
                self.mission_control_active.set(active);
                self.reset_mouse_move_sample_gate();
                if active && state.hide_count > 0 {
                    state.show_mouse();
                }
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
                // ScreenInfo.frame excludes menu bar/Dock areas; cursor edges use raw CG bounds.
                let mut screens = self.warp_screens.borrow_mut();
                *screens = space_state
                    .screens
                    .iter()
                    .map(|screen| CGDisplayBounds(screen.id.as_u32()))
                    .collect();
                sort_warp_screens(&mut screens, self.horizontal_mouse_warp.get());
                state.screen_spaces = space_state
                    .screens
                    .into_iter()
                    .filter_map(|screen| screen.space.map(|space| (screen.frame, space)))
                    .collect();
                state.converter = converter;
            }
            Request::SetEventProcessing(enabled) => {
                if state.captured_button.take().is_some() {
                    self.events_tx.send(Event::DragCancel);
                }
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
            Request::EnableHotkeys => {
                if !self.hotkeys_active.replace(true) {
                    self.rebuild_binding_maps();
                    should_rebuild_mask = true;
                }
            }
            Request::SetBindingMode(target) => self.transition_binding_mode(&target),
            Request::KeyboardLayoutChanged => {
                if self.hotkeys_active.get() {
                    self.rebuild_binding_maps();
                    should_rebuild_mask = true;
                }
            }
            Request::ConfigUpdated(new_config) => {
                if *self.binding_mode_specs.borrow() != new_config.binding_mode_specs {
                    self.install_binding_specs(new_config.binding_mode_specs.clone());
                }
                let cancel_captured_drag = state.captured_button.is_some()
                    && (!new_config.settings.drag_drop.enabled
                        || new_config.settings.drag_drop != state.mouse_settings);
                if cancel_captured_drag {
                    state.captured_button = None;
                    self.events_tx.send(Event::DragCancel);
                }
                state.gesture_settings = super::gesture::Settings::new(&new_config);
                let mouse_hides_on_focus = new_config.settings.mouse_hides_on_focus;
                let focus_follows_mouse_config_enabled = new_config.settings.focus_follows_mouse;
                let stack_line_enabled = new_config.settings.ui.stack_line.enabled;
                let stack_line_hover_mode = new_config.settings.ui.stack_line.hover;
                let default_layout_mode = new_config.settings.layout.mode;
                let mouse_features_enabled = new_config.settings.drag_drop.enabled;
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
                    let direction = new_config.settings.horizontal_mouse_warp;
                    if self.horizontal_mouse_warp.replace(direction) != direction {
                        sort_warp_screens(&mut self.warp_screens.borrow_mut(), direction);
                    }
                    state.stack_line_enabled = stack_line_enabled;
                    state.stack_line_hover_mode = stack_line_hover_mode;
                    state.default_layout_mode = default_layout_mode;
                    state.mouse_features_enabled = mouse_features_enabled;
                    state.mouse_settings = new_config.settings.drag_drop;
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
        if configure_gestures {
            self.gesture_control.configure(
                state.gesture_settings,
                state.event_processing_enabled && !self.mission_control_active.get(),
                state
                    .screen_spaces
                    .iter()
                    .map(|&(frame, space)| {
                        (
                            frame,
                            space,
                            state
                                .layout_mode_by_space
                                .get(&space)
                                .copied()
                                .unwrap_or(state.default_layout_mode),
                        )
                    })
                    .collect(),
            );
        }
        drop(state);

        if should_rebuild_mask {
            self.rebuild_event_tap_mask_if_needed(recovery_tx);
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
        self.reset_gestures();
        if state.captured_button.take().is_some() {
            self.events_tx.send(Event::DragCancel);
        }
        let flags = CGEventSource::flags_state(CGEventSourceStateID::HIDSystemState);
        debug!(?flags, "Event tap was re-enabled; reconciling pressed keys");
        state.reconcile_after_event_tap_reenabled(flags);
        drop(state);
        self.refresh_disable_hotkey_state(&mut self.state.borrow_mut());
    }

    fn on_event(
        &self,
        event_type: CGEventType,
        event: &CGEvent,
        proxy: Option<CGEventTapProxy>,
    ) -> bool {
        match event_type {
            ty if ty.0 == gesture::CGS_EVENT_GESTURE || ty.0 == gesture::CGS_EVENT_DOCK_CONTROL => {
                self.native_gesture_forward(ty, event, proxy)
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
                        self.send_overview(super::mission_control::Event::Input(input));
                        return false;
                    }
                }
                self.handle_keyboard_event(event_type, event, &mut self.state.borrow_mut())
            }
            CGEventType::ScrollWheel if self.mission_control_active.get() => {
                let continuous = CGEvent::integer_value_field(
                    Some(event),
                    CGEventField::ScrollWheelEventIsContinuous,
                ) != 0;
                let (x, y, scale) = if continuous {
                    (
                        CGEventField::ScrollWheelEventPointDeltaAxis2,
                        CGEventField::ScrollWheelEventPointDeltaAxis1,
                        1.0,
                    )
                } else {
                    (
                        CGEventField::ScrollWheelEventDeltaAxis2,
                        CGEventField::ScrollWheelEventDeltaAxis1,
                        16.0,
                    )
                };
                self.send_overview(super::mission_control::Event::Input(
                    super::mission_control::Input::Scroll {
                        point: CGEvent::location(Some(event)),
                        delta: overview_scroll_delta(
                            CGPoint::new(
                                CGEvent::integer_value_field(Some(event), x) as f64 * scale,
                                CGEvent::integer_value_field(Some(event), y) as f64 * scale,
                            ),
                            CGEvent::flags(Some(event)),
                        ),
                    },
                ));
                false
            }
            CGEventType::ScrollWheel => self.native_gesture_forward(event_type, event, proxy),
            CGEventType::MouseMoved => self.on_mouse_moved(event, CGEvent::location(Some(event))),
            CGEventType::LeftMouseDragged | CGEventType::RightMouseDragged => {
                if self.mission_control_active.get() && event_type == CGEventType::LeftMouseDragged
                {
                    self.send_overview(super::mission_control::Event::Input(
                        super::mission_control::Input::PointerDrag(CGEvent::location(Some(event))),
                    ));
                    // Keep the hardware cursor moving without delivering a drag to apps.
                    CGEvent::set_type(Some(event), CGEventType::MouseMoved);
                    return true;
                }
                if self.mission_control_active.get() {
                    self.send_overview(super::mission_control::Event::Input(
                        super::mission_control::Input::Scroll {
                            point: CGEvent::location(Some(event)),
                            delta: CGPoint::new(
                                CGEvent::integer_value_field(
                                    Some(event),
                                    CGEventField::MouseEventDeltaX,
                                ) as f64,
                                0.0,
                            ),
                        },
                    ));
                    CGEvent::set_type(Some(event), CGEventType::MouseMoved);
                    return true;
                }
                let button = if event_type == CGEventType::LeftMouseDragged {
                    crate::actor::drag::MouseButton::Left
                } else {
                    crate::actor::drag::MouseButton::Right
                };
                let point = self
                    .maybe_horizontal_mouse_warp(event)
                    .unwrap_or_else(|| CGEvent::location(Some(event)));
                let captured = self.state.borrow().captured_button == Some(button);
                if captured || self.native_motion_active.load(Ordering::Acquire) {
                    let publisher = &self.drag_motion_publisher;
                    if publisher.publish(crate::actor::drag::DragMotion { point }) {
                        self.events_tx.send(Event::DragMotionPending(publisher.clone()));
                    }
                }
                !captured
            }
            CGEventType::LeftMouseDown | CGEventType::RightMouseDown => {
                let mut state = self.state.borrow_mut();
                if state.hide_count > 0 {
                    state.show_mouse();
                }
                if self.mission_control_active.get() && event_type == CGEventType::LeftMouseDown {
                    self.send_overview(super::mission_control::Event::Input(
                        super::mission_control::Input::PointerDown(CGEvent::location(Some(event))),
                    ));
                    return false;
                }
                if self.mission_control_active.get() {
                    return false;
                }
                let button = if event_type == CGEventType::LeftMouseDown {
                    crate::actor::drag::MouseButton::Left
                } else {
                    crate::actor::drag::MouseButton::Right
                };
                let action = if button == crate::actor::drag::MouseButton::Left {
                    state.mouse_settings.action1
                } else {
                    state.mouse_settings.action2
                };
                let flag = mouse_modifier_flag(state.mouse_settings.modifier);
                if state.mouse_features_enabled
                    && action != MouseAction::None
                    && CGEvent::flags(Some(event)).contains(flag)
                {
                    state.captured_button = Some(button);
                    self.events_tx.send(Event::ModifierMouseDown {
                        button,
                        point: CGEvent::location(Some(event)),
                        action,
                    });
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
                    self.send_overview(super::mission_control::Event::Input(
                        super::mission_control::Input::PointerUp(CGEvent::location(Some(event))),
                    ));
                    return false;
                }
                if self.mission_control_active.get() {
                    return false;
                }
                let button = if event_type == CGEventType::LeftMouseUp {
                    crate::actor::drag::MouseButton::Left
                } else {
                    crate::actor::drag::MouseButton::Right
                };
                let captured = self.state.borrow().captured_button == Some(button);
                if self.state.borrow().mouse_features_enabled {
                    self.events_tx.send(Event::MouseUp(button));
                }
                if captured {
                    self.state.borrow_mut().captured_button = None;
                }
                !captured
            }
            _ => true,
        }
    }

    fn send_overview(&self, event: super::mission_control::Event) {
        if let Some(tx) = &*self.mission_control_tx.borrow() {
            tx.send(event);
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
            self.send_overview(super::mission_control::Event::Input(
                super::mission_control::Input::Move(loc),
            ));
            return true;
        }

        // Recover modifier state at the sampled rate instead of once per raw
        // mouse event. Normal modifier transitions arrive through
        // FlagsChanged; this is only the defensive reconciliation path for
        // events lost while macOS UI interrupts the tap.
        if self.disable_hotkey.borrow().is_some() || state.mouse_features_enabled {
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
            && state.captured_button.is_none()
            && !state.current_flags.contains(mouse_modifier_flag(state.mouse_settings.modifier))
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

    fn maybe_horizontal_mouse_warp(&self, event: &CGEvent) -> Option<CGPoint> {
        self.horizontal_mouse_warp.get()?;
        if !self.state.borrow().event_processing_enabled {
            return None;
        }
        let point = CGEvent::location(Some(event));
        let delta = CGEvent::integer_value_field(Some(event), CGEventField::MouseEventDeltaX);
        let target = horizontal_warp_target(&self.warp_screens.borrow(), point, delta)?;
        if let Err(error) = event::warp_mouse(target) {
            warn!(?error, "Horizontal mouse warp failed");
            return None;
        }
        CGEvent::set_location(Some(event), target);
        Some(target)
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
                let active_mode = self.active_mode.get();
                let bindings = self.hotkeys.borrow();
                if let Some(commands) = bindings.get(active_mode).and_then(|map| map.get(&hotkey)) {
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
                            WmCommand::Wm(wm_controller::WmCmd::BindingMode(target)) => {
                                self.transition_binding_mode(target);
                            }
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

    fn active_binding_mode(&self) -> String {
        self.binding_mode_specs
            .borrow()
            .get(self.active_mode.get())
            .map(|(name, _)| name.clone())
            .unwrap_or_else(|| "default".into())
    }

    fn notify_binding_mode_changed(&self, previous_mode: String) {
        let mode = self.active_binding_mode();
        if mode != previous_mode {
            self.events_tx.send(Event::BindingModeChanged { mode });
        }
    }

    fn install_binding_specs(&self, specs: BindingModeSpecs) {
        let previous_mode = self.active_binding_mode();
        let indices = specs
            .iter()
            .enumerate()
            .map(|(index, (name, _))| (name.clone(), index))
            .collect();
        *self.binding_mode_specs.borrow_mut() = specs;
        *self.mode_indices.borrow_mut() = indices;
        self.active_mode.set(0);
        self.notify_binding_mode_changed(previous_mode);
        if self.hotkeys_active.get() {
            self.rebuild_binding_maps();
        }
    }

    fn transition_binding_mode(&self, target: &str) {
        if let Some(&index) = self.mode_indices.borrow().get(target) {
            let previous_mode = self.active_binding_mode();
            self.active_mode.set(index);
            self.notify_binding_mode_changed(previous_mode);
        }
    }

    fn rebuild_binding_maps(&self) {
        let specs = self.binding_mode_specs.borrow();
        let mut maps = Vec::with_capacity(specs.len());
        for (mode, bindings) in specs.iter() {
            let mut map: HashMap<Hotkey, Vec<WmCommand>> = HashMap::default();
            for (spec, command) in bindings {
                let Ok(hotkey) = Hotkey::from_str(spec) else {
                    warn!(%spec, %mode, "Skipping hotkey that no longer resolves for current keyboard layout");
                    continue;
                };
                let mut insert = |hotkey| {
                    let entry = map.entry(hotkey).or_default();
                    if !entry.contains(command) {
                        entry.push(command.clone());
                    }
                };
                if hotkey.modifiers.has_generic_modifiers() {
                    for modifiers in hotkey.modifiers.expand_to_specific() {
                        insert(Hotkey::new(modifiers, hotkey.key_code));
                    }
                } else {
                    insert(hotkey);
                }
            }
            maps.push(map);
        }
        trace!("Updated hotkey maps for current keyboard layout: {}", maps.len());
        *self.hotkeys.borrow_mut() = maps;
    }
}

fn overview_scroll_delta(delta: CGPoint, flags: CGEventFlags) -> CGPoint {
    if flags.contains(CGEventFlags::MaskShift) {
        CGPoint::new(
            if delta.y.abs() >= delta.x.abs() {
                delta.y
            } else {
                delta.x
            },
            0.0,
        )
    } else {
        delta
    }
}

unsafe extern "C-unwind" fn input_callback(
    proxy: CGEventTapProxy,
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
    // actor/state path. Edge crossings are checked before sampling so a quick
    // movement cannot stall at the edge.
    let this = unsafe { &*ctx.this };
    let mouse_point = if event_type == CGEventType::MouseMoved {
        let warped = if this.mission_control_active.get() {
            None
        } else {
            this.maybe_horizontal_mouse_warp(event)
        };
        match this.admit_mouse_move(event) {
            Some(point) => Some(warped.unwrap_or(point)),
            None => {
                return event_ref.as_ptr();
            }
        }
    } else {
        None
    };

    let was_holding = this.gesture_filter.borrow().hold_deadline().is_some();
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        if let Some(point) = mouse_point {
            this.on_mouse_moved(event, point)
        } else {
            this.on_event(event_type, event, Some(proxy))
        }
    }));

    if !was_holding && this.gesture_filter.borrow().hold_deadline().is_some() {
        let _ = ctx.recovery_tx.send(Recovery::NativeGestureHeld);
    }
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

    /// Leaves `captured_button` alone: dropping it here strands a modifier drag without a
    /// DragCancel, so callers that end a drag take it and cancel explicitly.
    fn reset(&mut self, enabled: bool) {
        if enabled {
            self.reset_mouse_sampling();
        }
    }

    #[inline]
    fn reset_mouse_sampling(&mut self) { self.last_stack_line_hit = None; }
}

fn mouse_modifier_flag(modifier: MouseModifier) -> CGEventFlags {
    match modifier {
        MouseModifier::Cmd => CGEventFlags::MaskCommand,
        MouseModifier::Alt => CGEventFlags::MaskAlternate,
        MouseModifier::Shift => CGEventFlags::MaskShift,
        MouseModifier::Ctrl => CGEventFlags::MaskControl,
        MouseModifier::Fn => CGEventFlags::MaskSecondaryFn,
    }
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

const WARP_EDGE_THRESHOLD: f64 = 3.0;
const WARP_LANDING_INSET: f64 = 6.0;

fn sort_warp_screens(screens: &mut [CGRect], direction: Option<HorizontalMouseWarp>) {
    screens.sort_by(|a, b| {
        let order = a
            .origin
            .y
            .total_cmp(&b.origin.y)
            .then_with(|| a.origin.x.total_cmp(&b.origin.x));
        if direction == Some(HorizontalMouseWarp::BottomToTop) {
            order.reverse()
        } else {
            order
        }
    });
}

fn horizontal_warp_target(screens: &[CGRect], point: CGPoint, delta_x: i64) -> Option<CGPoint> {
    if delta_x == 0 {
        return None;
    }
    let index = screens.iter().position(|screen| {
        point.x >= screen.origin.x
            && point.x < screen.origin.x + screen.size.width
            && point.y >= screen.origin.y
            && point.y < screen.origin.y + screen.size.height
    })?;
    let source = screens[index];
    let right = delta_x > 0;
    if right && point.x < source.origin.x + source.size.width - WARP_EDGE_THRESHOLD
        || !right && point.x > source.origin.x + WARP_EDGE_THRESHOLD
    {
        return None;
    }
    let target = *screens.get(if right {
        index.checked_add(1)?
    } else {
        index.checked_sub(1)?
    })?;
    // Keep the same distance from the top; a shorter target has no crossing below its bottom.
    let y = target.origin.y + point.y - source.origin.y;
    if y < target.origin.y || y >= target.origin.y + target.size.height {
        return None;
    }
    // Inset the landing point so a following reverse movement cannot immediately warp back.
    let inset = WARP_LANDING_INSET.min((target.size.width / 2.0).max(0.0));
    Some(CGPoint::new(
        if right {
            target.origin.x + inset
        } else {
            target.origin.x + target.size.width - inset
        },
        y,
    ))
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
    fn native_hold_timeout_does_not_reject_a_slow_physical_stroke() {
        let (input, _, _) = input();
        *input.gesture_control.ownership_guard() = gesture::Ownership {
            session: 1,
            owner: gesture::Owner::Undecided,
            consume: true,
            touching: true,
            dock_owner: None,
        };
        let event = |phase| {
            let event = CGEvent::new_scroll_wheel_event2(
                None,
                objc2_core_graphics::CGScrollEventUnit::Pixel,
                2,
                0,
                0,
                0,
            )
            .unwrap();
            CGEvent::set_integer_value_field(
                Some(&event),
                CGEventField::ScrollWheelEventScrollPhase,
                phase,
            );
            event
        };
        assert!(!input.native_gesture_forward(CGEventType::ScrollWheel, &event(1), None));
        std::thread::sleep(Duration::from_millis(70));
        input.native_gesture_forward(CGEventType::ScrollWheel, &event(2), None);
        assert_eq!(
            input.gesture_control.ownership_guard().owner,
            gesture::Owner::Undecided,
            "native delivery timeout must not impose a minimum recognition speed"
        );
        input.gesture_control.ownership_guard().owner = gesture::Owner::Rift;
        assert!(!input.native_gesture_forward(CGEventType::ScrollWheel, &event(2), None));
    }

    #[test]
    fn horizontal_warp_geometry() {
        let rect =
            |x, y, w, h| CGRect::new(CGPoint::new(x, y), objc2_core_foundation::CGSize::new(w, h));
        let top = rect(100.0, 0.0, 100.0, 100.0);
        let middle = rect(-50.0, 100.0, 80.0, 70.0);
        let bottom = rect(300.0, 170.0, 100.0, 100.0);
        let mut screens = vec![bottom, top, middle];
        sort_warp_screens(&mut screens, Some(HorizontalMouseWarp::TopToBottom));
        assert_eq!(
            horizontal_warp_target(&screens, CGPoint::new(198.0, 20.0), 1),
            Some(CGPoint::new(-44.0, 120.0))
        );
        assert_eq!(
            horizontal_warp_target(&screens, CGPoint::new(-49.0, 120.0), -1),
            Some(CGPoint::new(194.0, 20.0))
        );
        assert_eq!(
            horizontal_warp_target(&screens, CGPoint::new(101.0, 20.0), -1),
            None
        );
        assert_eq!(
            horizontal_warp_target(&screens, CGPoint::new(398.0, 190.0), 1),
            None
        );
        assert_eq!(
            horizontal_warp_target(&screens, CGPoint::new(101.0, 20.0), 1),
            None
        );
        assert_eq!(
            horizontal_warp_target(&screens, CGPoint::new(198.0, 20.0), -1),
            None
        );
        assert_eq!(
            horizontal_warp_target(&screens, CGPoint::new(198.0, 90.0), 1),
            None
        );
        assert_eq!(
            horizontal_warp_target(&screens, CGPoint::new(-49.0, 120.0), 1),
            None
        );
        assert_eq!(
            horizontal_warp_target(&screens, CGPoint::new(28.0, 120.0), 1),
            Some(CGPoint::new(306.0, 190.0))
        );
        assert_eq!(
            horizontal_warp_target(&screens, CGPoint::new(-44.0, 120.0), -1),
            None
        );
        sort_warp_screens(&mut screens, Some(HorizontalMouseWarp::BottomToTop));
        assert_eq!(
            horizontal_warp_target(&screens, CGPoint::new(398.0, 190.0), 1),
            Some(CGPoint::new(-44.0, 120.0))
        );
    }

    #[test]
    fn horizontal_warp_shared_boundary_belongs_to_lower_display() {
        let top = CGRect::new(
            CGPoint::new(0.0, 0.0),
            objc2_core_foundation::CGSize::new(100.0, 900.0),
        );
        let bottom = CGRect::new(CGPoint::new(0.0, 900.0), top.size);
        assert_eq!(
            horizontal_warp_target(&[top, bottom], CGPoint::new(0.0, 900.0), -1),
            Some(CGPoint::new(94.0, 0.0))
        );
        assert_eq!(
            horizontal_warp_target(&[bottom, top], CGPoint::new(99.0, 900.0), 1),
            Some(CGPoint::new(6.0, 0.0))
        );
        assert_eq!(
            horizontal_warp_target(&[top, bottom], CGPoint::new(100.0, 900.0), -1),
            None
        );
    }

    #[test]
    fn disabled_horizontal_warp_does_not_borrow_state_or_screens() {
        let (input, _, _) = input();
        let event = CGEvent::new(None).unwrap();
        let _state = input.state.borrow_mut();
        let _screens = input.warp_screens.borrow_mut();
        assert_eq!(input.maybe_horizontal_mouse_warp(&event), None);
    }

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
    fn overview_passes_sampled_and_skipped_mouse_motion_to_cursor() {
        let (input, mut wm_rx, mut native_rx) = input();
        let (tx, mut rx) = actor::channel();
        *input.mission_control_tx.borrow_mut() = Some(tx);
        input.mission_control_active.set(true);
        input.mouse_move_min_interval_ticks.set(100);
        let (recovery_tx, _) = tokio::sync::mpsc::unbounded_channel();
        let mut ctx = CallbackCtx {
            this: &input,
            recovery_tx,
            tap_generation: 0,
        };
        let event = CGEvent::new(None).unwrap();
        CGEvent::set_type(Some(&event), CGEventType::MouseMoved);
        CGEvent::set_timestamp(Some(&event), 1000);
        CGEvent::set_location(Some(&event), CGPoint::new(30.0, 40.0));
        let event_ptr = core::ptr::NonNull::from(&*event);
        let context = (&mut ctx as *mut CallbackCtx).cast();
        let result = unsafe {
            input_callback(
                core::ptr::null_mut(),
                CGEventType::MouseMoved,
                event_ptr,
                context,
            )
        };
        assert_eq!(result, event_ptr.as_ptr());
        assert!(matches!(
            rx.try_recv().unwrap().1,
            super::super::mission_control::Event::Input(
                super::super::mission_control::Input::Move(_)
            )
        ));
        CGEvent::set_timestamp(Some(&event), 1001);
        let result = unsafe {
            input_callback(
                core::ptr::null_mut(),
                CGEventType::MouseMoved,
                event_ptr,
                context,
            )
        };
        assert_eq!(result, event_ptr.as_ptr());
        assert!(rx.try_recv().is_err());
        assert!(wm_rx.try_recv().is_err());
        assert!(native_rx.try_recv().is_err());
    }

    #[test]
    fn overview_pointer_sequence_never_enters_native_drag_path() {
        let (input, mut wm_rx, mut native_rx) = input();
        let (tx, mut rx) = actor::channel();
        *input.mission_control_tx.borrow_mut() = Some(tx);
        input.mission_control_active.set(true);
        let event = CGEvent::new(None).unwrap();
        CGEvent::set_location(Some(&event), CGPoint::new(30.0, 40.0));
        for ty in [
            CGEventType::LeftMouseDown,
            CGEventType::LeftMouseDragged,
            CGEventType::LeftMouseUp,
        ] {
            assert_eq!(
                input.on_event(ty, &event, None),
                ty == CGEventType::LeftMouseDragged
            );
            if ty == CGEventType::LeftMouseDragged {
                assert_eq!(CGEvent::r#type(Some(&event)), CGEventType::MouseMoved);
            }
        }
        assert!(matches!(
            rx.try_recv().unwrap().1,
            super::super::mission_control::Event::Input(
                super::super::mission_control::Input::PointerDown(_)
            )
        ));
        assert!(matches!(
            rx.try_recv().unwrap().1,
            super::super::mission_control::Event::Input(
                super::super::mission_control::Input::PointerDrag(_)
            )
        ));
        assert!(matches!(
            rx.try_recv().unwrap().1,
            super::super::mission_control::Event::Input(
                super::super::mission_control::Input::PointerUp(_)
            )
        ));
        assert!(rx.try_recv().is_err());
        assert!(native_rx.try_recv().is_err());
        assert!(wm_rx.try_recv().is_err());
    }

    #[test]
    fn overview_right_drag_pans_without_native_drag() {
        let (input, mut wm_rx, mut native_rx) = input();
        let (tx, mut rx) = actor::channel();
        *input.mission_control_tx.borrow_mut() = Some(tx);
        input.mission_control_active.set(true);
        let event = CGEvent::new_mouse_event(
            None,
            CGEventType::RightMouseDragged,
            CGPoint::new(30.0, 40.0),
            objc2_core_graphics::CGMouseButton::Right,
        )
        .unwrap();
        CGEvent::set_location(Some(&event), CGPoint::new(30.0, 40.0));
        CGEvent::set_integer_value_field(Some(&event), CGEventField::MouseEventDeltaX, 25);
        for ty in [
            CGEventType::RightMouseDown,
            CGEventType::RightMouseDragged,
            CGEventType::RightMouseUp,
        ] {
            assert_eq!(
                input.on_event(ty, &event, None),
                ty == CGEventType::RightMouseDragged
            );
            if ty == CGEventType::RightMouseDragged {
                assert_eq!(CGEvent::r#type(Some(&event)), CGEventType::MouseMoved);
            }
        }
        let (
            _,
            super::super::mission_control::Event::Input(
                super::super::mission_control::Input::Scroll { delta, .. },
            ),
        ) = rx.try_recv().unwrap()
        else {
            panic!("horizontal Overview pan")
        };
        assert_eq!(delta, CGPoint::new(25.0, 0.0));
        assert!(rx.try_recv().is_err());
        assert!(native_rx.try_recv().is_err());
        assert!(wm_rx.try_recv().is_err());
    }

    #[test]
    fn shift_wheel_pans_strip_without_changing_workspace_axis() {
        let (input, _, _) = input();
        let (tx, mut rx) = actor::channel();
        *input.mission_control_tx.borrow_mut() = Some(tx);
        input.mission_control_active.set(true);
        let event = CGEvent::new_scroll_wheel_event2(
            None,
            objc2_core_graphics::CGScrollEventUnit::Pixel,
            2,
            -60,
            0,
            0,
        )
        .unwrap();
        CGEvent::set_flags(Some(&event), CGEventFlags::MaskShift);
        assert!(!input.on_event(CGEventType::ScrollWheel, &event, None));
        assert!(
            matches!(rx.try_recv().unwrap().1, super::super::mission_control::Event::Input(super::super::mission_control::Input::Scroll { delta, .. }) if delta == CGPoint::new(-60.0, 0.0))
        );
        assert_eq!(
            overview_scroll_delta(CGPoint::new(-30.0, 0.0), CGEventFlags::MaskShift),
            CGPoint::new(-30.0, 0.0)
        );
        assert_eq!(
            overview_scroll_delta(CGPoint::new(0.0, -60.0), CGEventFlags::empty()),
            CGPoint::new(0.0, -60.0)
        );
    }

    #[test]
    fn overview_consumes_scroll_and_routes_point_deltas() {
        let (input, _, _) = input();
        let (tx, mut rx) = actor::channel();
        *input.mission_control_tx.borrow_mut() = Some(tx);
        input.mission_control_active.set(true);
        let event = CGEvent::new_scroll_wheel_event2(
            None,
            objc2_core_graphics::CGScrollEventUnit::Pixel,
            2,
            -60,
            12,
            0,
        )
        .unwrap();
        CGEvent::set_location(Some(&event), CGPoint::new(30.0, 40.0));
        assert!(!input.on_event(CGEventType::ScrollWheel, &event, None));
        let (
            _,
            super::super::mission_control::Event::Input(
                super::super::mission_control::Input::Scroll { point, delta },
            ),
        ) = rx.try_recv().unwrap()
        else {
            panic!("expected Overview scroll")
        };
        assert_eq!(point, CGPoint::new(30.0, 40.0));
        assert_eq!(delta, CGPoint::new(12.0, -60.0));
        input.mission_control_active.set(false);
        assert!(input.on_event(CGEventType::ScrollWheel, &event, None));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn absent_overview_sender_safely_ignores_commands() {
        let (input, _, _) = input();
        input.mission_control_tx.borrow_mut().take();
        input.send_overview(super::super::mission_control::Event::ShowAll);
        assert!(!input.mission_control_active.get());
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
                Some(mc_tx),
                stack_line::new_shared_hit_rects(),
                Arc::default(),
            ),
            wm_rx,
            events_rx,
        )
    }

    #[test]
    fn mask_tracks_mouse_feature_enablement() {
        let (input, _, _) = input();
        assert!(input.hotkeys.borrow().is_empty());
        assert_eq!(input.desired_event_mask(), 0);
        input.state.borrow_mut().mouse_features_enabled = false;
        input.state.borrow_mut().event_processing_enabled = true;
        let stable_release_mask =
            (1u64 << CGEventType::LeftMouseUp.0) | (1u64 << CGEventType::RightMouseUp.0);
        assert_eq!(input.desired_event_mask(), stable_release_mask);
        input.state.borrow_mut().focus_follows_mouse_config_enabled = true;
        assert_eq!(
            input.desired_event_mask(),
            stable_release_mask | (1u64 << CGEventType::MouseMoved.0)
        );
        input.state.borrow_mut().mouse_settings.action2 = MouseAction::Move;
        input.state.borrow_mut().mouse_features_enabled = true;
        let mouse_mask = input.desired_event_mask();
        assert_ne!(mouse_mask & (1u64 << CGEventType::LeftMouseDown.0), 0);
        assert_ne!(mouse_mask & (1u64 << CGEventType::RightMouseDown.0), 0);
        assert_ne!(mouse_mask & (1u64 << CGEventType::LeftMouseDragged.0), 0);
        assert_ne!(mouse_mask & (1u64 << CGEventType::RightMouseDragged.0), 0);
        input.state.borrow_mut().mouse_settings.action2 = MouseAction::None;
        let left_only_mask = input.desired_event_mask();
        assert_ne!(left_only_mask & (1u64 << CGEventType::LeftMouseDown.0), 0);
        assert_eq!(left_only_mask & (1u64 << CGEventType::RightMouseDown.0), 0);
        assert_eq!(left_only_mask & (1u64 << CGEventType::RightMouseDragged.0), 0);
        input.state.borrow_mut().focus_follows_mouse_config_enabled = false;
        input.state.borrow_mut().mouse_features_enabled = false;
        input.mission_control_active.set(true);
        let mask = input.desired_event_mask();
        assert_ne!(mask & (1u64 << CGEventType::ScrollWheel.0), 0);
        assert_ne!(mask & (1u64 << CGEventType::KeyDown.0), 0);
        assert_eq!(mask & (1u64 << CGEventType::KeyUp.0), 0);
        assert_ne!(mask & (1u64 << CGEventType::RightMouseDown.0), 0);
        assert_ne!(mask & (1u64 << CGEventType::LeftMouseDragged.0), 0);
        input.mission_control_active.set(false);
        *input.disable_hotkey.borrow_mut() = Some(Hotkey::new(Modifiers::empty(), KeyCode::KeyA));
        assert_ne!(input.desired_event_mask() & (1u64 << CGEventType::KeyUp.0), 0);
        *input.disable_hotkey.borrow_mut() =
            Some(Hotkey::new(Modifiers::empty(), KeyCode::ShiftLeft));
        assert_eq!(input.desired_event_mask() & (1u64 << CGEventType::KeyUp.0), 0);
    }

    #[test]
    fn hotkey_maps_are_deferred_until_app_events_are_registered() {
        std::thread::spawn(|| {
            let (input, _, _) = input();
            assert!(!input.hotkeys_active.get());
            assert!(input.hotkeys.borrow().is_empty());

            input.hotkeys_active.set(true);
            input.rebuild_binding_maps();

            assert!(!input.hotkeys.borrow().is_empty());
            let key_mask = (1u64 << CGEventType::KeyDown.0) | (1u64 << CGEventType::FlagsChanged.0);
            assert_eq!(input.desired_event_mask() & key_mask, key_mask);
        })
        .join()
        .unwrap();
    }

    #[test]
    fn hotkeys_suppress_repeats_but_do_not_intercept_rift_synthetic_keys() {
        let (input, mut wm_rx, _) = input();
        input.hotkeys_active.set(true);
        input.rebuild_binding_maps();
        input
            .hotkeys
            .borrow_mut()
            .get_mut(0)
            .unwrap()
            .insert(Hotkey::new(Modifiers::empty(), KeyCode::KeyA), vec![
                WmCommand::Wm(wm_controller::WmCmd::ReloadConfig),
            ]);
        let event = CGEvent::new_keyboard_event(None, 0, true).unwrap();
        CGEvent::set_flags(Some(&event), CGEventFlags::empty());
        assert!(!input.on_event(CGEventType::KeyDown, &event, None));
        assert!(wm_rx.try_recv().unwrap().0.is_none());
        CGEvent::set_integer_value_field(Some(&event), CGEventField::KeyboardEventAutorepeat, 1);
        assert!(!input.on_event(CGEventType::KeyDown, &event, None));
        assert!(wm_rx.try_recv().is_err());
        CGEvent::set_integer_value_field(
            Some(&event),
            CGEventField::EventSourceUserData,
            0x5249_4654,
        );
        assert!(input.on_event(CGEventType::KeyDown, &event, None));
        assert!(wm_rx.try_recv().is_err());
    }

    #[test]
    fn binding_mode_changes_notify_and_reload_resets_to_default() {
        let (input, _, mut events) = input();
        let specs = vec![("default".into(), vec![]), ("resize".into(), vec![])];
        input.install_binding_specs(specs.clone());
        assert!(events.try_recv().is_err());
        input.transition_binding_mode("resize");
        assert!(
            matches!(events.try_recv().unwrap().1, Event::BindingModeChanged { mode } if mode == "resize")
        );
        input.transition_binding_mode("resize");
        input.transition_binding_mode("missing");
        assert!(events.try_recv().is_err());
        input.install_binding_specs(specs);
        assert!(
            matches!(events.try_recv().unwrap().1, Event::BindingModeChanged { mode } if mode == "default")
        );
        assert!(events.try_recv().is_err());
    }

    #[test]
    fn active_mode_replacement_unbinds_default_keys_and_mode_switches_are_immediate() {
        let (input, mut wm_rx, _) = input();
        let config = Config::parse(
            r#"
                [settings]
                [keys]
                "A" = "reload_config"
                "B" = { binding_mode = "resize" }
                [binding_modes.resize]
                "Escape" = { binding_mode = "default" }
            "#,
        )
        .unwrap();
        input.install_binding_specs(config.binding_mode_specs);
        input.hotkeys_active.set(true);
        input.rebuild_binding_maps();

        let b = CGEvent::new_keyboard_event(None, 11, true).unwrap();
        assert!(!input.on_event(CGEventType::KeyDown, &b, None));
        assert_eq!(input.active_mode.get(), 1);

        let a = CGEvent::new_keyboard_event(None, 0, true).unwrap();
        assert!(input.on_event(CGEventType::KeyDown, &a, None));
        assert!(wm_rx.try_recv().is_err());

        CGEvent::set_integer_value_field(Some(&b), CGEventField::KeyboardEventAutorepeat, 1);
        assert!(input.on_event(CGEventType::KeyDown, &b, None));
        assert_eq!(input.active_mode.get(), 1);

        let escape = CGEvent::new_keyboard_event(None, 53, true).unwrap();
        assert!(!input.on_event(CGEventType::KeyDown, &escape, None));
        assert_eq!(input.active_mode.get(), 0);

        assert!(!input.on_event(CGEventType::KeyDown, &a, None));
        assert!(matches!(
            wm_rx.try_recv().unwrap().1,
            WmEvent::Command(WmCommand::Wm(wm_controller::WmCmd::ReloadConfig))
        ));
        assert!(wm_rx.try_recv().is_err());
    }

    #[test]
    fn normal_command_after_binding_mode_transition_still_runs() {
        let (input, mut wm_rx, _) = input();
        input.install_binding_specs(vec![
            ("default".into(), vec![(
                "A".into(),
                WmCommand::Wm(wm_controller::WmCmd::BindingMode("other".into())),
            )]),
            ("other".into(), vec![]),
        ]);
        input.hotkeys_active.set(true);
        input.rebuild_binding_maps();
        input.hotkeys.borrow_mut()[0]
            .get_mut(&Hotkey::new(Modifiers::empty(), KeyCode::KeyA))
            .unwrap()
            .push(WmCommand::Wm(wm_controller::WmCmd::ReloadConfig));

        let a = CGEvent::new_keyboard_event(None, 0, true).unwrap();
        assert!(!input.on_event(CGEventType::KeyDown, &a, None));
        assert_eq!(input.active_mode.get(), 1);
        assert!(matches!(
            wm_rx.try_recv().unwrap().1,
            WmEvent::Command(WmCommand::Wm(wm_controller::WmCmd::ReloadConfig))
        ));
    }

    #[test]
    fn layout_rebuild_preserves_active_mode_and_rebuilds_each_map() {
        let (input, _, _) = input();
        input.install_binding_specs(vec![
            ("default".into(), vec![(
                "Ctrl + A".into(),
                WmCommand::Wm(wm_controller::WmCmd::ReloadConfig),
            )]),
            ("other".into(), vec![(
                "Ctrl + B".into(),
                WmCommand::Wm(wm_controller::WmCmd::ReloadConfig),
            )]),
        ]);
        input.hotkeys_active.set(true);
        input.active_mode.set(1);
        input.rebuild_binding_maps();
        assert_eq!(input.active_mode.get(), 1);
        let maps = input.hotkeys.borrow();
        assert!(!maps[0].is_empty());
        assert!(!maps[1].is_empty());
    }

    #[test]
    fn generic_modifiers_expand_inside_each_mode_and_replacement_resets_to_default() {
        let (input, _, _) = input();
        let specs = vec![
            ("default".into(), vec![(
                "Ctrl + A".into(),
                WmCommand::Wm(wm_controller::WmCmd::ReloadConfig),
            )]),
            ("other".into(), vec![(
                "Alt + B".into(),
                WmCommand::Wm(wm_controller::WmCmd::ReloadConfig),
            )]),
        ];
        input.install_binding_specs(specs.clone());
        input.hotkeys_active.set(true);
        input.rebuild_binding_maps();
        input.active_mode.set(1);
        input.install_binding_specs(specs);
        assert_eq!(input.active_mode.get(), 0);
        let maps = input.hotkeys.borrow();
        assert!(maps[0].keys().any(|key| {
            key.key_code == KeyCode::KeyA && !key.modifiers.has_generic_modifiers()
        }));
        assert!(maps[1].keys().any(|key| {
            key.key_code == KeyCode::KeyB && !key.modifiers.has_generic_modifiers()
        }));
    }

    #[test]
    fn releases_are_always_forwarded_when_mouse_features_are_enabled() {
        let (input, _, mut events_rx) = input();
        input.state.borrow_mut().mouse_features_enabled = false;
        let event = CGEvent::new_mouse_event(
            None,
            CGEventType::LeftMouseUp,
            CGPoint::new(20.0, 30.0),
            objc2_core_graphics::CGMouseButton::Left,
        )
        .unwrap();
        assert!(input.on_event(CGEventType::LeftMouseUp, &event, None));
        assert!(events_rx.try_recv().is_err());
        input.state.borrow_mut().mouse_features_enabled = true;
        assert!(input.on_event(CGEventType::LeftMouseUp, &event, None));
        assert!(matches!(
            events_rx.try_recv().unwrap().1,
            Event::MouseUp(crate::actor::drag::MouseButton::Left)
        ));
        assert!(input.on_event(CGEventType::LeftMouseUp, &event, None));
        assert!(matches!(
            events_rx.try_recv().unwrap().1,
            Event::MouseUp(crate::actor::drag::MouseButton::Left)
        ));
    }

    #[test]
    fn non_owning_release_does_not_clear_the_captured_button() {
        let (input, _, mut events_rx) = input();
        {
            let mut state = input.state.borrow_mut();
            state.mouse_features_enabled = true;
            state.captured_button = Some(crate::actor::drag::MouseButton::Left);
        }
        let right_up = CGEvent::new_mouse_event(
            None,
            CGEventType::RightMouseUp,
            CGPoint::new(20.0, 30.0),
            objc2_core_graphics::CGMouseButton::Right,
        )
        .unwrap();
        assert!(input.on_event(CGEventType::RightMouseUp, &right_up, None));
        assert_eq!(
            input.state.borrow().captured_button,
            Some(crate::actor::drag::MouseButton::Left)
        );
        assert!(matches!(
            events_rx.try_recv().unwrap().1,
            Event::MouseUp(crate::actor::drag::MouseButton::Right)
        ));
    }

    #[test]
    fn focus_follows_mouse_reset_keeps_a_captured_drag() {
        let (input, _, _) = input();
        let mut state = input.state.borrow_mut();
        state.captured_button = Some(crate::actor::drag::MouseButton::Left);
        state.reset(false);
        state.reset(true);
        assert_eq!(
            state.captured_button,
            Some(crate::actor::drag::MouseButton::Left)
        );
    }

    #[test]
    fn drag_motion_uses_rewritten_event_position() {
        for (event_type, button, cg_button) in [
            (
                CGEventType::LeftMouseDragged,
                crate::actor::drag::MouseButton::Left,
                objc2_core_graphics::CGMouseButton::Left,
            ),
            (
                CGEventType::RightMouseDragged,
                crate::actor::drag::MouseButton::Right,
                objc2_core_graphics::CGMouseButton::Right,
            ),
        ] {
            let (input, _, _) = input();
            input.state.borrow_mut().captured_button = Some(button);
            let event =
                CGEvent::new_mouse_event(None, event_type, CGPoint::new(99.0, 20.0), cg_button)
                    .unwrap();
            let target = CGPoint::new(6.0, 920.0);
            // Horizontal warping rewrites the event before the drag publisher sees it.
            CGEvent::set_location(Some(&event), target);
            assert!(!input.on_event(event_type, &event, None));
            assert_eq!(input.drag_motion_publisher.take_latest().unwrap().point, target);
        }
    }

    #[test]
    fn drag_motion_publishes_only_for_captured_or_native_drags() {
        let (input, _, mut events_rx) = input();
        let event = CGEvent::new_mouse_event(
            None,
            CGEventType::LeftMouseDragged,
            CGPoint::new(20.0, 30.0),
            objc2_core_graphics::CGMouseButton::Left,
        )
        .unwrap();
        assert!(input.on_event(CGEventType::LeftMouseDragged, &event, None));
        assert!(events_rx.try_recv().is_err());

        input.state.borrow_mut().captured_button = Some(crate::actor::drag::MouseButton::Left);
        assert!(!input.on_event(CGEventType::LeftMouseDragged, &event, None));
        assert!(matches!(
            events_rx.try_recv().unwrap().1,
            Event::DragMotionPending(_)
        ));
        input.drag_motion_publisher.take_latest();

        input.state.borrow_mut().captured_button = None;
        input.native_motion_active.store(true, Ordering::Release);
        assert!(input.on_event(CGEventType::LeftMouseDragged, &event, None));
        assert!(matches!(
            events_rx.try_recv().unwrap().1,
            Event::DragMotionPending(_)
        ));
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

impl Input {
    fn native_gesture_forward(
        &self,
        ty: CGEventType,
        event: &CGEvent,
        proxy: Option<CGEventTapProxy>,
    ) -> bool {
        let mut filter = self.gesture_filter.borrow_mut();
        filter.forward(
            ty,
            event,
            || self.gesture_control.ownership_guard(),
            |held| {
                if let Some(proxy) = proxy {
                    // The proxy is valid only during this tap callback. Held events
                    // must reach downstream taps before the current event returns.
                    unsafe { CGEvent::tap_post_event(proxy, Some(held)) };
                } else {
                    CGEvent::post(CGTapLoc::SessionEventTap, Some(held));
                }
            },
        )
    }

    fn reset_gestures(&self) {
        self.gesture_control.reset(&self.events_tx);
        self.gesture_filter.borrow_mut().reset();
    }
}
