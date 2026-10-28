//! Persistent window geometry; only the camera projects it into the overview.
use std::time::{Duration, Instant};

use cgs::{CgsWindow, LayerTransaction, WindowSurface, release_layer_tree};
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSColor, NSCursor, NSFont, NSImage, NSRunningApplication, NSScreen, NSWorkspace,
};
use objc2_core_foundation::{
    CFRetained, CFString, CFType, CGAffineTransform, CGPoint, CGRect, CGSize,
};
use objc2_core_graphics::{CGColor, CGDisplayBounds, CGImage, CGPath};
use objc2_foundation::MainThreadMarker;
use objc2_quartz_core::{CALayer, CATextLayer};

use crate::actor::app::WindowId;
use crate::actor::mission_control::Input;
use crate::actor::reactor::{OverviewDrop, OverviewTarget, ReactorHandle};
use crate::common::config::MissionControlSettings;
use crate::model::server::{RuntimeDisplayData, RuntimeWindowData, RuntimeWorkspaceData};
use crate::sys::screen::{NSScreenExt, ScreenInfo};

const MIN_ZOOM: f64 = 0.08;
const MAX_ZOOM: f64 = 2.0;
const OVERVIEW_ZOOM: f64 = 0.34;
const EXIT_MOTION: f64 = 0.28;

fn rect(x: f64, y: f64, w: f64, h: f64) -> CGRect {
    CGRect::new(CGPoint::new(x, y), CGSize::new(w, h))
}
fn contains(r: CGRect, p: CGPoint) -> bool {
    p.x >= r.min().x && p.y >= r.min().y && p.x < r.max().x && p.y < r.max().y
}
fn intersects(a: CGRect, b: CGRect) -> bool {
    a.min().x < b.max().x && b.min().x < a.max().x && a.min().y < b.max().y && b.min().y < a.max().y
}
fn union(a: CGRect, b: CGRect) -> CGRect {
    rect(
        a.min().x.min(b.min().x),
        a.min().y.min(b.min().y),
        a.max().x.max(b.max().x) - a.min().x.min(b.min().x),
        a.max().y.max(b.max().y) - a.min().y.min(b.min().y),
    )
}

/// Position is the world coordinate at the viewport's top-left, in logical points.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Camera {
    position: CGPoint,
    zoom: f64,
}
impl Camera {
    fn project(self, p: CGPoint) -> CGPoint {
        CGPoint::new(
            (p.x - self.position.x) * self.zoom,
            (p.y - self.position.y) * self.zoom,
        )
    }

    fn unproject(self, p: CGPoint) -> CGPoint {
        CGPoint::new(
            p.x / self.zoom + self.position.x,
            p.y / self.zoom + self.position.y,
        )
    }

    fn frame(self, r: CGRect) -> CGRect {
        CGRect::new(
            self.project(r.origin),
            CGSize::new(r.size.width * self.zoom, r.size.height * self.zoom),
        )
    }

    fn zoom_at(&mut self, factor: f64, pivot: CGPoint) {
        let world = self.unproject(pivot);
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        self.position = CGPoint::new(world.x - pivot.x / self.zoom, world.y - pivot.y / self.zoom);
    }

    fn fit(frame: CGRect, viewport: CGSize) -> Self {
        let zoom = ((viewport.width - 112.) / frame.size.width.max(1.))
            .min((viewport.height - 160.) / frame.size.height.max(1.))
            .clamp(MIN_ZOOM, 0.75);
        Self {
            zoom,
            position: CGPoint::new(
                frame.mid().x - viewport.width / (2. * zoom),
                frame.mid().y - (viewport.height - 32.) / (2. * zoom),
            ),
        }
    }
}

#[derive(Clone, Copy)]
enum MotionCurve {
    Settle,
    Enter,
    Smooth,
}

struct Motion {
    from: Camera,
    to: Camera,
    start: Instant,
    duration: f64,
    curve: MotionCurve,
}
impl Motion {
    fn sample(&self, now: Instant) -> (Camera, bool) {
        let t = (now.duration_since(self.start).as_secs_f64() / self.duration).min(1.);
        // Entry settles quickly; focus accelerates into the window without a first-frame jump.
        let t = match self.curve {
            MotionCurve::Settle => (1. - (-6. * t).exp()) / (1. - (-6.0_f64).exp()),
            // A direct initial response with a zero-velocity landing, without bounce.
            MotionCurve::Enter => 1. - (1. - t).powi(3),
            MotionCurve::Smooth => t * t * (3. - 2. * t),
        };
        let mix = |a: f64, b: f64| a + (b - a) * t;
        let zoom = mix(self.from.zoom, self.to.zoom);
        (
            Camera {
                zoom,
                position: CGPoint::new(
                    mix(
                        self.from.position.x * self.from.zoom,
                        self.to.position.x * self.to.zoom,
                    ) / zoom,
                    mix(
                        self.from.position.y * self.from.zoom,
                        self.to.position.y * self.to.zoom,
                    ) / zoom,
                ),
            },
            t >= 1.,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub workspace: usize,
    pub window: Option<WindowId>,
}
pub(crate) enum OverviewAction {
    Dismiss,
    Activate {
        display: String,
        workspace: crate::model::VirtualWorkspaceId,
        selection: Selection,
    },
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
fn set_text(l: &CATextLayer, text: &str) {
    let text = CFString::from_str(text);
    unsafe {
        l.setString(Some(&*(text.as_ref() as *const AnyObject)));
    }
}
fn text(
    parent: &CALayer,
    value: &str,
    frame: CGRect,
    scale: f64,
    size: f64,
) -> Retained<CATextLayer> {
    let l = CATextLayer::layer();
    l.setFrame(frame);
    l.setContentsScale(scale);
    let font = NSFont::systemFontOfSize(size);
    unsafe {
        l.setFont(Some(&*(Retained::as_ptr(&font) as *const CFType)));
        l.setTruncationMode(objc2_quartz_core::kCATruncationEnd);
    }
    l.setFontSize(size);
    l.setForegroundColor(Some(&color(1., 1., 1., 0.9)));
    set_text(&l, value);
    parent.addSublayer(&l);
    l
}
fn pill(frame: CGRect, scale: f64, radius: f64) -> Retained<CALayer> {
    let l = layer(frame, scale);
    l.setCornerRadius(radius);
    l.setMasksToBounds(true);
    if let Some(blur) = cgs::backdrop_blur(16.) {
        blur.setFrame(l.bounds());
        blur.setAutoresizingMask(
            objc2_quartz_core::CAAutoresizingMask::LayerWidthSizable
                | objc2_quartz_core::CAAutoresizingMask::LayerHeightSizable,
        );
        l.addSublayer(&blur);
    }
    let tint = layer(l.bounds(), scale);
    tint.setAutoresizingMask(
        objc2_quartz_core::CAAutoresizingMask::LayerWidthSizable
            | objc2_quartz_core::CAAutoresizingMask::LayerHeightSizable,
    );
    tint.setBackgroundColor(Some(&color(0.12, 0.13, 0.14, 0.72)));
    l.addSublayer(&tint);
    l.setBorderWidth(0.5);
    l.setBorderColor(Some(&color(1., 1., 1., 0.15)));
    l
}
fn symbol(parent: &CALayer, name: &str, x: f64, scale: f64) {
    let config = objc2_app_kit::NSImageSymbolConfiguration::configurationWithHierarchicalColor(
        &NSColor::colorWithWhite_alpha(0.85, 0.7),
    );
    if let Some(image) =
        cgs::Symbol::header(name).and_then(|i| i.imageWithSymbolConfiguration(&config))
        && let Some(cg) =
            unsafe { image.CGImageForProposedRect_context_hints(std::ptr::null_mut(), None, None) }
    {
        let l = layer(rect(x, 11., 13., 13.), scale);
        image_contents(&l, &cg);
        parent.addSublayer(&l);
    }
}
fn image_contents(l: &CALayer, image: &CGImage) {
    unsafe {
        l.setContents(Some(&*(image as *const CGImage as *const AnyObject)));
    }
}
fn app_icon(pid: i32) -> Option<Retained<CGImage>> {
    let image = NSRunningApplication::runningApplicationWithProcessIdentifier(pid)?.icon()?;
    unsafe { image.CGImageForProposedRect_context_hints(std::ptr::null_mut(), None, None) }
}

fn window_badge(
    name: &str,
    icon: Option<&CGImage>,
    scale: f64,
) -> (
    Retained<CALayer>,
    Retained<CATextLayer>,
    Retained<CALayer>,
    Retained<CALayer>,
    f64,
) {
    let width = (name.chars().count() as f64 * 7.5 + 42.).clamp(64., 240.);
    let badge = pill(rect(0., 0., width, 28.), scale, 14.);
    badge.setAnchorPoint(CGPoint::ZERO);
    badge.setZPosition(2.);
    let first = layer(rect(7., 6., 16., 16.), scale);
    if let Some(icon) = icon {
        image_contents(&first, icon);
    }
    badge.addSublayer(&first);
    let second = layer(rect(21., 6., 16., 16.), scale);
    second.setHidden(true);
    badge.addSublayer(&second);
    let title = text(&badge, name, rect(30., 5., width - 36., 20.), scale, 14.);
    (badge, title, first, second, width)
}

fn same_workspace_scene(
    a: &RuntimeWorkspaceData,
    b: &RuntimeWorkspaceData,
    shared: bool,
    space: Option<crate::sys::screen::SpaceId>,
) -> bool {
    a.workspace_id == b.workspace_id
        && a.index == b.index
        && a.name == b.name
        && a.space == b.space
        && a.display_frame == b.display_frame
        && a.layout_mode == b.layout_mode
        && a.display_uuid == b.display_uuid
        && a.focused_window == b.focused_window
        && a.is_active == (b.is_active && (!shared || Some(b.space) == space))
        && a.windows.len() == b.windows.len()
        && a.windows.iter().zip(&b.windows).all(|(a, b)| {
            a.id == b.id
                && a.info.sys_id == b.info.sys_id
                && a.app_name == b.app_name
                && a.is_floating == b.is_floating
                && a.layout_position == b.layout_position
                && a.layout_frame.unwrap_or(a.info.frame) == b.layout_frame.unwrap_or(b.info.frame)
                && a.info.is_minimized == b.info.is_minimized
        })
}

struct SceneWindow {
    data: RuntimeWindowData,
    workspace: crate::model::VirtualWorkspaceId,
    workspace_index: usize,
    world: CGRect,
    // Decorations share the camera but stay sharp and understated at small scales.
    shadow: Retained<CALayer>,
    preview: cgs::PreviewLayer,
    icon_image: Option<Retained<CGImage>>,
}
impl SceneWindow {
    fn new(
        data: RuntimeWindowData,
        ws: &RuntimeWorkspaceData,
        world: CGRect,
        root: &CALayer,
        scale: f64,
        icon: Option<Retained<CGImage>>,
    ) -> Self {
        let shadow = layer(CGRect::ZERO, scale);
        shadow.setCornerRadius(8.);
        shadow.setBackgroundColor(Some(&color(0.10, 0.105, 0.115, 1.)));
        shadow.setShadowColor(Some(&color(0., 0., 0., 1.)));
        shadow.setShadowOpacity(0.28);
        shadow.setShadowRadius(12.);
        shadow.setShadowOffset(CGSize::new(0., 4.));
        shadow.setZPosition(0.5);
        root.addSublayer(&shadow);
        let preview = cgs::PreviewLayer::new();
        preview.layer().setMasksToBounds(true);
        shadow.addSublayer(preview.layer());
        Self {
            data,
            workspace: ws.workspace_id,
            workspace_index: ws.index,
            world,
            shadow,
            preview,
            icon_image: icon,
        }
    }

    fn detach(&self) { self.shadow.removeFromSuperlayer(); }

    fn apply_geometry(&self) {
        let bounds = CGRect::new(CGPoint::ZERO, self.world.size);
        if self.preview.layer().bounds() != bounds {
            let path = unsafe { CGPath::with_rounded_rect(bounds, 10., 10., std::ptr::null()) };
            self.shadow.setShadowPath(Some(&path));
            self.preview.layer().setFrame(bounds);
        }
        self.shadow.setFrame(self.world);
        self.shadow.setCornerRadius(10.);
        self.shadow.setShadowRadius(18.);
        self.shadow.setShadowOffset(CGSize::new(0., 10.));
        self.preview.layer().setCornerRadius(10.);
    }
}

// Translate scrolling workspaces once around remembered focus; camera motion never relayouts them.
fn workspace_offset(ws: &RuntimeWorkspaceData, info: &ScreenInfo) -> f64 {
    if ws.layout_mode != "scrolling" {
        return (info.frame.size.width - ws.display_frame.size.width) / 2.;
    }
    ws.focused_window
        .and_then(|id| ws.windows.iter().find(|w| w.id == id))
        .map(|w| {
            let frame = w.layout_frame.unwrap_or(w.info.frame);
            info.frame.size.width / 2. - (frame.mid().x - ws.display_frame.origin.x)
        })
        .unwrap_or(0.)
}

struct WorkspaceRegion {
    id: crate::model::VirtualWorkspaceId,
    origin: CGPoint,
    label: Retained<CALayer>,
    title: Retained<CATextLayer>,
    caption: String,
    icon: Retained<CALayer>,
    extra_icon: Retained<CALayer>,
    bounds: Option<CGRect>,
    drops: crate::actor::drag::DragScene,
}
impl WorkspaceRegion {
    fn new(ws: &RuntimeWorkspaceData, origin: CGPoint, root: &CALayer, scale: f64) -> Self {
        let (label, title, icon, extra_icon, _) = window_badge("", None, scale);
        label.setOpacity(0.);
        root.addSublayer(&label);
        let mut region = Self {
            id: ws.workspace_id,
            origin,
            label,
            title,
            caption: String::new(),
            icon,
            extra_icon,
            bounds: None,
            drops: Default::default(),
        };
        region.update_label(ws, &[]);
        region
    }

    fn update_label(&mut self, ws: &RuntimeWorkspaceData, windows: &[SceneWindow]) {
        let members = windows.iter().filter(|w| w.workspace == self.id);
        self.bounds = members.clone().map(|w| w.world).reduce(union);
        let count = members.clone().count();
        let first = members.clone().min_by(|a, b| {
            a.world
                .origin
                .y
                .total_cmp(&b.world.origin.y)
                .then(a.world.origin.x.total_cmp(&b.world.origin.x))
        });
        let second = first.and_then(|first| members.clone().find(|w| w.data.id != first.data.id));
        for (layer, window) in [(&self.icon, first), (&self.extra_icon, second)] {
            let image = window.and_then(|w| w.icon_image.as_deref());
            layer.setHidden(image.is_none());
            if let Some(image) = image {
                image_contents(layer, image);
            } else {
                unsafe {
                    layer.setContents(None);
                }
            }
        }
        let detail = match count {
            0 => "Empty".into(),
            1 => first.and_then(|w| w.data.app_name.clone()).unwrap_or_else(|| "Window".into()),
            n => format!("{n} windows"),
        };
        let caption = format!("{} · {detail}", ws.name);
        let leading = if count == 0 {
            10.
        } else if count == 1 {
            30.
        } else {
            44.
        };
        if caption == self.caption {
            return;
        }
        let width = (caption.chars().count() as f64 * 7.5 + leading + 12.).clamp(64., 240.);
        self.label.setBounds(rect(0., 0., width, 28.));
        self.title.setFrame(rect(leading, 5., width - leading - 6., 20.));
        set_text(&self.title, &caption);
        self.caption = caption;
    }
}

struct DisplayOverview {
    info: ScreenInfo,
    bounds: CGRect,
    scale: f64,
    home_anchor: CGPoint,
    workspace_height: f64,
    regions: Vec<WorkspaceRegion>,
    workspaces: Vec<RuntimeWorkspaceData>,
    selection: Selection,
    windows: Vec<SceneWindow>,
    camera: Camera,
    home: Camera,
    motion: Option<Motion>,
    background: Retained<CALayer>,
    desktop_snapshot: Option<cgs::DesktopSnapshot>,
    background_ready: bool,
    wallpaper_key: Option<String>,
    root: Retained<CALayer>,
    pictures: Retained<CALayer>,
    focus_ring: Retained<CALayer>,
    controls: Retained<CALayer>,
    percentage: Retained<CATextLayer>,
    help: Retained<CATextLayer>,
    help_index: Option<usize>,
    last_percentage: i32,
    captures: Option<cgs::PreviewCompositor>,
    clock: Option<crate::sys::display_link::DisplayLink>,
    camera_input: Instant,
    sources: Vec<cgs::PreviewSource>,
    policy_dirty: bool,
    dragged: Option<WindowId>,
    applied: Option<(Camera, f32)>,
    // CAContext must detach before its CGS window is released.
    _surface: WindowSurface,
    _window: CgsWindow,
}
impl DisplayOverview {
    fn new(
        info: ScreenInfo,
        workspaces: Vec<RuntimeWorkspaceData>,
        mtm: MainThreadMarker,
    ) -> Option<Self> {
        let screen =
            NSScreen::screens(mtm).iter().find(|s| s.get_number().ok() == Some(info.id))?;
        let full = CGDisplayBounds(info.id.as_u32());
        let inset = (screen.frame().max().y - screen.visibleFrame().max().y).max(0.);
        let bounds = rect(
            full.origin.x,
            full.origin.y + inset,
            full.size.width,
            full.size.height - inset,
        );
        let scale = screen.backingScaleFactor();
        let root = layer(CGRect::new(CGPoint::ZERO, bounds.size), scale);
        root.setGeometryFlipped(true);
        root.setMasksToBounds(true);
        let background = layer(rect(0., -inset, full.size.width, full.size.height), scale);
        // A native desktop asset, decoded once per overview context; no wallpaper capture stream.
        let wallpaper_url = NSWorkspace::sharedWorkspace().desktopImageURLForScreen(&screen);
        let wallpaper_key = wallpaper_url
            .as_ref()
            .and_then(|url| url.absoluteString())
            .map(|url| url.to_string());
        let mut background_ready = false;
        if let Some(url) = wallpaper_url
            && let Some(image) = NSImage::initWithContentsOfURL(NSImage::alloc(), &url)
            && let Some(image) = unsafe {
                image.CGImageForProposedRect_context_hints(std::ptr::null_mut(), None, None)
            }
        {
            image_contents(&background, &image);
            background_ready = true;
            unsafe {
                background.setContentsGravity(objc2_quartz_core::kCAGravityResizeAspectFill);
            }
        } else {
            // Native solid-color desktops and unavailable dynamic assets have no file to decode.
            background.setBackgroundColor(Some(&NSColor::windowBackgroundColor().CGColor()));
        }
        background.setOpacity(0.);
        root.addSublayer(&background);
        let pictures = layer(CGRect::ZERO, scale);
        pictures.setZPosition(0.5);
        pictures.setAnchorPoint(CGPoint::ZERO);
        root.addSublayer(&pictures);
        let focus_ring = layer(CGRect::ZERO, scale);
        focus_ring.setZPosition(1.5);
        focus_ring.setBorderWidth(2.5);
        focus_ring.setBorderColor(Some(&NSColor::controlAccentColor().CGColor()));
        focus_ring.setHidden(true);
        root.addSublayer(&focus_ring);
        let window = CgsWindow::new_compositor(bounds, 0.).ok()?;
        window.set_resolution(scale).ok()?;
        window.set_level(19).ok()?;
        window.disable_shadow();
        let surface = WindowSurface::new_scaled(window.id(), root.bounds(), &root, scale).ok()?;
        let controls = pill(rect(18., bounds.size.height - 138., 270., 35.), scale, 17.5);

        controls.setBorderWidth(0.5);
        controls.setBorderColor(Some(&color(1., 1., 1., 0.15)));
        controls.setZPosition(3.);
        controls.setOpacity(0.);
        root.addSublayer(&controls);
        symbol(&controls, "minus", 12., scale);
        let percentage = text(&controls, "34%", rect(42., 9., 40., 18.), scale, 13.);
        symbol(&controls, "plus", 88., scale);
        symbol(&controls, "arrow.up.left.and.arrow.down.right", 134., scale);
        symbol(&controls, "square.grid.2x2", 169., scale);
        // The final two compact glyphs focus the selected window / return to desktop.
        symbol(&controls, "rectangle.topthird.inset.filled", 211., scale);
        symbol(&controls, "macwindow", 242., scale);
        let help = text(
            &root,
            "",
            rect(18., bounds.size.height - 165., 620., 20.),
            scale,
            11.,
        );
        help.setZPosition(3.);
        help.setShadowOpacity(0.8);
        help.setShadowRadius(3.);
        help.setShadowOffset(CGSize::ZERO);
        help.setHidden(true);
        let camera = Camera {
            position: CGPoint::ZERO,
            zoom: 1.,
        };
        let origin_workspace = workspaces.iter().position(|w| w.is_active).unwrap_or(0);
        let home_anchor = CGPoint::new(0., origin_workspace as f64);
        let workspace_height = workspaces
            .iter()
            .map(|ws| ws.display_frame.size.height)
            .fold(info.frame.size.height, f64::max);
        let selection = Selection {
            workspace: workspaces[origin_workspace].index,
            window: workspaces[origin_workspace].focused_window,
        };
        let regions = workspaces
            .iter()
            .enumerate()
            .map(|(slot, ws)| {
                WorkspaceRegion::new(
                    ws,
                    CGPoint::new(
                        workspace_offset(ws, &info),
                        (slot as f64 - home_anchor.y) * (workspace_height + 120.),
                    ),
                    &root,
                    scale,
                )
            })
            .collect();
        let mut d = Self {
            info,
            bounds,
            scale,
            home_anchor,
            workspace_height,
            regions,
            workspaces,
            selection,
            windows: Vec::new(),
            camera,
            home: camera,
            motion: None,
            background,
            desktop_snapshot: None,
            background_ready,
            wallpaper_key,
            root,
            pictures,
            focus_ring,
            controls,
            percentage,
            help,
            help_index: None,
            last_percentage: 34,
            captures: None,
            clock: None,
            camera_input: Instant::now(),
            sources: Vec::new(),
            policy_dirty: true,
            dragged: None,
            applied: None,
            _surface: surface,
            _window: window,
        };
        d.reconcile();
        d.home = d.default_camera();
        d.camera = d.desktop_camera();
        d._window.order_above(None).ok()?;
        Some(d)
    }

    fn workspace_origin(&self, index: usize) -> CGPoint {
        self.workspaces
            .iter()
            .find(|ws| ws.index == index)
            .and_then(|ws| self.regions.iter().find(|r| r.id == ws.workspace_id))
            .map_or(CGPoint::ZERO, |r| r.origin)
    }

    fn window_origin(&self, ws: &RuntimeWorkspaceData) -> CGPoint {
        let home = self.workspace_origin(ws.index);
        CGPoint::new(
            home.x - ws.display_frame.origin.x,
            home.y - ws.display_frame.origin.y
                + (self.workspace_height - ws.display_frame.size.height) / 2.,
        )
    }

    fn home_for_slot(&self, slot: usize) -> CGPoint {
        CGPoint::new(
            0.,
            (slot as f64 - self.home_anchor.y) * (self.workspace_height + 120.),
        )
    }

    fn reconcile(&mut self) {
        self.regions.retain(|r| {
            let keep = self.workspaces.iter().any(|ws| ws.workspace_id == r.id);
            if !keep {
                r.label.removeFromSuperlayer();
            }
            keep
        });
        for ws in &self.workspaces {
            if !self.regions.iter().any(|r| r.id == ws.workspace_id) {
                let slot = (0..=self.regions.len())
                    .find(|&i| !self.regions.iter().any(|r| r.origin.y == self.home_for_slot(i).y))
                    .unwrap();
                let mut origin = self.home_for_slot(slot);
                origin.x = workspace_offset(ws, &self.info);
                self.regions.push(WorkspaceRegion::new(ws, origin, &self.root, self.scale));
            }
        }
        let previous_order: Vec<_> =
            self.windows.iter().map(|w| (w.data.id, w.data.info.sys_id)).collect();
        let mut old = std::mem::take(&mut self.windows);
        // Map each workspace's real unparked geometry by a stable translation, never a layout.
        for ws in &self.workspaces {
            let home = if ws.layout_mode == "scrolling" {
                // Native viewport scrolling is a shared translation. Preserve a
                // stable anchor while real topology/size edits move its neighbors.
                old.iter()
                    .filter(|w| w.workspace == ws.workspace_id)
                    .find_map(|w| {
                        let data = ws.windows.iter().find(|data| {
                            data.id == w.data.id && data.layout_position == w.data.layout_position
                        })?;
                        let native = data.layout_frame.unwrap_or(data.info.frame);
                        Some(CGPoint::new(
                            w.world.origin.x - native.origin.x,
                            w.world.origin.y - native.origin.y,
                        ))
                    })
                    .unwrap_or_else(|| self.window_origin(ws))
            } else {
                self.window_origin(ws)
            };
            for data in &ws.windows {
                if data.info.is_minimized || !data.info.is_standard || data.info.sys_id.is_none() {
                    continue;
                }
                if let Some(i) = old.iter().position(|w| w.data.id == data.id) {
                    let mut w = old.remove(i);
                    let native = data.layout_frame.unwrap_or(data.info.frame);
                    w.world = CGRect::new(
                        CGPoint::new(native.origin.x + home.x, native.origin.y + home.y),
                        native.size,
                    );
                    w.data = data.clone();
                    w.workspace = ws.workspace_id;
                    w.workspace_index = ws.index;
                    self.windows.push(w);
                } else {
                    let native = data.layout_frame.unwrap_or(data.info.frame);
                    let world = rect(
                        native.origin.x + home.x,
                        native.origin.y + home.y,
                        native.size.width,
                        native.size.height,
                    );
                    let icon = app_icon(data.id.pid);
                    self.windows.push(SceneWindow::new(
                        data.clone(),
                        ws,
                        world,
                        &self.root,
                        self.scale,
                        icon,
                    ));
                }
            }
        }
        for w in old {
            w.detach();
        }
        let inventory_changed = previous_order.len() != self.windows.len()
            || self
                .windows
                .iter()
                .any(|w| !previous_order.contains(&(w.data.id, w.data.info.sys_id)));
        if inventory_changed {
            let order = crate::sys::window_server::stacking_order();
            self.windows.sort_by_key(|w| {
                std::cmp::Reverse(
                    order
                        .iter()
                        .position(|id| Some(*id) == w.data.info.sys_id)
                        .unwrap_or(usize::MAX),
                )
            });
        } else {
            self.windows.sort_by_key(|w| {
                previous_order.iter().position(|(id, _)| *id == w.data.id).unwrap()
            });
        }
        for w in &self.windows {
            self.pictures.addSublayer(&w.shadow);
        }
        for region in &mut self.regions {
            if let Some(ws) = self.workspaces.iter().find(|ws| ws.workspace_id == region.id) {
                region.update_label(ws, &self.windows);
                region.drops.action_override = (ws.layout_mode == "stack")
                    .then_some(crate::layout_engine::WindowDropAction::Swap);
                region.drops.targets.clear();
                region.drops.targets.extend(
                    self.windows
                        .iter()
                        .rev()
                        .filter(|w| w.workspace == region.id && !w.data.is_floating)
                        .map(|w| crate::actor::drag::DragSceneTarget {
                            window: w.data.id,
                            space: ws.space,
                            frame: w.world,
                        }),
                );
            }
        }
        self.invalidate_scene();
    }

    fn desktop_camera(&self) -> Camera {
        let workspace = self.workspaces.iter().find(|ws| ws.is_active);
        let window = workspace.and_then(|ws| {
            self.windows
                .iter()
                .find(|w| Some(w.data.id) == ws.focused_window)
                .or_else(|| self.windows.iter().find(|w| w.workspace == ws.workspace_id))
        });
        let position = window
            .map(|w| {
                CGPoint::new(
                    w.world.origin.x - w.data.info.frame.origin.x + self.bounds.origin.x,
                    w.world.origin.y - w.data.info.frame.origin.y + self.bounds.origin.y,
                )
            })
            .unwrap_or_else(|| {
                workspace.map_or(CGPoint::ZERO, |ws| self.workspace_origin(ws.index))
            });
        Camera { zoom: 1., position }
    }

    fn default_camera(&self) -> Camera {
        let workspace = self.workspaces.iter().find(|w| w.is_active).unwrap_or(&self.workspaces[0]);
        let origin = self.workspace_origin(workspace.index);
        Camera {
            zoom: OVERVIEW_ZOOM,
            position: CGPoint::new(
                self.info.frame.size.width / 2. - self.bounds.size.width / (2. * OVERVIEW_ZOOM),
                origin.y + self.workspace_height / 2.
                    - self.bounds.size.height / (2. * OVERVIEW_ZOOM),
            ),
        }
    }

    fn fit(&self) -> Camera {
        self.windows
            .iter()
            .map(|w| w.world)
            .reduce(union)
            .map(|r| Camera::fit(r, self.bounds.size))
            .unwrap_or(self.camera)
    }

    fn invalidate_scene(&mut self) {
        self.applied = None;
        self.policy_dirty = true;
    }

    fn apply_camera(&mut self, decorations: f32) {
        if self.applied == Some((self.camera, decorations)) {
            return;
        }
        let previous = self.applied.replace((self.camera, decorations));
        let zoom = self.camera.zoom;
        let geometry = previous.is_none();
        let zoom_changed = previous.is_none_or(|(camera, _)| camera.zoom != zoom);
        let opacity_changed = previous.is_none_or(|(_, opacity)| opacity != decorations);
        self.pictures.setPosition(CGPoint::new(
            -self.camera.position.x * zoom,
            -self.camera.position.y * zoom,
        ));
        if zoom_changed {
            self.pictures.setAffineTransform(CGAffineTransform {
                a: zoom,
                b: 0.,
                c: 0.,
                d: zoom,
                tx: 0.,
                ty: 0.,
            });
        }
        let factor = (zoom / OVERVIEW_ZOOM).sqrt().clamp(0.75, 1.1);
        let viewport = CGRect::new(CGPoint::ZERO, self.bounds.size);
        for region in &self.regions {
            let bounds = region.bounds.unwrap_or(CGRect::new(
                region.origin,
                CGSize::new(self.info.frame.size.width, self.workspace_height),
            ));
            let group = self.camera.frame(bounds);
            let hidden = !intersects(group, viewport) || group.max().y < 44. * factor;
            if region.label.isHidden() != hidden {
                region.label.setHidden(hidden);
            }
            if !hidden {
                let width = region.label.bounds().size.width * factor;
                let x =
                    group.origin.x.max(12.).min((self.bounds.size.width - width - 12.).max(12.));
                let y = (group.origin.y - 38. * factor).max(12.);
                region.label.setPosition(CGPoint::new(x, y));
                if zoom_changed {
                    region.label.setAffineTransform(CGAffineTransform {
                        a: factor,
                        b: 0.,
                        c: 0.,
                        d: factor,
                        tx: 0.,
                        ty: 0.,
                    });
                }
            }
            if opacity_changed {
                region.label.setOpacity(decorations);
            }
        }
        for w in &self.windows {
            let visible = intersects(self.camera.frame(w.world), viewport);
            let hidden = !visible || self.dragged == Some(w.data.id);
            if w.shadow.isHidden() != hidden {
                w.shadow.setHidden(hidden);
            }
            if opacity_changed {
                w.shadow.setOpacity(if w.preview.has_frame() || decorations > 0. {
                    1.
                } else {
                    0.
                });
                w.shadow.setShadowOpacity(0.28 * decorations);
            }
            if geometry {
                w.apply_geometry();
            }
        }
        self.update_focus_ring(decorations);
        let percent = (self.camera.zoom * 100.).round() as i32;
        if percent != self.last_percentage {
            set_text(&self.percentage, &format!("{percent}%"));
            self.last_percentage = percent;
        }
        if opacity_changed {
            self.controls.setOpacity(decorations);
        }
    }

    fn update_focus_ring(&self, opacity: f32) {
        let selected = self.windows.iter().find(|w| Some(w.data.id) == self.selection.window);
        let visible = !self.controls.isHidden()
            && self.dragged != self.selection.window
            && selected.is_some();
        self.focus_ring.setHidden(!visible);
        if let Some(w) = selected {
            let camera = self.applied.map_or(self.camera, |(camera, _)| camera);
            self.focus_ring.setFrame(camera.frame(w.world));
            self.focus_ring.setCornerRadius(10. * camera.zoom);
        }
        self.focus_ring.setOpacity(opacity);
    }

    fn request_camera_frame(&mut self) {
        self.camera_input = Instant::now();
        if let Some(clock) = &self.clock {
            clock.set_paused(false);
        }
    }

    fn animate(&mut self, to: Camera, duration: f64) {
        self.motion = Some(Motion {
            from: self.camera,
            to,
            start: Instant::now(),
            duration: duration.max(0.001),
            curve: MotionCurve::Settle,
        });
        self.request_camera_frame();
    }

    fn sample(&mut self, now: Instant) {
        if let Some(motion) = &self.motion {
            let (camera, done) = motion.sample(now);
            self.camera = camera;
            if done {
                self.motion = None;
                self.policy_dirty = true;
            }
        }
    }

    fn local(&self, p: CGPoint) -> CGPoint {
        CGPoint::new(p.x - self.bounds.origin.x, p.y - self.bounds.origin.y)
    }

    fn workspace_label_at(&self, local: CGPoint) -> Option<crate::model::VirtualWorkspaceId> {
        self.regions
            .iter()
            .find(|r| !r.label.isHidden() && contains(r.label.frame(), local))
            .map(|r| r.id)
    }

    fn hit(&self, p: CGPoint) -> Option<usize> {
        let world = self.camera.unproject(self.local(p));
        self.windows.iter().rposition(|w| contains(w.world, world))
    }

    fn update_sources(&mut self, selected: Option<WindowId>, immediate: bool) {
        self.update_sources_at(selected, immediate, self.camera);
    }

    fn update_sources_at(&mut self, selected: Option<WindowId>, immediate: bool, camera: Camera) {
        if !self.policy_dirty && !immediate {
            return;
        }
        let viewport = CGRect::new(CGPoint::ZERO, self.bounds.size);
        let inventory_changed = self.sources.len() != self.windows.len()
            || self.windows.iter().zip(&self.sources).any(|(w, s)| {
                w.data.info.sys_id.unwrap().as_u32() != s.window_id || w.data.id.pid != s.process_id
            });
        self.sources.clear();
        for w in &self.windows {
            let projected = camera.frame(w.world);
            let visible = intersects(projected, viewport) || self.dragged == Some(w.data.id);
            let important = selected == Some(w.data.id);
            let zoom = self
                .motion
                .as_ref()
                .filter(|m| intersects(m.to.frame(w.world), viewport))
                .map_or(camera.zoom, |m| m.to.zoom.max(camera.zoom));
            let zoom = zoom.min(if visible { 1. } else { 0.20 });
            let width = (w.world.size.width * zoom * self.scale).ceil().max(1.) as usize;
            let height = (w.world.size.height * zoom * self.scale).ceil().max(1.) as usize;
            self.sources.push(cgs::PreviewSource {
                window_id: w.data.info.sys_id.unwrap().as_u32(),
                process_id: w.data.id.pid,
                mode: if visible {
                    cgs::CaptureMode::Live
                } else {
                    cgs::CaptureMode::Snapshot
                },
                resolution: cgs::CaptureResolution::Thumbnail { width, height },
                priority: if important && visible {
                    3
                } else if visible {
                    1
                } else {
                    -1
                },
                frame_interval: Duration::from_secs_f64(
                    1. / if important {
                        if camera.zoom >= 0.75 { 30. } else { 15. }
                    } else {
                        5.
                    },
                ),
            });
        }
        if let Some(m) = &mut self.captures {
            let result = if inventory_changed {
                m.set_sources(&self.sources)
            } else {
                m.update_sources(&self.sources)
            };
            if let Err(error) = result {
                tracing::warn!(?error, "Spatial source update failed");
            }
        }
        self.policy_dirty = false;
    }
}

// Logical scene state survives focus handoffs; capture producers never do.
struct ViewState {
    camera: Camera,
    anchor: CGPoint,
    workspace: Option<crate::model::VirtualWorkspaceId>,
    selection: Selection,
    homes: Vec<(crate::model::VirtualWorkspaceId, CGPoint)>,
    windows: Vec<(RuntimeWindowData, crate::model::VirtualWorkspaceId, CGRect)>,
    wallpaper: Option<Retained<AnyObject>>,
    wallpaper_key: Option<String>,
}
#[derive(Default)]
pub(crate) struct PreviewCache {
    captures: Vec<(ScreenInfo, CGRect, f64, cgs::PreviewCompositor, ViewState)>,
}
impl PreviewCache {
    fn take(&mut self, d: &mut DisplayOverview) -> Option<cgs::PreviewCompositor> {
        self.captures.retain(|(key, bounds, scale, _, _)| {
            key.display_uuid != d.info.display_uuid
                || (key.id == d.info.id
                    && key.space == d.info.space
                    && *bounds == d.bounds
                    && *scale == d.scale)
        });
        let i = self
            .captures
            .iter()
            .position(|(key, _, _, _, _)| key.display_uuid == d.info.display_uuid)?;
        let (_, _, _, captures, state) = self.captures.swap_remove(i);
        if !d.background_ready
            && state.wallpaper_key == d.wallpaper_key
            && let Some(image) = state.wallpaper
        {
            unsafe {
                d.background.setContents(Some(&image));
            }
            d.background_ready = true;
        }
        if state
            .homes
            .iter()
            .map(|(id, _)| *id)
            .eq(d.workspaces.iter().map(|ws| ws.workspace_id))
        {
            d.home_anchor = state.anchor;
            for region in &mut d.regions {
                if let Some((_, origin)) = state.homes.iter().find(|(id, _)| *id == region.id) {
                    region.origin.y = origin.y;
                }
            }
            d.home = state.camera;
            if state.workspace
                == d.workspaces.iter().find(|ws| ws.is_active).map(|ws| ws.workspace_id)
                && d.workspaces.iter().any(|ws| ws.index == state.selection.workspace)
            {
                d.selection = state.selection;
                if !d.windows.iter().any(|w| Some(w.data.id) == d.selection.window) {
                    d.selection.window = None;
                }
            }
            d.home.position.x =
                d.info.frame.size.width / 2. - d.bounds.size.width / (2. * d.home.zoom);
            if state.workspace
                != d.workspaces.iter().find(|ws| ws.is_active).map(|ws| ws.workspace_id)
            {
                let origin = d
                    .workspaces
                    .iter()
                    .find(|ws| ws.is_active)
                    .map_or(CGPoint::ZERO, |ws| d.workspace_origin(ws.index));
                d.home.position.y =
                    origin.y + d.workspace_height / 2. - d.bounds.size.height / (2. * d.home.zoom);
            }
            for ws in &d.workspaces {
                let home = d.workspace_origin(ws.index);
                let origin = d.window_origin(ws);
                let old = state.windows.iter().find(|(_, id, _)| *id == ws.workspace_id);
                let shift = old.and_then(|(old, _, _)| {
                    ws.windows.iter().find(|w| w.id == old.id).map(|current| {
                        let a = old.layout_frame.unwrap_or(old.info.frame);
                        let b = current.layout_frame.unwrap_or(current.info.frame);
                        CGPoint::new(b.origin.x - a.origin.x, b.origin.y - a.origin.y)
                    })
                });
                let unchanged = shift.is_some_and(|shift| {
                    ws.windows.iter().all(|current| {
                        state
                            .windows
                            .iter()
                            .find(|(old, id, _)| old.id == current.id && *id == ws.workspace_id)
                            .is_none_or(|(old, _, _)| {
                                let a = old.layout_frame.unwrap_or(old.info.frame);
                                let b = current.layout_frame.unwrap_or(current.info.frame);
                                old.layout_position == current.layout_position
                                    && a.size == b.size
                                    && (b.origin.x - a.origin.x - shift.x).abs() < 1.
                                    && (b.origin.y - a.origin.y - shift.y).abs() < 1.
                            })
                    })
                });
                for w in d.windows.iter_mut().filter(|w| w.workspace == ws.workspace_id) {
                    let native = w.data.layout_frame.unwrap_or(w.data.info.frame);
                    let old = state.windows.iter().find(|(data, _, _)| data.id == w.data.id);
                    w.world = if unchanged && let Some((_, _, world)) = old {
                        let previous = state
                            .homes
                            .iter()
                            .find(|(id, _)| *id == ws.workspace_id)
                            .map_or(CGPoint::ZERO, |(_, p)| *p);
                        rect(
                            world.origin.x + home.x - previous.x,
                            world.origin.y,
                            world.size.width,
                            world.size.height,
                        )
                    } else {
                        rect(
                            native.origin.x + origin.x,
                            native.origin.y + origin.y,
                            native.size.width,
                            native.size.height,
                        )
                    };
                }
            }
            d.reconcile();
            // Every display starts aligned to its own native desktop, including negative origins.
            d.camera = d.desktop_camera();
        }
        Some(captures)
    }
}

fn start_display(
    d: &mut DisplayOverview,
    enabled: bool,
    wake: &crate::actor::mission_control::Sender,
    mut cache: Option<&mut PreviewCache>,
) {
    let tx = wake.clone();
    d.clock = Some(crate::sys::display_link::DisplayLink::for_display_callback(
        d.info.id.as_u32(),
        move || tx.send(crate::actor::mission_control::Event::PumpPreviews),
    ));
    if enabled {
        d.captures = cache.as_mut().and_then(|c| c.take(d));
        if !d.background_ready {
            let tx = wake.clone();
            d.desktop_snapshot =
                cgs::DesktopSnapshot::new(d.info.id.as_u32(), d.scale, move || {
                    tx.send(crate::actor::mission_control::Event::PumpPreviews)
                })
                .map_err(|error| tracing::warn!(?error, "Desktop snapshot unavailable"))
                .ok();
        }
        if let Some(m) = &mut d.captures {
            let _ = m.attach_to(d._window.id(), &d.root);
        } else {
            let tx = wake.clone();
            d.captures = cgs::PreviewCompositor::new_attached(
                d._window.id(),
                &d.root,
                d.bounds,
                d.scale,
                move || tx.send(crate::actor::mission_control::Event::PumpPreviews),
            )
            .map_err(|error| tracing::warn!(?error, "Spatial capture unavailable"))
            .ok();
        }
        // Bootstrap at the useful overview resolution. Capturing a full Retina
        // desktop first can exhaust admission before the entry animation starts.
        d.update_sources_at(d.selection.window, true, d.home);
        if let Some(m) = &mut d.captures {
            if let Err(error) = m.set_visual_scene(&[]).and_then(|()| {
                if d.background_ready || d.desktop_snapshot.is_none() {
                    m.show()
                } else {
                    Ok(())
                }
            }) {
                tracing::warn!(?error, "Spatial capture startup failed");
            }
            tracing::debug!(windows=d.windows.len(),stats=?m.stats(),"Spatial capture started");
        }
    }
    if d.captures.is_none() {
        d.camera = d.home;
        d.background.setOpacity(1.);
        d.apply_camera(1.);
    }
}

struct Drag {
    window: WindowId,
    display: usize,
    start: CGPoint,
    anchor: CGPoint,
    original: CGRect,
    moved: bool,
    pending: bool,
    destination: usize,
    frame: CGRect,
    canvas: Option<cgs::PreviewLayer>,
    badge: Retained<CALayer>,
    overlay: Retained<CALayer>,
    point: CGPoint,
    last_tick: Instant,
    indicator: Retained<CALayer>,
}
// Waiting keeps the real desktop visible until an actual preview has presented.
// Exiting keeps textures until the reactor has installed a visible, matching real window.
enum Phase {
    Waiting(Instant),
    Entering(Instant),
    Overview,
    Exiting {
        window: Option<WindowId>,
        start: Instant,
        target_ready: bool,
    },
}

struct ExitScene {
    target: OverviewTarget,
    geometry: Vec<(WindowId, CGRect, CGRect)>,
    outgoing: Vec<WindowId>,
    start: Instant,
    duration: f64,
}
pub struct OverviewSession {
    displays: Vec<DisplayOverview>,
    active: usize,
    selection: Selection,
    drag: Option<Drag>,
    pending_drop: Option<(Drag, Option<cgs::PreviewFrame>)>,
    pressed: Option<(CGPoint, Camera)>,
    drop: Option<OverviewDrop>,
    phase: Phase,
    exit: Option<ExitScene>,
    show_empty: bool,
    shared: bool,
    duration: f64,
    fade_duration: f64,
    wake: Option<crate::actor::mission_control::Sender>,
    previews: bool,
}
impl Drop for OverviewSession {
    fn drop(&mut self) {
        NSCursor::arrowCursor().set();
        let _tx = LayerTransaction::disabled().flush_on_commit();
        for d in &self.displays {
            release_layer_tree(&d.root);
        }
        self.displays.clear();
    }
}
impl OverviewSession {
    pub(crate) fn new(
        reactor: &ReactorHandle,
        mtm: MainThreadMarker,
        settings: &MissionControlSettings,
        shared: bool,
        _cache: &mut Option<PreviewCache>,
    ) -> Option<Self> {
        let _tx = LayerTransaction::disabled().flush_on_commit();
        let mut displays = Vec::new();
        let mut active = 0;
        for d in reactor.query_displays() {
            if (shared && !d.is_active_context) || d.info.space.is_none() || !d.is_visible_space {
                continue;
            }
            let mut workspaces = reactor.query_workspaces(if shared { None } else { d.info.space });
            if shared {
                for ws in &mut workspaces {
                    ws.is_active &= Some(ws.space) == d.info.space;
                }
            }
            if !settings.show_empty_workspaces {
                workspaces.retain(|w| !w.windows.is_empty());
            }
            if workspaces.is_empty() {
                continue;
            }
            if let Some(view) = DisplayOverview::new(d.info, workspaces, mtm) {
                if d.is_active_context {
                    active = displays.len();
                }
                displays.push(view);
            }
        }
        tracing::debug!(displays = displays.len(), "Spatial overview hosts created");
        let d = displays.get(active)?;
        let ws = d.workspaces.iter().find(|w| w.is_active).unwrap_or(&d.workspaces[0]);
        let selection = Selection {
            workspace: ws.index,
            window: ws.focused_window,
        };
        Some(Self {
            displays,
            active,
            selection,
            drag: None,
            pending_drop: None,
            pressed: None,
            drop: None,
            phase: Phase::Waiting(Instant::now()),
            exit: None,
            wake: None,
            previews: false,
            show_empty: settings.show_empty_workspaces,
            shared,
            duration: settings.fade_duration_ms.max(1.) / 1000.,
            fade_duration: if settings.fade_enabled {
                settings.fade_duration_ms.max(1.) / 1000.
            } else {
                0.
            },
        })
    }

    pub(crate) fn input_bounds(&self) -> Vec<CGRect> {
        self.displays.iter().map(|d| d.bounds).collect()
    }

    pub(crate) fn is_empty(&self) -> bool { self.displays.is_empty() }

    pub(crate) fn take_drop(&mut self) -> Option<OverviewDrop> { self.drop.take() }

    pub(crate) fn start_previews(
        &mut self,
        enabled: bool,
        wake: crate::actor::mission_control::Sender,
        mut cache: Option<&mut PreviewCache>,
    ) {
        let _tx = LayerTransaction::disabled().flush_on_commit();
        self.activate_display(self.active);
        self.wake = Some(wake.clone());
        self.previews = enabled;
        for d in &mut self.displays {
            start_display(d, enabled, &wake, cache.as_deref_mut());
        }
        self.selection = self.displays[self.active].selection;
        self.change_selection(self.selection);
        if self.displays.iter().all(|d| d.captures.is_none() || d.windows.is_empty()) {
            self.phase = Phase::Overview;
            for d in &mut self.displays {
                d.background.setOpacity(1.);
                d.apply_camera(1.);
            }
        }
    }

    pub(crate) fn retain_previews(&mut self, cache: &mut Option<PreviewCache>) {
        let cache = cache.get_or_insert_with(Default::default);
        cache.captures.clear();
        let mut reserved = 0;
        for d in &mut self.displays {
            if let Some(mut m) = d.captures.take() {
                tracing::debug!(display = %d.info.display_uuid, stats = ?m.stats(), diagnostics = ?m.diagnostics(), "Spatial overview closed");
                let _ = m.hide();
                let _ = m.set_visual_scene(&[]);
                let _ = m.set_sources(&[]);
                reserved +=
                    (d.bounds.size.width * d.scale * d.bounds.size.height * d.scale * 16.) as usize;
                if reserved <= 256 * 1024 * 1024 {
                    let camera = if matches!(self.phase, Phase::Overview) {
                        d.motion.as_ref().map_or(d.camera, |m| m.to)
                    } else {
                        d.home
                    };
                    let state = ViewState {
                        camera,
                        anchor: d.home_anchor,
                        workspace: d
                            .workspaces
                            .iter()
                            .find(|ws| ws.is_active)
                            .map(|ws| ws.workspace_id),
                        selection: d.selection,
                        homes: d.regions.iter().map(|r| (r.id, r.origin)).collect(),
                        windows: d
                            .windows
                            .iter()
                            .map(|w| (w.data.clone(), w.workspace, w.world))
                            .collect(),
                        wallpaper: unsafe { d.background.contents() },
                        wallpaper_key: d.wallpaper_key.clone(),
                    };
                    cache.captures.push((d.info.clone(), d.bounds, d.scale, m, state));
                }
            }
        }
    }

    pub(crate) fn refresh(
        &mut self,
        topology: &[RuntimeDisplayData],
        catalog: &[RuntimeWorkspaceData],
        moved: Option<WindowId>,
    ) {
        let active_uuid = self.displays.get(self.active).map(|d| d.info.display_uuid.clone());
        // Workspace activation changes the live model behind us. Keep the visible
        // scene stable until the handoff; topology changes must still tear down hosts.
        if matches!(self.phase, Phase::Exiting { .. })
            && self.displays.iter().all(|d| {
                topology.iter().any(|s| {
                    s.is_visible_space
                        && s.info.id == d.info.id
                        && s.info.space == d.info.space
                        && s.info.frame == d.info.frame
                        && s.info.backing_scale == d.scale
                })
            })
        {
            return;
        }
        let contexts = topology
            .iter()
            .filter(|s| {
                s.is_visible_space
                    && s.info.space.is_some()
                    && (!self.shared || Some(&s.info.display_uuid) == active_uuid.as_ref())
            })
            .count();
        if moved.is_none()
            && contexts == self.displays.len()
            && self.displays.iter().all(|d| {
                topology.iter().any(|s| {
                    s.is_visible_space
                        && s.info.id == d.info.id
                        && s.info.space == d.info.space
                        && s.info.frame == d.info.frame
                        && s.info.backing_scale == d.scale
                }) && {
                    let mut incoming = catalog.iter().filter(|w| {
                        (self.shared || Some(w.space) == d.info.space)
                            && (self.show_empty || !w.windows.is_empty())
                    });
                    d.workspaces.iter().all(|a| {
                        incoming
                            .next()
                            .is_some_and(|b| same_workspace_scene(a, b, self.shared, d.info.space))
                    }) && incoming.next().is_none()
                }
            })
        {
            return;
        }
        let shared = self.shared;
        let workspaces_for = |space| {
            catalog
                .iter()
                .filter(|w| shared || Some(w.space) == space)
                .cloned()
                .collect::<Vec<_>>()
        };
        let _tx = LayerTransaction::disabled().flush_on_commit();
        let completed =
            if self.pending_drop.as_ref().is_some_and(|(drag, _)| Some(drag.window) == moved) {
                self.pending_drop.take().map(|(drag, frame)| {
                    let id = drag.window;
                    self.end_drag(drag, false);
                    (id, frame)
                })
            } else {
                None
            };
        if let Some(d) = self.displays.get_mut(self.active) {
            d.selection = self.selection;
        }
        let same_displays = self.displays.iter().all(|d| {
            topology.iter().any(|s| {
                s.is_visible_space
                    && s.info.id == d.info.id
                    && s.info.space == d.info.space
                    && s.info.frame == d.info.frame
                    && s.info.backing_scale == d.scale
            })
        });
        if !same_displays {
            self.pressed = None;
            if let Some(drag) = self.drag.take() {
                self.end_drag(drag, true);
            }
            if let Some((drag, _)) = self.pending_drop.take() {
                self.end_drag(drag, true);
            }
        }
        let active_uuid = self.displays.get(self.active).map(|d| d.info.display_uuid.clone());
        self.displays.retain(|d| {
            topology.iter().any(|s| {
                s.is_visible_space
                    && s.info.id == d.info.id
                    && s.info.space == d.info.space
                    && s.info.frame == d.info.frame
                    && s.info.backing_scale == d.scale
            })
        });
        for native in topology {
            if (self.shared && Some(&native.info.display_uuid) != active_uuid.as_ref())
                || native.info.space.is_none()
                || !native.is_visible_space
                || self.displays.iter().any(|d| d.info.id == native.info.id)
            {
                continue;
            }
            let mut workspaces = workspaces_for(native.info.space);
            if self.shared {
                for ws in &mut workspaces {
                    ws.is_active &= Some(ws.space) == native.info.space;
                }
            }
            if !self.show_empty {
                workspaces.retain(|w| !w.windows.is_empty());
            }
            if workspaces.is_empty() {
                continue;
            }
            if let Some(mut d) = DisplayOverview::new(
                native.info.clone(),
                workspaces,
                MainThreadMarker::new().unwrap(),
            ) {
                if let Some(wake) = &self.wake {
                    start_display(&mut d, self.previews, wake, None);
                }
                d.camera = d.home;
                d.background.setOpacity(1.);
                d.apply_camera(1.);
                self.displays.push(d);
            }
        }
        self.active = self
            .displays
            .iter()
            .position(|d| Some(&d.info.display_uuid) == active_uuid.as_ref())
            .unwrap_or(0);
        self.selection = self.displays.get(self.active).map_or(self.selection, |d| d.selection);
        for d in &mut self.displays {
            d.workspaces = workspaces_for(d.info.space);
            if self.shared {
                for ws in &mut d.workspaces {
                    ws.is_active &= Some(ws.space) == d.info.space;
                }
            }
            if !self.show_empty {
                d.workspaces.retain(|w| !w.windows.is_empty());
            }
            let empty = d.windows.is_empty();
            if let Some(workspace) = moved
                .and_then(|id| d.workspaces.iter().find(|ws| ws.windows.iter().any(|w| w.id == id)))
            {
                let home = d.window_origin(workspace);
                for window in &workspace.windows {
                    if let Some(w) = d.windows.iter_mut().find(|w| w.data.id == window.id) {
                        let native = window.layout_frame.unwrap_or(window.info.frame);
                        w.world.origin =
                            CGPoint::new(native.origin.x + home.x, native.origin.y + home.y);
                    }
                }
            }
            d.reconcile();
            if let Some(w) = d.windows.iter().find(|w| Some(w.data.id) == d.selection.window) {
                d.selection.workspace = w.workspace_index;
            } else if !d.workspaces.iter().any(|ws| ws.index == d.selection.workspace) {
                if let Some(ws) =
                    d.workspaces.iter().find(|ws| ws.is_active).or(d.workspaces.first())
                {
                    d.selection = Selection {
                        workspace: ws.index,
                        window: ws.focused_window,
                    };
                }
            } else if !d.windows.iter().any(|w| Some(w.data.id) == d.selection.window) {
                d.selection.window = d
                    .workspaces
                    .iter()
                    .find(|ws| ws.index == d.selection.workspace)
                    .and_then(|ws| ws.focused_window);
            }
            if empty && !d.windows.is_empty() {
                d.home = d.default_camera();
                d.animate(d.home, self.duration);
            }
            d.apply_camera(
                if matches!(self.phase, Phase::Waiting(_) | Phase::Exiting { .. }) {
                    0.
                } else {
                    1.
                },
            );
            d.update_sources(d.selection.window, true);
        }
        if self.drag.as_ref().is_some_and(|drag| {
            !self.displays[drag.display].windows.iter().any(|w| w.data.id == drag.window)
                || self.displays.iter().any(|d| d.workspaces.is_empty())
        }) && let Some(drag) = self.drag.take()
        {
            self.end_drag(drag, true);
        }
        if let Some((id, Some(frame))) = completed {
            for d in &mut self.displays {
                if d.windows.iter().any(|w| w.data.id == id)
                    && let Some(m) = &mut d.captures
                {
                    m.reuse_frame(&frame);
                }
            }
        }
        self.displays.retain(|d| !d.workspaces.is_empty());
        self.active = self
            .displays
            .iter()
            .position(|d| Some(&d.info.display_uuid) == active_uuid.as_ref())
            .unwrap_or(0);
        let Some(d) = self.displays.get(self.active) else {
            return;
        };
        self.selection = d.selection;
        self.change_selection(self.selection);
        self.activate_display(self.active);
        if !self
            .displays
            .iter()
            .flat_map(|d| &d.windows)
            .any(|w| Some(w.data.id) == self.selection.window)
        {
            self.selection.window = None;
        }
    }

    pub(crate) fn needs_handoff(&self) -> bool {
        self.displays
            .get(self.active)
            .and_then(|d| d.captures.as_ref())
            .is_some_and(|m| m.is_visible() && m.stats().presented_frames > 0)
    }

    fn activate(&mut self, dismiss: bool) -> Option<OverviewAction> {
        let d = &mut self.displays[self.active];
        let chosen = if dismiss {
            d.windows.iter().find(|w| {
                w.data.is_focused
                    && d.workspaces.iter().any(|ws| ws.workspace_id == w.workspace && ws.is_active)
            })
        } else {
            self.selection.window.and_then(|id| d.windows.iter().find(|w| w.data.id == id))
        };
        let workspace = d.workspaces.iter().find(|ws| {
            if dismiss {
                ws.is_active
            } else {
                ws.index == self.selection.workspace
            }
        });
        let remote =
            workspace.is_some_and(|ws| ws.display_uuid.as_deref() != Some(&d.info.display_uuid));
        let target = if dismiss || remote {
            d.desktop_camera()
        } else {
            Camera {
                zoom: 1.,
                position: workspace.map_or(CGPoint::ZERO, |ws| d.workspace_origin(ws.index)),
            }
        };
        let action = if !dismiss {
            chosen
                .map(|w| OverviewAction::Activate {
                    display: d.info.display_uuid.clone(),
                    workspace: w.workspace,
                    selection: Selection {
                        workspace: w.workspace_index,
                        window: Some(w.data.id),
                    },
                })
                .or_else(|| {
                    workspace.map(|ws| OverviewAction::Activate {
                        display: d.info.display_uuid.clone(),
                        workspace: ws.workspace_id,
                        selection: Selection {
                            workspace: ws.index,
                            window: None,
                        },
                    })
                })
        } else {
            None
        };
        if !matches!(self.phase, Phase::Exiting { .. }) {
            d.home = d.motion.as_ref().map_or(d.camera, |m| m.to);
        }
        if !self.needs_handoff() {
            return action.or(Some(OverviewAction::Dismiss));
        }
        let window = if dismiss { None } else { self.selection.window };
        if let Some(w) =
            self.displays[self.active].windows.iter().find(|w| Some(w.data.id) == window)
        {
            // The selected preview must match the real window being raised at handoff.
            w.shadow.setZPosition(1.);
        }
        let now = Instant::now();
        for d in &mut self.displays {
            d.sample(now);
            d.motion = None;
        }
        self.phase = Phase::Exiting {
            window,
            start: now,
            target_ready: dismiss,
        };
        // Focus/AX work runs behind the animation, never ahead of its first frame.
        self.animate_exit(target);
        action
    }

    pub(crate) fn activation_stamp(&self) -> Option<Instant> {
        if let Phase::Exiting { start, .. } = self.phase {
            Some(start)
        } else {
            None
        }
    }

    pub(crate) fn activation_target(&mut self, prepared: OverviewTarget) {
        if !matches!(self.phase, Phase::Exiting { window, start, target_ready: false }
            if window == prepared.window && start == prepared.stamp)
        {
            return;
        }
        let _tx = LayerTransaction::disabled().flush_on_commit();
        let d = &self.displays[self.active];
        let target = d
            .windows
            .iter()
            .find(|w| Some(w.data.id) == prepared.window)
            .and_then(|w| {
                let ws = d.workspaces.iter().find(|ws| ws.workspace_id == w.workspace)?;
                let frame = prepared.frames.iter().find(|(id, _)| *id == w.data.id)?.1;
                (ws.display_uuid.as_deref() == Some(&d.info.display_uuid)).then(|| Camera {
                    zoom: 1.,
                    position: CGPoint::new(
                        w.world.origin.x - frame.origin.x + d.bounds.origin.x,
                        w.world.origin.y - frame.origin.y + d.bounds.origin.y,
                    ),
                })
            })
            .unwrap_or_else(|| d.motion.as_ref().map_or(d.camera, |m| m.to));
        if let Phase::Exiting { target_ready, window, .. } = &mut self.phase {
            *target_ready = true;
            if prepared.window.is_some_and(|id| !prepared.frames.iter().any(|(w, _)| *w == id)) {
                *window = None;
            }
        }
        let now = Instant::now();
        let remaining = (EXIT_MOTION - now.duration_since(prepared.stamp).as_secs_f64()).max(0.08);
        let d = &mut self.displays[self.active];
        if d.motion.as_ref().map_or(d.camera, |m| m.to) != target {
            // Correct from the currently displayed position, without restarting the whole exit.
            d.sample(now);
            d.animate(target, remaining);
            d.motion.as_mut().unwrap().curve = MotionCurve::Smooth;
            d.policy_dirty = true;
        }
        let local_workspace = d
            .workspaces
            .iter()
            .find(|ws| ws.workspace_id == prepared.workspace)
            .is_some_and(|ws| ws.display_uuid.as_deref() == Some(&d.info.display_uuid));
        let geometry = prepared
            .frames
            .iter()
            .filter(|_| local_workspace)
            .filter_map(|&(id, frame)| {
                let w = d.windows.iter().find(|w| w.data.id == id)?;
                let local = CGPoint::new(
                    frame.origin.x - d.bounds.origin.x,
                    frame.origin.y - d.bounds.origin.y,
                );
                let to = CGRect::new(target.unproject(local), frame.size);
                Some((id, w.world, to))
            })
            .collect();
        let outgoing = d
            .windows
            .iter()
            .filter(|w| {
                w.workspace != prepared.workspace
                    && d.workspaces.iter().any(|ws| {
                        ws.workspace_id == w.workspace
                            && ws.is_active
                            && ws.display_uuid.as_deref() == Some(&d.info.display_uuid)
                    })
            })
            .map(|w| w.data.id)
            .collect();
        self.exit = Some(ExitScene {
            target: prepared,
            geometry,
            outgoing,
            start: now,
            duration: remaining,
        });
    }

    fn animate_exit(&mut self, target: Camera) {
        let now = Instant::now();
        for (i, d) in self.displays.iter_mut().enumerate() {
            d.animate(
                if i == self.active {
                    target
                } else {
                    d.desktop_camera()
                },
                EXIT_MOTION,
            );
            let motion = d.motion.as_mut().unwrap();
            motion.start = now;
            motion.curve = MotionCurve::Smooth;
            d.policy_dirty = true;
        }
    }

    fn interrupt(&mut self) {
        if matches!(self.phase, Phase::Overview) && self.displays.iter().all(|d| d.motion.is_none())
        {
            return;
        }
        if matches!(self.phase, Phase::Exiting { .. }) {
            if let Some(wake) = &self.wake {
                wake.send(crate::actor::mission_control::Event::RefreshCurrentWorkspace);
            }
            for d in &self.displays {
                if let Some(w) = d.windows.iter().find(|w| Some(w.data.id) == self.selection.window)
                {
                    w.shadow.setZPosition(0.5);
                }
            }
        }
        let now = Instant::now();
        self.exit = None;
        for d in &mut self.displays {
            d.sample(now);
            d.motion = None;
            d.background.setOpacity(1.);
        }
        self.phase = Phase::Overview;
    }

    fn select_workspace(&mut self, index: usize) {
        let d = &mut self.displays[self.active];
        let ws = &d.workspaces[index];
        let selection = Selection {
            workspace: ws.index,
            window: ws.focused_window,
        };
        let mut target = d.camera;
        target.position.y = d.workspace_origin(ws.index).y + d.workspace_height / 2.
            - d.bounds.size.height / (2. * target.zoom);
        d.animate(target, self.duration);
        d.policy_dirty = true;
        self.change_selection(selection);
    }

    fn select(&mut self, index: usize) {
        let d = &mut self.displays[self.active];
        let w = &d.windows[index];
        let selection = Selection {
            workspace: w.workspace_index,
            window: Some(w.data.id),
        };
        let viewport = CGRect::new(CGPoint::ZERO, d.bounds.size);
        if !contains(viewport, d.camera.project(w.world.mid())) {
            let mut target = d.camera;
            target.position = CGPoint::new(
                w.world.mid().x - d.bounds.size.width / (2. * target.zoom),
                w.world.mid().y - d.bounds.size.height / (2. * target.zoom),
            );
            d.animate(target, self.duration);
        }
        d.policy_dirty = true;
        self.change_selection(selection);
    }

    // Zoom is one overview operation; workspace scrolling and ownership remain per display.
    fn zoom_view(&mut self, target: Camera, duration: Option<f64>) {
        let now = Instant::now();
        for (i, d) in self.displays.iter_mut().enumerate() {
            let mut camera = d.camera;
            if i == self.active {
                camera = target;
            } else {
                camera.zoom_at(
                    target.zoom / camera.zoom,
                    CGPoint::new(d.bounds.size.width / 2., d.bounds.size.height / 2.),
                );
            }
            if let Some(duration) = duration {
                d.animate(camera, duration);
                d.motion.as_mut().unwrap().start = now;
            } else {
                d.camera = camera;
            }
            d.policy_dirty = true;
            d.request_camera_frame();
        }
    }

    fn activate_display(&mut self, index: usize) {
        if self.active != index {
            self.displays[self.active].selection = self.selection;
            self.active = index;
            self.selection = self.displays[index].selection;
        }
        for (i, d) in self.displays.iter().enumerate() {
            d.controls.setHidden(i != self.active);
            d.update_focus_ring(d.applied.map_or(0., |(_, opacity)| opacity));
            if i != self.active {
                d.help.setHidden(true);
            }
        }
    }

    fn change_selection(&mut self, selection: Selection) {
        let previous = self.selection.window;
        self.selection = selection;
        self.displays[self.active].selection = selection;
        let selected = selection.window;
        for d in &mut self.displays {
            d.policy_dirty |= previous != selected;
            d.update_focus_ring(d.applied.map_or(0., |(_, opacity)| opacity));
        }
    }

    pub(crate) fn input(&mut self, input: Input) -> Option<OverviewAction> {
        if self.displays.is_empty() {
            return Some(OverviewAction::Dismiss);
        }
        if matches!(self.phase, Phase::Exiting { .. }) && matches!(input, Input::Move(_)) {
            return None;
        }
        let transaction = LayerTransaction::disabled();
        let _tx = if matches!(
            input,
            Input::Pan(_)
                | Input::BeginPan(_)
                | Input::EndPan(_)
                | Input::Scroll { .. }
                | Input::Zoom { .. }
                | Input::Magnify { .. }
        ) || matches!(input, Input::PointerDrag(_))
        {
            transaction
        } else {
            transaction.flush_on_commit()
        };
        if matches!(input, Input::Dismiss) {
            if let Some(drag) = self.drag.take() {
                self.end_drag(drag, true);
                return None;
            }
            return self.activate(true);
        }
        if matches!(
            input,
            Input::Zoom { .. } | Input::Magnify { .. } | Input::Scroll { .. }
        ) {
            self.pressed = None;
            if let Some(drag) = self.drag.take() {
                self.end_drag(drag, true);
            }
        }
        if matches!(input, Input::Activate) {
            return self.activate(false);
        }
        if !matches!(input, Input::Move(_)) {
            self.interrupt();
        }
        match input {
            Input::Scroll { point, delta } | Input::Zoom { point, delta } => {
                if let Some(i) = self.displays.iter().position(|d| contains(d.bounds, point)) {
                    self.activate_display(i);
                }
                let d = &mut self.displays[self.active];
                if matches!(input, Input::Zoom { .. }) {
                    let mut target = d.camera;
                    target.zoom_at((delta.y * 0.008).exp(), d.local(point));
                    self.zoom_view(target, None);
                } else {
                    d.camera.position.x -= delta.x / d.camera.zoom;
                    d.camera.position.y -= delta.y / d.camera.zoom;
                    d.policy_dirty = true;
                }
            }
            Input::ZoomStep(step) => {
                let d = &mut self.displays[self.active];
                let pivot = self
                    .selection
                    .window
                    .and_then(|id| d.windows.iter().find(|w| w.data.id == id))
                    .map(|w| d.camera.project(w.world.mid()))
                    .unwrap_or(CGPoint::new(d.bounds.size.width / 2., d.bounds.size.height / 2.));
                let mut target = d.camera;
                target.zoom_at(
                    if step == 0 {
                        1. / target.zoom
                    } else {
                        1.2_f64.powi(step as i32)
                    },
                    pivot,
                );
                self.zoom_view(target, Some(0.18));
            }
            Input::Magnify { point, factor } => {
                if let Some(i) = self.displays.iter().position(|d| contains(d.bounds, point)) {
                    self.activate_display(i);
                }
                let d = &mut self.displays[self.active];
                let mut target = d.camera;
                target.zoom_at(factor, d.local(point));
                self.zoom_view(target, None);
            }
            Input::Move(point) => {
                if self.drag.is_none()
                    && let Some(i) = self.displays.iter().position(|d| contains(d.bounds, point))
                {
                    self.activate_display(i);
                }
                for d in &mut self.displays {
                    let p = d.local(point);
                    let index = contains(d.controls.frame(), p).then(|| {
                        let x = p.x - d.controls.frame().origin.x;
                        if x < 36. {
                            0
                        } else if x < 82. {
                            1
                        } else if x < 112. {
                            2
                        } else if x < 162. {
                            3
                        } else if x < 204. {
                            4
                        } else if x < 237. {
                            5
                        } else {
                            6
                        }
                    });
                    if index != d.help_index {
                        if let Some(i) = index {
                            set_text(
                                &d.help,
                                [
                                    "Zoom out · ⌘− · Pinch to zoom",
                                    "Actual size · ⌘0",
                                    "Zoom in · ⌘+ · Pinch to zoom",
                                    "Fit workspace · ↑↓ switch workspace",
                                    "Fit all workspaces · Scroll to explore",
                                    "Open selected window · Return · ←→ select",
                                    "Return to desktop · Escape",
                                ][i],
                            );
                        }
                        d.help.setHidden(index.is_none());
                        d.help_index = index;
                    }
                }
                if self.drag.is_none() && self.pressed.is_none() {
                    let d = &self.displays[self.active];
                    let local = d.local(point);
                    if contains(d.bounds, point)
                        && !contains(d.controls.frame(), local)
                        && !d
                            .regions
                            .iter()
                            .any(|r| !r.label.isHidden() && contains(r.label.frame(), local))
                        && let Some(w) = d.hit(point).map(|i| &d.windows[i])
                        && Some(w.data.id) != self.selection.window
                    {
                        self.change_selection(Selection {
                            workspace: w.workspace_index,
                            window: Some(w.data.id),
                        });
                    }
                }
            }
            Input::PointerDown(point) => {
                if self.pending_drop.is_some() {
                    return None;
                }
                self.pressed = None;
                if let Some(i) = self.displays.iter().position(|d| contains(d.bounds, point)) {
                    self.activate_display(i);
                    let d = &self.displays[i];
                    if contains(d.controls.frame(), d.local(point))
                        || d.workspace_label_at(d.local(point)).is_some()
                    {
                        return None;
                    }
                    if let Some(j) = d.hit(point) {
                        let w = &d.windows[j];
                        self.drag = Some(Drag {
                            window: w.data.id,
                            display: i,
                            start: point,
                            anchor: d.camera.unproject(d.local(point)),
                            original: w.world,
                            moved: false,
                            pending: false,
                            destination: i,
                            frame: d.camera.frame(w.world),
                            canvas: None,
                            badge: window_badge(
                                w.data.app_name.as_deref().unwrap_or("Window"),
                                w.icon_image.as_deref(),
                                d.scale,
                            )
                            .0,
                            overlay: {
                                let l = layer(CGRect::new(CGPoint::ZERO, d.bounds.size), d.scale);
                                l.setZPosition(4.);
                                l
                            },
                            point,
                            last_tick: Instant::now(),
                            indicator: {
                                let l = layer(CGRect::ZERO, d.scale);
                                l.setZPosition(3.);
                                l.setBorderWidth(1.5);
                                l.setBorderColor(Some(&color(1., 1., 1., 0.55)));
                                l.setBackgroundColor(Some(&color(1., 1., 1., 0.05)));
                                l.setHidden(true);
                                l
                            },
                        });
                        self.change_selection(Selection {
                            workspace: w.workspace_index,
                            window: Some(w.data.id),
                        });
                    } else {
                        self.pressed = Some((point, d.camera));
                        NSCursor::closedHandCursor().set();
                    }
                }
            }
            Input::BeginPan(point) => {
                if let Some(drag) = self.drag.take() {
                    self.end_drag(drag, true);
                }
                if let Some(i) = self.displays.iter().position(|d| contains(d.bounds, point)) {
                    self.activate_display(i);
                }
                self.pressed = Some((point, self.displays[self.active].camera));
                NSCursor::closedHandCursor().set();
            }
            Input::EndPan(point) => {
                NSCursor::arrowCursor().set();
                self.pan_to(point);
                self.pressed = None;
            }
            Input::Pan(point) | Input::PointerDrag(point) => {
                if let Some(drag) = &mut self.drag {
                    drag.point = point;
                    drag.pending = true;
                    let display = self.displays.iter_mut().find(|d| contains(d.bounds, point));
                    if let Some(display) = display {
                        display.request_camera_frame();
                    }
                } else {
                    self.pan_to(point);
                }
            }
            Input::PointerUp(point) | Input::Click(point) => {
                if self.drag.as_ref().is_some_and(|drag| drag.pending) {
                    self.move_drag(point);
                }
                NSCursor::arrowCursor().set();
                if self.pressed.is_some() {
                    self.pan_to(point);
                    self.pressed = None;
                } else if let Some(drag) = self.drag.take().filter(|d| d.moved) {
                    self.drop = self.drop_target(&drag, point).map(|(intent, _)| intent);
                    if self.drop.is_some() {
                        let frame = self.displays[drag.display]
                            .windows
                            .iter()
                            .find(|w| w.data.id == drag.window)
                            .and_then(|w| {
                                self.displays[drag.display]
                                    .captures
                                    .as_ref()?
                                    .captured_frame(w.data.info.sys_id?.as_u32())
                            });
                        drag.indicator.removeFromSuperlayer();
                        self.pending_drop = Some((drag, frame));
                    } else {
                        self.end_drag(drag, true);
                    }
                } else {
                    let d = &mut self.displays[self.active];
                    let p = d.local(point);
                    if contains(d.controls.frame(), p) {
                        let x = p.x - d.controls.frame().origin.x;
                        let pivot =
                            CGPoint::new(d.bounds.size.width / 2., d.bounds.size.height / 2.);
                        let (target, duration) = if x < 112. {
                            let mut target = d.camera;
                            target.zoom_at(
                                if x < 36. {
                                    1. / 1.2
                                } else if x < 82. {
                                    1. / target.zoom
                                } else {
                                    1.2
                                },
                                pivot,
                            );
                            (target, 0.18)
                        } else if x < 162. {
                            let ws = d
                                .workspaces
                                .iter()
                                .find(|ws| ws.index == self.selection.workspace)
                                .unwrap_or(&d.workspaces[0]);
                            let frame = d
                                .windows
                                .iter()
                                .filter(|w| w.workspace == ws.workspace_id)
                                .map(|w| w.world)
                                .reduce(union)
                                .unwrap_or(CGRect::new(
                                    d.workspace_origin(ws.index),
                                    d.info.frame.size,
                                ));
                            (Camera::fit(frame, d.bounds.size), self.duration)
                        } else if x < 204. {
                            (d.fit(), self.duration)
                        } else if x < 237. {
                            return self.activate(false);
                        } else {
                            return self.activate(true);
                        };
                        self.zoom_view(target, Some(duration));
                    } else if let Some(index) = d
                        .workspace_label_at(p)
                        .and_then(|id| d.workspaces.iter().position(|w| w.workspace_id == id))
                    {
                        self.select_workspace(index);
                    } else if let Some(i) = d.hit(point) {
                        self.select(i);
                        return self.activate(false);
                    }
                }
            }
            Input::Up | Input::Down => {
                let d = &mut self.displays[self.active];
                let current = d
                    .workspaces
                    .iter()
                    .position(|ws| ws.index == self.selection.workspace)
                    .unwrap_or(0);
                let next = if matches!(input, Input::Up) {
                    current.saturating_sub(1)
                } else {
                    (current + 1).min(d.workspaces.len() - 1)
                };
                self.select_workspace(next);
            }
            Input::Left | Input::Right | Input::Cycle(_) => {
                let d = &self.displays[self.active];
                let ws = d.workspaces.iter().find(|ws| ws.index == self.selection.workspace)?;
                if ws.windows.is_empty() {
                    return None;
                }
                let current = ws.windows.iter().position(|w| Some(w.id) == self.selection.window);
                let forward = matches!(input, Input::Right | Input::Cycle(true));
                let next = match current {
                    Some(i) if forward => (i + 1) % ws.windows.len(),
                    Some(i) => (i + ws.windows.len() - 1) % ws.windows.len(),
                    None => 0,
                };
                if let Some(i) = d.windows.iter().position(|w| w.data.id == ws.windows[next].id) {
                    self.select(i);
                }
            }
            _ => {}
        }
        if matches!(input, Input::Scroll { .. } | Input::Pan(_))
            || (matches!(input, Input::PointerDrag(_)) && self.drag.is_none())
        {
            let d = &self.displays[self.active];
            let center = d
                .camera
                .unproject(CGPoint::new(d.bounds.size.width / 2., d.bounds.size.height / 2.));
            if let Some(ws) = d.workspaces.iter().min_by(|a, b| {
                let distance = |ws: &RuntimeWorkspaceData| {
                    (d.workspace_origin(ws.index).y + d.workspace_height / 2. - center.y).abs()
                };
                distance(a).total_cmp(&distance(b))
            }) && ws.index != self.selection.workspace
            {
                self.change_selection(Selection {
                    workspace: ws.index,
                    window: ws.focused_window,
                });
            }
        }
        if !matches!(input, Input::Move(_) | Input::PointerDown(_))
            && !(matches!(input, Input::PointerDrag(_)) && self.drag.is_some())
        {
            let d = &mut self.displays[self.active];
            d.request_camera_frame();
        }
        None
    }

    fn pan_to(&mut self, point: CGPoint) {
        if let Some((start, camera)) = self.pressed {
            let d = &mut self.displays[self.active];
            d.camera = Camera {
                zoom: camera.zoom,
                position: CGPoint::new(
                    camera.position.x - (point.x - start.x) / camera.zoom,
                    camera.position.y - (point.y - start.y) / camera.zoom,
                ),
            };
            d.policy_dirty = true;
        }
    }

    fn move_drag(&mut self, point: CGPoint) {
        let Some(mut drag) = self.drag.take() else {
            return;
        };
        drag.pending = false;
        if !drag.moved && (point.x - drag.start.x).hypot(point.y - drag.start.y) < 4. {
            self.drag = Some(drag);
            return;
        }
        drag.point = point;
        let destination = self
            .displays
            .iter()
            .position(|d| contains(d.bounds, point))
            .unwrap_or(drag.destination);
        let d = &self.displays[destination];
        let zoom = d.camera.zoom;
        let frame = rect(
            point.x - d.bounds.origin.x - (drag.anchor.x - drag.original.origin.x) * zoom,
            point.y - d.bounds.origin.y - (drag.anchor.y - drag.original.origin.y) * zoom,
            drag.original.size.width * zoom,
            drag.original.size.height * zoom,
        );
        if !drag.moved || destination != drag.destination {
            NSCursor::closedHandCursor().set();
            drag.canvas = None;
            drag.overlay.setFrame(CGRect::new(CGPoint::ZERO, d.bounds.size));
            d.root.addSublayer(&drag.overlay);
            let mut canvas = cgs::PreviewLayer::new();
            if let Some(owner) = &self.displays[drag.display].captures
                && let Some(w) =
                    self.displays[drag.display].windows.iter().find(|w| w.data.id == drag.window)
                && let Some(frame) = owner.captured_frame(w.data.info.sys_id.unwrap().as_u32())
            {
                canvas.set_frame(&frame);
            }
            canvas.layer().setMasksToBounds(true);
            canvas.layer().setCornerRadius(10. * zoom);
            canvas.layer().setBorderWidth(2.5);
            canvas.layer().setBorderColor(Some(&NSColor::controlAccentColor().CGColor()));
            canvas.layer().setFrame(frame);
            drag.overlay.addSublayer(canvas.layer());
            drag.canvas = Some(canvas);
            drag.overlay.addSublayer(&drag.badge);
            let source = &mut self.displays[drag.display];
            source.dragged = Some(drag.window);
            source.invalidate_scene();
            source.apply_camera(1.);
        } else if let Some(canvas) = &drag.canvas {
            canvas.layer().setFrame(frame);
        }
        if let Some((intent, marker)) = self.drop_target(&drag, point) {
            self.displays[destination].root.addSublayer(&drag.indicator);
            drag.indicator.setFrame(marker);
            drag.indicator.setHidden(false);
            if let Some(region) =
                self.displays[destination].regions.iter().find(|r| r.id == intent.workspace)
            {
                region.label.setOpacity(1.);
            }
        } else {
            drag.indicator.setHidden(true);
        }
        self.displays[destination].request_camera_frame();
        drag.moved = true;
        drag.destination = destination;
        drag.frame = frame;
        drag.badge.setPosition(CGPoint::new(frame.origin.x, frame.origin.y - 30.));
        drag.badge.setHidden(false);
        self.drag = Some(drag);
    }

    fn end_drag(&mut self, mut drag: Drag, _cancel: bool) {
        NSCursor::arrowCursor().set();
        drag.indicator.removeFromSuperlayer();
        drag.overlay.removeFromSuperlayer();
        drag.canvas = None;
        if let Some(d) = self.displays.get_mut(drag.display) {
            d.dragged = None;
            d.reconcile();
            d.apply_camera(1.);
        }
    }

    pub(crate) fn toggle(&mut self) {
        if matches!(self.phase, Phase::Exiting { .. }) {
            self.interrupt();
            for d in &mut self.displays {
                d.animate(d.home, self.duration);
                d.motion.as_mut().unwrap().curve = MotionCurve::Enter;
            }
            self.phase = Phase::Entering(Instant::now());
        } else {
            self.activate(true);
        }
    }

    fn drop_target(&self, drag: &Drag, point: CGPoint) -> Option<(OverviewDrop, CGRect)> {
        let Some(destination) = self.displays.iter().position(|d| contains(d.bounds, point)) else {
            return None;
        };
        let d = &self.displays[destination];
        let world = d.camera.unproject(d.local(point));
        let ws = d
            .workspace_label_at(d.local(point))
            .and_then(|id| d.workspaces.iter().find(|w| w.workspace_id == id))
            .or_else(|| {
                d.workspaces.iter().min_by(|a, b| {
                    let distance = |ws: &RuntimeWorkspaceData| {
                        let p = d.workspace_origin(ws.index);
                        (world.y - p.y - d.workspace_height / 2.).abs()
                    };
                    distance(a).total_cmp(&distance(b))
                })
            });
        let Some(ws) = ws else {
            return None;
        };
        let source = &self.displays[drag.display];
        let Some(w) = source.windows.iter().find(|w| w.data.id == drag.window) else {
            return None;
        };
        let region = d.regions.iter().find(|r| r.id == ws.workspace_id)?;
        let target = crate::actor::drag::hit_test_projected(&region.drops, drag.window, world);
        let action = target.map(|t| (t.window, t.action));
        let marker = target.map_or(
            rect(
                d.camera.position.x,
                region.origin.y,
                d.bounds.size.width / d.camera.zoom,
                d.info.frame.size.height,
            ),
            |t| {
                let mut frame = t.frame;
                let line = 2. / d.camera.zoom;
                match t.action {
                    crate::layout_engine::WindowDropAction::Insert(
                        crate::layout_engine::Direction::Left,
                    ) => frame.size.width = line,
                    crate::layout_engine::WindowDropAction::Insert(
                        crate::layout_engine::Direction::Right,
                    ) => {
                        frame.origin.x += frame.size.width - line;
                        frame.size.width = line;
                    }
                    crate::layout_engine::WindowDropAction::Insert(
                        crate::layout_engine::Direction::Up,
                    ) => frame.size.height = line,
                    crate::layout_engine::WindowDropAction::Insert(
                        crate::layout_engine::Direction::Down,
                    ) => {
                        frame.origin.y += frame.size.height - line;
                        frame.size.height = line;
                    }
                    _ => {}
                }
                frame
            },
        );
        Some((
            OverviewDrop {
                window: drag.window,
                workspace: ws.workspace_id,
                target: if w.data.is_floating { None } else { action },
                frame: w.data.is_floating.then(|| {
                    rect(
                        world.x - d.window_origin(ws).x - (drag.anchor.x - drag.original.origin.x),
                        world.y - d.window_origin(ws).y - (drag.anchor.y - drag.original.origin.y),
                        drag.original.size.width,
                        drag.original.size.height,
                    )
                }),
            },
            d.camera.frame(marker),
        ))
    }

    pub(crate) fn pump_previews(&mut self) {
        let _tx = LayerTransaction::disabled().flush_on_commit();
        let now = Instant::now();
        if let Phase::Waiting(start) = self.phase {
            let ready = self.displays.iter().all(|d| {
                d.windows.is_empty()
                    || d.captures.as_ref().is_none_or(|_| {
                        d.windows.iter().any(|w| {
                            intersects(
                                d.camera.frame(w.world),
                                CGRect::new(CGPoint::ZERO, d.bounds.size),
                            ) && w.preview.has_frame()
                        })
                    })
            });
            if (ready && self.displays.iter().all(|d| d.background_ready))
                || now.duration_since(start) > Duration::from_secs(2)
            {
                self.phase = Phase::Entering(now);
                for d in &mut self.displays {
                    d.animate(d.home, self.duration);
                    let motion = d.motion.as_mut().unwrap();
                    motion.start = now;
                    motion.curve = MotionCurve::Enter;
                    d.background.setOpacity(1.);
                }
            }
        }
        let decorations = match self.phase {
            Phase::Waiting(_) => 0.,
            Phase::Entering(start) if self.fade_duration > 0. => {
                (now.duration_since(start).as_secs_f64() / self.fade_duration).min(1.) as f32
            }
            Phase::Exiting { start, .. } => {
                1. - (now.duration_since(start).as_secs_f64()
                    / self.fade_duration.min(0.12).max(0.001))
                .min(1.) as f32
            }
            _ => 1.,
        };
        let drag_frame =
            self.displays.iter().any(|d| d.clock.as_ref().is_some_and(|c| c.has_update()));
        if drag_frame && let Some(point) = self.drag.as_ref().filter(|d| d.pending).map(|d| d.point)
        {
            self.move_drag(point);
        }
        if drag_frame
            && let Some(drag) = &mut self.drag
            && drag.moved
        {
            let d = &mut self.displays[drag.destination];
            let y = d.local(drag.point).y;
            let edge = 64.;
            let velocity = if !contains(d.bounds, drag.point) {
                0.
            } else if y < edge {
                -((edge - y) / edge).clamp(0., 1.)
            } else {
                ((y - d.bounds.size.height + edge) / edge).clamp(0., 1.)
            };
            let elapsed = now.duration_since(drag.last_tick).as_secs_f64().min(0.05);
            drag.last_tick = now;
            if velocity != 0. {
                d.camera.position.y += velocity * 700. * elapsed / d.camera.zoom;
                d.policy_dirty = true;
                d.apply_camera(decorations);
                if let Some(clock) = &d.clock {
                    clock.set_paused(false);
                }
                let point = drag.point;
                self.move_drag(point);
            }
        }
        for drag in self.drag.iter_mut().chain(self.pending_drop.iter_mut().map(|(d, _)| d)) {
            if let Some(canvas) = &mut drag.canvas
                && let Some(owner) = &self.displays[drag.display].captures
                && let Some(w) =
                    self.displays[drag.display].windows.iter().find(|w| w.data.id == drag.window)
                && let Some(frame) = owner.captured_frame(w.data.info.sys_id.unwrap().as_u32())
            {
                canvas.set_frame(&frame);
            }
        }
        for (display, d) in self.displays.iter_mut().enumerate() {
            if let Some(snapshot) = &d.desktop_snapshot {
                match snapshot.take_image() {
                    Ok(Some(image)) => {
                        image_contents(&d.background, &image);
                        d.background_ready = true;
                        d.desktop_snapshot = None;
                    }
                    Err(error) => {
                        tracing::warn!(?error, "Desktop background unavailable");
                        d.desktop_snapshot = None;
                    }
                    _ => {}
                }
            }
            let tick = d.clock.as_ref().and_then(|clock| clock.latest());
            if let Some((_, target)) = tick {
                d.sample(target.max(now));
            }
            if display == self.active
                && tick.is_some()
                && let Some(exit) = &self.exit
            {
                let sample = tick.map_or(now, |(_, target)| target.max(now));
                let t = (sample.duration_since(exit.start).as_secs_f64() / exit.duration).min(1.);
                let t = t * t * (3. - 2. * t);
                for &(id, from, to) in &exit.geometry {
                    if let Some(w) = d.windows.iter_mut().find(|w| w.data.id == id) {
                        let mix = |a: f64, b: f64| a + (b - a) * t;
                        w.world = rect(
                            mix(from.origin.x, to.origin.x),
                            mix(from.origin.y, to.origin.y),
                            mix(from.size.width, to.size.width),
                            mix(from.size.height, to.size.height),
                        );
                        w.apply_geometry();
                    }
                }
            }
            if tick.is_some() || d.applied.is_none() {
                d.apply_camera(decorations);
            }
            if !matches!(self.phase, Phase::Exiting { .. }) {
                d.update_sources(d.selection.window, false);
            }
            if let Some(m) = &mut d.captures {
                if !m.is_visible() && (d.background_ready || d.desktop_snapshot.is_none()) {
                    let _ = m.show();
                }
                if let Err(e) = m.render(now) {
                    tracing::warn!(?e, "Spatial capture update failed");
                }
                for w in &mut d.windows {
                    if !matches!(self.phase, Phase::Exiting { .. })
                        && let Some(frame) = m.captured_frame(w.data.info.sys_id.unwrap().as_u32())
                    {
                        if w.preview.set_frame(&frame) {
                            w.shadow.setOpacity(1.);
                        }
                    }
                }
            }
            if let Some(clock) = &d.clock {
                clock.set_paused(
                    d.motion.is_none()
                        && !matches!(self.phase, Phase::Waiting(_) | Phase::Exiting { .. })
                        && self.pressed.is_none()
                        && d.camera_input.elapsed() > Duration::from_millis(100)
                        && !self.drag.as_ref().is_some_and(|drag| {
                            drag.moved && contains(d.bounds, drag.point) && {
                                let y = d.local(drag.point).y;
                                y < 64. || y > d.bounds.size.height - 64.
                            }
                        }),
                );
            }
        }
        if let Some(drag) = &self.drag
            && drag.moved
        {
            drag.badge
                .setPosition(CGPoint::new(drag.frame.origin.x, drag.frame.origin.y - 30.));
            drag.badge.setHidden(false);
        }
        if matches!(self.phase, Phase::Entering(_))
            && self.displays.iter().all(|d| d.motion.is_none())
        {
            self.phase = Phase::Overview;
        }
    }

    pub(crate) fn handoff_ready(&mut self) -> bool {
        let Phase::Exiting { window, start, target_ready } = self.phase else {
            return false;
        };
        if start.elapsed() > Duration::from_secs(2) {
            return true;
        }
        if !target_ready || self.displays.iter().any(|d| d.motion.is_some()) {
            return false;
        }
        let d = &self.displays[self.active];
        if let Some(exit) = &self.exit {
            if exit.start.elapsed().as_secs_f64() < exit.duration {
                return false;
            }
            // A focused window alone is insufficient: its neighbours may still be
            // moving out of parking positions on other app threads.
            for &(id, frame) in &exit.target.frames {
                let visible = d
                    .workspaces
                    .iter()
                    .find(|ws| ws.workspace_id == exit.target.workspace)
                    .is_some_and(|ws| intersects(frame, ws.display_frame));
                if !visible {
                    continue;
                }
                let Some(w) = d.windows.iter().find(|w| w.data.id == id) else {
                    continue;
                };
                let Some(sys_id) = w.data.info.sys_id else {
                    return false;
                };
                let Some(native) = crate::sys::window_server::get_window(sys_id) else {
                    continue;
                };
                if native.pid != id.pid {
                    continue;
                }
                if !crate::sys::window_server::window_is_ordered_in(sys_id)
                    || (native.frame.origin.x - frame.origin.x).abs() > 1.
                    || (native.frame.origin.y - frame.origin.y).abs() > 1.
                    || (native.frame.size.width - frame.size.width).abs() > 1.
                    || (native.frame.size.height - frame.size.height).abs() > 1.
                {
                    return false;
                }
            }
            for &id in &exit.outgoing {
                let native = d
                    .windows
                    .iter()
                    .find(|w| w.data.id == id)
                    .and_then(|w| w.data.info.sys_id)
                    .filter(|&id| crate::sys::window_server::window_is_ordered_in(id))
                    .and_then(crate::sys::window_server::get_window);
                if let Some(native) = native.filter(|w| w.pid == id.pid) {
                    let width = native.frame.max().x.min(d.bounds.max().x)
                        - native.frame.min().x.max(d.bounds.min().x);
                    let height = native.frame.max().y.min(d.bounds.max().y)
                        - native.frame.min().y.max(d.bounds.min().y);
                    if width > 1. && height > 1. {
                        return false;
                    }
                }
            }
        }
        let aligned = window.is_none_or(|id| {
            let Some(w) = d.windows.iter().find(|w| w.data.id == id) else {
                return true;
            };
            let Some(sys_id) = w.data.info.sys_id else {
                return false;
            };
            let Some(native) = crate::sys::window_server::get_window(sys_id) else {
                return false;
            };
            if native.pid != id.pid || !crate::sys::window_server::window_is_ordered_in(sys_id) {
                return false;
            }
            let Some(ws) = d.workspaces.iter().find(|ws| ws.workspace_id == w.workspace) else {
                return true;
            };
            if ws.display_uuid.as_deref() != Some(&d.info.display_uuid) {
                return intersects(native.frame, ws.display_frame);
            }
            let preview = d.camera.frame(w.world);
            let tolerance = 1.;
            (preview.origin.x + d.bounds.origin.x - native.frame.origin.x).abs() <= tolerance
                && (preview.origin.y + d.bounds.origin.y - native.frame.origin.y).abs() <= tolerance
                && (preview.size.width - native.frame.size.width).abs() <= tolerance
                && (preview.size.height - native.frame.size.height).abs() <= tolerance
        });
        aligned
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn zoom_keeps_pointer_world_position_and_picking_consistent() {
        let mut c = Camera {
            position: CGPoint::new(-100., 50.),
            zoom: 0.5,
        };
        c.zoom_at(2., CGPoint::new(200., 100.));
        assert_eq!(c, Camera {
            position: CGPoint::new(100., 150.),
            zoom: 1.
        });
        assert_eq!(c.unproject(CGPoint::new(200., 100.)), CGPoint::new(300., 250.));
        assert!(contains(
            rect(280., 230., 40., 40.),
            c.unproject(CGPoint::new(200., 100.))
        ));
        let motion = Motion {
            from: Camera {
                position: CGPoint::new(-100., 50.),
                zoom: 0.5,
            },
            to: c,
            start: Instant::now(),
            duration: 0.3,
            curve: MotionCurve::Settle,
        };
        for ms in [25, 125, 250] {
            let camera = motion.sample(motion.start + Duration::from_millis(ms)).0;
            let point = camera.project(CGPoint::new(300., 250.));
            assert!(
                (point.x - 200.).abs() < 1e-9 && (point.y - 100.).abs() < 1e-9,
                "animated zoom moved its pivot: {point:?}"
            );
        }
        c.zoom_at(0.5, CGPoint::new(200., 100.));
        assert_eq!(c, Camera {
            position: CGPoint::new(-100., 50.),
            zoom: 0.5
        });
    }
    #[test]
    fn camera_fit_preserves_world_aspect_and_leaves_controls_clear() {
        let c = Camera::fit(rect(-600., 200., 2400., 1600.), CGSize::new(1200., 900.));
        let r = c.frame(rect(-600., 200., 2400., 1600.));
        assert!(r.min().x >= 55.999 && r.max().x <= 1144.001);
        assert!(r.min().y >= 47.999 && r.max().y <= 820.001);
        assert!((r.size.width / r.size.height - 1.5).abs() < 1e-9);
    }
    #[test]
    fn interrupted_motion_starts_from_current_camera_without_overshoot() {
        let start = Instant::now();
        let first = Motion {
            from: Camera {
                position: CGPoint::ZERO,
                zoom: 1.,
            },
            to: Camera {
                position: CGPoint::new(800., 400.),
                zoom: 0.2,
            },
            start,
            duration: 0.3,
            curve: MotionCurve::Enter,
        };
        let quarter = first.sample(start + Duration::from_millis(75)).0;
        let progress = (quarter.zoom - first.from.zoom) / (first.to.zoom - first.from.zoom);
        assert!(
            (0.4..0.65).contains(&progress),
            "entry must ease out without an initial lurch"
        );
        let (middle, done) = first.sample(start + Duration::from_millis(120));
        assert!(!done && middle.zoom > 0.2 && middle.zoom < 1.);
        let reverse = Motion {
            from: middle,
            to: first.from,
            start: start + Duration::from_millis(120),
            duration: 0.3,
            curve: MotionCurve::Smooth,
        };
        assert_eq!(reverse.sample(reverse.start).0, middle);
        let early = reverse.sample(reverse.start + Duration::from_millis(75)).0;
        assert!(
            (early.zoom - middle.zoom) / (first.from.zoom - middle.zoom) < 0.25,
            "focus zoom should accelerate rather than jump most of the way on its first frames"
        );
        let (end, done) = reverse.sample(reverse.start + Duration::from_millis(300));
        assert!(done);
        assert_eq!(end, first.from);
    }
}
