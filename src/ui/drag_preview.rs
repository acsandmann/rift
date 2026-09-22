use std::sync::LazyLock;

use objc2::rc::Retained;
use objc2_core_foundation::{CGPoint, CGRect};
use objc2_core_graphics::CGColor;
use objc2_quartz_core::CALayer;

use crate::actor::drag::{DropTarget, preview_frame};
use crate::layout_engine::WindowDropAction;
use crate::sys::cgs_window::{CgsWindow, CgsWindowError};
use crate::sys::geometry::SameAs;
use crate::ui::common::{render_layer_to_cgs_window, with_disabled_actions};

static FILL: LazyLock<Retained<CGColor>> =
    LazyLock::new(|| CGColor::new_generic_rgb(0.12, 0.48, 1.0, 0.20).into());
static BORDER: LazyLock<Retained<CGColor>> =
    LazyLock::new(|| CGColor::new_generic_rgb(0.38, 0.72, 1.0, 0.92).into());
static STACK_FILL: LazyLock<Retained<CGColor>> =
    LazyLock::new(|| CGColor::new_generic_rgb(0.34, 0.64, 1.0, 0.16).into());

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreviewStyle {
    Tile,
    Stack,
}

/// A single reusable WindowServer overlay. It owns no drag semantics and redraws
/// only when the target frame or visual style changes.
pub struct DragPreview {
    window: CgsWindow,
    frame: CGRect,
    style: PreviewStyle,
    relative_window: Option<u32>,
    visible: bool,
}

impl DragPreview {
    pub fn new(target: DropTarget) -> Result<Self, CgsWindowError> {
        let frame = preview_frame(target);
        let window = CgsWindow::new(frame)?;
        window.set_opacity(false)?;
        window.set_alpha(1.0)?;
        window.set_tags(1 << 3)?;
        let _ = window.set_blur(24, None);

        let preview = Self {
            window,
            frame,
            style: Self::style(target),
            relative_window: None,
            visible: false,
        };
        preview.redraw();
        Ok(preview)
    }

    pub fn show(&mut self, target: DropTarget) -> Result<(), CgsWindowError> {
        let frame = preview_frame(target);
        let style = Self::style(target);
        let relative_window = target.intent.window.idx.get();
        let frame_changed = !self.frame.same_as(frame);
        let style_changed = self.style != style;
        if self.visible
            && !frame_changed
            && !style_changed
            && self.relative_window == Some(relative_window)
        {
            return Ok(());
        }
        if frame_changed {
            self.window.set_shape(frame)?;
            self.frame = frame;
        }
        if frame_changed || style_changed {
            self.style = style;
            self.redraw();
        }
        self.window.order_above(Some(relative_window))?;
        self.relative_window = Some(relative_window);
        self.visible = true;
        Ok(())
    }

    pub fn hide(&mut self) {
        if self.visible {
            let _ = self.window.order_out();
            self.visible = false;
        }
    }

    fn style(target: DropTarget) -> PreviewStyle {
        if target.intent.action == WindowDropAction::Stack {
            PreviewStyle::Stack
        } else {
            PreviewStyle::Tile
        }
    }

    fn redraw(&self) {
        let bounds = CGRect::new(CGPoint::new(0.0, 0.0), self.frame.size);
        let root = CALayer::layer();
        let card = CALayer::layer();
        with_disabled_actions(|| {
            root.setFrame(bounds);
            root.setCornerRadius(12.0);
            root.setMasksToBounds(true);
            root.setBackgroundColor(Some(&FILL));
            root.setBorderColor(Some(&BORDER));
            root.setBorderWidth(2.0);

            let inset = if self.style == PreviewStyle::Stack {
                9.0
            } else {
                0.0
            };
            card.setHidden(self.style != PreviewStyle::Stack);
            card.setFrame(CGRect::new(
                CGPoint::new(inset, inset),
                objc2_core_foundation::CGSize::new(
                    (bounds.size.width - inset * 2.0).max(0.0),
                    (bounds.size.height - inset * 2.0).max(0.0),
                ),
            ));
            card.setCornerRadius(8.0);
            card.setBackgroundColor(Some(&STACK_FILL));
            card.setBorderColor(Some(&BORDER));
            card.setBorderWidth(1.0);
            root.addSublayer(&card);
        });
        render_layer_to_cgs_window(self.window.id(), self.frame.size, &root);
    }
}
