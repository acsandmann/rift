//! Session-owned Overview. Runtime frames are truth; captured images only decorate cards.
use std::collections::VecDeque;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicUsize, Ordering};

use block2::RcBlock;
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSColor, NSFont, NSPopUpMenuWindowLevel, NSRunningApplication, NSScreen};
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
const CAPTION: f64 = 48.0;

/// All layer changes for one input/capture update reach the compositor together.
/// Nested helpers may create transactions, but only this outer scope flushes.
struct OverviewTransaction;
impl OverviewTransaction {
    fn begin() -> Self {
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        Self
    }
}
impl Drop for OverviewTransaction {
    fn drop(&mut self) {
        CATransaction::commit();
        CATransaction::flush();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub workspace: usize,
    pub window: Option<WindowId>,
}

#[derive(Debug)]
struct WindowProjection {
    id: WindowId,
    floating: bool,
    source: usize,
    frame: CGRect,
}
#[derive(Debug)]
struct WorkspaceProjection {
    source: usize,
    frame: CGRect,
    scene: crate::actor::drag::DragScene,
    windows: Vec<WindowProjection>,
}

fn preview_frame(size: CGSize) -> CGRect {
    let available_width = (size.width - 12.0).max(1.0);
    let available_height = (size.height - CAPTION - 6.0).max(1.0);
    let ratio =
        (available_width / size.width.max(1.0)).min(available_height / size.height.max(1.0));
    let width = size.width.max(1.0) * ratio;
    let height = size.height.max(1.0) * ratio;
    rect((size.width - width) / 2.0, 6.0, width, height)
}

fn pixel_aligned(frame: CGRect, scale: f64) -> CGRect {
    let snap = |value: f64| (value * scale).round() / scale;
    rect(
        snap(frame.origin.x),
        snap(frame.origin.y),
        snap(frame.size.width),
        snap(frame.size.height),
    )
}

fn workspace_backdrop(ws: &WorkspaceProjection) -> CGRect {
    let margin = 24.0_f64.min(ws.frame.size.width / 4.0);
    let width = (ws.frame.size.width * 0.55).max(360.0).min(ws.frame.size.width - margin * 2.0);
    let center = ws.frame.size.width / 2.0;
    let (left, right) = ws.windows.iter().fold(
        (center - width / 2.0, center + width / 2.0),
        |(left, right), w| {
            (
                left.min(w.frame.origin.x - 16.0),
                right.max(w.frame.origin.x + w.frame.size.width + 16.0),
            )
        },
    );
    let left = left.max(margin);
    let right = right.min(ws.frame.size.width - margin);
    rect(left, 0.0, (right - left).max(1.0), ws.frame.size.height)
}

fn workspace_hit_frame(ws: &WorkspaceProjection) -> CGRect {
    let backdrop = workspace_backdrop(ws);
    rect(
        ws.frame.origin.x + backdrop.origin.x,
        ws.frame.origin.y,
        backdrop.size.width,
        ws.frame.size.height,
    )
}

fn workspace_heading(ws: &WorkspaceProjection, scale: f64) -> CGRect {
    let backdrop = workspace_backdrop(ws);
    pixel_aligned(
        rect(
            backdrop.origin.x + 16.0,
            ws.frame.origin.y - 26.0,
            (backdrop.size.width - 32.0).max(1.0),
            20.0,
        ),
        scale,
    )
}

fn update_heading(heading: &CATextLayer, data: &RuntimeWorkspaceData) {
    let text = CFString::from_str(&format!(
        "{}  ·  {} window{}",
        if data.name.parse::<usize>().is_ok() {
            format!("Workspace {}", data.name)
        } else {
            data.name.clone()
        },
        data.windows.len(),
        if data.windows.len() == 1 { "" } else { "s" }
    ));
    unsafe {
        heading.setString(Some(&*(text.as_ref() as *const AnyObject)));
    }
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
    let scrolling = workspace.layout_mode == "scrolling";
    // Navigation is relative to the projected desktop, not the larger overlay.
    let (left, right) = if scrolling {
        (base - 12.0, base + display.size.width * scale + 12.0)
    } else {
        (12.0, overview.width - 12.0)
    };
    let (min, max) = workspace.windows.iter().filter(|w| !scrolling || !w.is_floating).fold(
        (0.0_f64, 0.0_f64),
        |(min, max), w| {
            let frame = w.layout_frame.unwrap_or(w.info.frame);
            let x = base + (frame.origin.x - display.origin.x) * scale;
            (min.min(x - left), max.max(x + frame.size.width * scale - right))
        },
    );
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
                    let w = window.layout_frame.unwrap_or(window.info.frame);
                    let projected = rect(
                        x + (w.origin.x - display.origin.x) * scale
                            - offsets.get(&ws.index).copied().unwrap_or(0.0),
                        (w.origin.y - display.origin.y) * scale,
                        w.size.width * scale,
                        w.size.height * scale,
                    );
                    intersects(projected, rect(0.0, 0.0, frame.size.width, frame.size.height))
                        .then_some(WindowProjection {
                            id: window.id,
                            floating: window.is_floating,
                            source,
                            frame: projected,
                        })
                })
                .collect();
            windows.sort_by_key(|w| w.floating);
            let scene = crate::actor::drag::DragScene {
                action_override: (ws.layout_mode == "stack")
                    .then_some(crate::layout_engine::WindowDropAction::Swap),
                source_neighbors: [false; 4],
                targets: windows
                    .iter()
                    .rev()
                    .filter_map(|w| {
                        (!w.floating).then_some(crate::actor::drag::DragSceneTarget {
                            window: w.id,
                            space: ws.space,
                            frame: w.frame,
                        })
                    })
                    .collect(),
            };
            Some(WorkspaceProjection { source, frame, scene, windows })
        })
        .collect()
}

fn hit(
    workspaces: &[RuntimeWorkspaceData],
    projection: &[WorkspaceProjection],
    point: CGPoint,
) -> Option<Selection> {
    for ws in projection.iter().rev() {
        if !contains(workspace_hit_frame(ws), point) {
            continue;
        }
        let local = CGPoint::new(point.x - ws.frame.origin.x, point.y - ws.frame.origin.y);
        let workspace = &workspaces[ws.source];
        let window = ws
            .windows
            .iter()
            .rev()
            .find(|w| {
                let image = preview_frame(w.frame.size);
                contains(
                    rect(
                        w.frame.origin.x + image.origin.x,
                        w.frame.origin.y + image.origin.y,
                        image.size.width,
                        image.size.height + CAPTION,
                    ),
                    local,
                )
            })
            .map(|w| w.id);
        return Some(Selection {
            workspace: workspace.index,
            window,
        });
    }
    None
}

pub(crate) enum OverviewAction {
    Dismiss,
    Activate {
        display: String,
        workspace: crate::model::VirtualWorkspaceId,
        selection: Selection,
        sys_id: Option<WindowServerId>,
    },
}

struct WindowCard {
    id: WindowId,
    layer: Retained<CALayer>,
    image: Retained<CALayer>,
    caption: Retained<CALayer>,
    title: Retained<CATextLayer>,
    icon: Option<Retained<CALayer>>,
}
impl WindowCard {
    fn new(
        data: &RuntimeWindowData,
        frame: CGRect,
        scale: f64,
        image_contents: Option<&AnyObject>,
        icon_contents: Option<&AnyObject>,
    ) -> Self {
        let card = layer(frame, scale);
        let image = layer(CGRect::ZERO, scale);
        image.setCornerRadius(CORNER);
        image.setMasksToBounds(true);
        unsafe {
            // Fill resized tiles, including cached images with an older aspect ratio.
            image.setContentsGravity(objc2_quartz_core::kCAGravityResize);
            image.setContents(image_contents);
        }
        card.addSublayer(&image);
        let caption = layer(CGRect::ZERO, scale);
        card.addSublayer(&caption);
        let icon = icon_contents.map(|contents| {
            let icon = layer(CGRect::ZERO, scale);
            unsafe {
                icon.setContents(Some(contents));
            }
            caption.addSublayer(&icon);
            icon
        });
        let title = label(
            &caption,
            if data.info.title.is_empty() {
                data.app_name.as_deref().unwrap_or("Window")
            } else {
                &data.info.title
            },
            CGRect::ZERO,
            scale,
        );
        unsafe {
            title.setAlignmentMode(objc2_quartz_core::kCAAlignmentCenter);
        }
        title.setOpacity(0.65);
        let result = Self {
            id: data.id,
            layer: card,
            image,
            caption,
            title,
            icon,
        };
        result.set_frame(frame, scale);
        result
    }

    fn set_frame(&self, frame: CGRect, scale: f64) {
        self.layer.setFrame(frame);
        let image = pixel_aligned(preview_frame(frame.size), scale);
        self.image.setFrame(image);
        self.caption
            .setFrame(rect(image.origin.x, image.max().y, image.size.width, CAPTION));
        if let Some(icon) = &self.icon {
            icon.setFrame(rect((image.size.width - 20.0) / 2.0, 4.0, 20.0, 20.0));
        }
        self.title.setFrame(rect(0.0, 27.0, image.size.width, 20.0));
    }
}
struct WorkspaceView {
    index: usize,
    layer: Retained<CALayer>,
    heading: Retained<CATextLayer>,
    empty: Retained<CATextLayer>,
    backdrop: Retained<CALayer>,
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
    _surface: WindowSurface,
    _window: CgsWindow,
}

// Draw on the fitted image, leaving the caption and transparent card gutters unboxed.
fn set_preview_border(image: &CALayer, width: f64) {
    image.setBorderWidth(width);
    if width > 0.0 {
        image.setBorderColor(Some(&NSColor::controlAccentColor().CGColor()));
    } else {
        image.setBorderColor(None);
    }
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
        let Some(ws) = self.workspaces.iter().find(|ws| ws.index == workspace) else {
            return false;
        };
        let Some(w) = ws.windows.iter().find(|w| w.id == window) else {
            return false;
        };
        let scale =
            ((self.bounds.size.height - 2.0 * GAP) * 0.5).max(1.0) / self.info.frame.size.height;
        let base = (self.bounds.size.width - self.info.frame.size.width * scale) / 2.0;
        let frame = w.layout_frame.unwrap_or(w.info.frame);
        let x = base + (frame.origin.x - self.info.frame.origin.x) * scale;
        let right = x + frame.size.width * scale;
        let offset = self.offsets.entry(workspace).or_default();
        let previous = *offset;
        let (left_edge, right_edge) = if ws.layout_mode == "scrolling" {
            (base - 12.0, base + self.info.frame.size.width * scale + 12.0)
        } else {
            (12.0, self.bounds.size.width - 12.0)
        };
        if x - *offset < left_edge {
            *offset = x - left_edge;
        } else if right - *offset > right_edge {
            *offset = right - right_edge;
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
                view.layer.setFrame(pixel_aligned(ws.frame, self.scale));
                let mut heading = view.heading.frame();
                heading.origin.y =
                    pixel_aligned(rect(0.0, ws.frame.origin.y - 26.0, 1.0, 20.0), self.scale)
                        .origin
                        .y;
                view.heading.setFrame(heading);
            }
        });
        Some(visibility_changed)
    }

    fn reproject(&mut self, cache: Option<&PreviewCache>) {
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
                    && a.windows.iter().zip(&b.windows).all(|(a, b)| a.id == b.id)
            });
        if !same_cards {
            self.rebuild(cache);
            return;
        }
        with_disabled_actions(|| {
            for ((view, projected), previous) in self.views.iter().zip(&next).zip(&self.projection)
            {
                view.layer.setFrame(pixel_aligned(projected.frame, self.scale));
                view.heading.setFrame(workspace_heading(projected, self.scale));
                view.backdrop.setFrame(pixel_aligned(workspace_backdrop(projected), self.scale));
                for ((card, window), old) in
                    view.cards.iter().zip(&projected.windows).zip(&previous.windows)
                {
                    if window.frame != old.frame {
                        card.set_frame(window.frame, self.scale);
                    }
                }
            }
        });
        self.projection = next;
    }

    fn rebuild(&mut self, cache: Option<&PreviewCache>) {
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
                        && view
                            .cards
                            .iter()
                            .zip(&ws.windows)
                            .all(|(card, w)| card.id == w.id && card.layer.frame() == w.frame)
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
                    view.layer.setFrame(pixel_aligned(ws.frame, self.scale));
                    view.heading.setFrame(workspace_heading(ws, self.scale));
                    view.backdrop.setFrame(pixel_aligned(workspace_backdrop(ws), self.scale));
                    update_heading(&view.heading, data);
                    self.views.push(existing.unwrap());
                    continue;
                }
                let (container, heading, empty, backdrop) = if let Some(view) = existing {
                    (view.layer, view.heading, view.empty, view.backdrop)
                } else {
                    let container = layer(ws.frame, self.scale);
                    let backdrop =
                        layer(pixel_aligned(workspace_backdrop(ws), self.scale), self.scale);
                    backdrop.setCornerRadius(16.0);
                    backdrop.setBackgroundColor(Some(&color(1.0, 1.0, 1.0, 0.035)));
                    container.addSublayer(&backdrop);
                    let heading = label(
                        &self.root,
                        &data.name,
                        workspace_heading(ws, self.scale),
                        self.scale,
                    );
                    unsafe {
                        heading.setAlignmentMode(objc2_quartz_core::kCAAlignmentLeft);
                    }
                    let font = unsafe {
                        NSFont::systemFontOfSize_weight(13.0, objc2_app_kit::NSFontWeightMedium)
                    };
                    unsafe {
                        heading.setFont(Some(&*(Retained::as_ptr(&font) as *const CFType)));
                    }
                    heading.setFontSize(13.0);
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
                    (container, heading, empty, backdrop)
                };
                container.setFrame(pixel_aligned(ws.frame, self.scale));
                container.setMasksToBounds(true);
                empty.setHidden(!data.windows.is_empty());
                backdrop.setFrame(pixel_aligned(workspace_backdrop(ws), self.scale));
                update_heading(&heading, data);
                heading.setFrame(workspace_heading(ws, self.scale));
                self.root.addSublayer(&container);
                self.root.addSublayer(&heading);
                let mut cards = Vec::new();
                for w in &ws.windows {
                    let data = &data.windows[w.source];
                    if let Some((card, previous)) = old_cards.remove(&data.id) {
                        card.set_frame(w.frame, self.scale);
                        set_image(&card.image, cache.and_then(|c| c.get(data)).map(|i| &**i));
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
                    let image = cache.and_then(|c| c.get(data));
                    let card = unsafe {
                        WindowCard::new(
                            data,
                            w.frame,
                            self.scale,
                            image.map(|i| &*(i.as_ref() as *const CGImage as *const AnyObject)),
                            icon.as_ref()
                                .map(|i| &*(i.as_ref() as *const CGImage as *const AnyObject)),
                        )
                    };
                    container.addSublayer(&card.layer);
                    cards.push(card);
                }
                self.views.push(WorkspaceView {
                    index: data.index,
                    layer: container,
                    heading,
                    empty,
                    backdrop,
                    cards,
                });
            }
        });
        for view in old_views {
            view.layer.removeFromSuperlayer();
            view.heading.removeFromSuperlayer();
        }
        self.animate = false;
    }
}

struct OverviewDrag {
    window: WindowId,
    floating: bool,
    intent: Option<crate::actor::reactor::OverviewDrop>,
    start: CGPoint,
    point: CGPoint,
    size: CGSize,
    started: bool,
    card: Retained<CALayer>,
    preview: Retained<CALayer>,
    indicator: Retained<CALayer>,
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
    hovered: Option<WindowId>,
    pub(crate) previews: Option<PreviewSession>,
    drag: Option<OverviewDrag>,
    drop: Option<crate::actor::reactor::OverviewDrop>,
    pressed: Option<CGPoint>,
}

fn release_layer_tree(root: &CALayer) {
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

impl Drop for OverviewSession {
    fn drop(&mut self) {
        let _transaction = OverviewTransaction::begin();
        // Close the receiver first so concurrent capture completions are discarded.
        self.previews.take();
        if let Some(drag) = self.end_drag() {
            release_layer_tree(&drag.card);
        }
        for display in &self.displays {
            release_layer_tree(&display.root);
        }
        // Detach CAContexts and release CGS windows before flushing this teardown.
        self.displays.clear();
    }
}

impl OverviewSession {
    pub(crate) fn new(
        reactor: &ReactorHandle,
        mtm: MainThreadMarker,
        settings: &MissionControlSettings,
        generation: u64,
        cache: &mut Option<PreviewCache>,
    ) -> Option<Self> {
        let _transaction = OverviewTransaction::begin();
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
        if let Some(cache) = cache {
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
            root.setBackgroundColor(Some(&color(0.035, 0.035, 0.045, 0.32)));
            let result = (|| {
                let window = CgsWindow::new_compositor(bounds, 0.0)?;
                window.set_resolution(scale)?;
                window.set_level(NSPopUpMenuWindowLevel as i32)?;
                window.set_blur(24, None)?;
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
                _surface: surface,
                _window: window,
            };
            view.rebuild(cache.as_ref());
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
            hovered: None,
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

    pub(crate) fn edge_tick(&mut self, cache: Option<&PreviewCache>) {
        let _transaction = OverviewTransaction::begin();
        let Some(point) = self.drag.as_ref().filter(|drag| drag.started).map(|drag| drag.point)
        else {
            return;
        };
        if let Some(d) = self.displays.iter_mut().find(|d| contains(d.bounds, point)) {
            let direction = edge_direction(d.bounds.size.height, point.y - d.bounds.origin.y);
            d.workspace_offset = (d.centered as f64 + d.workspace_offset + direction * 0.2)
                .clamp(0.0, d.workspaces.len().saturating_sub(1) as f64)
                - d.centered as f64;
            d.reproject(cache);
        }
        self.move_drag(point);
        self.request_previews(cache);
    }

    pub(crate) fn refresh(
        &mut self,
        reactor: &ReactorHandle,
        cache: Option<&PreviewCache>,
        moved: Option<WindowId>,
    ) {
        let _transaction = OverviewTransaction::begin();
        self.end_drag();
        let selected = moved.or(self.selection.window);
        for (i, d) in self.displays.iter_mut().enumerate() {
            let centered_id = d.workspaces.get(d.centered).map(|ws| ws.workspace_id);
            d.workspaces = reactor.query_workspaces(d.info.space);
            filter_workspaces(&mut d.workspaces, self.show_empty_workspaces);

            if d.workspaces.is_empty() {
                continue;
            }
            d.centered = d
                .workspaces
                .iter()
                .position(|ws| Some(&ws.workspace_id) == centered_id.as_ref())
                .unwrap_or_else(|| d.centered.min(d.workspaces.len() - 1));
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
            d.rebuild(cache);
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
        self.sync_images(cache);
        self.request_previews(cache);
    }

    fn end_drag(&mut self) -> Option<OverviewDrag> {
        let drag = self.drag.take()?;
        drag.card.removeFromSuperlayer();
        drag.indicator.removeFromSuperlayer();
        for card in self.displays.iter().flat_map(|d| &d.views).flat_map(|ws| &ws.cards) {
            if card.id == drag.window {
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
        let ghost = unsafe {
            WindowCard::new(
                data,
                rect(
                    0.0,
                    0.0,
                    source.layer.frame().size.width,
                    source.layer.frame().size.height,
                ),
                d.scale,
                source.image.contents().as_deref(),
                source.icon.as_ref().and_then(|icon| icon.contents()).as_deref(),
            )
        };
        set_preview_border(&ghost.image, 2.5);
        let indicator = layer(CGRect::ZERO, d.scale);
        indicator.setBackgroundColor(Some(&color(0.35, 0.65, 1.0, 0.8)));
        indicator.setCornerRadius(2.0);
        self.drag = Some(OverviewDrag {
            window: data.id,
            floating: data.is_floating,
            intent: None,
            start: point,
            point,
            size: source.layer.frame().size,
            started: false,
            card: ghost.layer,
            preview: ghost.image,
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
        drag.intent = None;
        with_disabled_actions(|| {
            for card in self.displays.iter().flat_map(|d| &d.views).flat_map(|ws| &ws.cards) {
                if card.id == drag.window {
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
            let Some(ws) = d.projection.iter().find(|ws| contains(workspace_hit_frame(ws), local))
            else {
                return;
            };
            let data = &d.workspaces[ws.source];
            let mut intent = crate::actor::reactor::OverviewDrop {
                window: drag.window,
                workspace: data.workspace_id,
                target: None,
                frame: None,
            };
            let p = CGPoint::new(local.x, local.y - ws.frame.origin.y);
            let backdrop = workspace_backdrop(ws);
            let mut indicator = rect(
                backdrop.origin.x + 16.0,
                ws.frame.origin.y + ws.frame.size.height - 4.0,
                (backdrop.size.width - 32.0).max(1.0),
                2.0,
            );
            if drag.floating {
                intent.frame = Some(floating_drop_frame(
                    d.info.frame,
                    ws.frame,
                    drag.size,
                    p,
                    d.offsets.get(&data.index).copied().unwrap_or(0.0),
                ));
                let ghost = drag.card.frame();
                let image = drag.preview.frame();
                indicator = rect(
                    ghost.origin.x + image.origin.x,
                    ghost.origin.y + image.origin.y + image.size.height + 2.0,
                    image.size.width,
                    2.0,
                );
            } else if let Some(target) =
                crate::actor::drag::hit_test_projected(&ws.scene, drag.window, p)
            {
                let action = target.action;
                intent.target = Some((target.window, action));
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
            drag.intent = Some(intent);
            drag.indicator.setFrame(indicator);
            drag.indicator.setHidden(false);
        });
    }

    pub(crate) fn start_previews(
        &mut self,
        generation: u64,
        enabled: bool,
        wake: crate::actor::mission_control::Sender,
        cache: Option<&PreviewCache>,
    ) {
        if generation != self.generation || !enabled {
            return;
        }
        let _transaction = OverviewTransaction::begin();
        self.previews = PreviewSession::open(enabled, generation, wake);
        self.request_previews(cache);
    }

    fn pointer_target(&self, point: CGPoint) -> Option<(usize, Selection)> {
        self.displays
            .iter()
            .enumerate()
            .find(|(_, d)| contains(d.bounds, point))
            .and_then(|(i, d)| {
                let local = CGPoint::new(point.x - d.bounds.origin.x, point.y - d.bounds.origin.y);
                hit(&d.workspaces, &d.projection, local).map(|selection| (i, selection))
            })
    }

    fn update_hover(&mut self, point: CGPoint) -> bool {
        let hovered = self.pointer_target(point).and_then(|(_, selection)| selection.window);
        if hovered != self.hovered {
            self.hovered = hovered;
            self.update_preview_borders();
            return true;
        }
        false
    }

    fn update_preview_borders(&self) {
        for card in self.displays.iter().flat_map(|d| &d.views).flat_map(|ws| &ws.cards) {
            set_preview_border(
                &card.image,
                if Some(card.id) == self.hovered {
                    1.5
                } else {
                    0.0
                },
            );
        }
    }

    fn highlight(&self, previous: Option<(usize, Selection)>) {
        with_disabled_actions(|| {
            self.update_preview_borders();
            for (display, selection, width) in previous
                .into_iter()
                .map(|(d, s)| (d, s, 0.5))
                .chain(std::iter::once((self.active, self.selection, 2.0)))
            {
                if let Some(ws) =
                    self.displays[display].views.iter().find(|w| w.index == selection.workspace)
                {
                    ws.backdrop.setBackgroundColor(Some(&color(
                        1.0,
                        1.0,
                        1.0,
                        if width > 0.5 { 0.065 } else { 0.035 },
                    )));
                    ws.heading.setForegroundColor(Some(&color(
                        1.0,
                        1.0,
                        1.0,
                        if width > 0.5 { 0.95 } else { 0.55 },
                    )));
                    if let Some(id) = selection.window
                        && let Some(card) = ws.cards.iter().find(|c| c.id == id)
                    {
                        let selected = width > 0.5;
                        card.title.setOpacity(if selected { 1.0 } else { 0.65 });
                        if let Some(icon) = &card.icon {
                            icon.setOpacity(if selected { 1.0 } else { 0.82 });
                        }
                    }
                }
            }
        });
    }

    pub(crate) fn input(
        &mut self,
        input: Input,
        cache: Option<&PreviewCache>,
    ) -> Option<OverviewAction> {
        // Most sampled motion stays in the same card: no layer writes or compositor flush.
        if let Input::Move(point) = &input {
            if self.drag.is_some() {
                return None;
            }
            let target = self.pointer_target(*point);
            let hovered = target.and_then(|(_, selection)| selection.window);
            if hovered == self.hovered
                && target.is_none_or(|target| target == (self.active, self.selection))
            {
                return None;
            }
        }
        if let Input::PointerDrag(point) = &input
            && let Some(drag) = &mut self.drag
            && !drag.started
            && (point.x - drag.start.x).hypot(point.y - drag.start.y) < 5.0
        {
            drag.point = *point;
            return None;
        }
        let _transaction = OverviewTransaction::begin();
        let pointer = match &input {
            Input::Move(point)
            | Input::Click(point)
            | Input::PointerDown(point)
            | Input::PointerUp(point)
            | Input::Scroll { point, .. } => Some(*point),
            _ => None,
        };
        if self.drag.is_none()
            && let Some(point) = pointer
        {
            self.update_hover(point);
        }
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
                self.update_hover(point);
                let pressed = self.pressed.take();
                if let Some(drag) = drag.filter(|drag| drag.started) {
                    self.drop = drag.intent;
                    return None;
                }
                if pressed.is_some() {
                    return self.input(Input::Click(point), cache);
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
                let target = self.pointer_target(point);
                let Some((i, selection)) = target else {
                    return activate.then_some(OverviewAction::Dismiss);
                };
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
                        d.reproject(cache);
                    }
                    if refresh_previews {
                        self.request_previews(cache);
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
                d.reproject(cache);
                reprojected = true;
                self.request_previews(cache);
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
                        d.reproject(cache);
                        reprojected = true;
                        self.request_previews(cache);
                    }
                }
            }
            _ => {}
        }
        if self.drag.is_none()
            && let Some(point) = pointer
        {
            self.update_hover(point);
        }
        if old != (self.active, self.selection) || (reprojected && !only_translated) {
            self.highlight(Some(old));
        }
        if activate {
            let d = &self.displays[self.active];
            let sys_id = self
                .selection
                .window
                .and_then(|id| d.workspaces.iter().flat_map(|w| &w.windows).find(|w| w.id == id))
                .and_then(|w| w.info.sys_id);
            let workspace = d.workspaces.iter().find(|ws| ws.index == self.selection.workspace)?;
            return Some(OverviewAction::Activate {
                display: d.info.display_uuid.clone(),
                workspace: workspace.workspace_id,
                selection: self.selection,
                sys_id,
            });
        }
        None
    }

    fn request_previews(&mut self, cache: Option<&PreviewCache>) {
        let Some(previews) = &mut self.previews else {
            return;
        };
        if previews.failed {
            return;
        }
        let mut visible = HashSet::default();
        if let Some(id) = self.selection.window {
            visible.insert(id);
        }
        // The lifted layer shares this image; account for its pixels inside the session cap.
        if let Some(drag) = &self.drag {
            visible.insert(drag.window);
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
                        if visible.len() >= 32 && !visible.contains(&data.id) {
                            continue;
                        }
                        visible.insert(data.id);
                        if previews.schedule.needs(data.id) {
                            let (width, height) =
                                capture_size(preview_frame(w.frame.size).size, display.scale);
                            previews.schedule.enqueue(PreviewRequest {
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
        previews
            .schedule
            .attempted
            .retain(|id| visible.contains(id) || previews.schedule.active.contains(id));
        previews.visible = visible.clone();
        // Requests already target card size; cap each against the shared pixel budget.
        let per_image_bytes = PREVIEW_BYTES / visible.len().max(1);
        let pixel_budget = per_image_bytes / 4;
        for request in &mut previews.schedule.pending {
            let factor = (pixel_budget as f64 / (request.width * request.height).max(1) as f64)
                .sqrt()
                .min(1.0);
            request.width = (request.width as f64 * factor).floor().max(1.0) as usize;
            request.height = (request.height as f64 * factor).floor().max(1.0) as usize;
        }
        previews.schedule.pending.retain(|p| visible.contains(&p.id));
        previews
            .schedule
            .pending
            .make_contiguous()
            .sort_by_key(|p| self.selection.window != Some(p.id));
        previews.pump(self.generation);
        self.sync_images(cache);
    }

    pub(crate) fn preview_ready(
        &mut self,
        result: PreviewEvent,
        current: Option<&crate::model::server::RuntimeWindowData>,
        cache: &mut Option<PreviewCache>,
    ) {
        let _transaction = OverviewTransaction::begin();
        let Some(previews) = &mut self.previews else {
            return;
        };
        match result {
            PreviewEvent::Content(generation, content) if generation == self.generation => {
                if let Some(content) = content {
                    let managed: HashSet<_> = self
                        .displays
                        .iter()
                        .flat_map(|d| &d.workspaces)
                        .flat_map(|ws| &ws.windows)
                        .filter_map(|w| w.info.sys_id.map(|id| id.as_u32()))
                        .collect();
                    previews.windows = Some(unsafe {
                        content
                            .windows()
                            .iter()
                            .filter(|w| managed.contains(&w.windowID()))
                            .map(|w| (w.windowID(), w))
                            .collect()
                    });
                } else {
                    previews.schedule.pending.clear();
                    previews.failed = true;
                }
            }
            PreviewEvent::Image(generation, request, image) if generation == self.generation => {
                previews.schedule.complete(request.id);
                if preview_is_current(generation, self.generation, &request, current)
                    && let Some(image) = image
                {
                    if previews.visible.contains(&request.id) {
                        let budget = PREVIEW_BYTES / previews.visible.len().max(1);
                        if let Some(owned) = owned_preview(&image, budget) {
                            let cache = cache.get_or_insert_with(PreviewCache::default);
                            cache.prepare_visible(&previews.visible);
                            cache.insert(&request, owned);
                        }
                    } else {
                        previews.schedule.attempted.remove(&request.id);
                    }
                }
            }
            _ => return,
        }
        previews.pump(self.generation);
        self.sync_images(cache.as_ref());
    }

    fn sync_images(&self, cache: Option<&PreviewCache>) {
        with_disabled_actions(|| {
            if let Some(drag) = &self.drag {
                let data = self
                    .displays
                    .iter()
                    .flat_map(|d| &d.workspaces)
                    .flat_map(|ws| &ws.windows)
                    .find(|w| w.id == drag.window);
                let image = data.and_then(|data| cache.and_then(|c| c.get(data)));
                set_image(&drag.preview, image.map(|i| &**i));
            }
            for display in &self.displays {
                for (view, ws) in display.views.iter().zip(&display.projection) {
                    for (card, w) in view.cards.iter().zip(&ws.windows) {
                        let data = &display.workspaces[ws.source].windows[w.source];
                        let image = cache.and_then(|c| c.get(data));
                        set_image(&card.image, image.map(|i| &**i));
                    }
                }
            }
        });
    }
}

#[derive(Debug, Clone)]
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

/// Framework results are retained before transfer and only inspected on main.
/// Keep this explicit unsafe handoff local: the native types do not implement Send.
fn deliver(tx: actor::Sender<PreviewEvent>, result: PreviewEvent) {
    if tx.is_closed() {
        return;
    }
    extern "C" fn send(ctx: *mut std::ffi::c_void) {
        let (tx, result) =
            *unsafe { Box::from_raw(ctx.cast::<(actor::Sender<PreviewEvent>, PreviewEvent)>()) };
        tx.send(result);
    }
    let ctx = Box::into_raw(Box::new((tx, result))).cast();
    unsafe { dispatchr::queue::main().after_f(dispatchr::time::Time::NOW, ctx, send) };
}

// Framework captures may finish after a session closes. Bound jobs across all sessions.
static CAPTURE_JOBS: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn outstanding_preview_captures() -> usize { CAPTURE_JOBS.load(Ordering::Acquire) }

fn acquire_capture_slot(jobs: &AtomicUsize) -> bool {
    jobs.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| (n < 2).then_some(n + 1))
        .is_ok()
}

pub(crate) struct PreviewSession {
    wake: crate::actor::mission_control::Sender,
    pub(crate) rx: actor::Receiver<PreviewEvent>,
    tx: actor::Sender<PreviewEvent>,
    windows: Option<HashMap<u32, Retained<SCWindow>>>,
    schedule: PreviewSchedule,
    failed: bool,
    visible: HashSet<WindowId>,
}

impl PreviewSession {
    fn open(
        enabled: bool,
        generation: u64,
        wake: crate::actor::mission_control::Sender,
    ) -> Option<Self> {
        if !enabled {
            return None;
        }
        if !CGPreflightScreenCaptureAccess() {
            tracing::info!(
                "Overview previews unavailable: allow Rift in System Settings > Privacy & Security > Screen Recording"
            );
            return None;
        }
        Some(Self::new(generation, wake))
    }

    fn new(generation: u64, wake: crate::actor::mission_control::Sender) -> Self {
        let (tx, rx) = actor::channel();
        let callback_tx = tx.clone();
        let callback = RcBlock::new(move |content: *mut SCShareableContent, _: *mut NSError| {
            if callback_tx.is_closed() {
                return;
            }
            let content = unsafe { Retained::retain(content) };
            deliver(callback_tx.clone(), PreviewEvent::Content(generation, content));
        });
        unsafe {
            SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(true,false,&callback);
        }
        Self {
            wake,
            rx,
            tx,
            windows: None,
            schedule: PreviewSchedule::default(),
            failed: false,
            visible: HashSet::default(),
        }
    }

    pub(crate) fn pump(&mut self, generation: u64) {
        if self.failed {
            return;
        }
        let Some(windows) = &self.windows else {
            return;
        };
        while self.schedule.active.len() < 2 && !self.schedule.pending.is_empty() {
            if !acquire_capture_slot(&CAPTURE_JOBS) {
                break;
            }
            let request = self.schedule.next().unwrap();
            let Some(window) = windows.get(&request.sys_id.as_u32()) else {
                CAPTURE_JOBS.fetch_sub(1, Ordering::AcqRel);
                self.schedule.complete(request.id);
                continue;
            };
            let owner = unsafe { window.owningApplication() };
            if !owner.is_some_and(|app| unsafe {
                owner_matches(&request, app.processID(), &app.bundleIdentifier().to_string())
            }) {
                CAPTURE_JOBS.fetch_sub(1, Ordering::AcqRel);
                self.schedule.complete(request.id);
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
                // Match the requested tile bounds without baking letterboxing into the image.
                config.setPreservesAspectRatio(false);
            }
            let tx = self.tx.clone();
            let wake = self.wake.clone();
            let callback = RcBlock::new(move |image: *mut CGImage, _: *mut NSError| {
                CAPTURE_JOBS.fetch_sub(1, Ordering::AcqRel);
                let result = if tx.is_closed() {
                    None
                } else {
                    let image =
                        NonNull::new(image).map(|image| unsafe { CFRetained::retain(image) });
                    Some(PreviewEvent::Image(generation, request.clone(), image))
                };
                // One main-thread handoff for both completion and scheduling the next capture.
                extern "C" fn complete(ctx: *mut std::ffi::c_void) {
                    let (tx, wake, result) = *unsafe {
                        Box::from_raw(ctx.cast::<(
                            actor::Sender<PreviewEvent>,
                            crate::actor::mission_control::Sender,
                            Option<PreviewEvent>,
                        )>())
                    };
                    wake.send(crate::actor::mission_control::Event::PumpPreviews);
                    if let Some(result) = result {
                        tx.send(result);
                    }
                }
                // The retained image and local preview channel are consumed on main.
                let ctx = Box::into_raw(Box::new((tx.clone(), wake.clone(), result))).cast();
                unsafe {
                    dispatchr::queue::main().after_f(dispatchr::time::Time::NOW, ctx, complete)
                };
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
    pending: VecDeque<PreviewRequest>,
    active: HashSet<WindowId>,
}
impl PreviewSchedule {
    fn needs(&self, id: WindowId) -> bool { !self.attempted.contains(&id) }

    fn enqueue(&mut self, request: PreviewRequest) {
        if self.attempted.insert(request.id) {
            self.pending.push_back(request);
        }
    }

    fn next(&mut self) -> Option<PreviewRequest> {
        if self.active.len() >= 2 || self.pending.is_empty() {
            return None;
        }
        let request = self.pending.pop_front()?;
        self.active.insert(request.id);
        self.attempted.insert(request.id);
        Some(request)
    }

    fn complete(&mut self, id: WindowId) { self.active.remove(&id); }
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

const PREVIEW_BYTES: usize = 8 * 1024 * 1024;
const CLOSED_PREVIEW_BYTES: usize = 8 * 1024 * 1024;

fn image_bytes(image: &CGImage) -> usize {
    CGImage::bytes_per_row(Some(image)).saturating_mul(CGImage::height(Some(image)))
}
fn capture_size(size: CGSize, backing: f64) -> (usize, usize) {
    let width = (size.width * backing).max(1.0);
    let height = (size.height * backing).max(1.0);
    let scale = (1024.0 / width).min(768.0 / height).min(1.0);
    ((width * scale).ceil() as usize, (height * scale).ceil() as usize)
}
fn owned_preview(image: &CGImage, budget: usize) -> Option<CFRetained<CGImage>> {
    if budget < 4 {
        return None;
    }
    let bytes = CGImage::width(Some(image))
        .saturating_mul(CGImage::height(Some(image)))
        .saturating_mul(4);
    if bytes == 0 {
        return None;
    }
    // Even at 1:1, make independent pixels so layers cannot pin an SCK IOSurface.
    resized_preview(image, (budget as f64 / bytes as f64).sqrt().min(1.0))
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
struct PreviewEntry {
    id: WindowId,
    sys_id: WindowServerId,
    bundle: Option<String>,
    image: CFRetained<CGImage>,
}
#[derive(Default)]
pub(crate) struct PreviewCache {
    entries: VecDeque<PreviewEntry>,
    bytes: usize,
}
impl PreviewCache {
    fn bytes(&self) -> usize { self.bytes }

    pub(crate) fn close(&mut self) { self.trim(CLOSED_PREVIEW_BYTES); }

    fn trim(&mut self, budget: usize) {
        while self.bytes > budget || self.entries.len() > 32 {
            if let Some(entry) = self.entries.pop_front() {
                self.bytes -= image_bytes(&entry.image);
            }
        }
    }

    pub(crate) fn stats(&self) -> (usize, usize) { (self.entries.len(), self.bytes()) }

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
            let keep = windows.clone().any(|w| {
                w.id == e.id && w.info.sys_id == Some(e.sys_id) && w.info.bundle_id == e.bundle
            });
            if !keep {
                self.bytes -= image_bytes(&e.image);
            }
            keep
        });
    }

    // A cached image from a sparse workspace can exceed a dense workspace's share.
    // Rebalance that same cache before insertion, then evict offscreen entries first.
    fn prepare_visible(&mut self, visible: &HashSet<WindowId>) {
        let budget = PREVIEW_BYTES / visible.len().max(1);
        for entry in &mut self.entries {
            let old = image_bytes(&entry.image);
            if visible.contains(&entry.id)
                && old > budget
                && let Some(image) = owned_preview(&entry.image, budget)
            {
                self.bytes = self.bytes - old + image_bytes(&image);
                entry.image = image;
            }
        }
        self.entries.make_contiguous().sort_by_key(|entry| visible.contains(&entry.id));
    }

    fn insert(&mut self, request: &PreviewRequest, image: CFRetained<CGImage>) {
        self.entries.retain(|e| {
            if e.id == request.id {
                self.bytes -= image_bytes(&e.image);
                false
            } else {
                true
            }
        });
        let cost = image_bytes(&image);
        if cost > PREVIEW_BYTES {
            return;
        }
        self.bytes += cost;
        self.entries.push_back(PreviewEntry {
            id: request.id,
            sys_id: request.sys_id,
            bundle: request.bundle.clone(),
            image,
        });
        self.trim(PREVIEW_BYTES);
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
            workspace_id: slotmap::KeyData::from_ffi((1u64 << 32) | (index as u64 + 1)).into(),
            space: crate::sys::screen::SpaceId::new(1),
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
    fn full_preview_copy_releases_capture_backing_and_enforces_pixel_budget() {
        let source = bitmap(1024, 768);
        let source_ref: &CGImage = &source;
        let source_cf: &CFType = source_ref.as_ref();
        let baseline = source_cf.retain_count();
        let owned = owned_preview(&source, 1024 * 768 * 4).unwrap();
        assert_eq!(CGImage::width(Some(&owned)), 1024);
        assert_eq!(CGImage::height(Some(&owned)), 768);
        assert_eq!(source_cf.retain_count(), baseline);
        let bounded = owned_preview(&source, 512 * 384 * 4).unwrap();
        assert!(image_bytes(&bounded) <= 512 * 384 * 4);
        assert_eq!(source_cf.retain_count(), baseline);
        assert!(owned_preview(&source, 0).is_none());
    }

    #[test]
    fn cache_does_not_retain_capture_storage() {
        let source = bitmap(1024, 768);
        let source_ref: &CGImage = &source;
        let source_cf: &objc2_core_foundation::CFType = source_ref.as_ref();
        let count = source_cf.retain_count();
        let image = owned_preview(&source, 320 * 240 * 4).unwrap();
        assert_eq!(CGImage::width(Some(&image)), 320);
        assert_eq!(CGImage::height(Some(&image)), 240);
        assert_eq!(image_bytes(&image), 320 * 240 * 4);
        assert_eq!(source_cf.retain_count(), count);
        let mut cache = PreviewCache::default();
        cache.insert(&request(1), image);
        drop(source);
        assert_eq!(cache.bytes(), 320 * 240 * 4);
        assert!(cache.get(&window(1, rect(0.0, 0.0, 100.0, 100.0), false)).is_some());
    }

    #[test]
    fn visible_previews_survive_new_captures_and_density_changes() {
        let mut cache = PreviewCache::default();
        cache.insert(&request(1), bitmap(1024, 768));
        cache.insert(&request(2), bitmap(1024, 768));
        let visible: HashSet<_> = (1..=12).map(|id| request(id).id).collect();
        for id in 3..=12 {
            cache.prepare_visible(&visible);
            cache.insert(&request(id), bitmap(256, 256));
            for previous in 1..=id {
                assert!(cache.get(&window(previous, CGRect::ZERO, false)).is_some());
            }
            assert!(cache.bytes() <= PREVIEW_BYTES);
        }
        // Offscreen images, rather than these displayed images, are evicted first.
        cache.insert(&request(99), bitmap(1024, 768));
        cache.prepare_visible(&visible);
        cache.insert(&request(12), bitmap(256, 256));
        for id in 1..=12 {
            assert!(cache.get(&window(id, CGRect::ZERO, false)).is_some());
        }
    }

    #[test]
    fn replacing_and_closing_cache_releases_pixels_and_keeps_accounting_exact() {
        let mut cache = PreviewCache::default();
        cache.insert(&request(1), bitmap(1024, 768));
        cache.insert(&request(1), bitmap(100, 100));
        assert_eq!(cache.stats(), (1, 40_000));
        cache.insert(&request(2), bitmap(1024, 768));
        cache.insert(&request(3), bitmap(1024, 768));
        cache.close();
        assert!(cache.bytes() <= CLOSED_PREVIEW_BYTES);
        assert_eq!(
            cache.bytes(),
            cache.entries.iter().map(|e| image_bytes(&e.image)).sum::<usize>()
        );
        cache.prune(std::iter::empty());
        assert_eq!(cache.stats(), (0, 0));
    }

    #[test]
    fn projected_drag_scene_uses_logical_geometry_and_excludes_floating_windows() {
        let mut tiled = window(1, rect(1200.0, 0.0, 200.0, 400.0), false);
        tiled.layout_frame = Some(rect(200.0, 0.0, 200.0, 400.0));
        let data = workspace(0, vec![tiled, window(2, rect(200.0, 0.0, 200.0, 400.0), true)]);
        let projection = project(
            rect(0.0, 0.0, 1000.0, 800.0),
            CGSize::new(1000.0, 800.0),
            &[data],
            0,
            0.0,
            &HashMap::default(),
        );
        let ws = &projection[0];
        assert_eq!(ws.scene.targets.len(), 1);
        assert_eq!(ws.scene.targets[0].frame, ws.windows[0].frame);
        let center = ws.scene.targets[0].frame;
        let center = CGPoint::new(
            center.origin.x + center.size.width / 2.0,
            center.origin.y + center.size.height / 2.0,
        );
        let target =
            crate::actor::drag::hit_test_projected(&ws.scene, WindowId::new(123, 99), center)
                .unwrap();
        assert_eq!(target.window, WindowId::new(123, 1));
        assert!(crate::actor::drag::hit_test_projected(&ws.scene, target.window, center).is_none());
    }

    #[test]
    fn cache_enforces_byte_and_entry_caps_and_prunes_identity() {
        let mut cache = PreviewCache::default();
        for id in 1..65 {
            cache.insert(&request(id), bitmap(320, 240));
            assert!(cache.bytes() <= PREVIEW_BYTES);
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
    fn strip_pans_columns_that_fit_overlay_but_extend_past_desktop() {
        let display = rect(0.0, 0.0, 1200.0, 800.0);
        let ws = workspace(0, vec![
            window(1, rect(0.0, 0.0, 600.0, 800.0), false),
            window(2, rect(600.0, 0.0, 600.0, 800.0), false),
            window(3, rect(1200.0, 0.0, 600.0, 800.0), false),
        ]);
        let p = project(
            display,
            display.size,
            std::slice::from_ref(&ws),
            0,
            0.0,
            &HashMap::default(),
        );
        assert_eq!(p[0].windows.len(), 3); // All three fit in the physical overlay.
        let next = scrolled_horizontal_offset(display, display.size, &ws, 0.0, -80.0);
        assert_eq!(next, 80.0);
        let end = scrolled_horizontal_offset(display, display.size, &ws, next, -10000.0);
        assert_eq!(end, 258.0);
        assert_eq!(
            scrolled_horizontal_offset(display, display.size, &ws, end, 10000.0),
            0.0
        );
        let mut fitting = ws.clone();
        fitting.windows.pop();
        assert_eq!(
            scrolled_horizontal_offset(display, display.size, &fitting, 0.0, -80.0),
            0.0
        );
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
    fn workspace_surface_encloses_visible_cards_and_stays_inside_screen() {
        let ws = WorkspaceProjection {
            source: 0,
            frame: rect(0.0, 200.0, 1200.0, 360.0),
            scene: Default::default(),
            windows: vec![
                WindowProjection {
                    id: WindowId::new(123, 1),
                    floating: false,
                    source: 0,
                    frame: rect(150.0, 0.0, 300.0, 360.0),
                },
                WindowProjection {
                    id: WindowId::new(123, 2),
                    floating: false,
                    source: 1,
                    frame: rect(600.0, 0.0, 400.0, 360.0),
                },
            ],
        };
        let bounds = workspace_backdrop(&ws);
        assert!(bounds.origin.x <= 150.0);
        assert!(bounds.origin.x + bounds.size.width >= 1000.0);
        assert!(bounds.origin.x >= 24.0);
        assert!(bounds.origin.x + bounds.size.width <= 1176.0);
        assert_eq!(bounds.size.height, 360.0);
    }

    #[test]
    fn transparent_preview_gutters_select_workspace_instead_of_window() {
        let workspaces = vec![workspace(1, vec![window(
            1,
            rect(0.0, 0.0, 400.0, 200.0),
            false,
        )])];
        let projection = vec![WorkspaceProjection {
            source: 0,
            frame: rect(0.0, 100.0, 1200.0, 360.0),
            scene: Default::default(),
            windows: vec![WindowProjection {
                id: WindowId::new(123, 1),
                floating: false,
                source: 0,
                frame: rect(400.0, 0.0, 400.0, 200.0),
            }],
        }];
        assert_eq!(
            hit(&workspaces, &projection, CGPoint::new(402.0, 150.0)).unwrap().window,
            None
        );
        assert_eq!(
            hit(&workspaces, &projection, CGPoint::new(600.0, 150.0)).unwrap().window,
            Some(WindowId::new(123, 1))
        );
    }

    #[test]
    fn preview_geometry_preserves_aspect_ratio_and_caption_space() {
        for size in [
            CGSize::new(600.0, 300.0),
            CGSize::new(180.0, 500.0),
            CGSize::new(800.0, 100.0),
        ] {
            let image = preview_frame(size);
            assert!(
                (image.size.width / image.size.height - size.width / size.height).abs() < 1e-10
            );
            assert!(image.origin.x >= 6.0);
            assert!(image.origin.y + image.size.height + CAPTION <= size.height + 1e-10);
            assert!((image.origin.x * 2.0 + image.size.width - size.width).abs() < 1e-10);
        }
    }

    #[test]
    fn workspace_and_heading_share_backing_pixel_scroll_alignment() {
        let frame = rect(0.0, 123.27, 1200.0, 360.0);
        for scale in [1.0, 2.0] {
            let aligned = pixel_aligned(frame, scale);
            let heading = workspace_heading(
                &WorkspaceProjection {
                    source: 0,
                    frame,
                    scene: Default::default(),
                    windows: vec![],
                },
                scale,
            );
            assert_eq!(heading.origin.y + 26.0, aligned.origin.y);
            assert_eq!(aligned.origin.y * scale, (frame.origin.y * scale).round());
        }
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
            hit(&workspaces, &projection, CGPoint::new(460.0, 280.0)),
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
    fn teardown_releases_layer_images_while_old_layers_remain_retained() {
        objc2::rc::autoreleasepool(|_| {
            let image = bitmap(640, 480);
            let image_ref: &CGImage = &image;
            let cf: &CFType = image_ref.as_ref();
            let baseline = cf.retain_count();
            for _ in 0..40 {
                let root = layer(rect(0.0, 0.0, 1200.0, 800.0), 2.0);
                let child = layer(rect(0.0, 0.0, 600.0, 400.0), 2.0);
                set_image(&child, Some(&image));
                root.addSublayer(&child);
                let sibling = layer(rect(600.0, 0.0, 600.0, 400.0), 2.0);
                set_image(&sibling, Some(&image));
                root.addSublayer(&sibling);
                assert!(cf.retain_count() > baseline);
                let _transaction = OverviewTransaction::begin();
                release_layer_tree(&root);
                assert!(unsafe { child.contents() }.is_none());
                assert_eq!(cf.retain_count(), baseline);
            }
        });
    }

    #[test]
    fn late_preview_for_closed_session_is_dropped_before_main_queue_dispatch() {
        let image = bitmap(640, 480);
        let image_ref: &CGImage = &image;
        let cf: &CFType = image_ref.as_ref();
        let baseline = cf.retain_count();
        let (tx, rx) = actor::channel();
        drop(rx);
        assert!(tx.is_closed());
        deliver(tx, PreviewEvent::Image(1, request(1), Some(image.clone())));
        assert_eq!(cf.retain_count(), baseline);
    }

    #[test]
    fn dropping_preview_session_releases_queued_images() {
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
        let (wake, _wake_rx) = actor::channel();
        let session = PreviewSession {
            wake,
            rx,
            tx: tx.clone(),
            windows: None,
            schedule: PreviewSchedule::default(),
            failed: false,
            visible: HashSet::default(),
        };
        tx.send(PreviewEvent::Image(1, request(2), Some(image.clone())));
        assert!(image_cf.retain_count() >= count + 1);
        drop(session);
        assert_eq!(image_cf.retain_count(), count);
        assert!(tx.try_send(PreviewEvent::Content(1, None)).is_err());
    }

    #[test]
    fn disabled_previews_never_enter_permission_or_capture_path() {
        let (wake, _rx) = actor::channel();
        assert!(PreviewSession::open(false, 1, wake).is_none());
    }

    #[test]
    fn capture_limit_survives_closing_and_reopening_sessions() {
        let jobs = AtomicUsize::new(0);
        assert!(acquire_capture_slot(&jobs));
        assert!(acquire_capture_slot(&jobs));
        // Closing a receiver does not mean its native capture has completed.
        assert!(!acquire_capture_slot(&jobs));
        jobs.fetch_sub(1, Ordering::AcqRel);
        assert!(acquire_capture_slot(&jobs));
        assert!(!acquire_capture_slot(&jobs));
        jobs.fetch_sub(2, Ordering::AcqRel);
        assert_eq!(jobs.load(Ordering::Acquire), 0);
    }

    #[test]
    fn scheduling_limits_two_jobs_and_remembers_attempts_for_the_session() {
        let mut schedule = PreviewSchedule::default();
        schedule.pending.extend([request(1), request(2), request(3)]);
        assert_eq!(schedule.next().unwrap().id, WindowId::new(123, 1));
        schedule.next().unwrap();
        assert!(schedule.next().is_none());
        assert_eq!(schedule.active.len(), 2);
        schedule.complete(WindowId::new(123, 1));
        assert_eq!(schedule.next().unwrap().id, WindowId::new(123, 3));
        assert!(!schedule.needs(WindowId::new(123, 1)));
        assert!(schedule.needs(WindowId::new(123, 4)));
        // An unstarted card removed when hidden may be queued when newly exposed.
        assert!(!schedule.attempted.contains(&WindowId::new(123, 4)));
        schedule.enqueue(request(4));
        schedule.complete(WindowId::new(123, 2));
        assert_eq!(schedule.next().unwrap().id, WindowId::new(123, 4));
    }

    #[test]
    fn discarded_or_evicted_capture_can_upgrade_on_return() {
        let mut schedule = PreviewSchedule::default();
        schedule.enqueue(request(1));
        let id = schedule.next().unwrap().id;
        assert!(!schedule.needs(id)); // No duplicate while in flight.
        schedule.complete(WindowId::new(123, 1));
        schedule.attempted.remove(&id); // Successful image discarded offscreen.
        assert!(schedule.needs(id));
        schedule.enqueue(request(1));
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
