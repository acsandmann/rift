use std::ptr::NonNull;

use objc2_core_foundation::{CFRetained, CFType, CGPoint, CGRect};
use objc2_core_graphics::CGError;

use crate::sys::cgs_window::{CFRegion, CgsWindow, CgsWindowError};
use crate::sys::skylight::{
    G_CONNECTION, SLSTransactionCommit, SLSTransactionCreate, SLSTransactionMoveWindowWithGroup,
    SLSTransactionSetSurfaceBounds, SLSTransactionSetWindowShape,
};

/// A batch of compositor geometry changes committed atomically to WindowServer.
#[must_use = "a WindowTransaction has no effect until it is committed"]
pub struct WindowTransaction {
    transaction: CFRetained<CFType>,
}

impl WindowTransaction {
    pub fn new() -> Result<Self, CgsWindowError> {
        let transaction = unsafe { SLSTransactionCreate(*G_CONNECTION) };
        let transaction =
            NonNull::new(transaction).ok_or(CgsWindowError::Surface(CGError(1000)))?;
        Ok(Self {
            transaction: unsafe { CFRetained::from_raw(transaction) },
        })
    }

    /// Queue both the window shape and its position in this transaction.
    pub fn set_rounded_frame(
        &self,
        window: &CgsWindow,
        frame: CGRect,
        corner_radius: f64,
    ) -> Result<(), CgsWindowError> {
        let region = CFRegion::from_rounded_rect(frame.size, corner_radius)
            .map_err(CgsWindowError::Region)?;
        unsafe {
            SLSTransactionSetWindowShape(
                self.as_ptr(),
                window.id(),
                frame.origin.x as f32,
                frame.origin.y as f32,
                region.as_ptr(),
            )
        };
        unsafe { SLSTransactionMoveWindowWithGroup(self.as_ptr(), window.id(), frame.origin) };
        Ok(())
    }

    pub fn move_window(&self, window: &CgsWindow, origin: CGPoint) {
        unsafe { SLSTransactionMoveWindowWithGroup(self.as_ptr(), window.id(), origin) };
    }

    pub(crate) fn set_surface_bounds(&self, window_id: u32, surface_id: u32, bounds: CGRect) {
        unsafe { SLSTransactionSetSurfaceBounds(self.as_ptr(), window_id, surface_id, bounds) };
    }

    pub fn commit(self) { unsafe { SLSTransactionCommit(self.as_ptr(), 0) } }

    fn as_ptr(&self) -> *mut CFType { CFRetained::as_ptr(&self.transaction).as_ptr() }
}
