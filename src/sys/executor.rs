//! A simple async executor that integrates with CFRunLoop.

use std::cell::RefCell;
use std::future::Future;
use std::mem;
use std::pin::Pin;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use parking_lot::Mutex;
use std::task::{Context, Poll, Wake};

use core_foundation::runloop::CFRunLoop;

use super::run_loop::WakeupHandle;

thread_local! {
    static HANDLE: Handle = Handle::new();
}

pub struct Executor;

impl Executor {
    pub fn run(task: impl Future<Output = ()>) {
        let task: Pin<Box<dyn Future<Output = ()> + '_>> = Box::pin(task);
        // Extend the lifetime.
        // Safety: We only poll the task within this function, then it is dropped.
        let task: Pin<Box<dyn Future<Output = ()> + 'static>> = unsafe { mem::transmute(task) };

        HANDLE.with(move |handle| {
            // Ensure we drop the main task, even on unwind.
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
                CFRunLoop::run_current();
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
                wakeup: Arc::new(WakerImpl(Mutex::new(wakeup))),
                main_task: None,
            };
            RefCell::new(state)
        }))
    }
}

struct State {
    wakeup: Arc<WakerImpl>,
    main_task: Option<Pin<Box<dyn Future<Output = ()>>>>,
}

impl State {
    fn process_tasks(&mut self) {
        let waker = self.wakeup.clone().into();
        let mut context = Context::from_waker(&waker);
        if self.main_task.as_mut().unwrap().as_mut().poll(&mut context) == Poll::Ready(()) {
            self.main_task.take();
            CFRunLoop::get_current().stop();
        }
    }
}

struct WakerImpl(Mutex<WakeupHandle>);

impl Wake for WakerImpl {
    fn wake(self: Arc<Self>) { self.0.lock().wake(); }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::panic::{AssertUnwindSafe, catch_unwind};
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

        let mut x = 0;
        Executor::run(async {
            x += 1;
            PendingThenReady::default().await;
            x += 1;
        });
        assert_eq!(2, x);
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

        let mut msgs = 0;
        Executor::run(async {
            while let Some(_msg) = rx.recv().await {
                msgs += 1;
                PendingThenReady::default().await;
            }
        });

        assert_eq!(2, msgs);
    }
}
