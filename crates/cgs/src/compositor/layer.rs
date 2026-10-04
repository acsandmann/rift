use std::ptr::{self, NonNull};

use objc2_core_foundation::{CFRetained, CFType, CGPoint, CGRect, CGSize};
use objc2_core_graphics::CGContext;
use objc2_quartz_core::{CALayer, CATransaction};

use super::private::{G_CONNECTION, SLSFlushWindowContentRegion, SLWindowContextCreate};
pub fn render_layer_to_cgs_window(window_id: u32, size: CGSize, layer: &CALayer) {
    unsafe {
        let ctx: *mut CGContext =
            SLWindowContextCreate(*G_CONNECTION, window_id, ptr::null_mut() as *mut CFType);
        let Some(ctx) = NonNull::new(ctx) else {
            return;
        };
        let ctx = CFRetained::from_raw(ctx);
        let clear = CGRect::new(CGPoint::new(0.0, 0.0), size);
        CGContext::clear_rect(Some(&*ctx), clear);
        CGContext::save_g_state(Some(&*ctx));
        CGContext::translate_ctm(Some(&*ctx), 0.0, size.height);
        CGContext::scale_ctm(Some(&*ctx), 1.0, -1.0);
        layer.renderInContext(&*ctx);
        CGContext::restore_g_state(Some(&*ctx));
        CGContext::flush(Some(&*ctx));
        SLSFlushWindowContentRegion(*G_CONNECTION, window_id, ptr::null_mut());
    }
}

/// Nested layer changes commit together; only a scope requesting a flush flushes.
pub struct LayerTransaction {
    flush: bool,
}
impl LayerTransaction {
    pub fn disabled() -> Self {
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        Self { flush: false }
    }

    pub fn flush_on_commit(mut self) -> Self {
        self.flush = true;
        self
    }
}
impl Drop for LayerTransaction {
    fn drop(&mut self) {
        CATransaction::commit();
        if self.flush {
            CATransaction::flush();
        }
    }
}

pub fn with_disabled_actions<F, R>(f: F) -> R
where F: FnOnce() -> R {
    let _transaction = LayerTransaction::disabled();
    f()
}

pub fn release_layer_tree(root: &CALayer) {
    root.removeAllAnimations();
    unsafe {
        root.setContents(None);
    }
    unsafe {
        root.setMask(None);
    }
    if let Some(children) = unsafe { root.sublayers() } {
        // Core Animation may return a live array; snapshot before detaching siblings.
        let children: Vec<_> = children.iter().collect();
        for child in children {
            release_layer_tree(&child);
            child.removeFromSuperlayer();
        }
    }
}
