//! Session-owned Overview. Runtime frames are truth; captured images only decorate cards.
use std::ptr::NonNull;

use block2::RcBlock;
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSFont, NSPopUpMenuWindowLevel, NSRunningApplication, NSScreen};
use objc2_core_foundation::{CFRetained, CFString, CFType, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGBitmapContextCreateImage, CGColor, CGColorSpace, CGContext,
    CGDisplayBounds, CGImage, CGImageAlphaInfo, CGPreflightScreenCaptureAccess,
};
use objc2_foundation::{MainThreadMarker, NSError};
use objc2_quartz_core::{
    CABasicAnimation, CAFrameRateRange, CALayer, CAMediaTiming, CATextLayer, CATransaction,
};
use objc2_screen_capture_kit::{
    SCContentFilter, SCScreenshotManager, SCShareableContent, SCStreamConfiguration, SCWindow,
};

use crate::actor::app::WindowId;
use crate::actor::mission_control::Input;
use crate::actor::reactor::ReactorHandle;
use crate::actor::{self};
use crate::common::collections::{HashMap, HashSet};
use crate::common::config::MissionControlSettings;
use crate::model::server::{RuntimeWindowData, RuntimeWorkspaceData};
use crate::sys::cgs_window::CgsWindow;
use crate::sys::dispatch::DispatchExt;
use crate::sys::screen::{NSScreenExt, ScreenInfo};
use crate::sys::window_server::WindowServerId;
use crate::sys::window_surface::WindowSurface;
use crate::ui::common::with_disabled_actions;

const GAP: f64 = 40.0;
const CORNER: f64 = 8.0;
const CAPTION: f64 = 28.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub workspace: usize,
    pub window: Option<WindowId>,
}

#[derive(Debug)]
struct WindowProjection {
    source: usize,
    frame: CGRect,
}
#[derive(Debug)]
struct WorkspaceProjection {
    source: usize,
    frame: CGRect,
    windows: Vec<WindowProjection>,
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> CGRect {
    CGRect::new(CGPoint::new(x, y), CGSize::new(w, h))
}
fn scrolled_workspace_offset(
    centered: usize,
    offset: f64,
    delta: f64,
    stride: f64,
    count: usize,
) -> f64 {
    (centered as f64 + offset - delta / stride).clamp(0.0, count.saturating_sub(1) as f64)
        - centered as f64
}

fn filter_workspaces(workspaces: &mut Vec<RuntimeWorkspaceData>, show_empty: bool) {
    if !show_empty {
        workspaces.retain(|ws| !ws.windows.is_empty());
    }
}

fn vertical_scroll(delta: CGPoint) -> bool { delta.y.abs() >= delta.x.abs() }

fn scrolled_horizontal_offset(
    display: CGRect,
    overview: CGSize,
    workspace: &RuntimeWorkspaceData,
    offset: f64,
    delta: f64,
) -> f64 {
    let scale = ((overview.height - 2.0 * GAP) * 0.5).max(1.0) / display.size.height;
    let base = (overview.width - display.size.width * scale) / 2.0;
    let (min, max) = workspace.windows.iter().fold((0.0_f64, 0.0_f64), |(min, max), w| {
        let x = base + (w.info.frame.origin.x - display.origin.x) * scale;
        (
            min.min(x - 12.0),
            max.max(x + w.info.frame.size.width * scale - overview.width + 12.0),
        )
    });
    (offset - delta).clamp(min, max)
}

fn contains(r: CGRect, p: CGPoint) -> bool {
    p.x >= r.origin.x
        && p.y >= r.origin.y
        && p.x < r.origin.x + r.size.width
        && p.y < r.origin.y + r.size.height
}
fn intersects(a: CGRect, b: CGRect) -> bool {
    a.size.width > 0.0
        && a.size.height > 0.0
        && b.size.width > 0.0
        && b.size.height > 0.0
        && a.origin.x < b.origin.x + b.size.width
        && b.origin.x < a.origin.x + a.size.width
        && a.origin.y < b.origin.y + b.size.height
        && b.origin.y < a.origin.y + a.size.height
}

fn card_visible(bounds: CGSize, ribbon: CGRect, card: CGRect) -> bool {
    let left = (ribbon.origin.x + card.origin.x).max(ribbon.origin.x).max(0.0);
    let top = (ribbon.origin.y + card.origin.y).max(ribbon.origin.y).max(0.0);
    let right = (ribbon.origin.x + card.origin.x + card.size.width)
        .min(ribbon.origin.x + ribbon.size.width)
        .min(bounds.width);
    let bottom = (ribbon.origin.y + card.origin.y + card.size.height)
        .min(ribbon.origin.y + ribbon.size.height)
        .min(bounds.height);
    left < right && top < bottom
}

/// Pure display-relative projection with one stride of vertical overscan.
fn project(
    display: CGRect,
    overview: CGSize,
    workspaces: &[RuntimeWorkspaceData],
    centered: usize,
    workspace_offset: f64,
    offsets: &HashMap<usize, f64>,
) -> Vec<WorkspaceProjection> {
    if display.size.height <= 0.0 || display.size.width <= 0.0 {
        return Vec::new();
    }
    let height = ((overview.height - 2.0 * GAP) * 0.5).max(1.0);
    let stride = height + GAP;
    let scale = height / display.size.height;
    let bounds = rect(0.0, -stride, overview.width, overview.height + stride * 2.0);
    workspaces
        .iter()
        .enumerate()
        .filter_map(|(source, ws)| {
            let frame = rect(
                0.0,
                overview.height / 2.0
                    + (source as f64 - centered as f64 - workspace_offset) * stride
                    - height / 2.0,
                overview.width,
                height,
            );
            if !intersects(frame, bounds) {
                return None;
            }
            let x = (overview.width - display.size.width * scale) / 2.0;
            let mut windows: Vec<_> = ws
                .windows
                .iter()
                .enumerate()
                .filter_map(|(source, window)| {
                    let w = window.info.frame;
                    let projected = rect(
                        x + (w.origin.x - display.origin.x) * scale
                            - offsets.get(&ws.index).copied().unwrap_or(0.0),
                        (w.origin.y - display.origin.y) * scale,
                        w.size.width * scale,
                        w.size.height * scale,
                    );
                    intersects(projected, rect(0.0, 0.0, frame.size.width, frame.size.height))
                        .then_some(WindowProjection { source, frame: projected })
                })
                .collect();
            windows.sort_by_key(|w| ws.windows[w.source].is_floating);
            Some(WorkspaceProjection { source, frame, windows })
        })
        .collect()
}

fn hit(
    workspaces: &[RuntimeWorkspaceData],
    projection: &[WorkspaceProjection],
    point: CGPoint,
) -> Option<Selection> {
    for ws in projection.iter().rev() {
        if !contains(ws.frame, point) {
            continue;
        }
        let local = CGPoint::new(point.x - ws.frame.origin.x, point.y - ws.frame.origin.y);
        let workspace = &workspaces[ws.source];
        let window = ws
            .windows
            .iter()
            .rev()
            .find(|w| contains(w.frame, local))
            .map(|w| workspace.windows[w.source].id);
        return Some(Selection {
            workspace: workspace.index,
            window,
        });
    }
    None
}

struct WindowCard {
    id: WindowId,
    layer: Retained<CALayer>,
    image: Retained<CALayer>,
    caption: Retained<CALayer>,
    title: Retained<CATextLayer>,
}
struct WorkspaceView {
    index: usize,
    layer: Retained<CALayer>,
    heading: Retained<CATextLayer>,
    empty: Retained<CATextLayer>,
    cards: Vec<WindowCard>,
}
struct DisplayOverview {
    info: ScreenInfo,
    bounds: CGRect,
    scale: f64,
    workspaces: Vec<RuntimeWorkspaceData>,
    centered: usize,
    workspace_offset: f64,
    offsets: HashMap<usize, f64>,
    projection: Vec<WorkspaceProjection>,
    views: Vec<WorkspaceView>,
    icons: HashMap<i32, Option<Retained<CGImage>>>,
    animate: bool,
    root: Retained<CALayer>,
    // Field order matters: unbind the surface before releasing its CGS window.
    surface: WindowSurface,
    _window: CgsWindow,
}

fn color(r: f64, g: f64, b: f64, a: f64) -> CFRetained<CGColor> {
    CGColor::new_generic_rgb(r, g, b, a)
}
fn layer(frame: CGRect, scale: f64) -> Retained<CALayer> {
    let l = CALayer::layer();
    l.setFrame(frame);
    l.setContentsScale(scale);
    l
}
fn label(parent: &CALayer, text: &str, frame: CGRect, scale: f64) -> Retained<CATextLayer> {
    let l = CATextLayer::layer();
    l.setFrame(frame);
    l.setContentsScale(scale);
    let font = NSFont::systemFontOfSize(12.0);
    unsafe {
        l.setFont(Some(&*(Retained::as_ptr(&font) as *const CFType)));
        l.setTruncationMode(objc2_quartz_core::kCATruncationEnd);
    }
    l.setFontSize(12.0);
    l.setForegroundColor(Some(&color(0.94, 0.94, 0.97, 0.86)));
    let text = CFString::from_str(text);
    unsafe {
        l.setString(Some(&*(text.as_ref() as *const AnyObject)));
    }
    parent.addSublayer(&l);
    l
}

impl DisplayOverview {
    fn reveal(&mut self, workspace: usize, window: WindowId) -> bool {
        let Some(w) = self
            .workspaces
            .iter()
            .find(|ws| ws.index == workspace)
            .and_then(|ws| ws.windows.iter().find(|w| w.id == window))
        else {
            return false;
        };
        let scale =
            ((self.bounds.size.height - 2.0 * GAP) * 0.5).max(1.0) / self.info.frame.size.height;
        let base = (self.bounds.size.width - self.info.frame.size.width * scale) / 2.0;
        let x = base + (w.info.frame.origin.x - self.info.frame.origin.x) * scale;
        let right = x + w.info.frame.size.width * scale;
        let offset = self.offsets.entry(workspace).or_default();
        let previous = *offset;
        if x - *offset < 12.0 {
            *offset = x - 12.0;
        } else if right - *offset > self.bounds.size.width - 12.0 {
            *offset = right - self.bounds.size.width + 12.0;
        }
        *offset != previous
    }

    /// Translate existing ribbons without allocating or projecting their window cards.
    /// Fall back to reconciliation only when a workspace crosses the overscan boundary.
    fn translate_vertical(&mut self, previous_offset: f64) -> Option<bool> {
        let height = ((self.bounds.size.height - 2.0 * GAP) * 0.5).max(1.0);
        let stride = height + GAP;
        let overscan = rect(
            0.0,
            -stride,
            self.bounds.size.width,
            self.bounds.size.height + stride * 2.0,
        );
        let delta = (previous_offset - self.workspace_offset) * stride;
        let included = |source: usize| {
            let y = self.bounds.size.height / 2.0
                + (source as f64 - self.centered as f64 - self.workspace_offset) * stride
                - height / 2.0;
            intersects(rect(0.0, y, self.bounds.size.width, height), overscan)
        };
        if (0..self.workspaces.len())
            .filter(|&source| included(source))
            .ne(self.projection.iter().map(|ws| ws.source))
        {
            return None;
        }
        let mut visibility_changed = false;
        with_disabled_actions(|| {
            for (view, ws) in self.views.iter().zip(&mut self.projection) {
                let previous = ws.frame;
                ws.frame.origin.y += delta;
                visibility_changed |= ws.windows.iter().any(|w| {
                    card_visible(self.bounds.size, previous, w.frame)
                        != card_visible(self.bounds.size, ws.frame, w.frame)
                });
                view.layer.setFrame(ws.frame);
                let mut heading = view.heading.frame();
                heading.origin.y += delta;
                view.heading.setFrame(heading);
            }
        });
        Some(visibility_changed)
    }

    fn reproject(
        &mut self,
        previews: Option<&PreviewSession>,
        remembered: Option<&RememberedPreviewCache>,
    ) {
        let next = project(
            self.info.frame,
            self.bounds.size,
            &self.workspaces,
            self.centered,
            self.workspace_offset,
            &self.offsets,
        );
        let same_cards = next.len() == self.projection.len()
            && next.iter().zip(&self.projection).all(|(a, b)| {
                a.source == b.source
                    && a.windows.len() == b.windows.len()
                    && a.windows.iter().zip(&b.windows).all(|(a, b)| a.source == b.source)
            });
        if !same_cards {
            self.rebuild(previews, remembered);
            return;
        }
        with_disabled_actions(|| {
            for ((view, projected), previous) in self.views.iter().zip(&next).zip(&self.projection)
            {
                view.layer.setFrame(projected.frame);
                view.heading.setFrame(rect(
                    (projected.frame.size.width - 300.0) / 2.0,
                    projected.frame.origin.y - 24.0,
                    300.0,
                    20.0,
                ));
                for ((card, window), old) in
                    view.cards.iter().zip(&projected.windows).zip(&previous.windows)
                {
                    if window.frame != old.frame {
                        card.layer.setFrame(window.frame);
                    }
                }
            }
        });
        self.projection = next;
    }

    fn rebuild(
        &mut self,
        previews: Option<&PreviewSession>,
        remembered: Option<&RememberedPreviewCache>,
    ) {
        self.projection = project(
            self.info.frame,
            self.bounds.size,
            &self.workspaces,
            self.centered,
            self.workspace_offset,
            &self.offsets,
        );
        let mut old_views = std::mem::take(&mut self.views);
        let mut old_cards: HashMap<_, _> = old_views
            .iter_mut()
            .filter(|view| {
                !self.projection.iter().any(|ws| {
                    let data = &self.workspaces[ws.source];
                    view.index == data.index
                        && view.cards.len() == ws.windows.len()
                        && view.cards.iter().zip(&ws.windows).all(|(card, w)| {
                            card.id == data.windows[w.source].id && card.layer.frame() == w.frame
                        })
                })
            })
            .flat_map(|view| {
                let origin = view.layer.frame().origin;
                view.cards.drain(..).map(move |card| {
                    let mut frame = card.layer.frame();
                    frame.origin.x += origin.x;
                    frame.origin.y += origin.y;
                    card.layer.removeFromSuperlayer();
                    (card.id, (card, frame))
                })
            })
            .collect();
        with_disabled_actions(|| {
            for ws in &self.projection {
                let data = &self.workspaces[ws.source];
                let existing = old_views
                    .iter()
                    .position(|view| view.index == data.index)
                    .map(|pos| old_views.swap_remove(pos));
                if let Some(view) = existing.as_ref().filter(|view| !view.cards.is_empty()) {
                    view.layer.setFrame(ws.frame);
                    view.heading.setFrame(rect(
                        (ws.frame.size.width - 300.0) / 2.0,
                        ws.frame.origin.y - 24.0,
                        300.0,
                        20.0,
                    ));
                    self.views.push(existing.unwrap());
                    continue;
                }
                let (container, heading, empty) = if let Some(view) = existing {
                    (view.layer, view.heading, view.empty)
                } else {
                    let container = layer(ws.frame, self.scale);
                    let heading = label(
                        &self.root,
                        &data.name,
                        rect(
                            (ws.frame.size.width - 300.0) / 2.0,
                            ws.frame.origin.y - 24.0,
                            300.0,
                            20.0,
                        ),
                        self.scale,
                    );
                    unsafe {
                        heading.setAlignmentMode(objc2_quartz_core::kCAAlignmentCenter);
                    }
                    heading.setFontSize(12.0);
                    let empty = label(
                        &container,
                        "Empty workspace",
                        rect(
                            (ws.frame.size.width - 180.0) / 2.0,
                            ws.frame.size.height / 2.0 - 10.0,
                            180.0,
                            20.0,
                        ),
                        self.scale,
                    );
                    empty.setForegroundColor(Some(&color(0.94, 0.94, 0.97, 0.38)));
                    (container, heading, empty)
                };
                container.setFrame(ws.frame);
                container.setMasksToBounds(true);
                empty.setHidden(!data.windows.is_empty());
                heading.setFrame(rect(
                    (ws.frame.size.width - 300.0) / 2.0,
                    ws.frame.origin.y - 24.0,
                    300.0,
                    20.0,
                ));
                self.root.addSublayer(&container);
                self.root.addSublayer(&heading);
                let mut cards = Vec::new();
                for w in &ws.windows {
                    let data = &data.windows[w.source];
                    if let Some((card, previous)) = old_cards.remove(&data.id) {
                        card.layer.setFrame(w.frame);
                        let height = (w.frame.size.height - CAPTION).max(1.0);
                        card.image.setBorderWidth(0.5);
                        card.image.setBorderColor(Some(&color(1.0, 1.0, 1.0, 0.18)));
                        card.image.setFrame(rect(
                            4.0,
                            4.0,
                            (w.frame.size.width - 8.0).max(1.0),
                            (height - 4.0).max(1.0),
                        ));
                        card.caption.setFrame(rect(0.0, height, w.frame.size.width, CAPTION));
                        card.title.setFrame(rect(
                            26.0,
                            5.0,
                            (w.frame.size.width - 32.0).max(0.0),
                            20.0,
                        ));
                        container.addSublayer(&card.layer);
                        if self.animate {
                            let animation = CABasicAnimation::animationWithKeyPath(Some(
                                &objc2_foundation::NSString::from_str("position"),
                            ));
                            let from = CGPoint::new(
                                previous.origin.x - ws.frame.origin.x + previous.size.width / 2.0,
                                previous.origin.y - ws.frame.origin.y + previous.size.height / 2.0,
                            );
                            unsafe {
                                animation.setFromValue(Some(
                                    &*objc2_foundation::NSValue::valueWithPoint(from),
                                ));
                            }
                            animation.setPreferredFrameRateRange(CAFrameRateRange::new(
                                60.0, 120.0, 120.0,
                            ));
                            animation.setDuration(0.16);
                            card.layer.addAnimation_forKey(&animation, None);
                            let resize = CABasicAnimation::animationWithKeyPath(Some(
                                &objc2_foundation::NSString::from_str("bounds"),
                            ));
                            unsafe {
                                resize.setFromValue(Some(
                                    &*objc2_foundation::NSValue::valueWithRect(rect(
                                        0.0,
                                        0.0,
                                        previous.size.width,
                                        previous.size.height,
                                    )),
                                ));
                            }
                            resize.setPreferredFrameRateRange(CAFrameRateRange::new(
                                60.0, 120.0, 120.0,
                            ));
                            resize.setDuration(0.16);
                            card.layer.addAnimation_forKey(&resize, None);
                        }
                        cards.push(card);
                        continue;
                    }
                    let card = layer(w.frame, self.scale);
                    let height = (w.frame.size.height - CAPTION).max(1.0);
                    let image = layer(
                        rect(
                            4.0,
                            4.0,
                            (w.frame.size.width - 8.0).max(1.0),
                            (height - 4.0).max(1.0),
                        ),
                        self.scale,
                    );
                    image.setCornerRadius(CORNER);
                    image.setMasksToBounds(true);
                    image.setBorderWidth(0.5);
                    image.setBorderColor(Some(&color(1.0, 1.0, 1.0, 0.18)));
                    image.setBackgroundColor(Some(&color(0.13, 0.13, 0.14, 1.0)));
                    unsafe {
                        image.setContentsGravity(objc2_quartz_core::kCAGravityResizeAspect);
                    }
                    if let Some(image_data) = previews
                        .and_then(|p| p.images.get(&data.id))
                        .or_else(|| remembered.and_then(|c| c.get(data)))
                    {
                        unsafe {
                            image.setContents(Some(
                                &*(image_data.as_ref() as *const CGImage as *const AnyObject),
                            ));
                        }
                    }
                    card.addSublayer(&image);
                    let caption = layer(rect(0.0, height, w.frame.size.width, CAPTION), self.scale);
                    card.addSublayer(&caption);
                    let icon = self.icons.entry(data.id.pid).or_insert_with(|| {
                        let app = NSRunningApplication::runningApplicationWithProcessIdentifier(
                            data.id.pid,
                        )?;
                        let icon = app.icon()?;
                        unsafe {
                            icon.CGImageForProposedRect_context_hints(
                                std::ptr::null_mut(),
                                None,
                                None,
                            )
                        }
                    });
                    if let Some(icon) = icon {
                        let icon_layer = layer(rect(4.0, 6.0, 16.0, 16.0), self.scale);
                        unsafe {
                            icon_layer.setContents(Some(
                                &*(icon.as_ref() as *const CGImage as *const AnyObject),
                            ));
                        }
                        caption.addSublayer(&icon_layer);
                    }
                    let title = label(
                        &caption,
                        if data.info.title.is_empty() {
                            data.app_name.as_deref().unwrap_or("Window")
                        } else {
                            &data.info.title
                        },
                        rect(26.0, 5.0, (w.frame.size.width - 32.0).max(0.0), 20.0),
                        self.scale,
                    );
                    container.addSublayer(&card);
                    cards.push(WindowCard {
                        id: data.id,
                        layer: card,
                        image,
                        caption,
                        title,
                    });
                }
                self.views.push(WorkspaceView {
                    index: data.index,
                    layer: container,
                    heading,
                    empty,
                    cards,
                });
            }
        });
        for view in old_views {
            view.layer.removeFromSuperlayer();
            view.heading.removeFromSuperlayer();
        }
        self.animate = false;
        self.surface.flush();
    }
}

struct OverviewDrag {
    intent: crate::actor::reactor::OverviewDrop,
    start: CGPoint,
    point: CGPoint,
    size: CGSize,
    started: bool,
    card: Retained<CALayer>,
    preview: Retained<CALayer>,
    indicator: Retained<CALayer>,
}

fn drop_action(
    frame: CGRect,
    point: CGPoint,
    mode: &str,
) -> crate::layout_engine::WindowDropAction {
    use crate::layout_engine::{Direction, WindowDropAction};
    if mode == "stack" {
        return WindowDropAction::Swap;
    }
    let x = (point.x - frame.origin.x) / frame.size.width.max(1.0);
    if x < 0.25 {
        WindowDropAction::Insert(Direction::Left)
    } else if x >= 0.75 {
        WindowDropAction::Insert(Direction::Right)
    } else if mode == "scrolling" && point.y < frame.origin.y + frame.size.height / 2.0 {
        WindowDropAction::Insert(Direction::Up)
    } else {
        WindowDropAction::Stack
    }
}

fn floating_drop_frame(
    display: CGRect,
    ribbon: CGRect,
    card: CGSize,
    point: CGPoint,
    offset: f64,
) -> CGRect {
    let scale = ribbon.size.height / display.size.height;
    let base = (ribbon.size.width - display.size.width * scale) / 2.0;
    let width = (card.width / scale).min(display.size.width);
    let height = (card.height / scale).min(display.size.height);
    let x = (point.x - card.width / 2.0 - base + offset) / scale;
    let y = (point.y - card.height / 2.0) / scale;
    rect(
        display.origin.x + x.clamp(0.0, display.size.width - width),
        display.origin.y + y.clamp(0.0, display.size.height - height),
        width,
        height,
    )
}

fn tiled_target<'a>(
    data: &RuntimeWorkspaceData,
    ws: &'a WorkspaceProjection,
    point: CGPoint,
    source: Option<WindowId>,
) -> Option<&'a WindowProjection> {
    let target =
        ws.windows
            .iter()
            .filter(|w| !data.windows[w.source].is_floating)
            .min_by(|a, b| {
                let distances = |w: &WindowProjection| {
                    (
                        (w.frame.origin.x - point.x)
                            .max(0.0)
                            .max(point.x - w.frame.origin.x - w.frame.size.width),
                        (point.y - w.frame.origin.y - w.frame.size.height / 2.0).abs(),
                    )
                };
                let (ax, ay) = distances(a);
                let (bx, by) = distances(b);
                ax.total_cmp(&bx).then(ay.total_cmp(&by))
            })?;
    if source == Some(data.windows[target.source].id)
        && matches!(
            drop_action(target.frame, point, &data.layout_mode),
            crate::layout_engine::WindowDropAction::Insert(
                crate::layout_engine::Direction::Left | crate::layout_engine::Direction::Right
            )
        )
        && let Some(sibling) = ws.windows.iter().find(|w| {
            data.windows[w.source].id != data.windows[target.source].id
                && !data.windows[w.source].is_floating
                && w.frame.origin.x == target.frame.origin.x
                && w.frame.size.width == target.frame.size.width
        })
    {
        return Some(sibling);
    }
    Some(target)
}

fn edge_direction(height: f64, y: f64) -> f64 {
    if y < 56.0 {
        -1.0
    } else if y > height - 56.0 {
        1.0
    } else {
        0.0
    }
}

pub struct OverviewSession {
    generation: u64,
    show_empty_workspaces: bool,
    displays: Vec<DisplayOverview>,
    active: usize,
    selection: Selection,
    pub(crate) previews: Option<PreviewSession>,
    drag: Option<OverviewDrag>,
    drop: Option<crate::actor::reactor::OverviewDrop>,
    pressed: Option<CGPoint>,
}

impl OverviewSession {
    pub(crate) fn new(
        reactor: &ReactorHandle,
        mtm: MainThreadMarker,
        settings: &MissionControlSettings,
        generation: u64,
        remembered: &mut Option<RememberedPreviewCache>,
    ) -> Option<Self> {
        let mut displays = Vec::new();
        let mut active = 0;
        let snapshot: Vec<_> = reactor
            .query_displays()
            .into_iter()
            .filter_map(|display| {
                let space = display.info.space?;
                let mut workspaces = reactor.query_workspaces(Some(space));
                filter_workspaces(&mut workspaces, settings.show_empty_workspaces);
                Some((display, workspaces))
            })
            .collect();
        if let Some(cache) = remembered {
            cache.prune(snapshot.iter().flat_map(|(_, ws)| ws).flat_map(|ws| &ws.windows));
        }
        for (display, workspaces) in snapshot {
            if workspaces.is_empty() {
                continue;
            }
            let centered = workspaces.iter().position(|w| w.is_active).unwrap_or(0);
            let bounds = CGDisplayBounds(display.info.id.as_u32());
            let scale = NSScreen::screens(mtm)
                .iter()
                .find(|s| s.get_number().ok() == Some(display.info.id))
                .map(|s| s.backingScaleFactor())
                .unwrap_or(1.0);
            let root = layer(rect(0.0, 0.0, bounds.size.width, bounds.size.height), scale);
            root.setGeometryFlipped(true);
            root.setMasksToBounds(true);
            root.setBackgroundColor(Some(&color(0.035, 0.035, 0.045, 0.48)));
            let result = (|| {
                let window = CgsWindow::new_compositor(bounds, 0.0)?;
                window.set_resolution(scale)?;
                window.set_level(NSPopUpMenuWindowLevel as i32)?;
                window.set_blur(36, None)?;
                let surface = WindowSurface::new_scaled(window.id(), root.bounds(), &root, scale)?;
                Ok::<_, crate::sys::cgs_window::CgsWindowError>((window, surface))
            })();
            let (window, surface) = match result {
                Ok(r) => r,
                Err(error) => {
                    tracing::warn!(?error, "Overview display unavailable");
                    continue;
                }
            };
            if display.is_active_context {
                active = displays.len();
            }
            let mut view = DisplayOverview {
                info: display.info,
                bounds,
                scale,
                workspaces,
                centered,
                workspace_offset: 0.0,
                offsets: HashMap::default(),
                projection: Vec::new(),
                views: Vec::new(),
                icons: HashMap::default(),
                animate: false,
                root,
                surface,
                _window: window,
            };
            view.rebuild(None, remembered.as_ref());
            if let Err(error) = view._window.order_above(None) {
                tracing::warn!(?error, "Overview ordering failed");
                continue;
            }
            if settings.fade_enabled {
                let animation = CABasicAnimation::animationWithKeyPath(Some(
                    &objc2_foundation::NSString::from_str("opacity"),
                ));
                unsafe {
                    animation.setFromValue(Some(&*objc2_foundation::NSNumber::new_f64(0.0)));
                    animation.setToValue(Some(&*objc2_foundation::NSNumber::new_f64(1.0)));
                }
                animation.setPreferredFrameRateRange(CAFrameRateRange::new(60.0, 120.0, 120.0));
                animation.setDuration(settings.fade_duration_ms.max(0.0) / 1000.0);
                view.root.addAnimation_forKey(&animation, None);
            }
            displays.push(view);
        }
        if displays.is_empty() {
            return None;
        }
        active = active.min(displays.len() - 1);
        let ws = &displays[active].workspaces[displays[active].centered];
        let selection = Selection {
            workspace: ws.index,
            window: ws.windows.iter().find(|w| w.is_focused).map(|w| w.id),
        };
        let session = Self {
            generation,
            show_empty_workspaces: settings.show_empty_workspaces,
            displays,
            active,
            selection,
            previews: None,
            drag: None,
            drop: None,
            pressed: None,
        };
        session.highlight(None);
        // Cached/fallback layers are already visible; fresh capture starts on the next run-loop turn.
        Some(session)
    }

    pub(crate) fn is_empty(&self) -> bool { self.displays.is_empty() }

    pub(crate) fn take_drop(&mut self) -> Option<crate::actor::reactor::OverviewDrop> {
        self.drop.take()
    }

    pub(crate) fn edge_active(&self) -> bool {
        self.drag.as_ref().is_some_and(|drag| {
            drag.started
                && self.displays.iter().any(|d| {
                    contains(d.bounds, drag.point)
                        && edge_direction(d.bounds.size.height, drag.point.y - d.bounds.origin.y)
                            != 0.0
                })
        })
    }

    pub(crate) fn edge_tick(&mut self, remembered: Option<&RememberedPreviewCache>) {
        let Some(point) = self.drag.as_ref().filter(|drag| drag.started).map(|drag| drag.point)
        else {
            return;
        };
        if let Some(d) = self.displays.iter_mut().find(|d| contains(d.bounds, point)) {
            let direction = edge_direction(d.bounds.size.height, point.y - d.bounds.origin.y);
            d.workspace_offset = (d.centered as f64 + d.workspace_offset + direction * 0.2)
                .clamp(0.0, d.workspaces.len().saturating_sub(1) as f64)
                - d.centered as f64;
            d.reproject(self.previews.as_ref(), remembered);
        }
        self.move_drag(point);
        self.request_previews(remembered);
    }

    pub(crate) fn refresh(
        &mut self,
        reactor: &ReactorHandle,
        remembered: Option<&RememberedPreviewCache>,
        moved: Option<WindowId>,
    ) {
        self.end_drag();
        let selected = moved.or(self.selection.window);
        for (i, d) in self.displays.iter_mut().enumerate() {
            d.workspaces = reactor.query_workspaces(d.info.space);
            filter_workspaces(&mut d.workspaces, self.show_empty_workspaces);
            if d.workspaces.is_empty() {
                continue;
            }
            d.centered = d.centered.min(d.workspaces.len() - 1);
            d.workspace_offset = (d.centered as f64 + d.workspace_offset)
                .clamp(0.0, (d.workspaces.len() - 1) as f64)
                - d.centered as f64;
            if let Some((pos, ws)) = d
                .workspaces
                .iter()
                .enumerate()
                .find(|(_, ws)| ws.windows.iter().any(|w| Some(w.id) == selected))
                && (moved.is_some() || (self.active == i && self.selection.workspace != ws.index))
            {
                self.active = i;
                self.selection = Selection {
                    workspace: ws.index,
                    window: selected,
                };
                d.centered = pos;
                d.workspace_offset = 0.0;
            }
            if self.active == i
                && let Some(window) = moved
            {
                d.reveal(self.selection.workspace, window);
            }
            d.animate = true;
            d.rebuild(self.previews.as_ref(), remembered);
        }
        let active_uuid = self.displays.get(self.active).map(|d| d.info.display_uuid.clone());
        self.displays.retain(|d| !d.workspaces.is_empty());
        self.active = self
            .displays
            .iter()
            .position(|d| Some(&d.info.display_uuid) == active_uuid.as_ref())
            .unwrap_or(0);
        if let Some(d) = self.displays.get(self.active)
            && !d.workspaces.iter().any(|ws| ws.index == self.selection.workspace)
        {
            self.selection = Selection {
                workspace: d.workspaces[d.centered].index,
                window: None,
            };
        }
        self.highlight(None);
        self.request_previews(remembered);
    }

    fn end_drag(&mut self) -> Option<OverviewDrag> {
        let drag = self.drag.take()?;
        drag.card.removeFromSuperlayer();
        drag.indicator.removeFromSuperlayer();
        for card in self.displays.iter().flat_map(|d| &d.views).flat_map(|ws| &ws.cards) {
            if card.id == drag.intent.window {
                card.layer.setHidden(false);
            }
        }
        Some(drag)
    }

    fn begin_drag(&mut self, point: CGPoint) {
        self.end_drag();
        let Some(d) = self.displays.iter().find(|d| contains(d.bounds, point)) else {
            return;
        };
        let local = CGPoint::new(point.x - d.bounds.origin.x, point.y - d.bounds.origin.y);
        let Some(selected) = hit(&d.workspaces, &d.projection, local) else {
            return;
        };
        let Some(ws) = d.workspaces.iter().find(|ws| ws.index == selected.workspace) else {
            return;
        };
        let Some(data) = ws.windows.iter().find(|w| {
            Some(w.id) == selected.window
                && w.info.sys_id.is_some()
                && w.info.is_standard
                && !w.info.is_minimized
        }) else {
            return;
        };
        let Some(view) = d.views.iter().find(|ws| ws.index == selected.workspace) else {
            return;
        };
        let Some(source) = view.cards.iter().find(|c| c.id == data.id) else {
            return;
        };
        let card = layer(
            rect(
                0.0,
                0.0,
                source.layer.frame().size.width,
                source.layer.frame().size.height,
            ),
            d.scale,
        );
        let height = (source.layer.frame().size.height - CAPTION).max(1.0);
        let preview = layer(rect(0.0, 0.0, source.layer.frame().size.width, height), d.scale);
        unsafe {
            preview.setContents(source.image.contents().as_deref());
            preview.setContentsGravity(objc2_quartz_core::kCAGravityResizeAspect);
        }
        preview.setCornerRadius(CORNER);
        preview.setMasksToBounds(true);
        preview.setBackgroundColor(Some(&color(0.13, 0.13, 0.14, 1.0)));
        preview.setBorderColor(Some(&color(0.0, 0.48, 1.0, 1.0)));
        preview.setBorderWidth(2.0);
        card.addSublayer(&preview);
        label(
            &card,
            &data.info.title,
            rect(
                4.0,
                height + 7.0,
                (source.layer.frame().size.width - 8.0).max(0.0),
                20.0,
            ),
            d.scale,
        );
        let indicator = layer(CGRect::ZERO, d.scale);
        indicator.setBackgroundColor(Some(&color(0.0, 0.48, 1.0, 0.25)));
        self.drag = Some(OverviewDrag {
            intent: crate::actor::reactor::OverviewDrop {
                window: data.id,
                server_id: data.info.sys_id,
                bundle: data.info.bundle_id.clone(),
                source_space: d.info.space.unwrap(),
                source_workspace: ws.id.clone(),
                display: d.info.display_uuid.clone(),
                space: d.info.space.unwrap(),
                workspace: ws.id.clone(),
                floating: data.is_floating,
                target: None,
                frame: None,
            },
            start: point,
            point,
            size: source.layer.frame().size,
            started: false,
            card,
            preview,
            indicator,
        });
    }

    fn move_drag(&mut self, point: CGPoint) {
        let Some(drag) = &mut self.drag else { return };
        drag.point = point;
        if !drag.started && (point.x - drag.start.x).hypot(point.y - drag.start.y) < 5.0 {
            return;
        }
        drag.started = true;
        drag.intent.workspace.clear();
        with_disabled_actions(|| {
            for card in self.displays.iter().flat_map(|d| &d.views).flat_map(|ws| &ws.cards) {
                if card.id == drag.intent.window {
                    card.layer.setHidden(true);
                }
            }
            let Some(d) = self.displays.iter().find(|d| contains(d.bounds, point)) else {
                drag.card.setHidden(true);
                drag.indicator.setHidden(true);
                return;
            };
            let local = CGPoint::new(point.x - d.bounds.origin.x, point.y - d.bounds.origin.y);
            d.root.addSublayer(&drag.indicator);
            d.root.addSublayer(&drag.card);
            drag.card.setHidden(false);
            drag.card.setFrame(rect(
                local.x - drag.size.width / 2.0,
                local.y - drag.size.height / 2.0,
                drag.size.width,
                drag.size.height,
            ));
            drag.indicator.setHidden(true);
            let Some(ws) = d.projection.iter().find(|ws| contains(ws.frame, local)) else {
                return;
            };
            let data = &d.workspaces[ws.source];
            drag.intent.display.clone_from(&d.info.display_uuid);
            drag.intent.space = d.info.space.unwrap();
            drag.intent.workspace.clone_from(&data.id);
            drag.intent.target = None;
            drag.intent.frame = None;
            let p = CGPoint::new(local.x, local.y - ws.frame.origin.y);
            let mut indicator = ws.frame;
            if drag.intent.floating {
                drag.intent.frame = Some(floating_drop_frame(
                    d.info.frame,
                    ws.frame,
                    drag.size,
                    p,
                    d.offsets.get(&data.index).copied().unwrap_or(0.0),
                ));
                indicator = drag.card.frame();
            } else if let Some(target) = tiled_target(data, ws, p, Some(drag.intent.window)) {
                let action = drop_action(target.frame, p, &data.layout_mode);
                if data.windows[target.source].id == drag.intent.window {
                    drag.intent.workspace.clear();
                    return;
                }
                drag.intent.target = Some((data.windows[target.source].id, action));
                indicator = target.frame;
                indicator.origin.y += ws.frame.origin.y;
                match action {
                    crate::layout_engine::WindowDropAction::Insert(
                        crate::layout_engine::Direction::Left,
                    ) => indicator.size.width = 4.0,
                    crate::layout_engine::WindowDropAction::Insert(
                        crate::layout_engine::Direction::Right,
                    ) => {
                        indicator.origin.x += indicator.size.width - 4.0;
                        indicator.size.width = 4.0;
                    }
                    crate::layout_engine::WindowDropAction::Insert(
                        crate::layout_engine::Direction::Up,
                    ) => indicator.size.height = 4.0,
                    _ => {
                        indicator.origin.y += indicator.size.height - 4.0;
                        indicator.size.height = 4.0;
                    }
                }
            }
            drag.indicator.setFrame(indicator);
            drag.indicator.setHidden(false);
        });
        CATransaction::flush();
    }

    pub(crate) fn start_previews(
        &mut self,
        generation: u64,
        enabled: bool,
        remembered: Option<&RememberedPreviewCache>,
    ) {
        if generation != self.generation {
            return;
        }
        self.previews = PreviewSession::open(enabled, generation);
        self.request_previews(remembered);
    }

    fn highlight(&self, previous: Option<(usize, Selection)>) {
        with_disabled_actions(|| {
            for (display, selection, width) in previous
                .into_iter()
                .map(|(d, s)| (d, s, 0.5))
                .chain(std::iter::once((self.active, self.selection, 2.0)))
            {
                if let Some(ws) =
                    self.displays[display].views.iter().find(|w| w.index == selection.workspace)
                {
                    if let Some(id) = selection.window {
                        if let Some(card) = ws.cards.iter().find(|c| c.id == id) {
                            card.image.setBorderWidth(width);
                            let border = if width > 0.5 {
                                color(0.0, 0.48, 1.0, 1.0)
                            } else {
                                color(1.0, 1.0, 1.0, 0.14)
                            };
                            card.image.setBorderColor(Some(&border));
                        }
                    } else {
                        let foreground = if width > 0.5 {
                            color(1.0, 1.0, 1.0, 1.0)
                        } else {
                            color(0.94, 0.94, 0.97, 0.62)
                        };
                        ws.heading.setForegroundColor(Some(&foreground));
                    }
                }
            }
        });
        CATransaction::flush();
    }

    pub(crate) fn input(
        &mut self,
        input: Input,
        remembered: Option<&RememberedPreviewCache>,
    ) -> Option<(String, Selection, Option<WindowServerId>)> {
        match input {
            Input::PointerDown(point) => {
                self.pressed = Some(point);
                self.begin_drag(point);
                return None;
            }
            Input::PointerDrag(point) => {
                self.move_drag(point);
                return None;
            }
            Input::PointerUp(point) => {
                self.move_drag(point);
                let drag = self.end_drag();
                let pressed = self.pressed.take();
                if let Some(drag) = drag.filter(|drag| drag.started) {
                    if !drag.intent.workspace.is_empty() {
                        self.drop = Some(drag.intent);
                    }
                    return None;
                }
                if pressed.is_some() {
                    return self.input(Input::Click(point), remembered);
                }
                return None;
            }
            Input::Move(_) if self.drag.is_some() => return None,
            _ => {}
        }
        if self.displays.is_empty() {
            return None;
        }
        let old = (self.active, self.selection);
        let mut reprojected = false;
        let mut only_translated = false;
        let activate = matches!(input, Input::Activate | Input::Click(_));
        match input {
            Input::Move(point) | Input::Click(point) => {
                let i = self.displays.iter().position(|d| contains(d.bounds, point))?;
                let d = &self.displays[i];
                let local = CGPoint::new(point.x - d.bounds.origin.x, point.y - d.bounds.origin.y);
                let selection = hit(&d.workspaces, &d.projection, local)?;
                self.active = i;
                self.selection = selection;
            }
            Input::Scroll { point, delta } => {
                let i = self.displays.iter().position(|d| contains(d.bounds, point))?;
                self.active = i;
                let d = &mut self.displays[i];
                if delta.x == 0.0 && delta.y == 0.0 {
                    return None;
                }
                let mut refresh_previews = true;
                let mut translated = false;
                if vertical_scroll(delta) {
                    let stride = ((d.bounds.size.height - 2.0 * GAP) * 0.5).max(1.0) + GAP;
                    let previous = d.workspace_offset;
                    d.workspace_offset = scrolled_workspace_offset(
                        d.centered,
                        d.workspace_offset,
                        delta.y,
                        stride,
                        d.workspaces.len(),
                    );
                    reprojected = d.workspace_offset != previous;
                    if reprojected && let Some(changed) = d.translate_vertical(previous) {
                        translated = true;
                        refresh_previews = changed;
                    }
                    let selected = (d.centered as f64 + d.workspace_offset).round() as usize;
                    self.selection = Selection {
                        workspace: d.workspaces[selected].index,
                        window: None,
                    };
                } else {
                    let local =
                        CGPoint::new(point.x - d.bounds.origin.x, point.y - d.bounds.origin.y);
                    let target = hit(&d.workspaces, &d.projection, local)?;
                    let ws = d.workspaces.iter().find(|w| w.index == target.workspace)?;
                    let offset = d.offsets.entry(ws.index).or_default();
                    let previous = *offset;
                    *offset = scrolled_horizontal_offset(
                        d.info.frame,
                        d.bounds.size,
                        ws,
                        *offset,
                        delta.x,
                    );
                    reprojected = *offset != previous;
                    self.selection = target;
                }
                if reprojected {
                    only_translated = translated;
                    if !translated {
                        d.reproject(self.previews.as_ref(), remembered);
                    }
                    if refresh_previews {
                        self.request_previews(remembered);
                    }
                }
                if let Some(point) = self.drag.as_ref().map(|drag| drag.point) {
                    self.move_drag(point);
                }
            }
            Input::Up | Input::Down => {
                let d = &mut self.displays[self.active];
                let pos = d
                    .workspaces
                    .iter()
                    .position(|w| w.index == self.selection.workspace)
                    .unwrap_or(d.centered);
                d.centered = if matches!(input, Input::Up) {
                    pos.saturating_sub(1)
                } else {
                    (pos + 1).min(d.workspaces.len() - 1)
                };
                d.workspace_offset = 0.0;
                self.selection = Selection {
                    workspace: d.workspaces[d.centered].index,
                    window: None,
                };
                d.reproject(self.previews.as_ref(), remembered);
                reprojected = true;
                self.request_previews(remembered);
            }
            Input::Left | Input::Right | Input::Cycle(_) => {
                let d = &mut self.displays[self.active];
                let ws = d.workspaces.iter().find(|w| w.index == self.selection.workspace)?;
                if !ws.windows.is_empty() {
                    let forward = matches!(input, Input::Right | Input::Cycle(true));
                    let pos = ws.windows.iter().position(|w| Some(w.id) == self.selection.window);
                    let next = match pos {
                        Some(p) if forward => (p + 1) % ws.windows.len(),
                        Some(p) => (p + ws.windows.len() - 1) % ws.windows.len(),
                        None if forward => 0,
                        None => ws.windows.len() - 1,
                    };
                    let w = &ws.windows[next];
                    self.selection.window = Some(w.id);
                    if d.reveal(self.selection.workspace, w.id) {
                        d.reproject(self.previews.as_ref(), remembered);
                        reprojected = true;
                        self.request_previews(remembered);
                    }
                }
            }
            _ => {}
        }
        if old != (self.active, self.selection) || (reprojected && !only_translated) {
            self.highlight(Some(old));
        } else if reprojected {
            CATransaction::flush();
        }
        if activate {
            let d = &self.displays[self.active];
            let sys_id = self
                .selection
                .window
                .and_then(|id| d.workspaces.iter().flat_map(|w| &w.windows).find(|w| w.id == id))
                .and_then(|w| w.info.sys_id);
            return Some((d.info.display_uuid.clone(), self.selection, sys_id));
        }
        None
    }

    fn request_previews(&mut self, remembered: Option<&RememberedPreviewCache>) {
        let Some(previews) = &mut self.previews else {
            return;
        };
        if previews.failed {
            return;
        }
        let mut visible = HashSet::default();
        // The lifted layer shares this image; account for its pixels inside the session cap.
        if let Some(drag) = &self.drag {
            visible.insert(drag.intent.window);
        }
        for i in (0..self.displays.len()).map(|n| (self.active + n) % self.displays.len()) {
            let display = &self.displays[i];
            let viewport = rect(0.0, 0.0, display.bounds.size.width, display.bounds.size.height);
            let mut ribbons: Vec<_> =
                display.projection.iter().filter(|ws| intersects(ws.frame, viewport)).collect();
            ribbons.sort_by_key(|ws| {
                ((ws.source as f64 - display.centered as f64 - display.workspace_offset).abs()
                    * 1000.0) as usize
            });
            for ws in ribbons {
                for w in &ws.windows {
                    let data = &display.workspaces[ws.source].windows[w.source];
                    if !card_visible(display.bounds.size, ws.frame, w.frame) {
                        continue;
                    }
                    if let Some(sys_id) = data.info.sys_id {
                        visible.insert(data.id);
                        if previews.schedule.needs(data.id) {
                            let (width, height) = capture_size(w.frame.size, display.scale);
                            previews.schedule.pending.push(PreviewRequest {
                                id: data.id,
                                sys_id,
                                bundle: data.info.bundle_id.clone(),
                                width,
                                height,
                            });
                        }
                    }
                }
            }
        }
        if previews.visible == visible {
            previews.pump(self.generation);
            return;
        }
        for id in previews.visible.difference(&visible) {
            // Keep in-flight attempts deduplicated; successful nonresident captures
            // clear their own marker on completion.
            if previews.images.contains_key(id) {
                previews.schedule.attempted.remove(id);
            }
        }
        previews.visible = visible.clone();
        previews.images.retain(|id, _| {
            if visible.contains(id) {
                true
            } else {
                previews.schedule.attempted.remove(id);
                false
            }
        });
        // Divide the bounded pixel budget among visible cards, including queued captures.
        let per_image_bytes = SESSION_PREVIEW_BYTES / visible.len().max(1);
        for image in previews.images.values_mut() {
            if image_bytes(image) > per_image_bytes {
                let factor = (per_image_bytes as f64 / image_bytes(image) as f64).sqrt();
                if let Some(smaller) = resized_preview(image, factor) {
                    *image = smaller;
                }
            }
        }
        let pixel_budget = per_image_bytes / 4;
        for request in &mut previews.schedule.pending {
            let factor = (pixel_budget as f64 / (request.width * request.height).max(1) as f64)
                .sqrt()
                .min(1.0);
            request.width = (request.width as f64 * factor).floor().max(1.0) as usize;
            request.height = (request.height as f64 * factor).floor().max(1.0) as usize;
        }
        previews.schedule.pending.retain(|p| visible.contains(&p.id));
        previews.schedule.pending.sort_by_key(|p| self.selection.window != Some(p.id));
        previews.pump(self.generation);
        self.sync_images(remembered);
    }

    pub(crate) fn preview_ready(
        &mut self,
        result: PreviewEvent,
        current: Option<&crate::model::server::RuntimeWindowData>,
        remembered: &mut Option<RememberedPreviewCache>,
    ) {
        let Some(previews) = &mut self.previews else {
            return;
        };
        match result {
            PreviewEvent::Content(generation, content) if generation == self.generation => {
                if let Some(content) = content {
                    previews.windows = Some(unsafe {
                        content.windows().iter().map(|w| (w.windowID(), w)).collect()
                    });
                } else {
                    previews.schedule.pending.clear();
                    previews.failed = true;
                }
            }
            PreviewEvent::Image(generation, request, image) if generation == self.generation => {
                previews.schedule.complete();
                if preview_is_current(generation, self.generation, &request, current)
                    && let Some(image) = image
                {
                    let compact = compact_preview(&image);
                    // Small cards share the independent compact bitmap with the cache.
                    let image = if CGImage::width(Some(&image)) <= REMEMBERED_EDGE
                        && CGImage::height(Some(&image)) <= REMEMBERED_EDGE
                    {
                        compact.as_ref().cloned().unwrap_or(image)
                    } else {
                        image
                    };
                    if let Some(compact) = compact {
                        remembered
                            .get_or_insert_with(RememberedPreviewCache::default)
                            .insert(&request, compact);
                    }
                    let visible = previews.visible.contains(&request.id)
                        || self.displays.iter().any(|d| {
                            d.projection.iter().any(|ws| {
                                ws.windows.iter().any(|w| {
                                    d.workspaces[ws.source].windows[w.source].id == request.id
                                        && card_visible(d.bounds.size, ws.frame, w.frame)
                                })
                            })
                        });
                    let bytes = previews
                        .images
                        .iter()
                        .filter(|(id, _)| **id != request.id)
                        .map(|(_, image)| image_bytes(image))
                        .sum::<usize>();
                    let available = SESSION_PREVIEW_BYTES.saturating_sub(bytes);
                    let fair = SESSION_PREVIEW_BYTES / previews.visible.len().max(1);
                    let budget = available.min(fair);
                    let image = if image_bytes(&image) > budget && budget >= 4 {
                        let factor = (budget as f64 / image_bytes(&image) as f64).sqrt();
                        resized_preview(&image, factor)
                    } else {
                        Some(image)
                    };
                    if visible && let Some(image) = image.filter(|i| image_bytes(i) <= available) {
                        previews.images.insert(request.id, image);
                    } else {
                        // A successful capture discarded offscreen must be eligible on return.
                        previews.schedule.attempted.remove(&request.id);
                    }
                }
            }
            _ => return,
        }
        previews.pump(self.generation);
        self.sync_images(remembered.as_ref());
        CATransaction::flush();
    }

    fn sync_images(&self, remembered: Option<&RememberedPreviewCache>) {
        with_disabled_actions(|| {
            if let Some(drag) = &self.drag {
                let data = self
                    .displays
                    .iter()
                    .flat_map(|d| &d.workspaces)
                    .flat_map(|ws| &ws.windows)
                    .find(|w| w.id == drag.intent.window);
                let image = data.and_then(|data| {
                    self.previews
                        .as_ref()
                        .and_then(|p| p.images.get(&data.id))
                        .or_else(|| remembered.and_then(|c| c.get(data)))
                });
                set_image(&drag.preview, image.map(|i| &**i));
            }
            for display in &self.displays {
                for (view, ws) in display.views.iter().zip(&display.projection) {
                    for (card, w) in view.cards.iter().zip(&ws.windows) {
                        let data = &display.workspaces[ws.source].windows[w.source];
                        let image = self
                            .previews
                            .as_ref()
                            .and_then(|p| p.images.get(&data.id))
                            .or_else(|| remembered.and_then(|c| c.get(data)));
                        set_image(&card.image, image.map(|i| &**i));
                    }
                }
            }
        });
    }
}

#[derive(Debug)]
pub(crate) struct PreviewRequest {
    id: WindowId,
    sys_id: WindowServerId,
    bundle: Option<String>,
    width: usize,
    height: usize,
}
#[derive(Debug)]
pub(crate) enum PreviewEvent {
    Content(u64, Option<Retained<SCShareableContent>>),
    Image(u64, PreviewRequest, Option<CFRetained<CGImage>>),
}

impl PreviewEvent {
    pub(crate) fn window_id(&self) -> Option<WindowId> {
        match self {
            Self::Image(_, request, _) => Some(request.id),
            _ => None,
        }
    }
}

/// Callbacks transfer retained native results to the main queue before sending to this
/// local channel. It never crosses the input thread and needs no unsafe Send wrapper.
fn deliver(tx: actor::Sender<PreviewEvent>, result: PreviewEvent) {
    dispatchr::queue::main().after_f_s(dispatchr::time::Time::NOW, (tx, result), |(tx, result)| {
        tx.send(result)
    });
}

pub(crate) struct PreviewSession {
    pub(crate) rx: actor::Receiver<PreviewEvent>,
    tx: actor::Sender<PreviewEvent>,
    windows: Option<HashMap<u32, Retained<SCWindow>>>,
    schedule: PreviewSchedule,
    failed: bool,
    visible: HashSet<WindowId>,
    images: HashMap<WindowId, CFRetained<CGImage>>,
}

impl PreviewSession {
    fn open(enabled: bool, generation: u64) -> Option<Self> {
        if !enabled {
            return None;
        }
        if !CGPreflightScreenCaptureAccess() {
            tracing::info!(
                "Overview previews unavailable: allow Rift in System Settings > Privacy & Security > Screen Recording"
            );
            return None;
        }
        Some(Self::new(generation))
    }

    fn new(generation: u64) -> Self {
        let (tx, rx) = actor::channel();
        let callback_tx = tx.clone();
        let callback = RcBlock::new(move |content: *mut SCShareableContent, _: *mut NSError| {
            let content = unsafe { Retained::retain(content) };
            deliver(callback_tx.clone(), PreviewEvent::Content(generation, content));
        });
        unsafe {
            SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(true,false,&callback);
        }
        Self {
            rx,
            tx,
            windows: None,
            schedule: PreviewSchedule::default(),
            failed: false,
            visible: HashSet::default(),
            images: HashMap::default(),
        }
    }

    fn pump(&mut self, generation: u64) {
        if self.failed {
            return;
        }
        let Some(windows) = &self.windows else {
            return;
        };
        while let Some(request) = self.schedule.next() {
            let Some(window) = windows.get(&request.sys_id.as_u32()) else {
                self.schedule.complete();
                continue;
            };
            let owner = unsafe { window.owningApplication() };
            if !owner.is_some_and(|app| unsafe {
                owner_matches(&request, app.processID(), &app.bundleIdentifier().to_string())
            }) {
                self.schedule.complete();
                continue;
            }
            let filter = unsafe {
                SCContentFilter::initWithDesktopIndependentWindow(SCContentFilter::alloc(), window)
            };
            let config = unsafe { SCStreamConfiguration::new() };
            unsafe {
                config.setWidth(request.width);
                config.setHeight(request.height);
                config.setShowsCursor(false);
                config.setCapturesAudio(false);
                config.setIgnoreShadowsSingleWindow(true);
                config.setIgnoreGlobalClipSingleWindow(true);
                config.setScalesToFit(true);
                config.setPreservesAspectRatio(true);
            }
            let tx = self.tx.clone();
            let callback = RcBlock::new(move |image: *mut CGImage, _: *mut NSError| {
                let image = NonNull::new(image).map(|image| unsafe { CFRetained::retain(image) });
                deliver(
                    tx.clone(),
                    PreviewEvent::Image(
                        generation,
                        PreviewRequest {
                            id: request.id,
                            sys_id: request.sys_id,
                            bundle: request.bundle.clone(),
                            width: request.width,
                            height: request.height,
                        },
                        image,
                    ),
                );
            });
            unsafe {
                SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(
                    &filter,
                    &config,
                    Some(&callback),
                );
            }
        }
    }
}

#[derive(Default)]
struct PreviewSchedule {
    attempted: HashSet<WindowId>,
    pending: Vec<PreviewRequest>,
    active: usize,
}
impl PreviewSchedule {
    fn needs(&self, id: WindowId) -> bool {
        !self.attempted.contains(&id) && !self.pending.iter().any(|p| p.id == id)
    }

    fn next(&mut self) -> Option<PreviewRequest> {
        if self.active >= 2 || self.pending.is_empty() {
            return None;
        }
        self.active += 1;
        let request = self.pending.remove(0);
        self.attempted.insert(request.id);
        Some(request)
    }

    fn complete(&mut self) { self.active = self.active.saturating_sub(1); }
}
fn owner_matches(request: &PreviewRequest, pid: i32, bundle: &str) -> bool {
    request.id.pid == pid && request.bundle.as_ref().is_none_or(|expected| expected == bundle)
}
fn preview_is_current(
    generation: u64,
    current_generation: u64,
    request: &PreviewRequest,
    window: Option<&crate::model::server::RuntimeWindowData>,
) -> bool {
    generation == current_generation
        && window.is_some_and(|w| {
            w.id == request.id
                && w.info.sys_id == Some(request.sys_id)
                && w.info.bundle_id == request.bundle
        })
}

const REMEMBERED_EDGE: usize = 320;
const REMEMBERED_BYTES: usize = 4 * 1024 * 1024;
const SESSION_PREVIEW_BYTES: usize = 8 * 1024 * 1024;

fn image_bytes(image: &CGImage) -> usize {
    CGImage::bytes_per_row(Some(image)).saturating_mul(CGImage::height(Some(image)))
}
fn capture_size(size: CGSize, backing: f64) -> (usize, usize) {
    let width = (size.width * backing).max(1.0);
    let height = (size.height * backing).max(1.0);
    let scale = (1024.0 / width).min(768.0 / height).min(1.0);
    ((width * scale).ceil() as usize, (height * scale).ceil() as usize)
}
fn compact_preview(image: &CGImage) -> Option<CFRetained<CGImage>> {
    let width = CGImage::width(Some(image));
    let height = CGImage::height(Some(image));
    if width == 0 || height == 0 {
        return None;
    }
    let scale = (REMEMBERED_EDGE as f64 / width.max(height) as f64).min(1.0);
    resized_preview(image, scale)
}
fn resized_preview(image: &CGImage, scale: f64) -> Option<CFRetained<CGImage>> {
    let width = CGImage::width(Some(image));
    let height = CGImage::height(Some(image));
    let width = (width as f64 * scale).floor().max(1.0) as usize;
    let height = (height as f64 * scale).floor().max(1.0) as usize;
    let space = CGColorSpace::new_device_rgb()?;
    // Always make an independent bitmap: a cropped/resized view could retain the
    // original capture's backing storage while Overview is closed.
    let context = unsafe {
        CGBitmapContextCreate(
            std::ptr::null_mut(),
            width,
            height,
            8,
            width * 4,
            Some(&space),
            CGImageAlphaInfo::PremultipliedLast.0,
        )
    }?;
    CGContext::draw_image(
        Some(&context),
        rect(0.0, 0.0, width as f64, height as f64),
        Some(image),
    );
    CGBitmapContextCreateImage(Some(&context))
}
fn set_image(layer: &CALayer, image: Option<&CGImage>) {
    let pointer = image.map_or(std::ptr::null(), |i| i as *const CGImage as *const AnyObject);
    let current = unsafe { layer.contents() };
    if current.as_ref().map_or(std::ptr::null(), Retained::as_ptr) != pointer {
        unsafe {
            layer.setContents(image.map(|i| &*(i as *const CGImage as *const AnyObject)));
        }
    }
}
struct RememberedPreview {
    id: WindowId,
    sys_id: WindowServerId,
    bundle: Option<String>,
    image: CFRetained<CGImage>,
}
#[derive(Default)]
pub(crate) struct RememberedPreviewCache {
    entries: Vec<RememberedPreview>,
}
impl RememberedPreviewCache {
    fn bytes(&self) -> usize { self.entries.iter().map(|e| image_bytes(&e.image)).sum() }

    fn get(&self, window: &RuntimeWindowData) -> Option<&CFRetained<CGImage>> {
        self.entries
            .iter()
            .find(|e| {
                e.id == window.id
                    && Some(e.sys_id) == window.info.sys_id
                    && e.bundle == window.info.bundle_id
            })
            .map(|e| &e.image)
    }

    fn prune<'a>(&mut self, windows: impl Iterator<Item = &'a RuntimeWindowData> + Clone) {
        self.entries.retain(|e| {
            windows.clone().any(|w| {
                w.id == e.id && w.info.sys_id == Some(e.sys_id) && w.info.bundle_id == e.bundle
            })
        });
    }

    fn insert(&mut self, request: &PreviewRequest, image: CFRetained<CGImage>) {
        self.entries.retain(|e| e.id != request.id);
        let cost = image_bytes(&image);
        if cost > REMEMBERED_BYTES {
            return;
        }
        while !self.entries.is_empty()
            && (self.entries.len() >= 32 || self.bytes() > REMEMBERED_BYTES - cost)
        {
            self.entries.remove(0);
        }
        self.entries.push(RememberedPreview {
            id: request.id,
            sys_id: request.sys_id,
            bundle: request.bundle.clone(),
            image,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::server::RuntimeWindowData;

    fn window(id: u32, frame: CGRect, floating: bool) -> RuntimeWindowData {
        serde_json::from_value(serde_json::json!({
            "id": {"pid": 123,"idx": id}, "title": "Example", "frame": {
                "origin": {"x": frame.origin.x,"y": frame.origin.y},
                "size": {"width": frame.size.width,"height": frame.size.height}
            }, "is_floating": floating,"is_focused": false,"window_server_id": id,
            "bundle_id": "org.example"
        }))
        .unwrap()
    }
    fn workspace(index: usize, windows: Vec<RuntimeWindowData>) -> RuntimeWorkspaceData {
        RuntimeWorkspaceData {
            id: index.to_string(),
            index,
            name: index.to_string(),
            layout_mode: "scrolling".into(),
            is_active: index == 1,
            window_count: windows.len(),
            windows,
        }
    }
    fn request(id: u32) -> PreviewRequest {
        PreviewRequest {
            id: WindowId::new(123, id),
            sys_id: WindowServerId::new(id),
            bundle: Some("org.example".into()),
            width: 200,
            height: 100,
        }
    }

    fn bitmap(width: usize, height: usize) -> CFRetained<CGImage> {
        let space = CGColorSpace::new_device_rgb().unwrap();
        let context = unsafe {
            CGBitmapContextCreate(
                std::ptr::null_mut(),
                width,
                height,
                8,
                width * 4,
                Some(&space),
                CGImageAlphaInfo::PremultipliedLast.0,
            )
        }
        .unwrap();
        CGBitmapContextCreateImage(Some(&context)).unwrap()
    }

    #[test]
    fn compact_cache_does_not_retain_large_capture_storage() {
        let source = bitmap(1024, 768);
        let source_ref: &CGImage = &source;
        let source_cf: &objc2_core_foundation::CFType = source_ref.as_ref();
        let count = source_cf.retain_count();
        let image = compact_preview(&source).unwrap();
        assert_eq!(CGImage::width(Some(&image)), 320);
        assert_eq!(CGImage::height(Some(&image)), 240);
        assert_eq!(image_bytes(&image), 320 * 240 * 4);
        assert_eq!(source_cf.retain_count(), count);
        let mut cache = RememberedPreviewCache::default();
        cache.insert(&request(1), image);
        drop(source);
        assert_eq!(cache.bytes(), 320 * 240 * 4);
        assert!(cache.get(&window(1, rect(0.0, 0.0, 100.0, 100.0), false)).is_some());
    }

    #[test]
    fn remembered_cache_enforces_byte_and_entry_caps_and_prunes_identity() {
        let mut cache = RememberedPreviewCache::default();
        for id in 1..65 {
            cache.insert(&request(id), bitmap(320, 240));
            assert!(cache.bytes() <= REMEMBERED_BYTES);
            assert!(cache.entries.len() <= 32);
        }
        let mut current = window(64, rect(0.0, 0.0, 100.0, 100.0), false);
        assert!(cache.get(&current).is_some());
        current.info.sys_id = Some(WindowServerId::new(99));
        assert!(cache.get(&current).is_none());
        cache.prune(std::iter::once(&current));
        assert!(cache.entries.is_empty());
        assert_eq!(cache.bytes(), 0);
        for id in 1..65 {
            cache.insert(&request(id), bitmap(1, 1));
        }
        assert_eq!(cache.entries.len(), 32);
        assert!(cache.entries.iter().all(|e| e.id.idx.get() > 32));
    }

    #[test]
    fn capture_resolution_is_bounded_and_preserves_aspect_ratio() {
        assert_eq!(capture_size(CGSize::new(480.0, 300.0), 2.0), (960, 600));
        assert_eq!(capture_size(CGSize::new(5000.0, 3750.0), 2.0), (1024, 768));
        assert_eq!(capture_size(CGSize::new(100.0, 1000.0), 2.0), (77, 768));
    }

    #[test]
    fn clipped_cards_outside_viewport_are_not_retained_at_full_quality() {
        let bounds = CGSize::new(1200.0, 800.0);
        assert!(!card_visible(
            bounds,
            rect(0.0, 900.0, 1200.0, 300.0),
            rect(0.0, -200.0, 100.0, 500.0)
        ));
        assert!(!card_visible(
            bounds,
            rect(0.0, 700.0, 1200.0, 300.0),
            rect(0.0, 150.0, 100.0, 100.0)
        ));
        assert!(card_visible(
            bounds,
            rect(0.0, 700.0, 1200.0, 300.0),
            rect(0.0, 0.0, 100.0, 100.0)
        ));
    }

    #[test]
    fn drop_regions_and_stack_row_use_projected_geometry() {
        use crate::layout_engine::{Direction, WindowDropAction};
        let frame = rect(100.0, 50.0, 200.0, 100.0);
        assert_eq!(
            drop_action(frame, CGPoint::new(120.0, 100.0), "scrolling"),
            WindowDropAction::Insert(Direction::Left)
        );
        assert_eq!(
            drop_action(frame, CGPoint::new(280.0, 100.0), "scrolling"),
            WindowDropAction::Insert(Direction::Right)
        );
        assert_eq!(
            drop_action(frame, CGPoint::new(200.0, 80.0), "scrolling"),
            WindowDropAction::Insert(Direction::Up)
        );
        assert_eq!(
            drop_action(frame, CGPoint::new(200.0, 120.0), "scrolling"),
            WindowDropAction::Stack
        );
        let data = workspace(0, vec![
            window(1, rect(0.0, 0.0, 200.0, 100.0), false),
            window(2, rect(0.0, 100.0, 200.0, 100.0), false),
        ]);
        let ws = WorkspaceProjection {
            source: 0,
            frame: rect(0.0, 0.0, 600.0, 400.0),
            windows: vec![
                WindowProjection {
                    source: 0,
                    frame: rect(100.0, 0.0, 200.0, 100.0),
                },
                WindowProjection {
                    source: 1,
                    frame: rect(100.0, 100.0, 200.0, 100.0),
                },
            ],
        };
        assert_eq!(
            tiled_target(&data, &ws, CGPoint::new(110.0, 60.0), Some(data.windows[0].id))
                .unwrap()
                .source,
            1
        );
        assert_eq!(
            tiled_target(&data, &ws, CGPoint::new(200.0, 60.0), None).unwrap().source,
            0
        );
        assert_eq!(
            tiled_target(&data, &ws, CGPoint::new(200.0, 160.0), None).unwrap().source,
            1
        );
        assert!(
            tiled_target(
                &workspace(0, vec![]),
                &WorkspaceProjection {
                    source: 0,
                    frame: ws.frame,
                    windows: vec![]
                },
                CGPoint::new(200.0, 100.0),
                None
            )
            .is_none()
        );
    }

    #[test]
    fn floating_drop_inverse_projection_rebases_and_clamps() {
        let display = rect(1200.0, -200.0, 1200.0, 800.0);
        let ribbon = rect(0.0, 300.0, 1200.0, 400.0);
        let size = CGSize::new(200.0, 100.0);
        assert_eq!(
            floating_drop_frame(display, ribbon, size, CGPoint::new(600.0, 200.0), 0.0),
            rect(1600.0, 100.0, 400.0, 200.0)
        );
        assert_eq!(
            floating_drop_frame(display, ribbon, size, CGPoint::new(-100.0, -100.0), 0.0).origin,
            display.origin
        );
        assert_eq!(
            floating_drop_frame(display, ribbon, size, CGPoint::new(2000.0, 1000.0), 0.0).origin,
            CGPoint::new(2000.0, 400.0)
        );
    }

    #[test]
    fn drag_edge_scroll_is_bounded_presentation_state() {
        assert_eq!(edge_direction(800.0, 20.0), -1.0);
        assert_eq!(edge_direction(800.0, 400.0), 0.0);
        assert_eq!(edge_direction(800.0, 790.0), 1.0);
        assert!((scrolled_workspace_offset(1, 0.0, -80.0, 400.0, 5) - 0.2).abs() < 1e-12);
        assert_eq!(scrolled_workspace_offset(1, 3.0, -80.0, 400.0, 5), 3.0);
    }

    #[test]
    fn dominant_axis_and_pointer_workspace_pan_are_independent_and_bounded() {
        assert!(vertical_scroll(CGPoint::new(10.0, -10.0)));
        assert!(!vertical_scroll(CGPoint::new(-11.0, 10.0)));
        let display = rect(0.0, 0.0, 1200.0, 800.0);
        let workspaces: Vec<_> = (0..3)
            .map(|index| {
                workspace(index, vec![window(
                    index as u32 + 1,
                    rect(2500.0, 0.0, 400.0, 800.0),
                    false,
                )])
            })
            .collect();
        let before = format!("{workspaces:?}");
        let mut offsets = HashMap::from_iter([(0, 25.0), (2, 50.0)]);
        let projection = project(display, display.size, &workspaces, 1, 0.0, &offsets);
        let selected = hit(&workspaces, &projection, CGPoint::new(600.0, 400.0)).unwrap();
        assert_eq!(selected.workspace, 1);
        let next = scrolled_horizontal_offset(display, display.size, &workspaces[1], 0.0, -5000.0);
        assert!(next > 0.0);
        assert_eq!(
            next,
            scrolled_horizontal_offset(display, display.size, &workspaces[1], next, -5000.0)
        );
        offsets.insert(selected.workspace, next);
        assert_eq!(offsets[&0], 25.0);
        assert_eq!(offsets[&2], 50.0);
        assert_eq!(format!("{workspaces:?}"), before);
        assert_eq!(
            scrolled_horizontal_offset(display, display.size, &workspace(0, vec![]), 0.0, 5000.0),
            0.0
        );
    }

    #[test]
    fn empty_workspaces_are_hidden_without_renumbering() {
        let original = vec![
            workspace(0, vec![]),
            workspace(3, vec![window(1, rect(0.0, 0.0, 400.0, 300.0), false)]),
            workspace(7, vec![]),
        ];
        let mut filtered = original.clone();
        filter_workspaces(&mut filtered, false);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].index, 3);
        let mut included = original;
        filter_workspaces(&mut included, true);
        assert_eq!(included.len(), 3);
        filtered[0].windows.clear();
        filter_workspaces(&mut filtered, false);
        assert!(filtered.is_empty());
    }

    #[test]
    fn wheel_scrolling_moves_ribbons_smoothly_and_clamps_to_workspace_stack() {
        let workspaces: Vec<_> = (0..3).map(|i| workspace(i, vec![])).collect();
        let display = rect(0.0, 0.0, 1200.0, 800.0);
        let offset = scrolled_workspace_offset(1, 0.0, -100.0, 400.0, 3);
        assert_eq!(offset, 0.25);
        let projection = project(
            display,
            display.size,
            &workspaces,
            1,
            offset,
            &HashMap::default(),
        );
        assert_eq!(projection[1].frame.origin.y, 120.0);
        assert_eq!(scrolled_workspace_offset(1, offset, -10000.0, 400.0, 3), 1.0);
        assert_eq!(scrolled_workspace_offset(1, offset, 10000.0, 400.0, 3), -1.0);
    }

    #[test]
    fn ribbons_center_span_width_keep_empty_and_use_expected_stride() {
        let display = rect(0.0, 0.0, 1200.0, 800.0);
        let workspaces = vec![
            workspace(0, vec![]),
            workspace(1, vec![]),
            workspace(2, vec![]),
        ];
        let projection = project(display, display.size, &workspaces, 1, 0.0, &HashMap::default());
        assert_eq!(projection.len(), 3);
        assert_eq!(projection[1].frame, rect(0.0, 220.0, 1200.0, 360.0));
        assert_eq!(
            projection[2].frame.origin.y - projection[1].frame.origin.y,
            400.0
        );
        assert!(projection.iter().all(|p| p.frame.size.width == 1200.0 && p.windows.is_empty()));
        assert_eq!(
            hit(&workspaces, &projection, CGPoint::new(600.0, 400.0)),
            Some(Selection { workspace: 1, window: None })
        );
    }

    #[test]
    fn geometry_uses_source_display_and_floating_cards_are_on_top() {
        let display = rect(1600.0, -100.0, 1200.0, 800.0);
        let workspaces = vec![workspace(1, vec![
            window(1, rect(1700.0, 0.0, 400.0, 200.0), true),
            window(2, rect(1700.0, 0.0, 400.0, 200.0), false),
        ])];
        let projection = project(display, display.size, &workspaces, 0, 0.0, &HashMap::default());
        let cards = &projection[0].windows;
        assert_eq!(cards[0].source, 1);
        assert_eq!(cards[1].source, 0);
        assert_eq!(cards[0].frame, rect(375.0, 45.0, 180.0, 90.0));
        assert_eq!(
            hit(&workspaces, &projection, CGPoint::new(400.0, 280.0)),
            Some(Selection {
                workspace: 1,
                window: Some(WindowId::new(123, 1))
            })
        );
        assert!(hit(&workspaces, &projection, CGPoint::new(400.0, 10.0)).is_none());
    }

    #[test]
    fn scrolling_clips_windows_and_exposes_them_without_changing_runtime_frames() {
        let display = rect(0.0, 0.0, 1200.0, 800.0);
        let workspaces = vec![workspace(1, vec![
            window(1, rect(-800.0, 0.0, 400.0, 800.0), false),
            window(2, rect(2500.0, 0.0, 400.0, 800.0), false),
        ])];
        let original = workspaces[0].windows[1].info.frame;
        let projection = project(display, display.size, &workspaces, 0, 0.0, &HashMap::default());
        assert_eq!(projection[0].windows.len(), 1); // partially clipped left card
        assert!(projection[0].windows[0].frame.origin.x < 0.0);
        let offsets = HashMap::from_iter([(1, 800.0)]);
        let shifted = project(display, display.size, &workspaces, 0, 0.0, &offsets);
        assert_eq!(shifted[0].windows.len(), 1);
        assert_eq!(shifted[0].windows[0].source, 1);
        assert_eq!(workspaces[0].windows[1].info.frame, original);
    }

    #[test]
    fn separate_display_dimensions_and_workspace_sets_do_not_mix() {
        let first = vec![workspace(7, vec![])];
        let second = vec![workspace(9, vec![])];
        let a = project(
            rect(0.0, 0.0, 1200.0, 800.0),
            CGSize::new(1200.0, 800.0),
            &first,
            0,
            0.0,
            &HashMap::default(),
        );
        let b = project(
            rect(-900.0, 200.0, 900.0, 600.0),
            CGSize::new(900.0, 600.0),
            &second,
            0,
            0.0,
            &HashMap::default(),
        );
        assert_eq!(a[0].frame.size.width, 1200.0);
        assert_eq!(b[0].frame.size.width, 900.0);
        assert_eq!(hit(&first, &a, CGPoint::new(300.0, 400.0)).unwrap().workspace, 7);
        assert_eq!(
            hit(&second, &b, CGPoint::new(300.0, 300.0)).unwrap().workspace,
            9
        );
        let many: Vec<_> = (0..100).map(|i| workspace(i, vec![])).collect();
        assert!(
            project(
                rect(0.0, 0.0, 1200.0, 800.0),
                CGSize::new(1200.0, 800.0),
                &many,
                50,
                0.0,
                &HashMap::default()
            )
            .len()
                <= 5
        );
    }

    #[test]
    fn dropping_preview_session_releases_cached_and_queued_images() {
        use objc2_core_graphics::{
            CGBitmapContextCreate, CGBitmapContextCreateImage, CGColorSpace, CGImageAlphaInfo,
        };
        let space = CGColorSpace::new_device_rgb().unwrap();
        let context = unsafe {
            CGBitmapContextCreate(
                std::ptr::null_mut(),
                1,
                1,
                8,
                4,
                Some(&space),
                CGImageAlphaInfo::PremultipliedLast.0,
            )
        }
        .unwrap();
        let image = CGBitmapContextCreateImage(Some(&context)).unwrap();
        let image_ref: &CGImage = &image;
        let image_cf: &objc2_core_foundation::CFType = image_ref.as_ref();
        let count = image_cf.retain_count();
        let (tx, rx) = actor::channel();
        let mut session = PreviewSession {
            rx,
            tx: tx.clone(),
            windows: None,
            schedule: PreviewSchedule::default(),
            failed: false,
            visible: HashSet::default(),
            images: HashMap::default(),
        };
        session.images.insert(WindowId::new(123, 1), image.clone());
        tx.send(PreviewEvent::Image(1, request(2), Some(image.clone())));
        assert!(image_cf.retain_count() >= count + 2);
        drop(session);
        assert_eq!(image_cf.retain_count(), count);
        assert!(tx.try_send(PreviewEvent::Content(1, None)).is_err());
    }

    #[test]
    fn disabled_previews_never_enter_permission_or_capture_path() {
        assert!(PreviewSession::open(false, 1).is_none());
    }

    #[test]
    fn scheduling_limits_two_jobs_and_remembers_attempts_for_the_session() {
        let mut schedule = PreviewSchedule::default();
        schedule.pending.extend([request(1), request(2), request(3)]);
        assert_eq!(schedule.next().unwrap().id, WindowId::new(123, 1));
        schedule.next().unwrap();
        assert!(schedule.next().is_none());
        assert_eq!(schedule.active, 2);
        schedule.complete();
        assert_eq!(schedule.next().unwrap().id, WindowId::new(123, 3));
        assert!(!schedule.needs(WindowId::new(123, 1)));
        assert!(schedule.needs(WindowId::new(123, 4)));
        // An unstarted card removed when hidden may be queued when newly exposed.
        assert!(!schedule.attempted.contains(&WindowId::new(123, 4)));
        schedule.pending.push(request(4));
        schedule.complete();
        assert_eq!(schedule.next().unwrap().id, WindowId::new(123, 4));
    }

    #[test]
    fn discarded_or_evicted_capture_can_upgrade_on_return() {
        let mut schedule = PreviewSchedule::default();
        schedule.pending.push(request(1));
        let id = schedule.next().unwrap().id;
        assert!(!schedule.needs(id)); // No duplicate while in flight.
        schedule.complete();
        schedule.attempted.remove(&id); // Successful image discarded offscreen.
        assert!(schedule.needs(id));
        schedule.pending.push(request(1));
        assert!(!schedule.needs(id));
        assert_eq!(schedule.next().unwrap().id, id);
    }

    #[test]
    fn stale_generation_and_reused_window_owner_are_rejected() {
        let request = request(1);
        let mut current = window(1, rect(0.0, 0.0, 400.0, 200.0), false);
        assert!(preview_is_current(2, 2, &request, Some(&current)));
        assert!(!preview_is_current(1, 2, &request, Some(&current)));
        assert!(!preview_is_current(2, 2, &request, None));
        current.info.bundle_id = Some("other.app".into());
        assert!(!preview_is_current(2, 2, &request, Some(&current)));
        assert!(!owner_matches(&request, 456, "org.example"));
        assert!(!owner_matches(&request, 123, "other.app"));
        assert!(owner_matches(&request, 123, "org.example"));
    }
}
