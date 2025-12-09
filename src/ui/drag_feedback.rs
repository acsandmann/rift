use std::ptr;

use objc2::rc::Retained;
use objc2_app_kit::NSStatusWindowLevel;
use objc2_core_foundation::{CFType, CGPoint, CGRect, CGSize};
use objc2_core_graphics::CGContext;
use objc2_quartz_core::CALayer;
use tracing::warn;

use crate::layout_engine::Direction;
use crate::sys::cgs_window::{CgsWindow, CgsWindowError};
use crate::sys::skylight::{
    CFRelease, G_CONNECTION, SLSFlushWindowContentRegion, SLWindowContextCreate,
};

unsafe extern "C" {
    fn CGContextFlush(ctx: *mut CGContext);
    fn CGContextClearRect(ctx: *mut CGContext, rect: CGRect);
    fn CGContextSaveGState(ctx: *mut CGContext);
    fn CGContextRestoreGState(ctx: *mut CGContext);
    fn CGContextTranslateCTM(ctx: *mut CGContext, tx: f64, ty: f64);
    fn CGContextScaleCTM(ctx: *mut CGContext, sx: f64, sy: f64);
}

const BORDER_WIDTH: f64 = 2.0;
const CORNER_RADIUS: f64 = 9.0;
const FILL_ALPHA: f64 = 0.12;
const STROKE_ALPHA: f64 = 0.9;
const EDGE_THICKNESS: f64 = 8.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedbackKind {
    Outline { fill: bool },
    Edge(Direction),
}

pub struct DragFeedback {
    frame: CGRect,
    root_layer: Retained<CALayer>,
    highlight_layer: Retained<CALayer>,
    cgs_window: CgsWindow,
    visible: bool,
    last_kind: FeedbackKind,
}

impl DragFeedback {
    pub fn new() -> Result<Self, CgsWindowError> {
        let frame = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1.0, 1.0));

        let root_layer = CALayer::layer();
        root_layer.setFrame(CGRect::new(CGPoint::new(0.0, 0.0), frame.size));
        root_layer.setGeometryFlipped(true);

        let highlight_layer = CALayer::layer();
        highlight_layer.setFrame(CGRect::new(CGPoint::new(0.0, 0.0), frame.size));
        let stroke =
            objc2_app_kit::NSColor::colorWithRed_green_blue_alpha(0.0, 0.5, 1.0, STROKE_ALPHA);
        highlight_layer.setBorderWidth(BORDER_WIDTH);
        highlight_layer.setCornerRadius(CORNER_RADIUS);
        highlight_layer.setBorderColor(Some(&stroke.CGColor()));
        highlight_layer.setBackgroundColor(None);

        root_layer.addSublayer(&highlight_layer);

        let cgs_window = CgsWindow::new(frame)?;
        if let Err(err) = cgs_window.set_opacity(false) {
            warn!(error=?err, "failed to set drag feedback window opacity");
        }
        if let Err(err) = cgs_window.set_alpha(1.0) {
            warn!(error=?err, "failed to set drag feedback window alpha");
        }
        if let Err(err) = cgs_window.set_level(NSStatusWindowLevel as i32) {
            warn!(error=?err, "failed to set drag feedback window level");
        }

        Ok(Self {
            frame,
            root_layer,
            highlight_layer,
            cgs_window,
            visible: false,
            last_kind: FeedbackKind::Outline { fill: false },
        })
    }

    pub fn show(
        &mut self,
        frame: CGRect,
        relative_to: Option<u32>,
        kind: FeedbackKind,
    ) -> Result<(), CgsWindowError> {
        if frame.size.width <= 0.0 || frame.size.height <= 0.0 {
            return self.hide();
        }

        let same_frame = self.visible
            && (self.frame.origin.x - frame.origin.x).abs() < 0.5
            && (self.frame.origin.y - frame.origin.y).abs() < 0.5
            && (self.frame.size.width - frame.size.width).abs() < 0.5
            && (self.frame.size.height - frame.size.height).abs() < 0.5
            && self.last_kind == kind;

        self.frame = frame;
        self.last_kind = kind;

        if !same_frame {
            self.cgs_window.set_shape(frame)?;
            self.root_layer.setFrame(CGRect::new(CGPoint::new(0.0, 0.0), frame.size));
        }

        match kind {
            FeedbackKind::Outline { fill } => {
                self.highlight_layer.setCornerRadius(CORNER_RADIUS);
                self.highlight_layer.setBorderWidth(BORDER_WIDTH);
                self.highlight_layer.setFrame(CGRect::new(CGPoint::new(0.0, 0.0), frame.size));
                if fill {
                    let fill = objc2_app_kit::NSColor::colorWithRed_green_blue_alpha(
                        0.0, 0.5, 1.0, FILL_ALPHA,
                    );
                    self.highlight_layer.setBackgroundColor(Some(&fill.CGColor()));
                } else {
                    self.highlight_layer.setBackgroundColor(None);
                }
            }
            FeedbackKind::Edge(direction) => {
                self.highlight_layer.setCornerRadius(3.0);
                self.highlight_layer.setBorderWidth(0.0);
                self.highlight_layer.setBackgroundColor(None);
                let bar = match direction {
                    Direction::Left => CGRect::new(
                        CGPoint::new(0.0, 0.0),
                        CGSize::new(EDGE_THICKNESS, frame.size.height),
                    ),
                    Direction::Right => CGRect::new(
                        CGPoint::new(frame.size.width - EDGE_THICKNESS, 0.0),
                        CGSize::new(EDGE_THICKNESS, frame.size.height),
                    ),
                    Direction::Up => CGRect::new(
                        CGPoint::new(0.0, 0.0),
                        CGSize::new(frame.size.width, EDGE_THICKNESS),
                    ),
                    Direction::Down => CGRect::new(
                        CGPoint::new(0.0, frame.size.height - EDGE_THICKNESS),
                        CGSize::new(frame.size.width, EDGE_THICKNESS),
                    ),
                };
                self.highlight_layer.setFrame(bar);
            }
        }

        self.present();
        self.visible = true;
        self.cgs_window.order_above(relative_to)?;
        Ok(())
    }

    pub fn hide(&mut self) -> Result<(), CgsWindowError> {
        if !self.visible {
            return Ok(());
        }
        self.visible = false;
        self.cgs_window.order_out()
    }

    pub fn is_visible(&self) -> bool { self.visible }

    fn present(&self) {
        let frame = self.frame;
        let ctx: *mut CGContext = unsafe {
            SLWindowContextCreate(
                *G_CONNECTION,
                self.cgs_window.id(),
                ptr::null_mut() as *mut CFType,
            )
        };
        if ctx.is_null() {
            return;
        }

        unsafe {
            let clear = CGRect::new(CGPoint::new(0.0, 0.0), frame.size);
            CGContextClearRect(ctx, clear);
            CGContextSaveGState(ctx);
            CGContextTranslateCTM(ctx, 0.0, frame.size.height);
            CGContextScaleCTM(ctx, 1.0, -1.0);
            self.root_layer.renderInContext(&*ctx);
            CGContextRestoreGState(ctx);
            CGContextFlush(ctx);
            SLSFlushWindowContentRegion(*G_CONNECTION, self.cgs_window.id(), ptr::null_mut());
            CFRelease(ctx as *mut CFType);
        }
    }
}
