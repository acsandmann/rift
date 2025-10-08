use std::sync::Arc;
use std::sync::atomic::{AtomicPtr, Ordering};

pub struct SwapArc<T> {
    ptr: AtomicPtr<T>,
}

impl<T> SwapArc<T> {
    pub fn new(initial: Arc<T>) -> Self {
        let raw = Arc::into_raw(initial) as *mut T;
        Self { ptr: AtomicPtr::new(raw) }
    }

    pub fn from_value(value: T) -> Self { Self::new(Arc::new(value)) }

    #[inline]
    pub fn load(&self) -> Arc<T> {
        let p = self.ptr.load(Ordering::Acquire);
        assert!(!p.is_null(), "SwapArc pointer was null");
        unsafe {
            Arc::increment_strong_count(p);
            Arc::from_raw(p)
        }
    }

    #[inline]
    pub fn store(&self, new_val: Arc<T>) {
        let newp = Arc::into_raw(new_val) as *mut T;
        let oldp = self.ptr.swap(newp, Ordering::AcqRel);
        unsafe {
            drop(Arc::from_raw(oldp));
        }
    }

    #[inline]
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        let a = self.load();
        f(&a)
    }
}

impl<T> Drop for SwapArc<T> {
    fn drop(&mut self) {
        let p = self.ptr.load(Ordering::Relaxed);
        if !p.is_null() {
            unsafe {
                drop(Arc::from_raw(p));
            }
        }
    }
}

unsafe impl<T: Send + Sync> Send for SwapArc<T> {}
unsafe impl<T: Send + Sync> Sync for SwapArc<T> {}

impl<T> SwapArc<T> {
    pub fn update_cas<F>(&self, mut f: F)
    where F: FnMut(&T) -> Arc<T> {
        loop {
            let cur_ptr = self.ptr.load(Ordering::Acquire);
            assert!(!cur_ptr.is_null());

            unsafe {
                Arc::increment_strong_count(cur_ptr);
            }
            let cur = unsafe { Arc::from_raw(cur_ptr) };

            let next = f(&cur);
            let next_ptr = Arc::into_raw(next) as *mut T;

            unsafe {
                Arc::increment_strong_count(cur_ptr);
            }
            let expect_arc = unsafe { Arc::from_raw(cur_ptr) };
            let expect_ptr = Arc::into_raw(expect_arc) as *mut T;

            match self.ptr.compare_exchange(
                expect_ptr,
                next_ptr,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    unsafe {
                        drop(Arc::from_raw(expect_ptr));
                    }
                    break;
                }
                Err(_) => unsafe {
                    drop(Arc::from_raw(next_ptr));
                    drop(Arc::from_raw(expect_ptr));
                },
            }
        }
    }
}
