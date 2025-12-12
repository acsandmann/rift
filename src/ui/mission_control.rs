use core::ffi::c_void;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crossbeam_channel::{Sender, unbounded};
use dispatchr::queue;
use dispatchr::time::Time;
use objc2::msg_send;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSApplication, NSColor, NSPopUpMenuWindowLevel, NSScreen};
use objc2_core_foundation::{CFRetained, CFString, CFType, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGColor, CGContext, CGDisplayBounds, CGEvent, CGEventField, CGEventFlags, CGEventTapOptions,
    CGEventTapProxy, CGEventType,
};
use objc2_foundation::MainThreadMarker;
use objc2_quartz_core::{CALayer, CATextLayer, CATransaction};
use once_cell::sync::Lazy;
use parking_lot::{Mutex, RwLock};
use tracing::info;

use crate::actor::app::WindowId;
use crate::common::collections::{HashMap, HashSet, hash_map};
use crate::common::config::Config;
use crate::model::server::{WindowData, WorkspaceData};
use crate::model::virtual_workspace::VirtualWorkspaceId;
use crate::sys::cgs_window::CgsWindow;
use crate::sys::dispatch::DispatchExt;
use crate::sys::event::current_cursor_location;
use crate::sys::geometry::CGRectExt;
use crate::sys::screen::{CoordinateConverter, NSScreenExt, ScreenCache, ScreenId};
use crate::sys::skylight::{
    CFRelease, G_CONNECTION, SLSFlushWindowContentRegion, SLWindowContextCreate,
};
use crate::sys::window_server::{CapturedWindowImage, WindowServerId};

unsafe extern "C" {
    fn CGContextFlush(ctx: *mut CGContext);
    fn CGContextClearRect(ctx: *mut CGContext, rect: CGRect);
    fn CGContextSaveGState(ctx: *mut CGContext);
    fn CGContextRestoreGState(ctx: *mut CGContext);
    fn CGContextTranslateCTM(ctx: *mut CGContext, tx: f64, ty: f64);
    fn CGContextScaleCTM(ctx: *mut CGContext, sx: f64, sy: f64);
}

#[derive(Debug, Clone)]
struct CaptureTask {
    window_id: WindowId,
    window_server_id: u32,
    target_w: usize,
    target_h: usize,
}

struct CaptureJob {
    task: CaptureTask,
    cache: Arc<RwLock<HashMap<WindowId, CapturedWindowImage>>>,
    generation: u64,
    overlay_ptr_bits: usize,
}

struct CapturePool {
    sender: Sender<CaptureJob>,
}

static CURRENT_GENERATION: AtomicU64 = AtomicU64::new(1);
static IN_FLIGHT: Lazy<Mutex<HashSet<(u64, WindowId)>>> =
    Lazy::new(|| Mutex::new(HashSet::default()));

static CAPTURE_POOL: Lazy<CapturePool> = Lazy::new(|| {
    use std::thread;
    let (tx, rx) = unbounded::<CaptureJob>();

    let mut worker_count = std::thread::available_parallelism()
        .map(|n| n.get().saturating_sub(1))
        .unwrap_or(2);
    worker_count = worker_count.max(2).min(6);
    for _ in 0..worker_count {
        let rx = rx.clone();
        thread::spawn(move || {
            while let Ok(job) = rx.recv() {
                if job.generation != CURRENT_GENERATION.load(Ordering::Acquire) {
                    if let Some(mut set) = IN_FLIGHT.try_lock() {
                        set.remove(&(job.generation, job.task.window_id));
                    } else {
                        // best-effort; skip if contended
                    }
                    continue;
                }

                if let Some(img) = crate::sys::window_server::capture_window_image(
                    WindowServerId::new(job.task.window_server_id),
                    job.task.target_w,
                    job.task.target_h,
                ) {
                    {
                        let mut cache_lock = job.cache.write();
                        cache_lock.insert(job.task.window_id, img);
                    }
                    if let Some(mut set) = IN_FLIGHT.try_lock() {
                        set.remove(&(job.generation, job.task.window_id));
                    }
                    if let Some(overlay) =
                        unsafe { (job.overlay_ptr_bits as *const MissionControlOverlay).as_ref() }
                    {
                        overlay.request_refresh();
                    }
                } else {
                    if let Some(mut set) = IN_FLIGHT.try_lock() {
                        set.remove(&(job.generation, job.task.window_id));
                    }
                }
            }
        });
    }

    CapturePool { sender: tx }
});

extern "C" fn refresh_coalesced_cb(ctx: *mut c_void) {
    if ctx.is_null() {
        return;
    }
    let overlay = unsafe { &*(ctx as *const MissionControlOverlay) };
    overlay.refresh_pending.store(false, Ordering::Release);
    overlay.refresh_previews();
}

struct FadeCompletionCtx {
    overlay_ptr_bits: usize,
    fade_id: u64,
    final_alpha: f32,
}

extern "C" fn fade_completion_callback(ctx: *mut c_void) {
    if ctx.is_null() {
        return;
    }
    unsafe {
        let boxed = Box::from_raw(ctx as *mut FadeCompletionCtx);
        if boxed.overlay_ptr_bits == 0 {
            return;
        }
        if let Some(overlay) = (boxed.overlay_ptr_bits as *const MissionControlOverlay).as_ref() {
            overlay.finish_fade(boxed.fade_id, boxed.final_alpha);
        }
    }
}

fn schedule_fade_completion(overlay_ptr_bits: usize, fade_id: u64, final_alpha: f32) {
    if overlay_ptr_bits == 0 {
        return;
    }
    let ctx = Box::into_raw(Box::new(FadeCompletionCtx {
        overlay_ptr_bits,
        fade_id,
        final_alpha,
    })) as *mut c_void;
    queue::main().after_f(Time::NOW, ctx, fade_completion_callback);
}

static WORKSPACE_BACKGROUND_COLOR: Lazy<Retained<CGColor>> =
    Lazy::new(|| CGColor::new_generic_gray(1.0, 0.03).into());

static SELECTED_BORDER_COLOR: Lazy<Retained<CGColor>> =
    Lazy::new(|| CGColor::new_generic_rgb(0.2, 0.45, 1.0, 0.85).into());

static WORKSPACE_BORDER_COLOR: Lazy<Retained<CGColor>> =
    Lazy::new(|| CGColor::new_generic_gray(1.0, 0.12).into());

static WINDOW_BORDER_COLOR: Lazy<Retained<CGColor>> =
    Lazy::new(|| CGColor::new_generic_gray(0.0, 0.65).into());

static OVERLAY_BACKGROUND_COLOR: Lazy<Retained<CGColor>> =
    Lazy::new(|| CGColor::new_generic_gray(0.0, 0.25).into());

#[derive(Debug, Clone)]
pub enum MissionControlMode {
    AllWorkspaces(Vec<WorkspaceData>),
    CurrentWorkspace(Vec<WindowData>),
}

#[derive(Debug, Clone)]
pub enum MissionControlAction {
    SwitchToWorkspace(usize),
    FocusWindow {
        window_id: WindowId,
        window_server_id: Option<WindowServerId>,
    },
    Dismiss,
}

struct WorkspaceLabelText {
    text: String,
    attributed: CFRetained<CFString>,
}

impl WorkspaceLabelText {
    fn new(text: &str) -> Self {
        let cf_string = CFString::from_str(text);
        Self {
            text: text.to_owned(),
            attributed: cf_string,
        }
    }

    fn update(&mut self, text: &str) -> bool {
        if self.text == text {
            return false;
        }

        self.text.clear();
        self.text.push_str(text);
        self.attributed = CFString::from_str(text);
        true
    }

    unsafe fn apply_to(&self, layer: &CATextLayer) {
        let raw = self.attributed.as_ref() as *const AnyObject;
        unsafe {
            layer.setString(Some(&*raw));
        }
    }
}

#[derive(Default)]
struct PreviewLayerStyle {
    is_selected: Option<bool>,
}

impl PreviewLayerStyle {
    fn update_selected(&mut self, selected: bool) -> bool {
        if self.is_selected == Some(selected) {
            false
        } else {
            self.is_selected = Some(selected);
            true
        }
    }
}

pub struct MissionControlState {
    mode: Option<MissionControlMode>,
    on_action: Option<Rc<dyn Fn(MissionControlAction)>>,
    selection: Option<Selection>,
    preview_cache: Arc<RwLock<HashMap<WindowId, CapturedWindowImage>>>,
    preview_layers: HashMap<WindowId, Retained<CALayer>>,
    preview_layer_styles: HashMap<WindowId, PreviewLayerStyle>,
    workspace_layers: HashMap<String, Retained<CALayer>>,
    workspace_label_layers: HashMap<String, Retained<CATextLayer>>,
    workspace_label_strings: HashMap<String, WorkspaceLabelText>,
    ready_previews: HashSet<WindowId>,
    render_root: Option<Retained<CALayer>>,
    render_window_id: Option<u32>,
    render_size: Option<CGSize>,
    // This lets us avoid visible pop-in and reveal once a threshold is met.
    suppress_live_present: bool,
}

impl Default for MissionControlState {
    fn default() -> Self {
        Self {
            mode: None,
            on_action: None,
            selection: None,
            preview_cache: Arc::new(RwLock::new(HashMap::default())),
            preview_layers: HashMap::default(),
            preview_layer_styles: HashMap::default(),
            workspace_layers: HashMap::default(),
            workspace_label_layers: HashMap::default(),
            workspace_label_strings: HashMap::default(),
            ready_previews: HashSet::default(),
            render_root: None,
            render_window_id: None,
            render_size: None,
            suppress_live_present: false,
        }
    }
}

impl MissionControlState {
    fn set_mode(&mut self, mode: MissionControlMode) {
        self.mode = Some(mode);
        self.selection = None;
        let _new_gen = CURRENT_GENERATION.fetch_add(1, Ordering::AcqRel) + 1;
        self.ready_previews.clear();
        self.prune_preview_cache();
        self.ensure_selection();
    }

    fn mode(&self) -> Option<&MissionControlMode> { self.mode.as_ref() }

    fn purge(&mut self) {
        self.mode = None;
        self.selection = None;
        self.on_action = None;

        let _new_gen = CURRENT_GENERATION.fetch_add(1, Ordering::AcqRel) + 1;

        let mut cache = self.preview_cache.write();
        cache.clear();
        cache.shrink_to_fit();
        self.ready_previews.clear();

        for (_id, layer) in self.preview_layers.drain() {
            layer.removeFromSuperlayer();
        }
        self.preview_layer_styles.clear();
        for (_id, layer) in self.workspace_layers.drain() {
            layer.removeFromSuperlayer();
        }
        for (_id, layer) in self.workspace_label_layers.drain() {
            layer.removeFromSuperlayer();
        }
        self.workspace_label_strings.clear();

        self.render_root = None;
        self.render_window_id = None;
        self.render_size = None;
    }

    fn selection(&self) -> Option<Selection> { self.selection }

    fn set_selection(&mut self, selection: Selection) {
        let is_valid = match (selection, self.mode.as_ref()) {
            (Selection::Workspace(_), Some(MissionControlMode::AllWorkspaces(_)))
            | (Selection::Window(_), Some(MissionControlMode::CurrentWorkspace(_))) => true,
            _ => false,
        };
        if is_valid {
            self.selection = Some(selection);
        }
    }

    fn highlight_active_workspace(&mut self, active_id: Option<String>) -> bool {
        let target = active_id.as_deref();
        if let Some(mode) = self.mode.as_mut() {
            if let MissionControlMode::AllWorkspaces(workspaces) = mode {
                let mut changed = false;
                let mut visible_index = 0usize;
                let mut active_selection = None;
                for ws in workspaces.iter_mut() {
                    let should_be_active = target == Some(ws.id.as_str());
                    if ws.is_active != should_be_active {
                        ws.is_active = should_be_active;
                        changed = true;
                    }
                    let should_be_visible = !ws.windows.is_empty() || ws.is_active;
                    if should_be_visible {
                        if ws.is_active {
                            active_selection = Some(visible_index);
                        }
                        visible_index += 1;
                    }
                }
                if let Some(idx) = active_selection {
                    if self.selection() != Some(Selection::Workspace(idx)) {
                        self.selection = Some(Selection::Workspace(idx));
                        changed = true;
                    }
                }
                changed
            } else {
                false
            }
        } else {
            false
        }
    }

    fn ensure_selection(&mut self) {
        if self.selection.is_some() {
            return;
        }
        match self.mode.as_ref() {
            Some(MissionControlMode::AllWorkspaces(workspaces)) => {
                let mut visible_idx = 0usize;
                let mut desired = None;
                for ws in workspaces {
                    if !ws.windows.is_empty() || ws.is_active {
                        if desired.is_none() && ws.is_active {
                            desired = Some(Selection::Workspace(visible_idx));
                        }
                        visible_idx += 1;
                    }
                }
                if let Some(sel) = desired {
                    self.selection = Some(sel);
                } else if visible_idx > 0 {
                    self.selection = Some(Selection::Workspace(0));
                }
            }
            Some(MissionControlMode::CurrentWorkspace(windows)) => {
                if let Some((idx, _)) = windows.iter().enumerate().find(|(_, win)| win.is_focused) {
                    self.selection = Some(Selection::Window(idx));
                } else if !windows.is_empty() {
                    self.selection = Some(Selection::Window(0));
                }
            }
            None => {}
        }
    }

    fn selected_workspace(&self) -> Option<usize> {
        match self.selection {
            Some(Selection::Workspace(idx)) => Some(idx),
            _ => None,
        }
    }

    fn selected_window(&self) -> Option<usize> {
        match self.selection {
            Some(Selection::Window(idx)) => Some(idx),
            _ => None,
        }
    }

    fn prune_preview_cache(&mut self) {
        let mut cache = self.preview_cache.write();

        if cache.is_empty() {
            return;
        }

        let mut valid: HashSet<WindowId> = HashSet::default();
        if let Some(mode) = self.mode.as_ref() {
            match mode {
                MissionControlMode::AllWorkspaces(workspaces) => {
                    for ws in workspaces {
                        for window in &ws.windows {
                            valid.insert(window.id);
                        }
                    }
                }
                MissionControlMode::CurrentWorkspace(windows) => {
                    for window in windows {
                        valid.insert(window.id);
                    }
                }
            }
        }

        cache.retain(|window_id, _| valid.contains(window_id));

        let mut remove_keys = Vec::new();
        for (&wid, layer) in self.preview_layers.iter() {
            if !valid.contains(&wid) {
                layer.removeFromSuperlayer();
                remove_keys.push(wid);
            }
        }
        for k in remove_keys {
            self.preview_layers.remove(&k);
            self.preview_layer_styles.remove(&k);
        }

        self.ready_previews.retain(|wid| valid.contains(wid));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Selection {
    Workspace(usize),
    Window(usize),
}

#[derive(Clone, Copy)]
enum NavDirection {
    Left,
    Right,
    Up,
    Down,
}

fn workspace_column_count(count: usize) -> usize {
    if count == 0 {
        1
    } else {
        ((count + 1) / 2).max(1)
    }
}

const MISSION_CONTROL_MARGIN: f64 = 48.0;
const WINDOW_TILE_INSET: f64 = 3.0;
const WINDOW_TILE_GAP: f64 = 1.0;
const WINDOW_TILE_MIN_SIZE: f64 = 2.0;
const WINDOW_TILE_SCALE_FACTOR: f64 = 0.75;
const WINDOW_TILE_MAX_SCALE: f64 = 1.0;
const SMALL_TILE_MIN_FRACTION: f64 = 0.44;
const INNER_RELAX_FACTOR: f64 = 0.94;
const WORKSPACE_TILE_SPACING: f64 = 20.0;
const CURRENT_WS_TILE_SPACING: f64 = 48.0;
const CURRENT_WS_TILE_PADDING: f64 = 16.0;
const CURRENT_WS_TILE_SCALE_FACTOR: f64 = 0.9;
const SYNC_PREWARM_LIMIT: usize = 3;

struct WorkspaceGrid {
    bounds: CGRect,
    rows: usize,
    tile_size: CGSize,
}

impl WorkspaceGrid {
    fn new(tile_count: usize, bounds: CGRect) -> Option<Self> {
        if tile_count == 0 {
            return None;
        }
        let cols = workspace_column_count(tile_count);
        let rows = if tile_count > cols { 2 } else { 1 };
        let spacing = WORKSPACE_TILE_SPACING;
        let tile_w = (bounds.size.width - spacing * ((cols + 1) as f64)) / (cols as f64);
        let tile_h = (bounds.size.height - spacing * ((rows + 1) as f64)) / (rows as f64);
        Some(Self {
            bounds,
            rows,
            tile_size: CGSize::new(tile_w, tile_h),
        })
    }

    fn position_for(&self, order_idx: usize) -> (usize, usize) {
        if self.rows == 1 {
            (0, order_idx)
        } else {
            (order_idx % self.rows, order_idx / self.rows)
        }
    }

    fn rect_for(&self, order_idx: usize) -> CGRect {
        let (row, col) = self.position_for(order_idx);
        let spacing = WORKSPACE_TILE_SPACING;
        let x = self.bounds.origin.x + spacing + (self.tile_size.width + spacing) * (col as f64);
        let y = self.bounds.origin.y + spacing + (self.tile_size.height + spacing) * (row as f64);
        CGRect::new(CGPoint::new(x, y), self.tile_size)
    }
}

struct WindowLayoutMetrics {
    scale: f64,
    x_offset: f64,
    y_offset: f64,
    min_x: f64,
    min_y: f64,
    disp_h: f64,
}

#[derive(Clone, Copy)]
enum WindowLayoutKind {
    PreserveOriginal,
    Exploded,
}

impl WindowLayoutMetrics {
    fn rect_for(&self, window: &WindowData) -> CGRect {
        let wx = window.frame.origin.x - self.min_x;
        let wy_top = window.frame.origin.y - self.min_y + window.frame.size.height;
        let wy = self.disp_h - wy_top;
        let ww = window.frame.size.width;
        let wh = window.frame.size.height;

        let mut rx = self.x_offset + wx * self.scale;
        let mut ry = self.y_offset + wy * self.scale;
        let mut rw = (ww * self.scale).max(WINDOW_TILE_MIN_SIZE);
        let mut rh = (wh * self.scale).max(WINDOW_TILE_MIN_SIZE);

        if rw > (WINDOW_TILE_MIN_SIZE + WINDOW_TILE_GAP) {
            rx += WINDOW_TILE_GAP / 2.0;
            rw -= WINDOW_TILE_GAP;
        }
        if rh > (WINDOW_TILE_MIN_SIZE + WINDOW_TILE_GAP) {
            ry += WINDOW_TILE_GAP / 2.0;
            rh -= WINDOW_TILE_GAP;
        }

        CGRect::new(CGPoint::new(rx, ry), CGSize::new(rw, rh))
    }
}

struct FadeState {
    id: u64,
}

#[derive(Clone, Copy)]
struct ScreenMetrics {
    id: Option<ScreenId>,
    frame: CGRect,
    scale: f64,
    converter: CoordinateConverter,
}

impl MissionControlOverlay {
    fn gather_screen_metrics(&self) -> Option<(Vec<ScreenMetrics>, CoordinateConverter)> {
        let mut cache = ScreenCache::new(self.mtm);
        let Some((_descriptors, converter, _spaces)) = cache.refresh() else {
            return None;
        };

        let screens = NSScreen::screens(self.mtm);
        let mut metrics = Vec::new();
        for screen in screens.iter() {
            if let Ok(screen_id) = screen.get_number() {
                let frame = CGDisplayBounds(screen_id.as_u32());
                metrics.push(ScreenMetrics {
                    id: Some(screen_id),
                    frame,
                    scale: screen.backingScaleFactor(),
                    converter,
                });
            }
        }

        if metrics.is_empty() {
            None
        } else {
            Some((metrics, converter))
        }
    }

    fn screen_under_cursor_with(&self, metrics: &[ScreenMetrics]) -> Option<ScreenMetrics> {
        if let Ok(loc) = current_cursor_location() {
            return metrics.iter().find(|m| m.frame.contains(loc)).copied();
        }

        None
    }

    fn main_screen_metric(&self, metrics: &[ScreenMetrics]) -> Option<ScreenMetrics> {
        let screen = NSScreen::mainScreen(self.mtm)?;
        let screen_id = screen.get_number().ok()?;
        metrics.iter().find(|m| m.id == Some(screen_id)).copied()
    }

    fn rect_contains_point(rect: CGRect, point: CGPoint) -> bool {
        point.x >= rect.origin.x
            && point.x <= rect.origin.x + rect.size.width
            && point.y >= rect.origin.y
            && point.y <= rect.origin.y + rect.size.height
    }

    fn content_bounds(bounds: CGRect) -> CGRect {
        let width = (bounds.size.width - 2.0 * MISSION_CONTROL_MARGIN).max(0.0);
        let height = (bounds.size.height - 2.0 * MISSION_CONTROL_MARGIN).max(0.0);
        CGRect::new(
            CGPoint::new(
                bounds.origin.x + MISSION_CONTROL_MARGIN,
                bounds.origin.y + MISSION_CONTROL_MARGIN,
            ),
            CGSize::new(width, height),
        )
    }

    fn workspace_index_at_point(
        workspaces: &[WorkspaceData],
        point: CGPoint,
        bounds: CGRect,
    ) -> Option<(usize, usize)> {
        if !Self::rect_contains_point(bounds, point) {
            return None;
        }
        let visible = Self::visible_workspaces(workspaces);
        let grid = WorkspaceGrid::new(visible.len(), bounds)?;
        for (order_idx, (original_idx, _)) in visible.iter().enumerate() {
            let rect = grid.rect_for(order_idx);
            if Self::rect_contains_point(rect, point) {
                return Some((order_idx, *original_idx));
            }
        }
        None
    }

    fn window_at_point(
        windows: &[WindowData],
        point: CGPoint,
        bounds: CGRect,
        layout: WindowLayoutKind,
    ) -> Option<(usize, WindowId)> {
        if !Self::rect_contains_point(bounds, point) {
            return None;
        }
        let rects = Self::compute_window_rects(windows, bounds, layout)?;

        for idx in (0..windows.len()).rev() {
            let window = &windows[idx];
            let rect = rects[idx];
            if Self::rect_contains_point(rect, point) {
                return Some((idx, window.id));
            }
        }
        None
    }

    fn compute_window_layout(
        windows: &[WindowData],
        bounds: CGRect,
    ) -> Option<WindowLayoutMetrics> {
        if windows.is_empty() {
            return None;
        }

        let mut min_x = f64::INFINITY;
        let mut min_y = f64::INFINITY;
        let mut max_x = f64::NEG_INFINITY;
        let mut max_y = f64::NEG_INFINITY;

        for w in windows {
            let x0 = w.frame.origin.x;
            let y0 = w.frame.origin.y;
            let x1 = x0 + w.frame.size.width;
            let y1 = y0 + w.frame.size.height;
            if x0 < min_x {
                min_x = x0;
            }
            if y0 < min_y {
                min_y = y0;
            }
            if x1 > max_x {
                max_x = x1;
            }
            if y1 > max_y {
                max_y = y1;
            }
        }

        let disp_w = (max_x - min_x).max(1.0);
        let disp_h = (max_y - min_y).max(1.0);

        let cx = bounds.origin.x + WINDOW_TILE_INSET;
        let cy = bounds.origin.y + WINDOW_TILE_INSET;
        let cw = (bounds.size.width - 2.0 * WINDOW_TILE_INSET).max(1.0);
        let ch = (bounds.size.height - 2.0 * WINDOW_TILE_INSET).max(1.0);

        let scale =
            (cw / disp_w).min(ch / disp_h).min(WINDOW_TILE_MAX_SCALE) * WINDOW_TILE_SCALE_FACTOR;
        let x_offset = cx + (cw - disp_w * scale) / 2.0;
        let y_offset = cy + (ch - disp_h * scale) / 2.0;

        Some(WindowLayoutMetrics {
            scale,
            x_offset,
            y_offset,
            min_x,
            min_y,
            disp_h,
        })
    }

    fn compute_exploded_layout(windows: &[WindowData], bounds: CGRect) -> Option<Vec<CGRect>> {
        if windows.is_empty() {
            return None;
        }

        let spacing = CURRENT_WS_TILE_SPACING;
        let padding = CURRENT_WS_TILE_PADDING;
        let target_aspect = (bounds.size.width.max(1.0)) / (bounds.size.height.max(1.0));

        let mut best_layout: Option<(usize, usize, f64)> = None;
        for cols in 1..=windows.len() {
            let rows = (windows.len() + cols - 1) / cols;
            let total_spacing_x = spacing * ((cols + 1) as f64);
            let total_spacing_y = spacing * ((rows + 1) as f64);
            let cell_w =
                (bounds.size.width - total_spacing_x).max(WINDOW_TILE_MIN_SIZE) / cols as f64;
            let cell_h =
                (bounds.size.height - total_spacing_y).max(WINDOW_TILE_MIN_SIZE) / rows as f64;
            if cell_w <= 0.0 || cell_h <= 0.0 {
                continue;
            }

            let cell_aspect = cell_w / cell_h;
            let usage = ((cell_w * cell_h * windows.len() as f64)
                / ((bounds.size.width * bounds.size.height).max(1.0)))
            .clamp(0.0, 1.0);
            let empty_penalty = (rows * cols - windows.len()) as f64 * 0.02;
            let score = (cell_aspect - target_aspect).abs() * 0.7
                + (1.0 - usage) * 1.0
                + empty_penalty * 1.2;

            match best_layout {
                Some((_, _, best_score)) if score >= best_score => {}
                _ => best_layout = Some((rows, cols, score)),
            }
        }

        let (rows, cols) = best_layout.map(|(r, c, _)| (r, c)).unwrap_or((1, windows.len()));

        let total_spacing_x = spacing * ((cols + 1) as f64);
        let total_spacing_y = spacing * ((rows + 1) as f64);
        let cell_w = (bounds.size.width - total_spacing_x).max(WINDOW_TILE_MIN_SIZE) / cols as f64;
        let cell_h = (bounds.size.height - total_spacing_y).max(WINDOW_TILE_MIN_SIZE) / rows as f64;

        let inner_w = (cell_w - 2.0 * padding).max(WINDOW_TILE_MIN_SIZE);
        let inner_h = (cell_h - 2.0 * padding).max(WINDOW_TILE_MIN_SIZE);
        let relaxed_w = inner_w * INNER_RELAX_FACTOR;
        let relaxed_h = inner_h * INNER_RELAX_FACTOR;
        let remainder = windows.len() % cols;

        let mut ordered: Vec<(usize, &WindowData)> = windows.iter().enumerate().collect();
        ordered.sort_by(|(ai, a), (bi, b)| {
            use std::cmp::Ordering;
            let top_a = a.frame.origin.y + a.frame.size.height;
            let top_b = b.frame.origin.y + b.frame.size.height;
            top_b
                .partial_cmp(&top_a)
                .unwrap_or(Ordering::Equal)
                .then_with(|| {
                    a.frame.origin.x.partial_cmp(&b.frame.origin.x).unwrap_or(Ordering::Equal)
                })
                .then_with(|| ai.cmp(bi))
        });

        let mut rects =
            vec![CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(0.0, 0.0)); windows.len()];

        for (order_idx, (original_idx, window)) in ordered.into_iter().enumerate() {
            let row = order_idx / cols;
            let col_in_row = order_idx % cols;
            let row_window_count = if row == rows - 1 && remainder != 0 {
                remainder
            } else {
                cols
            };
            let row_offset = if row_window_count < cols {
                ((cols - row_window_count) as f64) * (cell_w + spacing) / 2.0
            } else {
                0.0
            };

            let cell_origin_x =
                bounds.origin.x + spacing + row_offset + (cell_w + spacing) * (col_in_row as f64);
            let cell_origin_y = bounds.origin.y + spacing + (cell_h + spacing) * (row as f64);

            let original_w = window.frame.size.width.max(1.0);
            let original_h = window.frame.size.height.max(1.0);
            let mut scale =
                (relaxed_w / original_w).min(relaxed_h / original_h).min(WINDOW_TILE_MAX_SCALE);
            if scale > 0.5 {
                scale *= CURRENT_WS_TILE_SCALE_FACTOR;
            }
            let min_scale_w = (inner_w * SMALL_TILE_MIN_FRACTION) / original_w;
            let min_scale_h = (inner_h * SMALL_TILE_MIN_FRACTION) / original_h;
            let min_scale = min_scale_w.max(min_scale_h);
            scale = scale.max(min_scale).min(WINDOW_TILE_MAX_SCALE);

            let scaled_w = (original_w * scale).clamp(WINDOW_TILE_MIN_SIZE, inner_w);
            let scaled_h = (original_h * scale).clamp(WINDOW_TILE_MIN_SIZE, inner_h);
            let origin_x = cell_origin_x + (cell_w - scaled_w) / 2.0;
            let origin_y = cell_origin_y + (cell_h - scaled_h) / 2.0;

            rects[original_idx] =
                CGRect::new(CGPoint::new(origin_x, origin_y), CGSize::new(scaled_w, scaled_h));
        }

        Some(rects)
    }

    fn compute_window_rects(
        windows: &[WindowData],
        bounds: CGRect,
        kind: WindowLayoutKind,
    ) -> Option<Vec<CGRect>> {
        match kind {
            WindowLayoutKind::PreserveOriginal => {
                let layout = Self::compute_window_layout(windows, bounds)?;
                Some(windows.iter().map(|w| layout.rect_for(w)).collect())
            }
            WindowLayoutKind::Exploded => Self::compute_exploded_layout(windows, bounds),
        }
    }

    fn navigate_workspaces(
        visible: &[(usize, &WorkspaceData)],
        current: usize,
        direction: NavDirection,
    ) -> Option<usize> {
        if visible.is_empty() {
            return None;
        }
        let len = visible.len();
        let mut idx = current.min(len.saturating_sub(1));
        let cols = workspace_column_count(len);
        let rows = if len > cols { 2 } else { 1 };

        if rows == 1 {
            match direction {
                NavDirection::Left | NavDirection::Up => {
                    idx = (idx + len - 1) % len;
                }
                NavDirection::Right | NavDirection::Down => {
                    idx = (idx + 1) % len;
                }
            }
            return Some(idx);
        }

        let row = idx % rows;
        let col = idx / rows;

        match direction {
            NavDirection::Left | NavDirection::Right => {
                let delta: isize = if matches!(direction, NavDirection::Right) {
                    1
                } else {
                    -1
                };
                let cols_isize = cols as isize;
                let mut new_col = col as isize;
                for _ in 0..cols {
                    new_col = (new_col + delta + cols_isize) % cols_isize;
                    let candidate = new_col as usize * rows + row;
                    if candidate < len {
                        return Some(candidate);
                    }
                }
                Some(idx)
            }
            NavDirection::Up => {
                if row == 1 {
                    Some(col * rows)
                } else {
                    let candidate = col * rows + 1;
                    if candidate < len {
                        Some(candidate)
                    } else {
                        Self::nearest_bottom_index(len, rows, col).or(Some(idx))
                    }
                }
            }
            NavDirection::Down => {
                if row == 0 {
                    let candidate = col * rows + 1;
                    if candidate < len {
                        Some(candidate)
                    } else {
                        Self::nearest_bottom_index(len, rows, col).or(Some(idx))
                    }
                } else {
                    Some(col * rows)
                }
            }
        }
    }

    fn navigate_windows(count: usize, current: usize, direction: NavDirection) -> Option<usize> {
        if count == 0 {
            return None;
        }
        let len = count;
        let mut idx = current.min(len.saturating_sub(1));
        match direction {
            NavDirection::Left | NavDirection::Up => {
                idx = (idx + len - 1) % len;
            }
            NavDirection::Right | NavDirection::Down => {
                idx = (idx + 1) % len;
            }
        }
        Some(idx)
    }

    fn nearest_bottom_index(len: usize, rows: usize, target_col: usize) -> Option<usize> {
        if rows < 2 {
            return None;
        }

        let mut best: Option<(usize, usize)> = None;
        for idx in 0..len {
            if idx % rows == 1 {
                let col = idx / rows;
                let delta = target_col.abs_diff(col);
                match best {
                    Some((best_delta, _)) if delta >= best_delta => continue,
                    _ => best = Some((delta, idx)),
                }
            }
        }
        best.map(|(_, idx)| idx)
    }

    fn adjust_selection(&self, direction: NavDirection) -> bool {
        let mut state = match self.state.try_borrow_mut() {
            Ok(state) => state,
            Err(_) => return false,
        };
        state.ensure_selection();
        let current = state.selection();

        let new_selection = match (state.mode(), current) {
            (
                Some(MissionControlMode::AllWorkspaces(workspaces)),
                Some(Selection::Workspace(idx)),
            ) => {
                let visible = Self::visible_workspaces(workspaces);
                if visible.is_empty() {
                    None
                } else {
                    let idx = idx.min(visible.len().saturating_sub(1));
                    Self::navigate_workspaces(&visible, idx, direction).map(Selection::Workspace)
                }
            }
            (Some(MissionControlMode::CurrentWorkspace(windows)), Some(Selection::Window(idx))) => {
                if windows.is_empty() {
                    None
                } else {
                    let idx = idx.min(windows.len().saturating_sub(1));
                    Self::navigate_windows(windows.len(), idx, direction).map(Selection::Window)
                }
            }
            (Some(MissionControlMode::AllWorkspaces(workspaces)), None) => {
                if Self::visible_workspaces(workspaces).is_empty() {
                    None
                } else {
                    Some(Selection::Workspace(0))
                }
            }
            (Some(MissionControlMode::CurrentWorkspace(windows)), None) => {
                if windows.is_empty() {
                    None
                } else {
                    Some(Selection::Window(0))
                }
            }
            _ => None,
        };

        if let Some(selection) = new_selection {
            if state.selection() != Some(selection) {
                state.set_selection(selection);
                return true;
            }
        }
        false
    }

    fn cycle_selection(&self, forward: bool) -> bool {
        let mut state = match self.state.try_borrow_mut() {
            Ok(state) => state,
            Err(_) => return false,
        };
        state.ensure_selection();
        let current = state.selection();
        let mode = state.mode();

        let new_selection = match (mode, current) {
            (
                Some(MissionControlMode::AllWorkspaces(workspaces)),
                Some(Selection::Workspace(idx)),
            ) => {
                let visible = Self::visible_workspaces(workspaces);
                if visible.is_empty() {
                    None
                } else {
                    let len = visible.len();
                    let idx = idx.min(len.saturating_sub(1));
                    Self::next_workspace_index(idx, len, forward).map(Selection::Workspace)
                }
            }
            (Some(MissionControlMode::CurrentWorkspace(windows)), Some(Selection::Window(idx))) => {
                if windows.is_empty() {
                    None
                } else {
                    let len = windows.len();
                    let idx = idx.min(len.saturating_sub(1));
                    let new_idx = if forward {
                        (idx + 1) % len
                    } else {
                        (idx + len - 1) % len
                    };
                    Some(Selection::Window(new_idx))
                }
            }
            (Some(MissionControlMode::AllWorkspaces(workspaces)), None) => {
                let visible = Self::visible_workspaces(workspaces);
                if visible.is_empty() {
                    None
                } else {
                    let len = visible.len();
                    let idx = if forward { 0 } else { len.saturating_sub(1) };
                    Some(Selection::Workspace(idx))
                }
            }
            (Some(MissionControlMode::CurrentWorkspace(windows)), None) => {
                if windows.is_empty() {
                    None
                } else {
                    let len = windows.len();
                    let idx = if forward { 0 } else { len - 1 };
                    Some(Selection::Window(idx))
                }
            }
            _ => None,
        };

        if let Some(selection) = new_selection {
            if state.selection() != Some(selection) {
                state.set_selection(selection);
                return true;
            }
        }
        false
    }

    fn next_workspace_index(current_idx: usize, len: usize, forward: bool) -> Option<usize> {
        if len == 0 {
            return None;
        }
        let columns = workspace_column_count(len);
        let rows = if len > columns { 2 } else { 1 };

        let mut order: Vec<usize> = (0..len).collect();
        order.sort_by_key(|&order_idx| {
            let (row, col) = Self::workspace_grid_position(order_idx, rows);
            (row, col)
        });

        let current_pos = order.iter().position(|&idx| idx == current_idx)?;
        let next_pos = if forward {
            (current_pos + 1) % len
        } else {
            (current_pos + len - 1) % len
        };
        order.get(next_pos).copied()
    }

    fn workspace_grid_position(order_idx: usize, rows: usize) -> (usize, usize) {
        if rows == 1 {
            (0, order_idx)
        } else {
            (order_idx % rows, order_idx / rows)
        }
    }

    fn activate_selection_action(&self) {
        let action = {
            let mut state = self.state.borrow_mut();
            state.ensure_selection();
            let mode = state.mode();
            let selection = state.selection();

            let action = match (mode, selection) {
                (
                    Some(MissionControlMode::AllWorkspaces(workspaces)),
                    Some(Selection::Workspace(idx)),
                ) => {
                    let visible = Self::visible_workspaces(workspaces);
                    if visible.is_empty() {
                        None
                    } else {
                        let idx = idx.min(visible.len().saturating_sub(1));
                        visible.get(idx).map(|(original_idx, _)| {
                            MissionControlAction::SwitchToWorkspace(*original_idx)
                        })
                    }
                }
                (
                    Some(MissionControlMode::CurrentWorkspace(windows)),
                    Some(Selection::Window(idx)),
                ) => {
                    if windows.is_empty() {
                        None
                    } else {
                        let idx = idx.min(windows.len().saturating_sub(1));
                        windows.get(idx).map(|window| {
                            let window_server_id = window.window_server_id.map(WindowServerId::new);
                            MissionControlAction::FocusWindow {
                                window_id: window.id,
                                window_server_id,
                            }
                        })
                    }
                }
                _ => None,
            };
            action
        };

        if let Some(action) = action {
            self.emit_action(action);
        }
    }

    fn visible_workspaces<'a>(workspaces: &'a [WorkspaceData]) -> Vec<(usize, &'a WorkspaceData)> {
        workspaces
            .iter()
            .enumerate()
            .filter(|(_, ws)| !ws.windows.is_empty() || ws.is_active)
            .collect()
    }

    fn draw_workspaces(
        &self,
        state: &RefCell<MissionControlState>,
        parent_layer: &CALayer,
        workspaces: &[WorkspaceData],
        bounds: CGRect,
        selected: Option<usize>,
    ) {
        let visible = Self::visible_workspaces(workspaces);
        let Some(grid) = WorkspaceGrid::new(visible.len(), bounds) else {
            return;
        };
        let parent_layer = parent_layer;
        let mut visible_ids: HashSet<String> = HashSet::default();
        visible_ids.reserve(visible.len());
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        for (order_idx, (original_idx, _)) in visible.iter().enumerate() {
            autoreleasepool(|_| {
                let ws = &workspaces[*original_idx];
                let rect = grid.rect_for(order_idx);
                visible_ids.insert(ws.id.clone());
                let (ws_layer, label_layer) = {
                    let mut st = state.borrow_mut();
                    let ws_layer = st
                        .workspace_layers
                        .entry(ws.id.clone())
                        .or_insert_with(|| {
                            let lay = CALayer::layer();
                            parent_layer.addSublayer(&lay);
                            lay.setContentsScale(self.scale);
                            lay
                        })
                        .clone();
                    let label_layer = st
                        .workspace_label_layers
                        .entry(ws.id.clone())
                        .or_insert_with(|| {
                            let tl = CATextLayer::layer();
                            parent_layer.addSublayer(&tl);
                            tl.setContentsScale(self.scale);
                            tl
                        })
                        .clone();
                    match st.workspace_label_strings.entry(ws.id.clone()) {
                        hash_map::Entry::Occupied(mut occ) => {
                            if occ.get_mut().update(&ws.name) {
                                unsafe {
                                    occ.get().apply_to(&label_layer);
                                }
                            }
                        }
                        hash_map::Entry::Vacant(vac) => {
                            let cache = WorkspaceLabelText::new(&ws.name);
                            unsafe {
                                cache.apply_to(&label_layer);
                            }
                            vac.insert(cache);
                        }
                    }
                    (ws_layer, label_layer)
                };
                ws_layer.setFrame(rect);
                ws_layer.setCornerRadius(6.0);
                ws_layer.setBackgroundColor(Some(&**WORKSPACE_BACKGROUND_COLOR));

                let is_selected = Some(order_idx) == selected;
                if is_selected {
                    ws_layer.setBorderColor(Some(&**SELECTED_BORDER_COLOR));

                    ws_layer.setBorderWidth(3.0);
                } else {
                    ws_layer.setBorderColor(Some(&**WORKSPACE_BORDER_COLOR));

                    ws_layer.setBorderWidth(1.0);
                }
                ws_layer.setZPosition(-1.0);
                self.draw_windows_tile(
                    state,
                    parent_layer,
                    &ws.windows,
                    rect,
                    None,
                    WindowLayoutKind::PreserveOriginal,
                );
                let label_height = 18.0;
                let label_frame = CGRect::new(
                    CGPoint::new(rect.origin.x + 6.0, rect.origin.y + 6.0),
                    CGSize::new((rect.size.width - 12.0).max(10.0), label_height),
                );
                label_layer.setFrame(label_frame);
                label_layer.setContentsScale(self.scale);
                label_layer.setMasksToBounds(false);

                label_layer.setFontSize(12.0);
                let fg = NSColor::labelColor();
                label_layer.setForegroundColor(Some(&fg.CGColor()));

                label_layer.setZPosition(2.0);
            });
        }
        CATransaction::commit();
        {
            let mut st = state.borrow_mut();
            let visible_ids = &visible_ids;
            st.workspace_layers.retain(|id, layer| {
                if visible_ids.contains(id) {
                    true
                } else {
                    layer.removeFromSuperlayer();
                    false
                }
            });
            st.workspace_label_layers.retain(|id, layer| {
                if visible_ids.contains(id) {
                    true
                } else {
                    layer.removeFromSuperlayer();
                    false
                }
            });
            st.workspace_label_strings.retain(|id, _| visible_ids.contains(id));
        }
    }

    fn draw_windows_tile(
        &self,
        state: &RefCell<MissionControlState>,
        parent_layer: &CALayer,
        windows: &[WindowData],
        tile: CGRect,
        selected: Option<usize>,
        layout: WindowLayoutKind,
    ) {
        let Some(rects) = Self::compute_window_rects(windows, tile, layout) else {
            return;
        };

        let selected_idx = selected.map(|s| s.min(windows.len().saturating_sub(1)));

        let parent_layer = parent_layer;

        CATransaction::begin();
        CATransaction::setDisableActions(true);

        for idx in (0..windows.len()).rev() {
            autoreleasepool(|_| {
                let window = &windows[idx];
                let rect = rects[idx];
                let is_selected = selected_idx.map_or(false, |s| s == idx);
                Self::draw_window_outline(rect, is_selected);

                let (layer, style_changed, had_image) = {
                    let mut s = state.borrow_mut();
                    let layer = s
                        .preview_layers
                        .entry(window.id)
                        .or_insert_with(|| {
                            let lay = CALayer::layer();
                            parent_layer.addSublayer(&lay);
                            lay.setContentsScale(self.scale);
                            lay
                        })
                        .clone();
                    let style_changed = s
                        .preview_layer_styles
                        .entry(window.id)
                        .or_insert_with(Default::default)
                        .update_selected(is_selected);
                    let maybe_img_ptr = {
                        let cache = s.preview_cache.read();
                        cache
                            .get(&window.id)
                            .map(|img| img.as_ptr() as *mut objc2::runtime::AnyObject)
                    };
                    let mut had_image = false;
                    if let Some(img_ptr) = maybe_img_ptr {
                        unsafe {
                            let _: () = msg_send![&**layer, setContents: img_ptr];
                        }
                        s.ready_previews.insert(window.id);
                        had_image = true;
                    } else if s.ready_previews.contains(&window.id) {
                        had_image = true;
                    }
                    (layer, style_changed, had_image)
                };

                layer.setFrame(rect);
                layer.setMasksToBounds(true);
                layer.setCornerRadius(4.0);
                layer.setContentsScale(self.scale);
                if style_changed {
                    if is_selected {
                        layer.setBorderColor(Some(&**SELECTED_BORDER_COLOR));
                        layer.setBorderWidth(3.0);
                        layer.setZPosition(1.0);
                    } else {
                        layer.setBorderColor(Some(&**WINDOW_BORDER_COLOR));

                        layer.setBorderWidth(0.4);
                        layer.setZPosition(0.0);
                    }
                }

                if !had_image {
                    let (tw, th) = if matches!(layout, WindowLayoutKind::Exploded) {
                        (
                            window.frame.size.width.max(1.0) as usize,
                            window.frame.size.height.max(1.0) as usize,
                        )
                    } else {
                        (
                            (rect.size.width * 1.5).max(2.0) as usize,
                            (rect.size.height * 1.5).max(2.0) as usize,
                        )
                    };
                    self.schedule_capture(state, window, tw, th);
                }
            });
        }

        CATransaction::commit();
    }

    fn draw_window_outline(_rect: CGRect, _is_selected: bool) {}

    fn schedule_capture(
        &self,
        state: &RefCell<MissionControlState>,
        window: &WindowData,
        target_w: usize,
        target_h: usize,
    ) {
        let Some(wsid) = window.window_server_id else { return };
        let st = state.borrow();
        if st.ready_previews.contains(&window.id) {
            return;
        }
        {
            let cache = st.preview_cache.read();
            if cache.contains_key(&window.id) {
                return;
            }
        }
        let generation = CURRENT_GENERATION.load(Ordering::Acquire);
        {
            let mut set = IN_FLIGHT.lock();
            if !set.insert((generation, window.id)) {
                return;
            }
        }
        let job = CaptureJob {
            task: CaptureTask {
                window_id: window.id,
                window_server_id: wsid,
                target_w,
                target_h,
            },
            cache: st.preview_cache.clone(),
            generation,
            overlay_ptr_bits: self as *const _ as usize,
        };
        let _ = CAPTURE_POOL.sender.send(job);
    }

    fn prewarm_previews(&self) {
        let state_cell = &self.state;

        let mut tasks: Vec<(u8, i64, CaptureTask)> = {
            let mut pending = Vec::new();
            {
                let state_ref = state_cell.borrow();
                let mut push_window = |window: &WindowData, priority: u8| {
                    let Some(wsid) = window.window_server_id else { return };

                    let src_w = window.frame.size.width.max(1.0);
                    let src_h = window.frame.size.height.max(1.0);

                    let area = (src_w * src_h) as i64;
                    pending.push((priority, area, CaptureTask {
                        window_id: window.id,
                        window_server_id: wsid,
                        target_w: src_w as usize,
                        target_h: src_h as usize,
                    }));
                };

                match state_ref.mode() {
                    Some(MissionControlMode::AllWorkspaces(workspaces)) => {
                        for ws in workspaces {
                            let workspace_priority = if ws.is_active { 1 } else { 2 };
                            for window in &ws.windows {
                                let priority = if window.is_focused {
                                    0
                                } else {
                                    workspace_priority
                                };
                                push_window(window, priority);
                            }
                        }
                    }
                    Some(MissionControlMode::CurrentWorkspace(wins)) => {
                        for window in wins {
                            let priority = if window.is_focused { 0 } else { 1 };
                            push_window(window, priority);
                        }
                    }
                    None => {}
                }
            }

            pending.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
            pending
        };

        if tasks.is_empty() {
            return;
        }

        let generation = CURRENT_GENERATION.fetch_add(1, Ordering::AcqRel) + 1;

        let (preview_cache, overlay_ptr_bits) = {
            let st = state_cell.borrow();
            (st.preview_cache.clone(), self as *const _ as usize)
        };

        let sync_limit = SYNC_PREWARM_LIMIT.min(tasks.len());
        let async_tasks = tasks.split_off(sync_limit);
        let sync_tasks = tasks;

        for (_, _, task) in sync_tasks.into_iter() {
            {
                let cache = preview_cache.read();
                if cache.contains_key(&task.window_id) {
                    continue;
                }
            }
            {
                let mut set = IN_FLIGHT.lock();
                if !set.insert((generation, task.window_id)) {
                    continue;
                }
            }

            let result = crate::sys::window_server::capture_window_image(
                WindowServerId::new(task.window_server_id),
                task.target_w,
                task.target_h,
            );

            match result {
                Some(img) => {
                    {
                        let mut cache = preview_cache.write();
                        cache.insert(task.window_id, img);
                    }
                    {
                        let mut set = IN_FLIGHT.lock();
                        set.remove(&(generation, task.window_id));
                    }
                    if let Ok(mut st) = state_cell.try_borrow_mut() {
                        st.ready_previews.insert(task.window_id);
                    }
                    if let Some(overlay) =
                        unsafe { (overlay_ptr_bits as *const MissionControlOverlay).as_ref() }
                    {
                        overlay.request_refresh();
                    }
                }
                None => {
                    let mut set = IN_FLIGHT.lock();
                    set.remove(&(generation, task.window_id));
                }
            }
        }

        for (_, _, task) in async_tasks.into_iter() {
            {
                let cache = preview_cache.read();
                if cache.contains_key(&task.window_id) {
                    continue;
                }
            }
            {
                let mut set = IN_FLIGHT.lock();
                if !set.insert((generation, task.window_id)) {
                    continue;
                }
            }

            let job = CaptureJob {
                task,
                cache: preview_cache.clone(),
                generation,
                overlay_ptr_bits,
            };
            if CAPTURE_POOL.sender.send(job).is_err() {
                break;
            }
        }
    }

    fn refresh_previews(&self) {
        let state_cell = &self.state;

        let (layers, cache_arc) = {
            let st = match state_cell.try_borrow() {
                Ok(s) => s,
                Err(_) => return,
            };
            let pairs: Vec<(WindowId, Retained<CALayer>)> =
                st.preview_layers.iter().map(|(wid, layer)| (*wid, layer.clone())).collect();
            (pairs, st.preview_cache.clone())
        };

        let mut ready_ids: Vec<WindowId> = Vec::new();

        CATransaction::begin();
        CATransaction::setDisableActions(true);

        {
            let cache = cache_arc.read();
            for (wid, layer) in layers.iter() {
                if let Some(img) = cache.get(wid) {
                    unsafe {
                        let img_ptr = img.as_ptr() as *mut objc2::runtime::AnyObject;
                        let _: () = msg_send![&**layer, setContents: img_ptr];
                    }
                    ready_ids.push(*wid);
                }
            }
        }

        CATransaction::commit();

        if !ready_ids.is_empty() {
            if let Ok(mut st) = state_cell.try_borrow_mut() {
                for wid in ready_ids.iter().copied() {
                    st.ready_previews.insert(wid);
                }
                if !st.suppress_live_present {
                    if let (Some(root), Some(wid), Some(size)) =
                        (st.render_root.clone(), st.render_window_id, st.render_size)
                    {
                        unsafe {
                            let ctx: *mut CGContext = SLWindowContextCreate(
                                *G_CONNECTION,
                                wid,
                                core::ptr::null_mut() as *mut CFType,
                            );
                            if !ctx.is_null() {
                                let clear = CGRect::new(CGPoint::new(0.0, 0.0), size);
                                CGContextClearRect(ctx, clear);
                                CGContextSaveGState(ctx);
                                CGContextTranslateCTM(ctx, 0.0, size.height);
                                CGContextScaleCTM(ctx, 1.0, -1.0);
                                root.renderInContext(&*ctx);
                                CGContextRestoreGState(ctx);
                                CGContextFlush(ctx);
                                SLSFlushWindowContentRegion(
                                    *G_CONNECTION,
                                    wid,
                                    std::ptr::null_mut(),
                                );
                                CFRelease(ctx as *mut CFType);
                            }
                        }
                    }
                }
            }
        }
    }

    fn draw_contents_into_layer(&self, bounds: CGRect, parent_layer: &CALayer) {
        let state_cell = &self.state;
        let (mode, selected_workspace, selected_window) = {
            let mut state = state_cell.borrow_mut();
            let Some(mode) = state.mode().cloned() else {
                return;
            };
            state.ensure_selection();
            (mode, state.selected_workspace(), state.selected_window())
        };

        parent_layer.setBackgroundColor(Some(&**OVERLAY_BACKGROUND_COLOR));

        let content_bounds = Self::content_bounds(bounds);
        match mode {
            MissionControlMode::AllWorkspaces(workspaces) => {
                self.draw_workspaces(
                    &state_cell,
                    parent_layer,
                    &workspaces,
                    content_bounds,
                    selected_workspace,
                );
            }
            MissionControlMode::CurrentWorkspace(windows) => {
                self.draw_windows_tile(
                    &state_cell,
                    parent_layer,
                    &windows,
                    content_bounds,
                    selected_window,
                    WindowLayoutKind::Exploded,
                );
            }
        }
    }
}

pub struct MissionControlOverlay {
    cgs_window: CgsWindow,
    root_layer: Retained<CALayer>,
    frame: CGRect,
    mtm: MainThreadMarker,
    key_tap: RefCell<Option<crate::sys::event_tap::EventTap>>,
    fade_enabled: bool,
    fade_duration_ms: f64,
    has_shown: RefCell<bool>,
    state: RefCell<MissionControlState>,
    fade_state: RefCell<Option<FadeState>>,
    fade_counter: AtomicU64,
    pending_hide: RefCell<bool>,
    refresh_pending: AtomicBool,
    scale: f64,
    coordinate_converter: CoordinateConverter,
}

impl MissionControlOverlay {
    pub fn new(config: Config, mtm: MainThreadMarker, frame: CGRect, scale: f64) -> Self {
        let mut frame = frame;
        let mut scale = scale;
        let mut coordinate_converter = CoordinateConverter::default();

        if let Some(screen) = NSScreen::mainScreen(mtm) {
            let mut cache = ScreenCache::new(mtm);
            if let Some((_descriptors, converter, _spaces)) = cache.refresh() {
                coordinate_converter = converter;
            }
            scale = screen.backingScaleFactor();
            if let Ok(screen_id) = screen.get_number() {
                frame = CGDisplayBounds(screen_id.as_u32());
            }
        }

        let root_layer = CALayer::layer();
        root_layer.setGeometryFlipped(true);

        root_layer.setFrame(CGRect::new(CGPoint::new(0.0, 0.0), frame.size));
        root_layer.setContentsScale(scale);

        let cgs_window = CgsWindow::new(frame).expect("failed to create CGS window");
        let _ = cgs_window.set_resolution(scale);
        let _ = cgs_window.set_opacity(false);
        let _ = cgs_window.set_alpha(1.0);
        let _ = cgs_window.set_level(NSPopUpMenuWindowLevel as i32);
        let _ = cgs_window.set_blur(30, None);

        Self {
            cgs_window,
            root_layer,
            frame,
            mtm,
            key_tap: RefCell::new(None),
            fade_enabled: config.settings.ui.mission_control.fade_enabled,
            fade_duration_ms: config.settings.ui.mission_control.fade_duration_ms,
            has_shown: RefCell::new(false),
            state: RefCell::new(MissionControlState::default()),
            fade_state: RefCell::new(None),
            fade_counter: AtomicU64::new(0),
            pending_hide: RefCell::new(false),
            refresh_pending: AtomicBool::new(false),
            scale,
            coordinate_converter,
        }
    }

    fn request_refresh(&self) {
        if !self.refresh_pending.swap(true, Ordering::AcqRel) {
            let ptr = self as *const _ as usize;
            queue::main().after_f(
                Time::new_after(Time::NOW, 8000000),
                ptr as *mut c_void,
                refresh_coalesced_cb,
            );
        }
    }

    pub fn set_action_handler(&self, f: Rc<dyn Fn(MissionControlAction)>) {
        self.state.borrow_mut().on_action = Some(f);
    }

    pub fn set_fade_enabled(&mut self, enabled: bool) { self.fade_enabled = enabled; }

    pub fn set_fade_duration_ms(&mut self, ms: f64) { self.fade_duration_ms = ms.max(0.0); }

    fn current_screen_metrics(&self) -> ScreenMetrics {
        if let Some((metrics, _converter)) = self.gather_screen_metrics() {
            if let Some(cursor_metric) = self.screen_under_cursor_with(&metrics) {
                return cursor_metric;
            }

            if let Some(main_metric) = self.main_screen_metric(&metrics) {
                return main_metric;
            }
        }

        ScreenMetrics {
            id: None,
            frame: self.frame,
            scale: self.scale,
            converter: self.coordinate_converter,
        }
    }

    pub fn update(&self, mode: MissionControlMode) {
        self.stop_active_fade();
        *self.pending_hide.borrow_mut() = false;

        {
            let metrics = self.current_screen_metrics();
            let new_frame = metrics.frame;
            let new_scale = metrics.scale;

            let frame_changed = new_frame.origin.x != self.frame.origin.x
                || new_frame.origin.y != self.frame.origin.y
                || new_frame.size.width != self.frame.size.width
                || new_frame.size.height != self.frame.size.height;
            let scale_changed = (new_scale - self.scale).abs() > f64::EPSILON;

            if frame_changed || scale_changed {
                let _ = self.cgs_window.set_shape(new_frame);
                let _ = self.cgs_window.set_resolution(new_scale);

                unsafe {
                    let me = self as *const _ as *mut MissionControlOverlay;
                    (*me).frame = new_frame;
                    (*me).scale = new_scale;
                }

                self.root_layer.setFrame(CGRect::new(CGPoint::new(0.0, 0.0), self.frame.size));
                self.root_layer.setContentsScale(self.scale);
            }
            unsafe {
                let me = self as *const _ as *mut MissionControlOverlay;
                (*me).coordinate_converter = metrics.converter;
            }
        }

        {
            let mut st = self.state.borrow_mut();
            st.set_mode(mode.clone());

            st.render_root = Some(self.root_layer.clone());
            st.render_window_id = Some(self.cgs_window.id());
            st.render_size = Some(self.frame.size);

            st.suppress_live_present = false;
        }
        self.prewarm_previews();

        if self.fade_enabled && !*self.has_shown.borrow() {
            let _ = self.cgs_window.set_alpha(0.0);
        } else {
            let _ = self.cgs_window.set_alpha(1.0);
        }
        let _ = self.cgs_window.order_above(None);

        let app = NSApplication::sharedApplication(self.mtm);
        let _ = app.activate();
        self.ensure_key_tap();

        self.draw_and_present();

        if self.fade_enabled && !*self.has_shown.borrow() {
            self.fade_in();
        }
        *self.has_shown.borrow_mut() = true;
    }

    pub fn hide(&self) {
        let was_shown = {
            let mut shown = self.has_shown.borrow_mut();
            let prev = *shown;
            *shown = false;
            prev
        };

        if self.fade_enabled && was_shown {
            *self.pending_hide.borrow_mut() = true;
            if !self.fade_out() {
                self.finalize_hide();
            }
        } else {
            self.finalize_hide();
        }
    }

    fn finalize_hide(&self) {
        objc2::rc::autoreleasepool(|_| {
            self.stop_active_fade();
            self.key_tap.borrow_mut().take();

            {
                let mut s = self.state.borrow_mut();
                s.purge();
            }

            let _ = self.cgs_window.order_out();
            let _ = self.cgs_window.set_alpha(1.0);
            CATransaction::flush();

            *self.has_shown.borrow_mut() = false;
            *self.pending_hide.borrow_mut() = false;
        });
    }

    fn fade_in(&self) {
        self.stop_active_fade();
        let duration_ms = self.fade_duration_ms.max(0.0);
        if duration_ms <= 0.0 {
            let _ = self.cgs_window.set_alpha(1.0);
            return;
        }

        let fade_id = self.fade_counter.fetch_add(1, Ordering::AcqRel) + 1;
        let overlay_ptr_bits = self as *const MissionControlOverlay as usize;

        CATransaction::begin();
        CATransaction::setAnimationDuration(duration_ms / 1000.0);
        self.root_layer.setOpacity(0.0);
        self.root_layer.setOpacity(1.0);

        CATransaction::commit();

        schedule_fade_completion(overlay_ptr_bits, fade_id, 1.0f32);

        self.fade_state.borrow_mut().replace(FadeState { id: fade_id });
    }

    fn fade_out(&self) -> bool {
        self.stop_active_fade();
        let duration_ms = self.fade_duration_ms.max(0.0);
        if duration_ms <= 0.0 {
            let _ = self.cgs_window.set_alpha(0.0);
            return false;
        }

        let fade_id = self.fade_counter.fetch_add(1, Ordering::AcqRel) + 1;
        let overlay_ptr_bits = self as *const MissionControlOverlay as usize;

        CATransaction::begin();
        CATransaction::setAnimationDuration(duration_ms / 1000.0);

        self.root_layer.setOpacity(1.0);
        self.root_layer.setOpacity(0.0);

        CATransaction::commit();

        schedule_fade_completion(overlay_ptr_bits, fade_id, 0.0f32);

        self.fade_state.borrow_mut().replace(FadeState { id: fade_id });
        true
    }

    fn stop_active_fade(&self) {
        self.root_layer.removeAllAnimations();
        self.fade_state.borrow_mut().take();
    }

    fn finish_fade(&self, fade_id: u64, final_alpha: f32) {
        match self.fade_state.try_borrow_mut() {
            Ok(mut slot) => {
                let matches = slot.as_ref().map_or(false, |state| state.id == fade_id);
                if !matches {
                    return;
                }
                slot.take();
                drop(slot);
            }
            Err(_) => {
                let overlay_ptr_bits = self as *const MissionControlOverlay as usize;
                schedule_fade_completion(overlay_ptr_bits, fade_id, final_alpha);
                return;
            }
        }

        let _ = self.cgs_window.set_alpha(final_alpha);

        let should_finalize = if final_alpha <= 0.0 {
            *self.pending_hide.borrow()
        } else {
            false
        };

        if should_finalize {
            self.finalize_hide();
        }
    }

    pub fn refresh_active_workspace(&self, active_workspace: Option<VirtualWorkspaceId>) {
        let active_id = active_workspace.map(|ws| format!("{:?}", ws));
        let mut state = match self.state.try_borrow_mut() {
            Ok(state) => state,
            Err(_) => return,
        };
        if state.highlight_active_workspace(active_id) {
            drop(state);
            self.draw_and_present();
        }
    }

    fn draw_and_present(&self) {
        CATransaction::begin();
        CATransaction::setDisableActions(true);

        self.root_layer.setFrame(CGRect::new(CGPoint::new(0.0, 0.0), self.frame.size));
        self.root_layer.setGeometryFlipped(true);

        self.draw_contents_into_layer(
            CGRect::new(CGPoint::new(0.0, 0.0), self.frame.size),
            &self.root_layer,
        );
        CATransaction::commit();

        let ctx: *mut CGContext = unsafe {
            SLWindowContextCreate(
                *G_CONNECTION,
                self.cgs_window.id(),
                core::ptr::null_mut() as *mut CFType,
            )
        };
        if !ctx.is_null() {
            unsafe {
                let clear = CGRect::new(CGPoint::new(0.0, 0.0), self.frame.size);
                CGContextClearRect(ctx, clear);
                CGContextSaveGState(ctx);
                CGContextTranslateCTM(ctx, 0.0, self.frame.size.height);
                CGContextScaleCTM(ctx, 1.0, -1.0);
                self.root_layer.renderInContext(&*ctx);
                CGContextRestoreGState(ctx);
                CGContextFlush(ctx);
                SLSFlushWindowContentRegion(
                    *G_CONNECTION,
                    self.cgs_window.id(),
                    std::ptr::null_mut(),
                );
                CFRelease(ctx as *mut CFType);
            }
        }
    }

    fn emit_action(&self, action: MissionControlAction) {
        // Ensure the user-provided action handler runs on the main queue. Event taps
        // deliver events on a separate thread/CFRunLoop; invoking the handler
        // directly can cause UI work (like hiding the mission control overlay)
        // to happen off the main thread which can lead to races where the overlay
        // doesn't get hidden when using the mouse.
        let handler = self.state.borrow().on_action.clone();
        let Some(cb) = handler else {
            return;
        };

        type Ctx = (Rc<dyn Fn(MissionControlAction)>, MissionControlAction);

        extern "C" fn action_callback(ctx: *mut c_void) {
            if ctx.is_null() {
                return;
            }
            unsafe {
                let boxed = Box::from_raw(ctx as *mut Ctx);
                let (cb, action) = *boxed;
                cb(action);
            }
        }

        let ctx: Box<Ctx> = Box::new((cb, action));
        queue::main().after_f(Time::NOW, Box::into_raw(ctx) as *mut c_void, action_callback);
    }

    fn handle_keycode(&self, keycode: u16, flags: CGEventFlags) -> bool {
        let handled = match keycode {
            53 => {
                self.emit_action(MissionControlAction::Dismiss);
                true
            }
            123 => {
                if self.adjust_selection(NavDirection::Left) {
                    self.draw_and_present();
                }
                true
            }
            124 => {
                if self.adjust_selection(NavDirection::Right) {
                    self.draw_and_present();
                }
                true
            }
            125 => {
                if self.adjust_selection(NavDirection::Down) {
                    self.draw_and_present();
                }
                true
            }
            126 => {
                if self.adjust_selection(NavDirection::Up) {
                    self.draw_and_present();
                }
                true
            }
            36 | 76 => {
                self.activate_selection_action();
                true
            }
            48 => {
                let forward = !flags.contains(CGEventFlags::MaskShift);
                if self.cycle_selection(forward) {
                    self.draw_and_present();
                }
                true
            }
            _ => false,
        };
        handled
    }

    fn handle_click_global(&self, g_pt: CGPoint) {
        let lx = g_pt.x - self.frame.origin.x;
        let ly = g_pt.y - self.frame.origin.y;
        let pt = CGPoint::new(lx, ly);

        let mut state = match self.state.try_borrow_mut() {
            Ok(s) => s,
            Err(_) => return,
        };
        let mode = match state.mode() {
            Some(m) => m,
            None => return,
        };
        let content_bounds = Self::content_bounds(CGRect::new(
            CGPoint::new(0.0, 0.0),
            CGSize::new(self.frame.size.width, self.frame.size.height),
        ));

        let new_sel = match mode {
            MissionControlMode::AllWorkspaces(workspaces) => {
                Self::workspace_index_at_point(workspaces, pt, content_bounds)
                    .map(|(order_idx, _)| Selection::Workspace(order_idx))
            }
            MissionControlMode::CurrentWorkspace(windows) => {
                Self::window_at_point(windows, pt, content_bounds, WindowLayoutKind::Exploded)
                    .map(|(order_idx, _)| Selection::Window(order_idx))
            }
        };

        match new_sel {
            Some(sel) => {
                state.set_selection(sel);
                drop(state);
                self.draw_and_present();
                self.activate_selection_action();
            }
            None => {
                drop(state);
                self.emit_action(MissionControlAction::Dismiss);
            }
        }
    }

    fn handle_move_global(&self, g_pt: CGPoint) {
        let lx = g_pt.x - self.frame.origin.x;
        let ly = g_pt.y - self.frame.origin.y;
        let pt = CGPoint::new(lx, ly);

        let mut state = match self.state.try_borrow_mut() {
            Ok(s) => s,
            Err(_) => return,
        };
        let mode = match state.mode() {
            Some(m) => m,
            None => return,
        };
        let content_bounds = Self::content_bounds(CGRect::new(
            CGPoint::new(0.0, 0.0),
            CGSize::new(self.frame.size.width, self.frame.size.height),
        ));

        let new_sel = match mode {
            MissionControlMode::AllWorkspaces(workspaces) => {
                Self::workspace_index_at_point(workspaces, pt, content_bounds)
                    .map(|(order_idx, _)| Selection::Workspace(order_idx))
            }
            MissionControlMode::CurrentWorkspace(windows) => {
                Self::window_at_point(windows, pt, content_bounds, WindowLayoutKind::Exploded)
                    .map(|(order_idx, _)| Selection::Window(order_idx))
            }
        };

        if let Some(sel) = new_sel {
            if state.selection() != Some(sel) {
                state.set_selection(sel);
                drop(state);
                self.draw_and_present();
            }
        }
    }

    fn ensure_key_tap(&self) {
        if self.key_tap.borrow().is_some() {
            return;
        }

        #[repr(C)]
        struct KeyCtx {
            overlay: *const MissionControlOverlay,
            consumes: bool,
        }

        unsafe fn drop_ctx(ptr: *mut c_void) {
            unsafe {
                drop(Box::from_raw(ptr as *mut KeyCtx));
            }
        }

        unsafe extern "C-unwind" fn key_callback(
            _proxy: CGEventTapProxy,
            etype: CGEventType,
            event: core::ptr::NonNull<CGEvent>,
            user_info: *mut c_void,
        ) -> *mut CGEvent {
            let ctx = unsafe { &*(user_info as *const KeyCtx) };
            let mut handled = false;
            if let Some(overlay) = unsafe { ctx.overlay.as_ref() } {
                match etype {
                    CGEventType::KeyDown => {
                        let keycode = unsafe {
                            CGEvent::integer_value_field(
                                Some(event.as_ref()),
                                CGEventField::KeyboardEventKeycode,
                            ) as u16
                        };
                        let flags = unsafe { CGEvent::flags(Some(event.as_ref())) };
                        handled = overlay.handle_keycode(keycode, flags);
                    }
                    CGEventType::LeftMouseDown => {
                        let loc = unsafe { CGEvent::location(Some(event.as_ref())) };
                        overlay.handle_click_global(loc);
                        handled = true;
                    }
                    CGEventType::LeftMouseUp => {
                        handled = true;
                    }
                    CGEventType::MouseMoved => {
                        let loc = unsafe { CGEvent::location(Some(event.as_ref())) };
                        overlay.handle_move_global(loc);
                        handled = true;
                    }
                    _ => {}
                }
            }
            if handled && ctx.consumes {
                core::ptr::null_mut()
            } else {
                event.as_ptr()
            }
        }

        let mask = (1u64 << CGEventType::KeyDown.0 as u64)
            | (1u64 << CGEventType::LeftMouseDown.0 as u64)
            | (1u64 << CGEventType::LeftMouseUp.0 as u64)
            | (1u64 << CGEventType::MouseMoved.0 as u64);

        let overlay_ptr = self as *const _;

        let tap = unsafe {
            let ctx_ptr = Box::into_raw(Box::new(KeyCtx {
                overlay: overlay_ptr,
                consumes: true,
            })) as *mut c_void;
            match crate::sys::event_tap::EventTap::new_with_options(
                CGEventTapOptions::Default,
                mask,
                Some(key_callback),
                ctx_ptr,
                Some(drop_ctx),
            ) {
                Some(tap) => Some(tap),
                None => {
                    drop_ctx(ctx_ptr);
                    let ctx_ptr = Box::into_raw(Box::new(KeyCtx {
                        overlay: overlay_ptr,
                        consumes: false,
                    })) as *mut c_void;
                    match crate::sys::event_tap::EventTap::new_listen_only(
                        mask,
                        Some(key_callback),
                        ctx_ptr,
                        Some(drop_ctx),
                    ) {
                        Some(tap) => {
                            info!(
                                "Falling back to listen-only event tap; Mission Control overlay input will pass through"
                            );
                            Some(tap)
                        }
                        None => {
                            drop_ctx(ctx_ptr);
                            None
                        }
                    }
                }
            }
        };

        if let Some(t) = tap {
            self.key_tap.borrow_mut().replace(t);
        }
    }
}
