//! Lazy main-thread Settings composition. ConfigActor owns all mutations and persistence.
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};

use cgs::*;
use rift_protocol::ApplicationData;
use tokio::sync::mpsc::UnboundedSender;

use crate::actor::config::SourceEdit;
use crate::common::config::{ConfigSchema, ConfigSource};
use crate::sys::screen::ScreenInfo;

mod applications;
mod commands;
mod editors;
mod pages;
mod schema;
mod search;
pub(crate) mod updates;

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
}

/// Sidebar categories; a category's index is its page id.
pub(super) const CATEGORIES: [(&str, &str); 9] = [
    ("General", "gearshape"),
    ("Layouts", "rectangle.split.2x2"),
    ("Workspaces", "square.grid.2x2"),
    ("Rules", "line.3.horizontal.decrease"),
    ("Keyboard", "keyboard"),
    ("Mouse & Trackpad", "computermouse"),
    ("Interface", "macwindow"),
    ("Advanced", "slider.horizontal.3"),
    ("About", "info.circle"),
];
const LAYOUTS: usize = 1;
/// Layout scopes follow the categories in the page table.
const SCOPES: usize = CATEGORIES.len();
const PAGE_COUNT: usize = SCOPES + pages::LAYOUT_PAGES.len();

fn scope_page(scope: usize) -> usize { SCOPES + scope }

fn category(id: usize) -> usize { if id >= SCOPES { LAYOUTS } else { id } }

/// Show an edit's outcome under the control that made it.
fn report(message: &Weak<ValidationMessage>, result: &Result<(), String>) {
    if let Some(message) = message.upgrade() {
        message.set_validation(&match result {
            Ok(()) => Validation::None,
            Err(error) => Validation::Error(error.clone()),
        });
    }
}

type Navigate = Rc<dyn Fn(usize)>;
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
    navigate: RefCell<Option<Navigate>>,
}

impl Env {
    fn send(&self, action: Action, finish: Finish) {
        let _ = self.requests.send(Request { action, finish });
    }

    fn navigate(&self, id: usize) {
        let navigate = self.navigate.borrow().clone();
        if let Some(navigate) = navigate {
            navigate(id);
        }
    }

    fn rebuild_applications(&self) {
        *self.inventory.borrow_mut() = applications::Inventory::new(
            &self.applications.borrow(),
            self.installed_applications.borrow().as_deref().unwrap_or(&[]),
        );
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
    base: ConfigSource,
    page: RefCell<Option<Rc<Page>>>,
    changed: RefCell<Option<DraftChanged>>,
}

struct Model {
    env: Rc<Env>,
    source: RefCell<ConfigSource>,
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
    fn new(env: Rc<Env>, source: ConfigSource, draft: Option<Draft>) -> Rc<Self> {
        Rc::new(Self {
            env,
            source: RefCell::new(source),
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
        let mut source = self.source.borrow().clone();
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

    fn replace_source(&self, source: ConfigSource) {
        if *self.source.borrow() != source {
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
        for sync in &self.sync {
            sync(&model.source.borrow());
        }
        model.syncing.set(syncing);
        self.synced_revision.set(Some(revision));
    }
}

struct NavigationState {
    history: Vec<usize>,
    cursor: usize,
}
impl NavigationState {
    fn current(&self) -> usize { self.history[self.cursor] }

    fn select(&mut self, page: usize) {
        if self.current() != page {
            self.history.truncate(self.cursor + 1);
            self.history.push(page);
            self.cursor += 1;
        }
    }

    fn step(&mut self, forward: bool) -> Option<usize> {
        let next = if forward {
            self.cursor + 1
        } else {
            self.cursor.checked_sub(1)?
        };
        let page = *self.history.get(next)?;
        self.cursor = next;
        Some(page)
    }
}

/// Page cache, history, and the window chrome that follows the current page.
struct Router {
    ui: Ui,
    model: Rc<Model>,
    host: Rc<PageHost>,
    /// Built lazily and kept while the window is open.
    pages: RefCell<Vec<Option<Page>>>,
    history: RefCell<NavigationState>,
    title: Rc<Label>,
    toolbar: RefCell<Weak<Toolbar>>,
    sidebar: RefCell<Weak<Sidebar<usize>>>,
    /// Set while the router itself moves the sidebar selection.
    routing: Cell<bool>,
}

impl Router {
    fn current(&self) -> usize { self.history.borrow().current() }

    fn navigate(&self, id: usize) {
        self.model.close_sheet();
        self.history.borrow_mut().select(id);
        self.routing.set(true);
        if let Some(sidebar) = self.sidebar.borrow().upgrade() {
            sidebar.set_selected(category(id));
        }
        self.routing.set(false);
        self.show(id);
        if let Some(toolbar) = self.toolbar.borrow().upgrade() {
            let history = self.history.borrow();
            toolbar.update_navigation(
                history.cursor > 0,
                history.cursor + 1 < history.history.len(),
                if id >= SCOPES {
                    pages::LAYOUT_PAGES[id - SCOPES].0
                } else {
                    "All"
                },
                category(id) == LAYOUTS,
            );
        }
    }

    fn step(&self, forward: bool) {
        let id = self.history.borrow_mut().step(forward);
        if let Some(id) = id {
            self.navigate(id);
        }
    }

    fn show(&self, id: usize) {
        self.title.set_text(CATEGORIES[category(id)].0);
        if self.pages.borrow()[id].is_none() {
            let page = if id >= SCOPES {
                pages::layout_scope(self.ui, &self.model, id - SCOPES)
            } else {
                pages::build(self.ui, &self.model, id)
            };
            self.pages.borrow_mut()[id] = Some(page);
        }
        let pages = self.pages.borrow();
        let page = pages[id].as_ref().unwrap();
        page.synchronize(&self.model);
        self.host.set_cached_page(page.view.clone());
        if let Some(toolbar) = self.toolbar.borrow().upgrade() {
            toolbar.set_page_controls(&self.ui, page.header.as_deref());
        }
    }
}

pub struct Settings {
    window: SettingsWindow,
    router: Rc<Router>,
}

impl Settings {
    pub fn new(
        ui: Ui,
        source: ConfigSource,
        config_path: PathBuf,
        displays: Vec<ScreenInfo>,
        applications: Vec<ApplicationData>,
        requests: UnboundedSender<Request>,
        on_close: impl FnMut() + 'static,
    ) -> Self {
        let env = Rc::new(Env {
            requests,
            window: RefCell::new(WindowRef::default()),
            displays: RefCell::new(displays),
            config_path,
            applications: RefCell::new(applications),
            installed_applications: RefCell::new(None),
            inventory: RefCell::default(),
            navigate: RefCell::new(None),
        });
        env.rebuild_applications();
        let host = Rc::new(PageHost::new(&ui));
        let router = Rc::new(Router {
            ui,
            model: Model::new(env.clone(), source, None),
            host: host.clone(),
            pages: RefCell::new((0..PAGE_COUNT).map(|_| None).collect()),
            history: RefCell::new(NavigationState { history: vec![0], cursor: 0 }),
            title: Rc::new(Label::new(&ui, CATEGORIES[0].0)),
            toolbar: RefCell::new(Weak::new()),
            sidebar: RefCell::new(Weak::new()),
            routing: Cell::new(false),
        });
        let navigate: Rc<dyn Fn(usize)> = {
            let router = Rc::downgrade(&router);
            Rc::new(move |id| {
                if let Some(router) = router.upgrade() {
                    router.navigate(id);
                }
            })
        };
        *env.navigate.borrow_mut() = Some(navigate.clone());

        let entries = CATEGORIES
            .iter()
            .enumerate()
            .map(|(id, (title, symbol))| SidebarItem {
                id,
                title: (*title).into(),
                symbol: (*symbol).into(),
            })
            .collect();
        let weak_router = Rc::downgrade(&router);
        let sidebar = Rc::new(Sidebar::new(&ui, entries).on_select(move |id| {
            if let Some(router) = weak_router.upgrade()
                && !router.routing.get()
            {
                router.navigate(id);
            }
        }));
        sidebar.set_selected(0);
        *router.sidebar.borrow_mut() = Rc::downgrade(&sidebar);

        let window = SettingsWindow::new(&ui, "Rift Settings")
            .page_title(&ui, router.title.clone())
            .on_close(on_close)
            .content(NavigationSplitView::new(
                &ui,
                sidebar_pane(ui, &router, sidebar.clone()),
                host,
            ));
        window.initial_focus(&*sidebar);
        let menu = Rc::new(Menu::new(&ui));
        let route = navigate.clone();
        menu.add(MenuItem::new(&ui, "All").tag(LAYOUTS as isize).on_click(move || route(LAYOUTS)));
        menu.add_separator();
        for (title, range) in pages::LAYOUT_SECTIONS {
            menu.add_section_header(title);
            for scope in range {
                let route = navigate.clone();
                let id = scope_page(scope);
                let item = MenuItem::new(&ui, pages::LAYOUT_PAGES[scope].0).tag(id as isize);
                menu.add(item.on_click(move || route(id)));
            }
        }
        let (back, forward) = (Rc::downgrade(&router), Rc::downgrade(&router));
        window.toolbar().set_navigation(
            &ui,
            move || {
                if let Some(router) = back.upgrade() {
                    router.step(false);
                }
            },
            move || {
                if let Some(router) = forward.upgrade() {
                    router.step(true);
                }
            },
            menu,
        );
        window.toolbar().update_navigation(false, false, CATEGORIES[0].0, false);
        *router.toolbar.borrow_mut() = Rc::downgrade(window.toolbar());
        *env.window.borrow_mut() = window.handle();
        window.set_size_centered(CGSize::new(880.0, 660.0));
        router.show(0);
        Self { window, router }
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
        // Layout scopes read the display list; parked copies unmount once replaced.
        self.router.pages.borrow_mut()[SCOPES..].fill_with(|| None);
        let id = self.router.current();
        if id >= SCOPES {
            self.router.show(id);
        }
    }

    pub fn show(&self) { self.window.present(); }

    /// Updates the current page even while hidden or minimized; other pages sync when shown.
    pub fn synchronize(&self, source: ConfigSource) {
        self.model().replace_source(source);
        if let Some(page) = &self.router.pages.borrow()[self.router.current()] {
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
    }
}

/// The sidebar, replaced by search results while a query is entered.
fn sidebar_pane(ui: Ui, router: &Rc<Router>, sidebar: Rc<Sidebar<usize>>) -> View {
    let content = Rc::new(PageHost::new(&ui));
    content.set_cached_page(sidebar.clone());
    let weak_router = Rc::downgrade(router);
    let open = Rc::new(move |entry: &'static search::Entry| {
        if let Some(router) = weak_router.upgrade() {
            router.navigate(entry.page_id());
            router.host.reveal(search::section(&entry.location), entry.title);
        }
    });
    let pick = open.clone();
    let results = Rc::new(
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
    );
    let empty = Rc::new(
        VStack::new(&ui)
            .insets(uniform_insets(12.0))
            .push(Caption::new(&ui, "No matching settings")),
    );
    let first = Rc::new(Cell::new(None::<&'static search::Entry>));
    let top = first.clone();
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
                    empty.clone()
                });
                return;
            }
            results.set_rows(search::grouped(&hits));
            results.scroll_to(0);
            content.set_cached_page(results.clone());
        })
        .on_commit(move |_| {
            if let Some(entry) = first.get() {
                open(entry);
            }
        });
    let pane = VStack::new(&ui)
        .spacing(8.0)
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
    gap_preview: Option<Preview>,
}
impl FormBuilder {
    fn new(ui: Ui, model: &Rc<Model>) -> Self {
        Self {
            ui,
            model: Rc::downgrade(model),
            draft: model.is_draft(),
            sync: Vec::new(),
            gap_preview: None,
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

    /// Submit `value` unless the source already holds it.
    fn change<T: PartialEq + Send + 'static>(
        &self,
        get: impl Fn(&ConfigSource) -> T + 'static,
        set: impl Fn(&mut ConfigSource, T) -> Result<(), String> + Send + Clone + 'static,
        error: Weak<ValidationMessage>,
    ) -> impl Fn(T) + Clone + 'static {
        let model = self.model.clone();
        let get = Rc::new(get);
        move |value| {
            if model.upgrade().is_some_and(|model| get(&model.source.borrow()) == value) {
                return;
            }
            let set = set.clone();
            Model::submit(&model, Box::new(move |source| set(source, value)), error.clone());
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
        self.sync(&input, move |input, s| input.set_value(get(s)));
        self.row(title, input, message)
    }

    fn gap(
        &mut self,
        title: &str,
        get: impl Fn(&ConfigSource) -> f64 + 'static,
        set: impl Fn(&mut ConfigSource, f64) + Send + Clone + 'static,
    ) -> SettingsRow {
        self.numeric(title, 1.0, false, Some(Rc::new(|_| (0.0, 100.0))), get, set)
    }

    fn percentage(
        &mut self,
        title: &str,
        range: impl Fn(&ConfigSource) -> (f64, f64) + 'static,
        get: impl Fn(&ConfigSource) -> f64 + 'static,
        set: impl Fn(&mut ConfigSource, f64) + Send + Clone + 'static,
    ) -> SettingsRow {
        self.numeric(title, 100.0, false, Some(Rc::new(range)), get, set)
    }

    fn numeric(
        &mut self,
        title: &str,
        scale: f64,
        integer: bool,
        slider_range: Option<SliderRange>,
        get: impl Fn(&ConfigSource) -> f64 + 'static,
        set: impl Fn(&mut ConfigSource, f64) + Send + Clone + 'static,
    ) -> SettingsRow {
        let (slider, input, message) =
            self.numeric_controls(title, scale, integer, slider_range, get, set);
        if let Some(slider) = slider {
            slider.width(120.0);
            input.width(60.0);
            self.row(
                title,
                HStack::new(&self.ui).spacing(8.0).push(slider).push(input),
                message,
            )
        } else {
            self.row(title, input, message)
        }
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
            let preview = self.gap_preview.clone();
            let preview_input = Rc::downgrade(&input);
            let mut local_preview = None::<ConfigSource>;
            let app = Application::shared(&self.ui);
            let slider = Slider::new(&self.ui).range(min, max).continuous(preview.is_some());
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
                    let local = local_preview.get_or_insert_with(|| model.source.borrow().clone());
                    edit_preview(local, value / scale);
                    preview(local);
                    if let Some(input) = preview_input.upgrade() {
                        input.set_value(value);
                    }
                    // Preview while dragging; commit once on release.
                    if app.is_mouse_dragging() {
                        return;
                    }
                }
                local_preview = None;
                commit(value);
            }));
            slider.accessibility_label(title);
            slider
        });
        input.accessibility_label(title);
        input.min_width(60.0);
        let weak = Rc::downgrade(&input);
        let weak_slider = slider.as_ref().map(Rc::downgrade);
        self.sync.push(Box::new(move |s| {
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

    fn gap_cells(
        &mut self,
        title: &str,
        get: impl Fn(&ConfigSource) -> f64 + 'static,
        set: impl Fn(&mut ConfigSource, f64) + Send + Clone + 'static,
    ) -> Vec<Box<dyn NativeView>> {
        let (slider, input, message) =
            self.numeric_controls(title, 1.0, true, Some(Rc::new(|_| (0.0, 100.0))), get, set);
        let slider = slider.unwrap();
        slider.width(120.0);
        input.width(44.0);
        let value_field = input.unit_field(&self.ui, "pt");
        value_field.width(82.0);
        // The bezel hosts the field's view; keep its Rust delegate alive with the cell.
        let value = VStack::new(&self.ui).spacing(2.0).push(value_field).push(message).keep(input);
        vec![
            Box::new(Label::new(&self.ui, title)),
            Box::new(slider),
            Box::new(value),
        ]
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
        input.width(220.0);
        self.sync(&input, move |input, s| input.set_value(&get(s)));
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
        self.sync(&input, move |input, s| {
            input.set_selected(items.iter().position(|v| *v == get(s)).unwrap_or(0));
        });
        self.row(title, input, message)
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
                if current(&source) == value {
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
        self.sync(&input, move |input, source| {
            input.set_items(labels.iter().map(String::as_str));
            let inherited = default(source);
            for (index, item) in items.iter().enumerate() {
                input.set_item_badge(index, (*item == inherited).then_some("Default"));
            }
            let effective = get(source).unwrap_or(inherited);
            input.set_selected(items.iter().position(|item| *item == effective).unwrap_or(0));
        });
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

#[cfg(test)]
mod navigation_tests {
    use super::*;
    #[test]
    fn history_revisits_pages_and_discards_forward_branch_on_new_selection() {
        let mut history = NavigationState { history: vec![0], cursor: 0 };
        assert_eq!(history.step(false), None);
        history.select(1);
        history.select(4);
        assert_eq!(history.step(false), Some(1));
        history.select(1); // Replaying a sidebar selection adds no entry.
        assert_eq!(history.step(true), Some(4));
        assert_eq!(history.step(true), None);
        assert_eq!(history.step(false), Some(1));
        history.select(3);
        assert_eq!(history.step(true), None);
        assert_eq!(history.step(false), Some(1));
        assert_eq!(history.step(false), Some(0));
        assert_eq!(history.step(false), None);
    }
}
