//! Lazy main-thread Settings composition. ConfigActor owns all mutations and persistence.
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use cgs::*;
use objc2::MainThreadOnly;
use objc2_app_kit::NSApplication;
use tokio::sync::mpsc::UnboundedSender;

use crate::actor::config::SourceEdit;
use crate::common::config::ConfigSource;

mod commands;
mod editors;
mod pages;

pub enum Action {
    Edit(SourceEdit),
    Reload,
}
pub struct Request {
    pub action: Action,
    pub finish: Box<dyn FnOnce(Result<ConfigSource, String>)>,
}

struct Model {
    source: RefCell<ConfigSource>,
    requests: UnboundedSender<Request>,
    syncing: Cell<bool>,
    sheet: RefCell<Option<Sheet>>,
    window: RefCell<objc2::rc::Weak<objc2_app_kit::NSWindow>>,
    displays: RefCell<Vec<crate::sys::screen::ScreenInfo>>,
    config_path: std::path::PathBuf,
}

type SyncControl = Box<dyn Fn(&ConfigSource)>;
pub(super) struct Page {
    view: Rc<dyn NativeView>,
    sync: Vec<SyncControl>,
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
        requests: UnboundedSender<Request>,
    ) -> Self {
        let model = Rc::new(Model {
            source: RefCell::new(source),
            requests,
            syncing: Cell::new(false),
            sheet: RefCell::new(None),
            window: RefCell::new(objc2::rc::Weak::default()),
            displays: RefCell::new(displays),
            config_path,
        });
        let host = Rc::new(PageHost::new(&ui));
        let pages = Rc::new(RefCell::new((0..8).map(|_| None).collect()));
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
                selected_page.set(id);
                Self::select(ui, &model, &host, &pages, id);
            }
        });
        sidebar.set_selected(0);
        let window = SettingsWindow::new(&ui, "Rift Settings").content(NavigationSplitView::new(
            &ui,
            sidebar,
            host.clone(),
        ));
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
        if pages.borrow()[id].is_none() {
            let page = pages::build(ui, model, id);
            pages.borrow_mut()[id] = Some(page);
        }
        let pages = pages.borrow();
        let page = pages[id].as_ref().unwrap();
        model.syncing.set(true);
        for sync in &page.sync {
            sync(&model.source.borrow());
        }
        model.syncing.set(false);
        host.set_page(page.view.clone());
    }

    pub fn refresh_displays(&self, displays: Vec<crate::sys::screen::ScreenInfo>) {
        if *self.model.displays.borrow() != displays {
            *self.model.displays.borrow_mut() = displays;
            self.pages.borrow_mut()[1] = None;
            if self.selected.get() == 1 {
                Self::select(
                    Ui::new(self.window.ns_window().mtm()),
                    &self.model,
                    &self._host,
                    &self.pages,
                    1,
                );
            }
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
        *self.model.source.borrow_mut() = source;
        if !self.visible() {
            return;
        }
        self.model.syncing.set(true);
        if let Some(page) = &self.pages.borrow()[self.selected.get()] {
            for sync in &page.sync {
                sync(&self.model.source.borrow());
            }
        }
        self.model.syncing.set(false);
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
        }
    }

    fn submit(model: &Weak<Model>, edit: SourceEdit, error: Weak<ValidationMessage>) {
        let Some(model) = model.upgrade() else {
            return;
        };
        if model.syncing.get() {
            return;
        }
        let weak_model = Rc::downgrade(&model);
        let finish = Box::new(move |result: Result<ConfigSource, String>| {
            if let Some(label) = error.upgrade() {
                label.set_validation(&match &result {
                    Ok(_) => Validation::None,
                    Err(e) => Validation::Error(e.clone()),
                });
            }
            if let (Some(model), Ok(source)) = (weak_model.upgrade(), result) {
                *model.source.borrow_mut() = source;
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

    fn enabled(&mut self, row: &SettingsRow, enabled: impl Fn(&ConfigSource) -> bool + 'static) {
        let view = objc2::rc::Weak::new(row.control_view());
        self.sync.push(Box::new(move |s| {
            fn apply(view: &objc2_app_kit::NSView, value: bool) {
                if let Some(control) = view.downcast_ref::<objc2_app_kit::NSControl>() {
                    control.setEnabled(value);
                }
                for child in view.subviews() {
                    apply(&child, value);
                }
            }
            if let Some(view) = view.load() {
                apply(&view, enabled(s));
            }
        }));
    }
}
