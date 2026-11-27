use cgs::compositor::{
    CgsWindow, CgsWindowError, WindowSurface, WindowTransaction, with_disabled_actions,
};
use objc2::rc::Retained;
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGColor, kCGUtilityWindowLevel};
use objc2_quartz_core::CALayer;

use crate::common::collections::{HashMap, HashSet};
use crate::model::hidden_window_placement::ParkingLocation;
use crate::sys::geometry::CGRectExt;
use crate::sys::screen::{ScreenId, ScreenInfo, SpaceId};
use crate::sys::skylight::SLSWindowTags;

#[derive(Default)]
pub(super) struct ParkingOccluders {
    physical: HashMap<ScreenId, CGRect>,
    covers: HashMap<(ScreenId, SpaceId), [Option<ParkingCover>; 4]>,
}

impl ParkingOccluders {
    pub(super) fn update_displays(&mut self, screens: &[ScreenInfo], active: &HashSet<SpaceId>) {
        if !cfg!(test) {
            self.physical = screens
                .iter()
                .map(|screen| {
                    (
                        screen.id,
                        objc2_core_graphics::CGDisplayBounds(screen.id.as_u32()),
                    )
                })
                .collect();
        }
        self.retire(screens, active);
    }

    pub(super) fn retire(&mut self, screens: &[ScreenInfo], active: &HashSet<SpaceId>) {
        self.covers.retain(|&(id, space), _| {
            active.contains(&space) && screens.iter().any(|s| s.id == id && s.space == Some(space))
        });
    }

    pub(super) fn clear(&mut self) { self.covers.clear(); }

    pub(super) fn physical_bounds(&self, screen: &ScreenInfo) -> CGRect {
        self.physical.get(&screen.id).copied().unwrap_or(screen.frame)
    }

    pub(super) fn reconcile(
        &mut self,
        screen: &ScreenInfo,
        space: SpaceId,
        strips: [Option<CGRect>; 4],
    ) {
        let key = (screen.id, space);
        if cfg!(test) || strips == [None; 4] {
            self.covers.remove(&key);
            return;
        }
        let scale = screen.backing_scale.max(1.0);
        for (slot, frame) in self.covers.entry(key).or_default().iter_mut().zip(strips) {
            let Some(frame) = frame else {
                *slot = None;
                continue;
            };
            let result = match slot.as_mut() {
                Some(cover) if cover.scale == scale => cover.set_frame(frame),
                _ => ParkingCover::new(frame, scale).map(|cover| *slot = Some(cover)),
            };
            if let Err(error) = result {
                *slot = None;
                tracing::warn!(?error, "Could not configure parking cover");
            }
        }
    }
}

pub(super) fn strip(
    location: ParkingLocation,
    target: CGRect,
    usable: CGRect,
    physical: CGRect,
    scale: f64,
) -> Option<CGRect> {
    let exposed = target.intersection(&physical);
    // A side Dock inset can leave a wide region on the physical display.
    if exposed.area() == 0.0 || exposed.size.width > 2.0 {
        return None;
    }
    let top = if location.bottom {
        usable.max().y - 64.0
    } else {
        usable.min().y
    };
    let bottom = if location.bottom {
        physical.max().y
    } else {
        exposed.max().y
    };
    let origin = physical.min();
    let scale = scale.max(1.0);
    // Cover only the revealed columns of pixels. The usable edge fixes the
    // horizontal exposure for every window parked at this location.
    let x = origin.x + ((exposed.min().x - origin.x) * scale).floor() / scale;
    let end_x = origin.x + ((exposed.max().x - origin.x) * scale).ceil() / scale;
    let y = origin.y + ((top - origin.y) * scale).floor() / scale;
    let end_y = origin.y + ((bottom - origin.y) * scale).ceil() / scale;
    let cover =
        CGRect::new(CGPoint::new(x, y), CGSize::new(end_x - x, end_y - y)).intersection(&physical);
    (cover.size.width <= 2.0 && cover.contains_rect(exposed)).then_some(cover)
}

struct ParkingCover {
    surface: WindowSurface,
    root: Retained<CALayer>,
    window: CgsWindow,
    frame: CGRect,
    scale: f64,
}

// Like the drag preview, a cover is created, used and dropped on the reactor
// thread. The reactor is moved to its thread before it creates any covers.
unsafe impl Send for ParkingCover {}

impl ParkingCover {
    fn new(frame: CGRect, scale: f64) -> Result<Self, CgsWindowError> {
        let window = CgsWindow::new_compositor(frame, 0.0)?;
        window.set_resolution(scale)?;
        window.set_opacity(true)?;
        // Stay above parked app windows and below the Dock, menu bar and Rift overview.
        window.set_level(kCGUtilityWindowLevel - 1)?;
        window.set_tags(
            (SLSWindowTags::DISABLE_SHADOW
                | SLSWindowTags::PREVENTS_ACTIVATION
                | SLSWindowTags::IGNORES_CYCLE
                | SLSWindowTags::IGNORE_FOR_EXPOSE
                | SLSWindowTags::ON_ALL_WORKSPACES)
                .bits(),
        )?;
        window.disable_shadow();

        // Window opacity alone leaves this window out of occlusion calculations.
        // An opaque layer hosted by WindowServer makes the cover participate.
        let bounds = CGRect::new(CGPoint::ZERO, frame.size);
        let root = CALayer::layer();
        root.setFrame(bounds);
        root.setContentsScale(scale);
        root.setOpaque(true);
        root.setBackgroundColor(Some(&CGColor::new_generic_gray(0.0, 1.0)));
        let surface = WindowSurface::new_scaled(window.id(), bounds, &root, scale)?;
        surface.flush();
        window.order_above(None)?;
        Ok(Self {
            surface,
            root,
            window,
            frame,
            scale,
        })
    }

    fn set_frame(&mut self, frame: CGRect) -> Result<(), CgsWindowError> {
        if frame == self.frame {
            return Ok(());
        }
        let transaction = WindowTransaction::new()?;
        if frame.size == self.frame.size {
            transaction.move_window(&self.window, frame.origin);
        } else {
            let bounds = CGRect::new(CGPoint::ZERO, frame.size);
            transaction.set_rounded_frame(&self.window, frame, 0.0)?;
            self.surface.set_bounds_in(&transaction, bounds);
            with_disabled_actions(|| self.root.setFrame(bounds));
            self.surface.flush();
        }
        transaction.commit();
        self.frame = frame;
        Ok(())
    }
}
