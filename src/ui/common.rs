use std::ptr;

use objc2_core_foundation::{CFType, CGPoint, CGRect, CGSize};
use objc2_core_graphics::CGContext;
use objc2_quartz_core::{CALayer, CATransaction};

use crate::model::server::WindowData;
use crate::sys::skylight::{
    CFRelease, G_CONNECTION, SLSFlushWindowContentRegion, SLWindowContextCreate,
};

pub fn render_layer_to_cgs_window(window_id: u32, size: CGSize, layer: &CALayer) {
    unsafe {
        let ctx: *mut CGContext =
            SLWindowContextCreate(*G_CONNECTION, window_id, ptr::null_mut() as *mut CFType);
        if ctx.is_null() {
            return;
        }

        let clear = CGRect::new(CGPoint::new(0.0, 0.0), size);
        CGContext::clear_rect(Some(&*ctx), clear);
        CGContext::save_g_state(Some(&*ctx));
        CGContext::translate_ctm(Some(&*ctx), 0.0, size.height);
        CGContext::scale_ctm(Some(&*ctx), 1.0, -1.0);
        layer.renderInContext(&*ctx);
        CGContext::restore_g_state(Some(&*ctx));
        CGContext::flush(Some(&*ctx));
        SLSFlushWindowContentRegion(*G_CONNECTION, window_id, ptr::null_mut());
        CFRelease(ctx as *mut CFType);
    }
}

pub fn with_disabled_actions<F, R>(f: F) -> R
where F: FnOnce() -> R {
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    let result = f();
    CATransaction::commit();
    result
}

#[derive(Clone, Copy, Debug)]
pub struct WindowLayoutMetrics {
    pub scale: f64,
    pub x_offset: f64,
    pub y_offset: f64,
    pub min_x: f64,
    pub min_y: f64,
    pub disp_h: f64,
}

impl WindowLayoutMetrics {
    pub fn rect_for(&self, window: &WindowData, min_size: f64, gap: f64) -> CGRect {
        let wx = window.info.frame.origin.x - self.min_x;
        let wy_top = window.info.frame.origin.y - self.min_y + window.info.frame.size.height;
        let wy = self.disp_h - wy_top;
        let ww = window.info.frame.size.width;
        let wh = window.info.frame.size.height;

        let mut rx = self.x_offset + wx * self.scale;
        let mut ry = self.y_offset + wy * self.scale;
        let mut rw = (ww * self.scale).max(min_size);
        let mut rh = (wh * self.scale).max(min_size);

        if rw > (min_size + gap) {
            rx += gap / 2.0;
            rw -= gap;
        }
        if rh > (min_size + gap) {
            ry += gap / 2.0;
            rh -= gap;
        }

        CGRect::new(CGPoint::new(rx, ry), CGSize::new(rw, rh))
    }
}

pub fn compute_window_layout_metrics(
    windows: &[WindowData],
    bounds: CGRect,
    inset: f64,
    scale_factor: f64,
    max_scale: Option<f64>,
) -> Option<WindowLayoutMetrics> {
    if windows.is_empty() {
        return None;
    }

    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;

    for w in windows {
        let x0 = w.info.frame.origin.x;
        let y0 = w.info.frame.origin.y;
        let x1 = x0 + w.info.frame.size.width;
        let y1 = y0 + w.info.frame.size.height;
        if x0 < min_x {
            min_x = x0;
        }
        if y0 < min_y {
            min_y = y0;
        }
        if x1 > max_x {
            max_x = x1;
        }
        if y1 > max_y {
            max_y = y1;
        }
    }

    let disp_w = (max_x - min_x).max(1.0);
    let disp_h = (max_y - min_y).max(1.0);

    let cx = bounds.origin.x + inset;
    let cy = bounds.origin.y + inset;
    let cw = (bounds.size.width - 2.0 * inset).max(1.0);
    let ch = (bounds.size.height - 2.0 * inset).max(1.0);

    let mut scale = (cw / disp_w).min(ch / disp_h) * scale_factor;
    if let Some(max_scale) = max_scale {
        scale = scale.min(max_scale);
    }
    let x_offset = cx + (cw - disp_w * scale) / 2.0;
    let y_offset = cy + (ch - disp_h * scale) / 2.0;

    Some(WindowLayoutMetrics {
        scale,
        x_offset,
        y_offset,
        min_x,
        min_y,
        disp_h,
    })
}
