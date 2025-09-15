use std::cell::RefCell;
use std::ffi::c_void;
use std::rc::Rc;
use std::sync::Arc;

use objc2_app_kit::{NSEvent, NSEventType, NSTouchPhase};
use objc2_core_graphics::{self as ocg, CGEvent, CGEventMask};
use tracing::trace;

use crate::actor;
use crate::actor::reactor;
use crate::actor::wm_controller::{self, WmCommand, WmEvent};
use crate::common::config::{Config, HapticPattern};
use crate::layout_engine::LayoutCommand as LC;

#[derive(Debug, Clone)]
pub struct SwipeConfig {
    pub enabled: bool,
    pub invert_horizontal: bool,
    pub vertical_tolerance: f64,
    pub skip_empty_workspaces: Option<bool>,
    pub fingers: usize,
    pub distance_pct: f64,
    pub haptics_enabled: bool,
    pub haptic_pattern: HapticPattern,
}

impl SwipeConfig {
    pub fn from_config(config: &Config) -> Self {
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

pub struct Swipe {
    cfg: SwipeConfig,
    wm_sender: wm_controller::Sender,
    tap: RefCell<Option<crate::sys::event_tap::EventTap>>,
    requests_rx: Option<Receiver>,
}

impl Swipe {
    pub fn new(
        config: Arc<Config>,
        wm_sender: wm_controller::Sender,
        requests_rx: Receiver,
    ) -> Option<Self> {
        let cfg = SwipeConfig::from_config(&config);
        if !cfg.enabled {
            return None;
        }
        Some(Self {
            cfg,
            wm_sender,
            tap: RefCell::new(None),
            requests_rx: Some(requests_rx),
        })
    }

    pub async fn run(mut self) {
        let mut requests_rx = match self.requests_rx.take() {
            Some(rx) => rx,
            None => return,
        };

        let this = Rc::new(self);
        let mask: CGEventMask = 1u64 << (NSEventType::Gesture.0 as u64);

        let ctx = Box::new(CallbackCtx {
            swipe: Rc::clone(&this),
            state: RefCell::new(SwipeState::default()),
        });
        let ctx_ptr = Box::into_raw(ctx) as *mut c_void;

        unsafe fn drop_ctx(ptr: *mut c_void) {
            unsafe { drop(Box::from_raw(ptr as *mut CallbackCtx)) };
        }

        let tap = unsafe {
            crate::sys::event_tap::EventTap::new_listen_only(
                mask,
                Some(gesture_callback),
                ctx_ptr,
                Some(drop_ctx),
            )
        };

        if let Some(tap) = tap {
            *this.tap.borrow_mut() = Some(tap);
        } else {
            unsafe { drop(Box::from_raw(ctx_ptr as *mut CallbackCtx)) };
            return;
        }

        while let Some((_span, request)) = requests_rx.recv().await {
            match request {
                Request::Stop => {
                    this.teardown_tap();
                    break;
                }
            }
        }

        this.teardown_tap();
    }
}

#[derive(Default, Debug)]
struct SwipeState {
    state: GesturePhase,
    start_x: f64,
    start_y: f64,
}

impl SwipeState {
    fn reset(&mut self) {
        self.state = GesturePhase::Idle;
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

struct CallbackCtx {
    swipe: Rc<Swipe>,
    state: RefCell<SwipeState>,
}

unsafe extern "C-unwind" fn gesture_callback(
    _proxy: ocg::CGEventTapProxy,
    event_type: ocg::CGEventType,
    event_ref: core::ptr::NonNull<ocg::CGEvent>,
    user_info: *mut c_void,
) -> *mut ocg::CGEvent {
    let ctx = unsafe { &*(user_info as *const CallbackCtx) };

    let ety = event_type.0 as i64;
    if ety == -1 || ety == -2 {
        if let Some(tap) = ctx.swipe.tap.borrow().as_ref() {
            tap.set_enabled(true);
        }
        return event_ref.as_ptr();
    }

    let cg_event: &CGEvent = unsafe { event_ref.as_ref() };
    if let Some(nsevent) = unsafe { NSEvent::eventWithCGEvent(cg_event) } {
        handle_gesture(&ctx.swipe, &ctx.state, &nsevent);
    }

    event_ref.as_ptr()
}

fn handle_gesture(swipe: &Swipe, state: &RefCell<SwipeState>, nsevent: &NSEvent) {
    let touches = unsafe { nsevent.allTouches() };
    let mut sum_x = 0.0f64;
    let mut sum_y = 0.0f64;
    let mut any_ended = false;
    let mut touch_count = 0usize;
    let mut active_count = 0usize;
    let mut too_many_touches = false;

    for t in touches.iter() {
        let phase = unsafe { t.phase() };
        if phase.contains(NSTouchPhase::Stationary) {
            continue;
        }

        touch_count += 1;
        if touch_count > swipe.cfg.fingers {
            too_many_touches = true;
            break;
        }

        let ended = phase.contains(NSTouchPhase::Ended) || phase.contains(NSTouchPhase::Cancelled);
        any_ended |= ended;

        if !ended {
            let pos = unsafe { t.normalizedPosition() };
            sum_x += pos.x as f64;
            sum_y += pos.y as f64;
            active_count += 1;
        }
    }

    if too_many_touches || touch_count != swipe.cfg.fingers {
        state.borrow_mut().reset();
        return;
    }

    if active_count == 0 {
        state.borrow_mut().reset();
        return;
    }

    let avg_x = sum_x / active_count as f64;
    let avg_y = sum_y / active_count as f64;

    let mut st = state.borrow_mut();
    match st.state {
        GesturePhase::Idle => {
            st.start_x = avg_x;
            st.start_y = avg_y;
            st.state = GesturePhase::Armed;
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

            if horizontal >= swipe.cfg.distance_pct && vertical <= swipe.cfg.vertical_tolerance {
                let mut dir_left = dx < 0.0;
                if swipe.cfg.invert_horizontal {
                    dir_left = !dir_left;
                }
                let cmd = if dir_left {
                    LC::NextWorkspace(swipe.cfg.skip_empty_workspaces)
                } else {
                    LC::PrevWorkspace(swipe.cfg.skip_empty_workspaces)
                };

                if swipe.cfg.haptics_enabled {
                    let _ = crate::sys::haptics::perform_haptic(swipe.cfg.haptic_pattern);
                }
                swipe.wm_sender.send(WmEvent::Command(WmCommand::ReactorCommand(
                    reactor::Command::Layout(cmd),
                )));
                st.state = GesturePhase::Committed;
            }
        }
        GesturePhase::Committed => {
            if any_ended {
                st.reset();
            }
        }
    }
}

#[derive(Debug)]
pub enum Request {
    Stop,
}

pub type Sender = actor::Sender<Request>;
pub type Receiver = actor::Receiver<Request>;

impl Swipe {
    fn teardown_tap(&self) { self.tap.borrow_mut().take(); }
}
