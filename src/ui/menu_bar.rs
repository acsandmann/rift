// many ideas for how this works were taken from https://github.com/xiamaz/YabaiIndicator
use std::borrow::Cow;
#[cfg(test)]
use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use cgs::{Canvas, Menu, MenuItem, OpenPanel, SavePanel, StatusItem, Ui};
use objc2::Message;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2_app_kit::{
    NSAlert, NSColor, NSControlStateValueOff, NSControlStateValueOn, NSEventModifierFlags, NSFont,
    NSFontAttributeName, NSFontWeightBold, NSFontWeightMedium, NSMenuItem, NSSavePanel, NSView,
};
use objc2_core_foundation::{
    CFAttributedString, CFDictionary, CFRetained, CFString, CGFloat, CGPoint, CGRect, CGSize,
};
use objc2_core_graphics::{CGBlendMode, CGContext};
use objc2_core_text::{CTLine, kCTForegroundColorFromContextAttributeName};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSAttributedStringKey, NSDictionary, NSMutableDictionary, NSNumber,
    NSSize, NSString, NSURL,
};
use tokio::sync::mpsc::UnboundedSender;

use crate::actor::menu_bar::{DisplayWorkspaces, Workspace};
use crate::actor::reactor::{
    Command as ReactorTopCommand, Event as ReactorEvent, ReactorCommand, Sender as ReactorSender,
};
use crate::actor::wm_controller::{WmCmd, WmCommand};
use crate::common::config::{
    ActiveWorkspaceLabel, LayoutMode, MenuBarDisplayMode, MenuBarSettings, WorkspaceDisplayStyle,
    WorkspaceSelector, restore_file,
};
use crate::layout_engine::{LayoutCommand, LayoutEngine, RestoreScope, RestoreSource};
use crate::sys::hotkey::{Hotkey, KeyCode, Modifiers};
use crate::sys::screen::SpaceId;
use crate::ui::common::compute_window_layout_metrics;

const CELL_WIDTH: f64 = 20.0;
const CELL_HEIGHT: f64 = 15.0;
const CELL_SPACING: f64 = 4.0;
const SEPARATOR_SPACING: f64 = 3.0;
const SEPARATOR_BOTTOM_PADDING: f64 = 3.0;
// Compact point-space geometry: single digits stay square, longer labels expand.
const LABEL_HEIGHT: f64 = 16.0;
const LABEL_SPACING: f64 = 3.0;
const LABEL_BORDER_WIDTH: f64 = 1.0;
const LABEL_CORNER_RADIUS: f64 = 2.0;
const LABEL_PADDING: f64 = 3.0;
const CORNER_RADIUS: f64 = 3.0;
const BORDER_WIDTH: f64 = 1.0;
const CONTENT_INSET: f64 = 2.0;
const FONT_SIZE: f64 = 11.0;

#[cfg(test)]
thread_local! {
    static LAYOUT_LIBRARY_SCANS: Cell<usize> = const { Cell::new(0) };
}

#[derive(Debug, Clone)]
pub enum MenuAction {
    SetLayout(LayoutMode),
    ToggleSpaceActivated,
    NextWorkspace,
    PrevWorkspace,
    SwitchToWorkspace(usize),
    SaveLayout(PathBuf),
    SaveMasterFile,
    RestoreLayout {
        path: PathBuf,
        scope: RestoreScope,
        source: RestoreSource,
    },
    RestoreMasterFile(RestoreScope),
    RefreshLayoutFiles,
    OpenGitHub,
    OpenDocumentation,
    OpenMatrix,
    OpenSponsor,
    OpenConfig,
    OpenSettings,
    SettingsClosed,
    /// Background Settings application discovery finished.
    InstalledApplications(Vec<(String, String)>),
    ReloadConfig,
    QuitRift,
}

pub struct MenuIcon {
    status_item: StatusItem,
    view: MenuIconView,
    _menu: Rc<Menu>,
    menu_handler: Rc<MenuActionHandler>,
    layout_items: Vec<(LayoutMode, Retained<NSMenuItem>)>,
    workspace_item: Retained<NSMenuItem>,
    workspace_submenu: Rc<Menu>,
    workspace_items: Vec<WorkspaceMenuItem>,
    next_workspace_item: Retained<NSMenuItem>,
    prev_workspace_item: Retained<NSMenuItem>,
    tiling_item: Retained<NSMenuItem>,
    reload_item: Retained<NSMenuItem>,
    quit_item: Retained<NSMenuItem>,
    restore_workspace_menu: Rc<Menu>,
    restore_space_menu: Rc<Menu>,
    layout_folder: PathBuf,
    mtm: MainThreadMarker,
    prev_width: f64,
    render_key: Option<MenuIconRenderKey>,
}

struct WorkspaceMenuItem {
    identity: String,
    index: usize,
    name: String,
    item: Retained<NSMenuItem>,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkspaceTopology {
    identity: String,
    index: usize,
    name: String,
}

#[cfg(test)]
fn workspace_topology(workspaces: &[Workspace]) -> Vec<WorkspaceTopology> {
    workspaces
        .iter()
        .map(|workspace| WorkspaceTopology {
            identity: workspace.id.clone(),
            index: workspace.index,
            name: workspace.name.clone(),
        })
        .collect()
}

impl MenuIcon {
    pub fn new(
        mtm: MainThreadMarker,
        action_tx: UnboundedSender<MenuAction>,
        reactor_tx: ReactorSender,
        layout_folder: &Path,
    ) -> Self {
        let status_item = StatusItem::new(&Ui::new(mtm));
        let view = MenuIconView::new(mtm);
        let menu_handler = MenuActionHandler::new(action_tx, reactor_tx);
        let built = build_static_menu(mtm, &menu_handler);
        status_item.ns_status_item().setMenu(Some(built.menu.ns_menu()));
        if let Some(btn) = status_item.ns_status_item().button(mtm) {
            btn.addSubview(&*view);
            view.setFrameSize(NSSize::new(0.0, 0.0));
            status_item.ns_status_item().setVisible(true);
        }

        let mut this = Self {
            status_item,
            view,
            _menu: built.menu,
            menu_handler,
            layout_items: built.layout_items,
            workspace_item: built.workspace_item,
            workspace_submenu: built.workspace_submenu,
            workspace_items: Vec::new(),
            next_workspace_item: built.next_workspace_item,
            prev_workspace_item: built.prev_workspace_item,
            tiling_item: built.tiling_item,
            reload_item: built.reload_item,
            quit_item: built.quit_item,
            restore_workspace_menu: built.restore_workspace_menu,
            restore_space_menu: built.restore_space_menu,
            layout_folder: layout_folder.to_path_buf(),
            mtm,
            prev_width: 0.0,
            render_key: None,
        };
        this.menu_handler.set_layout_folder(layout_folder.to_path_buf());
        this.refresh_layout_library();
        this
    }

    pub fn update_config(&mut self, settings: &MenuBarSettings, hotkeys: &[(Hotkey, WmCommand)]) {
        let shortcuts = MenuShortcuts::from_hotkeys(hotkeys);
        set_menu_item_hotkey(&self.next_workspace_item, shortcuts.next_workspace.as_ref());
        set_menu_item_hotkey(&self.prev_workspace_item, shortcuts.prev_workspace.as_ref());
        set_menu_item_hotkey(&self.tiling_item, shortcuts.toggle_space_activation.as_ref());
        set_menu_item_hotkey(&self.reload_item, shortcuts.reload_config.as_ref());
        set_menu_item_hotkey(&self.quit_item, shortcuts.quit_rift.as_ref());
        for workspace in &self.workspace_items {
            let shortcut = shortcuts
                .switch_workspace_by_index
                .get(&workspace.index)
                .or_else(|| shortcuts.switch_workspace_by_name.get(&workspace.name));
            set_menu_item_hotkey(&workspace.item, shortcut);
        }

        let layout_folder = settings.resolved_layout_folder();
        if layout_folder != self.layout_folder {
            self.layout_folder = layout_folder.clone();
            self.menu_handler.set_layout_folder(layout_folder);
            self.refresh_layout_library();
        }
    }

    pub fn sync_workspace_topology(
        &mut self,
        workspaces: &[Workspace],
        hotkeys: &[(Hotkey, WmCommand)],
    ) {
        let unchanged = self.workspace_items.len() == workspaces.len()
            && self.workspace_items.iter().zip(workspaces).all(|(item, workspace)| {
                item.identity == workspace.id
                    && item.index == workspace.index
                    && item.name == workspace.name
            });
        if unchanged {
            return;
        }

        for workspace in self.workspace_items.drain(..) {
            self.workspace_submenu.remove(&workspace.item);
        }

        let shortcuts = MenuShortcuts::from_hotkeys(hotkeys);
        for workspace in workspaces {
            let item = make_menu_item(
                self.mtm,
                &workspace_menu_title(workspace),
                Some(MenuActionHandler::on_switch_workspace),
                Some(&self.menu_handler),
            );
            item.ns_menu_item().setTag(workspace.index as isize);
            set_menu_item_checked(item.ns_menu_item(), workspace.is_active);
            set_menu_item_hotkey(
                item.ns_menu_item(),
                shortcuts
                    .switch_workspace_by_index
                    .get(&workspace.index)
                    .or_else(|| shortcuts.switch_workspace_by_name.get(&workspace.name)),
            );
            let native = item.ns_menu_item().retain();
            self.workspace_submenu.add(item);
            self.workspace_items.push(WorkspaceMenuItem {
                identity: workspace.id.clone(),
                index: workspace.index,
                name: workspace.name.clone(),
                item: native,
            });
        }

        self.workspace_item.setEnabled(!workspaces.is_empty());
    }

    pub fn update_menu_state(&self, active_space_is_activated: bool, workspaces: &[Workspace]) {
        let active = workspaces.iter().find(|workspace| workspace.is_active);
        let active_layout = active.and_then(|workspace| parse_layout_mode(&workspace.layout_mode));
        let active_id = active.map(|workspace| workspace.id.as_str());

        for (mode, item) in &self.layout_items {
            set_menu_item_checked(item, active_layout == Some(*mode));
        }
        for workspace in &self.workspace_items {
            set_menu_item_checked(&workspace.item, active_id == Some(workspace.identity.as_str()));
        }
        set_menu_item_checked(&self.tiling_item, active_space_is_activated);
    }

    pub fn refresh_layout_library(&mut self) {
        let files = layout_library_files_in(&self.layout_folder);
        self.menu_handler
            .set_layout_files(files.iter().map(|(_, path)| path.clone()).collect());

        rebuild_restore_menu(
            self.mtm,
            &self.menu_handler,
            &self.restore_workspace_menu,
            &files,
            MenuActionHandler::on_restore_master_file_workspace,
            MenuActionHandler::on_restore_library_workspace,
            MenuActionHandler::on_restore_workspace,
        );
        rebuild_restore_menu(
            self.mtm,
            &self.menu_handler,
            &self.restore_space_menu,
            &files,
            MenuActionHandler::on_restore_master_file_space,
            MenuActionHandler::on_restore_library_space,
            MenuActionHandler::on_restore_space,
        );
    }

    pub fn update_status_icon(
        &mut self,
        displays: &[DisplayWorkspaces],
        settings: &MenuBarSettings,
    ) {
        let render_inputs = render_inputs(displays, settings);
        let render_changed =
            self.render_key.as_ref().is_none_or(|key| !key.matches_inputs(&render_inputs));

        if render_inputs.is_empty() {
            if self.status_item.ns_status_item().isVisible() {
                self.status_item.ns_status_item().setVisible(false);
            }
            self.prev_width = 0.0;
            if render_changed {
                self.render_key = Some(MenuIconRenderKey::from_inputs(&render_inputs));
            }
            return;
        }

        if render_changed {
            let layout = { build_layout(&render_inputs, self.view.text_attrs.as_ref()) };
            self.view.set_layout(layout);
            self.render_key = Some(MenuIconRenderKey::from_inputs(&render_inputs));
        }
        if !self.status_item.ns_status_item().isVisible() {
            self.status_item.ns_status_item().setVisible(true);
        }

        let size = self.view.layout.borrow().size;
        let width_changed = self.prev_width != size.width;
        if width_changed {
            self.prev_width = size.width;
            self.status_item.ns_status_item().setLength(size.width);
        }

        if let Some(button) = self.status_item.ns_status_item().button(self.mtm) {
            if width_changed {
                button.setNeedsLayout(true);
            }
            let frame = self.view.frame();
            if frame.size != size {
                self.view.setFrameSize(size);
            }
            let bounds = button.bounds();
            let origin = CGPoint::new(
                centered_origin(bounds.origin.x, bounds.size.width, size.width),
                centered_origin(bounds.origin.y, bounds.size.height, size.height),
            );
            if frame.origin != origin {
                self.view.setFrameOrigin(origin);
            }
        }
    }
}

#[derive(Default)]
struct MenuIconLayout {
    size: CGSize,
    workspaces: Vec<WorkspaceRenderData>,
    separators: Vec<f64>,
    separator_line: Option<CachedTextLine>,
}

struct WorkspaceRenderData {
    bg_rect: CGRect,
    fill_alpha: f64,
    windows: Vec<CGRect>,
    label_line: Option<CachedTextLine>,
}

struct WorkspaceRenderInput<'a> {
    display_uuid: &'a str,
    space: SpaceId,
    workspace: &'a Workspace,
    label: Cow<'a, str>,
    show_windows: bool,
}

fn render_inputs<'a>(
    displays: &'a [DisplayWorkspaces],
    settings: &MenuBarSettings,
) -> Vec<WorkspaceRenderInput<'a>> {
    let show_windows = settings.display_style == WorkspaceDisplayStyle::Layout;
    displays
        .iter()
        .flat_map(|display| {
            display
                .workspaces
                .iter()
                .filter(|workspace| match settings.mode {
                    MenuBarDisplayMode::All => {
                        settings.show_empty || workspace.window_count > 0 || workspace.is_active
                    }
                    MenuBarDisplayMode::Active => workspace.is_active,
                })
                .map(move |workspace| WorkspaceRenderInput {
                    display_uuid: &display.display_uuid,
                    space: display.space,
                    workspace,
                    label: if show_windows {
                        Cow::Borrowed("")
                    } else {
                        workspace_label(workspace, settings.active_label)
                    },
                    show_windows,
                })
        })
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
struct MenuIconRenderKey {
    workspaces: Vec<WorkspaceRenderKey>,
}

#[derive(Debug, PartialEq, Eq)]
struct WorkspaceRenderKey {
    display_uuid: String,
    space: SpaceId,
    workspace_id: String,
    label: String,
    show_windows: bool,
    is_active: bool,
    window_count: usize,
    window_frames: Vec<[u64; 4]>,
}

impl MenuIconRenderKey {
    fn from_inputs(inputs: &[WorkspaceRenderInput<'_>]) -> Self {
        let workspaces = inputs
            .iter()
            .map(|input| WorkspaceRenderKey {
                display_uuid: input.display_uuid.to_owned(),
                space: input.space,
                workspace_id: input.workspace.id.clone(),
                label: input.label.to_string(),
                show_windows: input.show_windows,
                is_active: input.workspace.is_active,
                window_count: input
                    .show_windows
                    .then_some(input.workspace.window_count)
                    .unwrap_or_default(),
                window_frames: input
                    .show_windows
                    .then(|| {
                        input
                            .workspace
                            .windows
                            .iter()
                            .map(|window| {
                                let frame = *window;
                                [
                                    frame.origin.x.to_bits(),
                                    frame.origin.y.to_bits(),
                                    frame.size.width.to_bits(),
                                    frame.size.height.to_bits(),
                                ]
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            })
            .collect();
        Self { workspaces }
    }

    fn matches_inputs(&self, inputs: &[WorkspaceRenderInput<'_>]) -> bool {
        self.workspaces.len() == inputs.len()
            && self.workspaces.iter().zip(inputs).all(|(key, input)| {
                key.display_uuid == input.display_uuid
                    && key.space == input.space
                    && key.workspace_id == input.workspace.id
                    && key.label == input.label
                    && key.show_windows == input.show_windows
                    && key.is_active == input.workspace.is_active
                    && (!input.show_windows || key.window_count == input.workspace.window_count)
                    && (!input.show_windows
                        || (key.window_frames.len() == input.workspace.windows.len()
                            && key.window_frames.iter().zip(&input.workspace.windows).all(
                                |(frame_key, window)| {
                                    let frame = *window;
                                    *frame_key
                                        == [
                                            frame.origin.x.to_bits(),
                                            frame.origin.y.to_bits(),
                                            frame.size.width.to_bits(),
                                            frame.size.height.to_bits(),
                                        ]
                                },
                            )))
            })
    }
}

struct CachedTextLine {
    line: CFRetained<CTLine>,
    width: f64,
    ascent: f64,
    descent: f64,
}

struct MenuIconView {
    canvas: Canvas,
    layout: Rc<RefCell<MenuIconLayout>>,
    text_attrs: Retained<NSDictionary<NSAttributedStringKey, AnyObject>>,
}

#[inline]
fn centered_origin(container_origin: f64, container_size: f64, content_size: f64) -> f64 {
    container_origin + (container_size - content_size) / 2.0
}

fn workspace_label(workspace: &Workspace, label_style: ActiveWorkspaceLabel) -> Cow<'_, str> {
    match label_style {
        ActiveWorkspaceLabel::Index => Cow::Owned((workspace.index + 1).to_string()),
        ActiveWorkspaceLabel::Name if !workspace.name.is_empty() => {
            Cow::Borrowed(workspace.name.as_str())
        }
        ActiveWorkspaceLabel::Name => Cow::Owned((workspace.index + 1).to_string()),
    }
}

#[inline]
fn workspace_cell_rect(x: f64, width: f64, height: f64) -> CGRect {
    // Keep the centered stroke entirely inside its cell so the first and last
    // borders are not clipped by the view bounds.
    let stroke_inset = BORDER_WIDTH / 2.0;
    CGRect::new(
        CGPoint::new(x + stroke_inset, stroke_inset),
        CGSize::new(width - BORDER_WIDTH, height - BORDER_WIDTH),
    )
}

fn as_any_object<T: Message>(obj: &T) -> &AnyObject {
    unsafe { &*(obj as *const T as *const AnyObject) }
}

fn parse_layout_mode(layout_mode: &str) -> Option<LayoutMode> {
    match layout_mode {
        "traditional" => Some(LayoutMode::Traditional),
        "bsp" => Some(LayoutMode::Bsp),
        "floating" => Some(LayoutMode::Floating),
        "stack" => Some(LayoutMode::Stack),
        "master_stack" => Some(LayoutMode::MasterStack),
        "scrolling" => Some(LayoutMode::Scrolling),
        _ => None,
    }
}

fn layout_title(mode: &LayoutMode) -> &'static str {
    match mode {
        LayoutMode::Traditional => "Traditional",
        LayoutMode::Bsp => "BSP",
        LayoutMode::Floating => "Floating",
        LayoutMode::Stack => "Stack",
        LayoutMode::MasterStack => "Master Stack",
        LayoutMode::Scrolling => "Scrolling",
    }
}

type MenuCallback = fn(&MenuActionHandler, Option<&NSMenuItem>);

fn make_menu(mtm: MainThreadMarker, title: &str) -> Rc<Menu> {
    let menu = Menu::new(&Ui::new(mtm));
    menu.ns_menu().setTitle(&NSString::from_str(title));
    Rc::new(menu)
}

fn make_menu_item(
    mtm: MainThreadMarker,
    title: &str,
    action: Option<MenuCallback>,
    target: Option<&Rc<MenuActionHandler>>,
) -> MenuItem {
    let item = MenuItem::new(&Ui::new(mtm), title);
    if let (Some(action), Some(target)) = (action, target) {
        let target = target.clone();
        item.on_activate(move |sender| action(&target, Some(sender)))
    } else {
        item
    }
}

fn add_action_item(
    menu: &Menu,
    mtm: MainThreadMarker,
    handler: &Rc<MenuActionHandler>,
    title: &str,
    action: MenuCallback,
) -> Retained<NSMenuItem> {
    let item = make_menu_item(mtm, title, Some(action), Some(handler));
    let native = item.ns_menu_item().retain();
    menu.add(item);
    native
}

fn add_submenu(
    menu: &Menu,
    mtm: MainThreadMarker,
    title: &str,
) -> (Retained<NSMenuItem>, Rc<Menu>) {
    let submenu = make_menu(mtm, title);
    let item = MenuItem::new(&Ui::new(mtm), title).submenu(submenu.clone());
    let native = item.ns_menu_item().retain();
    menu.add(item);
    (native, submenu)
}

fn add_separator(menu: &Menu) { menu.add_separator(); }

fn set_menu_item_checked(item: &NSMenuItem, checked: bool) {
    let state = if checked {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    };
    if item.state() != state {
        item.setState(state);
    }
}

fn set_menu_item_hotkey(item: &NSMenuItem, hotkey: Option<&Hotkey>) {
    let (key, modifiers) = hotkey
        .and_then(menu_hotkey_to_key_equivalent)
        .unwrap_or(("", NSEventModifierFlags::empty()));
    item.setKeyEquivalent(&NSString::from_str(key));
    item.setKeyEquivalentModifierMask(modifiers);
}

fn workspace_menu_title(workspace: &Workspace) -> String {
    if workspace.name.is_empty() {
        format!("Workspace {}", workspace.index + 1)
    } else {
        format!("{} ({})", workspace.name, workspace.index + 1)
    }
}

fn rebuild_restore_menu(
    mtm: MainThreadMarker,
    handler: &Rc<MenuActionHandler>,
    menu: &Menu,
    files: &[(String, PathBuf)],
    master_action: MenuCallback,
    library_action: MenuCallback,
    picker_action: MenuCallback,
) {
    menu.clear();
    add_action_item(menu, mtm, handler, "Master Layout", master_action);

    if !files.is_empty() {
        add_separator(menu);
        for (index, (name, _)) in files.iter().enumerate() {
            let item = add_action_item(menu, mtm, handler, name, library_action);
            item.setTag(index as isize);
        }
    }

    add_separator(menu);
    add_action_item(menu, mtm, handler, "Choose Layout File…", picker_action);
}

fn layout_file_title(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    (!stem.starts_with('.')).then(|| stem.to_owned())
}

fn layout_library_files_in(directory: &Path) -> Vec<(String, PathBuf)> {
    #[cfg(test)]
    LAYOUT_LIBRARY_SCANS.with(|scans| scans.set(scans.get() + 1));
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut layouts = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .filter(|path| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("ron"))
        })
        .filter_map(|path| layout_file_title(&path).map(|title| (title, path)))
        .collect::<Vec<_>>();
    layouts.sort_by(|(title_a, path_a), (title_b, path_b)| {
        title_a
            .to_ascii_lowercase()
            .cmp(&title_b.to_ascii_lowercase())
            .then_with(|| path_a.cmp(path_b))
    });
    layouts
}

struct BuiltStatusMenu {
    menu: Rc<Menu>,
    layout_items: Vec<(LayoutMode, Retained<NSMenuItem>)>,
    workspace_item: Retained<NSMenuItem>,
    workspace_submenu: Rc<Menu>,
    next_workspace_item: Retained<NSMenuItem>,
    prev_workspace_item: Retained<NSMenuItem>,
    tiling_item: Retained<NSMenuItem>,
    reload_item: Retained<NSMenuItem>,
    quit_item: Retained<NSMenuItem>,
    restore_workspace_menu: Rc<Menu>,
    restore_space_menu: Rc<Menu>,
}

fn build_static_menu(mtm: MainThreadMarker, handler: &Rc<MenuActionHandler>) -> BuiltStatusMenu {
    // Focus-follows-mouse must stand down while this menu tracks, or the
    // pointer raises whatever window sits underneath and dismisses it.
    let reactor = handler.reactor_tx.clone();
    let menu = Rc::new(Menu::new(&Ui::new(mtm)).on_tracking(move |open| {
        reactor.send(if open {
            ReactorEvent::MenuOpened(own_pid())
        } else {
            ReactorEvent::MenuClosed(own_pid())
        });
    }));
    menu.ns_menu().setTitle(&NSString::from_str("Rift"));

    let tiling_item = add_action_item(
        &menu,
        mtm,
        handler,
        "Tiling",
        MenuActionHandler::on_toggle_space_activation,
    );
    set_menu_item_checked(&tiling_item, false);
    add_separator(&menu);

    let (workspace_item, workspace_submenu) = add_submenu(&menu, mtm, "Workspace");
    let next_workspace_item = add_action_item(
        &workspace_submenu,
        mtm,
        handler,
        "Next Workspace",
        MenuActionHandler::on_next_workspace,
    );
    let prev_workspace_item = add_action_item(
        &workspace_submenu,
        mtm,
        handler,
        "Previous Workspace",
        MenuActionHandler::on_prev_workspace,
    );
    add_separator(&workspace_submenu);
    workspace_item.setEnabled(false);

    let (_, layout_submenu) = add_submenu(&menu, mtm, "Layout");
    let mut layout_items = Vec::with_capacity(5);
    for mode in [
        LayoutMode::Traditional,
        LayoutMode::Bsp,
        LayoutMode::Floating,
        LayoutMode::Stack,
        LayoutMode::MasterStack,
        LayoutMode::Scrolling,
    ] {
        let action = match mode {
            LayoutMode::Traditional => MenuActionHandler::on_set_layout_traditional,
            LayoutMode::Bsp => MenuActionHandler::on_set_layout_bsp,
            LayoutMode::Floating => MenuActionHandler::on_set_layout_floating,
            LayoutMode::Stack => MenuActionHandler::on_set_layout_stack,
            LayoutMode::MasterStack => MenuActionHandler::on_set_layout_master_stack,
            LayoutMode::Scrolling => MenuActionHandler::on_set_layout_scrolling,
        };
        let item = add_action_item(&layout_submenu, mtm, handler, layout_title(&mode), action);
        set_menu_item_checked(&item, false);
        layout_items.push((mode, item));
    }

    let (_, saved_layouts_menu) = add_submenu(&menu, mtm, "Saved Layouts");
    add_action_item(
        &saved_layouts_menu,
        mtm,
        handler,
        "Save Layout As…",
        MenuActionHandler::on_save_layout,
    );
    add_action_item(
        &saved_layouts_menu,
        mtm,
        handler,
        "Update Master Layout",
        MenuActionHandler::on_save_master_file,
    );
    add_separator(&saved_layouts_menu);
    let (_, restore_workspace_menu) = add_submenu(&saved_layouts_menu, mtm, "Restore Workspace");
    let (_, restore_space_menu) = add_submenu(&saved_layouts_menu, mtm, "Restore Space");

    add_separator(&menu);
    let reload_item = add_action_item(
        &menu,
        mtm,
        handler,
        "Reload Config",
        MenuActionHandler::on_reload_config,
    );
    let settings_item = add_action_item(
        &menu,
        mtm,
        handler,
        "Settings…",
        MenuActionHandler::on_open_settings,
    );
    settings_item.setKeyEquivalent(&NSString::from_str(","));
    settings_item.setKeyEquivalentModifierMask(NSEventModifierFlags::Command);

    add_separator(&menu);
    let (_, help_menu) = add_submenu(&menu, mtm, "Help");
    for (title, action) in [
        (
            "Documentation",
            MenuActionHandler::on_open_documentation as MenuCallback,
        ),
        ("GitHub", MenuActionHandler::on_open_github),
        ("Matrix", MenuActionHandler::on_open_matrix),
    ] {
        add_action_item(&help_menu, mtm, handler, title, action);
    }

    add_separator(&menu);
    let sponsor_item = add_action_item(
        &menu,
        mtm,
        handler,
        "Sponsor Rift on GitHub",
        MenuActionHandler::on_open_sponsor,
    );
    sponsor_item.setSubtitle(Some(&NSString::from_str("help fund continued development")));

    add_separator(&menu);
    let quit_item =
        add_action_item(&menu, mtm, handler, "Quit Rift", MenuActionHandler::on_quit_rift);

    BuiltStatusMenu {
        menu,
        layout_items,
        workspace_item,
        workspace_submenu,
        next_workspace_item,
        prev_workspace_item,
        tiling_item,
        reload_item,
        quit_item,
        restore_workspace_menu,
        restore_space_menu,
    }
}

#[derive(Default)]
struct MenuShortcuts {
    toggle_space_activation: Option<Hotkey>,
    next_workspace: Option<Hotkey>,
    prev_workspace: Option<Hotkey>,
    quit_rift: Option<Hotkey>,
    switch_workspace_by_index: HashMap<usize, Hotkey>,
    switch_workspace_by_name: HashMap<String, Hotkey>,
    reload_config: Option<Hotkey>,
}

impl MenuShortcuts {
    fn from_hotkeys(hotkeys: &[(Hotkey, WmCommand)]) -> Self {
        let mut out = Self::default();

        for (hotkey, command) in hotkeys {
            match command {
                WmCommand::Wm(WmCmd::ToggleSpaceActivated) => {
                    out.toggle_space_activation.get_or_insert_with(|| hotkey.clone());
                }
                WmCommand::Wm(WmCmd::NextWorkspace) => {
                    out.next_workspace.get_or_insert_with(|| hotkey.clone());
                }
                WmCommand::Wm(WmCmd::PrevWorkspace) => {
                    out.prev_workspace.get_or_insert_with(|| hotkey.clone());
                }
                WmCommand::Wm(WmCmd::SwitchToWorkspace(WorkspaceSelector::Index(i))) => {
                    out.switch_workspace_by_index.entry(*i).or_insert_with(|| hotkey.clone());
                }
                WmCommand::Wm(WmCmd::SwitchToWorkspace(WorkspaceSelector::Name(name))) => {
                    out.switch_workspace_by_name
                        .entry(name.clone())
                        .or_insert_with(|| hotkey.clone());
                }
                WmCommand::ReactorCommand(ReactorTopCommand::Reactor(
                    ReactorCommand::ToggleSpaceActivated,
                )) => {
                    out.toggle_space_activation.get_or_insert_with(|| hotkey.clone());
                }
                WmCommand::ReactorCommand(ReactorTopCommand::Layout(
                    LayoutCommand::NextWorkspace(_),
                )) => {
                    out.next_workspace.get_or_insert_with(|| hotkey.clone());
                }
                WmCommand::ReactorCommand(ReactorTopCommand::Layout(
                    LayoutCommand::PrevWorkspace(_),
                )) => {
                    out.prev_workspace.get_or_insert_with(|| hotkey.clone());
                }
                WmCommand::ReactorCommand(ReactorTopCommand::Layout(
                    LayoutCommand::SwitchToWorkspace(i),
                )) => {
                    out.switch_workspace_by_index.entry(*i).or_insert_with(|| hotkey.clone());
                }
                WmCommand::ReactorCommand(ReactorTopCommand::Reactor(
                    ReactorCommand::SaveAndExit,
                )) => {
                    out.quit_rift.get_or_insert_with(|| hotkey.clone());
                }
                WmCommand::Wm(WmCmd::ReloadConfig) => {
                    out.reload_config.get_or_insert_with(|| hotkey.clone());
                }

                _ => {}
            }
        }

        out
    }
}

fn menu_hotkey_to_key_equivalent(hotkey: &Hotkey) -> Option<(&'static str, NSEventModifierFlags)> {
    let key = match hotkey.key_code {
        KeyCode::KeyA => "a",
        KeyCode::KeyB => "b",
        KeyCode::KeyC => "c",
        KeyCode::KeyD => "d",
        KeyCode::KeyE => "e",
        KeyCode::KeyF => "f",
        KeyCode::KeyG => "g",
        KeyCode::KeyH => "h",
        KeyCode::KeyI => "i",
        KeyCode::KeyJ => "j",
        KeyCode::KeyK => "k",
        KeyCode::KeyL => "l",
        KeyCode::KeyM => "m",
        KeyCode::KeyN => "n",
        KeyCode::KeyO => "o",
        KeyCode::KeyP => "p",
        KeyCode::KeyQ => "q",
        KeyCode::KeyR => "r",
        KeyCode::KeyS => "s",
        KeyCode::KeyT => "t",
        KeyCode::KeyU => "u",
        KeyCode::KeyV => "v",
        KeyCode::KeyW => "w",
        KeyCode::KeyX => "x",
        KeyCode::KeyY => "y",
        KeyCode::KeyZ => "z",
        KeyCode::Digit0 => "0",
        KeyCode::Digit1 => "1",
        KeyCode::Digit2 => "2",
        KeyCode::Digit3 => "3",
        KeyCode::Digit4 => "4",
        KeyCode::Digit5 => "5",
        KeyCode::Digit6 => "6",
        KeyCode::Digit7 => "7",
        KeyCode::Digit8 => "8",
        KeyCode::Digit9 => "9",
        KeyCode::Minus => "-",
        KeyCode::Equal => "=",
        KeyCode::BracketLeft => "[",
        KeyCode::BracketRight => "]",
        KeyCode::Semicolon => ";",
        KeyCode::Quote => "'",
        KeyCode::Backquote => "`",
        KeyCode::Backslash => "\\",
        KeyCode::Comma => ",",
        KeyCode::Period => ".",
        KeyCode::Slash => "/",
        _ => return None,
    };

    let mut flags = NSEventModifierFlags::empty();
    if hotkey.modifiers.intersects(Modifiers::META) {
        flags.insert(NSEventModifierFlags::Command);
    }
    if hotkey.modifiers.intersects(Modifiers::CONTROL) {
        flags.insert(NSEventModifierFlags::Control);
    }
    if hotkey.modifiers.intersects(Modifiers::ALT) {
        flags.insert(NSEventModifierFlags::Option);
    }
    if hotkey.modifiers.intersects(Modifiers::SHIFT) {
        flags.insert(NSEventModifierFlags::Shift);
    }

    Some((key, flags))
}

fn own_pid() -> i32 { std::process::id() as i32 }

struct MenuActionHandler {
    action_tx: UnboundedSender<MenuAction>,
    reactor_tx: ReactorSender,
    layout_files: RefCell<Vec<PathBuf>>,
    layout_folder: RefCell<PathBuf>,
}

impl MenuActionHandler {
    fn new(action_tx: UnboundedSender<MenuAction>, reactor_tx: ReactorSender) -> Rc<Self> {
        Rc::new(Self {
            action_tx,
            reactor_tx,
            layout_files: RefCell::new(Vec::new()),
            layout_folder: RefCell::new(PathBuf::new()),
        })
    }

    fn emit(&self, action: MenuAction) { let _ = self.action_tx.send(action); }

    fn set_layout_files(&self, paths: Vec<PathBuf>) { *self.layout_files.borrow_mut() = paths; }

    fn set_layout_folder(&self, path: PathBuf) { *self.layout_folder.borrow_mut() = path; }

    fn layout_file_for_item(&self, item: Option<&NSMenuItem>) -> Option<PathBuf> {
        let index = usize::try_from(item?.tag()).ok()?;
        self.layout_files.borrow().get(index).cloned()
    }

    fn set_default_layout_directory(&self, panel: &NSSavePanel) {
        let directory = self.layout_folder.borrow();
        if std::fs::create_dir_all(&*directory).is_ok() {
            let path = NSString::from_str(&directory.to_string_lossy());
            let url = NSURL::fileURLWithPath_isDirectory(&path, true);
            panel.setDirectoryURL(Some(&url));
        }
    }

    #[allow(deprecated)]
    fn restrict_to_layout_files(panel: &NSSavePanel) {
        let extension = NSString::from_str("ron");
        let extensions = NSArray::from_slice(&[&*extension]);
        panel.setAllowedFileTypes(Some(&extensions));
        panel.setAllowsOtherFileTypes(false);
    }

    fn choose_save_path(&self) -> Option<PathBuf> {
        let panel = SavePanel::new(&Ui::new(MainThreadMarker::new()?)).filename("layout.ron");
        self.set_default_layout_directory(panel.ns_save_panel());
        Self::restrict_to_layout_files(panel.ns_save_panel());
        panel.ns_save_panel().setCanCreateDirectories(true);
        panel.choose()
    }

    fn choose_layout_path(&self) -> Option<PathBuf> {
        let panel = OpenPanel::new(&Ui::new(MainThreadMarker::new()?)).files();
        self.set_default_layout_directory(panel.ns_open_panel());
        Self::restrict_to_layout_files(panel.ns_open_panel());
        panel.ns_open_panel().setAllowsMultipleSelection(false);
        panel.choose()
    }

    fn validate_layout_path(path: PathBuf) -> Option<PathBuf> {
        match LayoutEngine::load(path.clone()) {
            Ok(_) => Some(path),
            Err(error) => {
                let mtm = MainThreadMarker::new()?;
                let alert = NSAlert::new(mtm);
                alert.setMessageText(&NSString::from_str("Couldn’t Load Layout"));
                alert.setInformativeText(&NSString::from_str(&format!(
                    "The layout file at “{}” could not be read.\n\n{error}",
                    path.display()
                )));
                let _ = alert.runModal();
                None
            }
        }
    }
}

impl MenuActionHandler {
    fn on_set_layout_traditional(&self, _sender: Option<&NSMenuItem>) {
        self.emit(MenuAction::SetLayout(LayoutMode::Traditional));
    }

    fn on_set_layout_bsp(&self, _sender: Option<&NSMenuItem>) {
        self.emit(MenuAction::SetLayout(LayoutMode::Bsp));
    }

    fn on_set_layout_floating(&self, _sender: Option<&NSMenuItem>) {
        self.emit(MenuAction::SetLayout(LayoutMode::Floating));
    }

    fn on_set_layout_stack(&self, _sender: Option<&NSMenuItem>) {
        self.emit(MenuAction::SetLayout(LayoutMode::Stack));
    }

    fn on_set_layout_master_stack(&self, _sender: Option<&NSMenuItem>) {
        self.emit(MenuAction::SetLayout(LayoutMode::MasterStack));
    }

    fn on_set_layout_scrolling(&self, _sender: Option<&NSMenuItem>) {
        self.emit(MenuAction::SetLayout(LayoutMode::Scrolling));
    }

    fn on_toggle_space_activation(&self, _sender: Option<&NSMenuItem>) {
        self.emit(MenuAction::ToggleSpaceActivated);
    }

    fn on_next_workspace(&self, _sender: Option<&NSMenuItem>) {
        self.emit(MenuAction::NextWorkspace);
    }

    fn on_prev_workspace(&self, _sender: Option<&NSMenuItem>) {
        self.emit(MenuAction::PrevWorkspace);
    }

    fn on_switch_workspace(&self, sender: Option<&NSMenuItem>) {
        if let Some(sender) = sender {
            let tag = sender.tag();
            if tag >= 0 {
                self.emit(MenuAction::SwitchToWorkspace(tag as usize));
            }
        }
    }

    fn on_restore_workspace(&self, _sender: Option<&NSMenuItem>) {
        if let Some(path) = self.choose_layout_path().and_then(Self::validate_layout_path) {
            self.emit(MenuAction::RestoreLayout {
                path,
                scope: RestoreScope::Workspace,
                source: RestoreSource::SavedActiveSpace,
            });
        }
    }

    fn on_restore_library_workspace(&self, sender: Option<&NSMenuItem>) {
        if let Some(path) = self.layout_file_for_item(sender).and_then(Self::validate_layout_path) {
            self.emit(MenuAction::RestoreLayout {
                path,
                scope: RestoreScope::Workspace,
                source: RestoreSource::SavedActiveSpace,
            });
        }
    }

    fn on_save_layout(&self, _sender: Option<&NSMenuItem>) {
        if let Some(path) = self.choose_save_path() {
            self.emit(MenuAction::SaveLayout(path));
        }
    }

    fn on_save_master_file(&self, _sender: Option<&NSMenuItem>) {
        self.emit(MenuAction::SaveMasterFile);
    }

    fn on_restore_master_file_workspace(&self, _sender: Option<&NSMenuItem>) {
        if Self::validate_layout_path(restore_file()).is_some() {
            self.emit(MenuAction::RestoreMasterFile(RestoreScope::Workspace));
        }
    }

    fn on_restore_master_file_space(&self, _sender: Option<&NSMenuItem>) {
        if Self::validate_layout_path(restore_file()).is_some() {
            self.emit(MenuAction::RestoreMasterFile(RestoreScope::Space));
        }
    }

    fn on_restore_space(&self, _sender: Option<&NSMenuItem>) {
        if let Some(path) = self.choose_layout_path().and_then(Self::validate_layout_path) {
            self.emit(MenuAction::RestoreLayout {
                path,
                scope: RestoreScope::Space,
                source: RestoreSource::SavedActiveSpace,
            });
        }
    }

    fn on_restore_library_space(&self, sender: Option<&NSMenuItem>) {
        if let Some(path) = self.layout_file_for_item(sender).and_then(Self::validate_layout_path) {
            self.emit(MenuAction::RestoreLayout {
                path,
                scope: RestoreScope::Space,
                source: RestoreSource::SavedActiveSpace,
            });
        }
    }

    fn on_open_settings(&self, _sender: Option<&NSMenuItem>) {
        self.emit(MenuAction::OpenSettings);
    }

    fn on_open_documentation(&self, _sender: Option<&NSMenuItem>) {
        self.emit(MenuAction::OpenDocumentation);
    }

    fn on_open_github(&self, _sender: Option<&NSMenuItem>) { self.emit(MenuAction::OpenGitHub); }

    fn on_open_matrix(&self, _sender: Option<&NSMenuItem>) { self.emit(MenuAction::OpenMatrix); }

    fn on_open_sponsor(&self, _sender: Option<&NSMenuItem>) { self.emit(MenuAction::OpenSponsor); }

    fn on_reload_config(&self, _sender: Option<&NSMenuItem>) {
        self.emit(MenuAction::ReloadConfig);
    }

    fn on_quit_rift(&self, _sender: Option<&NSMenuItem>) { self.emit(MenuAction::QuitRift); }
}

#[cfg(test)]
mod layout_library_tests {
    use super::*;

    fn workspace(id: &str, index: usize, name: &str, active: bool, layout: &str) -> Workspace {
        Workspace {
            id: id.to_owned(),
            index,
            name: name.to_owned(),
            layout_mode: layout.to_owned(),
            is_active: active,
            window_count: 0,
            windows: Vec::new(),
        }
    }

    #[test]
    fn layout_library_only_lists_visible_ron_files_in_name_order() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("Work.RON"), "").unwrap();
        std::fs::write(directory.path().join("gaming.ron"), "").unwrap();
        std::fs::write(directory.path().join("notes.txt"), "").unwrap();
        std::fs::write(directory.path().join(".hidden.ron"), "").unwrap();
        std::fs::create_dir(directory.path().join("nested.ron")).unwrap();

        let layouts = layout_library_files_in(directory.path());
        let names = layouts.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>();

        assert_eq!(names, ["gaming", "Work"]);
    }

    #[test]
    fn workspace_state_changes_do_not_invalidate_topology_or_scan_library() {
        let before = vec![
            workspace("one", 0, "main", true, "bsp"),
            workspace("two", 1, "web", false, "stack"),
        ];
        let after_switch = vec![
            workspace("one", 0, "main", false, "bsp"),
            workspace("two", 1, "web", true, "master_stack"),
        ];
        let scans_before = LAYOUT_LIBRARY_SCANS.with(Cell::get);

        assert_eq!(workspace_topology(&before), workspace_topology(&after_switch));
        assert_eq!(LAYOUT_LIBRARY_SCANS.with(Cell::get), scans_before);
    }

    #[test]
    fn workspace_topology_changes_for_create_rename_delete_and_reorder() {
        let base = vec![
            workspace("one", 0, "main", true, "bsp"),
            workspace("two", 1, "web", false, "stack"),
        ];
        let created = vec![
            workspace("one", 0, "main", true, "bsp"),
            workspace("two", 1, "web", false, "stack"),
            workspace("three", 2, "chat", false, "bsp"),
        ];
        let renamed = vec![
            workspace("one", 0, "primary", true, "bsp"),
            workspace("two", 1, "web", false, "stack"),
        ];
        let deleted = vec![workspace("one", 0, "main", true, "bsp")];
        let reordered = vec![
            workspace("two", 0, "web", false, "stack"),
            workspace("one", 1, "main", true, "bsp"),
        ];
        let topology = workspace_topology(&base);

        for changed in [&created, &renamed, &deleted, &reordered] {
            assert_ne!(topology, workspace_topology(changed));
        }
    }

    #[test]
    fn refreshing_layout_library_sees_files_created_after_first_scan() {
        let directory = tempfile::tempdir().unwrap();
        assert!(layout_library_files_in(directory.path()).is_empty());

        std::fs::write(directory.path().join("new-layout.ron"), "").unwrap();

        let layouts = layout_library_files_in(directory.path());
        assert_eq!(layouts[0].0, "new-layout");
    }

    #[test]
    fn workspace_borders_are_centered_and_remain_inside_the_strip() {
        let first = workspace_cell_rect(0.0, CELL_WIDTH, CELL_HEIGHT);
        let last = workspace_cell_rect(2.0 * (CELL_WIDTH + CELL_SPACING), CELL_WIDTH, CELL_HEIGHT);
        let half_stroke = BORDER_WIDTH / 2.0;

        assert_eq!(first.origin.x - half_stroke, 0.0);
        assert_eq!(first.origin.y - half_stroke, 0.0);
        assert_eq!(first.origin.y + first.size.height + half_stroke, CELL_HEIGHT);
        assert_eq!(
            last.origin.x + last.size.width + half_stroke,
            3.0 * CELL_WIDTH + 2.0 * CELL_SPACING
        );
    }

    #[test]
    fn centered_origin_accounts_for_nonzero_container_origins() {
        assert_eq!(centered_origin(4.0, 20.0, 6.0), 11.0);
        assert_eq!(centered_origin(-2.0, 15.0, 5.0), 3.0);
    }

    fn displays() -> Vec<DisplayWorkspaces> {
        (1..=2)
            .map(|number| DisplayWorkspaces {
                display_uuid: format!("display-{number}"),
                space: SpaceId::new(number),
                is_active_context: number == 1,
                workspaces: vec![
                    workspace("one", 0, "main", true, "bsp"),
                    workspace("two", 1, "web", false, "stack"),
                ],
            })
            .collect()
    }

    #[test]
    fn each_display_filters_independently_and_retains_active_empty_workspaces() {
        let mut displays = displays();
        for display in &mut displays {
            for workspace in &mut display.workspaces {
                workspace.window_count = 0;
            }
        }
        displays[1].workspaces[1].window_count = 1;
        for display_style in [WorkspaceDisplayStyle::Label, WorkspaceDisplayStyle::Layout] {
            let mut settings = MenuBarSettings {
                display_style,
                ..Default::default()
            };
            let inputs = render_inputs(&displays, &settings);
            assert_eq!(inputs.len(), 3);
            assert_eq!(
                inputs.iter().filter(|input| input.workspace.is_active).count(),
                2
            );
            assert_eq!(inputs[0].display_uuid, "display-1");
            assert_eq!(inputs[1].display_uuid, "display-2");
            settings.show_empty = true;
            assert_eq!(render_inputs(&displays, &settings).len(), 4);
            settings.mode = MenuBarDisplayMode::Active;
            let inputs = render_inputs(&displays, &settings);
            assert_eq!(inputs.len(), 2);
            assert!(inputs.iter().all(|input| input.workspace.is_active));
        }
    }

    #[test]
    fn grouped_geometry_highlights_both_displays_without_outer_gaps() {
        let displays = displays();
        let attrs = build_text_attrs(&NSFont::systemFontOfSize_weight(FONT_SIZE, unsafe {
            NSFontWeightMedium
        }));
        for display_style in [WorkspaceDisplayStyle::Label, WorkspaceDisplayStyle::Layout] {
            let settings = MenuBarSettings {
                display_style,
                show_empty: true,
                ..Default::default()
            };
            let inputs = render_inputs(&displays, &settings);
            let layout = build_layout(&inputs, &attrs);
            let cells = &layout.workspaces;
            assert_eq!(cells.len(), 4);
            assert_eq!(cells[0].fill_alpha, 1.0);
            assert_eq!(cells[2].fill_alpha, 1.0);
            if display_style == WorkspaceDisplayStyle::Label {
                assert_eq!(cells[1].fill_alpha, 0.0);
                assert_eq!(cells[3].fill_alpha, 0.0);
            }
            let spacing = if display_style == WorkspaceDisplayStyle::Label {
                LABEL_SPACING
            } else {
                CELL_SPACING
            };
            let cell_widths: Vec<_> = cells
                .iter()
                .map(|cell| {
                    if display_style == WorkspaceDisplayStyle::Label {
                        cell.bg_rect.size.width
                    } else {
                        CELL_WIDTH
                    }
                })
                .collect();
            assert_eq!(
                layout.size.width,
                cell_widths.iter().sum::<f64>()
                    + 2.0 * spacing
                    + 2.0 * SEPARATOR_SPACING
                    + layout.separator_line.as_ref().unwrap().width
            );
            let inset = if display_style == WorkspaceDisplayStyle::Label {
                0.0
            } else {
                BORDER_WIDTH / 2.0
            };
            assert_eq!(layout.separators, vec![
                cell_widths[..2].iter().sum::<f64>() + spacing + SEPARATOR_SPACING
            ]);
            assert_eq!(cells[0].bg_rect.origin.x, inset);
            assert_eq!(cells[3].bg_rect.max().x + inset, layout.size.width);
            let single = build_layout(&render_inputs(&displays[..1], &settings), &attrs);
            assert_eq!(single.size.width, cell_widths[..2].iter().sum::<f64>() + spacing);
            assert!(single.separators.is_empty());
            assert!(single.separator_line.is_none());
            assert_eq!(build_layout(&[], &attrs).size.width, 0.0);
        }
    }

    #[test]
    fn labels_use_cached_text_width_for_numbers_and_names() {
        let mut displays = displays();
        displays[0].workspaces[1].index = 9;
        displays[0].workspaces[1].name = "Development".into();
        let attrs = build_text_attrs(&NSFont::systemFontOfSize_weight(FONT_SIZE, unsafe {
            NSFontWeightMedium
        }));
        for active_label in [ActiveWorkspaceLabel::Index, ActiveWorkspaceLabel::Name] {
            let settings = MenuBarSettings {
                display_style: WorkspaceDisplayStyle::Label,
                active_label,
                show_empty: true,
                ..Default::default()
            };
            let layout = build_layout(&render_inputs(&displays, &settings), &attrs);
            for cell in &layout.workspaces {
                let line = cell.label_line.as_ref().unwrap();
                assert!(cell.bg_rect.size.width >= line.width + 2.0 * LABEL_PADDING);
            }
            assert!(layout.workspaces[1].bg_rect.size.width > LABEL_HEIGHT);
            if active_label == ActiveWorkspaceLabel::Index {
                // A narrow single digit must not become a tall, cramped cell.
                assert_eq!(layout.workspaces[0].bg_rect.size.width, LABEL_HEIGHT);
                assert!(
                    layout.workspaces[0].bg_rect.size.width
                        < layout.workspaces[1].bg_rect.size.width
                );
            }
        }
    }

    #[test]
    fn render_key_tracks_second_display_identity_order_and_state() {
        let base = displays();
        let settings = MenuBarSettings {
            display_style: WorkspaceDisplayStyle::Label,
            show_empty: true,
            ..Default::default()
        };
        let key = MenuIconRenderKey::from_inputs(&render_inputs(&base, &settings));
        assert!(key.matches_inputs(&render_inputs(&base, &settings)));
        let mut changed = base.clone();
        changed[1].workspaces[0].is_active = false;
        assert!(!key.matches_inputs(&render_inputs(&changed, &settings)));
        changed = base.clone();
        changed.reverse();
        assert!(!key.matches_inputs(&render_inputs(&changed, &settings)));
        changed = base.clone();
        changed.pop();
        assert!(!key.matches_inputs(&render_inputs(&changed, &settings)));
        changed = base.clone();
        changed[1].display_uuid = "replacement".into();
        assert!(!key.matches_inputs(&render_inputs(&changed, &settings)));
        changed = base.clone();
        changed[1].is_active_context = true;
        changed[0].is_active_context = false;
        assert!(key.matches_inputs(&render_inputs(&changed, &settings)));
    }

    #[test]
    fn render_key_only_matches_visually_identical_inputs() {
        let mut workspace = workspace("one", 0, "main", true, "bsp");
        let key = MenuIconRenderKey::from_inputs(&[WorkspaceRenderInput {
            display_uuid: "a",
            space: SpaceId::new(1),
            workspace: &workspace,
            label: Cow::Borrowed("1"),
            show_windows: false,
        }]);

        assert!(key.matches_inputs(&[WorkspaceRenderInput {
            display_uuid: "a",
            space: SpaceId::new(1),
            workspace: &workspace,
            label: Cow::Borrowed("1"),
            show_windows: false,
        }]));

        workspace.is_active = false;
        assert!(!key.matches_inputs(&[WorkspaceRenderInput {
            display_uuid: "a",
            space: SpaceId::new(1),
            workspace: &workspace,
            label: Cow::Borrowed("1"),
            show_windows: false,
        }]));
    }
}

fn build_text_attrs(font: &NSFont) -> Retained<NSDictionary<NSAttributedStringKey, AnyObject>> {
    let dict = NSMutableDictionary::<NSAttributedStringKey, AnyObject>::new();
    unsafe {
        dict.setObject_forKeyedSubscript(
            Some(as_any_object(font)),
            ProtocolObject::from_ref(NSFontAttributeName),
        );
        dict.setObject_forKeyedSubscript(
            Some(as_any_object(&*NSNumber::numberWithBool(true))),
            ProtocolObject::from_ref::<NSString>(
                kCTForegroundColorFromContextAttributeName.as_ref(),
            ),
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
    fn new(mtm: MainThreadMarker) -> Self {
        let font = NSFont::systemFontOfSize_weight(FONT_SIZE, unsafe { NSFontWeightMedium });
        let text_attrs = build_text_attrs(font.as_ref());
        let layout = Rc::new(RefCell::new(MenuIconLayout::default()));
        let draw_layout = layout.clone();
        let canvas = Canvas::new(&Ui::new(mtm), move |cg, bounds| {
            draw_menu_icon(cg, bounds, &draw_layout.borrow())
        });
        Self { canvas, layout, text_attrs }
    }

    fn set_layout(&self, layout: MenuIconLayout) {
        *self.layout.borrow_mut() = layout;
        self.canvas.redraw();
    }
}
impl std::ops::Deref for MenuIconView {
    type Target = NSView;

    fn deref(&self) -> &NSView { self.canvas.ns_view() }
}

fn label_cell_width(text_width: f64) -> f64 {
    // Keep breathing room around narrow glyphs such as 1 without clipping names.
    LABEL_HEIGHT.max((text_width * 2.0).ceil() / 2.0 + 2.0 * LABEL_PADDING)
}

fn build_layout(
    inputs: &[WorkspaceRenderInput<'_>],
    text_attrs: &NSDictionary<NSAttributedStringKey, AnyObject>,
) -> MenuIconLayout {
    let count = inputs.len();
    let mut workspaces = Vec::with_capacity(count);
    let height = if inputs.first().is_some_and(|input| !input.show_windows) {
        LABEL_HEIGHT
    } else {
        CELL_HEIGHT
    };
    let mut width = 0.0;
    let mut previous_display = None;
    let mut separators = Vec::new();
    let mut separator_line = None;
    for input in inputs {
        let workspace = input.workspace;
        let display = (input.display_uuid, input.space);
        if let Some(previous) = previous_display {
            width += if previous == display {
                if input.show_windows {
                    CELL_SPACING
                } else {
                    LABEL_SPACING
                }
            } else {
                let line = separator_line.get_or_insert_with(|| {
                    // AeroSpace's largeTitle.bold pipe, rendered at half scale.
                    let font = NSFont::systemFontOfSize_weight(13.0, unsafe { NSFontWeightBold });
                    build_cached_text_line("|", &build_text_attrs(&font))
                        .expect("nonempty separator")
                });
                separators.push(width + SEPARATOR_SPACING);
                2.0 * SEPARATOR_SPACING + line.width
            };
        }
        previous_display = Some(display);
        let label_line = build_cached_text_line(&input.label, text_attrs);
        let cell_width = if input.show_windows {
            CELL_WIDTH
        } else {
            label_cell_width(label_line.as_ref().map_or(0.0, |line| line.width))
        };
        let bg_rect = if input.show_windows {
            workspace_cell_rect(width, cell_width, height)
        } else {
            CGRect::new(CGPoint::new(width, 0.0), CGSize::new(cell_width, height))
        };
        width += cell_width;

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
            0.0
        };

        let windows = if input.show_windows && !workspace.windows.is_empty() {
            let layout = compute_window_layout_metrics(
                &workspace.windows,
                bg_rect,
                CONTENT_INSET,
                1.0,
                None,
            );
            if let Some(layout) = layout {
                const MIN_TILE_SIZE: f64 = 2.0;
                const WIN_GAP: f64 = 0.75;
                let mut rects = Vec::with_capacity(workspace.windows.len());
                for window in workspace.windows.iter().rev() {
                    let rect = layout.rect_for(window, MIN_TILE_SIZE, WIN_GAP);
                    rects.push(rect);
                }
                rects
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        workspaces.push(WorkspaceRenderData {
            bg_rect,
            fill_alpha,
            windows,
            label_line,
        });
    }

    MenuIconLayout {
        workspaces,
        separators,
        separator_line,
        size: CGSize::new(width, height),
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

fn draw_menu_icon(cg: &CGContext, bounds: CGRect, layout: &MenuIconLayout) {
    CGContext::clear_rect(Some(cg), bounds);

    let y_offset = (bounds.size.height - layout.size.height) / 2.0;
    // Resolve dynamic AppKit color at draw time, under the view's effective appearance.
    let foreground = NSColor::labelColor();
    CGContext::set_rgb_stroke_color(Some(cg), 1.0, 1.0, 1.0, 1.0);
    CGContext::set_line_width(Some(cg), BORDER_WIDTH);

    for workspace in layout.workspaces.iter() {
        let is_label = workspace.label_line.is_some();
        let corner_radius = if is_label {
            LABEL_CORNER_RADIUS
        } else {
            CORNER_RADIUS
        };
        CGContext::set_line_width(
            Some(cg),
            if is_label {
                LABEL_BORDER_WIDTH
            } else {
                BORDER_WIDTH
            },
        );
        if is_label {
            foreground.setStroke();
            unsafe {
                CGContext::begin_transparency_layer(Some(cg), None);
            }
        } else {
            CGContext::set_rgb_stroke_color(Some(cg), 1.0, 1.0, 1.0, 1.0);
        }
        let rect = workspace.bg_rect;
        let bg_y = rect.origin.y + y_offset;

        if workspace.fill_alpha > 0.0 {
            add_rounded_rect(
                cg,
                rect.origin.x,
                bg_y,
                rect.size.width,
                rect.size.height,
                corner_radius,
            );
            CGContext::set_rgb_fill_color(Some(cg), 1.0, 1.0, 1.0, workspace.fill_alpha);
            if is_label {
                foreground.setFill();
            }
            CGContext::fill_path(Some(cg));
        }

        // SwiftUI's strokeBorder is entirely inside the cell; an active
        // label is a solid shape without an additional outline.
        if !is_label || workspace.fill_alpha == 0.0 {
            let inset = if is_label {
                LABEL_BORDER_WIDTH / 2.0
            } else {
                0.0
            };
            add_rounded_rect(
                cg,
                rect.origin.x + inset,
                bg_y + inset,
                rect.size.width - 2.0 * inset,
                rect.size.height - 2.0 * inset,
                corner_radius - inset,
            );
            CGContext::stroke_path(Some(cg));
        }

        for window in &workspace.windows {
            add_rounded_rect(
                cg,
                window.origin.x,
                window.origin.y + y_offset,
                window.size.width,
                window.size.height,
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
                window.origin.x,
                window.origin.y + y_offset,
                window.size.width,
                window.size.height,
                1.5,
            );
            CGContext::stroke_path(Some(cg));
            CGContext::restore_g_state(Some(cg));
        }

        if let Some(label_line) = &workspace.label_line {
            let text_x = centered_origin(rect.origin.x, rect.size.width, label_line.width);
            let baseline_y =
                centered_origin(bg_y, rect.size.height, label_line.ascent + label_line.descent)
                    + label_line.descent;

            CGContext::save_g_state(Some(cg));
            if workspace.fill_alpha > 0.0 {
                CGContext::set_blend_mode(Some(cg), CGBlendMode::DestinationOut);
                CGContext::set_rgb_fill_color(Some(cg), 1.0, 1.0, 1.0, 1.0);
            } else {
                foreground.setFill();
            }
            CGContext::set_text_position(Some(cg), text_x as CGFloat, baseline_y as CGFloat);
            let line_ref: &CTLine = label_line.line.as_ref();
            unsafe { line_ref.draw(cg) };
            CGContext::restore_g_state(Some(cg));
        }
        if is_label {
            CGContext::end_transparency_layer(Some(cg));
        }
    }

    if let Some(line) = &layout.separator_line {
        foreground.setFill();
        CGContext::set_alpha(Some(cg), 0.6);
        // Center the glyph plus bottom padding, as in AeroSpace's HStack.
        let baseline = centered_origin(
            y_offset,
            layout.size.height,
            line.ascent + line.descent + SEPARATOR_BOTTOM_PADDING,
        ) + line.descent
            + SEPARATOR_BOTTOM_PADDING;
        for &x in &layout.separators {
            CGContext::set_text_position(Some(cg), x, baseline);
            unsafe {
                line.line.draw(cg);
            }
        }
    }
}
