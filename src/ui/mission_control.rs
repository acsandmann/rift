//! Session-owned Overview. Runtime frames are truth; captured images only decorate cards.
use std::collections::VecDeque;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicUsize, Ordering};

use block2::RcBlock;
use cgs::{CgsWindow, LayerTransaction, WindowSurface, release_layer_tree, with_disabled_actions};
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
use objc2_quartz_core::{CABasicAnimation, CAFrameRateRange, CALayer, CAMediaTiming, CATextLayer};
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
use crate::sys::dispatch::DispatchExt;
use crate::sys::screen::{NSScreenExt, ScreenInfo};
use crate::sys::window_server::WindowServerId;

const GAP: f64 = 40.0;
const CORNER: f64 = 8.0;
const CAPTION: f64 = 48.0;

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
    metal: Option<cgs::PreviewCompositor>,
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
        // The already-presented Metal canvas moves in this same CA transaction
        // as the captions/ribbons. Capture and GPU availability cannot delay it.
        with_disabled_actions(|| {
            if let Some(metal) = &mut self.metal {
                if let Err(error) = metal.translate(CGPoint::new(
                    0.0,
                    self.projection
                        .first()
                        .map(|ws| {
                            pixel_aligned(rect(0., ws.frame.origin.y + delta, 1., 1.), self.scale)
                                .origin
                                .y
                                - pixel_aligned(ws.frame, self.scale).origin.y
                        })
                        .unwrap_or(delta),
                )) {
                    tracing::warn!(?error, "Overview preview translation failed");
                }
            }
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
        if let Some((old, new)) = self
            .projection
            .iter()
            .find_map(|old| next.iter().find(|new| new.source == old.source).map(|new| (old, new)))
        {
            let delta = pixel_aligned(new.frame, self.scale).origin.y
                - pixel_aligned(old.frame, self.scale).origin.y;
            if delta != 0.0 {
                if let Some(metal) = &mut self.metal {
                    let _ = metal.translate(CGPoint::new(0., delta));
                }
            }
        }
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

impl Drop for OverviewSession {
    fn drop(&mut self) {
        let _transaction = LayerTransaction::disabled().flush_on_commit();
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
        let _transaction = LayerTransaction::disabled().flush_on_commit();
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
            cache.metal.retain(|(info, _, _, _)| {
                snapshot.iter().any(|(d, _)| {
                    d.info.display_uuid == info.display_uuid
                        && d.info.id == info.id
                        && d.info.space == info.space
                })
            });
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
                Ok::<_, cgs::CgsWindowError>((window, surface))
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
                metal: None,
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
        // The actor starts capture immediately after installing this session.
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
        let _transaction = LayerTransaction::disabled().flush_on_commit();
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
        let _transaction = LayerTransaction::disabled().flush_on_commit();
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
        for display in &mut self.displays {
            if let Some(metal) = &mut display.metal {
                metal.refresh_sources();
            }
        }
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
        mut cache: Option<&mut PreviewCache>,
    ) {
        if generation != self.generation || !enabled {
            return;
        }
        let _transaction = LayerTransaction::disabled().flush_on_commit();
        if enabled {
            for display in &mut self.displays {
                if let Some(metal) = cache.as_deref_mut().and_then(|cache| {
                    cache.take_metal(&display.info, display.bounds, display.scale)
                }) {
                    display.metal = Some(metal);
                    continue;
                }
                let wake = wake.clone();
                match cgs::PreviewCompositor::new(display.bounds, display.scale, move || {
                    wake.send(crate::actor::mission_control::Event::PumpPreviews);
                }) {
                    Ok(metal) => {
                        if let Err(error) = metal.set_level(NSPopUpMenuWindowLevel as i32) {
                            tracing::warn!(?error, "Metal preview ordering unavailable");
                        } else {
                            display.metal = Some(metal);
                        }
                    }
                    Err(error) => {
                        tracing::warn!(?error, "Metal previews unavailable; using cached previews")
                    }
                }
            }
        }
        self.previews = PreviewSession::open(enabled, generation, wake);
        self.request_previews(cache.as_deref());
    }

    pub(crate) fn retain_previews(&mut self, cache: &mut Option<PreviewCache>) {
        // Keep the active display last so it survives a tight closed-cache budget.
        for n in 1..=self.displays.len() {
            let index = (self.active + n) % self.displays.len();
            let display = &mut self.displays[index];
            let Some(mut metal) = display.metal.take() else {
                continue;
            };
            if let Err(error) = metal.hide() {
                tracing::warn!(?error, "Metal preview retirement failed");
                continue;
            }
            let cache = cache.get_or_insert_with(PreviewCache::default);
            cache
                .metal
                .retain(|(info, _, _, _)| info.display_uuid != display.info.display_uuid);
            // Three BGRA drawables at physical display resolution. This is a
            // conservative reservation, separate from cgs's thumbnail cache.
            let bytes = |bounds: CGRect, scale: f64| -> usize {
                let row = ((bounds.size.width * scale).ceil() as usize * 4 + 255) & !255;
                row * (bounds.size.height * scale).ceil() as usize * 3
            };
            let required = bytes(display.bounds, display.scale);
            let budget = 256 * 1024 * 1024;
            if required > budget {
                continue;
            }
            let mut used: usize =
                cache.metal.iter().map(|(_, bounds, scale, _)| bytes(*bounds, *scale)).sum();
            while used + required > budget {
                let (_, bounds, scale, _) = cache.metal.remove(0);
                used -= bytes(bounds, scale);
            }
            cache.metal.push((display.info.clone(), display.bounds, display.scale, metal));
        }
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
        let _transaction = LayerTransaction::disabled().flush_on_commit();
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
                self.sync_metal();
                return None;
            }
            Input::PointerUp(point) => {
                self.move_drag(point);
                let drag = self.end_drag();
                self.sync_metal();
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
        self.sync_metal();
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

    fn sync_metal(&mut self) {
        for display in &mut self.displays {
            let Some(metal) = &mut display.metal else {
                continue;
            };
            let mut scene = Vec::new();
            let accent = NSColor::controlAccentColor()
                .colorUsingColorSpace(&objc2_app_kit::NSColorSpace::sRGBColorSpace());
            let border = accent
                .map(|c| {
                    [
                        c.redComponent() as f32,
                        c.greenComponent() as f32,
                        c.blueComponent() as f32,
                        1.,
                    ]
                })
                .unwrap_or([0.2, 0.5, 1., 1.]);
            let viewport = rect(0., 0., display.bounds.size.width, display.bounds.size.height);
            for ws in &display.projection {
                let ribbon = pixel_aligned(ws.frame, display.scale);
                for w in &ws.windows {
                    let data = &display.workspaces[ws.source].windows[w.source];
                    if !card_visible(display.bounds.size, ws.frame, w.frame)
                        || self
                            .drag
                            .as_ref()
                            .is_some_and(|drag| drag.started && drag.window == data.id)
                    {
                        continue;
                    }
                    let Some(sys_id) = data.info.sys_id else {
                        continue;
                    };
                    let card = pixel_aligned(w.frame, display.scale);
                    let mut frame = pixel_aligned(preview_frame(card.size), display.scale);
                    frame.origin.x += ribbon.origin.x + card.origin.x;
                    frame.origin.y += ribbon.origin.y + card.origin.y;
                    let mut p = cgs::Preview::window(sys_id.as_u32(), data.id.pid, frame);
                    p.corner_radius = CORNER as f32;
                    p.border_width = if self.hovered == Some(data.id) {
                        1.5
                    } else {
                        0.
                    };
                    p.border_color = border;
                    let left = ribbon.origin.x.max(viewport.origin.x);
                    let top = ribbon.origin.y.max(viewport.origin.y);
                    let right = ribbon.max().x.min(viewport.max().x);
                    let bottom = ribbon.max().y.min(viewport.max().y);
                    p.clip = Some(rect(left, top, (right - left).max(0.), (bottom - top).max(0.)));
                    p.priority = if self.selection.window == Some(data.id) {
                        2
                    } else if display.workspaces[ws.source].is_active {
                        1
                    } else {
                        0
                    };
                    p.resolution = if self.selection.window == Some(data.id) {
                        // Match this card's physical pixels, rather than capturing
                        // a much larger native window merely because it is selected.
                        cgs::CaptureResolution::Thumbnail {
                            width: (frame.size.width * display.scale).ceil().max(1.) as usize,
                            height: (frame.size.height * display.scale).ceil().max(1.) as usize,
                        }
                    } else {
                        let (width, height) = capture_size(frame.size, display.scale);
                        cgs::CaptureResolution::Thumbnail { width, height }
                    };
                    scene.push(p);
                }
            }
            if let Some(drag) = &self.drag {
                if drag.started && contains(display.bounds, drag.point) {
                    if let Some(data) = display
                        .workspaces
                        .iter()
                        .flat_map(|w| &w.windows)
                        .find(|w| w.id == drag.window)
                    {
                        if let Some(id) = data.info.sys_id {
                            let mut frame = drag.preview.frame();
                            frame.origin.x += drag.card.frame().origin.x;
                            frame.origin.y += drag.card.frame().origin.y;
                            let mut p = cgs::Preview::window(id.as_u32(), data.id.pid, frame);
                            p.corner_radius = CORNER as f32;
                            p.border_width = 2.5;
                            p.border_color = border;
                            p.priority = 3;
                            scene.push(p);
                        }
                    }
                }
            }
            // Register the entire inventory once. The capture pool starts
            // foreground streams first, then warms hidden sources in spare slots.
            for data in display.workspaces.iter().flat_map(|ws| &ws.windows) {
                let Some(id) = data.info.sys_id else { continue };
                if scene.iter().any(|p| p.window_id == id.as_u32()) {
                    continue;
                }
                let mut p = cgs::Preview::window(id.as_u32(), data.id.pid, rect(0., 0., 1., 1.));
                p.opacity = 0.;
                p.mode = cgs::CaptureMode::Snapshot;
                p.resolution = cgs::CaptureResolution::Thumbnail { width: 640, height: 480 };
                p.priority = -1;
                scene.push(p);
            }
            let result = metal
                .set_scene(&scene)
                .and_then(|()| metal.show())
                .and_then(|()| metal.render(std::time::Instant::now()).map(|_| ()));
            if let Err(error) = result {
                tracing::warn!(?error, "Metal previews failed; using cached previews");
                display.metal = None;
            }
        }
    }

    pub(crate) fn pump_previews(&mut self, generation: u64, cache: Option<&PreviewCache>) {
        let mut failed = false;
        if let Some(previews) = &mut self.previews {
            previews.pump(generation);
        }
        for display in &mut self.displays {
            if let Some(metal) = &mut display.metal {
                if let Err(error) = metal.render(std::time::Instant::now()) {
                    tracing::warn!(?error, "Metal preview rendering failed");
                    display.metal = None;
                    failed = true;
                }
            }
        }
        let source_failed = self.previews.as_ref().is_some_and(|previews| {
            self.displays.iter().any(|display| {
                let Some(metal) = &display.metal else { return false };
                display.projection.iter().any(|ws| {
                    ws.windows.iter().any(|w| {
                        let data = &display.workspaces[ws.source].windows[w.source];
                        previews.schedule.needs(data.id)
                            && card_visible(display.bounds.size, ws.frame, w.frame)
                            && data.info.sys_id.is_some_and(|id| {
                                !metal.has_frame(id.as_u32())
                                    && matches!(
                                        metal.source_status(id.as_u32()),
                                        Some(
                                            cgs::SourceStatus::Failed(_)
                                                | cgs::SourceStatus::Stopped
                                        )
                                    )
                            })
                    })
                })
            })
        });
        if failed || source_failed {
            self.request_previews(cache);
        }
    }

    fn request_previews(&mut self, cache: Option<&PreviewCache>) {
        self.sync_metal();
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
                        if display.metal.as_ref().is_some_and(|metal| {
                            metal.has_frame(sys_id.as_u32())
                                || !matches!(
                                    metal.source_status(sys_id.as_u32()),
                                    Some(cgs::SourceStatus::Failed(_) | cgs::SourceStatus::Stopped)
                                )
                        }) {
                            continue;
                        }
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
        previews.schedule.retain_visible(&visible, cache);
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
        let _transaction = LayerTransaction::disabled().flush_on_commit();
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
                    // A capture completing just after its card scrolls away
                    // is still useful on the return trip. Identity was checked
                    // above; the shared cache enforces its byte and entry caps.
                    let budget = PREVIEW_BYTES / previews.visible.len().max(1);
                    if let Some(owned) = owned_preview(&image, budget) {
                        let cache = cache.get_or_insert_with(PreviewCache::default);
                        cache.prepare_visible(&previews.visible);
                        cache.insert(&request, owned);
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
    jobs.try_update(Ordering::AcqRel, Ordering::Acquire, |n| (n < 2).then_some(n + 1))
        .is_ok()
}

pub(crate) struct PreviewSession {
    wake: crate::actor::mission_control::Sender,
    pub(crate) rx: actor::Receiver<PreviewEvent>,
    tx: actor::Sender<PreviewEvent>,
    windows: Option<HashMap<u32, Retained<SCWindow>>>,
    discovery_pending: bool,
    schedule: PreviewSchedule,
    failed: bool,
    visible: HashSet<WindowId>,
}

impl PreviewSession {
    fn open(
        enabled: bool,
        _generation: u64,
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
        Some(Self::new(wake))
    }

    fn new(wake: crate::actor::mission_control::Sender) -> Self {
        let (tx, rx) = actor::channel();
        Self {
            wake,
            rx,
            tx,
            windows: None,
            discovery_pending: false,
            schedule: PreviewSchedule::default(),
            failed: false,
            visible: HashSet::default(),
        }
    }

    pub(crate) fn pump(&mut self, generation: u64) {
        if self.failed {
            return;
        }
        if self.windows.is_none() && !self.discovery_pending && !self.schedule.pending.is_empty() {
            self.discovery_pending = true;
            let callback_tx = self.tx.clone();
            let callback =
                RcBlock::new(move |content: *mut SCShareableContent, _: *mut NSError| {
                    if callback_tx.is_closed() {
                        return;
                    }
                    let content = unsafe { Retained::retain(content) };
                    deliver(callback_tx.clone(), PreviewEvent::Content(generation, content));
                });
            unsafe {
                SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(
                true, false, &callback,
            );
            }
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

    fn retain_visible(&mut self, visible: &HashSet<WindowId>, cache: Option<&PreviewCache>) {
        // Cancel work that never started, so a return trip can still refresh
        // it. Completed cached previews remain attempted for this session;
        // opening a new Overview creates a fresh schedule and refreshes them.
        self.pending.retain(|p| {
            let keep = visible.contains(&p.id);
            if !keep {
                self.attempted.remove(&p.id);
            }
            keep
        });
        self.attempted.retain(|id| {
            visible.contains(id)
                || self.active.contains(id)
                || cache.is_some_and(|cache| cache.entries.iter().any(|entry| entry.id == *id))
        });
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
    // One paused presentation context per current display. Pixel reuse is
    // bounded independently inside cgs; no stream remains active here.
    metal: Vec<(ScreenInfo, CGRect, f64, cgs::PreviewCompositor)>,
}
impl PreviewCache {
    fn take_metal(
        &mut self,
        info: &ScreenInfo,
        bounds: CGRect,
        scale: f64,
    ) -> Option<cgs::PreviewCompositor> {
        // Native Space/display changes must never reuse a surface from the old
        // Space. A backing-size/scale change also retires its old drawable pool.
        self.metal.retain(|(key, rect, backing, _)| {
            key.display_uuid != info.display_uuid
                || (key.id == info.id
                    && key.space == info.space
                    && *rect == bounds
                    && *backing == scale)
        });
        let index = self
            .metal
            .iter()
            .position(|(key, _, _, _)| key.display_uuid == info.display_uuid)?;
        Some(self.metal.swap_remove(index).3)
    }

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
    }
}
