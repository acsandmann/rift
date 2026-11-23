//! A window has two frames, and a reader always says which one it means.
//!
//! - [`ActualFrame`]: where the window is, as last reported.
//! - [`PlannedFrame`]: where the window will be once Rift's open frame
//!   requests land.
//!
//! Neither is a bare `CGRect`, so one cannot be passed where the other is
//! expected. Unwrap with `rect()` only where the frame leaves Rift's model:
//! geometry math, messages to an app, query output.

use objc2_core_foundation::CGRect;

/// Where a window is: the frame its app (or the window server) last reported,
/// or where Rift's own presentation left it.
///
/// Built only where such a report is recorded on a window
/// ([`WindowState::record_actual`](crate::model::reactor::WindowState::record_actual)).
#[derive(Clone, Copy, Debug)]
pub(crate) struct ActualFrame(CGRect);

impl ActualFrame {
    pub(in crate::model) fn reported(frame: CGRect) -> Self { Self(frame) }

    pub(crate) fn rect(self) -> CGRect { self.0 }
}

/// Where a window will be once the requests Rift has sent land: the target of
/// a frame write in flight, otherwise the window's actual frame.
///
/// Built only by
/// [`WindowTxStore::planned`](crate::model::tx_store::WindowTxStore::planned).
#[derive(Clone, Copy, Debug)]
pub(crate) struct PlannedFrame(CGRect);

impl PlannedFrame {
    pub(in crate::model) fn new(frame: CGRect) -> Self { Self(frame) }

    pub(crate) fn rect(self) -> CGRect { self.0 }
}
