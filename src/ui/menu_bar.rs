// many ideas for how this works were taken from https://github.com/xiamaz/YabaiIndicator
#[cfg(test)]
use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{ClassType, DefinedClass, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAlert, NSColor, NSControlStateValueOff, NSControlStateValueOn, NSEventModifierFlags, NSFont,
    NSFontAttributeName, NSForegroundColorAttributeName, NSGraphicsContext, NSImage, NSMenu,
    NSMenuItem, NSModalResponseOK, NSOpenPanel, NSSavePanel, NSStatusBar, NSStatusItem,
    NSVariableStatusItemLength, NSView,
};
use objc2_core_foundation::{
    CFAttributedString, CFDictionary, CFRetained, CFString, CGFloat, CGPoint, CGRect, CGSize,
};
use objc2_core_graphics::{CGBlendMode, CGContext};
use objc2_core_text::CTLine;
use objc2_foundation::{
    MainThreadMarker, NSArray, NSAttributedStringKey, NSDictionary, NSMutableDictionary, NSObject,
    NSRect, NSSize, NSString, NSURL,
};
use tokio::sync::mpsc::UnboundedSender;
use tracing::debug;

use crate::actor::reactor::{Command as ReactorTopCommand, ReactorCommand};
use crate::actor::wm_controller::{WmCmd, WmCommand};
use crate::common::config::{
    ActiveWorkspaceLabel, LayoutMode, MenuBarDisplayMode, MenuBarSettings, WorkspaceDisplayStyle,
    WorkspaceSelector, restore_file,
};
use crate::layout_engine::{LayoutCommand, LayoutEngine, RestoreScope, RestoreSource};
use crate::model::server::RuntimeWorkspaceData;
use crate::sys::hotkey::{Hotkey, KeyCode, Modifiers};
use crate::ui::common::compute_window_layout_metrics;

const CELL_WIDTH: f64 = 20.0;
const CELL_HEIGHT: f64 = 15.0;
const CELL_SPACING: f64 = 4.0;
const CORNER_RADIUS: f64 = 3.0;
const BORDER_WIDTH: f64 = 1.0;
const CONTENT_INSET: f64 = 2.0;
const FONT_SIZE: f64 = 12.0;

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
    ReloadConfig,
    QuitRift,
}

pub struct MenuIcon {
    status_item: Retained<NSStatusItem>,
    view: Retained<MenuIconView>,
    _menu: Retained<NSMenu>,
    menu_handler: Retained<MenuActionHandler>,
    layout_items: Vec<(LayoutMode, Retained<NSMenuItem>)>,
    workspace_item: Retained<NSMenuItem>,
    workspace_submenu: Retained<NSMenu>,
    workspace_items: Vec<WorkspaceMenuItem>,
    workspace_topology: Vec<WorkspaceTopology>,
    next_workspace_item: Retained<NSMenuItem>,
    prev_workspace_item: Retained<NSMenuItem>,
    tiling_item: Retained<NSMenuItem>,
    reload_item: Retained<NSMenuItem>,
    quit_item: Retained<NSMenuItem>,
    restore_workspace_menu: Retained<NSMenu>,
    restore_space_menu: Retained<NSMenu>,
    library_workspace_items: Vec<Retained<NSMenuItem>>,
    library_space_items: Vec<Retained<NSMenuItem>>,
    layout_folder: PathBuf,
    mtm: MainThreadMarker,
    prev_width: f64,
}

struct WorkspaceMenuItem {
    identity: String,
    index: usize,
    name: String,
    item: Retained<NSMenuItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkspaceTopology {
    identity: String,
    index: usize,
    name: String,
}

fn workspace_topology(workspaces: &[RuntimeWorkspaceData]) -> Vec<WorkspaceTopology> {
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
        layout_folder: &Path,
    ) -> Self {
        let status_bar = NSStatusBar::systemStatusBar();
        let status_item = status_bar.statusItemWithLength(NSVariableStatusItemLength);
        let view = MenuIconView::new(mtm);
        let menu_handler = MenuActionHandler::new(mtm, action_tx);
        let built = build_static_menu(mtm, &menu_handler);
        status_item.setMenu(Some(&built.menu));
        if let Some(btn) = status_item.button(mtm) {
            btn.addSubview(&*view);
            view.setFrameSize(NSSize::new(0.0, 0.0));
            status_item.setVisible(true);
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
            workspace_topology: Vec::new(),
            next_workspace_item: built.next_workspace_item,
            prev_workspace_item: built.prev_workspace_item,
            tiling_item: built.tiling_item,
            reload_item: built.reload_item,
            quit_item: built.quit_item,
            restore_workspace_menu: built.restore_workspace_menu,
            restore_space_menu: built.restore_space_menu,
            library_workspace_items: Vec::new(),
            library_space_items: Vec::new(),
            layout_folder: layout_folder.to_path_buf(),
            mtm,
            prev_width: 0.0,
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
        workspaces: &[RuntimeWorkspaceData],
        hotkeys: &[(Hotkey, WmCommand)],
    ) {
        let new_topology = workspace_topology(workspaces);
        if self.workspace_topology == new_topology {
            return;
        }

        for workspace in self.workspace_items.drain(..) {
            self.workspace_submenu.removeItem(&workspace.item);
        }
        let shortcuts = MenuShortcuts::from_hotkeys(hotkeys);
        for workspace in workspaces {
            let title = workspace_menu_title(workspace);
            let shortcut = shortcuts
                .switch_workspace_by_index
                .get(&workspace.index)
                .or_else(|| shortcuts.switch_workspace_by_name.get(&workspace.name));
            let item = make_menu_item(
                self.mtm,
                &title,
                Some(sel!(onSwitchWorkspace:)),
                Some(&self.menu_handler),
                Some(workspace.is_active),
                shortcut,
                Some(workspace.index as isize),
                None,
            );
            self.workspace_submenu.addItem(&item);
            self.workspace_items.push(WorkspaceMenuItem {
                identity: workspace.id.clone(),
                index: workspace.index,
                name: workspace.name.clone(),
                item,
            });
        }
        self.workspace_topology = new_topology;
        self.workspace_item.setEnabled(!workspaces.is_empty());
    }

    pub fn update_menu_state(
        &self,
        active_space_is_activated: bool,
        workspaces: &[RuntimeWorkspaceData],
    ) {
        let active_layout = workspaces
            .iter()
            .find(|workspace| workspace.is_active)
            .and_then(|workspace| parse_layout_mode(&workspace.layout_mode));
        for (mode, item) in &self.layout_items {
            set_menu_item_checked(item, active_layout == Some(*mode));
        }
        for workspace_item in &self.workspace_items {
            let is_active = workspaces
                .iter()
                .find(|workspace| workspace.id == workspace_item.identity)
                .is_some_and(|workspace| workspace.is_active);
            set_menu_item_checked(&workspace_item.item, is_active);
        }
        set_menu_item_checked(&self.tiling_item, active_space_is_activated);
    }

    pub fn refresh_layout_library(&mut self) {
        let library_files = layout_library_files_in(&self.layout_folder);
        self.menu_handler
            .set_layout_files(library_files.iter().map(|(_, path)| path.clone()).collect());
        rebuild_library_items(
            self.mtm,
            &self.menu_handler,
            &self.restore_workspace_menu,
            &mut self.library_workspace_items,
            &library_files,
            sel!(onRestoreLibraryWorkspace:),
        );
        rebuild_library_items(
            self.mtm,
            &self.menu_handler,
            &self.restore_space_menu,
            &mut self.library_space_items,
            &library_files,
            sel!(onRestoreLibrarySpace:),
        );
    }

    pub fn update_status_icon(
        &mut self,
        workspaces: &[RuntimeWorkspaceData],
        settings: &MenuBarSettings,
    ) {
        let mode = settings.mode;
        let style = settings.display_style;
        let label_for = |workspace: &RuntimeWorkspaceData| match settings.active_label {
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
                    workspaces.to_vec()
                } else {
                    workspaces
                        .iter()
                        .cloned()
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
                .iter()
                .cloned()
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
                .iter()
                .cloned()
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
                .iter()
                .cloned()
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
    windows: Vec<CGRect>,
    label_line: Option<CachedTextLine>,
    show_windows: bool,
}

struct WorkspaceRenderInput {
    workspace: RuntimeWorkspaceData,
    label: String,
    show_windows: bool,
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

fn parse_layout_mode(layout_mode: &str) -> Option<LayoutMode> {
    match layout_mode {
        "traditional" => Some(LayoutMode::Traditional),
        "bsp" => Some(LayoutMode::Bsp),
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
        LayoutMode::Stack => "Stack",
        LayoutMode::MasterStack => "Master Stack",
        LayoutMode::Scrolling => "Scrolling",
    }
}

fn make_menu_item(
    mtm: MainThreadMarker,
    title: &str,
    action: Option<objc2::runtime::Sel>,
    target: Option<&MenuActionHandler>,
    checked: Option<bool>,
    key_equivalent: Option<&Hotkey>,
    tag: Option<isize>,
    image: Option<Retained<NSImage>>,
) -> Retained<NSMenuItem> {
    let ns_title = NSString::from_str(title);
    let key_equivalent_empty = NSString::from_str("");
    let item: Retained<NSMenuItem> = unsafe {
        msg_send![NSMenuItem::alloc(mtm), initWithTitle: &*ns_title, action: action, keyEquivalent: &*key_equivalent_empty]
    };
    if let Some(target) = target {
        unsafe {
            item.setTarget(Some(target));
        }
    }
    if let Some(checked) = checked {
        item.setState(if checked {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
    }

    if let Some((key, modifiers)) = key_equivalent.and_then(menu_hotkey_to_key_equivalent) {
        let key = NSString::from_str(key);
        item.setKeyEquivalent(&key);
        item.setKeyEquivalentModifierMask(modifiers);
    }
    if let Some(tag) = tag {
        item.setTag(tag);
    }
    item.setImage(image.as_deref());

    if image.is_some() {
        force_menu_item_image_visible(&item);
    }

    item
}

fn menu_image(symbol_name: &str, accessibility_description: &str) -> Option<Retained<NSImage>> {
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(symbol_name),
        Some(&NSString::from_str(accessibility_description)),
    )?;
    image.setTemplate(true);
    Some(image)
}

fn add_separator(menu: &NSMenu) {
    let separator = menu_separator();
    menu.addItem(&separator);
}

fn menu_separator() -> Retained<NSMenuItem> {
    unsafe { msg_send![NSMenuItem::class(), separatorItem] }
}

fn set_menu_item_checked(item: &NSMenuItem, checked: bool) {
    item.setState(if checked {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    });
}

fn set_menu_item_hotkey(item: &NSMenuItem, hotkey: Option<&Hotkey>) {
    let (key, modifiers) = hotkey
        .and_then(menu_hotkey_to_key_equivalent)
        .unwrap_or(("", NSEventModifierFlags::empty()));
    item.setKeyEquivalent(&NSString::from_str(key));
    item.setKeyEquivalentModifierMask(modifiers);
}

fn workspace_menu_title(workspace: &RuntimeWorkspaceData) -> String {
    if workspace.name.is_empty() {
        format!("Workspace {}", workspace.index + 1)
    } else {
        format!("{} ({})", workspace.name, workspace.index + 1)
    }
}

fn rebuild_library_items(
    mtm: MainThreadMarker,
    handler: &MenuActionHandler,
    menu: &NSMenu,
    current_items: &mut Vec<Retained<NSMenuItem>>,
    library_files: &[(String, PathBuf)],
    action: objc2::runtime::Sel,
) {
    for item in current_items.drain(..) {
        menu.removeItem(&item);
    }
    if library_files.is_empty() {
        return;
    }

    let separator = menu_separator();
    menu.addItem(&separator);
    current_items.push(separator);
    let heading = make_menu_item(mtm, "Saved Layouts", None, None, None, None, None, None);
    heading.setEnabled(false);
    menu.addItem(&heading);
    current_items.push(heading);
    for (index, (name, _)) in library_files.iter().enumerate() {
        let item = make_menu_item(
            mtm,
            name,
            Some(action),
            Some(handler),
            None,
            None,
            Some(index as isize),
            None,
        );
        menu.addItem(&item);
        current_items.push(item);
    }
}

const NS_MENU_ITEM_IMAGE_VISIBILITY_VISIBLE: isize = 1;

fn force_menu_item_image_visible(item: &NSMenuItem) {
    let selector = sel!(setPreferredImageVisibility:);

    let responds: bool = unsafe { msg_send![item, respondsToSelector: selector] };

    if responds {
        let _: () = unsafe {
            msg_send![
                item,
                setPreferredImageVisibility: NS_MENU_ITEM_IMAGE_VISIBILITY_VISIBLE
            ]
        };
    }
}

fn layout_file_title(path: &Path) -> Option<String> {
    let file_name = path.file_name()?.to_str()?;
    if file_name.starts_with('.') {
        return None;
    }
    path.file_stem()?.to_str().map(str::to_owned)
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
    menu: Retained<NSMenu>,
    layout_items: Vec<(LayoutMode, Retained<NSMenuItem>)>,
    workspace_item: Retained<NSMenuItem>,
    workspace_submenu: Retained<NSMenu>,
    next_workspace_item: Retained<NSMenuItem>,
    prev_workspace_item: Retained<NSMenuItem>,
    tiling_item: Retained<NSMenuItem>,
    reload_item: Retained<NSMenuItem>,
    quit_item: Retained<NSMenuItem>,
    restore_workspace_menu: Retained<NSMenu>,
    restore_space_menu: Retained<NSMenu>,
}

fn build_static_menu(mtm: MainThreadMarker, handler: &MenuActionHandler) -> BuiltStatusMenu {
    let title = NSString::from_str("Rift");
    let menu: Retained<NSMenu> = unsafe { msg_send![NSMenu::alloc(mtm), initWithTitle: &*title] };

    let layout_item = make_menu_item(mtm, "Layout", None, None, None, None, None, None);

    let layout_submenu_title = NSString::from_str("Layout");
    let layout_submenu: Retained<NSMenu> =
        unsafe { msg_send![NSMenu::alloc(mtm), initWithTitle: &*layout_submenu_title] };

    let mut layout_items = Vec::new();
    for mode in [
        LayoutMode::Traditional,
        LayoutMode::Bsp,
        LayoutMode::Stack,
        LayoutMode::MasterStack,
        LayoutMode::Scrolling,
    ] {
        let action = match mode {
            LayoutMode::Traditional => sel!(onSetLayoutTraditional:),
            LayoutMode::Bsp => sel!(onSetLayoutBsp:),
            LayoutMode::Stack => sel!(onSetLayoutStack:),
            LayoutMode::MasterStack => sel!(onSetLayoutMasterStack:),
            LayoutMode::Scrolling => sel!(onSetLayoutScrolling:),
        };
        let item = make_menu_item(
            mtm,
            layout_title(&mode),
            Some(action),
            Some(handler),
            Some(false),
            None,
            None,
            None,
        );
        layout_submenu.addItem(&item);
        layout_items.push((mode, item));
    }
    layout_item.setSubmenu(Some(&layout_submenu));
    menu.addItem(&layout_item);

    let workspace_item = make_menu_item(mtm, "Workspaces", None, None, None, None, None, None);

    let ws_submenu_title = NSString::from_str("Workspace");
    let ws_submenu: Retained<NSMenu> =
        unsafe { msg_send![NSMenu::alloc(mtm), initWithTitle: &*ws_submenu_title] };

    let next_workspace_item = make_menu_item(
        mtm,
        "Next Workspace",
        Some(sel!(onNextWorkspace:)),
        Some(handler),
        None,
        None,
        None,
        None,
    );
    ws_submenu.addItem(&next_workspace_item);
    let prev_workspace_item = make_menu_item(
        mtm,
        "Previous Workspace",
        Some(sel!(onPrevWorkspace:)),
        Some(handler),
        None,
        None,
        None,
        None,
    );
    ws_submenu.addItem(&prev_workspace_item);
    add_separator(&ws_submenu);
    workspace_item.setSubmenu(Some(&ws_submenu));
    workspace_item.setEnabled(false);
    menu.addItem(&workspace_item);

    let layouts_item = make_menu_item(mtm, "Layout Presets", None, None, None, None, None, None);
    let layouts_title = NSString::from_str("Layout Presets");
    let layouts_submenu: Retained<NSMenu> =
        unsafe { msg_send![NSMenu::alloc(mtm), initWithTitle: &*layouts_title] };
    let save_item = make_menu_item(mtm, "Save Layout", None, None, None, None, None, None);
    let save_title = NSString::from_str("Save Layout");
    let save_submenu: Retained<NSMenu> =
        unsafe { msg_send![NSMenu::alloc(mtm), initWithTitle: &*save_title] };
    for (title, action) in [
        ("Update Master File", sel!(onSaveMasterFile:)),
        ("Save Layout File…", sel!(onSaveLayout:)),
    ] {
        save_submenu.addItem(&make_menu_item(
            mtm,
            title,
            Some(action),
            Some(handler),
            None,
            None,
            None,
            None,
        ));
    }
    save_item.setSubmenu(Some(&save_submenu));
    layouts_submenu.addItem(&save_item);
    add_separator(&layouts_submenu);

    let mut restore_menus = Vec::new();
    for (title, master_action, picker_action) in [
        (
            "Restore to Workspace",
            sel!(onRestoreMasterFileWorkspace:),
            sel!(onRestoreWorkspace:),
        ),
        (
            "Restore to Space",
            sel!(onRestoreMasterFileSpace:),
            sel!(onRestoreSpace:),
        ),
    ] {
        let restore_item = make_menu_item(mtm, title, None, None, None, None, None, None);
        let restore_title = NSString::from_str(title);
        let restore_submenu: Retained<NSMenu> =
            unsafe { msg_send![NSMenu::alloc(mtm), initWithTitle: &*restore_title] };
        for (source, action) in [
            ("Master File", master_action),
            ("Choose File…", picker_action),
        ] {
            restore_submenu.addItem(&make_menu_item(
                mtm,
                source,
                Some(action),
                Some(handler),
                None,
                None,
                None,
                None,
            ));
        }
        restore_item.setSubmenu(Some(&restore_submenu));
        layouts_submenu.addItem(&restore_item);
        restore_menus.push(restore_submenu);
    }
    layouts_item.setSubmenu(Some(&layouts_submenu));
    menu.addItem(&layouts_item);

    add_separator(&menu);
    let tiling_item = make_menu_item(
        mtm,
        "Enable Tiling",
        Some(sel!(onToggleSpaceActivation:)),
        Some(handler),
        Some(false),
        None,
        None,
        None,
    );
    menu.addItem(&tiling_item);

    add_separator(&menu);
    menu.addItem(&make_menu_item(
        mtm,
        "Settings…",
        Some(sel!(onOpenConfig:)),
        Some(handler),
        None,
        None,
        None,
        None,
    ));
    let reload_item = make_menu_item(
        mtm,
        "Reload Config",
        Some(sel!(onReloadConfig:)),
        Some(handler),
        None,
        None,
        None,
        None,
    );
    menu.addItem(&reload_item);

    let help_item = make_menu_item(mtm, "Help / Documentation", None, None, None, None, None, None);
    let help_submenu_title = NSString::from_str("Help / Documentation");
    let help_submenu: Retained<NSMenu> =
        unsafe { msg_send![NSMenu::alloc(mtm), initWithTitle: &*help_submenu_title] };
    help_submenu.addItem(&make_menu_item(
        mtm,
        "Documentation",
        Some(sel!(onOpenDocumentation:)),
        Some(handler),
        None,
        None,
        None,
        None,
    ));
    help_submenu.addItem(&make_menu_item(
        mtm,
        "GitHub",
        Some(sel!(onOpenGitHub:)),
        Some(handler),
        None,
        None,
        None,
        None,
    ));
    help_submenu.addItem(&make_menu_item(
        mtm,
        "Matrix",
        Some(sel!(onOpenMatrix:)),
        Some(handler),
        None,
        None,
        None,
        None,
    ));

    help_item.setSubmenu(Some(&help_submenu));
    menu.addItem(&help_item);

    add_separator(&menu);
    menu.addItem(&make_menu_item(
        mtm,
        "Support Rift…",
        Some(sel!(onOpenSponsor:)),
        Some(handler),
        None,
        None,
        None,
        menu_image("heart", "Support Rift"),
    ));

    add_separator(&menu);
    let quit_item = make_menu_item(
        mtm,
        "Quit Rift",
        Some(sel!(onQuitRift:)),
        Some(handler),
        None,
        None,
        None,
        None,
    );
    menu.addItem(&quit_item);

    BuiltStatusMenu {
        menu,
        layout_items,
        workspace_item,
        workspace_submenu: ws_submenu,
        next_workspace_item,
        prev_workspace_item,
        tiling_item,
        reload_item,
        quit_item,
        restore_workspace_menu: restore_menus.remove(0),
        restore_space_menu: restore_menus.remove(0),
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

struct MenuActionHandlerIvars {
    action_tx: UnboundedSender<MenuAction>,
    layout_files: RefCell<Vec<PathBuf>>,
    layout_folder: RefCell<PathBuf>,
}

impl MenuActionHandler {
    fn new(mtm: MainThreadMarker, action_tx: UnboundedSender<MenuAction>) -> Retained<Self> {
        let this = mtm.alloc().set_ivars(MenuActionHandlerIvars {
            action_tx,
            layout_files: RefCell::new(Vec::new()),
            layout_folder: RefCell::new(PathBuf::new()),
        });
        unsafe { msg_send![super(this), init] }
    }

    fn emit(&self, action: MenuAction) { let _ = self.ivars().action_tx.send(action); }

    fn set_layout_files(&self, paths: Vec<PathBuf>) {
        *self.ivars().layout_files.borrow_mut() = paths;
    }

    fn set_layout_folder(&self, path: PathBuf) { *self.ivars().layout_folder.borrow_mut() = path; }

    fn layout_file_for_item(&self, item: Option<&NSMenuItem>) -> Option<PathBuf> {
        let index = usize::try_from(item?.tag()).ok()?;
        self.ivars().layout_files.borrow().get(index).cloned()
    }

    fn set_default_layout_directory(&self, panel: &NSSavePanel) {
        let directory = self.ivars().layout_folder.borrow();
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
        let mtm = MainThreadMarker::new()?;
        let panel = NSSavePanel::savePanel(mtm);
        self.set_default_layout_directory(&panel);
        Self::restrict_to_layout_files(&panel);
        panel.setCanCreateDirectories(true);
        panel.setNameFieldStringValue(&NSString::from_str("layout.ron"));
        (panel.runModal() == NSModalResponseOK)
            .then(|| panel.URL())
            .flatten()?
            .path()
            .map(|path| PathBuf::from(path.to_string()))
    }

    fn choose_layout_path(&self) -> Option<PathBuf> {
        let mtm = MainThreadMarker::new()?;
        let panel = NSOpenPanel::openPanel(mtm);
        self.set_default_layout_directory(&panel);
        Self::restrict_to_layout_files(&panel);
        panel.setCanChooseFiles(true);
        panel.setCanChooseDirectories(false);
        panel.setAllowsMultipleSelection(false);
        (panel.runModal() == NSModalResponseOK)
            .then(|| panel.URL())
            .flatten()?
            .path()
            .map(|path| PathBuf::from(path.to_string()))
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

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "RiftMenuBarActionHandler"]
    #[ivars = MenuActionHandlerIvars]
    struct MenuActionHandler;

    impl MenuActionHandler {
        #[unsafe(method(onSetLayoutTraditional:))]
        fn on_set_layout_traditional(&self, _sender: Option<&AnyObject>) {
            self.emit(MenuAction::SetLayout(LayoutMode::Traditional));
        }

        #[unsafe(method(onSetLayoutBsp:))]
        fn on_set_layout_bsp(&self, _sender: Option<&AnyObject>) {
            self.emit(MenuAction::SetLayout(LayoutMode::Bsp));
        }

        #[unsafe(method(onSetLayoutStack:))]
        fn on_set_layout_stack(&self, _sender: Option<&AnyObject>) {
            self.emit(MenuAction::SetLayout(LayoutMode::Stack));
        }

        #[unsafe(method(onSetLayoutMasterStack:))]
        fn on_set_layout_master_stack(&self, _sender: Option<&AnyObject>) {
            self.emit(MenuAction::SetLayout(LayoutMode::MasterStack));
        }

        #[unsafe(method(onSetLayoutScrolling:))]
        fn on_set_layout_scrolling(&self, _sender: Option<&AnyObject>) {
            self.emit(MenuAction::SetLayout(LayoutMode::Scrolling));
        }

        #[unsafe(method(onToggleSpaceActivation:))]
        fn on_toggle_space_activation(&self, _sender: Option<&AnyObject>) {
            self.emit(MenuAction::ToggleSpaceActivated);
        }

        #[unsafe(method(onNextWorkspace:))]
        fn on_next_workspace(&self, _sender: Option<&AnyObject>) {
            self.emit(MenuAction::NextWorkspace);
        }

        #[unsafe(method(onPrevWorkspace:))]
        fn on_prev_workspace(&self, _sender: Option<&AnyObject>) {
            self.emit(MenuAction::PrevWorkspace);
        }

        #[unsafe(method(onSwitchWorkspace:))]
        fn on_switch_workspace(&self, sender: Option<&NSMenuItem>) {
            if let Some(sender) = sender {
                let tag = sender.tag();
                if tag >= 0 {
                    self.emit(MenuAction::SwitchToWorkspace(tag as usize));
                }
            }
        }

        #[unsafe(method(onRestoreWorkspace:))]
        fn on_restore_workspace(&self, _sender: Option<&AnyObject>) {
            if let Some(path) = self.choose_layout_path().and_then(Self::validate_layout_path) {
                self.emit(MenuAction::RestoreLayout {
                    path,
                    scope: RestoreScope::Workspace,
                    source: RestoreSource::SavedActiveSpace,
                });
            }
        }

        #[unsafe(method(onRestoreLibraryWorkspace:))]
        fn on_restore_library_workspace(&self, sender: Option<&NSMenuItem>) {
            if let Some(path) = self
                .layout_file_for_item(sender)
                .and_then(Self::validate_layout_path)
            {
                self.emit(MenuAction::RestoreLayout {
                    path,
                    scope: RestoreScope::Workspace,
                    source: RestoreSource::SavedActiveSpace,
                });
            }
        }

        #[unsafe(method(onSaveLayout:))]
        fn on_save_layout(&self, _sender: Option<&AnyObject>) {
            if let Some(path) = self.choose_save_path() {
                self.emit(MenuAction::SaveLayout(path));
            }
        }

        #[unsafe(method(onSaveMasterFile:))]
        fn on_save_master_file(&self, _sender: Option<&AnyObject>) {
            self.emit(MenuAction::SaveMasterFile);
        }

        #[unsafe(method(onRestoreMasterFileWorkspace:))]
        fn on_restore_master_file_workspace(&self, _sender: Option<&AnyObject>) {
            if Self::validate_layout_path(restore_file()).is_some() {
                self.emit(MenuAction::RestoreMasterFile(RestoreScope::Workspace));
            }
        }

        #[unsafe(method(onRestoreMasterFileSpace:))]
        fn on_restore_master_file_space(&self, _sender: Option<&AnyObject>) {
            if Self::validate_layout_path(restore_file()).is_some() {
                self.emit(MenuAction::RestoreMasterFile(RestoreScope::Space));
            }
        }

        #[unsafe(method(onRestoreSpace:))]
        fn on_restore_space(&self, _sender: Option<&AnyObject>) {
            if let Some(path) = self.choose_layout_path().and_then(Self::validate_layout_path) {
                self.emit(MenuAction::RestoreLayout {
                    path,
                    scope: RestoreScope::Space,
                    source: RestoreSource::SavedActiveSpace,
                });
            }
        }

        #[unsafe(method(onRestoreLibrarySpace:))]
        fn on_restore_library_space(&self, sender: Option<&NSMenuItem>) {
            if let Some(path) = self
                .layout_file_for_item(sender)
                .and_then(Self::validate_layout_path)
            {
                self.emit(MenuAction::RestoreLayout {
                    path,
                    scope: RestoreScope::Space,
                    source: RestoreSource::SavedActiveSpace,
                });
            }
        }

        #[unsafe(method(onOpenConfig:))]
        fn on_open_config(&self, _sender: Option<&AnyObject>) {
            self.emit(MenuAction::OpenConfig);
        }

        #[unsafe(method(onOpenDocumentation:))]
        fn on_open_documentation(&self, _sender: Option<&AnyObject>) {
            self.emit(MenuAction::OpenDocumentation);
        }

        #[unsafe(method(onOpenGitHub:))]
        fn on_open_github(&self, _sender: Option<&AnyObject>) {
            self.emit(MenuAction::OpenGitHub);
        }

        #[unsafe(method(onOpenMatrix:))]
        fn on_open_matrix(&self, _sender: Option<&AnyObject>) {
            self.emit(MenuAction::OpenMatrix);
        }

        #[unsafe(method(onOpenSponsor:))]
        fn on_open_sponsor(&self, _sender: Option<&AnyObject>) {
            self.emit(MenuAction::OpenSponsor);
        }

        #[unsafe(method(onReloadConfig:))]
        fn on_reload_config(&self, _sender: Option<&AnyObject>) {
            self.emit(MenuAction::ReloadConfig);
        }

        #[unsafe(method(onQuitRift:))]
        fn on_quit_rift(&self, _sender: Option<&AnyObject>) {
            self.emit(MenuAction::QuitRift);
        }
    }
);

#[cfg(test)]
mod layout_library_tests {
    use super::*;

    fn workspace(
        id: &str,
        index: usize,
        name: &str,
        active: bool,
        layout: &str,
    ) -> RuntimeWorkspaceData {
        RuntimeWorkspaceData {
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
                                window.origin.y,
                                window.size.width,
                                window.size.height,
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
