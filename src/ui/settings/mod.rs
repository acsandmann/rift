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

pub enum Action {
    Edit(SourceEdit),
    Reload,
    RefreshRuntime,
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
    window: RefCell<objc2::rc::Weak<objc2_app_kit::NSWindow>>,
    displays: RefCell<Vec<crate::sys::screen::ScreenInfo>>,
    config_path: std::path::PathBuf,
    applications: RefCell<Vec<rift_protocol::ApplicationData>>,
    installed_applications: RefCell<Option<Vec<(String, String)>>>,
    application_inventory: RefCell<Vec<applications::Choice>>,
    page_title: Rc<Label>,
    toolbar: RefCell<Weak<Toolbar>>,
}

type SyncControl = Box<dyn Fn(&ConfigSource)>;
pub(super) struct Page {
    view: Rc<dyn NativeView>,
    sync: Vec<SyncControl>,
    synced_revision: Cell<Option<u64>>,
    back: Option<Rc<dyn Fn()>>,
}

impl Model {
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

/// One-level drill-in shared by the settings collections; callbacks hold weak references.
pub(super) struct DetailNavigation {
    ui: Ui,
    model: Weak<Model>,
    host: Rc<NavigationHost>,
    current: RefCell<Option<Rc<Page>>>,
    title: String,
}
impl DetailNavigation {
    fn new(ui: Ui, model: &Rc<Model>, title: &str) -> Rc<Self> {
        Rc::new(Self {
            ui,
            model: Rc::downgrade(model),
            host: Rc::new(NavigationHost::new(&ui)),
            current: RefCell::new(None),
            title: title.into(),
        })
    }

    fn pop(&self) {
        self.host.pop();
        self.current.borrow_mut().take();
        if let Some(model) = self.model.upgrade() {
            model.page_title.set_text(&self.title);
            if let Some(toolbar) = model.toolbar.borrow().upgrade() {
                toolbar.set_back(None);
            }
        }
    }

    fn detail(self: &Rc<Self>, mut page: Page) -> Rc<Page> {
        page.view = Rc::new(SettingsPage::new(&self.ui, "").section(page.view));
        Rc::new(page)
    }

    fn push(self: &Rc<Self>, page: Rc<Page>, title: &str) {
        if let Some(model) = self.model.upgrade() {
            page.synchronize(&model);
            model.page_title.set_text(title);
            if let Some(toolbar) = model.toolbar.borrow().upgrade() {
                let weak = Rc::downgrade(self);
                toolbar.set_back(Some((
                    &format!("Back to {}", self.title),
                    Box::new(move || {
                        if let Some(nav) = weak.upgrade() {
                            nav.pop();
                        }
                    }),
                )));
            }
            self.host.push(page.view.clone());
            *self.current.borrow_mut() = Some(page);
        }
    }

    fn finish(self: &Rc<Self>, mut page: Page) -> Page {
        self.host.set_root(page.view.clone());
        page.view = self.host.clone();
        let nav = self.clone();
        page.back = Some(Rc::new(move || nav.pop()));
        let nav = self.clone();
        page.sync.push(Box::new(move |_| {
            if let (Some(model), Some(page)) = (nav.model.upgrade(), nav.current.borrow().as_ref())
            {
                page.synchronize(&model);
            }
        }));
        page
    }
}

pub struct Settings {
    window: SettingsWindow,
    model: Rc<Model>,
    _host: Rc<PageHost>,
    pages: Rc<RefCell<Vec<Option<Page>>>>,
    selected: Rc<Cell<usize>>,
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
            window: RefCell::new(objc2::rc::Weak::default()),
            displays: RefCell::new(displays),
            config_path,
            applications: RefCell::new(applications),
            installed_applications: RefCell::new(None),
            application_inventory: RefCell::new(Vec::new()),
            page_title: Rc::new(Label::new(&ui, "General")),
            toolbar: RefCell::new(Weak::new()),
        });
        model.rebuild_applications();
        let host = Rc::new(PageHost::new(&ui));
        let pages = Rc::new(RefCell::new((0..8).map(|_| None::<Page>).collect::<Vec<_>>()));
        let selected = Rc::new(Cell::new(0));
        let weak_model = Rc::downgrade(&model);
        let weak_host = Rc::downgrade(&host);
        let weak_pages = Rc::downgrade(&pages);
        let selected_page = selected.clone();
        let sidebar = Sidebar::new(
            &ui,
            [
                ("General", "gearshape"),
                ("Layouts", "rectangle.split.2x2"),
                ("Workspaces", "square.grid.2x2"),
                ("Rules", "line.3.horizontal.decrease"),
                ("Keyboard", "keyboard"),
                ("Mouse & Trackpad", "computermouse"),
                ("Interface", "macwindow"),
                ("Advanced", "slider.horizontal.3"),
            ]
            .into_iter()
            .enumerate()
            .map(|(id, (title, symbol))| SidebarItem {
                id,
                title: title.into(),
                symbol: symbol.into(),
            })
            .collect(),
        )
        .on_select(move |id| {
            if let (Some(model), Some(host), Some(pages)) =
                (weak_model.upgrade(), weak_host.upgrade(), weak_pages.upgrade())
            {
                if selected_page.replace(id) != id {
                    Self::select(ui, &model, &host, &pages, id);
                }
            }
        });
        let sidebar = Rc::new(sidebar);
        sidebar.set_selected(0);
        let window = SettingsWindow::new(&ui, "Rift Settings")
            .page_title(&ui, model.page_title.clone())
            .on_close(on_close)
            .content(NavigationSplitView::new(&ui, sidebar, host.clone()));
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
            selected,
        }
    }

    fn select(
        ui: Ui,
        model: &Rc<Model>,
        host: &Rc<PageHost>,
        pages: &Rc<RefCell<Vec<Option<Page>>>>,
        id: usize,
    ) {
        if let Some(toolbar) = model.toolbar.borrow().upgrade() {
            toolbar.set_back(None);
        }
        model.page_title.set_text(
            [
                "General",
                "Layouts",
                "Workspaces",
                "Rules",
                "Keyboard",
                "Mouse & Trackpad",
                "Interface",
                "Advanced",
            ][id],
        );
        if pages.borrow()[id].is_none() {
            let page = pages::build(ui, model, id);
            pages.borrow_mut()[id] = Some(page);
        }
        let pages = pages.borrow();
        let page = pages[id].as_ref().unwrap();
        if let Some(back) = &page.back {
            back();
        }
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
            if self.selected.get() != 1 {
                return;
            }
            self._host.clear();
            Self::select(
                Ui::new(self.window.ns_window().mtm()),
                &self.model,
                &self._host,
                &self.pages,
                self.selected.get(),
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
            if let Some(page) = &self.pages.borrow()[self.selected.get()] {
                page.synchronize(&self.model);
            }
        }
    }
}

impl Drop for Settings {
    fn drop(&mut self) {
        // End the sheet while its weak parent still points to the live Settings window.
        self.model.sheet.borrow_mut().take();
        self._host.clear();
        self.pages.borrow_mut().clear();
    }
}

pub(super) struct FormBuilder {
    ui: Ui,
    model: Weak<Model>,
    sync: Vec<SyncControl>,
}
impl FormBuilder {
    fn new(ui: Ui, model: &Rc<Model>) -> Self {
        Self {
            ui,
            model: Rc::downgrade(model),
            sync: Vec::new(),
        }
    }

    fn finish(self, view: impl NativeView) -> Page {
        Page {
            view: Rc::new(view),
            sync: self.sync,
            synced_revision: Cell::new(None),
            back: None,
        }
    }

    fn submit(model: &Weak<Model>, edit: SourceEdit, error: Weak<ValidationMessage>) {
        let Some(model) = model.upgrade() else {
            return;
        };
        if model.syncing.get() {
            return;
        }
        let finish = Box::new(move |result: Result<(), String>| {
            if let Some(label) = error.upgrade() {
                label.set_validation(&match &result {
                    Ok(_) => Validation::None,
                    Err(e) => Validation::Error(e.clone()),
                });
            }
        });
        let _ = model.requests.send(Request {
            action: Action::Edit(edit),
            finish,
        });
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

    fn switch(
        &mut self,
        title: &str,
        get: impl Fn(&ConfigSource) -> bool + 'static,
        set: impl Fn(&mut ConfigSource, bool) + Send + Clone + 'static,
    ) -> SettingsRow {
        let message = Rc::new(ValidationMessage::new(&self.ui));
        let error = Rc::downgrade(&message);
        let model = self.model.clone();
        let get = Rc::new(get);
        let current = get.clone();
        let input = Rc::new(Switch::new(&self.ui).on_change(move |v| {
            if model.upgrade().is_some_and(|m| current(&m.source.borrow()) == v) {
                return;
            }
            let set = set.clone();
            Self::submit(
                &model,
                Box::new(move |s| {
                    set(s, v);
                    Ok(())
                }),
                error.clone(),
            );
        }));
        let weak = Rc::downgrade(&input);
        self.sync.push(Box::new(move |s| {
            if let Some(input) = weak.upgrade() {
                input.set_value(get(s));
            }
        }));
        self.row(title, input, message)
    }

    fn number(
        &mut self,
        title: &str,
        scale: f64,
        get: impl Fn(&ConfigSource) -> f64 + 'static,
        set: impl Fn(&mut ConfigSource, f64) + Send + Clone + 'static,
    ) -> SettingsRow {
        self.numeric(title, scale, false, get, set)
    }

    fn integer(
        &mut self,
        title: &str,
        get: impl Fn(&ConfigSource) -> f64 + 'static,
        set: impl Fn(&mut ConfigSource, f64) + Send + Clone + 'static,
    ) -> SettingsRow {
        self.numeric(title, 1.0, true, get, set)
    }

    fn numeric(
        &mut self,
        title: &str,
        scale: f64,
        integer: bool,
        get: impl Fn(&ConfigSource) -> f64 + 'static,
        set: impl Fn(&mut ConfigSource, f64) + Send + Clone + 'static,
    ) -> SettingsRow {
        let message = Rc::new(ValidationMessage::new(&self.ui));
        let error = Rc::downgrade(&message);
        let model = self.model.clone();
        let get = Rc::new(get);
        let current = get.clone();
        let input = Rc::new(
            if integer {
                NumberField::new(&self.ui).integer()
            } else {
                NumberField::new(&self.ui)
            }
            .on_change(move |v| {
                if model.upgrade().is_some_and(|m| current(&m.source.borrow()) == v / scale) {
                    return;
                }
                let set = set.clone();
                Self::submit(
                    &model,
                    Box::new(move |s| {
                        set(s, v / scale);
                        Ok(())
                    }),
                    error.clone(),
                );
            }),
        );
        input.min_width(60.0);
        let weak = Rc::downgrade(&input);
        self.sync.push(Box::new(move |s| {
            if let Some(input) = weak.upgrade() {
                input.set_value(get(s) * scale);
            }
        }));
        self.row(title, input, message)
    }

    fn text(
        &mut self,
        title: &str,
        get: impl Fn(&ConfigSource) -> String + 'static,
        set: impl Fn(&mut ConfigSource, String) -> Result<(), String> + Send + Clone + 'static,
    ) -> SettingsRow {
        let message = Rc::new(ValidationMessage::new(&self.ui));
        let error = Rc::downgrade(&message);
        let model = self.model.clone();
        let get = Rc::new(get);
        let current = get.clone();
        let input = Rc::new(TextField::new(&self.ui).on_commit(move |v| {
            if model.upgrade().is_some_and(|m| current(&m.source.borrow()) == v) {
                return;
            }
            let set = set.clone();
            Self::submit(&model, Box::new(move |s| set(s, v)), error.clone());
        }));
        input.min_width(140.0);
        input.max_width(260.0);
        let weak = Rc::downgrade(&input);
        self.sync.push(Box::new(move |s| {
            if let Some(input) = weak.upgrade() {
                input.set_value(&get(s));
            }
        }));
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
        let model = self.model.clone();
        let get = Rc::new(get);
        let current = get.clone();
        let items: Vec<_> = values.iter().map(|(_, v)| v.clone()).collect();
        let choices = items.clone();
        let input = Rc::new(
            Popup::new(&self.ui).items(values.iter().map(|(label, _)| *label)).on_change(
                move |index| {
                    if model
                        .upgrade()
                        .is_some_and(|m| current(&m.source.borrow()) == choices[index].clone())
                    {
                        return;
                    }
                    let value = choices[index].clone();
                    let set = set.clone();
                    Self::submit(
                        &model,
                        Box::new(move |s| {
                            set(s, value);
                            Ok(())
                        }),
                        error.clone(),
                    );
                },
            ),
        );
        let weak = Rc::downgrade(&input);
        self.sync.push(Box::new(move |s| {
            if let Some(input) = weak.upgrade() {
                input.set_selected(items.iter().position(|v| *v == get(s)).unwrap_or(0));
            }
        }));
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
            Self::submit(
                &model,
                Box::new(move |source| {
                    set(source, value);
                    Ok(())
                }),
                error.clone(),
            );
        }));
        let weak = Rc::downgrade(&input);
        self.sync.push(Box::new(move |source| {
            if let Some(input) = weak.upgrade() {
                input.set_items(labels.iter().map(String::as_str));
                let inherited = default(source);
                for (index, item) in items.iter().enumerate() {
                    input.set_item_badge(index, (*item == inherited).then_some("Default"));
                }
                let effective = get(source).unwrap_or(inherited);
                input.set_selected(items.iter().position(|item| *item == effective).unwrap_or(0));
            }
        }));
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
