use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::{AnyThread, extern_class, extern_methods};
use objc2_core_foundation::{CFRetained, CGRect};
use objc2_core_graphics::{CGColorSpace, CGError};
use objc2_quartz_core::{CALayer, CATransaction};

use crate::sys::cg_ok;
use crate::sys::cgs_window::CgsWindowError;
use crate::sys::skylight::{
    G_CONNECTION, SLSAddSurface, SLSBindSurface, SLSOrderSurface, SLSRemoveSurface,
    SLSSetSurfaceBounds, SLSSetSurfaceColorSpace, SLSSetSurfaceOpacity, SLSSetSurfaceResolution,
};
use crate::sys::window_transaction::WindowTransaction;

extern_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = AnyThread]
    struct CAContext;
);

impl CAContext {
    extern_methods!(
        #[unsafe(method(contextWithCGSConnection:options:))]
        #[unsafe(method_family = none)]
        fn with_connection(
            connection: u32,
            options: Option<&objc2::runtime::AnyObject>,
        ) -> Option<Retained<Self>>;

        #[unsafe(method(setLayer:))]
        fn set_layer(&self, layer: Option<&CALayer>);

        #[unsafe(method(contextId))]
        #[unsafe(method_family = none)]
        fn context_id(&self) -> u32;

        #[unsafe(method(invalidate))]
        fn invalidate(&self);
    );
}

/// A persistent Core Animation surface hosted directly by a WindowServer window.
///
/// Unlike `SLWindowContextCreate`, this keeps the layer tree attached to the
/// compositor. Updating a layer therefore requires only a transaction flush,
/// without allocating a bitmap context or rasterizing the full tree on the CPU.
pub struct WindowSurface {
    connection: i32,
    window_id: u32,
    surface_id: u32,
    context: Retained<CAContext>,
}

impl WindowSurface {
    pub fn new(window_id: u32, bounds: CGRect, root: &CALayer) -> Result<Self, CgsWindowError> {
        let connection = *G_CONNECTION;
        let context = CAContext::with_connection(connection as u32, None)
            .ok_or(CgsWindowError::Surface(CGError(1000)))?;
        context.set_layer(Some(root));

        let mut surface_id = 0;
        if let Err(error) = unsafe { cg_ok(SLSAddSurface(connection, window_id, &mut surface_id)) }
        {
            context.set_layer(None);
            context.invalidate();
            return Err(CgsWindowError::Surface(error));
        }
        if surface_id == 0 {
            context.set_layer(None);
            context.invalidate();
            return Err(CgsWindowError::Surface(CGError(1000)));
        }

        let result = (|| {
            unsafe {
                cg_ok(SLSBindSurface(
                    connection,
                    window_id,
                    surface_id,
                    0x4,
                    0,
                    context.context_id(),
                ))
            }
            .map_err(CgsWindowError::Surface)?;
            unsafe { cg_ok(SLSSetSurfaceBounds(connection, window_id, surface_id, bounds)) }
                .map_err(CgsWindowError::Surface)?;
            unsafe { cg_ok(SLSSetSurfaceResolution(connection, window_id, surface_id, 2.0)) }
                .map_err(CgsWindowError::Surface)?;
            unsafe { cg_ok(SLSSetSurfaceOpacity(connection, window_id, surface_id, false)) }
                .map_err(CgsWindowError::Surface)?;

            if let Some(color_space) = CGColorSpace::new_device_rgb() {
                let _ = unsafe {
                    cg_ok(SLSSetSurfaceColorSpace(
                        connection,
                        window_id,
                        surface_id,
                        CFRetained::as_ptr(&color_space).as_ptr(),
                    ))
                };
            }

            unsafe { cg_ok(SLSOrderSurface(connection, window_id, surface_id, 1, 0)) }
                .map_err(CgsWindowError::Surface)
        })();

        if let Err(error) = result {
            context.set_layer(None);
            context.invalidate();
            let _ = unsafe { SLSRemoveSurface(connection, window_id, surface_id) };
            return Err(error);
        }

        Ok(Self {
            connection,
            window_id,
            surface_id,
            context,
        })
    }

    /// Queue a bounds change in the same WindowServer transaction as its window.
    pub fn set_bounds_in(&self, transaction: &WindowTransaction, bounds: CGRect) {
        transaction.set_surface_bounds(self.window_id, self.surface_id, bounds);
    }

    #[inline]
    pub fn flush(&self) { CATransaction::flush(); }
}

impl Drop for WindowSurface {
    fn drop(&mut self) {
        let _ = unsafe { SLSRemoveSurface(self.connection, self.window_id, self.surface_id) };
        self.context.set_layer(None);
        self.context.invalidate();
    }
}
