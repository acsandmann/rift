//! Lazy main-thread Settings composition. ConfigActor owns all mutations and persistence.
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::Arc;

use cgs::*;
use objc2::rc::Retained;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSEventModifierFlags, NSMenu};
use rift_protocol::ApplicationData;
use tokio::sync::mpsc::UnboundedSender;

use crate::actor::config::SourceEdit;
use crate::common::config::{ConfigSchema, ConfigSource};
use crate::sys::screen::ScreenInfo;

mod applications;
mod commands;
mod editors;
mod interface;
mod interface_preview;
mod layouts;
mod pages;
pub mod process;
mod schema;
mod search;
mod sidebar;
use sidebar::SettingsSidebar;
pub mod timing;
mod updates;

pub type Finish = Box<dyn FnOnce(Result<(), String>)>;

pub enum Action {
    Edit(SourceEdit),
    Reload,
    RefreshRuntime,
    CheckUpdates(updates::Completion),
}
pub struct Request {
    pub action: Action,
    pub finish: Finish,
    /// Identity of an absolute control assignment; transactions have no identity.
    pub control: Option<Rc<()>>,
}

/// One Settings hierarchy; enum order is the sidebar order and cache index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Category {
    General,
    Layouts,
    Workspaces,
    Rules,
    Keyboard,
    Mouse,
    MenuBar,
    Overview,
    StackTabs,
    Advanced,
    About,
}
impl Category {
    fn index(self) -> usize { self as usize }

    fn title(self) -> &'static str { CATEGORIES[self.index()].1 }
}
pub(super) const CATEGORIES: [(Category, &str, &str); 11] = [
    (Category::General, "General", "gearshape"),
    (Category::Layouts, "Layouts", "rectangle.split.2x2"),
    (Category::Workspaces, "Workspaces", "square.grid.2x2"),
    (Category::Rules, "Rules", "line.3.horizontal.decrease"),
    (Category::Keyboard, "Keyboard", "keyboard"),
    (Category::Mouse, "Mouse & Trackpad", "computermouse"),
    (Category::MenuBar, "Menu Bar", "menubar.rectangle"),
    (Category::Overview, "Overview", "rectangle.3.group"),
    (Category::StackTabs, "Stack Tabs", "square.stack.3d.up"),
    (Category::Advanced, "Advanced", "slider.horizontal.3"),
    (Category::About, "About", "info.circle"),
];
/// Show an edit's outcome under the control that made it.
fn report(message: &Weak<ValidationMessage>, result: &Result<(), String>) {
    if let Some(message) = message.upgrade() {
        message.set_validation(&match result {
            Ok(()) => Validation::None,
            Err(error) => Validation::Error(error.clone()),
        });
    }
}

/// The shipped defaults, parsed once, so menus can mark each setting’s default choice.
fn defaults() -> &'static ConfigSource {
    static DEFAULTS: std::sync::OnceLock<ConfigSource> = std::sync::OnceLock::new();
    DEFAULTS.get_or_init(|| {
        crate::common::config::ConfigDocument::default()
            .source()
            .expect("the shipped defaults parse")
    })
}

type DraftChanged = Box<dyn Fn(bool)>;
type Preview = Rc<dyn Fn(&ConfigSource)>;

/// Runtime context shared by the window model and its sheet drafts.
struct Env {
    requests: UnboundedSender<Request>,
    window: RefCell<WindowRef>,
    displays: RefCell<Vec<ScreenInfo>>,
    config_path: PathBuf,
    applications: RefCell<Vec<ApplicationData>>,
    installed_applications: RefCell<Option<Vec<(String, String)>>>,
    inventory: RefCell<applications::Inventory>,
    /// Weak native picker updates; expired views are pruned on the next inventory change.
    application_views: RefCell<Vec<Box<dyn Fn() -> bool>>>,
}

impl Env {
    fn send(&self, action: Action, finish: Finish) {
        let _ = self.requests.send(Request { action, finish, control: None });
    }

    fn rebuild_applications(&self) {
        *self.inventory.borrow_mut() = applications::Inventory::new(
            &self.applications.borrow(),
            self.installed_applications.borrow().as_deref().unwrap_or(&[]),
        );
        self.application_views.borrow_mut().retain(|update| update());
    }

    fn set_installed_applications(&self, apps: Vec<(String, String)>) {
        *self.installed_applications.borrow_mut() = Some(apps);
        self.rebuild_applications();
    }

    fn refresh_applications(&self, applications: Vec<ApplicationData>) {
        let changed = applications::running(&self.applications.borrow())
            .ne(applications::running(&applications));
        *self.applications.borrow_mut() = applications;
        if changed {
            self.rebuild_applications();
        }
    }
}

/// An editor sheet's uncommitted copy of the source.
struct Draft {
    base: Arc<ConfigSource>,
    page: RefCell<Option<Rc<Page>>>,
    changed: RefCell<Option<DraftChanged>>,
}

struct Model {
    env: Rc<Env>,
    source: RefCell<Arc<ConfigSource>>,
    source_revision: Cell<u64>,
    syncing: Cell<bool>,
    /// The open sheet and the draft it edits; only the window model opens sheets.
    sheet: RefCell<Option<Sheet>>,
    sheet_model: RefCell<Option<Rc<Model>>>,
    draft: Option<Draft>,
}

type SyncControl = Box<dyn Fn(&ConfigSource)>;
pub(super) struct Page {
    view: Rc<dyn NativeView>,
    sync: Vec<SyncControl>,
    header: Option<Rc<HeaderControls>>,
    synced_revision: Cell<Option<u64>>,
}

impl Model {
    fn new(env: Rc<Env>, source: impl Into<Arc<ConfigSource>>, draft: Option<Draft>) -> Rc<Self> {
        Rc::new(Self {
            env,
            source: RefCell::new(source.into()),
            source_revision: Cell::new(0),
            syncing: Cell::new(false),
            sheet: RefCell::new(None),
            sheet_model: RefCell::new(None),
            draft,
        })
    }

    fn submit(model: &Weak<Self>, edit: SourceEdit, error: Weak<ValidationMessage>) {
        if let Some(model) = model.upgrade() {
            model.submit_edit(edit, Box::new(move |result| report(&error, &result)));
        }
    }

    fn is_draft(&self) -> bool { self.draft.is_some() }

    fn close_sheet(&self) {
        self.sheet.borrow_mut().take();
        self.sheet_model.borrow_mut().take();
    }

    fn draft(&self) -> Rc<Self> {
        let base = self.source.borrow().clone();
        Self::new(
            self.env.clone(),
            base.clone(),
            Some(Draft {
                base,
                page: RefCell::new(None),
                changed: RefCell::new(None),
            }),
        )
    }

    fn submit_edit(&self, edit: SourceEdit, finish: Finish) {
        if self.syncing.get() {
            return;
        }
        let Some(draft) = &self.draft else {
            self.env.send(Action::Edit(edit), finish);
            return;
        };
        let mut source = self.source.borrow().as_ref().clone();
        let result = edit(&mut source);
        if result.is_ok() {
            self.replace_source(source);
            if let Some(page) = draft.page.borrow().as_ref() {
                page.synchronize(self);
            }
            if let Some(changed) = draft.changed.borrow().as_ref() {
                changed(*self.source.borrow() != draft.base);
            }
        }
        finish(result);
    }

    fn replace_source(&self, source: impl Into<Arc<ConfigSource>>) {
        let source = source.into();
        if !Arc::ptr_eq(&self.source.borrow(), &source) && *self.source.borrow() != source {
            *self.source.borrow_mut() = source;
            self.source_revision.set(self.source_revision.get() + 1);
        }
    }
}
impl Page {
    fn synchronize(&self, model: &Model) {
        let revision = model.source_revision.get();
        if self.synced_revision.get() == Some(revision) {
            return;
        }
        let syncing = model.syncing.replace(true);
        let source = model.source.borrow();
        for sync in &self.sync {
            sync(&source);
        }
        model.syncing.set(syncing);
        self.synced_revision.set(Some(revision));
    }
}

/// Lazy top-level pages and the chrome that follows the sidebar selection.
struct Router {
    ui: Ui,
    model: Rc<Model>,
    host: Rc<PageHost>,
    /// Built lazily and kept while the window is open.
    pages: RefCell<Vec<Option<Page>>>,
    selected: Cell<Category>,
    history: RefCell<Vec<Category>>,
    history_index: Cell<usize>,
    layouts: std::cell::OnceCell<Rc<layouts::Inspector>>,
    title: Rc<Label>,
    toolbar: RefCell<Weak<Toolbar>>,
    sidebar: RefCell<Weak<SettingsSidebar>>,
    /// Set while the router itself moves the sidebar selection.
    routing: Cell<bool>,
}

impl Router {
    fn current(&self) -> Category { self.selected.get() }

    fn navigate(&self, id: Category) {
        if id != self.current() {
            let mut history = self.history.borrow_mut();
            history.truncate(self.history_index.get() + 1);
            history.push(id);
            self.history_index.set(history.len() - 1);
        }
        self.visit(id);
    }

    fn step_history(&self, offset: isize) {
        let Some(index) = self.history_index.get().checked_add_signed(offset) else {
            return;
        };
        let Some(id) = self.history.borrow().get(index).copied() else {
            return;
        };
        self.history_index.set(index);
        self.visit(id);
    }

    fn visit(&self, id: Category) {
        self.model.close_sheet();
        self.selected.set(id);
        self.routing.set(true);
        if let Some(sidebar) = self.sidebar.borrow().upgrade() {
            sidebar.set_selected(id);
        }
        self.routing.set(false);
        self.show(id);
    }

    fn layouts(&self) -> &Rc<layouts::Inspector> {
        self.layouts.get_or_init(|| layouts::Inspector::new(self.ui, &self.model))
    }

    fn open_search(&self, entry: &search::Entry) {
        self.navigate(entry.page);
        if entry.page == Category::Layouts {
            if let Some(target) = entry.target() {
                self.layouts().select_target(target);
            }
        }
        self.host.reveal(search::section(&entry.location), entry.title);
    }

    fn show(&self, id: Category) {
        self.title.set_text(id.title());
        if self.pages.borrow()[id.index()].is_none() {
            let page_timing = timing::start();
            let page = if id == Category::Layouts {
                self.layouts().page()
            } else {
                pages::build(self.ui, &self.model, id)
            };
            self.pages.borrow_mut()[id.index()] = Some(page);
            if id == Category::General {
                timing::finish(page_timing, "initial page construction");
            }
        }
        let pages = self.pages.borrow();
        let page = pages[id.index()].as_ref().unwrap();
        page.synchronize(&self.model);
        self.host.set_cached_page(page.view.clone());
        if let Some(toolbar) = self.toolbar.borrow().upgrade() {
            toolbar.set_page_controls(&self.ui, page.header.as_deref());
            let trailing = (id == Category::Layouts).then(|| self.layouts().default_control());
            toolbar.set_trailing_control(&self.ui, trailing.as_deref());
            toolbar.update_navigation(
                self.history_index.get() > 0,
                self.history_index.get() + 1 < self.history.borrow().len(),
                id.title(),
                false,
            );
        }
    }
}

pub struct Settings {
    window: SettingsWindow,
    router: Rc<Router>,
    activation: Rc<SettingsActivation>,
}

/// Keep menu-bar ownership for the whole window lifetime, including its sheets.
struct SettingsActivation {
    ui: Ui,
    menu: Menu,
    previous: RefCell<Option<(NSApplicationActivationPolicy, Option<Retained<NSMenu>>)>>,
}

impl SettingsActivation {
    fn activate(&self) {
        let app = NSApplication::sharedApplication(self.ui.mtm());
        if self.previous.borrow().is_none() {
            *self.previous.borrow_mut() = Some((app.activationPolicy(), app.mainMenu()));
        }
        app.setMainMenu(Some(self.menu.ns_menu()));
        if app.activationPolicy() != NSApplicationActivationPolicy::Regular {
            app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
        }
    }

    fn restore(&self) {
        if let Some((policy, menu)) = self.previous.borrow_mut().take() {
            let app = NSApplication::sharedApplication(self.ui.mtm());
            app.setMainMenu(menu.as_deref());
            app.setActivationPolicy(policy);
        }
    }
}

impl Settings {
    pub fn new(
        ui: Ui,
        source: impl Into<Arc<ConfigSource>>,
        config_path: PathBuf,
        displays: Vec<ScreenInfo>,
        applications: Vec<ApplicationData>,
        requests: UnboundedSender<Request>,
        mut on_close: impl FnMut() + 'static,
    ) -> Self {
        let env = Rc::new(Env {
            requests,
            window: RefCell::new(WindowRef::default()),
            displays: RefCell::new(displays),
            config_path,
            applications: RefCell::new(applications),
            installed_applications: RefCell::new(None),
            inventory: RefCell::default(),
            application_views: RefCell::default(),
        });
        env.rebuild_applications();
        let host = Rc::new(PageHost::new(&ui));
        let router = Rc::new(Router {
            ui,
            model: Model::new(env.clone(), source, None),
            host: host.clone(),
            pages: RefCell::new((0..CATEGORIES.len()).map(|_| None).collect()),
            selected: Cell::new(Category::General),
            history: RefCell::new(vec![Category::General]),
            history_index: Cell::new(0),
            layouts: std::cell::OnceCell::new(),
            title: Rc::new(Label::new(&ui, Category::General.title())),
            toolbar: RefCell::new(Weak::new()),
            sidebar: RefCell::new(Weak::new()),
            routing: Cell::new(false),
        });
        let sidebar_timing = timing::start();
        let weak_router = Rc::downgrade(&router);
        let sidebar = Rc::new(SettingsSidebar::new(&ui, move |id| {
            if let Some(router) = weak_router.upgrade()
                && !router.routing.get()
            {
                router.navigate(id);
            }
        }));
        sidebar.set_selected(Category::General);
        *router.sidebar.borrow_mut() = Rc::downgrade(&sidebar);

        timing::finish(sidebar_timing, "sidebar construction");
        let chrome_timing = timing::start();
        let activation = Rc::new(SettingsActivation {
            ui,
            menu: Menu::new(&ui),
            previous: RefCell::new(None),
        });
        let close_activation = activation.clone();
        let window = SettingsWindow::with_size(&ui, "Rift Settings", CGSize::new(880.0, 660.0));
        timing::finish(chrome_timing, "native window creation");
        let attachment_timing = timing::start();
        let window = window.page_title(&ui, router.title.clone()).on_close(move || {
            close_activation.restore();
            on_close();
        });
        timing::finish(attachment_timing, "toolbar attachment");
        window.ns_window().center();
        let content_timing = timing::start();
        let navigation =
            NavigationSplitView::new(&ui, sidebar_pane(ui, &router, sidebar.clone()), host);
        navigation
            .ns_split_view_controller()
            .splitViewItems()
            .objectAtIndex(0)
            .setHoldingPriority(250.0);
        let window = window.content(navigation);
        timing::finish(content_timing, "content attachment");
        let navigation_timing = timing::start();
        window.initial_focus(&*sidebar);
        let app_menu = Rc::new(Menu::new(&ui));
        let close_window = objc2::rc::Weak::new(window.ns_window());
        app_menu.add(
            MenuItem::new(&ui, "Close Settings")
                .shortcut("w", NSEventModifierFlags::Command)
                .on_click(move || {
                    if let Some(window) = close_window.load() {
                        window.performClose(None);
                    }
                }),
        );
        activation.menu.add(MenuItem::new(&ui, "Rift").submenu(app_menu));
        *router.toolbar.borrow_mut() = Rc::downgrade(window.toolbar());
        let back = Rc::downgrade(&router);
        let forward = Rc::downgrade(&router);
        window.toolbar().set_navigation(
            &ui,
            move || {
                if let Some(router) = back.upgrade() {
                    router.step_history(-1);
                }
            },
            move || {
                if let Some(router) = forward.upgrade() {
                    router.step_history(1);
                }
            },
            Rc::new(Menu::new(&ui)),
        );
        *env.window.borrow_mut() = window.handle();
        timing::finish(navigation_timing, "navigation/menu");
        router.show(Category::General);
        Self { window, router, activation }
    }

    fn model(&self) -> &Rc<Model> { &self.router.model }

    pub fn has_installed_applications(&self) -> bool {
        self.model().env.installed_applications.borrow().is_some()
    }

    pub fn set_installed_applications(&self, apps: Vec<(String, String)>) {
        self.model().env.set_installed_applications(apps);
    }

    pub fn refresh_applications(&self, applications: Vec<ApplicationData>) {
        self.model().env.refresh_applications(applications);
    }

    pub fn refresh_displays(&self, displays: Vec<ScreenInfo>) {
        let env = &self.model().env;
        if *env.displays.borrow() == displays {
            return;
        }
        *env.displays.borrow_mut() = displays;
        if let Some(draft) = self.model().sheet_model.borrow().as_ref()
            && let Some(page) = draft.draft.as_ref().and_then(|draft| draft.page.borrow().clone())
        {
            page.synced_revision.set(None);
            page.synchronize(draft);
        }
        if let Some(layouts) = self.router.layouts.get() {
            layouts.refresh_displays();
        }
    }

    pub fn show(&self) {
        let activation = timing::start();
        self.activation.activate();
        timing::finish(activation, "activation policy/menu");
        let present = timing::start();
        self.window.present();
        timing::finish(present, "window presentation");
    }

    /// Updates the current page even while hidden or minimized; other pages sync when shown.
    pub fn synchronize(&self, source: impl Into<Arc<ConfigSource>>) {
        self.model().replace_source(source);
        if let Some(page) = &self.router.pages.borrow()[self.router.current().index()] {
            page.synchronize(self.model());
        }
    }
}

impl Drop for Settings {
    fn drop(&mut self) {
        // End the sheet while its weak parent still points to the live Settings window.
        self.model().close_sheet();
        self.window.toolbar().set_page_controls(&self.router.ui, None);
        self.router.host.clear();
        self.router.pages.borrow_mut().clear();
        self.activation.restore();
    }
}

/// The sidebar, replaced by search results while a query is entered.
fn sidebar_pane(ui: Ui, router: &Rc<Router>, sidebar: Rc<SettingsSidebar>) -> View {
    let content = Rc::new(PageHost::new(&ui));
    content.set_cached_page(sidebar.clone());
    let weak_router = Rc::downgrade(router);
    let open = Rc::new(move |entry: &'static search::Entry| {
        if let Some(router) = weak_router.upgrade() {
            router.open_search(entry);
        }
    });
    let mut results = None;
    let mut empty = None;
    let first = Rc::new(Cell::new(None::<&'static search::Entry>));
    let top = first.clone();
    let open_results = open.clone();
    let weak_content = Rc::downgrade(&content);
    let search = SearchField::new(&ui)
        .placeholder("Search")
        .on_change(move |query| {
            let Some(content) = weak_content.upgrade() else {
                return;
            };
            let hits = search::results(&query);
            top.set(hits.first().copied());
            if hits.is_empty() {
                content.set_cached_page(if query.trim().is_empty() {
                    sidebar.clone()
                } else {
                    empty
                        .get_or_insert_with(|| {
                            Rc::new(
                                VStack::new(&ui)
                                    .insets(uniform_insets(12.0))
                                    .push(Caption::new(&ui, "No matching settings")),
                            )
                        })
                        .clone()
                });
                return;
            }
            let results = results.get_or_insert_with(|| {
                let pick = open_results.clone();
                Rc::new(
                    Table::new(&ui)
                        .source_list()
                        .column("setting", "", 0.0)
                        .cells(move |row: &search::Row, _| {
                            let content = VStack::new(&ui).spacing(3.0).insets(Insets {
                                top: 6.0,
                                left: 12.0,
                                bottom: 6.0,
                                right: 12.0,
                            });
                            match row {
                                search::Row::Heading(title) => {
                                    Box::new(content.push(SubsectionTitle::new(&ui, title)))
                                        as Box<dyn NativeView>
                                }
                                search::Row::Setting(entry) => Box::new(
                                    content
                                        .push(Label::new(&ui, entry.title))
                                        .push(Caption::new(&ui, &entry.description).max_lines(2)),
                                ),
                            }
                        })
                        .group_rows(|row| matches!(row, search::Row::Heading(_)))
                        .selectable(|row| matches!(row, search::Row::Setting(_)))
                        .row_heights(|row| {
                            if matches!(row, search::Row::Heading(_)) {
                                30.0
                            } else {
                                54.0
                            }
                        })
                        .on_select_item(move |row| {
                            if let Some(search::Row::Setting(entry)) = row {
                                pick(entry);
                            }
                        }),
                )
            });
            results.set_rows(search::grouped(&hits));
            results.scroll_to(0);
            content.set_cached_page(results.clone());
        })
        .on_commit(move |_| {
            if let Some(entry) = first.get() {
                open(entry);
            }
        });
    search.control_size(objc2_app_kit::NSControlSize::Large);
    search.height(28.0);
    let pane = VStack::new(&ui)
        .spacing(4.0)
        .push(
            HStack::new(&ui)
                .insets(Insets {
                    top: 6.0,
                    left: 10.0,
                    bottom: 0.0,
                    right: 10.0,
                })
                .push(search),
        )
        .push(content);
    View::new(&ui).safe_area_content(pane)
}

type SliderRange = Rc<dyn Fn(&ConfigSource) -> (f64, f64)>;

pub(super) struct FormBuilder {
    ui: Ui,
    model: Weak<Model>,
    draft: bool,
    sync: Vec<SyncControl>,
    live_preview: Option<Preview>,
    /// Explain settings with help tags rather than inline info buttons.
    tooltips: bool,
}
impl FormBuilder {
    fn new(ui: Ui, model: &Rc<Model>) -> Self {
        Self {
            ui,
            model: Rc::downgrade(model),
            draft: model.is_draft(),
            sync: Vec::new(),
            live_preview: None,
            tooltips: false,
        }
    }

    fn finish(self, view: impl NativeView) -> Page {
        Page {
            view: Rc::new(view),
            sync: self.sync,
            header: None,
            synced_revision: Cell::new(None),
        }
    }

    fn row(
        &self,
        title: &str,
        control: impl NativeView,
        message: Rc<ValidationMessage>,
    ) -> SettingsRow {
        let (title, suffix) = if let Some(title) = title.strip_suffix(" (%)") {
            (title, Some("%"))
        } else if let Some(title) = title.strip_suffix(" (points)") {
            (title, Some("pt"))
        } else {
            (title, None)
        };
        let row = SettingsRow::with_validation(&self.ui, title, control, message);
        if let Some(suffix) = suffix {
            row.suffix(suffix)
        } else {
            row
        }
    }

    fn sync<C: NativeView>(&mut self, input: &Rc<C>, update: impl Fn(&C, &ConfigSource) + 'static) {
        let weak = Rc::downgrade(input);
        self.sync.push(Box::new(move |source| {
            if let Some(input) = weak.upgrade() {
                update(&input, source);
            }
        }));
    }

    /// Cache each control's effective dependencies, rather than the whole source.
    fn sync_when<C: NativeView, V: PartialEq + 'static>(
        &mut self,
        input: &Rc<C>,
        value: impl Fn(&ConfigSource) -> V + 'static,
        update: impl Fn(&C, &ConfigSource) + 'static,
    ) {
        let last = RefCell::new(None);
        self.sync(input, move |input, source| {
            let value = value(source);
            if last.borrow().as_ref() != Some(&value) {
                update(input, source);
                *last.borrow_mut() = Some(value);
            }
        });
    }

    /// Submit an absolute assignment; only local drafts can skip it against their source.
    fn change<T: PartialEq + Send + 'static>(
        &self,
        get: impl Fn(&ConfigSource) -> T + 'static,
        set: impl Fn(&mut ConfigSource, T) -> Result<(), String> + Send + Clone + 'static,
        error: Weak<ValidationMessage>,
    ) -> impl Fn(T) + Clone + 'static {
        let model = self.model.clone();
        let control = Rc::new(());
        let preview = self.live_preview.as_ref().map(Rc::downgrade);
        let get = Rc::new(get);
        move |value| {
            let Some(model) = model.upgrade() else { return };
            // Only drafts have no unacknowledged edits. Comparing with the daemon's
            // old snapshot here could discard a quick return to the original value.
            if model.is_draft() && get(&model.source.borrow()) == value {
                return;
            }
            let set = set.clone();
            let edit: SourceEdit = Box::new(move |source| set(source, value));
            let error = error.clone();
            let preview = preview.clone();
            let recovery_model = Rc::downgrade(&model);
            let finish: Finish = Box::new(move |result| {
                report(&error, &result);
                if result.is_err()
                    && let Some(preview) = preview.and_then(|preview| preview.upgrade())
                    && let Some(model) = recovery_model.upgrade()
                {
                    preview(&model.source.borrow());
                }
            });
            if model.is_draft() {
                model.submit_edit(edit, finish);
            } else if !model.syncing.get() {
                let _ = model.env.requests.send(Request {
                    action: Action::Edit(edit),
                    finish,
                    control: Some(control.clone()),
                });
            }
        }
    }

    /// `change` for setters that cannot fail.
    fn assign<T: PartialEq + Send + 'static>(
        &self,
        get: impl Fn(&ConfigSource) -> T + 'static,
        set: impl Fn(&mut ConfigSource, T) + Send + Clone + 'static,
        error: Weak<ValidationMessage>,
    ) -> impl Fn(T) + Clone + 'static {
        self.change(
            get,
            move |source, value| {
                set(source, value);
                Ok(())
            },
            error,
        )
    }

    /// Schema rows for `keys`, in order.
    fn fields<T: ConfigSchema>(
        &mut self,
        mut section: Section,
        keys: &[&str],
        get: fn(&ConfigSource) -> &T,
        set: fn(&mut ConfigSource) -> &mut T,
    ) -> Section {
        for key in keys {
            section = section.row(self.field(key, get, set));
        }
        section
    }

    fn field<T: ConfigSchema>(
        &mut self,
        key: &str,
        get: fn(&ConfigSource) -> &T,
        set: fn(&mut ConfigSource) -> &mut T,
    ) -> SettingsRow {
        let field = T::field(key).unwrap_or_else(|| panic!("unknown setting `{key}`"));
        self.schema_field(field, get, set)
            .unwrap_or_else(|| panic!("setting `{key}` needs a custom editor"))
    }

    fn switch(
        &mut self,
        title: &str,
        get: impl Fn(&ConfigSource) -> bool + 'static,
        set: impl Fn(&mut ConfigSource, bool) + Send + Clone + 'static,
    ) -> SettingsRow {
        let message = Rc::new(ValidationMessage::new(&self.ui));
        let get = Rc::new(get);
        let current = get.clone();
        let change = self.assign(move |s| current(s), set, Rc::downgrade(&message));
        let input = Rc::new(Switch::new(&self.ui).on_change(change));
        self.sync_when(
            &input,
            {
                let get = get.clone();
                move |s| get(s)
            },
            move |input, s| input.set_value(get(s)),
        );
        self.row(title, input, message)
    }

    fn percentage(
        &mut self,
        title: &str,
        range: impl Fn(&ConfigSource) -> (f64, f64) + 'static,
        get: impl Fn(&ConfigSource) -> f64 + 'static,
        set: impl Fn(&mut ConfigSource, f64) + Send + Clone + 'static,
    ) -> SettingsRow {
        let (slider, input, message) =
            self.numeric_controls(title, 100.0, false, Some(Rc::new(range)), get, set);
        // Percentages move in steps of five; the field still takes exact values.
        let slider = slider.unwrap();
        slider.set_step(5.0);
        self.row(title, NumberSlider::new(&self.ui, slider, input), message)
    }

    // Bind native controls independently of their form layout.
    fn numeric_controls(
        &mut self,
        title: &str,
        scale: f64,
        integer: bool,
        slider_range: Option<SliderRange>,
        get: impl Fn(&ConfigSource) -> f64 + 'static,
        set: impl Fn(&mut ConfigSource, f64) + Send + Clone + 'static,
    ) -> (Option<Rc<Slider>>, Rc<NumberField>, Rc<ValidationMessage>) {
        let message = Rc::new(ValidationMessage::new(&self.ui));
        let get = Rc::new(get);
        let current = get.clone();
        let edit_preview = set.clone();
        let change = self.assign(move |s| current(s), set, Rc::downgrade(&message));
        let commit = move |value: f64| change(value / scale);
        let field = if integer {
            NumberField::new(&self.ui).integer()
        } else {
            NumberField::new(&self.ui)
        };
        let input = Rc::new(if self.draft {
            field.on_edit(commit.clone())
        } else {
            field.on_change(commit.clone())
        });
        let slider = slider_range.as_ref().map(|range| {
            let model = self.model.clone();
            let (min, max) = range(&model.upgrade().unwrap().source.borrow());
            let range = range.clone();
            let preview = self.live_preview.clone();
            let preview_input = Rc::downgrade(&input);
            let mut local_preview = None::<ConfigSource>;
            let app = Application::shared(&self.ui);
            let slider = Slider::new(&self.ui).range(min, max).continuous(true);
            let slider = Rc::new(slider.on_change(move |v| {
                let Some(model) = model.upgrade() else {
                    return;
                };
                let (min, max) = range(&model.source.borrow());
                let value = if preview.is_some() && !integer {
                    (v * 10.0).round() / 10.0
                } else {
                    v.round()
                }
                .clamp(min, max);
                if let Some(preview) = &preview {
                    let local =
                        local_preview.get_or_insert_with(|| model.source.borrow().as_ref().clone());
                    edit_preview(local, value / scale);
                    preview(local);
                }
                // Show the value while dragging; commit once on release.
                if let Some(input) = preview_input.upgrade() {
                    input.set_value(value);
                }
                if app.is_mouse_dragging() {
                    return;
                }
                local_preview = None;
                commit(value);
            }));
            slider.accessibility_label(title);
            slider
        });
        input.accessibility_label(title);
        let weak = Rc::downgrade(&input);
        let weak_slider = slider.as_ref().map(Rc::downgrade);
        let last = RefCell::new(None);
        self.sync.push(Box::new(move |s| {
            let effective = (get(s) * scale, slider_range.as_ref().map(|range| range(s)));
            if last.borrow().as_ref() == Some(&effective) {
                return;
            }
            *last.borrow_mut() = Some(effective);
            let Some(input) = weak.upgrade() else {
                return;
            };
            let value = get(s) * scale;
            input.set_value(value);
            if let Some(slider) = weak_slider.as_ref().and_then(Weak::upgrade) {
                if let Some(range) = &slider_range {
                    let (min, max) = range(s);
                    slider.set_range(min, max);
                }
                slider.set_value(value);
            }
        }));
        (slider, input, message)
    }

    fn text(
        &mut self,
        title: &str,
        get: impl Fn(&ConfigSource) -> String + 'static,
        set: impl Fn(&mut ConfigSource, String) -> Result<(), String> + Send + Clone + 'static,
    ) -> SettingsRow {
        let message = Rc::new(ValidationMessage::new(&self.ui));
        let get = Rc::new(get);
        let current = get.clone();
        let commit = self.change(move |s| current(s), set, Rc::downgrade(&message));
        let field = TextField::new(&self.ui);
        let input = Rc::new(if self.draft {
            field.on_change(commit)
        } else {
            field.on_commit(commit)
        });
        self.sync_when(
            &input,
            {
                let get = get.clone();
                move |s| get(s)
            },
            move |input, s| input.set_value(&get(s)),
        );
        self.row(title, input, message)
    }

    fn popup<T: Clone + PartialEq + Send + 'static>(
        &mut self,
        title: &str,
        values: Vec<(&str, T)>,
        get: impl Fn(&ConfigSource) -> T + 'static,
        set: impl Fn(&mut ConfigSource, T) + Send + Clone + 'static,
    ) -> SettingsRow {
        let message = Rc::new(ValidationMessage::new(&self.ui));
        let get = Rc::new(get);
        let current = get.clone();
        let items: Rc<[T]> = values.iter().map(|(_, value)| value.clone()).collect();
        let choices = items.clone();
        let change = self.assign(move |s| current(s), set, Rc::downgrade(&message));
        let input = Rc::new(
            Popup::new(&self.ui)
                .items(values.iter().map(|(label, _)| *label))
                .on_change(move |index| change(choices[index].clone())),
        );
        let default = get(defaults());
        if let Some(index) = items.iter().position(|item| *item == default) {
            input.set_item_badge(index, Some("Default"));
        }
        self.sync_when(
            &input,
            {
                let get = get.clone();
                move |s| get(s)
            },
            move |input, s| {
                input.set_selected(items.iter().position(|v| *v == get(s)).unwrap_or(0));
            },
        );
        self.row(title, input, message)
    }

    /// A choice that follows another value until overridden. `source` comes first, such
    /// as “Use shared setting”, above the explicit values; while it’s chosen, `describe`
    /// names the value in effect beneath the popup, and the menu badges it with `badge`.
    fn following_popup<T: Clone + PartialEq + Send + 'static>(
        &mut self,
        title: &str,
        (source, badge, describe): (&'static str, &'static str, fn(&str) -> String),
        values: Vec<(&'static str, T)>,
        get: impl Fn(&ConfigSource) -> Option<T> + 'static,
        followed: impl Fn(&ConfigSource) -> T + 'static,
        set: impl Fn(&mut ConfigSource, Option<T>) + Send + Clone + 'static,
    ) -> SettingsRow {
        let message = Rc::new(ValidationMessage::new(&self.ui));
        let values: Rc<[(&'static str, T)]> = values.into();
        let choices = values.clone();
        let get = Rc::new(get);
        let current = get.clone();
        let change = self.assign(move |s| current(s), set, Rc::downgrade(&message));
        let input = Rc::new(
            Popup::new(&self.ui)
                .sections([
                    vec![source],
                    values.iter().map(|(label, _)| *label).collect(),
                ])
                .on_change(move |index| {
                    change(index.checked_sub(1).map(|index| choices[index].1.clone()))
                }),
        );
        input.accessibility_label(title);
        let effective = Rc::new(Caption::new(&self.ui, ""));
        effective.ns_text_field().setAlignment(objc2_app_kit::NSTextAlignment::Right);
        let control = VStack::new(&self.ui)
            .spacing(3.0)
            .push(HStack::new(&self.ui).push(Spacer::new(&self.ui)).push(input.clone()))
            .push(effective.clone());
        let followed = Rc::new(followed);
        let caption = Rc::downgrade(&effective);
        self.sync_when(
            &input,
            {
                let get = get.clone();
                let followed = followed.clone();
                move |s| (get(s), followed(s))
            },
            move |input, s| {
                let followed = followed(s);
                let position = |value: &T| values.iter().position(|(_, v)| v == value);
                for index in 0..values.len() {
                    input.set_item_badge(
                        index + 1,
                        (position(&followed) == Some(index)).then_some(badge),
                    );
                }
                let current = get(s);
                input.set_selected(current.as_ref().and_then(position).map_or(0, |i| i + 1));
                if let Some(caption) = caption.upgrade() {
                    let label = position(&followed).map_or("", |i| values[i].0);
                    caption.set_text(&describe(label));
                    caption.set_hidden(current.is_some());
                }
            },
        );
        self.row(title, control, message).label_baseline(input.ns_view())
    }

    fn inherited_popup<T: Clone + PartialEq + Send + 'static>(
        &mut self,
        title: &str,
        values: Vec<(&str, T)>,
        get: impl Fn(&ConfigSource) -> Option<T> + 'static,
        default: impl Fn(&ConfigSource) -> T + 'static,
        set: impl Fn(&mut ConfigSource, Option<T>) + Send + Clone + 'static,
    ) -> SettingsRow {
        let message = Rc::new(ValidationMessage::new(&self.ui));
        let error = Rc::downgrade(&message);
        let model = self.model.clone();
        let get = Rc::new(get);
        let default = Rc::new(default);
        let current = get.clone();
        let items: Rc<[T]> = values.iter().map(|(_, value)| value.clone()).collect();
        let labels: Vec<String> = values.iter().map(|(label, _)| (*label).into()).collect();
        let choices = items.clone();
        let inherited = default.clone();
        let input = Rc::new(Popup::new(&self.ui).on_change(move |index| {
            let Some(choice) = choices.get(index).cloned() else {
                return;
            };
            let Some(owner) = model.upgrade() else {
                return;
            };
            let value = {
                let source = owner.source.borrow();
                let value = (choice != inherited(&source)).then_some(choice);
                if owner.is_draft() && current(&source) == value {
                    return;
                }
                value
            };
            let set = set.clone();
            Model::submit(
                &model,
                Box::new(move |source| {
                    set(source, value);
                    Ok(())
                }),
                error.clone(),
            );
        }));
        self.sync_when(
            &input,
            {
                let get = get.clone();
                let default = default.clone();
                move |s| {
                    let inherited = default(s);
                    (get(s).unwrap_or_else(|| inherited.clone()), inherited)
                }
            },
            move |input, source| {
                input.set_items(labels.iter().map(String::as_str));
                let inherited = default(source);
                for (index, item) in items.iter().enumerate() {
                    input.set_item_badge(index, (*item == inherited).then_some("Default"));
                }
                let effective = get(source).unwrap_or(inherited);
                input.set_selected(items.iter().position(|item| *item == effective).unwrap_or(0));
            },
        );
        self.row(title, input, message)
    }

    fn enabled(&mut self, row: &SettingsRow, enabled: impl Fn(&ConfigSource) -> bool + 'static) {
        let view = row.control();
        let last = Cell::new(None);
        self.sync.push(Box::new(move |s| {
            let value = enabled(s);
            if last.replace(Some(value)) != Some(value) {
                view.set_enabled(value);
            }
        }));
    }
}

#[cfg(test)]
pub mod native_tests;
