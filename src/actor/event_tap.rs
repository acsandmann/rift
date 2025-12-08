use std::cell::RefCell;
use std::mem::replace;
use std::rc::Rc;

use objc2_app_kit::{
    NSEvent, NSEventPhase, NSEventType, NSMainMenuWindowLevel, NSPopUpMenuWindowLevel,
    NSTouchPhase, NSTouchType, NSWindowLevel,
};
use objc2_core_foundation::{CGPoint, CGRect};
use objc2_core_graphics::{
    CGEvent, CGEventFlags, CGEventMask, CGEventTapOptions as CGTapOpt, CGEventTapProxy, CGEventType,
};
use tracing::{debug, error, trace, warn};

use super::reactor::{self, Event};
use crate::actor;
use crate::actor::wm_controller::{self, WmCommand, WmEvent};
use crate::common::collections::{HashMap, HashSet};
use crate::common::config::{Config, HapticPattern};
use crate::common::log::trace_misc;
use crate::layout_engine::LayoutCommand as LC;
use crate::sys::event::{self, Hotkey, KeyCode, MouseState, set_mouse_state};
use crate::sys::geometry::CGRectExt;
use crate::sys::haptics;
use crate::sys::hotkey::{
    Modifiers, is_modifier_key, key_code_from_event, modifier_flag_for_key,
    modifiers_from_flags_with_keys,
};
use crate::sys::screen::CoordinateConverter;
use crate::sys::window_server::{self, WindowServerId, window_level};

#[derive(Debug)]
pub enum Request {
    Warp(CGPoint),
    EnforceHidden,
    ScreenParametersChanged(Vec<CGRect>, CoordinateConverter),
    SetEventProcessing(bool),
    SetFocusFollowsMouseEnabled(bool),
    SetHotkeys(Vec<(Hotkey, WmCommand)>),
}

pub struct EventTap {
    config: Config,
    events_tx: reactor::Sender,
    requests_rx: Option<Receiver>,
    state: RefCell<State>,
    tap: RefCell<Option<crate::sys::event_tap::EventTap>>,
    disable_hotkey: Option<Hotkey>,
    swipe: Option<SwipeHandler>,
    hotkeys: RefCell<HashMap<Hotkey, Vec<WmCommand>>>,
    wm_sender: Option<wm_controller::Sender>,
}

struct State {
    hidden: bool,
    above_window: Option<WindowServerId>,
    above_window_level: NSWindowLevel,
    converter: CoordinateConverter,
    screens: Vec<CGRect>,
    event_processing_enabled: bool,
    focus_follows_mouse_enabled: bool,
    disable_hotkey_active: bool,
    pressed_keys: HashSet<KeyCode>,
    current_flags: CGEventFlags,
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
            focus_follows_mouse_enabled: true,
            disable_hotkey_active: false,
            pressed_keys: HashSet::default(),
            current_flags: CGEventFlags::empty(),
        }
    }
}

pub type Sender = actor::Sender<Request>;
pub type Receiver = actor::Receiver<Request>;

struct CallbackCtx {
    this: Rc<EventTap>,
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

unsafe fn drop_mouse_ctx(ptr: *mut std::ffi::c_void) {
    unsafe { drop(Box::from_raw(ptr as *mut CallbackCtx)) };
}

impl EventTap {
    pub fn new(
        config: Config,
        events_tx: reactor::Sender,
        requests_rx: Receiver,
        wm_sender: Option<wm_controller::Sender>,
    ) -> Self {
        let disable_hotkey = config
            .settings
            .focus_follows_mouse_disable_hotkey
            .clone()
            .and_then(|spec| spec.to_hotkey());
        let swipe_cfg = SwipeConfig::from_config(&config);
        let swipe = if swipe_cfg.enabled && wm_sender.is_some() {
            Some(SwipeHandler {
                cfg: swipe_cfg,
                state: RefCell::new(SwipeState::default()),
            })
        } else {
            None
        };
        EventTap {
            config,
            events_tx,
            requests_rx: Some(requests_rx),
            state: RefCell::new(State::default()),
            tap: RefCell::new(None),
            disable_hotkey,
            swipe,
            hotkeys: RefCell::new(HashMap::default()),
            wm_sender,
        }
    }

    pub async fn run(mut self) {
        let mut requests_rx = self.requests_rx.take().unwrap();

        let this = Rc::new(self);

        let mask = build_event_mask(this.swipe.is_some());

        let ctx = Box::new(CallbackCtx { this: Rc::clone(&this) });
        let ctx_ptr = Box::into_raw(ctx) as *mut std::ffi::c_void;

        let tap = unsafe {
            crate::sys::event_tap::EventTap::new_with_options(
                CGTapOpt::Default,
                mask,
                Some(mouse_callback),
                ctx_ptr,
                Some(drop_mouse_ctx),
            )
        };

        if let Some(tap) = tap {
            *this.tap.borrow_mut() = Some(tap);
        } else {
            unsafe { drop(Box::from_raw(ctx_ptr as *mut CallbackCtx)) };
            return;
        }

        if this.config.settings.mouse_hides_on_focus {
            if let Err(e) = window_server::allow_hide_mouse() {
                error!(
                    "Could not enable mouse hiding: {e:?}. \
                    mouse_hides_on_focus will have no effect."
                );
            }
        }

        while let Some((span, request)) = requests_rx.recv().await {
            let _ = span.enter();
            this.on_request(request);
        }
    }

    fn on_request(self: &Rc<Self>, request: Request) {
        let mut state = self.state.borrow_mut();
        match request {
            Request::Warp(point) => {
                if let Err(e) = event::warp_mouse(point) {
                    warn!("Failed to warp mouse: {e:?}");
                } else {
                    state.above_window = None;
                    state.above_window_level = NSWindowLevel::MIN;
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
            Request::SetFocusFollowsMouseEnabled(enabled) => {
                debug!(
                    "focus_follows_mouse temporarily {}",
                    if enabled { "enabled" } else { "disabled" }
                );
                state.focus_follows_mouse_enabled = enabled;
            }
            Request::SetHotkeys(bindings) => {
                let mut map = self.hotkeys.borrow_mut();
                map.clear();
                for (hotkey, command) in bindings {
                    if hotkey.modifiers.has_generic_modifiers() {
                        for expanded_mods in hotkey.modifiers.expand_to_specific() {
                            let expanded_hotkey = Hotkey::new(expanded_mods, hotkey.key_code);
                            let entry = map.entry(expanded_hotkey).or_default();
                            if !entry.contains(&command) {
                                entry.push(command.clone());
                            }
                        }
                    } else {
                        let entry = map.entry(hotkey).or_default();
                        if !entry.contains(&command) {
                            entry.push(command);
                        }
                    }
                }
                debug!("Updated hotkey bindings: {}", map.len());
            }
        }
    }

    fn on_event(self: &Rc<Self>, event_type: CGEventType, event: &CGEvent) -> bool {
        if event_type.0 == NSEventType::Gesture.0 as u32 {
            if let Some(handler) = &self.swipe {
                if let Some(nsevent) = NSEvent::eventWithCGEvent(event)
                    && nsevent.r#type() == NSEventType::Gesture
                {
                    self.handle_gesture_event(handler, &nsevent);
                }
            }
            return true;
        }

        match event_type {
            CGEventType::LeftMouseDown | CGEventType::RightMouseDown => {
                set_mouse_state(MouseState::Down);
            }
            CGEventType::LeftMouseDragged | CGEventType::RightMouseDragged => {
                set_mouse_state(MouseState::Down);
            }
            CGEventType::LeftMouseUp | CGEventType::RightMouseUp => set_mouse_state(MouseState::Up),
            _ => {}
        }

        let mut state = self.state.borrow_mut();

        if matches!(
            event_type,
            CGEventType::KeyDown | CGEventType::KeyUp | CGEventType::FlagsChanged
        ) {
            return self.handle_keyboard_event(event_type, event, &mut state);
        }

        if !state.event_processing_enabled {
            trace!("Mouse event processing disabled, ignoring {:?}", event_type);
            return true;
        }

        if state.hidden {
            debug!("Showing mouse");
            if let Err(e) = event::show_mouse() {
                warn!("Failed to show mouse: {e:?}");
            }
            state.hidden = false;
        }
        match event_type {
            CGEventType::RightMouseUp | CGEventType::LeftMouseUp => {
                _ = self.events_tx.send(Event::MouseUp);
            }
            CGEventType::MouseMoved
                if self.config.settings.focus_follows_mouse
                    && state.focus_follows_mouse_enabled
                    && !state.disable_hotkey_active =>
            {
                let loc = CGEvent::location(Some(event));
                if let Some(wsid) = state.track_mouse_move(loc) {
                    _ = self.events_tx.send(Event::MouseMovedOverWindow(wsid));
                }
            }
            _ => (),
        }

        true
    }

    fn handle_gesture_event(&self, handler: &SwipeHandler, nsevent: &NSEvent) {
        let cfg = &handler.cfg;
        let state = &handler.state;
        let Some(wm_sender) = self.wm_sender.as_ref() else {
            state.borrow_mut().reset();
            return;
        };

        let mut st = state.borrow_mut();

        let phase = nsevent.phase();
        if [
            NSEventPhase::Ended,
            NSEventPhase::Cancelled,
            NSEventPhase::Began,
        ]
        .contains(&phase)
        {
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

            if !ended && t.r#type() == NSTouchType::Indirect {
                let pos = t.normalizedPosition();
                sum_x += pos.x as f64;
                sum_y += pos.y as f64;
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
                    wm_sender.send(WmEvent::Command(WmCommand::ReactorCommand(
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

    fn handle_keyboard_event(
        &self,
        event_type: CGEventType,
        event: &CGEvent,
        state: &mut State,
    ) -> bool {
        let key_code_opt = key_code_from_event(event);

        if let Some(key_code) = key_code_opt {
            match event_type {
                CGEventType::KeyDown => state.note_key_down(key_code),
                CGEventType::KeyUp => state.note_key_up(key_code),
                CGEventType::FlagsChanged => state.note_flags_changed(key_code),
                _ => {}
            }
        }

        let flags = CGEvent::flags(Some(event));
        state.current_flags = flags;

        if let Some(target) = &self.disable_hotkey {
            let prev_active = state.disable_hotkey_active;
            state.disable_hotkey_active = state.compute_disable_hotkey_active(target.clone());
            if state.disable_hotkey_active != prev_active {
                if state.disable_hotkey_active {
                    debug!(?target, "focus_follows_mouse disabled while hotkey held");
                } else {
                    debug!(?target, "focus_follows_mouse re-enabled after hotkey release");
                }
            }
        }

        if event_type == CGEventType::KeyDown {
            if let Some(key_code) = key_code_opt {
                let hotkey = Hotkey::new(
                    modifiers_from_flags_with_keys(state.current_flags, &state.pressed_keys),
                    key_code,
                );
                let commands = {
                    let bindings = self.hotkeys.borrow();
                    bindings.get(&hotkey).cloned()
                };
                if let Some(commands) = commands {
                    if let Some(wm_sender) = &self.wm_sender {
                        for cmd in commands {
                            wm_sender.send(WmEvent::Command(cmd));
                        }
                        return false;
                    } else {
                        debug!(?hotkey, "Hotkey triggered but no WM sender available");
                    }
                }
            }
        }

        true
    }
}

unsafe extern "C-unwind" fn mouse_callback(
    _proxy: CGEventTapProxy,
    event_type: CGEventType,
    event_ref: core::ptr::NonNull<CGEvent>,
    user_info: *mut std::ffi::c_void,
) -> *mut CGEvent {
    let ctx = unsafe { &*(user_info as *const CallbackCtx) };

    let event = unsafe { event_ref.as_ref() };
    if ctx.this.on_event(event_type, event) {
        event_ref.as_ptr()
    } else {
        core::ptr::null_mut()
    }
}

impl State {
    fn note_key_down(&mut self, key_code: KeyCode) { self.pressed_keys.insert(key_code); }

    fn note_key_up(&mut self, key_code: KeyCode) { self.pressed_keys.remove(&key_code); }

    fn note_flags_changed(&mut self, key_code: KeyCode) {
        if is_modifier_key(key_code) {
            self.pressed_keys.remove(&key_code);
        }
    }

    fn compute_disable_hotkey_active(&self, target: Hotkey) -> bool {
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
            modifier_flag_for_key(key_code)
                .map(|flag| self.current_flags.contains(flag))
                .unwrap_or(false)
        } else {
            self.pressed_keys.contains(&key_code)
        }
    }

    fn track_mouse_move(&mut self, loc: CGPoint) -> Option<WindowServerId> {
        let new_window = window_server::get_window_at_point(loc);
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
            .and_then(|id| trace_misc("window_level", || window_level(id.into())))
            .unwrap_or(NSWindowLevel::MIN);
        let old_window_level = replace(&mut self.above_window_level, new_window_level);
        debug!(?old_window, ?old_window_level, ?new_window, ?new_window_level);

        if old_window_level >= NSPopUpMenuWindowLevel {
            return None;
        }

        if !(0..NSPopUpMenuWindowLevel).contains(&new_window_level)
            && new_window_level != NSWindowLevel::MIN
        {
            return None;
        }

        new_window
    }
}

fn build_event_mask(swipe_enabled: bool) -> CGEventMask {
    let mut m: u64 = 0;
    let add = |m: &mut u64, ty: CGEventType| *m |= 1u64 << (ty.0 as u64);

    for ty in [
        CGEventType::LeftMouseDown,
        CGEventType::LeftMouseUp,
        CGEventType::RightMouseDown,
        CGEventType::RightMouseUp,
        CGEventType::MouseMoved,
        CGEventType::LeftMouseDragged,
        CGEventType::RightMouseDragged,
    ] {
        add(&mut m, ty);
    }
    for ty in [
        CGEventType::KeyDown,
        CGEventType::KeyUp,
        CGEventType::FlagsChanged,
    ] {
        add(&mut m, ty);
    }
    if swipe_enabled {
        // NSEventType::Gesture is an NSEventType — it maps via .0
        *&mut m |= 1u64 << (NSEventType::Gesture.0 as u64);
    }
    m
}
