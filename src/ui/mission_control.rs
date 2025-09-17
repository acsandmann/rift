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

#[derive(Debug, Clone)]
pub enum MissionControlMode {
    AllWorkspaces(Vec<WorkspaceData>),
    CurrentWorkspace(Vec<WindowData>),
}

#[derive(Debug, Clone)]
pub enum MissionControlAction {
    SwitchToWorkspace(usize),
    FocusWindow(WindowId),
    Dismiss,
}

#[derive(Default)]
pub struct MissionControlState {
    mode: Option<MissionControlMode>,
    on_action: Option<Rc<dyn Fn(MissionControlAction)>>,
}

impl MissionControlState {
    fn set_mode(&mut self, mode: MissionControlMode) { self.mode = Some(mode); }

    fn mode(&self) -> Option<&MissionControlMode> { self.mode.as_ref() }

    fn reset(&mut self) { self.mode = None; }
}

const MISSION_CONTROL_MARGIN: f64 = 48.0;
const WINDOW_TILE_INSET: f64 = 3.0;
const WINDOW_TILE_GAP: f64 = 1.0;
const WINDOW_TILE_MIN_SIZE: f64 = 2.0;
const WINDOW_TILE_SCALE_FACTOR: f64 = 0.75;
const WINDOW_TILE_MAX_SCALE: f64 = 1.0;

struct WindowLayoutMetrics {
    scale: f64,
    x_offset: f64,
    y_offset: f64,
    min_x: f64,
    min_y: f64,
    disp_h: f64,
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
                    let images = Self::collect_images_for_workspaces(workspaces);
                    self.draw_workspaces(workspaces, content_bounds, &images);
                }
                MissionControlMode::CurrentWorkspace(windows) => {
                    let images = Self::collect_images_for_windows(windows);
                    self.draw_windows_tile(windows, content_bounds, &images);
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
                    if let Some(idx) = Self::workspace_index_at_point(&workspaces, p, content_bounds) {
                        cb(MissionControlAction::SwitchToWorkspace(idx));
                        true
                    } else {
                        false
                    }
                }
                MissionControlMode::CurrentWorkspace(windows) => {
                    if let Some(win) = Self::window_at_point(&windows, p, content_bounds) {
                        cb(MissionControlAction::FocusWindow(win));
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

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            let code: u16 = unsafe { msg_send![event, keyCode] };
            if code == 53 {
                if let Some(cb) = self.ivars().borrow().on_action.clone() {
                    cb(MissionControlAction::Dismiss);
                }
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
    ) -> Option<usize> {
        if !Self::rect_contains_point(bounds, point) {
            return None;
        }
        let visible = Self::visible_workspaces(workspaces);
        if visible.is_empty() {
            return None;
        }
        let cols = (visible.len() as f64 / 2.0).ceil().max(1.0) as usize;
        let rows = if visible.len() > cols { 2 } else { 1 };
        let spacing = 20.0;
        let tile_w = (bounds.size.width - spacing * ((cols + 1) as f64)) / (cols as f64);
        let tile_h = (bounds.size.height - spacing * ((rows + 1) as f64)) / (rows as f64);

        for (i, (original_idx, _)) in visible.iter().enumerate() {
            let (r, c) = if rows == 2 {
                (i % rows, i / rows)
            } else {
                (0, i)
            };
            let x = bounds.origin.x + spacing + (tile_w + spacing) * (c as f64);
            let y = bounds.origin.y + spacing + (tile_h + spacing) * (r as f64);
            let rect = NSRect::new(NSPoint::new(x, y), NSSize::new(tile_w, tile_h));
            if Self::rect_contains_point(rect, point) {
                return Some(*original_idx);
            }
        }
        None
    }

    fn window_at_point(windows: &[WindowData], point: NSPoint, bounds: NSRect) -> Option<WindowId> {
        if !Self::rect_contains_point(bounds, point) {
            return None;
        }
        let layout = Self::compute_window_layout(windows, bounds)?;

        for window in windows.iter().rev() {
            let rect = layout.rect_for(window);
            if Self::rect_contains_point(rect, point) {
                return Some(window.id);
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
    ) {
        let visible = Self::visible_workspaces(workspaces);
        if visible.is_empty() {
            return;
        }
        let cols = (visible.len() as f64 / 2.0).ceil().max(1.0) as usize;
        let rows = if visible.len() > cols { 2 } else { 1 };
        let spacing = 20.0;
        let tile_w = (bounds.size.width - spacing * ((cols + 1) as f64)) / (cols as f64);
        let tile_h = (bounds.size.height - spacing * ((rows + 1) as f64)) / (rows as f64);

        for (i, (original_idx, _)) in visible.iter().enumerate() {
            let ws = &workspaces[*original_idx];
            let (r, c) = if rows == 2 {
                (i % rows, i / rows)
            } else {
                (0, i)
            };
            let x = bounds.origin.x + spacing + (tile_w + spacing) * (c as f64);
            let y = bounds.origin.y + spacing + (tile_h + spacing) * (r as f64);
            let rect = NSRect::new(NSPoint::new(x, y), NSSize::new(tile_w, tile_h));

            unsafe {
                let bg = objc2_app_kit::NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                    rect, 6.0, 6.0,
                );
                NSColor::colorWithCalibratedWhite_alpha(1.0, 0.03).setFill();
                bg.fill();
            }

            self.draw_windows_tile(&ws.windows, rect, cache);

            unsafe {
                let name = objc2_foundation::NSString::from_str(&ws.name);
                let label_point = NSPoint::new(rect.origin.x + 6.0, rect.origin.y + 6.0);
                let nil_attrs: *const objc2::runtime::AnyObject = core::ptr::null();
                let _: () = msg_send![&*name, drawAtPoint: label_point, withAttributes: nil_attrs];

                let border = objc2_app_kit::NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                    rect, 6.0, 6.0,
                );
                if ws.is_active {
                    NSColor::colorWithCalibratedRed_green_blue_alpha(0.2, 0.45, 1.0, 0.85)
                        .setStroke();
                    border.setLineWidth(3.0);
                    border.stroke();
                } else {
                    NSColor::colorWithCalibratedWhite_alpha(1.0, 0.12).setStroke();
                    border.setLineWidth(1.0);
                    border.stroke();
                }
            }
        }
    }

    fn draw_windows_tile(
        &self,
        windows: &[WindowData],
        tile: NSRect,
        cache: &HashMap<WindowId, CapturedWindowImage>,
    ) {
        let Some(layout) = Self::compute_window_layout(windows, tile) else {
            return;
        };

        for window in windows.iter().rev() {
            let rect = layout.rect_for(window);

            unsafe {
                let path = objc2_app_kit::NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                    rect, 4.0, 4.0,
                );
                NSColor::colorWithCalibratedWhite_alpha(1.0, 0.15).setFill();
                path.fill();
                NSColor::colorWithCalibratedWhite_alpha(0.0, 0.65).setStroke();
                path.setLineWidth(1.0);
                path.stroke();
            }

            if let Some(captured) = cache.get(&window.id) {
                let cgimg = captured.as_ptr() as cg_sys::CGImageRef;
                if let Some(ctx) = unsafe { NSGraphicsContext::currentContext() } {
                    unsafe {
                        let port: *mut c_void = msg_send![&*ctx, graphicsPort];
                        let cgctx: cg_sys::CGContextRef = port as cg_sys::CGContextRef;

                        unsafe extern "C" {
                            fn CGContextSaveGState(c: cg_sys::CGContextRef);
                            fn CGContextRestoreGState(c: cg_sys::CGContextRef);
                            fn CGContextTranslateCTM(c: cg_sys::CGContextRef, tx: f64, ty: f64);
                            fn CGContextScaleCTM(c: cg_sys::CGContextRef, sx: f64, sy: f64);
                            fn CGContextDrawImage(
                                c: cg_sys::CGContextRef,
                                rect: cgt::CGRect,
                                image: cg_sys::CGImageRef,
                            );
                        }

                        CGContextSaveGState(cgctx);
                        // Flip vertically within the destination rect so the image draws right side up
                        CGContextTranslateCTM(
                            cgctx,
                            rect.origin.x,
                            rect.origin.y + rect.size.height,
                        );
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
        }
    }

    fn collect_images_for_workspaces(
        workspaces: &[WorkspaceData],
    ) -> HashMap<WindowId, CapturedWindowImage> {
        let mut requests = Vec::new();
        for ws in workspaces {
            for window in &ws.windows {
                if let Some(wsid) = window.window_server_id {
                    requests.push((window.id, WindowServerId::new(wsid)));
                }
            }
        }
        Self::collect_images_from_requests(requests)
    }

    fn collect_images_for_windows(
        windows: &[WindowData],
    ) -> HashMap<WindowId, CapturedWindowImage> {
        let mut requests = Vec::new();
        for window in windows {
            if let Some(wsid) = window.window_server_id {
                requests.push((window.id, WindowServerId::new(wsid)));
            }
        }
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
