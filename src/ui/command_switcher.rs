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
use objc2_app_kit::{NSApplication, NSColor, NSPopUpMenuWindowLevel};
use objc2_core_foundation::{CFRetained, CFString, CFType, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGColor, CGContext, CGEvent, CGEventField, CGEventTapOptions, CGEventTapProxy, CGEventType,
};
use objc2_foundation::MainThreadMarker;
use objc2_quartz_core::{CALayer, CATextLayer, CATransaction};
use once_cell::sync::Lazy;
use parking_lot::{Mutex, RwLock};

use crate::actor::app::WindowId;
use crate::common::collections::{HashMap, HashSet};
use crate::common::config::Config;
use crate::model::server::{WindowData, WorkspaceData};
use crate::sys::cgs_window::CgsWindow;
use crate::sys::dispatch::DispatchExt;
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

// Colors aligned with Mission Control overlay for consistent design language.
static WORKSPACE_BACKGROUND_COLOR: Lazy<Retained<CGColor>> =
    Lazy::new(|| CGColor::new_generic_gray(0.0, 0.25).into());
static SELECTED_BORDER_COLOR: Lazy<Retained<CGColor>> =
    Lazy::new(|| CGColor::new_generic_rgb(0.2, 0.45, 1.0, 0.85).into());
static WORKSPACE_BORDER_COLOR: Lazy<Retained<CGColor>> =
    Lazy::new(|| CGColor::new_generic_gray(1.0, 0.12).into());
static WINDOW_BORDER_COLOR: Lazy<Retained<CGColor>> =
    Lazy::new(|| CGColor::new_generic_gray(0.0, 0.65).into());

// Switcher-specific aliases (kept for clarity)
static ITEM_BG_COLOR: Lazy<Retained<CGColor>> =
    Lazy::new(|| CGColor::new_generic_gray(1.0, 0.03).into());
static ITEM_SELECTED_BG_COLOR: Lazy<Retained<CGColor>> =
    Lazy::new(|| CGColor::new_generic_gray(1.0, 0.03).into());
static ITEM_LABEL_COLOR: Lazy<Retained<CGColor>> = Lazy::new(|| NSColor::labelColor().CGColor());
const BASE_ITEM_WIDTH: f64 = 240.0;
const BASE_ITEM_HEIGHT: f64 = 170.0;
const ITEM_SPACING: f64 = 28.0;
const CONTAINER_PADDING: f64 = 32.0;
const LABEL_HEIGHT: f64 = 20.0;
const MAX_CONTAINER_WIDTH_RATIO: f64 = 0.82;
const MAX_CONTAINER_HEIGHT_RATIO: f64 = 0.88;
const WINDOW_TILE_INSET: f64 = 3.0;
const WINDOW_TILE_GAP: f64 = 1.0;
const WINDOW_TILE_MIN_SIZE: f64 = 2.0;
const WINDOW_TILE_SCALE_FACTOR: f64 = 0.75;
const WINDOW_TILE_MAX_SCALE: f64 = 1.0;
const PREVIEW_MAX_EDGE: f64 = 420.0;
const PREVIEW_MIN_EDGE: f64 = 96.0;
// Do a small synchronous capture pass to reduce initial pop-in
const SYNC_PREWARM_LIMIT: usize = 3;
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
    let (tx, rx) = unbounded::<CaptureJob>();
    let mut worker_count = std::thread::available_parallelism()
        .map(|n| n.get().saturating_sub(1))
        .unwrap_or(2);
    worker_count = worker_count.max(2).min(6);
    for _ in 0..worker_count {
        let rx = rx.clone();
        std::thread::spawn(move || {
            while let Ok(job) = rx.recv() {
                if job.generation != CURRENT_GENERATION.load(Ordering::Acquire) {
                    if let Some(mut set) = IN_FLIGHT.try_lock() {
                        set.remove(&(job.generation, job.task.window_id.clone()));
                    }
                    continue;
                }

                if let Some(img) = crate::sys::window_server::capture_window_image(
                    WindowServerId::new(job.task.window_server_id),
                    job.task.target_w,
                    job.task.target_h,
                ) {
                    {
                        let mut cache = job.cache.write();
                        cache.insert(job.task.window_id.clone(), img);
                    }
                    if let Some(mut set) = IN_FLIGHT.try_lock() {
                        set.remove(&(job.generation, job.task.window_id.clone()));
                    }
                    if let Some(overlay) =
                        unsafe { (job.overlay_ptr_bits as *const CommandSwitcherOverlay).as_ref() }
                    {
                        overlay.request_refresh();
                    }
                } else if let Some(mut set) = IN_FLIGHT.try_lock() {
                    set.remove(&(job.generation, job.task.window_id.clone()));
                }
            }
        });
    }

    CapturePool { sender: tx }
});

#[derive(Clone)]
enum SwitcherItemKind {
    Window(WindowData),
    Workspace(WorkspaceData),
}

#[derive(Clone)]
struct SwitcherItem {
    key: ItemKey,
    label: String,
    kind: SwitcherItemKind,
    is_primary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ItemKey {
    Window(WindowId),
    Workspace(String),
}

type PreviewLayerKey = (ItemKey, Option<WindowId>);

struct PreviewLayerEntry {
    layer: Retained<CALayer>,
    window_id: Option<WindowId>,
}

impl PreviewLayerEntry {
    fn new(layer: Retained<CALayer>, window_id: Option<WindowId>) -> Self {
        Self { layer, window_id }
    }

    fn layer(&self) -> &Retained<CALayer> { &self.layer }

    fn window_id(&self) -> Option<&WindowId> { self.window_id.as_ref() }

    fn set_window_id(&mut self, window_id: Option<WindowId>) { self.window_id = window_id; }
}

#[derive(Debug, Clone)]
pub enum CommandSwitcherMode {
    CurrentWorkspace(Vec<WindowData>),
    AllWindows(Vec<WindowData>),
    Workspaces(Vec<WorkspaceData>),
}

#[derive(Debug, Clone)]
pub enum CommandSwitcherAction {
    FocusWindow {
        window_id: WindowId,
        window_server_id: Option<WindowServerId>,
    },
    SwitchToWorkspace(usize),
    Dismiss,
}

struct CommandSwitcherState {
    mode: Option<CommandSwitcherMode>,
    items: Vec<SwitcherItem>,
    selection: Option<usize>,
    on_action: Option<Rc<dyn Fn(CommandSwitcherAction)>>,
    preview_cache: Arc<RwLock<HashMap<WindowId, CapturedWindowImage>>>,
    preview_layers: HashMap<PreviewLayerKey, PreviewLayerEntry>,
    label_layers: HashMap<ItemKey, Retained<CATextLayer>>,
    // Cache CFStrings to avoid reallocating on every draw
    label_strings: HashMap<ItemKey, CachedText>,
    item_layers: HashMap<ItemKey, Retained<CALayer>>,
    // Track selection style to avoid redundant layer property updates
    item_styles: HashMap<ItemKey, ItemLayerStyle>,
    ready_previews: HashSet<WindowId>,
    item_frames: Vec<(ItemKey, CGRect)>,
    grid_columns: usize,
    grid_rows: usize,
}

impl Default for CommandSwitcherState {
    fn default() -> Self {
        Self {
            mode: None,
            items: Vec::new(),
            selection: None,
            on_action: None,
            preview_cache: Arc::new(RwLock::new(HashMap::default())),
            preview_layers: HashMap::default(),
            label_layers: HashMap::default(),
            label_strings: HashMap::default(),
            item_layers: HashMap::default(),
            item_styles: HashMap::default(),
            ready_previews: HashSet::default(),
            item_frames: Vec::new(),
            grid_columns: 0,
            grid_rows: 0,
        }
    }
}

pub struct CommandSwitcherOverlay {
    cgs_window: CgsWindow,
    root_layer: Retained<CALayer>,
    container_layer: Retained<CALayer>,
    frame: CGRect,
    scale: f64,
    mtm: MainThreadMarker,
    state: RefCell<CommandSwitcherState>,
    key_tap: RefCell<Option<crate::sys::event_tap::EventTap>>,
    refresh_pending: AtomicBool,
    has_shown: RefCell<bool>,
    fade_enabled: bool,
    fade_duration_ms: f64,
}

impl CommandSwitcherState {
    fn set_mode(&mut self, mode: CommandSwitcherMode) {
        self.mode = Some(mode.clone());
        self.items.clear();
        self.selection = None;
        self.item_frames.clear();
        self.ready_previews.clear();
        CURRENT_GENERATION.fetch_add(1, Ordering::AcqRel);

        self.item_layers.retain(|_, layer| {
            layer.removeFromSuperlayer();
            false
        });
        self.label_layers.retain(|_, layer| {
            layer.removeFromSuperlayer();
            false
        });
        self.label_strings.clear();
        self.preview_layers.retain(|_, entry| {
            entry.layer().removeFromSuperlayer();
            false
        });
        self.item_styles.clear();
        self.grid_columns = 0;
        self.grid_rows = 0;

        match mode {
            CommandSwitcherMode::CurrentWorkspace(windows)
            | CommandSwitcherMode::AllWindows(windows) => {
                for window in windows {
                    let key = ItemKey::Window(window.id);
                    let label = format_window_label(&window);
                    let is_primary = window.is_focused;
                    self.items.push(SwitcherItem {
                        key,
                        label,
                        kind: SwitcherItemKind::Window(window),
                        is_primary,
                    });
                }
            }
            CommandSwitcherMode::Workspaces(workspaces) => {
                for workspace in workspaces {
                    let key = ItemKey::Workspace(workspace.id.clone());
                    let label = format_workspace_label(&workspace);
                    let is_primary = workspace.is_active;
                    self.items.push(SwitcherItem {
                        key,
                        label,
                        kind: SwitcherItemKind::Workspace(workspace),
                        is_primary,
                    });
                }
            }
        }
        self.prune_preview_cache();
        self.ensure_selection();
    }

    fn ensure_selection(&mut self) {
        if self.selection.is_some() {
            let idx = self.selection.unwrap();
            if idx < self.items.len() {
                return;
            }
        }
        let desired = self
            .items
            .iter()
            .enumerate()
            .find_map(|(idx, item)| item.is_primary.then_some(idx))
            .or_else(|| if self.items.is_empty() { None } else { Some(0) });
        self.selection = desired;
    }

    fn prune_preview_cache(&mut self) {
        let mut cache = self.preview_cache.write();
        if cache.is_empty() && self.preview_layers.is_empty() && self.ready_previews.is_empty() {
            return;
        }

        let mut valid: HashSet<WindowId> = HashSet::default();
        for item in &self.items {
            match &item.kind {
                SwitcherItemKind::Window(window) => {
                    valid.insert(window.id);
                }
                SwitcherItemKind::Workspace(workspace) => {
                    for window in &workspace.windows {
                        valid.insert(window.id);
                    }
                }
            }
        }

        cache.retain(|wid, _| valid.contains(wid));

        let mut to_remove: Vec<PreviewLayerKey> = Vec::new();
        for (key, entry) in self.preview_layers.iter() {
            if let Some(wid) = entry.window_id() {
                if !valid.contains(wid) {
                    entry.layer().removeFromSuperlayer();
                    to_remove.push(key.clone());
                }
            }
        }
        for key in to_remove {
            self.preview_layers.remove(&key);
        }

        self.ready_previews.retain(|wid| valid.contains(wid));
    }

    fn selection(&self) -> Option<usize> { self.selection }

    fn set_selection(&mut self, idx: usize) {
        if idx < self.items.len() {
            self.selection = Some(idx);
        }
    }

    fn selected_item(&self) -> Option<&SwitcherItem> {
        self.selection.and_then(|idx| self.items.get(idx))
    }
}

fn format_window_label(window: &WindowData) -> String {
    let mut title = window.title.trim().to_string();
    if title.is_empty() {
        if let Some(bundle) = &window.bundle_id {
            title = bundle.clone();
        } else {
            title = "Untitled Window".into();
        }
    }
    title
}

fn format_workspace_label(workspace: &WorkspaceData) -> String {
    let label = workspace.name.trim();
    if label.is_empty() {
        format!("Workspace {}", workspace.index + 1)
    } else {
        label.to_string()
    }
}

extern "C" fn refresh_coalesced_cb(ctx: *mut c_void) {
    if ctx.is_null() {
        return;
    }
    let overlay = unsafe { &*(ctx as *const CommandSwitcherOverlay) };
    overlay.refresh_pending.store(false, Ordering::Release);
    overlay.refresh_from_capture();
}

impl CommandSwitcherOverlay {
    pub fn new(_config: Config, mtm: MainThreadMarker, frame: CGRect, scale: f64) -> Self {
        let root_layer = CALayer::layer();
        root_layer.setGeometryFlipped(true);
        root_layer.setFrame(CGRect::new(CGPoint::new(0.0, 0.0), frame.size));
        root_layer.setContentsScale(scale);
        root_layer.setBackgroundColor(None);
        root_layer.setMasksToBounds(false);

        let container = CALayer::layer();
        container.setGeometryFlipped(true);

        container.setCornerRadius(10.0);
        container.setMasksToBounds(false);
        container.setBackgroundColor(Some(&**WORKSPACE_BACKGROUND_COLOR));
        container.setBorderWidth(0.6);
        container.setBorderColor(Some(&**WINDOW_BORDER_COLOR));
        root_layer.addSublayer(&container);

        let cgs_window = CgsWindow::new(frame).expect("failed to create CGS window");
        let _ = cgs_window.set_resolution(scale);
        let _ = cgs_window.set_opacity(false);
        let _ = cgs_window.set_alpha(0.0);
        let _ = cgs_window.set_level(NSPopUpMenuWindowLevel as i32);
        let _ = cgs_window.set_blur(30, None);

        Self {
            cgs_window,
            root_layer,
            container_layer: container,
            frame,
            scale,
            mtm,
            state: RefCell::new(CommandSwitcherState::default()),
            key_tap: RefCell::new(None),
            refresh_pending: AtomicBool::new(false),
            has_shown: RefCell::new(false),
            // Simple fade-in to appear smoothly
            fade_enabled: true,
            fade_duration_ms: 160.0,
        }
    }

    fn request_refresh(&self) {
        if !self.refresh_pending.swap(true, Ordering::AcqRel) {
            let ptr = self as *const _ as usize;
            queue::main().after_f(
                Time::new_after(Time::NOW, 6000000),
                ptr as *mut c_void,
                refresh_coalesced_cb,
            );
        }
    }

    pub fn set_action_handler(&self, f: Rc<dyn Fn(CommandSwitcherAction)>) {
        self.state.borrow_mut().on_action = Some(f);
    }

    pub fn update(&self, mode: CommandSwitcherMode) {
        {
            let (new_frame, new_scale) =
                if let Some(screen) = objc2_app_kit::NSScreen::mainScreen(self.mtm) {
                    (screen.frame(), screen.backingScaleFactor())
                } else {
                    (self.frame, self.scale)
                };

            let frame_changed = new_frame.origin.x != self.frame.origin.x
                || new_frame.origin.y != self.frame.origin.y
                || new_frame.size.width != self.frame.size.width
                || new_frame.size.height != self.frame.size.height;
            let scale_changed = (new_scale - self.scale).abs() > f64::EPSILON;

            if frame_changed || scale_changed {
                let _ = self.cgs_window.set_shape(new_frame);
                let _ = self.cgs_window.set_resolution(new_scale);

                unsafe {
                    let me = self as *const _ as *mut CommandSwitcherOverlay;
                    (*me).frame = new_frame;
                    (*me).scale = new_scale;
                }

                self.root_layer.setFrame(CGRect::new(CGPoint::new(0.0, 0.0), self.frame.size));
                self.root_layer.setContentsScale(self.scale);
            }
        }

        {
            let mut state = self.state.borrow_mut();
            state.set_mode(mode);
        }
        self.prewarm_previews();
        // Start transparent if we're about to fade in
        if self.fade_enabled && !*self.has_shown.borrow() {
            let _ = self.cgs_window.set_alpha(0.0);
        }
        let _ = self.cgs_window.set_alpha(1.0);
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
        if !*self.has_shown.borrow() {
            return;
        }
        *self.has_shown.borrow_mut() = false;
        let _ = self.cgs_window.set_alpha(0.0);
        let _ = self.cgs_window.order_out();
        if let Some(tap) = self.key_tap.borrow_mut().take() {
            drop(tap);
        }
    }

    pub fn select_next(&self) {
        if self.adjust_selection(1) {
            self.present_root_layer();
        }
    }

    pub fn select_prev(&self) {
        if self.adjust_selection(-1) {
            self.present_root_layer();
        }
    }

    pub fn activate_selection(&self) {
        let action = {
            let state = self.state.borrow();
            match state.selected_item() {
                Some(item) => match &item.kind {
                    SwitcherItemKind::Window(window) => {
                        let wsid = window.window_server_id.map(WindowServerId::new);
                        CommandSwitcherAction::FocusWindow {
                            window_id: window.id,
                            window_server_id: wsid,
                        }
                    }
                    SwitcherItemKind::Workspace(workspace) => {
                        CommandSwitcherAction::SwitchToWorkspace(workspace.index)
                    }
                },
                None => CommandSwitcherAction::Dismiss,
            }
        };
        self.emit_action(action);
    }

    pub fn dismiss(&self) { self.emit_action(CommandSwitcherAction::Dismiss); }

    fn adjust_selection(&self, delta: isize) -> bool {
        let (len, current) = {
            let state = match self.state.try_borrow() {
                Ok(s) => s,
                Err(_) => return false,
            };
            if state.items.is_empty() {
                return false;
            }
            (state.items.len(), state.selection().unwrap_or(0))
        };

        let len_isize = len as isize;
        if len_isize == 0 {
            return false;
        }

        let mut idx = (current as isize + delta) % len_isize;
        if idx < 0 {
            idx += len_isize;
        }

        self.set_selection_index(idx as usize)
    }

    fn adjust_selection_vertical(&self, delta_rows: isize) -> bool {
        let (len, current, columns, rows) = {
            let state = match self.state.try_borrow() {
                Ok(s) => s,
                Err(_) => return false,
            };
            if state.items.is_empty() || state.grid_columns == 0 {
                return false;
            }
            (
                state.items.len(),
                state.selection().unwrap_or(0),
                state.grid_columns,
                state.grid_rows.max(1),
            )
        };

        if columns == 0 {
            return false;
        }

        let current_row = current / columns;
        let current_col = current % columns;
        let mut target_row = current_row as isize + delta_rows;
        if target_row < 0 || target_row as usize >= rows {
            return false;
        }

        let target_row_usize = target_row as usize;
        let row_start = target_row_usize * columns;
        if row_start >= len {
            return false;
        }
        let row_end = ((target_row_usize + 1) * columns).min(len);
        let target_idx = (row_start + current_col).min(row_end.saturating_sub(1));

        self.set_selection_index(target_idx)
    }

    fn set_selection_index(&self, new_idx: usize) -> bool {
        let (old_key, new_key) = {
            let mut state = match self.state.try_borrow_mut() {
                Ok(s) => s,
                Err(_) => return false,
            };
            if state.items.is_empty() || new_idx >= state.items.len() {
                return false;
            }
            let previous = state.selection();
            if previous == Some(new_idx) {
                return false;
            }
            let new_key = state.items[new_idx].key.clone();
            let old_key = previous.and_then(|idx| state.items.get(idx).map(|it| it.key.clone()));
            state.set_selection(new_idx);
            (old_key, new_key)
        };

        if let Some(ok) = old_key {
            self.update_item_selected_style(&ok, false);
        }
        self.update_item_selected_style(&new_key, true);
        true
    }

    fn emit_action(&self, action: CommandSwitcherAction) {
        let handler = self.state.borrow().on_action.clone();
        let Some(cb) = handler else {
            return;
        };

        type Ctx = (Rc<dyn Fn(CommandSwitcherAction)>, CommandSwitcherAction);

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

    fn refresh_from_capture(&self) {
        if !*self.has_shown.borrow() {
            return;
        }
        self.refresh_previews();
    }

    fn draw_and_present(&self) {
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        self.root_layer.setFrame(CGRect::new(CGPoint::new(0.0, 0.0), self.frame.size));
        self.root_layer.setContentsScale(self.scale);
        self.root_layer.setGeometryFlipped(true);

        self.draw_items();

        CATransaction::commit();

        self.present_root_layer();
    }

    fn present_root_layer(&self) {
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

    fn fade_in(&self) {
        let duration_ms = self.fade_duration_ms.max(0.0);
        if duration_ms <= 0.0 {
            return;
        }
        CATransaction::begin();
        CATransaction::setAnimationDuration(duration_ms / 1000.0);
        self.root_layer.setOpacity(0.0);
        self.root_layer.setOpacity(1.0);
        CATransaction::commit();
    }

    fn refresh_previews(&self) {
        if !*self.has_shown.borrow() {
            return;
        }

        let (layers, cache_arc) = {
            let state = match self.state.try_borrow() {
                Ok(s) => s,
                Err(_) => return,
            };
            let pairs: Vec<(WindowId, Retained<CALayer>)> = state
                .preview_layers
                .iter()
                .filter_map(|(_, entry)| {
                    entry.window_id().copied().map(|wid| (wid, entry.layer().clone()))
                })
                .collect();
            (pairs, state.preview_cache.clone())
        };

        if layers.is_empty() {
            return;
        }

        let mut ready_ids: Vec<WindowId> = Vec::with_capacity(layers.len());

        CATransaction::begin();
        CATransaction::setDisableActions(true);
        {
            let cache = cache_arc.read();
            for (wid, layer) in layers.iter() {
                if let Some(img) = cache.get(wid) {
                    unsafe {
                        let img_ptr = img.as_ptr() as *mut AnyObject;
                        let _: () = msg_send![&**layer, setContents: img_ptr];
                    }
                    ready_ids.push(*wid);
                }
            }
        }
        CATransaction::commit();

        if ready_ids.is_empty() {
            return;
        }

        if let Ok(mut state) = self.state.try_borrow_mut() {
            for wid in ready_ids.iter().copied() {
                state.ready_previews.insert(wid);
            }
        }

        self.present_root_layer();
    }

    fn prewarm_previews(&self) {
        let mut tasks: Vec<(u8, i64, CaptureTask)> = {
            let state = self.state.borrow();
            let mut pending = Vec::with_capacity(state.items.len().saturating_mul(2));
            for item in &state.items {
                match &item.kind {
                    SwitcherItemKind::Window(window) => {
                        if let Some(wsid) = window.window_server_id {
                            let priority: u8 = if item.is_primary || window.is_focused {
                                0
                            } else {
                                1
                            };
                            let area = (window.frame.size.width * window.frame.size.height) as i64;
                            let (target_w, target_h) = capture_target_for_window(window);
                            pending.push((priority, area, CaptureTask {
                                window_id: window.id,
                                window_server_id: wsid,
                                target_w,
                                target_h,
                            }));
                        }
                    }
                    SwitcherItemKind::Workspace(workspace) => {
                        let base_priority: u8 = if item.is_primary { 0 } else { 1 };
                        for window in &workspace.windows {
                            if let Some(wsid) = window.window_server_id {
                                let focus_bonus: u8 = if window.is_focused { 0 } else { 1 };
                                let priority = base_priority.saturating_add(focus_bonus);
                                let area =
                                    (window.frame.size.width * window.frame.size.height) as i64;
                                let (target_w, target_h) = capture_target_for_window(window);
                                pending.push((priority, area, CaptureTask {
                                    window_id: window.id,
                                    window_server_id: wsid,
                                    target_w,
                                    target_h,
                                }));
                            }
                        }
                    }
                }
            }
            pending
        };

        if tasks.is_empty() {
            return;
        }

        // Prioritize lower priority value first, then larger area first
        tasks.sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(&a.1)));

        let generation = CURRENT_GENERATION.fetch_add(1, Ordering::AcqRel) + 1;

        let (cache, overlay_ptr_bits) = {
            let state = self.state.borrow();
            (state.preview_cache.clone(), self as *const _ as usize)
        };

        let sync_limit = SYNC_PREWARM_LIMIT.min(tasks.len());
        let mut async_tasks = tasks.split_off(sync_limit);
        let sync_tasks = tasks; // first N by priority/area

        // Synchronous prewarm: capture a few highest-priority previews immediately
        for (_, _, task) in sync_tasks.into_iter() {
            {
                let cache_read = cache.read();
                if cache_read.contains_key(&task.window_id) {
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
                        let mut cache_write = cache.write();
                        cache_write.insert(task.window_id, img);
                    }
                    {
                        let mut set = IN_FLIGHT.lock();
                        set.remove(&(generation, task.window_id));
                    }
                    if let Ok(mut state) = self.state.try_borrow_mut() {
                        state.ready_previews.insert(task.window_id);
                    }
                    self.request_refresh();
                }
                None => {
                    let mut set = IN_FLIGHT.lock();
                    set.remove(&(generation, task.window_id));
                }
            }
        }

        // Remaining tasks: dispatch to background workers
        for (_, _, task) in async_tasks.drain(..) {
            {
                let cache_read = cache.read();
                if cache_read.contains_key(&task.window_id) {
                    continue;
                }
            }
            let mut guard = IN_FLIGHT.lock();
            if !guard.insert((generation, task.window_id)) {
                continue;
            }
            drop(guard);
            let job = CaptureJob {
                task,
                cache: cache.clone(),
                generation,
                overlay_ptr_bits,
            };
            let _ = CAPTURE_POOL.sender.send(job);
        }
    }

    fn draw_items(&self) {
        let mut state = self.state.borrow_mut();
        let item_count = state.items.len();
        let layout = compute_layout(item_count, self.frame.size);
        state.grid_columns = layout.columns;
        state.grid_rows = layout.rows;
        self.container_layer.setFrame(layout.container_frame);
        // follow mission_control style: subtle tinted backdrop card with light border & gentle shadow
        self.container_layer.setBackgroundColor(Some(&**WORKSPACE_BACKGROUND_COLOR));
        self.container_layer.setBorderWidth(0.6);
        self.container_layer.setBorderColor(Some(&**WORKSPACE_BORDER_COLOR));
        self.container_layer.setMasksToBounds(false);
        self.container_layer.setContentsScale(self.scale);

        state.item_frames.clear();
        state.item_frames.reserve(item_count);

        let container_origin = layout.container_frame.origin;
        let mut visible_items: HashSet<ItemKey> = HashSet::default(); //with_capacity(item_count);
        let mut active_preview_keys: HashSet<(ItemKey, Option<WindowId>)> = HashSet::default(); //with_capacity(item_count.saturating_mul(2));
        let label_color = &**ITEM_LABEL_COLOR;
        // Reuse CFStrings within this pass to avoid redundant allocations
        let align_left = CFString::from_static_str("left");
        let trunc_end = CFString::from_static_str("end");

        for idx in 0..item_count {
            autoreleasepool(|_| {
                let item = state.items[idx].clone();
                let Some(item_frame) = layout.item_frames.get(idx) else {
                    return;
                };
                let is_selected = state.selection() == Some(idx);
                let key = item.key.clone();
                visible_items.insert(key.clone());

                let item_layer = state
                    .item_layers
                    .entry(key.clone())
                    .or_insert_with(|| {
                        let layer = CALayer::layer();
                        layer.setGeometryFlipped(true);
                        layer.setMasksToBounds(false);
                        self.container_layer.addSublayer(&layer);
                        layer
                    })
                    .clone();
                item_layer.setFrame(item_frame.item_frame);
                item_layer.setCornerRadius(12.0);
                item_layer.setContentsScale(self.scale);
                item_layer.setZPosition(0.0);
                // Only update style when selection changed for this key
                let style_changed = state
                    .item_styles
                    .entry(key.clone())
                    .or_insert_with(Default::default)
                    .update_selected(is_selected);
                if style_changed {
                    let bg_color = if is_selected {
                        &**ITEM_SELECTED_BG_COLOR
                    } else {
                        &**ITEM_BG_COLOR
                    };
                    item_layer.setBackgroundColor(Some(bg_color));
                    item_layer.setBorderWidth(if is_selected { 3.0 } else { 1.0 });
                    item_layer.setBorderColor(Some(if is_selected {
                        &**SELECTED_BORDER_COLOR
                    } else {
                        &**WORKSPACE_BORDER_COLOR
                    }));
                }

                let label_layer = state
                    .label_layers
                    .entry(key.clone())
                    .or_insert_with(|| {
                        let layer = CATextLayer::layer();
                        layer.setContentsScale(self.scale);
                        layer.setGeometryFlipped(true);
                        self.container_layer.addSublayer(&layer);
                        layer
                    })
                    .clone();
                label_layer.setFrame(item_frame.label_frame);
                label_layer.setAlignmentMode(align_left.as_ref());
                label_layer.setForegroundColor(Some(label_color));
                label_layer.setFontSize((12.0 * layout.scale).clamp(10.5, 13.0));
                label_layer.setTruncationMode(trunc_end.as_ref());
                label_layer.setWrapped(false);
                label_layer.setZPosition(3.0);
                // Cache CFString content; only update when changed
                self.update_text_layer_cached(&mut state, &key, &label_layer, &item.label);

                match &item.kind {
                    SwitcherItemKind::Window(window) => {
                        let key = self.draw_window_preview(
                            &mut state,
                            &key,
                            window,
                            item_frame.preview_frame,
                            is_selected,
                        );
                        active_preview_keys.insert(key);
                    }
                    SwitcherItemKind::Workspace(workspace) => {
                        for key in self.draw_workspace_preview(
                            &mut state,
                            &key,
                            workspace,
                            item_frame.preview_frame,
                            is_selected,
                        ) {
                            active_preview_keys.insert(key);
                        }
                    }
                }

                let stored_frame = CGRect::new(
                    CGPoint::new(
                        container_origin.x + item_frame.item_frame.origin.x,
                        container_origin.y + item_frame.item_frame.origin.y,
                    ),
                    item_frame.item_frame.size,
                );
                state.item_frames.push((key.clone(), stored_frame));
            });
        }

        state.item_layers.retain(|key, layer| {
            if visible_items.contains(key) {
                true
            } else {
                layer.removeFromSuperlayer();
                false
            }
        });
        state.label_layers.retain(|key, layer| {
            if visible_items.contains(key) {
                true
            } else {
                layer.removeFromSuperlayer();
                false
            }
        });
        state.label_strings.retain(|key, _| visible_items.contains(key));
        state.preview_layers.retain(|key, entry| {
            if active_preview_keys.contains(key) {
                true
            } else {
                entry.layer().removeFromSuperlayer();
                false
            }
        });
    }

    fn update_text_layer_cached(
        &self,
        state: &mut CommandSwitcherState,
        key: &ItemKey,
        layer: &CATextLayer,
        text: &str,
    ) {
        use crate::common::collections::hash_map;
        match state.label_strings.entry(key.clone()) {
            hash_map::Entry::Occupied(mut occ) => {
                if occ.get_mut().update(text) {
                    occ.get().apply_to(layer);
                }
            }
            hash_map::Entry::Vacant(v) => {
                let cache = CachedText::new(text);
                cache.apply_to(layer);
                v.insert(cache);
            }
        }
    }

    fn draw_window_preview(
        &self,
        state: &mut CommandSwitcherState,
        item_key: &ItemKey,
        window: &WindowData,
        frame: CGRect,
        selected: bool,
    ) -> (ItemKey, Option<WindowId>) {
        let key = (item_key.clone(), None);
        let entry = state.preview_layers.entry(key.clone()).or_insert_with(|| {
            let layer = CALayer::layer();
            layer.setGeometryFlipped(true);
            layer.setMasksToBounds(true);
            self.container_layer.addSublayer(&layer);
            PreviewLayerEntry::new(layer, Some(window.id))
        });
        entry.set_window_id(Some(window.id));
        let layer = entry.layer().clone();
        layer.setFrame(frame);
        layer.setCornerRadius(if selected { 9.0 } else { 8.0 });
        // Keep a subtle background while previews load; focus indication happens on the outer card
        let bg_color = &**ITEM_BG_COLOR;
        layer.setBackgroundColor(Some(bg_color));
        layer.setBorderWidth(0.4);
        layer.setBorderColor(Some(&**WINDOW_BORDER_COLOR));
        layer.setContentsScale(self.scale);
        layer.setZPosition(2.0);

        let maybe_img_ptr = {
            let cache = state.preview_cache.read();
            cache.get(&window.id).map(|img| img.as_ptr() as *mut AnyObject)
        };

        let mut had_image = false;
        if let Some(img_ptr) = maybe_img_ptr {
            unsafe {
                let _: () = msg_send![&**layer, setContents: img_ptr];
            }
            state.ready_previews.insert(window.id);
            had_image = true;
        } else if state.ready_previews.contains(&window.id) {
            had_image = true;
        }

        if !had_image {
            let (tw, th) = capture_target_for_rect(frame);
            self.schedule_capture(state, window, tw, th);
        }
        key
    }

    fn draw_workspace_preview(
        &self,
        state: &mut CommandSwitcherState,
        item_key: &ItemKey,
        workspace: &WorkspaceData,
        frame: CGRect,
        selected: bool,
    ) -> Vec<(ItemKey, Option<WindowId>)> {
        let key = (item_key.clone(), None);
        let container_entry = state.preview_layers.entry(key.clone()).or_insert_with(|| {
            let layer = CALayer::layer();
            layer.setGeometryFlipped(true);
            layer.setMasksToBounds(true);
            self.container_layer.addSublayer(&layer);
            PreviewLayerEntry::new(layer, None)
        });
        container_entry.set_window_id(None);
        let container = container_entry.layer().clone();
        container.setFrame(frame);
        container.setCornerRadius(if selected { 9.0 } else { 8.0 });
        container.setBorderWidth(0.0);
        container.setBorderColor(None);
        let bg_color = if selected {
            &**ITEM_SELECTED_BG_COLOR
        } else {
            &**ITEM_BG_COLOR
        };
        container.setBackgroundColor(Some(bg_color));
        container.setContentsScale(self.scale);
        container.setZPosition(1.0);

        let mut keys = Vec::with_capacity(1 + workspace.windows.len());
        keys.push(key.clone());

        let Some(layout) = compute_workspace_window_layout(&workspace.windows, frame) else {
            return keys;
        };

        // Disable implicit animations for sublayer updates in this pass
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        for (idx, window) in workspace.windows.iter().enumerate() {
            let rect = layout[idx];
            let window_id = window.id;
            let wk = (item_key.clone(), Some(window_id));
            let entry = state.preview_layers.entry(wk.clone()).or_insert_with(|| {
                let layer = CALayer::layer();
                layer.setGeometryFlipped(true);
                layer.setMasksToBounds(true);
                self.container_layer.addSublayer(&layer);
                PreviewLayerEntry::new(layer, Some(window_id))
            });
            entry.set_window_id(Some(window_id));
            let layer = entry.layer().clone();
            layer.setFrame(rect);
            layer.setCornerRadius(4.0);
            layer.setBorderWidth(0.3);
            layer.setBorderColor(Some(&**WINDOW_BORDER_COLOR));
            layer.setContentsScale(self.scale);

            let maybe_img_ptr = {
                let cache = state.preview_cache.read();
                cache.get(&window_id).map(|img| img.as_ptr() as *mut AnyObject)
            };
            let mut had_image = false;
            if let Some(img_ptr) = maybe_img_ptr {
                unsafe {
                    let _: () = msg_send![&**layer, setContents: img_ptr];
                }
                state.ready_previews.insert(window_id);
                had_image = true;
            } else if state.ready_previews.contains(&window_id) {
                had_image = true;
            }
            if !had_image {
                let (tw, th) = capture_target_for_rect(rect);
                self.schedule_capture(state, window, tw, th);
            }
            keys.push(wk);
        }
        CATransaction::commit();

        keys
    }

    fn schedule_capture(
        &self,
        state: &CommandSwitcherState,
        window: &WindowData,
        target_w: usize,
        target_h: usize,
    ) {
        let Some(wsid) = window.window_server_id else { return };
        if state.ready_previews.contains(&window.id) {
            return;
        }
        {
            let cache = state.preview_cache.read();
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
            cache: state.preview_cache.clone(),
            generation,
            overlay_ptr_bits: self as *const _ as usize,
        };
        let _ = CAPTURE_POOL.sender.send(job);
    }

    fn ensure_key_tap(&self) {
        if self.key_tap.borrow().is_some() {
            return;
        }

        #[repr(C)]
        struct KeyCtx {
            overlay: *const CommandSwitcherOverlay,
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
                        overlay.handle_keycode(keycode);
                        handled = true;
                    }
                    CGEventType::LeftMouseDown => {
                        let loc = unsafe { CGEvent::location(Some(event.as_ref())) };
                        overlay.handle_click_global(loc);
                        handled = true;
                    }
                    CGEventType::MouseMoved => {
                        let loc = unsafe { CGEvent::location(Some(event.as_ref())) };
                        overlay.handle_move_global(loc);
                        handled = true;
                    }
                    CGEventType::LeftMouseUp => handled = true,
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
                        Some(tap) => Some(tap),
                        None => {
                            drop_ctx(ctx_ptr);
                            None
                        }
                    }
                }
            }
        };

        if let Some(tap) = tap {
            self.key_tap.borrow_mut().replace(tap);
        }
    }

    fn handle_keycode(&self, keycode: u16) {
        match keycode {
            53 => self.emit_action(CommandSwitcherAction::Dismiss),
            36 | 76 => self.activate_selection(),
            48 | 124 => {
                if self.adjust_selection(1) {
                    self.present_root_layer();
                }
            }
            123 => {
                if self.adjust_selection(-1) {
                    self.present_root_layer();
                }
            }
            126 => {
                if self.adjust_selection_vertical(-1) {
                    self.present_root_layer();
                }
            }
            125 => {
                if self.adjust_selection_vertical(1) {
                    self.present_root_layer();
                }
            }
            _ => {}
        }
    }

    fn handle_click_global(&self, g_pt: CGPoint) {
        let pt = self.global_to_local_point(g_pt);
        let mut state = match self.state.try_borrow_mut() {
            Ok(s) => s,
            Err(_) => return,
        };
        let Some((idx, _)) = state
            .item_frames
            .iter()
            .enumerate()
            .find(|(_, (_, frame))| point_in_rect(pt, *frame))
        else {
            drop(state);
            self.emit_action(CommandSwitcherAction::Dismiss);
            return;
        };
        state.set_selection(idx);
        drop(state);
        self.draw_and_present();
        self.activate_selection();
    }

    fn handle_move_global(&self, g_pt: CGPoint) {
        let pt = self.global_to_local_point(g_pt);
        let mut state = match self.state.try_borrow_mut() {
            Ok(s) => s,
            Err(_) => return,
        };
        let maybe_idx = state
            .item_frames
            .iter()
            .enumerate()
            .find(|(_, (_, frame))| point_in_rect(pt, *frame))
            .map(|(idx, _)| idx);
        if let Some(idx) = maybe_idx {
            if state.selection() != Some(idx) {
                let prev = state.selection();
                state.set_selection(idx);
                let new_key = state.items[idx].key.clone();
                let old_key = prev.and_then(|p| state.items.get(p).map(|it| it.key.clone()));
                drop(state);
                if let Some(ok) = old_key.as_ref() {
                    self.update_item_selected_style(ok, false);
                }
                self.update_item_selected_style(&new_key, true);
                self.present_root_layer();
            }
        }
    }

    fn global_to_local_point(&self, g_pt: CGPoint) -> CGPoint {
        let lx = g_pt.x - self.frame.origin.x;
        let ly = (self.frame.origin.y + self.frame.size.height) - g_pt.y;
        CGPoint::new(lx, ly)
    }
}

#[derive(Clone)]
struct LayoutFrame {
    item_frame: CGRect,
    preview_frame: CGRect,
    label_frame: CGRect,
}

struct LayoutResult {
    container_frame: CGRect,
    item_frames: Vec<LayoutFrame>,
    scale: f64,
    columns: usize,
    rows: usize,
}

fn compute_layout(count: usize, bounds: CGSize) -> LayoutResult {
    if count == 0 {
        return LayoutResult {
            container_frame: CGRect::new(
                CGPoint::new(bounds.width / 2.0, bounds.height / 2.0),
                CGSize::new(0.0, 0.0),
            ),
            item_frames: Vec::new(),
            scale: 1.0,
            columns: 0,
            rows: 0,
        };
    }
    let max_container_width = (bounds.width * MAX_CONTAINER_WIDTH_RATIO).max(420.0);
    let max_container_height = (bounds.height * MAX_CONTAINER_HEIGHT_RATIO)
        .max(BASE_ITEM_HEIGHT + 2.0 * CONTAINER_PADDING);
    let available_width = (max_container_width - 2.0 * CONTAINER_PADDING).max(1.0);
    let available_height = (max_container_height - 2.0 * CONTAINER_PADDING).max(1.0);

    struct Candidate {
        scale: f64,
        columns: usize,
        rows: usize,
        container_width: f64,
        container_height: f64,
    }

    let mut best: Option<Candidate> = None;
    for columns in 1..=count {
        let rows = (count + columns - 1) / columns;
        let spacing_cols = (columns.saturating_sub(1)) as f64;
        let spacing_rows = (rows.saturating_sub(1)) as f64;
        let content_width = columns as f64 * BASE_ITEM_WIDTH + spacing_cols * ITEM_SPACING;
        let content_height = rows as f64 * BASE_ITEM_HEIGHT + spacing_rows * ITEM_SPACING;
        if content_width <= 0.0 || content_height <= 0.0 {
            continue;
        }

        let width_scale = (available_width / content_width).min(1.0);
        let height_scale = (available_height / content_height).min(1.0);
        let scale = width_scale.min(height_scale);
        if scale <= 0.0 {
            continue;
        }

        let container_width = content_width * scale + 2.0 * CONTAINER_PADDING;
        let container_height = content_height * scale + 2.0 * CONTAINER_PADDING;

        let better = match &best {
            None => true,
            Some(current) => {
                if (scale - current.scale).abs() > f64::EPSILON {
                    scale > current.scale
                } else if (container_height - current.container_height).abs() > f64::EPSILON {
                    container_height < current.container_height
                } else {
                    container_width < current.container_width
                }
            }
        };

        if better {
            best = Some(Candidate {
                scale,
                columns,
                rows,
                container_width,
                container_height,
            });
        }
    }

    let best = best.unwrap_or_else(|| Candidate {
        scale: 1.0,
        columns: count,
        rows: 1,
        container_width: (BASE_ITEM_WIDTH * count as f64)
            + (ITEM_SPACING * (count.saturating_sub(1) as f64))
            + 2.0 * CONTAINER_PADDING,
        container_height: BASE_ITEM_HEIGHT + 2.0 * CONTAINER_PADDING,
    });

    let item_width = BASE_ITEM_WIDTH * best.scale;
    let item_height = BASE_ITEM_HEIGHT * best.scale;
    let h_spacing = if best.columns > 1 {
        ITEM_SPACING * best.scale
    } else {
        0.0
    };
    let v_spacing = if best.rows > 1 {
        ITEM_SPACING * best.scale
    } else {
        0.0
    };
    let preview_width = (item_width - 16.0 * best.scale).max(40.0);
    let preview_height = (item_height - LABEL_HEIGHT * best.scale - 18.0 * best.scale).max(48.0);
    let label_height = LABEL_HEIGHT * best.scale;

    let origin_x = (bounds.width - best.container_width).max(0.0) / 2.0;
    let origin_y = (bounds.height - best.container_height).max(0.0) / 2.0;

    let container_frame = CGRect::new(
        CGPoint::new(origin_x, origin_y),
        CGSize::new(best.container_width, best.container_height),
    );

    let mut item_frames = Vec::with_capacity(count);
    for idx in 0..count {
        let row = idx / best.columns;
        let col = idx % best.columns;
        let offset_x = CONTAINER_PADDING + col as f64 * (item_width + h_spacing);
        let offset_y = CONTAINER_PADDING + row as f64 * (item_height + v_spacing);

        let item_frame = CGRect::new(
            CGPoint::new(offset_x, offset_y),
            CGSize::new(item_width, item_height),
        );

        let preview_frame = CGRect::new(
            CGPoint::new(
                offset_x + (item_width - preview_width) / 2.0,
                offset_y + 8.0 * best.scale,
            ),
            CGSize::new(preview_width, preview_height),
        );

        let label_frame = CGRect::new(
            CGPoint::new(
                offset_x + 8.0 * best.scale,
                preview_frame.origin.y + preview_frame.size.height + 6.0 * best.scale,
            ),
            CGSize::new(item_width - 16.0 * best.scale, label_height),
        );

        item_frames.push(LayoutFrame {
            item_frame,
            preview_frame,
            label_frame,
        });
    }

    LayoutResult {
        container_frame,
        item_frames,
        scale: best.scale,
        columns: best.columns,
        rows: best.rows,
    }
}

fn point_in_rect(pt: CGPoint, rect: CGRect) -> bool {
    pt.x >= rect.origin.x
        && pt.x <= rect.origin.x + rect.size.width
        && pt.y >= rect.origin.y
        && pt.y <= rect.origin.y + rect.size.height
}

struct WorkspaceLayoutMetrics {
    scale: f64,
    x_offset: f64,
    y_offset: f64,
    min_x: f64,
    min_y: f64,
    disp_height: f64,
}

impl WorkspaceLayoutMetrics {
    fn new(windows: &[WindowData], bounds: CGRect) -> Option<Self> {
        if windows.is_empty() {
            return None;
        }

        let mut min_x = f64::INFINITY;
        let mut min_y = f64::INFINITY;
        let mut max_x = f64::NEG_INFINITY;
        let mut max_y = f64::NEG_INFINITY;

        for window in windows {
            let x0 = window.frame.origin.x;
            let y0 = window.frame.origin.y;
            let x1 = x0 + window.frame.size.width;
            let y1 = y0 + window.frame.size.height;
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

        let content_w = (bounds.size.width - 2.0 * WINDOW_TILE_INSET).max(1.0);
        let content_h = (bounds.size.height - 2.0 * WINDOW_TILE_INSET).max(1.0);

        let scale = (content_w / disp_w).min(content_h / disp_h).min(WINDOW_TILE_MAX_SCALE)
            * WINDOW_TILE_SCALE_FACTOR;

        if !scale.is_finite() || scale <= 0.0 {
            return None;
        }

        let x_offset = bounds.origin.x + WINDOW_TILE_INSET + (content_w - disp_w * scale) / 2.0;
        let y_offset = bounds.origin.y + WINDOW_TILE_INSET + (content_h - disp_h * scale) / 2.0;

        Some(Self {
            scale,
            x_offset,
            y_offset,
            min_x,
            min_y,
            disp_height: disp_h,
        })
    }

    fn rect_for(&self, window: &WindowData) -> CGRect {
        let wx = window.frame.origin.x - self.min_x;
        let wy_top = window.frame.origin.y - self.min_y + window.frame.size.height;
        let wy = self.disp_height - wy_top;
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

fn compute_workspace_window_layout(windows: &[WindowData], frame: CGRect) -> Option<Vec<CGRect>> {
    let metrics = WorkspaceLayoutMetrics::new(windows, frame)?;
    Some(windows.iter().map(|window| metrics.rect_for(window)).collect())
}

fn capture_target_for_window(window: &WindowData) -> (usize, usize) {
    capture_target_for_dims(window.frame.size.width, window.frame.size.height)
}

fn capture_target_for_rect(rect: CGRect) -> (usize, usize) {
    capture_target_for_dims(rect.size.width, rect.size.height)
}

fn capture_target_for_dims(width: f64, height: f64) -> (usize, usize) {
    let width = width.max(1.0);
    let height = height.max(1.0);
    let max_edge = PREVIEW_MAX_EDGE;
    let scale = (max_edge / width.max(height)).min(1.0);
    let min_w = width.min(PREVIEW_MIN_EDGE);
    let min_h = height.min(PREVIEW_MIN_EDGE);
    let scaled_w = (width * scale).max(min_w);
    let scaled_h = (height * scale).max(min_h);
    (scaled_w.round() as usize, scaled_h.round() as usize)
}

// Mirror mission_control.rs: cache attributed strings and style diffs
struct CachedText {
    text: String,
    attributed: CFRetained<CFString>,
}

impl CachedText {
    fn new(text: &str) -> Self {
        let cf = CFString::from_str(text);
        Self {
            text: text.to_owned(),
            attributed: cf,
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

    fn apply_to(&self, layer: &CATextLayer) {
        let raw = self.attributed.as_ref() as *const AnyObject;
        unsafe { layer.setString(Some(&*raw)) };
    }
}

#[derive(Default)]
struct ItemLayerStyle {
    is_selected: Option<bool>,
}

impl ItemLayerStyle {
    fn update_selected(&mut self, sel: bool) -> bool {
        if self.is_selected == Some(sel) {
            false
        } else {
            self.is_selected = Some(sel);
            true
        }
    }
}

impl CommandSwitcherOverlay {
    fn update_item_selected_style(&self, key: &ItemKey, selected: bool) {
        if let Ok(mut state) = self.state.try_borrow_mut() {
            if let Some(layer) = state.item_layers.get(key).cloned() {
                let style_changed = state
                    .item_styles
                    .entry(key.clone())
                    .or_insert_with(Default::default)
                    .update_selected(selected);
                if style_changed {
                    let bg_color = if selected {
                        &**ITEM_SELECTED_BG_COLOR
                    } else {
                        &**ITEM_BG_COLOR
                    };
                    layer.setBackgroundColor(Some(bg_color));
                    layer.setBorderWidth(if selected { 3.0 } else { 1.0 });
                    layer.setBorderColor(Some(if selected {
                        &**SELECTED_BORDER_COLOR
                    } else {
                        &**WORKSPACE_BORDER_COLOR
                    }));
                }
            }
        }
    }
}
