//! Display cadence for a locked screen. AppKit objects stay on the main thread.
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use dispatchr::queue;
use dispatchr::time::Time;
use objc2::rc::Retained;
use objc2::{AnyThread, DeclaredClass, MainThreadMarker, define_class, msg_send, sel};
use objc2_app_kit::NSScreen;
use objc2_foundation::{NSObject, NSObjectProtocol, NSRunLoop, NSRunLoopCommonModes};
use objc2_quartz_core::CADisplayLink;
use tokio::sync::Notify;

use super::dispatch::DispatchExt;
use super::screen::{NSScreenExt, ScreenId};

thread_local! {
    static LINKS: RefCell<HashMap<u64, Retained<CADisplayLink>>> = RefCell::new(HashMap::new());
}
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

struct State {
    notify: Arc<Notify>,
    cancelled: AtomicBool,
    running: AtomicBool,
}

define_class! {
    // NSObject has no subclassing requirements; the selector matches CADisplayLink's callback.
    #[unsafe(super(NSObject))]
    #[ivars = Arc<State>]
    struct DisplayLinkTarget;

    impl DisplayLinkTarget {
        #[unsafe(method(tick:))]
        fn tick(&self, link: &CADisplayLink) {
            let state = self.ivars();
            if state.cancelled.load(Ordering::Acquire) {
                link.invalidate();
                return;
            }
            state.running.store(true, Ordering::Release);
            state.notify.notify_one(); // One outstanding permit, independent of callback rate.
        }
    }
}

/// Sendable lifetime handle; the retained link and its run-loop operations never leave main.
pub struct DisplayLink {
    id: u64,
    state: Arc<State>,
}

impl DisplayLink {
    pub fn for_display(display: u32, notify: Arc<Notify>) -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let state = Arc::new(State {
            notify,
            cancelled: AtomicBool::new(false),
            running: AtomicBool::new(false),
        });
        queue::main().after_f_s(
            Time::NOW,
            (id, display, state.clone()),
            |(id, display, state)| {
                if state.cancelled.load(Ordering::Acquire) {
                    return;
                }
                let mtm = MainThreadMarker::new().expect("main dispatch queue");
                let Some(screen) = NSScreen::screens(mtm)
                    .iter()
                    .find(|screen| screen.get_number() == Ok(ScreenId::new(display)))
                else {
                    return;
                };
                // Older macOS releases keep the existing timer fallback.
                if !screen.respondsToSelector(sel!(displayLinkWithTarget:selector:)) {
                    return;
                }
                let target = DisplayLinkTarget::alloc().set_ivars(state);
                let target: Retained<DisplayLinkTarget> = unsafe { msg_send![super(target), init] };
                // The target is retained by the link. It does not retain the link in return.
                let link = unsafe { screen.displayLinkWithTarget_selector(&target, sel!(tick:)) };
                unsafe {
                    link.addToRunLoop_forMode(&NSRunLoop::mainRunLoop(), NSRunLoopCommonModes)
                };
                LINKS.with(|links| {
                    links.borrow_mut().insert(id, link);
                });
            },
        );
        Self { id, state }
    }

    /// Until the first native callback, the consumer can use its timer fallback.
    pub fn is_running(&self) -> bool { self.state.running.load(Ordering::Acquire) }
}

impl Drop for DisplayLink {
    fn drop(&mut self) {
        self.state.cancelled.store(true, Ordering::Release);
        queue::main().after_f_s(Time::NOW, self.id, |id| {
            LINKS.with(|links| {
                if let Some(link) = links.borrow_mut().remove(&id) {
                    link.invalidate();
                }
            });
        });
    }
}
