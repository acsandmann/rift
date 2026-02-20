use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::sys::skylight::DisplayReconfigFlags;

static DISPLAY_CHURN_ACTIVE: AtomicBool = AtomicBool::new(false);
static DISPLAY_CHURN_EPOCH: AtomicU64 = AtomicU64::new(0);
static DISPLAY_CHURN_FLAGS: AtomicU64 = AtomicU64::new(0);

pub fn begin(flags: DisplayReconfigFlags) -> u64 {
    let was_active = DISPLAY_CHURN_ACTIVE.swap(true, Ordering::SeqCst);
    if !was_active {
        let epoch = DISPLAY_CHURN_EPOCH.fetch_add(1, Ordering::SeqCst).wrapping_add(1);
        DISPLAY_CHURN_FLAGS.store(flags.bits() as u64, Ordering::SeqCst);
        epoch
    } else {
        DISPLAY_CHURN_FLAGS.fetch_or(flags.bits() as u64, Ordering::SeqCst);
        DISPLAY_CHURN_EPOCH.load(Ordering::SeqCst)
    }
}

pub fn end() -> u64 {
    DISPLAY_CHURN_ACTIVE.store(false, Ordering::SeqCst);
    DISPLAY_CHURN_FLAGS.store(0, Ordering::SeqCst);
    DISPLAY_CHURN_EPOCH.fetch_add(1, Ordering::SeqCst).wrapping_add(1)
}

pub fn is_active() -> bool { DISPLAY_CHURN_ACTIVE.load(Ordering::SeqCst) }

pub fn epoch() -> u64 { DISPLAY_CHURN_EPOCH.load(Ordering::SeqCst) }

pub fn flags() -> DisplayReconfigFlags {
    DisplayReconfigFlags::from_bits_truncate(DISPLAY_CHURN_FLAGS.load(Ordering::SeqCst) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn begin_end_toggles_global_state() {
        let _ = begin(DisplayReconfigFlags::ADD);
        assert!(is_active());
        assert!(flags().contains(DisplayReconfigFlags::ADD));
        let _ = end();
        assert!(!is_active());
        assert!(flags().is_empty());
    }
}
