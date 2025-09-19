use core::ffi::c_void;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::thread;

use core_graphics::sys as cg_sys;
use core_graphics_types::geometry as cgt;
use objc2::rc::Retained;
use objc2::{DeclaredClass, MainThreadOnly, msg_send};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSColor, NSEvent, NSGraphicsContext, NSView,
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
    NSWindow, NSWindowStyleMask,
};
use objc2_core_foundation::CGRect;
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize};

use crate::actor::app::WindowId;
use crate::model::server::{WindowData, WorkspaceData};
use crate::sys::window_server::{CapturedWindowImage, WindowServerId, capture_window_image};

unsafe extern "C" {
    fn CGContextSaveGState(c: cg_sys::CGContextRef);
    fn CGContextRestoreGState(c: cg_sys::CGContextRef);
    fn CGContextTranslateCTM(c: cg_sys::CGContextRef, tx: f64, ty: f64);
    fn CGContextScaleCTM(c: cg_sys::CGContextRef, sx: f64, sy: f64);
    fn CGContextDrawImage(c: cg_sys::CGContextRef, rect: cgt::CGRect, image: cg_sys::CGImageRef);
}

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

#[derive(Default)]
pub struct MissionControlState {
    mode: Option<MissionControlMode>,
    on_action: Option<Rc<dyn Fn(MissionControlAction)>>,
    selection: Option<Selection>,
}

impl MissionControlState {
    fn set_mode(&mut self, mode: MissionControlMode) {
        self.mode = Some(mode);
        self.selection = None;
        self.ensure_selection();
    }

    fn mode(&self) -> Option<&MissionControlMode> { self.mode.as_ref() }

    fn reset(&mut self) {
        self.mode = None;
        self.selection = None;
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
const WORKSPACE_TILE_SPACING: f64 = 20.0;
const CURRENT_WS_TILE_SPACING: f64 = 48.0;
const CURRENT_WS_TILE_PADDING: f64 = 16.0;
const CURRENT_WS_TILE_SCALE_FACTOR: f64 = 0.9;

struct WorkspaceGrid {
    bounds: NSRect,
    rows: usize,
    tile_size: NSSize,
}

impl WorkspaceGrid {
    fn new(tile_count: usize, bounds: NSRect) -> Option<Self> {
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
            tile_size: NSSize::new(tile_w, tile_h),
        })
    }

    fn position_for(&self, order_idx: usize) -> (usize, usize) {
        if self.rows == 1 {
            (0, order_idx)
        } else {
            (order_idx % self.rows, order_idx / self.rows)
        }
    }

    fn rect_for(&self, order_idx: usize) -> NSRect {
        let (row, col) = self.position_for(order_idx);
        let spacing = WORKSPACE_TILE_SPACING;
        let x = self.bounds.origin.x + spacing + (self.tile_size.width + spacing) * (col as f64);
        let y = self.bounds.origin.y + spacing + (self.tile_size.height + spacing) * (row as f64);
        NSRect::new(NSPoint::new(x, y), self.tile_size)
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
    fn rect_for(&self, window: &WindowData) -> NSRect {
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

        NSRect::new(NSPoint::new(rx, ry), NSSize::new(rw, rh))
    }
}

objc2::define_class!(
    #[unsafe(super(NSView))]
    #[ivars = RefCell<MissionControlState>]
    pub struct MissionControlView;

    impl MissionControlView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool { true }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool { true }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            let state = self.ivars().borrow();
            let Some(mode) = state.mode() else { return };

            unsafe {
                NSColor::colorWithCalibratedWhite_alpha(0.0, 0.25).setFill();
                objc2_app_kit::NSBezierPath::fillRect(self.bounds());
            }

            let content_bounds = Self::content_bounds(self.bounds());
            match mode {
                MissionControlMode::AllWorkspaces(workspaces) => {
                    let images =
                        Self::collect_images(workspaces.iter().flat_map(|ws| ws.windows.iter()));
                    self.draw_workspaces(
                        workspaces,
                        content_bounds,
                        &images,
                        state.selected_workspace(),
                    );
                }
                MissionControlMode::CurrentWorkspace(windows) => {
                    let images = Self::collect_images(windows.iter());
                    self.draw_windows_tile(
                        windows,
                        content_bounds,
                        &images,
                        state.selected_window(),
                        WindowLayoutKind::Exploded,
                    );
                }
            }
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let p = unsafe { self.convertPoint_fromView(event.locationInWindow(), None) };
            let state = self.ivars().borrow();
            let mode = state.mode().cloned();
            let handler = state.on_action.clone();
            drop(state);

            let Some(mode) = mode else { return };
            let Some(cb) = handler else { return };

            let content_bounds = Self::content_bounds(self.bounds());

            let handled = match mode {
                MissionControlMode::AllWorkspaces(workspaces) => {
                    if let Some((order_idx, target_idx)) =
                        Self::workspace_index_at_point(&workspaces, p, content_bounds)
                    {
                        if let Some(mut state) = self.ivars().try_borrow_mut().ok() {
                            state.set_selection(Selection::Workspace(order_idx));
                        }
                        unsafe { self.setNeedsDisplay(true) };
                        cb(MissionControlAction::SwitchToWorkspace(target_idx));
                        true
                    } else {
                        false
                    }
                }
                MissionControlMode::CurrentWorkspace(windows) => {
                    if let Some((order_idx, win)) =
                        Self::window_at_point(
                            &windows,
                            p,
                            content_bounds,
                            WindowLayoutKind::Exploded,
                        )
                    {
                        if let Some(mut state) = self.ivars().try_borrow_mut().ok() {
                            state.set_selection(Selection::Window(order_idx));
                        }
                        unsafe { self.setNeedsDisplay(true) };
                        let window_server_id = windows
                            .get(order_idx)
                            .and_then(|w| w.window_server_id)
                            .map(WindowServerId::new);
                        cb(MissionControlAction::FocusWindow {
                            window_id: win,
                            window_server_id,
                        });
                        true
                    } else {
                        false
                    }
                }
            };

            if !handled {
                cb(MissionControlAction::Dismiss);
            }
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            let point = unsafe { self.convertPoint_fromView(event.locationInWindow(), None) };
            let content_bounds = Self::content_bounds(self.bounds());

            let mut state = match self.ivars().try_borrow_mut() {
                Ok(state) => state,
                Err(_) => return,
            };

            let mode = match state.mode().cloned() {
                Some(mode) => mode,
                None => return,
            };

            let new_selection = match mode {
                MissionControlMode::AllWorkspaces(workspaces) => Self::workspace_index_at_point(
                    &workspaces,
                    point,
                    content_bounds,
                )
                .map(|(order_idx, _)| Selection::Workspace(order_idx)),
                MissionControlMode::CurrentWorkspace(windows) => Self::window_at_point(
                    &windows,
                    point,
                    content_bounds,
                    WindowLayoutKind::Exploded,
                )
                .map(|(order_idx, _)| Selection::Window(order_idx)),
            };

            let Some(selection) = new_selection else { return };
            if state.selection() == Some(selection) {
                return;
            }

            state.set_selection(selection);
            drop(state);
            unsafe { self.setNeedsDisplay(true) };
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            let code: u16 = unsafe { msg_send![event, keyCode] };
            if code == 53 {
                if let Some(cb) = self.ivars().borrow().on_action.clone() {
                    cb(MissionControlAction::Dismiss);
                }
                return;
            }

            let handled = match code {
                123 => self.adjust_selection(NavDirection::Left),
                124 => self.adjust_selection(NavDirection::Right),
                125 => self.adjust_selection(NavDirection::Down),
                126 => self.adjust_selection(NavDirection::Up),
                36 | 76 => {
                    self.activate_selection_action();
                    true
                }
                _ => false,
            };

            if handled {
                unsafe { self.setNeedsDisplay(true) };
            }
        }

        #[unsafe(method(cancelOperation:))]
        fn cancel_operation(&self, _sender: *mut objc2::runtime::AnyObject) {
            if let Some(cb) = self.ivars().borrow().on_action.clone() {
                cb(MissionControlAction::Dismiss);
            }
        }
    }
);

impl MissionControlView {
    fn rect_contains_point(rect: NSRect, point: NSPoint) -> bool {
        point.x >= rect.origin.x
            && point.x <= rect.origin.x + rect.size.width
            && point.y >= rect.origin.y
            && point.y <= rect.origin.y + rect.size.height
    }

    fn content_bounds(bounds: NSRect) -> NSRect {
        let width = (bounds.size.width - 2.0 * MISSION_CONTROL_MARGIN).max(0.0);
        let height = (bounds.size.height - 2.0 * MISSION_CONTROL_MARGIN).max(0.0);
        NSRect::new(
            NSPoint::new(
                bounds.origin.x + MISSION_CONTROL_MARGIN,
                bounds.origin.y + MISSION_CONTROL_MARGIN,
            ),
            NSSize::new(width, height),
        )
    }

    fn workspace_index_at_point(
        workspaces: &[WorkspaceData],
        point: NSPoint,
        bounds: NSRect,
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
        point: NSPoint,
        bounds: NSRect,
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
        bounds: NSRect,
    ) -> Option<WindowLayoutMetrics> {
        if windows.is_empty() {
            return None;
        }

        let min_x = windows.iter().map(|w| w.frame.origin.x).fold(f64::INFINITY, f64::min);
        let min_y = windows.iter().map(|w| w.frame.origin.y).fold(f64::INFINITY, f64::min);
        let max_x = windows
            .iter()
            .map(|w| w.frame.origin.x + w.frame.size.width)
            .fold(f64::NEG_INFINITY, f64::max);
        let max_y = windows
            .iter()
            .map(|w| w.frame.origin.y + w.frame.size.height)
            .fold(f64::NEG_INFINITY, f64::max);

        let disp_w = (max_x - min_x).max(1.0);
        let disp_h = (max_y - min_y).max(1.0);

        let cx = bounds.origin.x + WINDOW_TILE_INSET;
        let cy = bounds.origin.y + WINDOW_TILE_INSET;
        let cw = (bounds.size.width - 2.0 * WINDOW_TILE_INSET).max(1.0);
        let ch = (bounds.size.height - 2.0 * WINDOW_TILE_INSET).max(1.0);

        let scale_x = cw / disp_w;
        let scale_y = ch / disp_h;
        let base_scale = scale_x.min(scale_y).min(WINDOW_TILE_MAX_SCALE);
        let scale = base_scale * WINDOW_TILE_SCALE_FACTOR;

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

    fn compute_exploded_layout(windows: &[WindowData], bounds: NSRect) -> Option<Vec<NSRect>> {
        if windows.is_empty() {
            return None;
        }

        let columns = Self::exploded_column_count(windows.len(), bounds);
        let rows = ((windows.len() + columns - 1) / columns).max(1);
        let spacing = CURRENT_WS_TILE_SPACING;

        let total_spacing_x = spacing * ((columns + 1) as f64);
        let total_spacing_y = spacing * ((rows + 1) as f64);

        let available_width = (bounds.size.width - total_spacing_x).max(1.0);
        let available_height = (bounds.size.height - total_spacing_y).max(1.0);
        let cell_width = available_width / columns as f64;
        let cell_height = available_height / rows as f64;

        let mut rects = Vec::with_capacity(windows.len());

        for (idx, window) in windows.iter().enumerate() {
            let row = idx / columns;
            let col = idx % columns;

            let cell_origin_x = bounds.origin.x + spacing + (cell_width + spacing) * (col as f64);
            let cell_origin_y = bounds.origin.y + spacing + (cell_height + spacing) * (row as f64);

            let inner_width =
                (cell_width - 2.0 * CURRENT_WS_TILE_PADDING).max(WINDOW_TILE_MIN_SIZE);
            let inner_height =
                (cell_height - 2.0 * CURRENT_WS_TILE_PADDING).max(WINDOW_TILE_MIN_SIZE);

            let original_width = window.frame.size.width.max(1.0);
            let original_height = window.frame.size.height.max(1.0);

            let mut scale = (inner_width / original_width)
                .min(inner_height / original_height)
                .min(WINDOW_TILE_MAX_SCALE);
            if scale > 0.5 {
                scale *= CURRENT_WS_TILE_SCALE_FACTOR;
            }
            let scaled_width = (original_width * scale).max(WINDOW_TILE_MIN_SIZE);
            let scaled_height = (original_height * scale).max(WINDOW_TILE_MIN_SIZE);

            let origin_x = cell_origin_x + (cell_width - scaled_width) / 2.0;
            let origin_y = cell_origin_y + (cell_height - scaled_height) / 2.0;

            rects.push(NSRect::new(
                NSPoint::new(origin_x, origin_y),
                NSSize::new(scaled_width, scaled_height),
            ));
        }

        Some(rects)
    }

    fn exploded_column_count(count: usize, bounds: NSRect) -> usize {
        if count <= 1 {
            return count.max(1);
        }

        let width = bounds.size.width.max(1.0);
        let height = bounds.size.height.max(1.0);
        let aspect = (width / height).clamp(0.5, 2.0);
        let estimate = ((count as f64) * aspect).sqrt().ceil() as usize;
        estimate.clamp(1, count)
    }

    fn compute_window_rects(
        windows: &[WindowData],
        bounds: NSRect,
        kind: WindowLayoutKind,
    ) -> Option<Vec<NSRect>> {
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
        let mut state = match self.ivars().try_borrow_mut() {
            Ok(state) => state,
            Err(_) => return false,
        };
        let mode = match state.mode().cloned() {
            Some(mode) => mode,
            None => return false,
        };
        state.ensure_selection();
        let current = state.selection();

        let new_selection = match (mode, current) {
            (MissionControlMode::AllWorkspaces(workspaces), Some(Selection::Workspace(idx))) => {
                let visible = Self::visible_workspaces(&workspaces);
                if visible.is_empty() {
                    None
                } else {
                    let idx = idx.min(visible.len().saturating_sub(1));
                    Self::navigate_workspaces(&visible, idx, direction).map(Selection::Workspace)
                }
            }
            (MissionControlMode::CurrentWorkspace(windows), Some(Selection::Window(idx))) => {
                if windows.is_empty() {
                    None
                } else {
                    let idx = idx.min(windows.len().saturating_sub(1));
                    Self::navigate_windows(windows.len(), idx, direction).map(Selection::Window)
                }
            }
            (MissionControlMode::AllWorkspaces(workspaces), None) => {
                if Self::visible_workspaces(&workspaces).is_empty() {
                    None
                } else {
                    Some(Selection::Workspace(0))
                }
            }
            (MissionControlMode::CurrentWorkspace(windows), None) => {
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

    fn activate_selection_action(&self) {
        let (mode, selection, handler) = {
            let mut state = self.ivars().borrow_mut();
            state.ensure_selection();
            (state.mode().cloned(), state.selection(), state.on_action.clone())
        };

        let Some(cb) = handler else { return };

        match (mode, selection) {
            (
                Some(MissionControlMode::AllWorkspaces(workspaces)),
                Some(Selection::Workspace(idx)),
            ) => {
                let visible = Self::visible_workspaces(&workspaces);
                if visible.is_empty() {
                    return;
                }
                let idx = idx.min(visible.len().saturating_sub(1));
                if let Some((original_idx, _)) = visible.get(idx) {
                    cb(MissionControlAction::SwitchToWorkspace(*original_idx));
                }
            }
            (Some(MissionControlMode::CurrentWorkspace(windows)), Some(Selection::Window(idx))) => {
                if windows.is_empty() {
                    return;
                }
                let idx = idx.min(windows.len().saturating_sub(1));
                if let Some(window) = windows.get(idx) {
                    let window_server_id = window.window_server_id.map(WindowServerId::new);
                    cb(MissionControlAction::FocusWindow {
                        window_id: window.id,
                        window_server_id,
                    });
                }
            }
            _ => {}
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
        workspaces: &[WorkspaceData],
        bounds: NSRect,
        cache: &HashMap<WindowId, CapturedWindowImage>,
        selected: Option<usize>,
    ) {
        let visible = Self::visible_workspaces(workspaces);
        let Some(grid) = WorkspaceGrid::new(visible.len(), bounds) else {
            return;
        };
        for (order_idx, (original_idx, _)) in visible.iter().enumerate() {
            let ws = &workspaces[*original_idx];
            let rect = grid.rect_for(order_idx);

            unsafe {
                let bg = objc2_app_kit::NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                    rect, 6.0, 6.0,
                );
                NSColor::colorWithCalibratedWhite_alpha(1.0, 0.03).setFill();
                bg.fill();
            }

            self.draw_windows_tile(
                &ws.windows,
                rect,
                cache,
                None,
                WindowLayoutKind::PreserveOriginal,
            );

            unsafe {
                let name = objc2_foundation::NSString::from_str(&ws.name);
                let label_point = NSPoint::new(rect.origin.x + 6.0, rect.origin.y + 6.0);
                let nil_attrs: *const objc2::runtime::AnyObject = core::ptr::null();
                let _: () = msg_send![&*name, drawAtPoint: label_point, withAttributes: nil_attrs];

                let border = objc2_app_kit::NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                    rect, 6.0, 6.0,
                );
                let is_selected = Some(order_idx) == selected;
                let stroke_color = if is_selected {
                    NSColor::colorWithCalibratedRed_green_blue_alpha(0.2, 0.45, 1.0, 0.85)
                } else {
                    NSColor::colorWithCalibratedWhite_alpha(1.0, 0.12)
                };
                stroke_color.setStroke();
                border.setLineWidth(if is_selected { 3.0 } else { 1.0 });
                border.stroke();
            }
        }
    }

    fn draw_windows_tile(
        &self,
        windows: &[WindowData],
        tile: NSRect,
        cache: &HashMap<WindowId, CapturedWindowImage>,
        selected: Option<usize>,
        layout: WindowLayoutKind,
    ) {
        let Some(rects) = Self::compute_window_rects(windows, tile, layout) else {
            return;
        };

        let selected_idx = selected.map(|s| s.min(windows.len().saturating_sub(1)));
        for idx in (0..windows.len()).rev() {
            let window = &windows[idx];
            let rect = rects[idx];
            let is_selected = selected_idx.map_or(false, |s| s == idx);
            Self::draw_window_outline(rect, is_selected);
            if let Some(captured) = cache.get(&window.id) {
                Self::draw_window_snapshot(rect, captured);
            }
        }
    }

    fn draw_window_outline(rect: NSRect, is_selected: bool) {
        unsafe {
            let path = objc2_app_kit::NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                rect, 4.0, 4.0,
            );
            NSColor::colorWithCalibratedWhite_alpha(1.0, 0.15).setFill();
            path.fill();
            if is_selected {
                NSColor::colorWithCalibratedRed_green_blue_alpha(0.2, 0.45, 1.0, 0.85).setStroke();
                path.setLineWidth(2.0);
            } else {
                NSColor::colorWithCalibratedWhite_alpha(0.0, 0.65).setStroke();
                path.setLineWidth(1.0);
            }
            path.stroke();
        }
    }

    fn draw_window_snapshot(rect: NSRect, captured: &CapturedWindowImage) {
        let cgimg = captured.as_ptr() as cg_sys::CGImageRef;
        if let Some(ctx) = unsafe { NSGraphicsContext::currentContext() } {
            unsafe {
                let port: *mut c_void = msg_send![&*ctx, graphicsPort];
                let cgctx: cg_sys::CGContextRef = port as cg_sys::CGContextRef;

                CGContextSaveGState(cgctx);
                // Flip vertically within the destination rect so the image draws right side up.
                CGContextTranslateCTM(cgctx, rect.origin.x, rect.origin.y + rect.size.height);
                CGContextScaleCTM(cgctx, 1.0, -1.0);

                let image_rect = cgt::CGRect {
                    origin: cgt::CGPoint { x: 0.0, y: 0.0 },
                    size: cgt::CGSize {
                        width: rect.size.width,
                        height: rect.size.height,
                    },
                };
                CGContextDrawImage(cgctx, image_rect, cgimg);
                CGContextRestoreGState(cgctx);
            }
        }
    }

    fn collect_images<'a, I>(windows: I) -> HashMap<WindowId, CapturedWindowImage>
    where I: IntoIterator<Item = &'a WindowData> {
        let requests: Vec<_> = windows
            .into_iter()
            .filter_map(|window| {
                window.window_server_id.map(|wsid| (window.id, WindowServerId::new(wsid)))
            })
            .collect();
        Self::collect_images_from_requests(requests)
    }

    fn collect_images_from_requests(
        requests: Vec<(WindowId, WindowServerId)>,
    ) -> HashMap<WindowId, CapturedWindowImage> {
        if requests.is_empty() {
            return HashMap::new();
        }

        if requests.len() == 1 {
            let (window_id, wsid) = requests.into_iter().next().unwrap();
            if let Some(image) = capture_window_image(wsid) {
                let mut images = HashMap::with_capacity(1);
                images.insert(window_id, image);
                return images;
            }
            return HashMap::new();
        }

        let mut images = HashMap::with_capacity(requests.len());
        thread::scope(|scope| {
            let mut handles = Vec::with_capacity(requests.len());
            for (window_id, wsid) in requests.into_iter() {
                handles.push((window_id, scope.spawn(move || capture_window_image(wsid))));
            }

            for (window_id, handle) in handles {
                match handle.join() {
                    Ok(Some(image)) => {
                        images.insert(window_id, image);
                    }
                    _ => {}
                }
            }
        });
        images
    }
}

objc2::define_class!(
    #[unsafe(super(NSWindow))]
    pub struct MissionControlWindow;

    impl MissionControlWindow {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key_window(&self) -> bool { true }

        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main_window(&self) -> bool { true }
    }
);

pub struct MissionControlOverlay {
    window: Retained<NSWindow>,
    view: Retained<MissionControlView>,
    mtm: MainThreadMarker,
    key_tap: RefCell<Option<crate::sys::event_tap::EventTap>>,
}

impl MissionControlOverlay {
    pub fn new(mtm: MainThreadMarker, frame: CGRect) -> Self {
        unsafe {
            let view = MissionControlView::alloc(mtm).set_ivars(RefCell::default());
            let view: Retained<_> = msg_send![super(view), initWithFrame: frame];
            view.setWantsLayer(true);

            let blur: Retained<NSVisualEffectView> = Retained::from(
                NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), frame),
            );
            blur.setMaterial(NSVisualEffectMaterial::HUDWindow);
            blur.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
            blur.setState(NSVisualEffectState::Active);

            let raw = MissionControlWindow::alloc(mtm);
            let window_sub: Retained<MissionControlWindow> = msg_send![
                raw,
                initWithContentRect: frame,
                styleMask: NSWindowStyleMask::Borderless,
                backing: NSBackingStoreType::Buffered,
                defer: false
            ];
            let window: Retained<NSWindow> = window_sub.into_super();
            window.setOpaque(false);
            window.setIgnoresMouseEvents(false);
            window.setAcceptsMouseMovedEvents(true);
            window.setLevel(crate::actor::mouse::NSPopUpMenuWindowLevel);
            let clear = NSColor::clearColor();
            window.setBackgroundColor(Some(&clear));
            if let Some(content) = window.contentView() {
                content.addSubview(&blur);
                content.addSubview(&view);
            }
            let _: () = objc2::msg_send![&*window, makeKeyAndOrderFront: std::ptr::null::<objc2::runtime::AnyObject>()];
            window.makeFirstResponder(Some(&view));

            Self {
                window,
                view,
                mtm,
                key_tap: RefCell::new(None),
            }
        }
    }

    pub fn set_action_handler(&self, f: Rc<dyn Fn(MissionControlAction)>) {
        self.view.ivars().borrow_mut().on_action = Some(f);
    }

    pub fn update(&self, mode: MissionControlMode) {
        self.view.ivars().borrow_mut().set_mode(mode);
        self.window.makeFirstResponder(Some(&self.view));
        self.window.makeKeyAndOrderFront(None);
        unsafe { self.view.setNeedsDisplay(true) };
        let app = NSApplication::sharedApplication(self.mtm);
        #[allow(deprecated)]
        let _ = app.activateIgnoringOtherApps(true);
        self.ensure_key_tap();
    }

    pub fn hide(&self) {
        self.view.ivars().borrow_mut().reset();
        unsafe {
            let _: () = objc2::msg_send![&*self.window, orderOut: std::ptr::null::<objc2::runtime::AnyObject>()];
        }
        self.key_tap.borrow_mut().take();
    }

    fn emit_action(&self, action: MissionControlAction) {
        let handler = self.view.ivars().borrow().on_action.clone();
        if let Some(cb) = handler {
            cb(action);
        }
    }

    fn ensure_key_tap(&self) {
        if self.key_tap.borrow().is_some() {
            return;
        }
        use objc2_core_graphics as ocg;
        let mask = 1u64 << ocg::CGEventType::KeyDown.0 as u64;

        #[repr(C)]
        struct KeyCtx(*const MissionControlOverlay);

        unsafe fn drop_ctx(ptr: *mut c_void) {
            unsafe {
                drop(Box::from_raw(ptr as *mut KeyCtx));
            }
        }

        unsafe extern "C-unwind" fn key_callback(
            _proxy: ocg::CGEventTapProxy,
            _etype: ocg::CGEventType,
            event: core::ptr::NonNull<ocg::CGEvent>,
            user_info: *mut c_void,
        ) -> *mut ocg::CGEvent {
            use objc2_core_graphics::{CGEvent, CGEventField};
            let keycode = unsafe {
                CGEvent::integer_value_field(
                    Some(event.as_ref()),
                    CGEventField::KeyboardEventKeycode,
                ) as u16
            };
            if keycode == 53 {
                let ctx = unsafe { &*(user_info as *const KeyCtx) };
                if let Some(overlay) = unsafe { ctx.0.as_ref() } {
                    overlay.emit_action(MissionControlAction::Dismiss);
                }
            }
            event.as_ptr()
        }

        let ctx = Box::new(KeyCtx(self as *const _));
        let ctx_ptr = Box::into_raw(ctx) as *mut c_void;

        let tap = unsafe {
            crate::sys::event_tap::EventTap::new_listen_only(
                mask,
                Some(key_callback),
                ctx_ptr,
                Some(drop_ctx),
            )
        };

        if let Some(t) = tap {
            self.key_tap.borrow_mut().replace(t);
        } else {
            unsafe { drop_ctx(ctx_ptr) };
        }
    }
}
