use cgs::Ui;
use cgs::collection::{EditableList, Table};
use cgs::controls::{Button, Popup, SheetActions};
use cgs::input::{
    Key, KeyRecorder, KeyShortcut, ModifierRecorder, Modifiers, SearchField, TextField,
};
use cgs::layout::{HStack, VStack};
use cgs::menu::{Menu, MenuItem};
use cgs::settings::{SettingsRow, ShortcutPresentation as Style, ValidationMessage};
use cgs::text::{Label, SecondaryLabel};
use cgs::view::NativeView;
use cgs::window::{Popover, Sheet};
use objc2::Message;
use rift_protocol::{
    CommandArgument as Arg, Direction, DisplaySelector, LayoutCommand as L, ReactorCommand as R,
    ResizeOrientation,
};

use super::*;
use crate::actor::reactor::Command;
use crate::actor::wm_controller::{ConfiguredLayoutCommand, ExecCmd, WmCmd, WmCommand};
use crate::common::config::{ConfigSource, WorkspaceSelector};
use crate::sys::hotkey::Hotkey;

mod arguments;
mod management;
use arguments::*;
use management::*;

/// Page sections, in order; actions without a template are listed last.
const SECTIONS: [&str; 7] = [
    "Window",
    "Focus",
    "Workspace",
    "Display",
    "Layout",
    "Overview",
    "Rift",
];
const OTHER: usize = 6;

fn inline_message(ui: &Ui) -> Rc<ValidationMessage> { Style::validation(ui) }

fn key_glyph(key: &str) -> &str {
    match key {
        "Left" => "←",
        "Right" => "→",
        "Up" => "↑",
        "Down" => "↓",
        "Comma" => ",",
        "Period" => ".",
        "Slash" => "/",
        "Backslash" => "\\",
        "Semicolon" => ";",
        "Quote" => "'",
        "Backquote" => "`",
        "BracketLeft" => "[",
        "BracketRight" => "]",
        "Equal" => "=",
        "Minus" => "-",
        "Enter" => "↩",
        "Tab" => "⇥",
        "Space" => "Space",
        "Escape" => "⎋",
        "Backspace" => "⌫",
        "Delete" => "⌦",
        "PageUp" => "⇞",
        "PageDown" => "⇟",
        "Home" => "↖",
        "End" => "↘",
        key => key,
    }
}
/// A binding key as the recorder shows it, such as ⌃⌥←.
fn shortcut(hotkey: &Hotkey) -> KeyShortcut {
    KeyShortcut {
        modifiers: from_rift_modifiers(hotkey.modifiers),
        key: Key {
            code: (0..128)
                .find(|code| {
                    crate::sys::hotkey::cg_keycode_to_keycode(*code) == Some(hotkey.key_code)
                })
                .unwrap_or(0),
            display: key_glyph(&hotkey.key_code.to_string()).into(),
        },
    }
}
fn glyphs(key: &str, s: &ConfigSource) -> String {
    s.hotkey(key).map_or_else(|| key.into(), |hotkey| shortcut(&hotkey).to_string())
}

const MODIFIER_NAMES: [(Modifiers, &str); 4] = [
    (Modifiers::Control, "Ctrl"),
    (Modifiers::Option, "Alt"),
    (Modifiers::Shift, "Shift"),
    (Modifiers::Command, "Meta"),
];

pub(super) fn to_rift_modifiers(flags: Modifiers) -> crate::sys::hotkey::Modifiers {
    let mut result = crate::sys::hotkey::Modifiers::empty();
    for (native, rift) in modifier_flags() {
        if flags.contains(native) {
            result.insert(rift);
        }
    }
    result
}

pub(super) fn from_rift_modifiers(flags: crate::sys::hotkey::Modifiers) -> Modifiers {
    let mut result = Modifiers::empty();
    for (native, rift) in modifier_flags() {
        if flags.intersects(rift) {
            result.insert(native);
        }
    }
    result
}

fn modifier_flags() -> [(Modifiers, crate::sys::hotkey::Modifiers); 4] {
    use crate::sys::hotkey::Modifiers as M;
    [
        (Modifiers::Control, M::CONTROL),
        (Modifiers::Option, M::ALT),
        (Modifiers::Shift, M::SHIFT),
        (Modifiers::Command, M::META),
    ]
}

pub(super) fn recorded_key(key: &KeyShortcut) -> Option<String> {
    let code = crate::sys::hotkey::cg_keycode_to_keycode(key.key.code)?;
    let mut modifiers = Vec::new();
    for (flag, name) in MODIFIER_NAMES {
        if key.modifiers.contains(flag) {
            modifiers.push(name.to_string());
        }
    }
    modifiers.push(code.to_string());
    Some(modifiers.join(" + "))
}

type Setter = Rc<dyn Fn(WmCommand)>;

/// Keep content at its natural width, but below the priority at which it would set the
/// window's minimum width, so rows shrink with the window instead.
fn compressible(view: &impl NativeView) { Style::flexible(view); }

#[derive(Clone, PartialEq, Eq)]
enum Id {
    Bound(String),
    /// An action without parameters or a binding, by template index.
    Unbound(usize),
}

/// One action row; rebuilt only when its binding or how it's described changes.
struct Row {
    id: Id,
    command: WmCommand,
    title: String,
    shortcut: String,
    section: usize,
    hotkey: Option<Hotkey>,
    /// Lowercased title, glyphs and key, for filtering.
    text: String,
    controls: std::cell::OnceCell<RowControls>,
    divider_hidden: Cell<bool>,
    first: Cell<bool>,
    starts_action: Cell<bool>,
    last: Cell<bool>,
}
impl PartialEq for Row {
    fn eq(&self, other: &Self) -> bool { std::ptr::eq(self, other) }
}
struct RowControls {
    view: Rc<cgs::view::View>,
    cell: cgs::view::View,
    divider: Rc<HStack>,
    name: Rc<Label>,
    _recorder: Rc<KeyRecorder>,
    conflicts: Rc<VStack>,
    _menu: Option<Menu>,
}

impl RowControls {
    fn present(&self, row: &Row) {
        self.divider.set_hidden(row.divider_hidden.get());
        self.name.set_text(if row.starts_action.get() {
            template(&row.command).map_or("Custom command", |i| actions()[i].1)
        } else {
            ""
        });
        self.view.set_card_edges(row.first.get(), row.last.get());
    }
}

/// The Keyboard page's transient view state; the config source stays authoritative.
#[derive(PartialEq)]
enum ShortcutItem {
    Section(usize),
    Binding(Rc<Row>),
    Welcome,
    Empty,
    Advanced,
}

struct Shortcuts {
    ui: Ui,
    model: Weak<Model>,
    mode: RefCell<String>,
    rendered_mode: RefCell<String>,
    query: RefCell<String>,
    all_actions: Cell<bool>,
    popover: RefCell<Option<Popover>>,
    rows: RefCell<Vec<Rc<Row>>>,
    table: Rc<Table<ShortcutItem>>,
    advanced: RefCell<Option<Rc<cgs::view::View>>>,
    empty: SecondaryLabel,
    welcome: Rc<VStack>,
    welcome_defaults: Rc<HStack>,
}

impl Shortcuts {
    fn mode(&self) -> String { self.mode.borrow().clone() }

    /// Submit a binding; `done` learns whether it was saved.
    fn save(
        &self,
        old: Option<(String, WmCommand)>,
        key: String,
        command: WmCommand,
        error: &Weak<ValidationMessage>,
        done: impl FnOnce(bool) + 'static,
    ) {
        let Some(model) = self.model.upgrade() else { return };
        let mode = self.mode();
        let error = error.clone();
        model.submit_edit(
            Box::new(move |s| {
                s.bind(&mode, old.as_ref().map(|(k, c)| (k.as_str(), c)), key, command)
            }),
            Box::new(move |result| {
                report(&error, &result);
                done(result.is_ok());
            }),
        );
    }

    fn remove(
        &self,
        key: String,
        command: WmCommand,
        error: &Weak<ValidationMessage>,
        done: impl FnOnce(bool) + 'static,
    ) {
        let Some(model) = self.model.upgrade() else { return };
        let mode = self.mode();
        let error = error.clone();
        model.submit_edit(
            Box::new(move |s| {
                let bindings = s.keymap_mut(&mode)?;
                if bindings.get(&key) != Some(&command) {
                    return Err("This shortcut changed elsewhere; try again".into());
                }
                bindings.remove(&key);
                Ok(())
            }),
            Box::new(move |result| {
                report(&error, &result);
                done(result.is_ok());
            }),
        );
    }

    fn recording_guard(
        &self,
        error: Weak<ValidationMessage>,
    ) -> impl FnMut(bool) -> Result<(), String> + 'static {
        let model = self.model.clone();
        move |recording| {
            let Some(model) = model.upgrade() else {
                return Err("Settings has closed".into());
            };
            let result = model
                .env
                .shortcut_capture
                .borrow()
                .as_ref()
                .map_or(Ok(()), |capture| capture(recording));
            if recording && result.is_err() {
                report(&error, &result);
            }
            result
        }
    }

    fn close_inspector(&self) {
        // Closing commits the active field, which can call back into this page.
        let popover = self.popover.borrow_mut().take();
        if let Some(popover) = popover {
            popover.close();
        }
    }

    fn inspect(
        self: &Rc<Self>,
        anchor: &objc2_app_kit::NSView,
        draft: &Rc<RefCell<WmCommand>>,
        set: &Setter,
        error: Weak<ValidationMessage>,
        unbound: bool,
    ) {
        self.close_inspector();
        let Some(model) = self.model.upgrade() else { return };
        let command = draft.borrow().clone();
        let (page, apply) = (Rc::downgrade(self), set.clone());
        let set: Setter = Rc::new(move |command| {
            apply(command);
            if let Some(page) = page.upgrade() {
                if let Some(popover) = page.popover.borrow().as_ref() {
                    popover.fit_content();
                }
            }
        });
        let Some(control) = parameters(self.ui, &model, &command, unbound, set, error) else {
            return;
        };
        let content = Style::popover(
            &self.ui,
            template(&command).map_or("Command options", |i| actions()[i].1),
            control,
            raw_parameters(&command) || matches!(command, WmCommand::Wm(WmCmd::Exec(_))),
        );
        let popover = Popover::new(&self.ui, content);
        popover.show(&anchor.retain());
        *self.popover.borrow_mut() = Some(popover);
    }

    /// Records over `id`'s shortcut, binding `draft`. Delete removes a binding; a failed
    /// save restores the shortcut shown.
    fn recorder(
        self: &Rc<Self>,
        id: &Id,
        draft: Rc<RefCell<WmCommand>>,
        hotkey: Option<&Hotkey>,
        error: Weak<ValidationMessage>,
        parameter_error: Weak<ValidationMessage>,
        panel: Rc<VStack>,
        finished: Option<Rc<dyn Fn()>>,
    ) -> Rc<KeyRecorder> {
        let except = match id {
            Id::Bound(key) => Some(key.clone()),
            Id::Unbound(_) => None,
        };
        let original = hotkey.map(shortcut);
        let this = Rc::new(RefCell::new(Weak::<KeyRecorder>::new()));
        let (weak, value, finish) = (this.clone(), original.clone(), finished.clone());
        let restore = move |saved: bool| {
            if saved {
                if let Some(finish) = &finish {
                    finish();
                }
            }
            if !saved && let Some(recorder) = weak.borrow().upgrade() {
                recorder.set_value(value.clone());
            }
        };
        let (page, conflict_error, recorder_ref, pending_command, conflict_id) = (
            Rc::downgrade(self),
            error.clone(),
            this.clone(),
            draft.clone(),
            id.clone(),
        );
        let (cancel_panel, cancel_error) = (Rc::downgrade(&panel), error.clone());
        let conflict = move |candidate: &KeyShortcut| {
            panel.clear();
            panel.set_hidden(true);
            let page = page.upgrade()?;
            if parameter_error.upgrade().is_some_and(|message| !message.ns_view().isHidden()) {
                let message = "Correct the command options before recording a shortcut".to_string();
                report(&conflict_error, &Err(message.clone()));
                return Some(message);
            }
            if matches!(&*pending_command.borrow(), WmCommand::Sequence(steps) if steps.is_empty())
                || matches!(&*pending_command.borrow(), WmCommand::Wm(WmCmd::Exec(ExecCmd::String(text))) if text.trim().is_empty())
            {
                let message = "Enter a command before recording its shortcut".to_string();
                report(&conflict_error, &Err(message.clone()));
                return Some(message);
            }
            let Some(key) = recorded_key(candidate) else {
                let message =
                    "Rift cannot assign this key. Choose a different key combination.".to_string();
                report(&conflict_error, &Err(message.clone()));
                return Some(message);
            };
            let model = page.model.upgrade()?;
            let mode = page.mode();
            let source = model.source.borrow();
            let hotkey = source.hotkey(&key)?;
            let conflicts: Vec<_> = source
                .keymap(&mode)?
                .iter()
                .filter(|(other, _)| {
                    Some(other.as_str()) != except.as_deref()
                        && source.hotkey(other).is_some_and(|other| other.overlaps(&hotkey))
                })
                .map(|(key, command)| (key.clone(), command.clone()))
                .collect();
            if conflicts.is_empty() {
                report(&conflict_error, &Ok(()));
                return None;
            }
            let names = conflicts
                .iter()
                .map(|(_, cmd)| title(cmd, &source, &model.env.displays.borrow()))
                .collect::<Vec<_>>()
                .join(", ");
            let message = format!(
                "{} is used by {names}. Your existing shortcuts have not changed.",
                candidate
            );
            drop(source);
            report(&conflict_error, &Err(message.clone()));
            let (recorder, weak_panel, error) = (
                recorder_ref.clone(),
                Rc::downgrade(&panel),
                conflict_error.clone(),
            );
            let choose = Button::new(&page.ui, "Choose Different").push().on_click(move || {
                if let Some(panel) = weak_panel.upgrade() {
                    panel.clear();
                    panel.set_hidden(true);
                }
                report(&error, &Ok(()));
                if let Some(recorder) = recorder.borrow().upgrade() {
                    recorder.cancel();
                    recorder.begin_recording();
                }
            });
            let old = match &conflict_id {
                Id::Bound(key) => Some((key.clone(), pending_command.borrow().clone())),
                Id::Unbound(_) => None,
            };
            let command = pending_command.borrow().clone();
            let (page_ref, weak_panel, error, recorder, candidate, finish) = (
                Rc::downgrade(&page),
                Rc::downgrade(&panel),
                conflict_error.clone(),
                recorder_ref.clone(),
                candidate.clone(),
                finished.clone(),
            );
            let replace = Button::new(&page.ui, "Replace Existing").push().on_click(move || {
                let Some(page) = page_ref.upgrade() else { return };
                let Some(model) = page.model.upgrade() else { return };
                let (mode, old, key, command, conflicts) = (
                    mode.clone(),
                    old.clone(),
                    key.clone(),
                    command.clone(),
                    conflicts.clone(),
                );
                let (panel, error, recorder, candidate, finish) = (
                    weak_panel.clone(),
                    error.clone(),
                    recorder.clone(),
                    candidate.clone(),
                    finish.clone(),
                );
                model.submit_edit(
                    Box::new(move |source| {
                        source.bind_replacing(
                            &mode,
                            old.as_ref().map(|(k, c)| (k.as_str(), c)),
                            key,
                            command,
                            &conflicts,
                        )
                    }),
                    Box::new(move |result| {
                        report(&error, &result);
                        if result.is_ok() {
                            if let Some(panel) = panel.upgrade() {
                                panel.clear();
                                panel.set_hidden(true);
                            }
                            if let Some(recorder) = recorder.borrow().upgrade() {
                                recorder.cancel();
                                recorder.set_value(Some(candidate));
                            }
                            if let Some(finish) = finish {
                                finish();
                            }
                        }
                    }),
                );
            });
            panel.add(Style::actions(&page.ui, choose, replace));
            panel.set_hidden(false);
            Some(message)
        };
        let (page, id) = (Rc::downgrade(self), id.clone());
        let recorder = KeyRecorder::new(&self.ui)
            .capture(self.recording_guard(error.clone()))
            .on_cancel(move || {
                if let Some(panel) = cancel_panel.upgrade() {
                    panel.clear();
                    panel.set_hidden(true);
                }
                report(&cancel_error, &Ok(()));
            })
            .conflict(conflict)
            .on_change(move |value| {
                let Some(page) = page.upgrade() else { return };
                let command = draft.borrow().clone();
                match (&id, value.as_ref().and_then(recorded_key)) {
                    (Id::Bound(key), _) if value.is_none() => {
                        page.remove(key.clone(), command, &error, restore.clone())
                    }
                    (Id::Bound(old), Some(key)) => {
                        let unchanged = page.model.upgrade().is_some_and(|model| {
                            let s = model.source.borrow();
                            s.hotkey(old).is_some() && s.hotkey(old) == s.hotkey(&key)
                        });
                        if unchanged {
                            return restore(false);
                        }
                        let old = Some((old.clone(), command.clone()));
                        page.save(old, key, command, &error, restore.clone());
                    }
                    (Id::Unbound(_), Some(key)) => {
                        page.save(None, key, command, &error, restore.clone())
                    }
                    _ => {}
                }
            });
        recorder.set_value(original);
        let recorder = Rc::new(recorder);
        *this.borrow_mut() = Rc::downgrade(&recorder);
        recorder
    }

    /// Compact action, optional parameter summary, and a consistently sized recorder.
    fn row_controls(
        self: &Rc<Self>,
        id: Id,
        command: WmCommand,
        title: String,
        s: &ConfigSource,
    ) -> RowControls {
        let ui = self.ui;
        let key = match &id {
            Id::Bound(key) => Some(key.clone()),
            Id::Unbound(_) => None,
        };
        let hotkey = key.as_deref().and_then(|key| s.hotkey(key));
        let parameter_message = inline_message(&ui);
        let message = inline_message(&ui);
        let error = Rc::downgrade(&parameter_message);
        let draft = Rc::new(RefCell::new(command.clone()));
        let set: Setter = if let Some(key) = &key {
            let (page, key, old, error) =
                (Rc::downgrade(self), key.clone(), command.clone(), error.clone());
            Rc::new(move |cmd| {
                if let Some(page) = page.upgrade() {
                    page.save(
                        Some((key.clone(), old.clone())),
                        key.clone(),
                        cmd,
                        &error,
                        |_| {},
                    );
                }
            })
        } else {
            let draft = draft.clone();
            Rc::new(move |cmd| *draft.borrow_mut() = cmd)
        };
        let action_name = template(&command).map_or("Custom command", |i| actions()[i].1);
        let name = Rc::new(Style::name(&ui, action_name));
        compressible(&*name);
        let parameter: Option<Box<dyn NativeView>> = if simple_parameter(&command) {
            self.model.upgrade().and_then(|model| {
                parameters(ui, &model, &command, key.is_none(), set.clone(), error.clone())
            })
        } else if has_parameters(&command) {
            let text = summary(
                &command,
                s,
                &self
                    .model
                    .upgrade()
                    .map_or_else(Vec::new, |model| model.env.displays.borrow().clone()),
            );
            let (page, draft, set, error, unbound) = (
                Rc::downgrade(self),
                draft.clone(),
                set.clone(),
                error.clone(),
                key.is_none(),
            );
            let button = Style::summary(&ui, &text);
            let anchor = objc2::rc::Weak::new(button.ns_view());
            let button = button.on_click(move || {
                if let (Some(page), Some(anchor)) = (page.upgrade(), anchor.load()) {
                    page.inspect(&anchor, &draft, &set, error.clone(), unbound);
                }
            });
            button.accessibility_label(&format!("Edit {action_name} options"));
            Some(Box::new(button))
        } else {
            None
        };
        let option_button = parameter.as_ref().and_then(|control| {
            control
                .ns_view()
                .downcast_ref::<objc2_app_kit::NSButton>()
                .map(objc2::rc::Weak::new)
        });
        let conflicts = Rc::new(Style::conflicts(&ui));
        conflicts.set_hidden(true);
        let recorder = self.recorder(
            &id,
            draft.clone(),
            hotkey.as_ref(),
            Rc::downgrade(&message),
            error,
            conflicts.clone(),
            None,
        );
        recorder.accessibility_label(&format!("Shortcut for {title}"));
        let page = Rc::downgrade(self);
        let another = Button::new(&ui, "").symbol("plus").borderless().on_click(move || {
            if let Some(page) = page.upgrade() {
                page.open_editor(draft.borrow().clone());
            }
        });
        another.accessibility_label(&format!("Add another {action_name} shortcut"));
        another.ns_view().setToolTip(Some(&objc2_foundation::NSString::from_str(
            "Add another shortcut for this action",
        )));
        if simple_parameter(&command) {
            if let Some(parameter) = parameter.as_ref() {
                parameter.accessibility_label(&title);
            }
        }
        let presentation = cgs::settings::ShortcutRow::new(
            &ui,
            name.clone(),
            parameter,
            &recorder,
            another,
            parameter_message,
            message.clone(),
            conflicts.clone(),
        );
        let (page, copy) = (Rc::downgrade(self), command.clone());
        let duplicate = MenuItem::new(&ui, "Duplicate Shortcut…").on_click(move || {
            if let Some(page) = page.upgrade() {
                page.open_editor(copy.clone());
            }
        });
        let (page, input) = (Rc::downgrade(self), Rc::downgrade(&recorder));
        let change = MenuItem::new(
            &ui,
            if key.is_some() {
                "Change Shortcut…"
            } else {
                "Record Shortcut…"
            },
        )
        .on_click(move || {
            if let (Some(page), Some(input)) = (page.upgrade(), input.upgrade()) {
                page.close_inspector();
                input.begin_recording();
            }
        });
        let menu = Menu::new(&ui).item(change).item(duplicate);
        if let Some(button) = option_button {
            menu.add(MenuItem::new(&ui, "Edit Options…").on_click(move || {
                if let Some(button) = button.load() {
                    unsafe {
                        button.performClick(None);
                    }
                }
            }));
        }
        if let Some(key) = &key {
            let (page, remove_key, expected, error) = (
                Rc::downgrade(self),
                key.clone(),
                command.clone(),
                Rc::downgrade(&message),
            );
            menu.add(MenuItem::new(&ui, "Remove Shortcut").on_click(move || {
                if let Some(page) = page.upgrade() {
                    page.remove(remove_key.clone(), expected.clone(), &error, |_| {});
                }
            }));
            if self.mode() == "default" {
                if let Some(default_key) = defaults().keys.iter().find_map(|(key, value)| {
                    (value == &command)
                        .then(|| defaults().hotkey(key).map(|key| key.to_string()))
                        .flatten()
                }) {
                    let (page, old, expected, error) = (
                        Rc::downgrade(self),
                        key.clone(),
                        command.clone(),
                        Rc::downgrade(&message),
                    );
                    menu.add(MenuItem::new(&ui, "Restore Default Shortcut").on_click(move || {
                        if let Some(page) = page.upgrade() {
                            page.save(
                                Some((old.clone(), expected.clone())),
                                default_key.clone(),
                                expected.clone(),
                                &error,
                                |_| {},
                            );
                        }
                    }));
                }
            }
        }
        unsafe { presentation.view.ns_view().setMenu(Some(menu.ns_menu())) };
        RowControls {
            view: presentation.view,
            cell: presentation.cell,
            divider: presentation.divider,
            name,
            conflicts,
            _recorder: recorder,
            _menu: Some(menu),
        }
    }

    fn realize<'a>(self: &Rc<Self>, row: &'a Row) -> Option<&'a RowControls> {
        let model = self.model.upgrade()?;
        Some(row.controls.get_or_init(|| {
            let phase = timing::start();
            let controls = self.row_controls(
                row.id.clone(),
                row.command.clone(),
                row.title.clone(),
                &model.source.borrow(),
            );
            controls.present(row);
            timing::finish(phase, "keyboard visible row controls");
            controls
        }))
    }

    fn row(
        &self,
        id: Id,
        command: WmCommand,
        title: String,
        shortcut: String,
        source: &ConfigSource,
    ) -> Row {
        let section = template(&command).map_or(OTHER, |i| actions()[i].0);
        let key = match &id {
            Id::Bound(key) => Some(key.as_str()),
            _ => None,
        };
        let hotkey = key.and_then(|key| source.hotkey(key));
        let options = self.model.upgrade().map_or_else(String::new, |model| {
            summary(&command, source, &model.env.displays.borrow())
        });
        let text = search_query(&format!(
            "{title} {shortcut} {options} {} {} {} {}",
            key.unwrap_or_default(),
            hotkey.as_ref().map(ToString::to_string).unwrap_or_default(),
            SECTIONS[section],
            toml_text(&command)
        ));
        Row {
            id,
            command,
            title,
            shortcut,
            section,
            hotkey,
            text,
            controls: std::cell::OnceCell::new(),
            divider_hidden: Cell::new(false),
            first: Cell::new(false),
            starts_action: Cell::new(true),
            last: Cell::new(false),
        }
    }

    /// Reconcile rows with the selected keymap, reusing rows whose binding is unchanged so
    /// focus, drafts and scrolling survive edits elsewhere.
    fn rebuild(self: &Rc<Self>, s: &ConfigSource) {
        let phase = timing::start();
        let Some(model) = self.model.upgrade() else { return };
        let displays = model.env.displays.borrow();
        let mode = self.mode();
        let keymap = s.keymap(&mode);
        let mut wanted: Vec<_> = keymap
            .into_iter()
            .flatten()
            .map(|(key, command)| (Id::Bound(key.clone()), command.clone()))
            .collect();
        // The assigned view does not need native controls for the action catalog.
        // Once browsed, retain those rows so their drafts survive switching tabs.
        if self.all_actions.get()
            || (*self.rendered_mode.borrow() == mode
                && self.rows.borrow().iter().any(|row| matches!(row.id, Id::Unbound(_))))
        {
            for (i, (_, _, command)) in actions().iter().enumerate() {
                if !keymap.is_some_and(|map| map.values().any(|other| same_action(other, command)))
                {
                    wanted.push((Id::Unbound(i), command.clone()));
                }
            }
        }
        let same_mode = *self.rendered_mode.borrow() == mode;
        *self.rendered_mode.borrow_mut() = mode.clone();
        let old = self.rows.take();
        let mut rows: Vec<Rc<Row>> = wanted
            .into_iter()
            .map(|(id, command)| {
                let title = title(&command, s, &displays);
                let shortcut = match &id {
                    Id::Bound(key) => glyphs(key, s),
                    Id::Unbound(_) => String::new(),
                };
                old.iter()
                    .find(|row| {
                        same_mode && {
                            row.id == id
                                && row.command == command
                                && row.title == title
                                && row.shortcut == shortcut
                        }
                    })
                    .cloned()
                    .unwrap_or_else(|| Rc::new(self.row(id, command, title, shortcut, s)))
            })
            .collect();
        timing::finish(phase, "keyboard row metadata");
        let phase = timing::start();
        rows.sort_by_cached_key(|row| {
            let key = match &row.id {
                Id::Bound(key) => key.clone(),
                Id::Unbound(_) => String::new(),
            };
            (row.section, template(&row.command).unwrap_or(usize::MAX), key)
        });
        drop(displays);
        let changed = old.len() != rows.len()
            || old.iter().zip(&rows).any(|(old, row)| !Rc::ptr_eq(old, row));
        if changed {
            *self.rows.borrow_mut() = rows;
        } else {
            *self.rows.borrow_mut() = old.clone();
        }
        drop(old);
        if changed {
            self.close_inspector();
        }
        self.filter();
        timing::finish(phase, "keyboard attach and filter");
    }

    /// Show rows matching the search: words in titles and keys, or a shortcut such as
    /// `alt+h`, which finds what those keys do.
    fn filter(&self) {
        let query = self.query.borrow();
        let hotkey = query
            .contains('+')
            .then(|| self.model.upgrade().and_then(|model| model.source.borrow().hotkey(&query)))
            .flatten();
        let rows = self.rows.borrow();
        let mut any = false;
        let mut items = Vec::new();
        for index in 0..SECTIONS.len() {
            let mut empty = true;
            let mut previous: Option<&Row> = None;
            for row in rows.iter().filter(|row| row.section == index) {
                let included = self.all_actions.get() || matches!(row.id, Id::Bound(_));
                let visible = included
                    && match &hotkey {
                        Some(hotkey) => {
                            row.hotkey.as_ref().is_some_and(|other| other.overlaps(hotkey))
                        }
                        None => query.split_whitespace().all(|word| row.text.contains(word)),
                    };
                if visible {
                    if empty {
                        items.push(ShortcutItem::Section(index));
                    }
                    items.push(ShortcutItem::Binding(row.clone()));
                }
                let starts_action = previous.is_none_or(|previous| {
                    !same_action(&previous.command, &row.command) && previous.command != row.command
                });
                row.first.set(visible && empty);
                row.starts_action.set(starts_action);
                row.divider_hidden.set(!visible || empty || !starts_action);
                if visible {
                    previous = Some(row);
                }
                empty &= !visible;
            }
            any |= !empty;
        }
        items.push(ShortcutItem::Advanced);
        for (index, item) in items.iter().enumerate() {
            if let ShortcutItem::Binding(row) = item {
                let last = !matches!(items.get(index + 1), Some(ShortcutItem::Binding(_)));
                row.last.set(last);
                if let Some(controls) = row.controls.get() {
                    controls.present(row);
                }
            }
        }
        self.empty.set_text(if query.is_empty() {
            "No shortcuts in this mode. Add a shortcut or browse All Actions."
        } else {
            "No matching shortcuts. Try another action, target, or key combination."
        });
        let onboarding = !any
            && query.is_empty()
            && !self.all_actions.get()
            && !rows.iter().any(|row| matches!(row.id, Id::Bound(_)));
        self.welcome.set_hidden(!onboarding);
        self.welcome_defaults.set_hidden(self.mode() != "default");
        self.empty.set_hidden(any || onboarding);
        if onboarding {
            items.insert(0, ShortcutItem::Welcome);
        } else if !any {
            items.insert(0, ShortcutItem::Empty);
        }
        self.table.reconcile_rows(items);
    }

    /// Catalog first, then a compact editor. Recording a valid key completes the add.
    fn open_catalog(self: &Rc<Self>) {
        let Some(model) = self.model.upgrade() else { return };
        self.close_inspector();
        let ui = self.ui;
        let actions = actions()
            .iter()
            .map(|(group, title, command)| cgs::settings::PickerAction {
                group: SECTIONS[*group].into(),
                title: (*title).into(),
                keywords: search_query(&summary(command, defaults(), &[])),
            })
            .collect();
        let (page, cancelled) = (Rc::downgrade(self), Rc::downgrade(self));
        let picker = Rc::new(cgs::settings::ActionPicker::new(
            &ui,
            actions,
            move |index| {
                if let Some(page) = page.upgrade() {
                    page.open_editor(self::actions()[index].2.clone());
                }
            },
            move || {
                if let Some(page) = cancelled.upgrade() {
                    page.close_editor();
                }
            },
        ));
        self.close_editor();
        show_sheet(ui, &model, "Add Shortcut", picker.clone());
        picker.focus_search();
    }

    fn open_editor(self: &Rc<Self>, command: WmCommand) {
        let Some(model) = self.model.upgrade() else { return };
        self.close_inspector();
        let ui = self.ui;
        let draft = Rc::new(RefCell::new(command.clone()));
        let parameter_error = inline_message(&ui);
        let message = inline_message(&ui);
        let conflicts = Rc::new(VStack::new(&ui));
        conflicts.set_hidden(true);
        let params = Rc::new(VStack::new(&ui));
        let (weak_params, weak_model, edited, error) = (
            Rc::downgrade(&params),
            Rc::downgrade(&model),
            draft.clone(),
            Rc::downgrade(&parameter_error),
        );
        let show_parameters = move || {
            let (Some(params), Some(model)) = (weak_params.upgrade(), weak_model.upgrade()) else {
                return;
            };
            params.clear();
            let command = edited.borrow().clone();
            let (changed, resize) = (edited.clone(), Rc::downgrade(&model));
            let set: Setter = Rc::new(move |command| {
                *changed.borrow_mut() = command;
                if let Some(model) = resize.upgrade() {
                    if let Some(sheet) = model.sheet.borrow().as_ref() {
                        sheet.fit_content();
                    }
                }
            });
            if let Some(control) = parameters(ui, &model, &command, true, set, error.clone()) {
                params.set_hidden(false);
                params.add(cgs::settings::ShortcutEditor::option(&ui, control));
            } else {
                params.set_hidden(true);
            }
            if let Some(sheet) = model.sheet.borrow().as_ref() {
                sheet.fit_content();
            }
        };
        show_parameters();
        let current = template(&command).unwrap_or(actions().len() - 1);
        let groups = (0..SECTIONS.len()).map(|section| {
            actions()
                .iter()
                .filter(move |(group, _, _)| *group == section)
                .map(|(_, name, _)| *name)
                .collect::<Vec<_>>()
        });
        let selected = draft.clone();
        let picked = Cell::new(current);
        let (conflict_panel, conflict_message) = (conflicts.clone(), Rc::downgrade(&message));
        let picker = Popup::new(&ui).sections(groups).on_change(move |i| {
            if picked.replace(i) == i {
                return;
            }
            *selected.borrow_mut() = actions()[i].2.clone();
            conflict_panel.clear();
            conflict_panel.set_hidden(true);
            report(&conflict_message, &Ok(()));
            show_parameters();
        });
        picker.set_selected(current);
        let page = Rc::downgrade(self);
        let finished: Rc<dyn Fn()> = Rc::new(move || {
            if let Some(page) = page.upgrade() {
                page.close_editor();
            }
        });
        let recorder = self.recorder(
            &Id::Unbound(current),
            draft,
            None,
            Rc::downgrade(&message),
            Rc::downgrade(&parameter_error),
            conflicts.clone(),
            Some(finished),
        );
        let page = Rc::downgrade(self);
        let cancel =
            Button::new(&ui, "Cancel").push().key_equivalent("\u{1b}").on_click(move || {
                if let Some(page) = page.upgrade() {
                    page.close_editor();
                }
            });
        let feedback =
            cgs::settings::ShortcutEditor::feedback(&ui, parameter_error, message, conflicts);
        let content =
            cgs::settings::ShortcutEditor::new(&ui, picker, params, &recorder, feedback, cancel);
        self.close_editor();
        show_sheet(ui, &model, "New Shortcut", content);
    }

    fn reset_mode(self: &Rc<Self>) {
        let Some(model) = self.model.upgrade() else { return };
        let mode = self.mode();
        let original = model.source.borrow().keymap(&mode).cloned().unwrap_or_default();
        let replacement: std::collections::BTreeMap<String, WmCommand> = if mode == "default" {
            defaults()
                .keys
                .iter()
                .filter_map(|(key, command)| {
                    defaults().hotkey(key).map(|key| (key.to_string(), command.clone()))
                })
                .collect()
        } else {
            Default::default()
        };
        let message = Rc::new(ValidationMessage::new(&self.ui));
        let (model_ref, error) = (Rc::downgrade(&model), Rc::downgrade(&message));
        let confirm = Button::new(
            &self.ui,
            if mode == "default" {
                "Restore Defaults"
            } else {
                "Clear Mode"
            },
        )
        .push()
        .on_click(move || {
            let (mode, original, replacement) =
                (mode.clone(), original.clone(), replacement.clone());
            if let Some(model) = model_ref.upgrade() {
                if let Some(message) = error.upgrade() {
                    save_sheet(
                        &Rc::downgrade(&model),
                        Box::new(move |source| {
                            if source.keymap(&mode) != Some(&original) {
                                return Err("The mode changed elsewhere. Try again.".into());
                            }
                            *source.keymap_mut(&mode)? = replacement;
                            Ok(())
                        }),
                        &message,
                    );
                }
            }
        });
        let model_ref = Rc::downgrade(&model);
        let cancel =
            Button::new(&self.ui, "Cancel")
                .push()
                .key_equivalent("\u{1b}")
                .on_click(move || {
                    if let Some(model) = model_ref.upgrade() {
                        model.close_sheet();
                    }
                });
        let text = if self.mode() == "default" {
            "Restore the shipped shortcuts in Default mode? Other modes and reusable modifiers are kept."
        } else {
            "Remove every shortcut in this mode? Other modes and reusable modifiers are kept."
        };
        let content = Style::sheet(&self.ui)
            .push(Style::description(&self.ui, text))
            .push(message)
            .push(Style::actions(&self.ui, cancel, confirm));
        show_sheet(
            self.ui,
            &model,
            if self.mode() == "default" {
                "Restore Shortcuts"
            } else {
                "Clear Mode"
            },
            content,
        );
    }

    fn close_editor(&self) {
        if let Some(model) = self.model.upgrade() {
            model.close_sheet();
        }
    }
}

pub(super) fn keyboard(ui: Ui, model: &Rc<Model>) -> Page {
    let phase = timing::start();
    let mut f = FormBuilder::new(ui, model);
    let owner: Rc<RefCell<Weak<Shortcuts>>> = Rc::new(RefCell::new(Weak::new()));
    let cells = owner.clone();
    let table = Rc::new(
        Style::list(&ui)
            .cells(move |item, _| {
                let Some(page) = cells.borrow().upgrade() else {
                    return Box::new(Label::new(&ui, "")) as Box<dyn NativeView>;
                };
                match item {
                    ShortcutItem::Section(index) => {
                        Box::new(Style::section(&ui, SECTIONS[*index])) as Box<dyn NativeView>
                    }
                    ShortcutItem::Binding(row) => page.realize(row).map_or_else(
                        || Box::new(Label::new(&ui, "")) as Box<dyn NativeView>,
                        |controls| {
                            Box::new(controls.cell.ns_view().retain()) as Box<dyn NativeView>
                        },
                    ),
                    ShortcutItem::Welcome => {
                        Box::new(page.welcome.ns_view().retain()) as Box<dyn NativeView>
                    }
                    ShortcutItem::Empty => {
                        Box::new(Style::pane_item(&ui, page.empty.ns_view().retain()))
                            as Box<dyn NativeView>
                    }
                    ShortcutItem::Advanced => page.advanced.borrow().as_ref().map_or_else(
                        || Box::new(Label::new(&ui, "")) as Box<dyn NativeView>,
                        |view| Box::new(view.ns_view().retain()) as Box<dyn NativeView>,
                    ),
                }
            })
            .group_rows(|item| matches!(item, ShortcutItem::Section(_)))
            .selectable(|item| matches!(item, ShortcutItem::Binding(_))),
    );
    let page = Rc::new(Shortcuts {
        ui,
        model: Rc::downgrade(model),
        mode: RefCell::new("default".into()),
        rendered_mode: RefCell::new(String::new()),
        query: RefCell::new(String::new()),
        all_actions: Cell::new(false),
        popover: RefCell::new(None),
        rows: RefCell::new(Vec::new()),
        table: table.clone(),
        advanced: RefCell::new(None),
        welcome: Rc::new(Style::welcome_container(&ui)),
        welcome_defaults: Rc::new(HStack::new(&ui)),
        empty: Style::empty(
            &ui,
            "No matching shortcuts. Try another action or key combination.",
        ),
    });
    *owner.borrow_mut() = Rc::downgrade(&page);
    page.empty.set_hidden(true);
    let keymaps = Rc::new(Menu::new(&ui));
    let names = Rc::new(RefCell::new(Vec::<String>::new()));
    let (sync_keymaps, sync_page) = (keymaps.clone(), page.clone());
    f.sync.push(Box::new(move |s| {
        let mut modes = vec!["default".to_string()];
        modes.extend(s.binding_modes.keys().cloned());
        if !modes.contains(&sync_page.mode()) {
            *sync_page.mode.borrow_mut() = "default".into();
        }
        if *names.borrow() != modes {
            sync_keymaps.clear();
            for name in &modes {
                let (weak, name) = (Rc::downgrade(&sync_page), name.clone());
                let item = MenuItem::new(&ui, keymap_title(&name)).on_activate(move |clicked| {
                    let Some(page) = weak.upgrade() else { return };
                    page.close_inspector();
                    for row in page.rows.borrow().iter() {
                        if let Some(controls) = row.controls.get() {
                            controls._recorder.cancel();
                            controls.conflicts.clear();
                            controls.conflicts.set_hidden(true);
                        }
                    }
                    if let Some(menu) = unsafe { clicked.menu() } {
                        for item in menu.itemArray() {
                            item.setState(if std::ptr::eq(&*item, clicked) { 1 } else { 0 });
                        }
                    }
                    *page.mode.borrow_mut() = name.clone();
                    if let Some(model) = page.model.upgrade() {
                        page.rebuild(&model.source.borrow());
                    }
                });
                sync_keymaps.add(item);
            }
            *names.borrow_mut() = modes.clone();
        }
        for (item, name) in sync_keymaps.ns_menu().itemArray().iter().zip(&modes) {
            item.setState(if *name == sync_page.mode() { 1 } else { 0 });
        }
        sync_page.rebuild(s);
    }));
    let weak = Rc::downgrade(&page);
    let add = MenuItem::new(&ui, "Add Shortcut…").on_click(move || {
        if let Some(page) = weak.upgrade() {
            page.open_catalog();
        }
    });
    let keymap_item = MenuItem::new(&ui, "Keymap: Default").submenu(keymaps);
    let keymap_label = objc2::rc::Weak::new(keymap_item.ns_menu_item());
    let menu = Menu::new(&ui).item(add).separator().item(keymap_item).separator();
    let mut managed = Vec::new();
    for (title, operation) in [
        ("New Mode…", KeymapOperation::Create),
        ("Rename Mode…", KeymapOperation::Rename),
        ("Delete Mode…", KeymapOperation::Delete),
    ] {
        let (weak_model, weak) = (Rc::downgrade(model), Rc::downgrade(&page));
        let item = MenuItem::new(&ui, title).on_click(move || {
            if let (Some(model), Some(page)) = (weak_model.upgrade(), weak.upgrade()) {
                mode_sheet(ui, &model, page.mode(), operation);
            }
        });
        if operation != KeymapOperation::Create {
            managed.push(item.weak());
        }
        menu.add(item);
    }
    let reset_page = Rc::downgrade(&page);
    menu.add_separator();
    let reset = MenuItem::new(&ui, "Restore Default Shortcuts…").on_click(move || {
        if let Some(page) = reset_page.upgrade() {
            page.reset_mode();
        }
    });
    let reset_item = objc2::rc::Weak::new(reset.ns_menu_item());
    menu.add(reset);
    let weak = Rc::downgrade(&page);
    let menu = menu.on_tracking(move |_| {
        let mode = weak.upgrade().map_or_else(|| "default".into(), |page| page.mode());
        let custom = mode != "default";
        if let Some(item) = keymap_label.load() {
            item.setTitle(&objc2_foundation::NSString::from_str(&format!(
                "Keymap: {}",
                keymap_title(&mode)
            )));
        }
        if let Some(item) = reset_item.load() {
            item.setTitle(&objc2_foundation::NSString::from_str(if custom {
                "Clear This Mode…"
            } else {
                "Restore Default Shortcuts…"
            }));
        }
        for item in &managed {
            item.set_enabled(custom);
        }
    });
    let management = Popup::actions(&ui, "Shortcut Set Actions", menu);
    let weak = Rc::downgrade(&page);
    let search = SearchField::new(&ui).placeholder("Filter Shortcuts…").on_change(move |value| {
        if let Some(page) = weak.upgrade() {
            *page.query.borrow_mut() = search_query(&value);
            page.filter();
        }
    });
    search.accessibility_label("Filter shortcuts");
    let weak = Rc::downgrade(&page);
    let add = Button::new(&ui, "Add").symbol("plus").on_click(move || {
        if let Some(page) = weak.upgrade() {
            page.open_catalog();
        }
    });
    add.accessibility_label("Add Shortcut…");
    let weak = Rc::downgrade(&page);
    let view = Rc::new(cgs::tabs::TabSelector::new(&ui, &["Assigned", "All"]).on_change(
        move |index| {
            if let Some(page) = weak.upgrade() {
                page.all_actions.set(index == 1);
                if index == 1 {
                    if let Some(model) = page.model.upgrade() {
                        page.rebuild(&model.source.borrow());
                    }
                } else {
                    page.filter();
                }
            }
        },
    ));
    view.set_selected(0);
    view.accessibility_label("Shortcut visibility");
    view.tooltip("Show assigned shortcuts or all available actions");
    add.ns_view()
        .setToolTip(Some(&objc2_foundation::NSString::from_str("Add Shortcut…")));
    management.tooltip("Manage modes and restore shortcuts");
    let filters = Style::filters(&ui, search, view.clone());
    let toolbar = Rc::new(Style::toolbar(&ui, add, management));
    let (weak, tabs) = (Rc::downgrade(&page), Rc::downgrade(&view));
    let browse = Button::new(&ui, "Browse Actions").push().on_click(move || {
        if let Some(page) = weak.upgrade() {
            page.all_actions.set(true);
            if let Some(tabs) = tabs.upgrade() {
                tabs.set_selected(1);
            }
            if let Some(model) = page.model.upgrade() {
                page.rebuild(&model.source.borrow());
            }
        }
    });
    let weak = Rc::downgrade(&page);
    let first = Button::new(&ui, "Add Shortcut…").push().on_click(move || {
        if let Some(page) = weak.upgrade() {
            page.open_catalog();
        }
    });
    let weak = Rc::downgrade(&page);
    let defaults = Button::new(&ui, "Use Default Shortcuts…").borderless().on_click(move || {
        if let Some(page) = weak.upgrade() {
            page.reset_mode();
        }
    });
    page.welcome.add(Style::welcome(
        &ui,
        first,
        browse,
        defaults,
        page.welcome_defaults.clone(),
    ));
    let combinations = modifier_combinations(&mut f, model);
    *page.advanced.borrow_mut() = Some(Rc::new(Style::advanced(
        &ui,
        "Advanced: reusable modifier combinations",
        combinations,
    )));
    let body = Style::body(&ui, table);
    let mut result = f.finish(cgs::settings::ScrollHeader::new(&ui, filters, body));
    result.header = Some(toolbar);
    timing::finish(phase, "keyboard page shell");
    result
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_name_parameters_and_commands_round_trip_as_toml() {
        let s = defaults();
        let title = |key: &str| super::title(&s.keys[key], s, &[]);
        assert_eq!(title("Alt + H"), "Focus window Left");
        assert_eq!(title("comb1 + 1"), "Move window to workspace second");
        assert_eq!(title("Alt + Shift + Equal"), "Grow window Horizontal");
        assert_eq!(title("Alt + Ctrl + S"), "Serialize state");
        assert_eq!(
            title("Alt + Enter"),
            r#"Run command open -a "/System/Applications/Utilities/Terminal.app""#
        );
        let mut named = s.clone();
        named.virtual_workspaces.workspace_names = vec!["Mail".into(), String::new()];
        let switch = |i| wm(WmCmd::SwitchToWorkspace(WorkspaceSelector::Index(i)));
        assert_eq!(super::title(&switch(0), &named, &[]), "Switch to workspace Mail");
        assert_eq!(super::title(&switch(1), &named, &[]), "Switch to workspace 2");
        let floating = parse_toml("{ toggle_window_floating = { center = true } }").unwrap();
        assert_eq!(super::title(&floating, s, &[]), "Toggle floating centered");
        assert_eq!(template(&floating), template(&layout(L::ToggleWindowFloating)));
        assert_eq!(parse_toml(&toml_text(&floating)).unwrap(), floating);
        let resize = layout(L::ResizeWindowBy { amount: 50.0 });
        assert_eq!(super::title(&resize, s, &[]), "Resize window by 50");
        let sequence = WmCommand::Sequence(vec![s.keys["Alt + H"].clone(), resize]);
        assert_eq!(
            super::title(&sequence, s, &[]),
            "Focus window Left → Resize window by 50"
        );
        // Commands without a template show, and edit, as their TOML value.
        let save = reactor(R::SaveLayout {
            path: "/tmp/layout.json".into(),
        });
        assert_eq!(
            super::title(&save, s, &[]),
            "Save layout to file /tmp/layout.json"
        );
        for command in actions().iter().map(|(_, _, c)| c).chain([&save, &sequence]) {
            if *command != WmCommand::Sequence(Vec::new()) {
                assert_eq!(parse_toml(&toml_text(command)).as_ref(), Ok(command));
            }
        }
        assert!(parse_toml("not toml").is_err());
        // Glyphs follow macOS modifier order and expand modifier combinations.
        assert_eq!(glyphs("Alt + Ctrl + E", s), "⌃⌥E");
        assert_eq!(glyphs("comb1 + H", s), "⌥⇧H");
        assert_eq!(glyphs("Alt + Shift + Left", s), "⌥⇧←");
    }

    #[test]
    fn every_template_is_listed_once_in_a_section() {
        for (i, (section, _, command)) in actions().iter().enumerate() {
            assert!(*section < SECTIONS.len());
            if !matches!(command, WmCommand::Sequence(_)) {
                assert_eq!(template(command), Some(i), "{command:?} shares a template");
            }
        }
    }
}
