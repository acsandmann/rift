//! Lazy main-thread Settings composition. ConfigActor owns all mutations and persistence.
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use cgs::*;
use objc2::MainThreadOnly;
use objc2_app_kit::NSApplication;
use tokio::sync::mpsc::UnboundedSender;

use crate::actor::config::SourceEdit;
use crate::common::config::ConfigSource;

mod applications;
mod commands;
mod editors;
mod pages;
mod schema;
mod search;
pub(crate) mod updates;

pub enum Action {
    Edit(SourceEdit),
    Reload,
    RefreshRuntime,
    CheckUpdates(updates::Completion),
}
pub struct Request {
    pub action: Action,
    pub finish: Box<dyn FnOnce(Result<(), String>)>,
}

struct Model {
    source: RefCell<ConfigSource>,
    source_revision: Cell<u64>,
    requests: UnboundedSender<Request>,
    syncing: Cell<bool>,
    sheet: RefCell<Option<Sheet>>,
    sheet_page: RefCell<Option<Rc<Page>>>,
    sheet_model: RefCell<Option<Rc<Model>>>,
    draft_changed: RefCell<Option<Box<dyn Fn(bool)>>>,
    draft_base: Option<ConfigSource>,
    window: RefCell<objc2::rc::Weak<objc2_app_kit::NSWindow>>,
    displays: RefCell<Vec<crate::sys::screen::ScreenInfo>>,
    config_path: std::path::PathBuf,
    applications: RefCell<Vec<rift_protocol::ApplicationData>>,
    installed_applications: RefCell<Option<Vec<(String, String)>>>,
    application_inventory: RefCell<Vec<applications::Choice>>,
    page_title: Rc<Label>,
    navigate: RefCell<Option<Rc<dyn Fn(usize)>>>,
    toolbar: RefCell<Weak<Toolbar>>,
}

type SyncControl = Box<dyn Fn(&ConfigSource)>;
pub(super) struct Page {
    view: Rc<dyn NativeView>,
    sync: Vec<SyncControl>,
    synced_revision: Cell<Option<u64>>,
}

impl Model {
    fn submit(model: &Weak<Self>, edit: SourceEdit, error: Weak<ValidationMessage>) {
        if let Some(model) = model.upgrade() {
            model.submit_edit(
                edit,
                Box::new(move |result| {
                    if let Some(label) = error.upgrade() {
                        label.set_validation(
                            &result.map_or_else(Validation::Error, |_| Validation::None),
                        );
                    }
                }),
            );
        }
    }

    fn close_sheet(&self) {
        self.sheet.borrow_mut().take();
        self.sheet_page.borrow_mut().take();
        self.sheet_model.borrow_mut().take();
    }

    fn draft(&self) -> Rc<Self> {
        let base = self.source.borrow().clone();
        let draft = Rc::new(Self {
            source: RefCell::new(base.clone()),
            source_revision: Cell::new(0),
            requests: self.requests.clone(),
            syncing: Cell::new(false),
            sheet: RefCell::new(None),
            sheet_page: RefCell::new(None),
            sheet_model: RefCell::new(None),
            draft_changed: RefCell::new(None),
            draft_base: Some(base),
            window: RefCell::new(self.window.borrow().clone()),
            displays: RefCell::new(self.displays.borrow().clone()),
            config_path: self.config_path.clone(),
            applications: RefCell::new(self.applications.borrow().clone()),
            installed_applications: RefCell::new(self.installed_applications.borrow().clone()),
            application_inventory: RefCell::new(Vec::new()),
            page_title: self.page_title.clone(),
            navigate: RefCell::new(None),
            toolbar: RefCell::new(Weak::new()),
        });
        draft.rebuild_applications();
        draft
    }

    fn submit_edit(&self, edit: SourceEdit, finish: Box<dyn FnOnce(Result<(), String>)>) {
        if self.syncing.get() {
            return;
        }
        if let Some(base) = &self.draft_base {
            let mut source = self.source.borrow().clone();
            let result = edit(&mut source);
            if result.is_ok() {
                self.replace_source(source);
                if let Some(page) = self.sheet_page.borrow().as_ref() {
                    page.synchronize(self);
                }
                if let Some(changed) = self.draft_changed.borrow().as_ref() {
                    changed(*self.source.borrow() != *base);
                }
            }
            finish(result);
        } else {
            let _ = self.requests.send(Request {
                action: Action::Edit(edit),
                finish,
            });
        }
    }

    fn rebuild_applications(&self) {
        *self.application_inventory.borrow_mut() = applications::inventory(
            &self.applications.borrow(),
            self.installed_applications.borrow().as_deref().unwrap_or(&[]),
        );
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
    fn select(&mut self, page: usize) {
        if self.history[self.cursor] != page {
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

pub struct Settings {
    window: SettingsWindow,
    model: Rc<Model>,
    _host: Rc<PageHost>,
    pages: Rc<RefCell<Vec<Option<Page>>>>,
    history: Rc<RefCell<NavigationState>>,
}

impl Settings {
    pub fn new(
        ui: Ui,
        source: ConfigSource,
        config_path: std::path::PathBuf,
        displays: Vec<crate::sys::screen::ScreenInfo>,
        applications: Vec<rift_protocol::ApplicationData>,
        requests: UnboundedSender<Request>,
        on_close: impl FnMut() + 'static,
    ) -> Self {
        let model = Rc::new(Model {
            source: RefCell::new(source),
            source_revision: Cell::new(0),
            requests,
            syncing: Cell::new(false),
            sheet: RefCell::new(None),
            sheet_page: RefCell::new(None),
            sheet_model: RefCell::new(None),
            draft_changed: RefCell::new(None),
            draft_base: None,
            window: RefCell::new(objc2::rc::Weak::default()),
            displays: RefCell::new(displays),
            config_path,
            applications: RefCell::new(applications),
            installed_applications: RefCell::new(None),
            application_inventory: RefCell::new(Vec::new()),
            page_title: Rc::new(Label::new(&ui, "General")),
            navigate: RefCell::new(None),
            toolbar: RefCell::new(Weak::new()),
        });
        model.rebuild_applications();
        let host = Rc::new(PageHost::new(&ui));
        let pages = Rc::new(RefCell::new((0..9).map(|_| None::<Page>).collect::<Vec<_>>()));
        let weak_model = Rc::downgrade(&model);
        let weak_host = Rc::downgrade(&host);
        let weak_pages = Rc::downgrade(&pages);
        let entries: Vec<_> = [
            ("General", "gearshape"),
            ("Layouts", "rectangle.split.2x2"),
            ("Workspaces", "square.grid.2x2"),
            ("Rules", "line.3.horizontal.decrease"),
            ("Keyboard", "keyboard"),
            ("Mouse & Trackpad", "computermouse"),
            ("Interface", "macwindow"),
            ("Advanced", "slider.horizontal.3"),
            ("About", "info.circle"),
        ]
        .into_iter()
        .enumerate()
        .map(|(id, (title, symbol))| SidebarItem {
            id,
            title: title.into(),
            symbol: symbol.into(),
        })
        .collect();
        let history = Rc::new(RefCell::new(NavigationState { history: vec![0], cursor: 0 }));
        let sidebar_slot = Rc::new(RefCell::new(Weak::<Sidebar<usize>>::new()));
        let routing = Rc::new(Cell::new(false));
        let history_router = history.clone();
        let sidebar_router = sidebar_slot.clone();
        let routing_router = routing.clone();
        let navigate: Rc<dyn Fn(usize)> = Rc::new(move |id| {
            let (Some(model), Some(host), Some(pages)) =
                (weak_model.upgrade(), weak_host.upgrade(), weak_pages.upgrade()) else { return; };
            let previous = { let history = history_router.borrow(); history.history[history.cursor] };
            history_router.borrow_mut().select(id);
            let category = if id >= 9 { 1 } else { id };
            routing_router.set(true);
            if let Some(sidebar) = sidebar_router.borrow().upgrade() { sidebar.set_selected(category); }
            routing_router.set(false);
            if previous >= 9 || id == 1 || id >= 9 { pages.borrow_mut()[1] = None; }
            Self::select(ui, &model, &host, &pages, id);
            if let Some(toolbar) = model.toolbar.borrow().upgrade() {
                let history = history_router.borrow();
                toolbar.update_navigation(history.cursor > 0, history.cursor + 1 < history.history.len(),
                    if id >= 9 { pages::LAYOUT_PAGES[id - 9].0 } else { "All" }, id == 1 || id >= 9);
            }
        });
        *model.navigate.borrow_mut() = Some(navigate.clone());
        let route = navigate.clone();
        let sidebar = Sidebar::new(&ui, entries.clone()).on_select(move |id| {
            if !routing.get() { route(id); }
        });
        let sidebar = Rc::new(sidebar);
        sidebar.set_selected(0);
        *sidebar_slot.borrow_mut() = Rc::downgrade(&sidebar);
        let found = Rc::new(RefCell::new(Vec::<search::Result>::new()));
        let destinations = found.clone();
        let opening_destinations = destinations.clone();
        let weak_model = Rc::downgrade(&model);
        let weak_host = Rc::downgrade(&host);
        let open_result = Rc::new(RefCell::new(move |index: usize| {
            let Some(destination) = opening_destinations.borrow().get(index).cloned() else {
                return;
            };
            if let (Some(model), Some(host)) = (weak_model.upgrade(), weak_host.upgrade()) {
                let id = destination.scope.map_or(destination.page, |scope| 9 + scope);
                if let Some(navigate) = model.navigate.borrow().as_ref() { navigate(id); }
                host.ns_view().layoutSubtreeIfNeeded();
                search::reveal(host.ns_view(), destination.title, destination.location);
            }
        }));
        let open_row = open_result.clone();
        let search_rows = Rc::new(RefCell::new(Vec::<search::Row>::new()));
        let selected_rows = search_rows.clone();
        let destinations_for_selection = destinations.clone();
        let results = Rc::new(
            Table::new(&ui)
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
                        search::Row::Setting(result) => {
                            let description = Caption::new(&ui, &result.description());
                            description.ns_text_field().setMaximumNumberOfLines(2);
                            let title = Label::new(&ui, result.title);
                            Box::new(content.push(title).push(description))
                        }
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
                .on_select(move |index| {
                    let destination =
                        index.and_then(|index| selected_rows.borrow().get(index).cloned());
                    if let Some(search::Row::Setting(result)) = destination {
                        let index = destinations_for_selection.borrow().iter().position(|entry| {
                            entry.title == result.title
                                && entry.page == result.page
                                && entry.scope == result.scope
                                && entry.location == result.location
                        });
                        if let Some(index) = index {
                            (open_row.borrow_mut())(index);
                        }
                    }
                }),
        );
        results.ns_table_view().setHeaderView(None);
        results.ns_table_view().setFloatsGroupRows(false);
        results.ns_table_view().setStyle(objc2_app_kit::NSTableViewStyle::SourceList);
        results.ns_scroll_view().setDrawsBackground(false);
        let sidebar_content = Rc::new(PageHost::new(&ui));
        sidebar_content.set_cached_page(sidebar.clone());
        let weak_sidebar_content = Rc::downgrade(&sidebar_content);
        let normal_sidebar = sidebar.clone();
        let empty = Rc::new(
            VStack::new(&ui)
                .insets(Insets {
                    top: 12.0,
                    left: 12.0,
                    bottom: 12.0,
                    right: 12.0,
                })
                .push(Caption::new(&ui, "No matching settings")),
        );
        let search = Rc::new(
            SearchField::new(&ui)
                .placeholder("Search")
                .on_change(move |query| {
                    let Some(sidebar_content) = weak_sidebar_content.upgrade() else {
                        return;
                    };
                    if query.trim().is_empty() {
                        sidebar_content.set_cached_page(normal_sidebar.clone());
                        return;
                    }
                    let matches = search::results(&query);
                    *found.borrow_mut() = matches.clone();
                    let rows = search::grouped(&matches);
                    *search_rows.borrow_mut() = rows.clone();
                    results.set_rows(rows);
                    if !found.borrow().is_empty() {
                        results.ns_table_view().scrollRowToVisible(0);
                    }
                    if found.borrow().is_empty() {
                        sidebar_content.set_cached_page(empty.clone());
                    } else {
                        sidebar_content.set_cached_page(results.clone());
                    }
                })
                .on_commit(move |query| {
                    if !query.trim().is_empty() {
                        (open_result.borrow_mut())(0);
                    }
                }),
        );
        let sidebar_pane = VStack::new(&ui)
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
            .push(sidebar_content);
        let sidebar_pane = View::new(&ui).safe_area_content(sidebar_pane);
        let window = SettingsWindow::new(&ui, "Rift Settings")
            .page_title(&ui, model.page_title.clone())
            .on_close(on_close)
            .content(NavigationSplitView::new(&ui, sidebar_pane, host.clone()));
        window.ns_window().setInitialFirstResponder(Some(sidebar.ns_table_view()));
        let menu = Rc::new(Menu::new(&ui));
        let route = navigate.clone();
        let overview = MenuItem::new(&ui, "All").on_click(move || route(1));
        overview.ns_menu_item().setTag(1);
        menu.add(overview);
        menu.add_separator();
        for (title, range) in [("Default", 0..1), ("Layouts", 1..7), ("Global", 7..9)] {
            menu.ns_menu().addItem(&objc2_app_kit::NSMenuItem::sectionHeaderWithTitle(
                &objc2_foundation::NSString::from_str(title), ui.mtm()));
            for index in range {
                let route = navigate.clone();
                let item = MenuItem::new(&ui, pages::LAYOUT_PAGES[index].0).on_click(move || route(9 + index));
                item.ns_menu_item().setTag((9 + index) as isize);
                menu.add(item);
            }
        }
        let history_back = history.clone();
        let route_back = navigate.clone();
        let route_forward = navigate.clone();
        let history_forward = history.clone();
        window.toolbar().set_navigation(&ui, move || {
            let id = history_back.borrow_mut().step(false);
            if let Some(id) = id { route_back(id); }
        }, move || {
            let id = history_forward.borrow_mut().step(true);
            if let Some(id) = id { route_forward(id); }
        }, menu);
        window.toolbar().update_navigation(false, false, "General", false);
        *model.toolbar.borrow_mut() = Rc::downgrade(window.toolbar());
        *model.window.borrow_mut() = objc2::rc::Weak::new(window.ns_window());
        window.ns_window().setContentSize(CGSize::new(880.0, 660.0));
        window.ns_window().center();
        Self::select(ui, &model, &host, &pages, 0);
        Self {
            window,
            model,
            _host: host,
            pages,
            history,
        }
    }

    fn select(
        ui: Ui,
        model: &Rc<Model>,
        host: &Rc<PageHost>,
        pages: &Rc<RefCell<Vec<Option<Page>>>>,
        id: usize,
    ) {
        let category = if id >= 9 { 1 } else { id };
        let title = if id >= 9 { pages::LAYOUT_PAGES[id - 9].0 } else {
            ["General", "Layouts", "Workspaces", "Rules", "Keyboard", "Mouse & Trackpad", "Interface", "Advanced", "About"][id]
        };
        model.page_title.set_text(if id >= 9 { "Layouts" } else { title });
        if id >= 9 {
            pages.borrow_mut()[1] = Some(pages::layout_scope(ui, model, id - 9));
        } else if pages.borrow()[category].is_none() {
            pages.borrow_mut()[category] = Some(pages::build(ui, model, category));
        }
        let pages = pages.borrow();
        let page = pages[category].as_ref().unwrap();
        page.synchronize(model);
        host.set_cached_page(page.view.clone());
    }

    pub async fn refresh_installed_applications(&self) {
        if self.model.installed_applications.borrow().is_some() {
            return;
        }
        let (send, receive) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            let _ = send.send(applications::installed());
        });
        if let Ok(apps) = receive.await {
            *self.model.installed_applications.borrow_mut() = Some(apps);
            self.model.rebuild_applications();
        }
    }

    pub fn refresh_applications(&self, applications: Vec<rift_protocol::ApplicationData>) {
        let changed = self
            .model
            .applications
            .borrow()
            .iter()
            .filter(|app| app.window_count > 0)
            .map(|app| (&app.name, &app.bundle_id))
            .ne(applications
                .iter()
                .filter(|app| app.window_count > 0)
                .map(|app| (&app.name, &app.bundle_id)));
        *self.model.applications.borrow_mut() = applications;
        if changed {
            self.model.rebuild_applications();
        }
    }

    pub fn refresh_displays(&self, displays: Vec<crate::sys::screen::ScreenInfo>) {
        if *self.model.displays.borrow() != displays {
            *self.model.displays.borrow_mut() = displays;
            self.pages.borrow_mut()[1] = None;
            let id = self.history.borrow().history[self.history.borrow().cursor];
            if id != 1 && id < 9 {
                return;
            }
            self._host.clear();
            Self::select(
                Ui::new(self.window.ns_window().mtm()),
                &self.model,
                &self._host,
                &self.pages,
                id,
            );
        }
    }

    pub fn show(&self) {
        // Accessory applications need activation to accept keyboard input.
        #[allow(deprecated)]
        NSApplication::sharedApplication(self.window.ns_window().mtm())
            .activateIgnoringOtherApps(true);
        self.window.show();
    }

    pub fn visible(&self) -> bool { self.window.ns_window().isVisible() }

    pub fn synchronize(&self, source: ConfigSource) {
        self.model.replace_source(source);
        if self.visible() {
            if let Some(page) = self.model.sheet_page.borrow().as_ref() {
                if let Some(draft) = self.model.sheet_model.borrow().as_ref() {
                    page.synchronize(draft);
                }
            }
            let id = self.history.borrow().history[self.history.borrow().cursor];
            if let Some(page) = &self.pages.borrow()[if id >= 9 { 1 } else { id }] {
                page.synchronize(&self.model);
            }
        }
    }
}

impl Drop for Settings {
    fn drop(&mut self) {
        // End the sheet while its weak parent still points to the live Settings window.
        self.model.close_sheet();
        self._host.clear();
        self.pages.borrow_mut().clear();
    }
}

pub(super) struct FormBuilder {
    ui: Ui,
    model: Weak<Model>,
    sync: Vec<SyncControl>,
    gap_preview: Option<Rc<dyn Fn(&ConfigSource)>>,
}
impl FormBuilder {
    fn new(ui: Ui, model: &Rc<Model>) -> Self {
        Self {
            ui,
            model: Rc::downgrade(model),
            sync: Vec::new(),
            gap_preview: None,
        }
    }

    fn finish(self, view: impl NativeView) -> Page {
        Page {
            view: Rc::new(view),
            sync: self.sync,
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

    fn change<T: PartialEq + Send + 'static>(
        &self,
        get: impl Fn(&ConfigSource) -> T + Clone + 'static,
        set: impl Fn(&mut ConfigSource, T) -> Result<(), String> + Send + Clone + 'static,
        error: Weak<ValidationMessage>,
    ) -> impl Fn(T) + Clone + 'static {
        let model = self.model.clone();
        move |value| {
            if model.upgrade().is_some_and(|model| get(&model.source.borrow()) == value) {
                return;
            }
            let set = set.clone();
            Model::submit(&model, Box::new(move |source| set(source, value)), error.clone());
        }
    }

    fn switch(
        &mut self,
        title: &str,
        get: impl Fn(&ConfigSource) -> bool + 'static,
        set: impl Fn(&mut ConfigSource, bool) + Send + Clone + 'static,
    ) -> SettingsRow {
        let message = Rc::new(ValidationMessage::new(&self.ui));
        let error = Rc::downgrade(&message);
        let get = Rc::new(get);
        let current = get.clone();
        let change = self.change(
            move |s| current(s),
            move |s, value| {
                set(s, value);
                Ok(())
            },
            error,
        );
        let input = Rc::new(Switch::new(&self.ui).on_change(change));
        self.sync(&input, move |input, s| {
            input.set_value(get(s));
        });
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
        slider_range: Option<Rc<dyn Fn(&ConfigSource) -> (f64, f64)>>,
        get: impl Fn(&ConfigSource) -> f64 + 'static,
        set: impl Fn(&mut ConfigSource, f64) + Send + Clone + 'static,
    ) -> SettingsRow {
        let (slider, input, message) = self.numeric_controls(title, scale, integer, slider_range, get, set);
        if let Some(slider) = slider {
            slider.width(120.0);
            input.width(60.0);
            self.row(title, HStack::new(&self.ui).spacing(8.0).push(slider).push(input), message)
        } else { self.row(title, input, message) }
    }

    // Bind native controls independently of their form layout.
    fn numeric_controls(
        &mut self,
        title: &str,
        scale: f64,
        integer: bool,
        slider_range: Option<Rc<dyn Fn(&ConfigSource) -> (f64, f64)>>,
        get: impl Fn(&ConfigSource) -> f64 + 'static,
        set: impl Fn(&mut ConfigSource, f64) + Send + Clone + 'static,
    ) -> (Option<Rc<Slider>>, Rc<NumberField>, Rc<ValidationMessage>) {
        let message = Rc::new(ValidationMessage::new(&self.ui));
        let error = Rc::downgrade(&message);
        let get = Rc::new(get);
        let current = get.clone();
        let set_preview = set.clone();
        let change = self.change(
            move |s| current(s),
            move |s, value| {
                set(s, value);
                Ok(())
            },
            error,
        );
        let commit = move |value: f64| change(value / scale);
        let field = if integer {
            NumberField::new(&self.ui).integer()
        } else {
            NumberField::new(&self.ui)
        };
        let draft = self.model.upgrade().is_some_and(|m| m.draft_base.is_some());
        let input = Rc::new(if draft {
            field.on_edit(commit.clone())
        } else {
            field.on_change(commit.clone())
        });
        let slider = slider_range.as_ref().map(|range| {
            let source = self.model.upgrade().unwrap();
            let (min, max) = range(&source.source.borrow());
            let range = range.clone();
            let model = self.model.clone();
            let preview = self.gap_preview.clone();
            let continuous = preview.is_some();
            let edit_preview = set_preview.clone();
            let preview_input = Rc::downgrade(&input);
            let mut local_preview = None::<ConfigSource>;
            let slider = Rc::new(Slider::new(&self.ui).range(min, max).on_change(move |v| {
                if let Some(model) = model.upgrade() {
                    let (min, max) = range(&model.source.borrow());
                    let value = if preview.is_some() {
                        if integer {
                            v.round()
                        } else {
                            (v * 10.0).round() / 10.0
                        }
                    } else {
                        v.round()
                    }
                    .clamp(min, max);
                    if let Some(preview) = &preview {
                        let local =
                            local_preview.get_or_insert_with(|| model.source.borrow().clone());
                        edit_preview(local, value / scale);
                        preview(local);
                        if let Some(input) = preview_input.upgrade() {
                            input.set_value(value);
                        }
                        let dragging =
                            NSApplication::sharedApplication(model.page_title.ns_view().mtm())
                                .currentEvent()
                                .is_some_and(|event| {
                                    matches!(
                                        event.r#type(),
                                        objc2_app_kit::NSEventType::LeftMouseDragged
                                            | objc2_app_kit::NSEventType::LeftMouseDown
                                    )
                                });
                        if dragging {
                            return;
                        }
                    }
                    local_preview = None;
                    commit(value);
                }
            }));
            slider.ns_slider().setContinuous(continuous);
            slider.accessibility_label(title);
            slider
        });
        input.accessibility_label(title);
        input.min_width(60.0);
        let weak = Rc::downgrade(&input);
        let weak_slider = slider.as_ref().map(Rc::downgrade);
        self.sync.push(Box::new(move |s| {
            if let Some(input) = weak.upgrade() {
                let value = get(s) * scale;
                input.set_value(value);
                if let Some(slider) = weak_slider.as_ref().and_then(Weak::upgrade) {
                    if let Some(range) = &slider_range {
                        let (min, max) = range(s);
                        slider.ns_slider().setMinValue(min);
                        slider.ns_slider().setMaxValue(max);
                    }
                    slider.set_value(value);
                }
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
        input.ns_text_field().setAlignment(objc2_app_kit::NSTextAlignment::Left);
        let value_field = UnitField::new(&self.ui, input.ns_text_field(), "pt");
        value_field.width(82.0);
        let value = VStack::new(&self.ui).spacing(2.0).push(value_field).push(message);
        // Keep the Rust field/delegate alive as well as its native view.
        struct ValueCell {
            view: VStack,
            _input: Rc<NumberField>,
        }
        impl NativeView for ValueCell {
            fn ns_view(&self) -> &objc2_app_kit::NSView { self.view.ns_view() }
        }
        vec![
            Box::new(Label::new(&self.ui, title)),
            Box::new(slider),
            Box::new(ValueCell { view: value, _input: input }),
        ]
    }

    fn text(
        &mut self,
        title: &str,
        get: impl Fn(&ConfigSource) -> String + 'static,
        set: impl Fn(&mut ConfigSource, String) -> Result<(), String> + Send + Clone + 'static,
    ) -> SettingsRow {
        let message = Rc::new(ValidationMessage::new(&self.ui));
        let error = Rc::downgrade(&message);
        let get = Rc::new(get);
        let current = get.clone();
        let draft = self.model.upgrade().is_some_and(|model| model.draft_base.is_some());
        let commit = self.change(move |s| current(s), set, error);
        let field = TextField::new(&self.ui);
        let input = Rc::new(if draft {
            field.on_change(commit)
        } else {
            field.on_commit(commit)
        });
        input.min_width(140.0);
        input.max_width(260.0);
        self.sync(&input, move |input, s| {
            input.set_value(&get(s));
        });
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
        let error = Rc::downgrade(&message);
        let get = Rc::new(get);
        let current = get.clone();
        let items: Vec<_> = values.iter().map(|(_, value)| value.clone()).collect();
        let choices = items.clone();
        let change = self.change(
            move |s| current(s),
            move |s, value| {
                set(s, value);
                Ok(())
            },
            error,
        );
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
        default: impl Fn(&ConfigSource) -> T + Send + Clone + 'static,
        set: impl Fn(&mut ConfigSource, Option<T>) + Send + Clone + 'static,
    ) -> SettingsRow {
        let message = Rc::new(ValidationMessage::new(&self.ui));
        let error = Rc::downgrade(&message);
        let model = self.model.clone();
        let get = Rc::new(get);
        let current = get.clone();
        let items: Vec<_> = values.iter().map(|(_, value)| value.clone()).collect();
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
            let source = owner.source.borrow();
            let value = (choice != inherited(&source)).then_some(choice);
            if current(&source) == value {
                return;
            }
            drop(source);
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
        let view = objc2::rc::Weak::new(row.control_view());
        let last = Cell::new(None);
        self.sync.push(Box::new(move |s| {
            fn apply(view: &objc2_app_kit::NSView, value: bool) {
                if let Some(control) = view.downcast_ref::<objc2_app_kit::NSControl>() {
                    control.setEnabled(value);
                }
                for child in view.subviews() {
                    apply(&child, value);
                }
            }
            let value = enabled(s);
            if last.replace(Some(value)) == Some(value) {
                return;
            }
            if let Some(view) = view.load() {
                apply(&view, value);
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
