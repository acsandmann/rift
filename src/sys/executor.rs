//! A simple async executor that integrates with CFRunLoop.

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::task::{Context, Poll, Wake};

use objc2::MainThreadMarker;
use objc2_app_kit::NSApp;
use objc2_core_foundation::CFRunLoop;

use super::run_loop::WakeupHandle;

thread_local! {
    static HANDLE: Handle = Handle::new();
}

pub struct Executor;

pub struct Session;

impl Drop for Session {
    fn drop(&mut self) {
        HANDLE.with(|handle| {
            handle.0.borrow_mut().main_task.take();
        });
    }
}

impl Executor {
    pub fn run(task: impl Future<Output = ()> + 'static) {
        Self::run_with_loop_fn(task, CFRunLoop::run);
    }

    pub fn run_main(mtm: MainThreadMarker, task: impl Future<Output = ()> + 'static) {
        // In macOS some events do not fire unless we call this function.
        // https://github.com/koekeishiya/yabai/issues/2680
        Self::run_with_loop_fn(task, || NSApp(mtm).run());
    }

    fn run_with_loop_fn(task: impl Future<Output = ()> + 'static, loop_fn: impl Fn()) {
        let task: Pin<Box<dyn Future<Output = ()> + 'static>> = Box::pin(task);

        HANDLE.with(move |handle| {
            struct Guard;
            impl Drop for Guard {
                fn drop(&mut self) {
                    HANDLE.with(|handle| {
                        handle.0.borrow_mut().main_task.take();
                    })
                }
            }
            let _guard = Guard;

            {
                let mut state = handle.0.borrow_mut();
                state.main_task.replace(task);
                state.wakeup.wake_by_ref();
            }

            while handle.0.borrow().main_task.is_some() {
                // Run the loop until it is stopped by process_tasks below.
                // We do this in a loop just in case there were "spurious"
                // stops by some other code.
                loop_fn();
            }
        })
    }
}

struct Handle(Rc<RefCell<State>>);

impl Handle {
    fn new() -> Self {
        Handle(Rc::new_cyclic(|weak: &Weak<RefCell<State>>| {
            let weak = weak.clone();
            let wakeup = WakeupHandle::for_current_thread(0, move || {
                if let Some(this) = weak.upgrade() {
                    this.borrow_mut().process_tasks();
                }
            });
            let state = State {
                wakeup: Arc::new(WakerImpl(wakeup)),
                main_task: None,
            };
            RefCell::new(state)
        }))
    }
}

struct State {
    wakeup: Arc<WakerImpl>,
    main_task: Option<Pin<Box<dyn Future<Output = ()> + 'static>>>,
}

impl State {
    fn process_tasks(&mut self) {
        let waker = self.wakeup.clone().into();
        let mut context = Context::from_waker(&waker);

        if self.main_task.as_mut().unwrap().as_mut().poll(&mut context) == Poll::Ready(()) {
            self.main_task.take();
            if let Some(rl) = CFRunLoop::current() {
                rl.stop();
            }
        }
    }
}

struct WakerImpl(WakeupHandle);

impl Wake for WakerImpl {
    fn wake(self: Arc<Self>) { self.0.wake(); }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::rc::Rc;
    use std::time::Duration;
    use std::{future, thread};

    use super::*;

    #[derive(Default)]
    struct PendingThenReady(bool);

    impl Future for PendingThenReady {
        type Output = ();

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            if self.0 {
                return Poll::Ready(());
            }
            self.0 = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }

    #[test]
    fn executor_runs() {
        Executor::run(future::ready(()));
        Executor::run(PendingThenReady::default());

        let x = Rc::new(Cell::new(0));
        let x2 = x.clone();
        Executor::run(async move {
            x2.set(x2.get() + 1);
            PendingThenReady::default().await;
            x2.set(x2.get() + 1);
        });
        assert_eq!(2, x.get());
    }

    #[test]
    fn executor_drops_main_task_on_unwind() {
        struct SignallingDrop(AssertUnwindSafe<Rc<Cell<bool>>>);
        impl Drop for SignallingDrop {
            fn drop(&mut self) { self.0.replace(true); }
        }

        let dropped = Rc::new(Cell::new(false));

        let dropper = SignallingDrop(AssertUnwindSafe(dropped.clone()));
        let result = catch_unwind(|| {
            Executor::run(async move {
                let _dropper = dropper;
                PendingThenReady::default().await;
                panic!("oh no");
            });
        });

        assert!(result.is_err());
        assert_eq!(true, dropped.take());
    }

    #[test]
    fn channel_works() {
        use tokio::sync::mpsc;

        let (tx, mut rx) = mpsc::unbounded_channel();

        thread::spawn(move || {
            thread::sleep(Duration::from_millis(25));
            _ = tx.send(());
            _ = tx.send(());
            drop(tx);
        });

        let msgs = Rc::new(Cell::new(0));
        let msgs2 = msgs.clone();

        Executor::run(async move {
            while let Some(_msg) = rx.recv().await {
                msgs2.set(msgs2.get() + 1);
                PendingThenReady::default().await;
            }
        });

        assert_eq!(2, msgs.get());
    }
}
