//! Session-owned Overview. Runtime frames are truth; captured images only decorate cards.

use cgs::{CgsWindow, LayerTransaction, WindowSurface, release_layer_tree};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSColor, NSFont, NSPopUpMenuWindowLevel, NSRunningApplication, NSScreen};
use objc2_core_foundation::{CFRetained, CFString, CFType, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGColor, CGDisplayBounds, CGImage};
use objc2_foundation::MainThreadMarker;
use objc2_quartz_core::{CABasicAnimation, CAFrameRateRange, CALayer, CAMediaTiming, CATextLayer};

use crate::actor::app::WindowId;
use crate::actor::mission_control::Input;
use crate::actor::reactor::ReactorHandle;
use crate::common::collections::HashMap;
use crate::common::config::MissionControlSettings;
use crate::model::server::{RuntimeWindowData, RuntimeWorkspaceData};
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

fn projected_window(
    display: CGRect,
    ribbon: CGSize,
    window: &RuntimeWindowData,
    offset: f64,
) -> CGRect {
    let scale = ribbon.height / display.size.height;
    let frame = window.layout_frame.unwrap_or(window.info.frame);
    rect(
        (ribbon.width - display.size.width * scale) / 2.
            + (frame.origin.x - display.origin.x) * scale
            - offset,
        (frame.origin.y - display.origin.y) * scale,
        frame.size.width * scale,
        frame.size.height * scale,
    )
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
            let mut windows: Vec<_> = ws
                .windows
                .iter()
                .enumerate()
                .filter_map(|(source, window)| {
                    let projected = projected_window(
                        display,
                        frame.size,
                        window,
                        offsets.get(&ws.index).copied().unwrap_or(0.),
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
        icon_contents: Option<&AnyObject>,
    ) -> Self {
        let card = layer(frame, scale);
        let image = layer(CGRect::ZERO, scale);
        image.setCornerRadius(CORNER);
        image.setMasksToBounds(true);
        image.setBorderColor(Some(&NSColor::controlAccentColor().CGColor()));
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
    metal: Option<cgs::PreviewCompositor>,
    scene: Vec<cgs::Preview>,
    offset: f64,
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
    metal_sources: Vec<cgs::PreviewSource>,
    metal_updates: Vec<cgs::PreviewSource>,
    metal_dirty: bool,
    metal_style: Option<(Option<WindowId>, Option<WindowId>)>,
    metal_border: [f32; 4],
    _surface: WindowSurface,
    _window: CgsWindow,
}

fn preview_source(preview: &cgs::Preview) -> cgs::PreviewSource {
    let mut source = cgs::PreviewSource::from(preview);
    source.frame_interval =
        std::time::Duration::from_secs_f64(1. / if preview.priority >= 2 { 60. } else { 30. });
    source
}

fn accent_border() -> [f32; 4] {
    NSColor::controlAccentColor()
        .colorUsingColorSpace(&objc2_app_kit::NSColorSpace::sRGBColorSpace())
        .map(|c| {
            [
                c.redComponent() as f32,
                c.greenComponent() as f32,
                c.blueComponent() as f32,
                1.,
            ]
        })
        .unwrap_or([0.2, 0.5, 1., 1.])
}

// Draw on the fitted image, leaving the caption and transparent card gutters unboxed.
fn set_preview_border(image: &CALayer, width: f64) { image.setBorderWidth(width); }

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
        {
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
        }
        self.metal_dirty |= visibility_changed;
        Some(visibility_changed)
    }

    /// Stable horizontal scrolling only shifts retained geometry and cached pixels.
    /// Reproject when the set of cards crossing the ribbon clip actually changes.
    fn translate_horizontal(&mut self, workspace: usize) -> bool {
        let Some((view, row)) = self
            .views
            .iter_mut()
            .zip(&mut self.projection)
            .find(|(view, _)| view.index == workspace)
        else {
            return false;
        };
        let offset = self.offsets.get(&workspace).copied().unwrap_or(0.);
        let data = &self.workspaces[row.source];
        let clip = CGRect::new(CGPoint::ZERO, row.frame.size);
        if data.windows.iter().any(|window| {
            intersects(
                projected_window(self.info.frame, row.frame.size, window, offset),
                clip,
            ) != row.windows.iter().any(|w| w.id == window.id)
        }) {
            return false;
        }
        let delta = view.offset - offset;
        if let Some(canvas) = &mut view.metal {
            let _ = canvas.translate(CGPoint::new(delta, 0.));
        }
        for (card, window) in view.cards.iter().zip(&mut row.windows) {
            window.frame.origin.x += delta;
            card.layer.setFrame(window.frame);
        }
        for target in &mut row.scene.targets {
            target.frame.origin.x += delta;
        }
        view.offset = offset;
        true
    }

    fn reproject(&mut self) {
        self.metal_dirty = true;
        let next = project(
            self.info.frame,
            self.bounds.size,
            &self.workspaces,
            self.centered,
            self.workspace_offset,
            &self.offsets,
        );
        for view in &mut self.views {
            let offset = self.offsets.get(&view.index).copied().unwrap_or(0.);
            let delta = view.offset - offset;
            if delta != 0. {
                if let Some(canvas) = &mut view.metal {
                    let _ = canvas.translate(CGPoint::new(delta, 0.));
                }
                view.offset = offset;
            }
        }
        let same_cards = next.len() == self.projection.len()
            && next.iter().zip(&self.projection).all(|(a, b)| {
                a.source == b.source
                    && a.windows.len() == b.windows.len()
                    && a.windows.iter().zip(&b.windows).all(|(a, b)| a.id == b.id)
            });
        if !same_cards {
            self.rebuild();
            return;
        }
        {
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
        }
        self.projection = next;
    }

    fn rebuild(&mut self) {
        self.metal_dirty = true;
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
        {
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
                let (container, heading, empty, backdrop, metal, scene, offset) =
                    if let Some(view) = existing {
                        (
                            view.layer,
                            view.heading,
                            view.empty,
                            view.backdrop,
                            view.metal,
                            view.scene,
                            view.offset,
                        )
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
                        (
                            container,
                            heading,
                            empty,
                            backdrop,
                            None,
                            Vec::new(),
                            self.offsets.get(&data.index).copied().unwrap_or(0.),
                        )
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
                    let card = unsafe {
                        WindowCard::new(
                            data,
                            w.frame,
                            self.scale,
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
                    metal,
                    scene,
                    offset,
                });
            }
        }
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
    frame: Option<cgs::PreviewFrame>,
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
    show_empty_workspaces: bool,
    displays: Vec<DisplayOverview>,
    active: usize,
    selection: Selection,
    hovered: Option<WindowId>,
    drag: Option<OverviewDrag>,
    drop: Option<crate::actor::reactor::OverviewDrop>,
    pressed: Option<CGPoint>,
}

impl Drop for OverviewSession {
    fn drop(&mut self) {
        let _transaction = LayerTransaction::disabled().flush_on_commit();
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
        cache: &mut Option<PreviewRenderers>,
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
            cache.metal.retain(|(info, _, _, _, _)| {
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
                metal_sources: Vec::new(),
                metal_updates: Vec::with_capacity(4),
                metal_dirty: true,
                metal_style: None,
                metal_border: accent_border(),
                _surface: surface,
                _window: window,
            };
            view.rebuild();
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
            show_empty_workspaces: settings.show_empty_workspaces,
            displays,
            active,
            selection,
            hovered: None,
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

    pub(crate) fn edge_tick(&mut self) {
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
            d.reproject();
        }
        self.move_drag(point);
        self.sync_metal();
    }

    pub(crate) fn refresh(&mut self, reactor: &ReactorHandle, moved: Option<WindowId>) {
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
            d.rebuild();
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
        self.update_preview_borders();
        self.highlight(None);
        for display in &mut self.displays {
            if let Some(metal) = &mut display.metal {
                metal.refresh_sources();
            }
        }
        self.sync_metal();
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
        if drag.started {
            for display in &mut self.displays {
                if let Some(metal) = &mut display.metal {
                    let _ = metal.set_visual_scene(&[]);
                    let _ = metal.render(std::time::Instant::now());
                }
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
            frame: d
                .metal
                .as_ref()
                .and_then(|m| data.info.sys_id.and_then(|id| m.captured_frame(id.as_u32()))),
            card: ghost.layer,
            preview: ghost.image,
            indicator,
        });
    }

    fn move_drag(&mut self, point: CGPoint) {
        let Some(drag) = &mut self.drag else { return };
        let previous = drag.point;
        let was_started = drag.started;
        drag.point = point;
        if !drag.started && (point.x - drag.start.x).hypot(point.y - drag.start.y) < 5.0 {
            return;
        }
        drag.started = true;
        let same_display = self
            .displays
            .iter()
            .any(|d| contains(d.bounds, previous) && contains(d.bounds, point));
        if !was_started || !same_display {
            for d in &mut self.displays {
                d.metal_dirty = true;
            }
        }
        drag.intent = None;
        {
            for card in self.displays.iter().flat_map(|d| &d.views).flat_map(|ws| &ws.cards) {
                if card.id == drag.window {
                    card.layer.setHidden(true);
                }
            }
            let Some(d) = self.displays.iter_mut().find(|d| contains(d.bounds, point)) else {
                drag.card.setHidden(true);
                drag.indicator.setHidden(true);
                return;
            };
            let local = CGPoint::new(point.x - d.bounds.origin.x, point.y - d.bounds.origin.y);
            d.root.addSublayer(&drag.indicator);
            d.root.addSublayer(&drag.card);
            drag.card.setHidden(false);
            let old_frame = drag.card.frame();
            let next_frame = rect(
                local.x - drag.size.width / 2.0,
                local.y - drag.size.height / 2.0,
                drag.size.width,
                drag.size.height,
            );
            if was_started && same_display {
                if let Some(metal) = &mut d.metal {
                    let _ = metal.translate(CGPoint::new(
                        next_frame.origin.x - old_frame.origin.x,
                        next_frame.origin.y - old_frame.origin.y,
                    ));
                }
            }
            drag.card.setFrame(next_frame);
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
        }
    }

    pub(crate) fn start_previews(
        &mut self,
        enabled: bool,
        wake: crate::actor::mission_control::Sender,
        mut cache: Option<&mut PreviewRenderers>,
    ) {
        if !enabled {
            return;
        }
        let _transaction = LayerTransaction::disabled().flush_on_commit();
        for display in &mut self.displays {
            if let Some((mut metal, canvases)) = cache
                .as_deref_mut()
                .and_then(|cache| cache.take_metal(&display.info, display.bounds, display.scale))
            {
                if let Err(error) = metal.attach_to(display._window.id(), &display.root) {
                    tracing::warn!(?error, "Metal preview reattachment failed");
                } else {
                    for (index, mut canvas) in canvases {
                        if let Some(view) = display.views.iter_mut().find(|v| v.index == index) {
                            if canvas.attach_to(display._window.id(), &view.layer).is_ok() {
                                view.metal = Some(canvas);
                            }
                        }
                    }
                    display.metal = Some(metal);
                    display.metal_dirty = true;
                    continue;
                }
            }
            let wake = wake.clone();
            match cgs::PreviewCompositor::new_attached(
                display._window.id(),
                &display.root,
                display.bounds,
                display.scale,
                move || {
                    wake.send(crate::actor::mission_control::Event::PumpPreviews);
                },
            ) {
                Ok(metal) => {
                    display.metal = Some(metal);
                    display.metal_dirty = true;
                }
                Err(error) => {
                    tracing::warn!(?error, "Metal previews unavailable; previews disabled")
                }
            }
        }
        self.sync_metal();
    }

    pub(crate) fn retain_previews(&mut self, cache: &mut Option<PreviewRenderers>) {
        // Keep the active display last so it survives a tight closed-cache budget.
        for n in 1..=self.displays.len() {
            let index = (self.active + n) % self.displays.len();
            let display = &mut self.displays[index];
            let canvases: Vec<_> = display
                .views
                .iter_mut()
                .filter_map(|view| {
                    let mut canvas = view.metal.take()?;
                    if canvas.hide().is_err() {
                        return None;
                    }
                    Some((view.index, canvas))
                })
                .collect();
            let Some(mut metal) = display.metal.take() else {
                continue;
            };
            if let Err(error) = metal.hide() {
                tracing::warn!(?error, "Metal preview retirement failed");
                continue;
            }
            let cache = cache.get_or_insert_with(PreviewRenderers::default);
            cache
                .metal
                .retain(|(info, _, _, _, _)| info.display_uuid != display.info.display_uuid);
            // Bound both the optional drag canvas and retained workspace drawable pools.
            let required = cached_canvas_bytes(display.bounds, display.scale, canvases.len());
            let budget = 256 * 1024 * 1024;
            if required > budget {
                continue;
            }
            let mut used: usize = cache
                .metal
                .iter()
                .map(|(_, bounds, scale, _, canvases)| {
                    cached_canvas_bytes(*bounds, *scale, canvases.len())
                })
                .sum();
            while used + required > budget {
                let (_, bounds, scale, _, canvases) = cache.metal.remove(0);
                used -= cached_canvas_bytes(bounds, scale, canvases.len());
            }
            cache.metal.push((
                display.info.clone(),
                display.bounds,
                display.scale,
                metal,
                canvases,
            ));
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
            let previous = self.hovered;
            self.hovered = hovered;
            for card in self.displays.iter().flat_map(|d| &d.views).flat_map(|ws| &ws.cards) {
                if Some(card.id) == previous || Some(card.id) == hovered {
                    set_preview_border(
                        &card.image,
                        if Some(card.id) == hovered { 1.5 } else { 0. },
                    );
                }
            }
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
        {
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
        }
    }

    pub(crate) fn input(&mut self, input: Input) -> Option<OverviewAction> {
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
                for d in &mut self.displays {
                    d.metal_dirty = true;
                }
                self.sync_metal();
                self.update_hover(point);
                let pressed = self.pressed.take();
                if let Some(drag) = drag.filter(|drag| drag.started) {
                    self.drop = drag.intent;
                    return None;
                }
                if pressed.is_some() {
                    return self.input(Input::Click(point));
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
                    self.sync_metal();
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
                    if reprojected && d.translate_vertical(previous).is_some() {
                        translated = true;
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
                    if reprojected {
                        translated = d.translate_horizontal(target.workspace);
                    }
                }
                if reprojected {
                    only_translated = translated;
                    if !translated {
                        d.reproject();
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
                d.reproject();
                reprojected = true;
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
                        d.reproject();
                        reprojected = true;
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
        if reprojected && !only_translated {
            self.update_preview_borders();
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
        let drag_source = self.drag.as_ref().and_then(|drag| {
            self.displays
                .iter()
                .flat_map(|d| &d.workspaces)
                .flat_map(|ws| &ws.windows)
                .find(|w| w.id == drag.window)
                .and_then(|w| w.info.sys_id.map(|id| (id, w.id.pid)))
        });
        for display in &mut self.displays {
            let Some(metal) = &mut display.metal else { continue };
            // Coalesce geometry changes until this display has a drawable. The
            // projection may change repeatedly between callbacks; build only its latest state.
            let ticking =
                display.views.iter().any(|v| v.metal.as_ref().is_some_and(|m| m.is_visible()));
            let ready = metal.has_display_update()
                || display
                    .views
                    .iter()
                    .any(|v| v.metal.as_ref().is_some_and(|m| m.has_display_update()));
            if display.metal_dirty && display.metal_style.is_some() && ticking && !ready {
                for view in &display.views {
                    if let Some(canvas) = &view.metal {
                        canvas.request_frame();
                    }
                }
                continue;
            }
            let style = (self.hovered, self.selection.window);
            if !display.metal_dirty {
                if display.metal_style != Some(style) {
                    let selected = self.selection.window.and_then(|id| {
                        display
                            .workspaces
                            .iter()
                            .flat_map(|ws| &ws.windows)
                            .find(|w| w.id == id)
                            .and_then(|w| w.info.sys_id)
                    });
                    let hovered = self.hovered.and_then(|id| {
                        display
                            .workspaces
                            .iter()
                            .flat_map(|ws| &ws.windows)
                            .find(|w| w.id == id)
                            .and_then(|w| w.info.sys_id)
                    });
                    let previous = display.metal_style.unwrap_or((None, None));
                    let mut affected = [None; 4];
                    for (slot, id) in
                        affected.iter_mut().zip([previous.0, previous.1, style.0, style.1])
                    {
                        *slot = id.and_then(|id| {
                            display
                                .workspaces
                                .iter()
                                .flat_map(|ws| &ws.windows)
                                .find(|w| w.id == id)
                                .and_then(|w| w.info.sys_id)
                        });
                    }
                    display.metal_updates.clear();
                    // Only the old/new hover and selection entries can change.
                    for (view, row) in display.views.iter_mut().zip(&display.projection) {
                        for p in &mut view.scene {
                            if !affected.iter().flatten().any(|id| id.as_u32() == p.window_id) {
                                continue;
                            }
                            p.border_width = if hovered.is_some_and(|id| id.as_u32() == p.window_id)
                            {
                                1.5
                            } else {
                                0.
                            };
                            p.priority = if selected.is_some_and(|id| id.as_u32() == p.window_id) {
                                2
                            } else if p.mode == cgs::CaptureMode::Snapshot {
                                -1
                            } else {
                                i32::from(display.workspaces[row.source].is_active)
                            };
                            let (width, height) = if p.priority == 2 {
                                (
                                    (p.frame.size.width * display.scale).ceil().max(1.) as usize,
                                    (p.frame.size.height * display.scale).ceil().max(1.) as usize,
                                )
                            } else {
                                capture_size(p.frame.size, display.scale)
                            };
                            p.resolution = cgs::CaptureResolution::Thumbnail { width, height };
                            display.metal_updates.push(preview_source(p));
                            if let Some(canvas) = &mut view.metal {
                                let _ = canvas.set_preview_style(
                                    p.window_id,
                                    p.border_width,
                                    p.border_color,
                                );
                            }
                        }
                    }
                    if display.metal_style.is_none_or(|(_, selection)| selection != style.1) {
                        let _ = metal.update_sources(&display.metal_updates);
                    }
                    display.metal_style = Some(style);
                }
                continue;
            }
            display.metal_dirty = false;
            display.metal_style = Some(style);
            let sources = &mut display.metal_sources;
            sources.clear();
            let mut drag_preview = None;
            let border = display.metal_border;
            let viewport = rect(0., 0., display.bounds.size.width, display.bounds.size.height);
            for (view, ws) in display.views.iter_mut().zip(&display.projection) {
                view.scene.clear();
                let row_visible = intersects(ws.frame, viewport);
                if !row_visible {
                    if let Some(canvas) = &mut view.metal {
                        let _ = canvas.hide();
                    }
                    continue;
                }
                for w in &ws.windows {
                    let data = &display.workspaces[ws.source].windows[w.source];
                    if self.drag.as_ref().is_some_and(|d| d.started && d.window == data.id) {
                        continue;
                    }
                    let Some(id) = data.info.sys_id else { continue };
                    let card = pixel_aligned(w.frame, display.scale);
                    let mut frame = pixel_aligned(preview_frame(card.size), display.scale);
                    frame.origin.x += card.origin.x;
                    frame.origin.y += card.origin.y;
                    let mut p = cgs::Preview::window(id.as_u32(), data.id.pid, frame);
                    p.corner_radius = CORNER as f32;
                    p.border_width = if self.hovered == Some(data.id) {
                        1.5
                    } else {
                        0.
                    };
                    p.border_color = border;
                    p.priority = if self.selection.window == Some(data.id) {
                        2
                    } else {
                        i32::from(display.workspaces[ws.source].is_active)
                    };
                    let (width, height) = if p.priority == 2 {
                        (
                            (frame.size.width * display.scale).ceil().max(1.) as usize,
                            (frame.size.height * display.scale).ceil().max(1.) as usize,
                        )
                    } else {
                        capture_size(frame.size, display.scale)
                    };
                    p.resolution = cgs::CaptureResolution::Thumbnail { width, height };
                    if !card_visible(display.bounds.size, ws.frame, w.frame) {
                        p.mode = cgs::CaptureMode::Snapshot;
                        p.priority = -1;
                    }
                    sources.push(preview_source(&p));
                    view.scene.push(p);
                }
                if view.metal.is_none() {
                    match metal.new_canvas(
                        display._window.id(),
                        &view.layer,
                        CGRect::new(CGPoint::ZERO, ws.frame.size),
                        display.scale,
                    ) {
                        Ok(canvas) => view.metal = Some(canvas),
                        Err(error) => {
                            tracing::warn!(?error, "Workspace preview canvas unavailable")
                        }
                    }
                }
                if let Some(canvas) = &mut view.metal {
                    if !canvas.is_visible() {
                        let _ = canvas.attach_to(display._window.id(), &view.layer);
                    }
                    let result = canvas.set_visual_scene(&view.scene).and_then(|()| canvas.show());
                    if let Err(error) = result {
                        tracing::warn!(?error, "Workspace preview scene failed");
                    }
                }
            }
            if let Some(drag) = &self.drag {
                if drag.started && contains(display.bounds, drag.point) {
                    if let Some((id, pid)) = drag_source {
                        let mut frame = drag.preview.frame();
                        frame.origin.x += drag.card.frame().origin.x;
                        frame.origin.y += drag.card.frame().origin.y;
                        let mut p = cgs::Preview::window(id.as_u32(), pid, frame);
                        p.corner_radius = CORNER as f32;
                        p.border_width = 2.5;
                        p.border_color = border;
                        p.priority = 3;
                        p.resolution = cgs::CaptureResolution::Thumbnail {
                            width: (frame.size.width * display.scale).ceil().max(1.) as usize,
                            height: (frame.size.height * display.scale).ceil().max(1.) as usize,
                        };
                        if let Some(frame) = &drag.frame {
                            metal.reuse_frame(frame);
                        }
                        sources.push(preview_source(&p));
                        drag_preview = Some(p);
                    }
                }
            }
            // Capture hidden identities once in spare capacity; they are not render quads.
            for data in display.workspaces.iter().flat_map(|ws| &ws.windows) {
                let Some(id) = data.info.sys_id else { continue };
                if sources.iter().any(|p| p.window_id == id.as_u32()) {
                    continue;
                }
                sources.push(cgs::PreviewSource {
                    window_id: id.as_u32(),
                    process_id: data.id.pid,
                    mode: cgs::CaptureMode::Snapshot,
                    resolution: cgs::CaptureResolution::Thumbnail { width: 640, height: 480 },
                    priority: -1,
                    frame_interval: std::time::Duration::from_secs_f64(1. / 30.),
                });
            }
            let result = metal
                .set_sources(sources)
                .and_then(|()| {
                    metal.set_visual_scene(
                        drag_preview.as_ref().map(std::slice::from_ref).unwrap_or(&[]),
                    )
                })
                .and_then(|()| metal.show());
            if let Err(error) = result {
                tracing::warn!(?error, "Metal previews failed; previews disabled");
                display.metal = None;
            }
        }
    }

    pub(crate) fn pump_previews(&mut self) {
        self.sync_metal();
        for display in &mut self.displays {
            if let Some(metal) = &mut display.metal {
                if let Err(error) = metal.render(std::time::Instant::now()) {
                    tracing::warn!(?error, "Metal preview rendering failed");
                    display.metal = None;
                }
            }
            for view in &mut display.views {
                if let Some(canvas) = &mut view.metal {
                    if let Err(error) = canvas.render(std::time::Instant::now()) {
                        tracing::warn!(?error, "Workspace preview rendering failed");
                        view.metal = None;
                    }
                }
            }
        }
    }
}

fn capture_size(size: CGSize, backing: f64) -> (usize, usize) {
    let width = (size.width * backing).max(1.0);
    let height = (size.height * backing).max(1.0);
    let scale = (1024.0 / width).min(768.0 / height).min(1.0);
    ((width * scale).ceil() as usize, (height * scale).ceil() as usize)
}
fn cached_canvas_bytes(bounds: CGRect, scale: f64, canvases: usize) -> usize {
    let row = ((bounds.size.width * scale).ceil() as usize * 4 + 255) & !255;
    // Each workspace is at most half a display high; reserve a full display
    // for the optional drag canvas, which allocates drawables only on first use.
    row * (bounds.size.height * scale).ceil() as usize * 3 * (2 + canvases) / 2
}

#[derive(Default)]
pub(crate) struct PreviewRenderers {
    // One paused presentation context per current display. Pixel reuse is
    // bounded independently inside cgs; no stream remains active here.
    metal: Vec<(
        ScreenInfo,
        CGRect,
        f64,
        cgs::PreviewCompositor,
        Vec<(usize, cgs::PreviewCompositor)>,
    )>,
}
impl PreviewRenderers {
    fn take_metal(
        &mut self,
        info: &ScreenInfo,
        bounds: CGRect,
        scale: f64,
    ) -> Option<(cgs::PreviewCompositor, Vec<(usize, cgs::PreviewCompositor)>)> {
        // Native Space/display changes must never reuse a surface from the old
        // Space. A backing-size/scale change also retires its old drawable pool.
        self.metal.retain(|(key, rect, backing, _, _)| {
            key.display_uuid != info.display_uuid
                || (key.id == info.id
                    && key.space == info.space
                    && *rect == bounds
                    && *backing == scale)
        });
        let index = self
            .metal
            .iter()
            .position(|(key, _, _, _, _)| key.display_uuid == info.display_uuid)?;
        let (_, _, _, owner, canvases) = self.metal.swap_remove(index);
        Some((owner, canvases))
    }
}
