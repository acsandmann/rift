// many ideas for how this works were taken from https://github.com/xiamaz/YabaiIndicator
use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::{
    NSColor, NSFont, NSFontAttributeName, NSForegroundColorAttributeName, NSGraphicsContext,
    NSStatusBar, NSStatusItem, NSVariableStatusItemLength, NSView,
};
use objc2_core_foundation::{
    CFAttributedString, CFDictionary, CFRetained, CFString, CGFloat, CGPoint, CGRect, CGSize,
};
use objc2_core_graphics::{CGBlendMode, CGContext};
use objc2_core_text::CTLine;
use objc2_foundation::{
    MainThreadMarker, NSAttributedStringKey, NSDictionary, NSMutableDictionary, NSRect, NSSize,
    NSString,
};
use tracing::debug;

use crate::common::config::{
    ActiveWorkspaceLabel, MenuBarDisplayMode, MenuBarSettings, WorkspaceDisplayStyle,
};
use crate::model::VirtualWorkspaceId;
use crate::model::server::{WindowData, WorkspaceData};
use crate::sys::screen::SpaceId;

const CELL_WIDTH: f64 = 20.0;
const CELL_HEIGHT: f64 = 15.0;
const CELL_SPACING: f64 = 4.0;
const CORNER_RADIUS: f64 = 3.0;
const BORDER_WIDTH: f64 = 1.0;
const CONTENT_INSET: f64 = 2.0;
const FONT_SIZE: f64 = 12.0;

pub struct MenuIcon {
    status_item: Retained<NSStatusItem>,
    view: Retained<MenuIconView>,
    mtm: MainThreadMarker,
    prev_width: f64,
}

impl MenuIcon {
    pub fn new(mtm: MainThreadMarker) -> Self {
        let status_bar = NSStatusBar::systemStatusBar();
        let status_item = status_bar.statusItemWithLength(NSVariableStatusItemLength);
        let view = MenuIconView::new(mtm);
        if let Some(btn) = status_item.button(mtm) {
            btn.addSubview(&*view);
            view.setFrameSize(NSSize::new(0.0, 0.0));
            status_item.setVisible(true);
        }

        Self {
            status_item,
            view,
            mtm,
            prev_width: 0.0,
        }
    }

    pub fn update(
        &mut self,
        _active_space: SpaceId,
        workspaces: Vec<WorkspaceData>,
        _active_workspace: Option<VirtualWorkspaceId>,
        _windows: Vec<WindowData>,
        settings: &MenuBarSettings,
    ) {
        let mode = settings.mode;
        let style = settings.display_style;
        let label_for = |workspace: &WorkspaceData| match settings.active_label {
            ActiveWorkspaceLabel::Index => format!("{}", workspace.index + 1),
            ActiveWorkspaceLabel::Name => {
                if workspace.name.is_empty() {
                    format!("{}", workspace.index + 1)
                } else {
                    workspace.name.clone()
                }
            }
        };

        let render_inputs = match (mode, style) {
            (MenuBarDisplayMode::All, WorkspaceDisplayStyle::Layout) => {
                let filtered = if settings.show_empty {
                    workspaces
                } else {
                    workspaces
                        .into_iter()
                        .filter(|w| w.window_count > 0 || w.is_active)
                        .collect::<Vec<_>>()
                };
                filtered
                    .into_iter()
                    .map(|ws| WorkspaceRenderInput {
                        workspace: ws,
                        label: String::new(),
                        show_windows: true,
                    })
                    .collect()
            }
            (MenuBarDisplayMode::All, WorkspaceDisplayStyle::Label) => workspaces
                .into_iter()
                .filter(|w| settings.show_empty || w.window_count > 0 || w.is_active)
                .map(|ws| {
                    let mut clone = ws.clone();
                    clone.windows.clear();
                    clone.window_count = 0;
                    WorkspaceRenderInput {
                        workspace: clone,
                        label: label_for(&ws),
                        show_windows: false,
                    }
                })
                .collect(),
            (MenuBarDisplayMode::Active, WorkspaceDisplayStyle::Layout) => workspaces
                .into_iter()
                .find(|w| w.is_active)
                .map(|ws| {
                    vec![WorkspaceRenderInput {
                        workspace: ws,
                        label: String::new(),
                        show_windows: true,
                    }]
                })
                .unwrap_or_default(),
            (MenuBarDisplayMode::Active, WorkspaceDisplayStyle::Label) => workspaces
                .into_iter()
                .find(|w| w.is_active)
                .map(|ws| {
                    let mut clone = ws.clone();
                    clone.windows.clear();
                    clone.window_count = 0;
                    vec![WorkspaceRenderInput {
                        workspace: clone,
                        label: label_for(&ws),
                        show_windows: false,
                    }]
                })
                .unwrap_or_default(),
        };

        if render_inputs.is_empty() {
            self.status_item.setVisible(false);
            self.prev_width = 0.0;
            return;
        }

        let layout = {
            let view_ivars = self.view.ivars();
            let active_attrs = view_ivars.active_text_attrs.as_ref();
            let inactive_attrs = view_ivars.inactive_text_attrs.as_ref();
            build_layout(&render_inputs, active_attrs, inactive_attrs)
        };
        if layout.workspaces.is_empty() {
            self.status_item.setVisible(false);
            self.prev_width = 0.0;
            return;
        }

        let size = NSSize::new(layout.total_width, layout.total_height);
        self.view.set_layout(layout);

        self.status_item.setLength(size.width);
        self.status_item.setVisible(true);

        if let Some(btn) = self.status_item.button(self.mtm) {
            if self.prev_width != size.width {
                self.prev_width = size.width;
                btn.setNeedsLayout(true);
            }

            self.view.setFrameSize(size);
            let btn_bounds = btn.bounds();
            let x = (btn_bounds.size.width - size.width) / 2.0;
            let y = (btn_bounds.size.height - size.height) / 2.0;
            self.view.setFrameOrigin(CGPoint::new(x, y));
        }

        self.view.setNeedsDisplay(true);
    }
}

impl Drop for MenuIcon {
    fn drop(&mut self) {
        debug!("Removing menu bar icon");

        let status_bar = NSStatusBar::systemStatusBar();
        status_bar.removeStatusItem(&self.status_item);
    }
}

#[derive(Default)]
struct MenuIconLayout {
    total_width: f64,
    total_height: f64,
    workspaces: Vec<WorkspaceRenderData>,
}

struct WorkspaceRenderData {
    bg_rect: CGRect,
    fill_alpha: f64,
    windows: Vec<WindowRenderRect>,
    label_line: Option<CachedTextLine>,
    show_windows: bool,
}

struct WorkspaceRenderInput {
    workspace: WorkspaceData,
    label: String,
    show_windows: bool,
}

struct WindowRenderRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

struct CachedTextLine {
    line: CFRetained<CTLine>,
    width: f64,
    ascent: f64,
    descent: f64,
}

struct MenuIconViewIvars {
    layout: RefCell<MenuIconLayout>,
    active_text_attrs: Retained<NSDictionary<NSAttributedStringKey, AnyObject>>,
    inactive_text_attrs: Retained<NSDictionary<NSAttributedStringKey, AnyObject>>,
}

fn as_any_object<T: Message>(obj: &T) -> &AnyObject {
    unsafe { &*(obj as *const T as *const AnyObject) }
}

fn build_text_attrs(
    font: &NSFont,
    color: &NSColor,
) -> Retained<NSDictionary<NSAttributedStringKey, AnyObject>> {
    let dict = NSMutableDictionary::<NSAttributedStringKey, AnyObject>::new();
    unsafe {
        dict.setObject_forKeyedSubscript(
            Some(as_any_object(font)),
            ProtocolObject::from_ref(NSFontAttributeName),
        );
        dict.setObject_forKeyedSubscript(
            Some(as_any_object(color)),
            ProtocolObject::from_ref(NSForegroundColorAttributeName),
        );
    }
    unsafe { Retained::cast_unchecked(dict) }
}

fn build_cached_text_line(
    label: &str,
    attrs: &NSDictionary<NSAttributedStringKey, AnyObject>,
) -> Option<CachedTextLine> {
    if label.is_empty() {
        return None;
    }

    let label_ns = NSString::from_str(label);
    let cf_string: &CFString = label_ns.as_ref();
    let cf_dict_ref: &CFDictionary<NSAttributedStringKey, AnyObject> = attrs.as_ref();
    let cf_dict: &CFDictionary = cf_dict_ref.as_opaque();
    let attr_string = unsafe { CFAttributedString::new(None, Some(cf_string), Some(cf_dict)) }?;
    let line: CFRetained<CTLine> = unsafe { CTLine::with_attributed_string(attr_string.as_ref()) };

    let mut ascent: CGFloat = 0.0;
    let mut descent: CGFloat = 0.0;
    let mut leading: CGFloat = 0.0;
    let line_ref: &CTLine = line.as_ref();
    let width = unsafe { line_ref.typographic_bounds(&mut ascent, &mut descent, &mut leading) };

    Some(CachedTextLine {
        line,
        width: width as f64,
        ascent: ascent as f64,
        descent: descent as f64,
    })
}

impl MenuIconView {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let font = NSFont::menuBarFontOfSize(FONT_SIZE);
        let active_color = NSColor::blackColor();
        let inactive_color = NSColor::whiteColor();
        let active_attrs = build_text_attrs(font.as_ref(), active_color.as_ref());
        let inactive_attrs = build_text_attrs(font.as_ref(), inactive_color.as_ref());

        let frame = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(0.0, 0.0));
        let view = mtm.alloc().set_ivars(MenuIconViewIvars {
            layout: RefCell::new(MenuIconLayout::default()),
            active_text_attrs: active_attrs,
            inactive_text_attrs: inactive_attrs,
        });
        unsafe { msg_send![super(view), initWithFrame: frame] }
    }

    fn set_layout(&self, layout: MenuIconLayout) {
        *self.ivars().layout.borrow_mut() = layout;
        self.setNeedsDisplay(true);
    }
}

fn build_layout(
    inputs: &[WorkspaceRenderInput],
    active_attrs: &NSDictionary<NSAttributedStringKey, AnyObject>,
    inactive_attrs: &NSDictionary<NSAttributedStringKey, AnyObject>,
) -> MenuIconLayout {
    let count = inputs.len();
    let total_width =
        (CELL_WIDTH * count as f64) + (CELL_SPACING * (count.saturating_sub(1) as f64));
    let total_height = CELL_HEIGHT;

    let mut workspaces = Vec::with_capacity(count);
    for (i, input) in inputs.iter().enumerate() {
        let workspace = &input.workspace;
        let bg_x = i as f64 * (CELL_WIDTH + CELL_SPACING);
        let bg_y = 0.0;
        let bg_rect = CGRect::new(CGPoint::new(bg_x, bg_y), CGSize::new(CELL_WIDTH, CELL_HEIGHT));

        let fill_alpha = if input.show_windows {
            if workspace.is_active {
                1.0
            } else if workspace.window_count > 0 {
                0.45
            } else {
                0.0
            }
        } else if workspace.is_active {
            1.0
        } else {
            0.35
        };

        let windows = if input.show_windows && !workspace.windows.is_empty() {
            let min_x =
                workspace.windows.iter().map(|w| w.frame.origin.x).fold(f64::INFINITY, f64::min);
            let min_y =
                workspace.windows.iter().map(|w| w.frame.origin.y).fold(f64::INFINITY, f64::min);
            let max_x = workspace
                .windows
                .iter()
                .map(|w| w.frame.origin.x + w.frame.size.width)
                .fold(f64::NEG_INFINITY, f64::max);
            let max_y = workspace
                .windows
                .iter()
                .map(|w| w.frame.origin.y + w.frame.size.height)
                .fold(f64::NEG_INFINITY, f64::max);

            let disp_w = (max_x - min_x).max(1.0);
            let disp_h = (max_y - min_y).max(1.0);

            let cx = bg_x + CONTENT_INSET;
            let cy = bg_y + CONTENT_INSET;
            let cw = (CELL_WIDTH - 2.0 * CONTENT_INSET).max(1.0);
            let ch = (CELL_HEIGHT - 2.0 * CONTENT_INSET).max(1.0);

            let scaling = if disp_h > disp_w {
                disp_h / ch
            } else {
                disp_w / cw
            };
            let sf = 1.0 / scaling;

            let xoffset = if disp_h > disp_w {
                (cw - disp_w * sf) / 2.0
            } else {
                0.0
            } + cx;
            let yoffset = if disp_h > disp_w {
                0.0
            } else {
                (ch - disp_h * sf) / 2.0
            } + cy;

            let mut rects = Vec::with_capacity(workspace.windows.len());
            for window in workspace.windows.iter().rev() {
                let wx = window.frame.origin.x - min_x;
                let wy_top = window.frame.origin.y - min_y + window.frame.size.height;
                let wy = disp_h - wy_top;
                let ww = window.frame.size.width;
                let wh = window.frame.size.height;

                let mut rx = xoffset + wx * sf;
                let mut ry = yoffset + wy * sf;
                let mut rw = (ww * sf).max(2.0);
                let mut rh = (wh * sf).max(2.0);

                const WIN_GAP: f64 = 0.75;
                if rw > (2.0 + WIN_GAP) {
                    rx += WIN_GAP / 2.0;
                    rw -= WIN_GAP;
                }
                if rh > (2.0 + WIN_GAP) {
                    ry += WIN_GAP / 2.0;
                    rh -= WIN_GAP;
                }

                rects.push(WindowRenderRect {
                    x: rx,
                    y: ry,
                    width: rw,
                    height: rh,
                });
            }
            rects
        } else {
            Vec::new()
        };

        let label_line = if !input.label.is_empty() {
            let attrs = if fill_alpha > 0.0 {
                active_attrs
            } else {
                inactive_attrs
            };
            build_cached_text_line(&input.label, attrs)
        } else {
            None
        };

        workspaces.push(WorkspaceRenderData {
            bg_rect,
            fill_alpha,
            windows,
            label_line,
            show_windows: input.show_windows,
        });
    }

    MenuIconLayout {
        total_width,
        total_height,
        workspaces,
    }
}

fn add_rounded_rect(ctx: &CGContext, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let ctx = Some(ctx);
    let r = r.min(w / 2.0).min(h / 2.0);
    CGContext::begin_path(ctx);
    CGContext::move_to_point(ctx, x + r, y + h);
    CGContext::add_line_to_point(ctx, x + w - r, y + h);
    CGContext::add_arc_to_point(ctx, x + w, y + h, x + w, y + h - r, r);
    CGContext::add_line_to_point(ctx, x + w, y + r);
    CGContext::add_arc_to_point(ctx, x + w, y, x + w - r, y, r);
    CGContext::add_line_to_point(ctx, x + r, y);
    CGContext::add_arc_to_point(ctx, x, y, x, y + r, r);
    CGContext::add_line_to_point(ctx, x, y + h - r);
    CGContext::add_arc_to_point(ctx, x, y + h, x + r, y + h, r);
    CGContext::close_path(ctx);
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "RiftMenuBarIconView"]
    #[ivars = MenuIconViewIvars]
    struct MenuIconView;

    impl MenuIconView {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty_rect: NSRect) {
            let layout = self.ivars().layout.borrow();
            let bounds = self.bounds();

            if let Some(context) = NSGraphicsContext::currentContext() {
                let cg_context = context.CGContext();
                let cg = cg_context.as_ref();
                CGContext::save_g_state(Some(cg));
                CGContext::clear_rect(Some(cg), bounds);

                let y_offset = (bounds.size.height - layout.total_height) / 2.0;

                for workspace in layout.workspaces.iter() {
                    let rect = workspace.bg_rect;
                    let bg_y = rect.origin.y + y_offset;
                    add_rounded_rect(
                        cg,
                        rect.origin.x,
                        bg_y,
                        rect.size.width,
                        rect.size.height,
                        CORNER_RADIUS,
                    );

                    if workspace.fill_alpha > 0.0 {
                        CGContext::set_rgb_fill_color(
                            Some(cg),
                            1.0,
                            1.0,
                            1.0,
                            workspace.fill_alpha,
                        );
                        CGContext::fill_path(Some(cg));
                    }

                    add_rounded_rect(
                        cg,
                        rect.origin.x,
                        bg_y,
                        rect.size.width,
                        rect.size.height,
                        CORNER_RADIUS,
                    );
                    CGContext::set_rgb_stroke_color(Some(cg), 1.0, 1.0, 1.0, 1.0);
                    CGContext::set_line_width(Some(cg), BORDER_WIDTH);
                    CGContext::stroke_path(Some(cg));

                    if workspace.show_windows {
                        for window in workspace.windows.iter() {
                            add_rounded_rect(
                                cg,
                                window.x,
                                window.y + y_offset,
                                window.width,
                                window.height,
                                1.5,
                            );
                            CGContext::set_rgb_fill_color(Some(cg), 1.0, 1.0, 1.0, 1.0);
                            CGContext::fill_path(Some(cg));

                            CGContext::save_g_state(Some(cg));
                            CGContext::set_blend_mode(Some(cg), CGBlendMode::DestinationOut);
                            CGContext::set_rgb_stroke_color(Some(cg), 1.0, 1.0, 1.0, 1.0);
                            CGContext::set_line_width(Some(cg), 1.5);
                            add_rounded_rect(
                                cg,
                                window.x,
                                window.y,
                                window.width,
                                window.height,
                                1.5,
                            );
                            CGContext::stroke_path(Some(cg));
                            CGContext::restore_g_state(Some(cg));
                        }
                    }

                    if let Some(label_line) = &workspace.label_line {
                        let text_width = label_line.width;
                        let text_center_y = bg_y + rect.size.height / 2.0;
                        let baseline_y = text_center_y - (label_line.ascent - label_line.descent) / 2.0;
                        let text_x = rect.origin.x + (rect.size.width - text_width) / 2.0;

                        CGContext::save_g_state(Some(cg));
                        if workspace.fill_alpha > 0.0 {
                            CGContext::set_rgb_fill_color(Some(cg), 0.0, 0.0, 0.0, 1.0);
                        } else {
                            CGContext::set_rgb_fill_color(Some(cg), 1.0, 1.0, 1.0, 1.0);
                        }
                        CGContext::set_text_position(Some(cg), text_x as CGFloat, baseline_y as CGFloat);
                        let line_ref: &CTLine = label_line.line.as_ref();
                        unsafe { line_ref.draw(cg) };
                        CGContext::restore_g_state(Some(cg));
                    }
                }

                CGContext::restore_g_state(Some(cg));
            }
        }
    }
);
