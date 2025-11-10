// many ideas for how this works were taken from https://github.com/xiamaz/YabaiIndicator
use std::ffi::c_void;
use std::ptr;
use std::ptr::NonNull;

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_app_kit::{NSImage, NSScreen, NSStatusBar, NSStatusItem, NSVariableStatusItemLength};
use objc2_core_foundation::CFRetained;
use objc2_core_graphics::{
    CGBitmapInfo, CGBlendMode, CGColorSpace, CGContext, CGImage, CGInterpolationQuality,
};
use objc2_foundation::{MainThreadMarker, NSSize, NSString};
use tracing::{debug, warn};

use crate::model::VirtualWorkspaceId;
use crate::model::server::{WindowData, WorkspaceData};
use crate::sys::screen::SpaceId;

#[allow(improper_ctypes)]
unsafe extern "C" {
    fn CGBitmapContextCreate(
        data: *mut c_void,
        width: usize,
        height: usize,
        bits_per_component: usize,
        bytes_per_row: usize,
        space: *mut CGColorSpace,
        bitmap_info: CGBitmapInfo,
    ) -> *mut CGContext;

    fn CGBitmapContextCreateImage(ctx: *mut CGContext) -> *mut CGImage;

    fn CGContextSaveGState(ctx: *mut CGContext);
    fn CGContextRestoreGState(ctx: *mut CGContext);
    fn CGContextTranslateCTM(ctx: *mut CGContext, tx: f64, ty: f64);
    fn CGContextScaleCTM(ctx: *mut CGContext, sx: f64, sy: f64);

    fn CGContextBeginPath(ctx: *mut CGContext);
    fn CGContextMoveToPoint(ctx: *mut CGContext, x: f64, y: f64);
    fn CGContextAddLineToPoint(ctx: *mut CGContext, x: f64, y: f64);
    fn CGContextAddArcToPoint(ctx: *mut CGContext, x1: f64, y1: f64, x2: f64, y2: f64, radius: f64);
    fn CGContextClosePath(ctx: *mut CGContext);
    fn CGContextFillPath(ctx: *mut CGContext);
    fn CGContextStrokePath(ctx: *mut CGContext);

    fn CGContextSetRGBFillColor(ctx: *mut CGContext, r: f64, g: f64, b: f64, a: f64);
    fn CGContextSetRGBStrokeColor(ctx: *mut CGContext, r: f64, g: f64, b: f64, a: f64);
    fn CGContextSetLineWidth(ctx: *mut CGContext, w: f64);

    fn CGContextClearRect(ctx: *mut CGContext, rect: objc2_core_foundation::CGRect);

    fn CGContextSetBlendMode(ctx: *mut CGContext, mode: i32);
}

pub struct MenuIcon {
    status_item: Retained<NSStatusItem>,
    mtm: MainThreadMarker,
}

impl MenuIcon {
    pub fn new(mtm: MainThreadMarker) -> Self {
        let status_item = {
            let status_bar = NSStatusBar::systemStatusBar();
            let status_item = status_bar.statusItemWithLength(NSVariableStatusItemLength);

            if let Some(button) = status_item.button(mtm) {
                debug!("MenuIcon::new: got status item button");
                let title = NSString::from_str("rift");
                button.setTitle(&title);
                status_item.setLength(50.0);
                status_item.setVisible(true);
            } else {
                warn!("MenuIcon::new: could not get button from status item");
            }

            status_item
        };

        Self { status_item, mtm }
    }

    pub fn update(
        &mut self,
        _active_space: SpaceId,
        workspaces: Vec<WorkspaceData>,
        _active_workspace: Option<VirtualWorkspaceId>,
        _windows: Vec<WindowData>,
        show_all_workspaces: bool,
    ) {
        debug!("workspaces = {}, windows = {}", workspaces.len(), _windows.len());

        unsafe {
            if let Some(button) = self.status_item.button(self.mtm) {
                let ws_to_render: Vec<&WorkspaceData> = if show_all_workspaces {
                    workspaces.iter().collect()
                } else {
                    workspaces.iter().filter(|w| w.window_count > 0).collect()
                };

                if ws_to_render.is_empty() {
                    self.status_item.setVisible(false);
                    return;
                }

                let cell_w = 20.0f64;
                let cell_h = 15.0f64;
                let spacing = 4.0f64;
                let count = ws_to_render.len();
                let total_w =
                    (cell_w * count as f64) + (spacing * (count.saturating_sub(1) as f64));
                let total_h = cell_h;

                let scale = if let Some(window) = button.window() {
                    window.backingScaleFactor()
                } else if let Some(screen) = NSScreen::mainScreen(self.mtm) {
                    screen.backingScaleFactor()
                } else {
                    1.0
                };
                let scale = if scale.is_finite() && scale > 0.0 {
                    scale
                } else {
                    1.0
                };

                let bitmap_w = (total_w * scale).max(1.0).round() as usize;
                let bitmap_h = (total_h * scale).max(1.0).round() as usize;

                let cs = CGColorSpace::new_device_rgb()
                    .unwrap_or(CGColorSpace::new_device_rgb().unwrap());

                // kCGImageAlphaPremultipliedFirst = 2, kCGBitmapByteOrder32Little = 2 << 12
                let bitmap_info = CGBitmapInfo(2u32 | 2 << 12);

                let raw_ctx = CGBitmapContextCreate(
                    ptr::null_mut(),
                    bitmap_w,
                    bitmap_h,
                    8,
                    0,
                    CFRetained::as_ptr(&cs).as_ptr(),
                    bitmap_info,
                );

                CGContext::set_interpolation_quality(
                    raw_ctx.as_ref(),
                    CGInterpolationQuality::None,
                );

                let ctx_ptr = raw_ctx as *mut CGContext;

                let pixel_height = bitmap_h as f64;
                CGContextSaveGState(ctx_ptr);
                CGContextTranslateCTM(ctx_ptr, 0.0, pixel_height);
                CGContextScaleCTM(ctx_ptr, scale, -scale);

                let full_rect = objc2_core_foundation::CGRect::new(
                    objc2_core_foundation::CGPoint::new(0.0, 0.0),
                    objc2_core_foundation::CGSize::new(total_w, total_h),
                );
                CGContextClearRect(ctx_ptr, full_rect);

                const radius: f64 = 3.0f64;
                const border_w: f64 = 1.0f64;
                const content_inset: f64 = 2.0f64;

                for (i, ws) in ws_to_render.iter().enumerate() {
                    let x = i as f64 * (cell_w + spacing);
                    let bg_x = x;
                    let bg_y = 0.0;
                    let bg_w = cell_w;
                    let bg_h = cell_h;

                    add_rounded_rect(ctx_ptr, bg_x, bg_y, bg_w, bg_h, radius);

                    let fill_alpha = if ws.is_active {
                        1.0
                    } else if ws.window_count > 0 {
                        0.45
                    } else {
                        0.0
                    };

                    if fill_alpha > 0.0 {
                        CGContextSetRGBFillColor(ctx_ptr, 0.0, 0.0, 0.0, fill_alpha);
                        CGContextFillPath(ctx_ptr);
                    }

                    add_rounded_rect(ctx_ptr, bg_x, bg_y, bg_w, bg_h, radius);
                    CGContextSetRGBStrokeColor(ctx_ptr, 0.0, 0.0, 0.0, 1.0);
                    CGContextSetLineWidth(ctx_ptr, border_w);
                    CGContextStrokePath(ctx_ptr);

                    if ws.windows.is_empty() {
                        continue;
                    }

                    let min_x =
                        ws.windows.iter().map(|w| w.frame.origin.x).fold(f64::INFINITY, f64::min);
                    let min_y =
                        ws.windows.iter().map(|w| w.frame.origin.y).fold(f64::INFINITY, f64::min);
                    let max_x = ws
                        .windows
                        .iter()
                        .map(|w| w.frame.origin.x + w.frame.size.width)
                        .fold(f64::NEG_INFINITY, f64::max);
                    let max_y = ws
                        .windows
                        .iter()
                        .map(|w| w.frame.origin.y + w.frame.size.height)
                        .fold(f64::NEG_INFINITY, f64::max);

                    let disp_w = (max_x - min_x).max(1.0);
                    let disp_h = (max_y - min_y).max(1.0);

                    let cx = bg_x + content_inset;
                    let cy = bg_y + content_inset;
                    let cw = (bg_w - 2.0 * content_inset).max(1.0);
                    let ch = (bg_h - 2.0 * content_inset).max(1.0);

                    let scaling = if disp_h > disp_w {
                        disp_h / ch
                    } else {
                        disp_w / cw
                    };
                    let sf = 1.0 / scaling;

                    let xoffset = if disp_h > disp_w {
                        (cw - disp_w * sf) / 2.0
                    } else {
                        0.0
                    } + cx;
                    let yoffset = if disp_h > disp_w {
                        0.0
                    } else {
                        (ch - disp_h * sf) / 2.0
                    } + cy;

                    for window in ws.windows.iter().rev() {
                        let wx = window.frame.origin.x - min_x;
                        let wy_top = window.frame.origin.y - min_y + window.frame.size.height;
                        let wy = disp_h - wy_top;
                        let ww = window.frame.size.width;
                        let wh = window.frame.size.height;

                        let mut rx = xoffset + wx * sf;
                        let mut ry = yoffset + wy * sf;
                        let mut rw = (ww * sf).max(2.0);
                        let mut rh = (wh * sf).max(2.0);

                        let win_gap = 0.75f64;
                        if rw > (2.0 + win_gap) {
                            rx += win_gap / 2.0;
                            rw -= win_gap;
                        }
                        if rh > (2.0 + win_gap) {
                            ry += win_gap / 2.0;
                            rh -= win_gap;
                        }

                        add_rounded_rect(ctx_ptr, rx, ry, rw, rh, 1.5);
                        CGContextSetRGBFillColor(ctx_ptr, 0.0, 0.0, 0.0, 1.0);
                        CGContextFillPath(ctx_ptr);

                        const K_CG_BLEND_MODE_DESTINATION_OUT: i32 = CGBlendMode::DestinationOut.0;
                        CGContextSaveGState(ctx_ptr);
                        CGContextSetBlendMode(ctx_ptr, K_CG_BLEND_MODE_DESTINATION_OUT);
                        CGContextSetRGBStrokeColor(ctx_ptr, 0.0, 0.0, 0.0, 1.0);
                        CGContextSetLineWidth(ctx_ptr, 1.5);

                        add_rounded_rect(ctx_ptr, rx, ry, rw, rh, 1.5);
                        CGContextStrokePath(ctx_ptr);
                        CGContextRestoreGState(ctx_ptr);
                    }
                }

                let out_ptr = CGBitmapContextCreateImage(raw_ctx);
                let cg_ret = CFRetained::from_raw(NonNull::new_unchecked(out_ptr as *mut CGImage));

                let ns_img_ptr = NSImage::initWithCGImage_size(
                    NSImage::alloc(),
                    cg_ret.as_ref(),
                    NSSize::new(total_w, total_h),
                );
                let image: Retained<NSImage> = Retained::from(ns_img_ptr);
                image.setTemplate(true);
                let empty = NSString::from_str("");
                button.setTitle(&empty);
                button.setImage(Some(&*image));

                CGContextRestoreGState(ctx_ptr);

                self.status_item.setLength(total_w);
                self.status_item.setVisible(true);
            } else {
                warn!("could not get button from status item");
            }
        }
    }
}

impl Drop for MenuIcon {
    fn drop(&mut self) {
        debug!("Removing menu bar icon");

        let status_bar = NSStatusBar::systemStatusBar();
        status_bar.removeStatusItem(&self.status_item);
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn add_rounded_rect(ctx: *mut CGContext, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let r = r.min(w / 2.0).min(h / 2.0);
    CGContextBeginPath(ctx);
    // start at (x + r, y + h)
    CGContextMoveToPoint(ctx, x + r, y + h);
    // top edge
    CGContextAddLineToPoint(ctx, x + w - r, y + h);
    // top-right corner arc
    CGContextAddArcToPoint(ctx, x + w, y + h, x + w, y + h - r, r);
    // right edge
    CGContextAddLineToPoint(ctx, x + w, y + r);
    // bottom-right
    CGContextAddArcToPoint(ctx, x + w, y, x + w - r, y, r);
    // bottom edge
    CGContextAddLineToPoint(ctx, x + r, y);
    // bottom-left
    CGContextAddArcToPoint(ctx, x, y, x, y + r, r);
    // left edge
    CGContextAddLineToPoint(ctx, x, y + h - r);
    // top-left
    CGContextAddArcToPoint(ctx, x, y + h, x + r, y + h, r);
    CGContextClosePath(ctx);
}
