use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSEvent, NSImage, NSStatusWindowLevel, NSView, NSWindow,
    NSWindowCollectionBehavior, NSWindowLevel, NSWindowStyleMask,
};
use objc2_core_foundation::{CFAttributedString, CFDictionary, CFRetained, CGFloat, CGPoint, CGRect, CGSize};
use objc2_core_graphics::CGContext;
use objc2_core_text::CTLine;
use objc2_foundation::{
    MainThreadMarker, NSAttributedStringKey, NSDictionary, NSMutableDictionary, NSPoint, NSRect,
    NSSize, NSString,
};
use objc2::runtime::ProtocolObject;

use crate::actor;
use crate::actor::app::{WindowId, pid_t};
use crate::actor::config as config_actor;
use crate::actor::reactor;
use crate::common::config::{
    CenteredBarPosition, CenteredBarSettings, CenteredBarWindowLevel, Config,
};
use crate::model::server::{WindowData, WorkspaceData};
use crate::sys::app::NSRunningApplicationExt;
use crate::sys::screen::SpaceId;
use crate::sys::window_server::WindowServerId;

#[derive(Debug, Clone)]
pub struct UpdateDisplay {
    pub screen_id: u32,
    pub frame: CGRect,
    pub visible_frame: CGRect,
    pub space: Option<SpaceId>,
    pub workspaces: Vec<WorkspaceData>,
    pub active_workspace_idx: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct Update {
    pub displays: Vec<UpdateDisplay>,
}

pub enum Event {
    Update(Update),
    ConfigUpdated(Config),
}

pub type Sender = actor::Sender<Event>;
pub type Receiver = actor::Receiver<Event>;

const BAR_HEIGHT: f64 = 26.0;
const WORKSPACE_SPACING: f64 = 8.0;
const CONTENT_PADDING: f64 = 6.0;
const ICON_SIZE: f64 = 14.0;
const ICON_SPACING: f64 = 4.0;
const CORNER_RADIUS: f64 = 6.0;
const FONT_SIZE: f64 = 12.0;

pub struct CenteredBar {
    config: Config,
    rx: Receiver,
    mtm: MainThreadMarker,
    panels: HashMap<u32, DisplayPanel>,
    reactor_tx: reactor::Sender,
    _config_tx: config_actor::Sender,
    icon_cache: HashMap<pid_t, Retained<NSImage>>,
    last_signature: Option<u64>,
}

struct DisplayPanel {
    view: Retained<CenteredBarView>,
    panel: Retained<NSWindow>,
}

impl CenteredBar {
    pub fn new(
        config: Config,
        rx: Receiver,
        mtm: MainThreadMarker,
        reactor_tx: reactor::Sender,
        config_tx: config_actor::Sender,
    ) -> Self {
        Self {
            config,
            rx,
            mtm,
            panels: HashMap::new(),
            reactor_tx,
            _config_tx: config_tx,
            icon_cache: HashMap::new(),
            last_signature: None,
        }
    }

    pub async fn run(mut self) {
        while let Some((span, event)) = self.rx.recv().await {
            let _guard = span.enter();
            match event {
                Event::Update(update) => self.handle_update(update),
                Event::ConfigUpdated(cfg) => self.handle_config_updated(cfg),
            }
        }
        self.dispose_panels();
    }

    fn handle_config_updated(&mut self, cfg: Config) {
        self.config = cfg;
        self.last_signature = None;
        if !self.config.settings.ui.centered_bar.enabled {
            self.dispose_panels();
        }
    }

    fn handle_update(&mut self, update: Update) {
        if !self.config.settings.ui.centered_bar.enabled {
            self.dispose_panels();
            self.last_signature = None;
            return;
        }

        let sig = self.calc_signature(&update);
        if self.last_signature == Some(sig) {
            return;
        }
        self.last_signature = Some(sig);

        self.prune_panels(&update);

        for display in update.displays {
            if display.space.is_none() {
                continue;
            }
            if !self.panels.contains_key(&display.screen_id) {
                let panel = self.make_panel();
                self.panels.insert(display.screen_id, panel);
            }
            if let Some(mut panel) = self.panels.remove(&display.screen_id) {
                self.apply_display(&mut panel, &display);
                self.panels.insert(display.screen_id, panel);
            }
        }
    }

    fn dispose_panels(&mut self) {
        for (_, disp) in self.panels.drain() {
            disp.panel.orderOut(None::<&AnyObject>);
            disp.panel.close();
        }
    }

    fn prune_panels(&mut self, update: &Update) {
        let keep: std::collections::HashSet<u32> =
            update.displays.iter().map(|d| d.screen_id).collect();
        self.panels.retain(|screen_id, panel| {
            if keep.contains(screen_id) {
                true
            } else {
                panel.panel.orderOut(None::<&AnyObject>);
                panel.panel.close();
                false
            }
        });
    }

    fn make_panel(&self) -> DisplayPanel {
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0));
        let style = NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel;
        let panel: Retained<NSWindow> = unsafe {
            let obj = NSWindow::alloc(self.mtm);
            msg_send![obj, initWithContentRect: frame, styleMask: style, backing: NSBackingStoreType::Buffered, defer: false]
        };
        panel.setHasShadow(false);
        panel.setIgnoresMouseEvents(false);
        panel.setOpaque(false);
        panel.setBackgroundColor(Some(NSColor::clearColor().as_ref()));
        panel.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::Stationary,
        );

        let view = CenteredBarView::new(self.mtm);
        panel.setContentView(Some(&*view));
        DisplayPanel { view, panel }
    }

    fn apply_display(&mut self, display: &mut DisplayPanel, inputs: &UpdateDisplay) {
        let settings = &self.config.settings.ui.centered_bar;
        let layout = build_layout(inputs, settings, &mut self.icon_cache, self.mtm);

        if layout.workspaces.is_empty() {
            display.panel.orderOut(None);
            return;
        }

        display.view.set_layout(layout.clone());
        let size = NSSize::new(layout.total_width, layout.total_height);
        display.view.setFrameSize(size);

        let (x, y, width) = self.position_panel(inputs, size.width, settings);
        let frame = NSRect::new(NSPoint::new(x, y), NSSize::new(width, size.height));

        display.panel.setFrame_display(frame, true);
        let inset_x = ((width - size.width) / 2.0).max(0.0);
        display.view.setFrameOrigin(NSPoint::new(inset_x, 0.0));
        display.panel.setLevel(window_level_for(settings.window_level));
        display.panel.orderFrontRegardless();

        let reactor_tx = self.reactor_tx.clone();
        display.view.set_action_handler(Rc::new(move |action| match action {
            BarAction::Workspace(idx) => {
                let _ = reactor_tx.try_send(reactor::Event::Command(
                    reactor::Command::Layout(crate::layout_engine::LayoutCommand::SwitchToWorkspace(
                        idx,
                    )),
                ));
            }
            BarAction::Window { window_id, window_server_id } => {
                let _ = reactor_tx.try_send(reactor::Event::Command(
                    reactor::Command::Reactor(reactor::ReactorCommand::FocusWindow {
                        window_id,
                        window_server_id: window_server_id.map(WindowServerId::new),
                    }),
                ));
            }
        }));
    }

    fn position_panel(
        &self,
        inputs: &UpdateDisplay,
        content_width: f64,
        settings: &CenteredBarSettings,
    ) -> (f64, f64, f64) {
        let usable = inputs.frame;
        let full = inputs.visible_frame;
        let bar_w = content_width + 8.0;
        let target_height = BAR_HEIGHT;
        let mut x = (full.origin.x + full.size.width / 2.0) - (bar_w / 2.0);
        let y = match settings.position {
            CenteredBarPosition::BelowMenuBar => usable.origin.y + usable.size.height - target_height,
            CenteredBarPosition::OverlappingMenuBar => full.origin.y + full.size.height - target_height,
        };

        if settings.notch_aware && (full.size.width - usable.size.width).abs() > 40.0 {
            x = (usable.origin.x + usable.size.width / 2.0) - (bar_w / 2.0);
        }

        (x, y, bar_w)
    }

    fn calc_signature(&self, update: &Update) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.config.settings.ui.centered_bar.hash(&mut h);
        for display in &update.displays {
            display.screen_id.hash(&mut h);
            display.workspaces.len().hash(&mut h);
            for ws in &display.workspaces {
                ws.index.hash(&mut h);
                ws.is_active.hash(&mut h);
                ws.window_count.hash(&mut h);
                for win in &ws.windows {
                    win.id.hash(&mut h);
                    win.is_focused.hash(&mut h);
                }
            }
        }
        h.finish()
    }
}

fn window_level_for(level: CenteredBarWindowLevel) -> NSWindowLevel {
    match level {
        CenteredBarWindowLevel::Normal => objc2_app_kit::NSNormalWindowLevel,
        CenteredBarWindowLevel::Floating => objc2_app_kit::NSFloatingWindowLevel,
        CenteredBarWindowLevel::Status => NSStatusWindowLevel,
        CenteredBarWindowLevel::Screensaver => objc2_app_kit::NSScreenSaverWindowLevel,
        CenteredBarWindowLevel::Popup => objc2_app_kit::NSPopUpMenuWindowLevel,
    }
}

#[derive(Clone)]
struct WorkspaceRenderData {
    rect: CGRect,
    label: Option<CachedTextLine>,
    windows: Vec<WindowRenderData>,
    is_active: bool,
}

#[derive(Clone)]
struct WindowRenderData {
    rect: CGRect,
    rel_x: f64,
    icon: Option<Retained<NSImage>>,
    window_id: WindowId,
    window_server_id: Option<u32>,
    count: usize,
}

#[derive(Clone, Default)]
struct BarLayout {
    total_width: f64,
    total_height: f64,
    workspaces: Vec<WorkspaceRenderData>,
    hits: Vec<HitTarget>,
}

#[derive(Clone)]
struct HitTarget {
    rect: CGRect,
    target: BarAction,
}

#[derive(Clone)]
enum BarAction {
    Workspace(usize),
    Window { window_id: WindowId, window_server_id: Option<u32> },
}

#[derive(Clone)]
struct CachedTextLine {
    line: CFRetained<CTLine>,
    width: f64,
    ascent: f64,
    descent: f64,
}

struct CenteredBarViewIvars {
    layout: RefCell<BarLayout>,
    action_handler: RefCell<Option<Rc<dyn Fn(BarAction)>>>,
}

fn build_text_attrs(font: &objc2_app_kit::NSFont, color: &NSColor) -> Retained<NSDictionary<NSAttributedStringKey, AnyObject>> {
    let dict = NSMutableDictionary::<NSAttributedStringKey, AnyObject>::new();
    unsafe {
        dict.setObject_forKeyedSubscript(
            Some(as_any_object(font)),
            ProtocolObject::from_ref(objc2_app_kit::NSFontAttributeName),
        );
        dict.setObject_forKeyedSubscript(
            Some(as_any_object(color)),
            ProtocolObject::from_ref(objc2_app_kit::NSForegroundColorAttributeName),
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
    let cf_string: &objc2_core_foundation::CFString = label_ns.as_ref();
    let cf_dict_ref: &CFDictionary<NSAttributedStringKey, AnyObject> = attrs.as_ref();
    let cf_dict: &CFDictionary = cf_dict_ref.as_opaque();
    let attr_string = unsafe { CFAttributedString::new(None, Some(cf_string), Some(cf_dict)) }?;
    let line: CFRetained<CTLine> =
        unsafe { CTLine::with_attributed_string(attr_string.as_ref()) };

    let mut ascent: CGFloat = 0.0;
    let mut descent: CGFloat = 0.0;
    let mut leading: CGFloat = 0.0;
    let width = unsafe { line.typographic_bounds(&mut ascent, &mut descent, &mut leading) };

    Some(CachedTextLine {
        line,
        width: width as f64,
        ascent: ascent as f64,
        descent: descent as f64,
    })
}

fn as_any_object<T: Message>(obj: &T) -> &AnyObject {
    unsafe { &*(obj as *const T as *const AnyObject) }
}

fn build_layout(
    display: &UpdateDisplay,
    settings: &CenteredBarSettings,
    icon_cache: &mut HashMap<pid_t, Retained<NSImage>>,
    mtm: MainThreadMarker,
) -> BarLayout {
    let font = objc2_app_kit::NSFont::menuBarFontOfSize(FONT_SIZE);
    let color = NSColor::whiteColor();
    let text_attrs = build_text_attrs(font.as_ref(), color.as_ref());

    let mut layout = BarLayout::default();
    layout.total_height = BAR_HEIGHT;
    let mut offset_x = 0.0;

    for ws in display.workspaces.iter() {
        if settings.hide_empty_workspaces && ws.window_count == 0 {
            continue;
        }

        let label_text = if settings.show_numbers {
            if settings.show_mode_indicator && ws.is_active {
                format!("[M] {}", ws.index + 1)
            } else {
                format!("{}", ws.index + 1)
            }
        } else {
            ws.name.clone()
        };

        let label = build_cached_text_line(&label_text, &text_attrs);
        let label_width = label.as_ref().map(|l| l.width).unwrap_or(0.0);
        let mut windows = build_windows(ws, settings, icon_cache, mtm);
        let icons_width = if windows.is_empty() {
            0.0
        } else {
            (windows.len() as f64 * ICON_SIZE)
                + (windows.len().saturating_sub(1) as f64) * ICON_SPACING
        };

        let label_spacing = if label_width > 0.0 && icons_width > 0.0 {
            ICON_SPACING
        } else {
            0.0
        };
        let content_width = label_width + label_spacing + icons_width;
        let width = content_width.max(42.0) + (CONTENT_PADDING * 2.0);

        let rect = CGRect::new(CGPoint::new(offset_x, 0.0), CGSize::new(width, BAR_HEIGHT));
        let start_x = rect.origin.x + CONTENT_PADDING + label_width + label_spacing;
        for window in windows.iter_mut() {
            window.rect.origin.x = start_x + window.rel_x;
            window.rect.origin.y = rect.origin.y + (BAR_HEIGHT - ICON_SIZE) / 2.0;
        }
        layout.hits.push(HitTarget {
            rect,
            target: BarAction::Workspace(ws.index),
        });
        for window in &windows {
            layout.hits.push(HitTarget {
                rect: window.rect,
                target: BarAction::Window {
                    window_id: window.window_id,
                    window_server_id: window.window_server_id,
                },
            });
        }
        layout.workspaces.push(WorkspaceRenderData {
            rect,
            label,
            windows,
            is_active: ws.is_active,
        });
        offset_x += width + WORKSPACE_SPACING;
    }

    layout.total_width = if offset_x > 0.0 { offset_x - WORKSPACE_SPACING } else { 0.0 };
    layout
}

fn build_windows(
    ws: &WorkspaceData,
    settings: &CenteredBarSettings,
    icon_cache: &mut HashMap<pid_t, Retained<NSImage>>,
    mtm: MainThreadMarker,
) -> Vec<WindowRenderData> {
    let mut windows = Vec::new();
    let mut arrangement: Vec<(Option<Retained<NSImage>>, usize, WindowId, Option<u32>, bool)> =
        Vec::new();
    if settings.deduplicate_icons {
        let mut grouped: HashMap<(Option<String>, pid_t), Vec<&WindowData>> = HashMap::new();
        for w in &ws.windows {
            grouped
                .entry((w.bundle_id.clone(), w.id.pid))
                .or_default()
                .push(w);
        }
        for ((_bundle, pid), group) in grouped {
            let icon = icon_for_pid(pid, icon_cache, mtm);
            let first = group[0];
            let _focused = group.iter().any(|w| w.is_focused);
            arrangement.push((icon, group.len(), first.id, first.window_server_id, false));
        }
    } else {
        for w in &ws.windows {
            arrangement.push((
                icon_for_pid(w.id.pid, icon_cache, mtm),
                1,
                w.id,
                w.window_server_id,
                false,
            ));
        }
    }

    let mut x = 0.0;
    for (icon, count, window_id, window_server_id, _focused) in arrangement {
        let rect = CGRect::new(
            CGPoint::new(x, (BAR_HEIGHT - ICON_SIZE) / 2.0),
            CGSize::new(ICON_SIZE, ICON_SIZE),
        );
        windows.push(WindowRenderData {
            rect,
            rel_x: x,
            icon,
            window_id,
            window_server_id,
            count,
        });
        x += ICON_SIZE + ICON_SPACING;
    }
    windows
}

fn icon_for_pid(
    pid: pid_t,
    cache: &mut HashMap<pid_t, Retained<NSImage>>,
    mtm: MainThreadMarker,
) -> Option<Retained<NSImage>> {
    if cache.contains_key(&pid) {
        return cache.get(&pid).cloned();
    }

    let icon: Option<Retained<NSImage>> = NSRunningApplicationExt::with_process_id(pid)
        .and_then(|app: Retained<objc2_app_kit::NSRunningApplication>| app.icon());
    if let Some(ic) = icon {
        cache.insert(pid, ic.clone());
        Some(ic)
    } else {
        let _ = mtm; // keep for consistency; nothing to do
        None
    }
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "RiftCenteredBarView"]
    #[ivars = CenteredBarViewIvars]
    struct CenteredBarView;

    impl CenteredBarView {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            let ivars = self.ivars();
            let layout = ivars.layout.borrow();
            let bounds = self.bounds();
            if let Some(ctx) = objc2_app_kit::NSGraphicsContext::currentContext() {
                let cg_ctx = ctx.CGContext();
                let cg = cg_ctx.as_ref();
                CGContext::save_g_state(Some(cg));
                CGContext::clear_rect(Some(cg), bounds);

                let y_offset = (bounds.size.height - layout.total_height) / 2.0;
                for workspace in layout.workspaces.iter() {
                    let rect = workspace.rect;
                    let y = rect.origin.y + y_offset;
                    add_rounded_rect(cg, rect.origin.x, y, rect.size.width, rect.size.height, CORNER_RADIUS);
                    let (fill_r, fill_g, fill_b, fill_a) = if workspace.is_active {
                        (1.0, 1.0, 1.0, 0.18)
                    } else {
                        (0.1, 0.1, 0.1, 0.55)
                    };
                    CGContext::set_rgb_fill_color(Some(cg), fill_r, fill_g, fill_b, fill_a);
                    CGContext::fill_path(Some(cg));

                    add_rounded_rect(cg, rect.origin.x, y, rect.size.width, rect.size.height, CORNER_RADIUS);
                    let border_alpha = if workspace.is_active { 0.9 } else { 0.4 };
                    CGContext::set_rgb_stroke_color(Some(cg), 1.0, 1.0, 1.0, border_alpha);
                    CGContext::set_line_width(Some(cg), 1.0);
                    CGContext::stroke_path(Some(cg));

                    if let Some(label) = &workspace.label {
                        let text_center_y = y + rect.size.height / 2.0;
                        let baseline_y = text_center_y - (label.ascent - label.descent) / 2.0;
                        let text_x = rect.origin.x + CONTENT_PADDING;
                        CGContext::save_g_state(Some(cg));
                        CGContext::set_rgb_fill_color(Some(cg), 1.0, 1.0, 1.0, 0.9);
                        CGContext::set_text_position(Some(cg), text_x as CGFloat, baseline_y as CGFloat);
                        let line_ref: &CTLine = label.line.as_ref();
                        unsafe { line_ref.draw(cg) };
                        CGContext::restore_g_state(Some(cg));

                        for window in workspace.windows.iter() {
                            let rect = CGRect::new(
                                CGPoint::new(window.rect.origin.x, window.rect.origin.y + y_offset),
                                window.rect.size,
                            );
                            draw_window_icon(cg, window, rect);
                        }
                    } else {
                        for window in workspace.windows.iter() {
                            let rect = CGRect::new(
                                CGPoint::new(window.rect.origin.x, window.rect.origin.y + y_offset),
                                window.rect.size,
                            );
                            draw_window_icon(cg, window, rect);
                        }
                    }
                }

                CGContext::restore_g_state(Some(cg));
            }
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let pt_window = event.locationInWindow();
            let local = self.convertPoint_fromView(pt_window, None);
            let layout = self.ivars().layout.borrow();
            for hit in layout.hits.iter() {
                let rect = hit.rect;
                if rect_contains(rect, CGPoint::new(local.x, local.y)) {
                    if let Some(handler) = self.ivars().action_handler.borrow().as_ref() {
                        handler(hit.target.clone());
                    }
                    break;
                }
            }
        }
    }
);

impl CenteredBarView {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let frame = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(0.0, 0.0));
        let view = mtm.alloc().set_ivars(CenteredBarViewIvars {
            layout: RefCell::new(BarLayout::default()),
            action_handler: RefCell::new(None),
        });
        unsafe { msg_send![super(view), initWithFrame: frame] }
    }

    fn set_layout(&self, layout: BarLayout) {
        *self.ivars().layout.borrow_mut() = layout;
        self.setNeedsDisplay(true);
    }

    fn set_action_handler(&self, handler: Rc<dyn Fn(BarAction)>) {
        *self.ivars().action_handler.borrow_mut() = Some(handler);
    }
}

fn draw_window_icon(ctx: &CGContext, window: &WindowRenderData, rect: CGRect) {
    if let Some(icon) = &window.icon {
        icon.drawInRect(rect);
    } else {
        add_rounded_rect(ctx, rect.origin.x, rect.origin.y, rect.size.width, rect.size.height, 3.0);
        CGContext::set_rgb_fill_color(Some(ctx), 1.0, 1.0, 1.0, 0.6);
        CGContext::fill_path(Some(ctx));
    }

    if window.count > 1 {
        let badge_size = 12.0_f64;
        let badge_rect = CGRect::new(
            CGPoint::new(rect.origin.x + rect.size.width - (badge_size / 2.0), rect.origin.y + rect.size.height - (badge_size / 2.0)),
            CGSize::new(badge_size, badge_size),
        );
        add_rounded_rect(ctx, badge_rect.origin.x, badge_rect.origin.y, badge_rect.size.width, badge_rect.size.height, badge_size / 2.0);
        CGContext::set_rgb_fill_color(Some(ctx), 0.8, 0.2, 0.2, 1.0);
        CGContext::fill_path(Some(ctx));
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

fn rect_contains(rect: CGRect, point: CGPoint) -> bool {
    point.x >= rect.origin.x
        && point.x <= rect.origin.x + rect.size.width
        && point.y >= rect.origin.y
        && point.y <= rect.origin.y + rect.size.height
}
