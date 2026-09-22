use std::sync::LazyLock;

use objc2::rc::Retained;
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::CGColor;
use objc2_quartz_core::CALayer;

use crate::actor::drag::{DropTarget, preview_frame};
use crate::layout_engine::WindowDropAction;
use crate::sys::backdrop_layer::backdrop_blur;
use crate::sys::cgs_window::{CgsWindow, CgsWindowError};
use crate::sys::geometry::SameAs;
use crate::sys::window_surface::WindowSurface;
use crate::sys::window_transaction::WindowTransaction;
use crate::ui::common::with_disabled_actions;

static FILL: LazyLock<Retained<CGColor>> =
    LazyLock::new(|| CGColor::new_generic_rgb(0.13, 0.62, 1.0, 0.18).into());
static BORDER: LazyLock<Retained<CGColor>> =
    LazyLock::new(|| CGColor::new_generic_rgb(0.13, 0.62, 1.0, 0.95).into());
static STACK_FILL: LazyLock<Retained<CGColor>> =
    LazyLock::new(|| CGColor::new_generic_rgb(0.34, 0.64, 1.0, 0.16).into());

const PREVIEW_CORNER_RADIUS: f64 = 12.0;
const PREVIEW_BLUR_RADIUS: f64 = 6.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreviewStyle {
    Tile,
    Stack,
}

/// A single reusable WindowServer overlay backed by a persistent compositor
/// surface. Layers are allocated once and only changed properties are flushed.
pub struct DragPreview {
    // The surface must be detached before its owning WindowServer window drops.
    surface: WindowSurface,
    window: CgsWindow,
    root: Retained<CALayer>,
    backdrop: Option<Retained<CALayer>>,
    tint: Retained<CALayer>,
    card: Retained<CALayer>,
    frame: CGRect,
    style: PreviewStyle,
    relative_window: Option<u32>,
    visible: bool,
}

// The preview is exclusively owned by Reactor. Reactor itself is moved once to
// its dedicated thread before a preview can be created; the CA objects are
// never accessed concurrently or moved again after creation.
unsafe impl Send for DragPreview {}

impl DragPreview {
    pub fn new(target: DropTarget) -> Result<Self, CgsWindowError> {
        let frame = preview_frame(target);
        let style = Self::style(target);
        let bounds = Self::bounds(frame.size);
        let window = CgsWindow::new_compositor(frame, PREVIEW_CORNER_RADIUS)?;
        window.set_alpha(1.0)?;
        window.set_tags(1 << 3)?;

        let root = CALayer::layer();
        let backdrop = backdrop_blur(PREVIEW_BLUR_RADIUS);
        let tint = CALayer::layer();
        let card = CALayer::layer();
        with_disabled_actions(|| {
            root.setBounds(bounds);
            root.setPosition(CGPoint::new(0.0, 0.0));
            root.setAnchorPoint(CGPoint::new(0.0, 0.0));
            root.setContentsScale(2.0);
            root.setOpaque(false);
            root.setGeometryFlipped(false);
            root.setMasksToBounds(true);
            root.setCornerRadius(PREVIEW_CORNER_RADIUS);
            root.setBorderColor(Some(&BORDER));
            root.setBorderWidth(2.0);

            if let Some(backdrop) = &backdrop {
                backdrop.setFrame(bounds);
                root.addSublayer(backdrop);
            }

            tint.setFrame(bounds);
            tint.setBackgroundColor(Some(&FILL));
            root.addSublayer(&tint);

            card.setHidden(style != PreviewStyle::Stack);
            card.setFrame(Self::card_frame(frame.size));
            card.setCornerRadius(8.0);
            card.setBackgroundColor(Some(&STACK_FILL));
            card.setBorderColor(Some(&BORDER));
            card.setBorderWidth(1.0);
            root.addSublayer(&card);
        });

        let surface = WindowSurface::new(window.id(), bounds, &root)?;
        surface.flush();

        Ok(Self {
            surface,
            window,
            root,
            backdrop,
            tint,
            card,
            frame,
            style,
            relative_window: None,
            visible: false,
        })
    }

    pub fn show(&mut self, target: DropTarget) -> Result<(), CgsWindowError> {
        let frame = preview_frame(target);
        let style = Self::style(target);
        let relative_window = target.intent.window.idx.get();
        let size_changed = !self.frame.size.same_as(frame.size);
        let origin_changed = !self.frame.origin.same_as(frame.origin);
        let frame_changed = size_changed || origin_changed;
        let style_changed = self.style != style;
        let ordering_changed = !self.visible || self.relative_window != Some(relative_window);

        if !frame_changed && !style_changed && !ordering_changed {
            return Ok(());
        }

        if size_changed || style_changed {
            with_disabled_actions(|| {
                if size_changed {
                    self.root.setBounds(Self::bounds(frame.size));
                    if let Some(backdrop) = &self.backdrop {
                        backdrop.setFrame(Self::bounds(frame.size));
                    }
                    self.tint.setFrame(Self::bounds(frame.size));
                    self.card.setFrame(Self::card_frame(frame.size));
                }
                if style_changed {
                    self.card.setHidden(style != PreviewStyle::Stack);
                }
            });
            self.style = style;
        }

        if frame_changed {
            let transaction = WindowTransaction::new()?;
            transaction.set_rounded_frame(&self.window, frame, PREVIEW_CORNER_RADIUS)?;
            if size_changed {
                self.surface.set_bounds_in(&transaction, Self::bounds(frame.size));
            }
            transaction.commit();
            self.frame = frame;
        }

        if size_changed || style_changed {
            self.surface.flush();
        }

        if ordering_changed {
            if !self.visible {
                self.window.set_alpha(1.0)?;
            }
            self.window.order_above(Some(relative_window))?;
            self.relative_window = Some(relative_window);
        }
        self.visible = true;
        Ok(())
    }

    pub fn hide(&mut self) {
        if self.visible {
            let _ = self.window.set_alpha(0.0);
            let _ = self.window.order_out();
            self.visible = false;
        }
    }

    #[inline]
    fn style(target: DropTarget) -> PreviewStyle {
        if target.intent.action == WindowDropAction::Stack {
            PreviewStyle::Stack
        } else {
            PreviewStyle::Tile
        }
    }

    #[inline]
    fn bounds(size: CGSize) -> CGRect { CGRect::new(CGPoint::new(0.0, 0.0), size) }

    #[inline]
    fn card_frame(size: CGSize) -> CGRect {
        const INSET: f64 = 9.0;
        CGRect::new(
            CGPoint::new(INSET, INSET),
            CGSize::new(
                (size.width - INSET * 2.0).max(0.0),
                (size.height - INSET * 2.0).max(0.0),
            ),
        )
    }
}
