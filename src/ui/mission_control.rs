use core::ffi::c_void;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use block2::RcBlock;
use dispatchr::queue;
use dispatchr::time::Time;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{AnyThread, msg_send};
use objc2_app_kit::{NSApplication, NSColor, NSPopUpMenuWindowLevel, NSScreen};
use objc2_core_foundation::{CFRetained, CFString, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGColor, CGDisplayBounds, CGEvent, CGEventField, CGEventFlags, CGEventTapOptions,
    CGEventTapProxy, CGEventType,
};
use objc2_core_media::CMSampleBuffer;
use objc2_core_video::CVPixelBufferGetIOSurface;
use objc2_foundation::{MainThreadMarker, NSDictionary, NSError};
use objc2_io_surface::IOSurfaceRef;
use objc2_quartz_core::{CALayer, CATextLayer, CATransaction};
use objc2_screen_capture_kit::{
    SCCaptureResolutionType, SCContentFilter, SCScreenshotManager, SCShareableContent,
    SCStreamConfiguration,
};
use once_cell::sync::Lazy;
use tracing::info;

use crate::actor::app::WindowId;
use crate::common::collections::{HashMap, HashSet};
use crate::common::config::Config;
use crate::model::server::{RuntimeWindowData, RuntimeWorkspaceData};
use crate::sys::cgs_window::CgsWindow;
use crate::sys::dispatch::DispatchExt;
use crate::sys::event::current_cursor_location;
use crate::sys::geometry::CGRectExt;
use crate::sys::screen::{
    CoordinateConverter, NSScreenExt, ScreenCache, ScreenId, ScreenInfo, get_active_space_number,
};
use crate::sys::window_server::WindowServerId;
use crate::ui::common::{
    compute_window_layout_metrics, render_layer_to_cgs_window, with_disabled_actions,
};

#[derive(Clone, Copy)]
struct CaptureTarget {
    window_id: WindowId,
    window_server_id: WindowServerId,
    width: usize,
    height: usize,
    revision: u64,
}

#[derive(Clone)]
struct CapturedFrame {
    surface: CFRetained<IOSurfaceRef>,
    revision: u64,
}

// IOSurface objects are explicitly shareable across threads and processes. The retained
// reference keeps the surface alive until the main thread attaches it to Core Animation.
unsafe impl Send for CapturedFrame {}

struct CaptureRequest {
    target: CaptureTarget,
    filter: Retained<SCContentFilter>,
    config: Retained<SCStreamConfiguration>,
}

// ScreenCaptureKit configuration and filter objects are immutable once queued here and are
// consumed only by ScreenCaptureKit's thread-safe class capture method.
unsafe impl Send for CaptureRequest {}

const MAX_CONCURRENT_CAPTURES: usize = 4;

#[inline]
fn capture_batch_size(active: usize, pending: usize) -> usize {
    MAX_CONCURRENT_CAPTURES.saturating_sub(active).min(pending)
}

#[derive(Default)]
struct CaptureState {
    frames: HashMap<WindowId, CapturedFrame>,
    in_flight: HashSet<(WindowId, usize, usize)>,
    notification_pending: bool,
    pending: VecDeque<CaptureRequest>,
    active: usize,
}

#[derive(Clone)]
struct CapturePipeline {
    state: Arc<Mutex<CaptureState>>,
    notify: Arc<dyn Fn() + Send + Sync>,
    revision: Arc<AtomicU64>,
}

impl CapturePipeline {
    fn new(notify: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self {
            state: Arc::new(Mutex::new(CaptureState::default())),
            notify,
            revision: Arc::new(AtomicU64::new(0)),
        }
    }

    fn capture(&self, targets: Vec<CaptureTarget>) {
        let revision = self.revision.fetch_add(1, Ordering::Relaxed) + 1;
        let targets = {
            let mut state = self.state.lock().unwrap();
            targets
                .into_iter()
                .filter_map(|mut target| {
                    target.revision = revision;
                    state
                        .in_flight
                        .insert((target.window_id, target.width, target.height))
                        .then_some(target)
                })
                .collect::<Vec<_>>()
        };
        if targets.is_empty() {
            return;
        }

        let pipeline = self.clone();
        let block = RcBlock::new(move |content: *mut SCShareableContent, _error: *mut NSError| {
            if targets
                .first()
                .is_none_or(|target| target.revision != pipeline.revision.load(Ordering::Acquire))
            {
                pipeline.finish_failed(&targets);
                return;
            }
            let Some(content) = NonNull::new(content) else {
                pipeline.finish_failed(&targets);
                return;
            };
            let windows = unsafe { content.as_ref().windows() };
            let mut requests = Vec::with_capacity(targets.len());
            for target in &targets {
                let window = windows.iter().find(|window| unsafe {
                    window.windowID() == target.window_server_id.as_u32()
                });
                let Some(window) = window else {
                    pipeline.finish_failed(std::slice::from_ref(target));
                    continue;
                };

                let filter = unsafe {
                    SCContentFilter::initWithDesktopIndependentWindow(
                        SCContentFilter::alloc(),
                        &window,
                    )
                };
                let config = unsafe { SCStreamConfiguration::new() };
                unsafe {
                    config.setWidth(target.width.max(1));
                    config.setHeight(target.height.max(1));
                    config.setPixelFormat(u32::from_be_bytes(*b"BGRA"));
                    config.setShowsCursor(false);
                    config.setScalesToFit(true);
                    config.setPreservesAspectRatio(true);
                    config.setQueueDepth(3);
                    config.setCapturesAudio(false);
                    config.setIgnoreShadowsSingleWindow(true);
                    config.setIgnoreGlobalClipSingleWindow(true);
                    config.setShouldBeOpaque(false);
                    config.setCaptureResolution(SCCaptureResolutionType::Nominal);
                }
                requests.push(CaptureRequest {
                    target: *target,
                    filter,
                    config,
                });
            }
            pipeline.enqueue(requests);
        });
        unsafe {
            SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(
                true, false, &block,
            );
        }
    }

    fn enqueue(&self, requests: Vec<CaptureRequest>) {
        {
            let mut state = self.state.lock().unwrap();
            state.pending.extend(requests);
        }
        self.pump();
    }

    fn pump(&self) {
        let requests = {
            let mut state = self.state.lock().unwrap();
            let count = capture_batch_size(state.active, state.pending.len());
            let requests = state.pending.drain(..count).collect::<Vec<_>>();
            state.active += requests.len();
            requests
        };

        for request in requests {
            let target = request.target;
            let pipeline = self.clone();
            let completion =
                RcBlock::new(move |sample: *mut CMSampleBuffer, _error: *mut NSError| {
                    let surface = NonNull::new(sample)
                        .and_then(|sample| unsafe { sample.as_ref().image_buffer() })
                        .and_then(|buffer| CVPixelBufferGetIOSurface(Some(&buffer)));
                    pipeline.finish(target, surface);
                });
            unsafe {
                SCScreenshotManager::captureSampleBufferWithFilter_configuration_completionHandler(
                    &request.filter,
                    &request.config,
                    Some(&completion),
                );
            }
        }
    }

    fn finish(&self, target: CaptureTarget, surface: Option<CFRetained<IOSurfaceRef>>) {
        let should_notify = {
            let mut state = self.state.lock().unwrap();
            state.in_flight.remove(&(target.window_id, target.width, target.height));
            if target.revision != self.revision.load(Ordering::Acquire) {
                return;
            }
            state.active = state.active.saturating_sub(1);
            if let Some(surface) = surface {
                let replaces = state
                    .frames
                    .get(&target.window_id)
                    .is_none_or(|cached| target.revision >= cached.revision);
                if replaces {
                    state.frames.insert(target.window_id, CapturedFrame {
                        surface,
                        revision: target.revision,
                    });
                }
                if replaces && !state.notification_pending {
                    state.notification_pending = true;
                    true
                } else {
                    false
                }
            } else {
                false
            }
        };
        if should_notify {
            (self.notify)();
        }
        self.pump();
    }

    fn finish_failed(&self, targets: &[CaptureTarget]) {
        let mut state = self.state.lock().unwrap();
        for target in targets {
            state.in_flight.remove(&(target.window_id, target.width, target.height));
        }
    }

    fn take_frames(&self) -> HashMap<WindowId, CapturedFrame> {
        let mut state = self.state.lock().unwrap();
        state.notification_pending = false;
        std::mem::take(&mut state.frames)
    }

    fn clear(&self) {
        self.revision.fetch_add(1, Ordering::AcqRel);
        let mut state = self.state.lock().unwrap();
        state.frames.clear();
        state.in_flight.clear();
        state.notification_pending = false;
        state.pending.clear();
        state.active = 0;
    }
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
    AllWorkspaces(Vec<RuntimeWorkspaceData>),
    CurrentWorkspace(Vec<RuntimeWindowData>),
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

#[derive(Default)]
pub struct MissionControlState {
    mode: Option<MissionControlMode>,
    on_action: Option<Rc<dyn Fn(MissionControlAction)>>,
    selection: Option<Selection>,
    preview_layers: HashMap<WindowId, Retained<CALayer>>,
    workspace_layers: HashMap<String, Retained<CALayer>>,
    workspace_label_layers: HashMap<String, Retained<CATextLayer>>,
}

impl MissionControlState {
    fn set_mode(&mut self, mode: MissionControlMode) {
        self.mode = Some(mode);
        self.selection = None;
        self.prune_preview_layers();
        self.ensure_selection();
    }

    fn mode(&self) -> Option<&MissionControlMode> { self.mode.as_ref() }

    fn purge(&mut self) {
        self.mode = None;
        self.selection = None;

        for (_id, layer) in self.preview_layers.drain() {
            layer.removeFromSuperlayer();
        }
        for (_id, layer) in self.workspace_layers.drain() {
            layer.removeFromSuperlayer();
        }
        for (_id, layer) in self.workspace_label_layers.drain() {
            layer.removeFromSuperlayer();
        }
    }

    fn selection(&self) -> Option<Selection> { self.selection }

    fn set_selection(&mut self, selection: Selection) {
        let is_valid = matches!(
            (selection, self.mode.as_ref()),
            (
                Selection::Workspace(_),
                Some(MissionControlMode::AllWorkspaces(_))
            ) | (
                Selection::Window(_),
                Some(MissionControlMode::CurrentWorkspace(_))
            )
        );
        if is_valid {
            self.selection = Some(selection);
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
                    if !ws.windows.is_empty() {
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

    fn prune_preview_layers(&mut self) {
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

        let mut remove_keys = Vec::new();
        for (&wid, layer) in self.preview_layers.iter() {
            if !valid.contains(&wid) {
                layer.removeFromSuperlayer();
                remove_keys.push(wid);
            }
        }
        for k in remove_keys {
            self.preview_layers.remove(&k);
        }
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
        count.div_ceil(2).max(1)
    }
}

const MISSION_CONTROL_MARGIN: f64 = 48.0;
const WINDOW_TILE_INSET: f64 = 3.0;
const WINDOW_TILE_GAP: f64 = 1.0;
const WINDOW_TILE_MIN_SIZE: f64 = 2.0;
const WINDOW_TILE_SCALE_FACTOR: f64 = 0.75;
const WINDOW_TILE_MAX_SCALE: f64 = 1.0;
const WORKSPACE_TILE_SPACING: f64 = 20.0;
const CURRENT_WS_TILE_SPACING: f64 = 48.0;
const CURRENT_WS_TILE_PADDING: f64 = 16.0;

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

#[derive(Clone, Copy)]
enum WindowLayoutKind {
    PreserveOriginal,
    Exploded,
}

struct FadeState {
    id: u64,
}

impl MissionControlOverlay {
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
        workspaces: &[RuntimeWorkspaceData],
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
        windows: &[RuntimeWindowData],
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

    fn compute_exploded_layout(
        windows: &[RuntimeWindowData],
        bounds: CGRect,
    ) -> Option<Vec<CGRect>> {
        if windows.is_empty() {
            return None;
        }
        let aspect = bounds.size.width / bounds.size.height.max(1.0);
        let cols = ((windows.len() as f64 * aspect).sqrt().ceil() as usize).max(1);
        let rows = windows.len().div_ceil(cols);
        let spacing = CURRENT_WS_TILE_SPACING;
        let cell_w = (bounds.size.width - spacing * (cols + 1) as f64) / cols as f64;
        let cell_h = (bounds.size.height - spacing * (rows + 1) as f64) / rows as f64;
        let inner_w = (cell_w - CURRENT_WS_TILE_PADDING * 2.0).max(WINDOW_TILE_MIN_SIZE);
        let inner_h = (cell_h - CURRENT_WS_TILE_PADDING * 2.0).max(WINDOW_TILE_MIN_SIZE);

        Some(
            windows
                .iter()
                .enumerate()
                .map(|(index, window)| {
                    let row = index / cols;
                    let row_len = (windows.len() - row * cols).min(cols);
                    let col = index % cols;
                    let row_offset = (cols - row_len) as f64 * (cell_w + spacing) / 2.0;
                    let source = window.info.frame.size;
                    let scale = (inner_w / source.width.max(1.0))
                        .min(inner_h / source.height.max(1.0))
                        .min(WINDOW_TILE_MAX_SCALE);
                    let size = CGSize::new(
                        (source.width * scale).max(WINDOW_TILE_MIN_SIZE),
                        (source.height * scale).max(WINDOW_TILE_MIN_SIZE),
                    );
                    let cell_x =
                        bounds.origin.x + spacing + row_offset + col as f64 * (cell_w + spacing);
                    let cell_y = bounds.origin.y + spacing + row as f64 * (cell_h + spacing);
                    CGRect::new(
                        CGPoint::new(
                            cell_x + (cell_w - size.width) / 2.0,
                            cell_y + (cell_h - size.height) / 2.0,
                        ),
                        size,
                    )
                })
                .collect(),
        )
    }

    fn compute_window_rects(
        windows: &[RuntimeWindowData],
        bounds: CGRect,
        kind: WindowLayoutKind,
    ) -> Option<Vec<CGRect>> {
        match kind {
            WindowLayoutKind::PreserveOriginal => {
                let layout = compute_window_layout_metrics(
                    windows,
                    bounds,
                    WINDOW_TILE_INSET,
                    WINDOW_TILE_SCALE_FACTOR,
                    Some(WINDOW_TILE_MAX_SCALE),
                )?;
                Some(
                    windows
                        .iter()
                        .map(|w| layout.rect_for(w, WINDOW_TILE_MIN_SIZE, WINDOW_TILE_GAP))
                        .collect(),
                )
            }
            WindowLayoutKind::Exploded => Self::compute_exploded_layout(windows, bounds),
        }
    }

    fn navigate_workspaces(
        visible: &[(usize, &RuntimeWorkspaceData)],
        current: usize,
        direction: NavDirection,
    ) -> Option<usize> {
        if visible.is_empty() {
            return None;
        }
        let len = visible.len();
        let current = current.min(len - 1);
        let cols = workspace_column_count(len);
        let rows = if len > cols { 2 } else { 1 };
        if rows == 1 {
            return Some(match direction {
                NavDirection::Left | NavDirection::Up => (current + len - 1) % len,
                NavDirection::Right | NavDirection::Down => (current + 1) % len,
            });
        }
        let row = current % rows;
        let col = current / rows;
        match direction {
            NavDirection::Up | NavDirection::Down => {
                let candidate = col * rows + (1 - row);
                Some(if candidate < len { candidate } else { current })
            }
            horizontal => {
                let step = if matches!(horizontal, NavDirection::Right) {
                    1
                } else {
                    cols - 1
                };
                let mut next_col = col;
                for _ in 0..cols {
                    next_col = (next_col + step) % cols;
                    let candidate = next_col * rows + row;
                    if candidate < len {
                        return Some(candidate);
                    }
                }
                Some(current)
            }
        }
    }

    fn navigate_windows(count: usize, current: usize, direction: NavDirection) -> Option<usize> {
        if count == 0 {
            return None;
        }
        let current = current.min(count - 1);
        Some(match direction {
            NavDirection::Left | NavDirection::Up => (current + count - 1) % count,
            NavDirection::Right | NavDirection::Down => (current + 1) % count,
        })
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

        if let Some(selection) = new_selection
            && state.selection() != Some(selection)
        {
            state.set_selection(selection);
            return true;
        }
        false
    }

    fn cycle_selection(&self, forward: bool) -> bool {
        let mut state = match self.state.try_borrow_mut() {
            Ok(state) => state,
            Err(_) => return false,
        };
        state.ensure_selection();
        let (len, workspace) = match state.mode() {
            Some(MissionControlMode::AllWorkspaces(workspaces)) => {
                (Self::visible_workspaces(workspaces).len(), true)
            }
            Some(MissionControlMode::CurrentWorkspace(windows)) => (windows.len(), false),
            None => return false,
        };
        if len == 0 {
            return false;
        }
        let current = state
            .selection()
            .map(|selection| match selection {
                Selection::Workspace(index) | Selection::Window(index) => index,
            })
            .unwrap_or(if forward { len - 1 } else { 0 })
            .min(len - 1);
        let next = if forward {
            (current + 1) % len
        } else {
            (current + len - 1) % len
        };
        let selection = if workspace {
            Selection::Workspace(next)
        } else {
            Selection::Window(next)
        };
        if state.selection() != Some(selection) {
            state.set_selection(selection);
            return true;
        }
        false
    }

    fn activate_selection_action(&self) {
        let action = {
            let mut state = self.state.borrow_mut();
            state.ensure_selection();
            let mode = state.mode();
            let selection = state.selection();

            match (mode, selection) {
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
                            let window_server_id = window.info.sys_id;
                            MissionControlAction::FocusWindow {
                                window_id: window.id,
                                window_server_id,
                            }
                        })
                    }
                }
                _ => None,
            }
        };

        if let Some(action) = action {
            self.emit_action(action);
        }
    }

    fn visible_workspaces(
        workspaces: &[RuntimeWorkspaceData],
    ) -> Vec<(usize, &RuntimeWorkspaceData)> {
        workspaces.iter().enumerate().filter(|(_, ws)| !ws.windows.is_empty()).collect()
    }

    fn draw_workspaces(
        &self,
        state: &RefCell<MissionControlState>,
        parent_layer: &CALayer,
        workspaces: &[RuntimeWorkspaceData],
        bounds: CGRect,
        selected: Option<usize>,
    ) {
        let visible = Self::visible_workspaces(workspaces);
        let Some(grid) = WorkspaceGrid::new(visible.len(), bounds) else {
            return;
        };
        let mut visible_ids: HashSet<String> = HashSet::default();
        visible_ids.reserve(visible.len());
        with_disabled_actions(|| {
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
                        (ws_layer, label_layer)
                    };
                    let text = CFString::from_str(&ws.name);
                    unsafe {
                        label_layer.setString(Some(&*(text.as_ref() as *const AnyObject)));
                    }
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
        });
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
        }
    }

    fn draw_windows_tile(
        &self,
        state: &RefCell<MissionControlState>,
        parent_layer: &CALayer,
        windows: &[RuntimeWindowData],
        tile: CGRect,
        selected: Option<usize>,
        layout: WindowLayoutKind,
    ) {
        let Some(rects) = Self::compute_window_rects(windows, tile, layout) else {
            return;
        };

        let selected_idx = selected.map(|s| s.min(windows.len().saturating_sub(1)));

        with_disabled_actions(|| {
            for idx in (0..windows.len()).rev() {
                autoreleasepool(|_| {
                    let window = &windows[idx];
                    let rect = rects[idx];
                    let is_selected = selected_idx == Some(idx);
                    let layer = {
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
                        layer
                    };

                    layer.setFrame(rect);
                    layer.setMasksToBounds(true);
                    layer.setCornerRadius(4.0);
                    layer.setContentsScale(self.scale);
                    if is_selected {
                        layer.setBorderColor(Some(&**SELECTED_BORDER_COLOR));
                        layer.setBorderWidth(3.0);
                        layer.setZPosition(1.0);
                    } else {
                        layer.setBorderColor(Some(&**WINDOW_BORDER_COLOR));
                        layer.setBorderWidth(0.4);
                        layer.setZPosition(0.0);
                    }
                });
            }
        });
    }

    fn capture_targets(&self, mode: &MissionControlMode) -> Vec<CaptureTarget> {
        let bounds = Self::content_bounds(CGRect::new(CGPoint::new(0.0, 0.0), self.frame.size));
        let mut targets = Vec::<CaptureTarget>::new();
        let mut target_indices: HashMap<WindowId, usize> = HashMap::default();
        let mut add = |window: &RuntimeWindowData, rect: CGRect| {
            let Some(window_server_id) = window.info.sys_id else {
                return;
            };
            let width = (rect.size.width * self.scale).ceil().clamp(2.0, 1200.0) as usize;
            let height = (rect.size.height * self.scale).ceil().clamp(2.0, 800.0) as usize;
            let target = CaptureTarget {
                window_id: window.id,
                window_server_id,
                width,
                height,
                revision: 0,
            };
            if let Some(&index) = target_indices.get(&window.id) {
                if let Some(old) = targets.get_mut(index) {
                    old.width = old.width.max(width);
                    old.height = old.height.max(height);
                }
            } else {
                target_indices.insert(window.id, targets.len());
                targets.push(target);
            }
        };

        match mode {
            MissionControlMode::AllWorkspaces(workspaces) => {
                let visible = Self::visible_workspaces(workspaces);
                if let Some(grid) = WorkspaceGrid::new(visible.len(), bounds) {
                    let mut capture_order = (0..visible.len()).collect::<Vec<_>>();
                    capture_order.sort_by_key(|&order| !visible[order].1.is_active);
                    for order in capture_order {
                        let workspace = visible[order].1;
                        let tile = grid.rect_for(order);
                        if let Some(rects) = Self::compute_window_rects(
                            &workspace.windows,
                            tile,
                            WindowLayoutKind::PreserveOriginal,
                        ) {
                            for (window, rect) in workspace.windows.iter().zip(rects) {
                                add(window, rect);
                            }
                        }
                    }
                }
            }
            MissionControlMode::CurrentWorkspace(windows) => {
                if let Some(rects) =
                    Self::compute_window_rects(windows, bounds, WindowLayoutKind::Exploded)
                {
                    for (window, rect) in windows.iter().zip(rects) {
                        add(window, rect);
                    }
                }
            }
        }
        targets
    }

    fn capture_previews(&self, mode: &MissionControlMode) {
        self.capture.capture(self.capture_targets(mode));
    }

    pub fn refresh_previews(&self) {
        let frames = self.capture.take_frames();
        if frames.is_empty() {
            return;
        }
        let state = self.state.borrow();
        let mut changed = false;
        with_disabled_actions(|| {
            for (id, frame) in frames {
                let Some(layer) = state.preview_layers.get(&id) else {
                    continue;
                };
                unsafe {
                    let surface = CFRetained::as_ptr(&frame.surface).as_ptr()
                        as *mut objc2::runtime::AnyObject;
                    let _: () = msg_send![&**layer, setContents: surface];
                }
                changed = true;
            }
        });
        if changed && *self.has_shown.borrow() {
            self.present();
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
                    state_cell,
                    parent_layer,
                    &workspaces,
                    content_bounds,
                    selected_workspace,
                );
            }
            MissionControlMode::CurrentWorkspace(windows) => {
                self.draw_windows_tile(
                    state_cell,
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

#[cfg(test)]
mod tests {
    use super::{MAX_CONCURRENT_CAPTURES, capture_batch_size};

    #[test]
    fn capture_batch_never_exceeds_pending_queue() {
        assert_eq!(capture_batch_size(3, 0), 0);
        assert_eq!(capture_batch_size(3, 1), 1);
        assert_eq!(capture_batch_size(0, 2), 2);
        assert_eq!(capture_batch_size(0, 20), MAX_CONCURRENT_CAPTURES);
        assert_eq!(capture_batch_size(MAX_CONCURRENT_CAPTURES, 20), 0);
    }
}

pub struct MissionControlOverlay {
    cgs_window: CgsWindow,
    _layer_context: Option<Retained<AnyObject>>,
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
    capture: CapturePipeline,
    scale: f64,
    coordinate_converter: CoordinateConverter,
}

impl MissionControlOverlay {
    pub fn new(
        config: Config,
        mtm: MainThreadMarker,
        frame: CGRect,
        scale: f64,
        preview_ready: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        let mut frame = frame;
        let mut scale = scale;
        let mut coordinate_converter = CoordinateConverter::default();

        let mut cache = ScreenCache::new(mtm);
        if let Some((screens, converter)) = cache.refresh() {
            coordinate_converter = converter;

            let active_space = get_active_space_number();
            if let Some(target) = screens
                .iter()
                .find(|screen| screen.space == active_space)
                .or_else(|| screens.first())
            {
                frame = CGDisplayBounds(target.id.as_u32());
                scale = NSScreen::screens(mtm)
                    .iter()
                    .find_map(|ns| {
                        let id = ns.get_number().ok()?;
                        if id == target.id {
                            Some(ns.backingScaleFactor())
                        } else {
                            None
                        }
                    })
                    .unwrap_or(scale);
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
        let layer_context = Self::host_layer(&cgs_window, &root_layer);

        Self {
            cgs_window,
            _layer_context: layer_context,
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
            capture: CapturePipeline::new(preview_ready),
            scale,
            coordinate_converter,
        }
    }

    fn host_layer(window: &CgsWindow, root: &CALayer) -> Option<Retained<AnyObject>> {
        let class = AnyClass::get(c"CAContext")?;
        let options = NSDictionary::<AnyObject, AnyObject>::new();
        unsafe {
            let raw: *mut AnyObject = msg_send![class, remoteContextWithOptions: &*options];
            let context = Retained::retain_autoreleased(raw)?;
            let _: () = msg_send![&*context, setLayer: root];
            window.bind_layer_context(Retained::as_ptr(&context).cast_mut().cast()).ok()?;
            CATransaction::flush();
            Some(context)
        }
    }

    #[inline]
    fn present(&self) {
        if self._layer_context.is_some() {
            CATransaction::flush();
        } else {
            render_layer_to_cgs_window(self.cgs_window.id(), self.frame.size, &self.root_layer);
        }
    }

    pub fn set_action_handler(&self, f: Rc<dyn Fn(MissionControlAction)>) {
        self.state.borrow_mut().on_action = Some(f);
    }

    fn current_screen_metrics(&self) -> (ScreenInfo, f64, CoordinateConverter) {
        let mut cache = ScreenCache::new(self.mtm);
        if let Some((screens, converter)) = cache.refresh() {
            let cursor = current_cursor_location().ok();
            let active_space = get_active_space_number();
            let center = CGPoint::new(
                self.frame.origin.x + self.frame.size.width / 2.0,
                self.frame.origin.y + self.frame.size.height / 2.0,
            );
            let selected = cursor
                .and_then(|point| {
                    screens
                        .iter()
                        .find(|screen| CGDisplayBounds(screen.id.as_u32()).contains(point))
                })
                .or_else(|| screens.iter().find(|screen| screen.space == active_space))
                .or_else(|| {
                    screens
                        .iter()
                        .find(|screen| CGDisplayBounds(screen.id.as_u32()).contains(center))
                })
                .or_else(|| screens.first());
            if let Some(screen) = selected {
                let scale = NSScreen::screens(self.mtm)
                    .iter()
                    .find_map(|candidate| {
                        (candidate.get_number().ok()? == screen.id)
                            .then(|| candidate.backingScaleFactor())
                    })
                    .unwrap_or(self.scale);
                return (screen.clone(), scale, converter);
            }
        }

        (
            ScreenInfo {
                id: ScreenId::new(0),
                frame: self.frame,
                display_uuid: String::new(),
                name: None,
                space: None,
            },
            self.scale,
            self.coordinate_converter,
        )
    }

    pub fn update(&self, mode: MissionControlMode) {
        self.stop_active_fade();
        *self.pending_hide.borrow_mut() = false;
        self.capture.clear();

        {
            let (screen, scale, converter) = self.current_screen_metrics();
            let screen_id = screen.id.as_u32();
            let new_frame = if screen_id == 0 {
                self.frame
            } else {
                CGDisplayBounds(screen_id)
            };
            let new_scale = scale;

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
                (*me).coordinate_converter = converter;
            }
        }

        {
            let mut st = self.state.borrow_mut();
            st.set_mode(mode.clone());
        }

        if self.fade_enabled && !*self.has_shown.borrow() {
            let _ = self.cgs_window.set_alpha(0.0);
        } else {
            let _ = self.cgs_window.set_alpha(1.0);
        }
        let app = NSApplication::sharedApplication(self.mtm);
        app.activate();
        self.ensure_key_tap();

        self.draw_and_present();
        let _ = self.cgs_window.order_above(None);
        self.capture_previews(&mode);

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
            self.capture.clear();

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
                let matches = slot.as_ref().is_some_and(|state| state.id == fade_id);
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

    fn draw_and_present(&self) {
        with_disabled_actions(|| {
            self.root_layer.setFrame(CGRect::new(CGPoint::new(0.0, 0.0), self.frame.size));
            self.root_layer.setGeometryFlipped(true);

            self.draw_contents_into_layer(
                CGRect::new(CGPoint::new(0.0, 0.0), self.frame.size),
                &self.root_layer,
            );
        });

        self.present();
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
        match keycode {
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
        }
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

        if let Some(sel) = new_sel
            && state.selection() != Some(sel)
        {
            state.set_selection(sel);
            drop(state);
            self.draw_and_present();
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
