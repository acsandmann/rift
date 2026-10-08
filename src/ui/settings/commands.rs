use rift_protocol::{Direction, DisplaySelector, LayoutCommand as L, ReactorCommand as R, ResizeOrientation};

use super::*;
use crate::actor::reactor::Command;
use crate::actor::wm_controller::{ConfiguredLayoutCommand, ExecCmd, WmCmd, WmCommand};
use crate::common::config::{ConfigSource, WorkspaceSelector};

fn layout(cmd: L) -> WmCommand { WmCommand::ReactorCommand(Command::Layout(cmd)) }
fn reactor(cmd: R) -> WmCommand { WmCommand::ReactorCommand(Command::Reactor(cmd)) }
fn actions() -> Vec<(&'static str, WmCommand)> {
    vec![
        ("Focus · Move focus", layout(L::MoveFocus(Direction::Left))),
        ("Focus · Next window", layout(L::NextWindow)),
        ("Focus · Previous window", layout(L::PrevWindow)),
        ("Focus · Ascend", layout(L::Ascend)),
        ("Focus · Descend", layout(L::Descend)),
        ("Window · Move window", layout(L::MoveNode(Direction::Left))),
        ("Window · Join window", layout(L::JoinWindow(Direction::Left))),
        (
            "Window · Consume or expel",
            layout(L::ConsumeOrExpelWindow(Direction::Left)),
        ),
        ("Window · Toggle floating", layout(L::ToggleWindowFloating)),
        ("Window · Toggle focus floating", layout(L::ToggleFocusFloating)),
        ("Window · Fullscreen", layout(L::ToggleFullscreen)),
        (
            "Window · Fullscreen within gaps",
            layout(L::ToggleFullscreenWithinGaps),
        ),
        (
            "Window · Grow",
            layout(L::ResizeWindowGrow(ResizeOrientation::Smart)),
        ),
        (
            "Window · Shrink",
            layout(L::ResizeWindowShrink(ResizeOrientation::Smart)),
        ),
        ("Window · Resize by", layout(L::ResizeWindowBy { amount: 10.0 })),
        ("Window · Center selection", layout(L::CenterSelection)),
        ("Window · Close", WmCommand::Wm(WmCmd::CloseWindow)),
        (
            "Workspace · Switch workspace",
            WmCommand::Wm(WmCmd::SwitchToWorkspace(WorkspaceSelector::Index(0))),
        ),
        (
            "Workspace · Move window to workspace",
            WmCommand::Wm(WmCmd::MoveWindowToWorkspace(WorkspaceSelector::Index(0))),
        ),
        ("Workspace · Next", WmCommand::Wm(WmCmd::NextWorkspace)),
        ("Workspace · Previous", WmCommand::Wm(WmCmd::PrevWorkspace)),
        ("Workspace · Create", WmCommand::Wm(WmCmd::CreateWorkspace)),
        (
            "Workspace · Last workspace",
            WmCommand::Wm(WmCmd::SwitchToLastWorkspace),
        ),
        (
            "Workspace · Set layout",
            layout(L::SetWorkspaceLayout {
                workspace: None,
                mode: rift_protocol::LayoutMode::Traditional,
            }),
        ),
        (
            "Display · Focus display",
            reactor(R::FocusDisplay(DisplaySelector::Direction(Direction::Left))),
        ),
        (
            "Display · Move window to display",
            reactor(R::MoveWindowToDisplay {
                selector: DisplaySelector::Direction(Direction::Left),
                window_id: None,
            }),
        ),
        (
            "Display · Move workspace to display",
            reactor(R::MoveWorkspaceToDisplay {
                selector: DisplaySelector::Direction(Direction::Left),
                wrap_around: false,
            }),
        ),
        (
            "Display · Move pointer to display",
            reactor(R::MoveMouseToDisplay(DisplaySelector::Direction(
                Direction::Left,
            ))),
        ),
        (
            "Space · Switch macOS space",
            reactor(R::SwitchSpace(Direction::Left)),
        ),
        (
            "Space · Toggle tiling",
            WmCommand::Wm(WmCmd::ToggleSpaceActivated),
        ),
        ("Layout · Toggle stack", layout(L::ToggleStack)),
        ("Layout · Toggle orientation", layout(L::ToggleOrientation)),
        ("Layout · Unjoin windows", layout(L::UnjoinWindows)),
        ("Layout · Scroll strip", layout(L::ScrollStrip { delta: 100.0 })),
        ("Layout · Snap strip", layout(L::SnapStrip)),
        (
            "Layout · Next preset width",
            WmCommand::ConfiguredLayout(ConfiguredLayoutCommand::SwitchPresetColumnWidth),
        ),
        (
            "Layout · Adjust master ratio",
            layout(L::AdjustMasterRatio(0.05)),
        ),
        (
            "Layout · Adjust master count",
            layout(L::AdjustMasterCount { delta: 1 }),
        ),
        ("Layout · Promote to master", layout(L::PromoteToMaster)),
        ("Layout · Swap master and stack", layout(L::SwapMasterStack)),
        (
            "Overview · All workspaces",
            WmCommand::Wm(WmCmd::ShowMissionControlAll),
        ),
        (
            "Overview · Current workspace",
            WmCommand::Wm(WmCmd::ShowMissionControlCurrent),
        ),
        ("Overview · Dismiss", WmCommand::Wm(WmCmd::DismissMissionControl)),
        (
            "Rift · Binding mode",
            WmCommand::Wm(WmCmd::BindingMode("default".into())),
        ),
        (
            "Rift · Run command",
            WmCommand::Wm(WmCmd::Exec(ExecCmd::String(String::new()))),
        ),
        ("Rift · Reload config", WmCommand::Wm(WmCmd::ReloadConfig)),
        ("Rift · Open Settings", reactor(R::OpenSettings)),
        ("Rift · Save and exit", reactor(R::SaveAndExit)),
    ]
}
fn same_action(a: &WmCommand, b: &WmCommand) -> bool {
    use std::mem::discriminant as d;
    match (a, b) {
        (WmCommand::Wm(a), WmCommand::Wm(b)) => d(a) == d(b),
        (WmCommand::ConfiguredLayout(a), WmCommand::ConfiguredLayout(b)) => d(a) == d(b),
        (WmCommand::ReactorCommand(Command::Layout(a)), WmCommand::ReactorCommand(Command::Layout(b))) => {
            d(a) == d(b)
        }
        (WmCommand::ReactorCommand(Command::Reactor(a)), WmCommand::ReactorCommand(Command::Reactor(b))) => {
            d(a) == d(b)
        }
        _ => false,
    }
}
fn action_name(cmd: &WmCommand) -> String {
    let title = actions()
        .into_iter()
        .find(|(_, a)| same_action(a, cmd))
        .map(|(name, _)| name.split_once(" · ").map_or(name, |(_, action)| action).to_string())
        .unwrap_or("Custom command".into());
    let argument = match cmd {
        WmCommand::ReactorCommand(Command::Layout(
            L::MoveFocus(d) | L::MoveNode(d) | L::JoinWindow(d) | L::ConsumeOrExpelWindow(d),
        )) => format!("{d:?}"),
        WmCommand::Wm(WmCmd::SwitchToWorkspace(w) | WmCmd::MoveWindowToWorkspace(w)) => match w {
            WorkspaceSelector::Index(i) => (i + 1).to_string(),
            WorkspaceSelector::Name(n) => n.clone(),
        },
        WmCommand::Wm(WmCmd::BindingMode(n)) => n.clone(),
        WmCommand::ReactorCommand(Command::Reactor(
            R::FocusDisplay(target)
            | R::MoveMouseToDisplay(target)
            | R::MoveWindowToDisplay { selector: target, .. }
            | R::MoveWorkspaceToDisplay { selector: target, .. },
        )) => match target {
            DisplaySelector::Direction(direction) => format!("{direction:?}"),
            DisplaySelector::Index(index) => (index + 1).to_string(),
            DisplaySelector::Uuid(_) => "(specific display)".into(),
        },
        _ => String::new(),
    };
    if argument.is_empty() {
        title
    } else {
        format!("{title} {argument}")
    }
}

fn glyphs(key: &str, s: &ConfigSource) -> String {
    fn tokens(key: &str) -> String {
        key.split('+')
            .map(|part| match part.trim() {
                "Alt" | "Option" => "⌥",
                "Shift" => "⇧",
                "Ctrl" | "Control" => "⌃",
                "Meta" | "Cmd" | "Command" => "⌘",
                "Left" | "ArrowLeft" => "←",
                "Right" | "ArrowRight" => "→",
                "Up" | "ArrowUp" => "↑",
                "Down" | "ArrowDown" => "↓",
                "Comma" => ",",
                "Slash" => "/",
                "Equal" => "=",
                "Minus" => "−",
                "Enter" | "Return" => "↩",
                "Tab" => "⇥",
                "Space" => "␣",
                value => value,
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
    key.split('+')
        .map(|part| {
            s.modifier_combinations
                .get(part.trim())
                .map(|v| tokens(v))
                .unwrap_or_else(|| tokens(part))
        })
        .collect::<Vec<_>>()
        .join(" ")
}
pub(super) fn recorded_key(key: &KeyShortcut) -> Option<String> {
    let code = crate::sys::hotkey::cg_keycode_to_keycode(key.key.code)?;
    let mut modifiers = Vec::new();
    for (flag, name) in [
        (Modifiers::Control, "Ctrl"),
        (Modifiers::Option, "Alt"),
        (Modifiers::Shift, "Shift"),
        (Modifiers::Command, "Meta"),
    ] {
        if key.modifiers.contains(flag) {
            modifiers.push(name.to_string());
        }
    }
    modifiers.push(code.to_string());
    Some(modifiers.join(" + "))
}

pub(super) fn keyboard(ui: Ui, model: &Rc<Model>) -> Page {
    let mut f = FormBuilder::new(ui, model);
    let mode = Rc::new(RefCell::new("default".to_string()));
    let query = Rc::new(RefCell::new(String::new()));
    let table =
        SettingsList::<(String, String)>::new(&ui, |(_, action)| action.clone(), |(key, _)| key.clone())
            .full_length()
            .trailing_summary()
            .empty_message("No shortcuts in this shortcut set");
    let weak_model = Rc::downgrade(model);
    let mode_edit = mode.clone();
    let edit_query = query.clone();
    let edit_binding: Rc<dyn Fn(usize)> = Rc::new(move |i| {
        if let Some(model) = weak_model.upgrade() {
            let item = matching_bindings(&model.source.borrow(), &mode_edit.borrow(), &edit_query.borrow())
                .nth(i)
                .map(|(key, command)| (key.clone(), command.clone()));
            if let Some((key, cmd)) = item {
                binding_sheet(ui, &model, mode_edit.borrow().clone(), Some(key), cmd);
            }
        }
    });
    let action = edit_binding.clone();
    let table = Rc::new(table.on_open(move |i| action(i)));
    table.min_height(80.0);
    table.flexible_height(160.0);
    let names = Rc::new(RefCell::new(vec!["default".to_string()]));
    let selected_mode = mode.clone();
    let mode_names = names.clone();
    let weak_model = Rc::downgrade(model);
    let weak_table = Rc::downgrade(&table);
    let mode_query = query.clone();
    let popup = Rc::new(Popup::new(&ui).on_change(move |i| {
        if let Some(name) = mode_names.borrow().get(i) {
            *selected_mode.borrow_mut() = name.clone();
        }
        if let (Some(model), Some(table)) = (weak_model.upgrade(), weak_table.upgrade()) {
            table.set_rows_if_changed(binding_rows(
                &model.source.borrow(),
                &selected_mode.borrow(),
                &mode_query.borrow(),
            ));
        }
    }));
    let weak_popup = Rc::downgrade(&popup);
    let weak_table = Rc::downgrade(&table);
    let current_mode = mode.clone();
    let sync_query = query.clone();
    let last = RefCell::new(Vec::new());
    f.sync.push(Box::new(move |s| {
        let mut modes = vec!["default".to_string()];
        modes.extend(s.binding_modes.keys().cloned());
        if !modes.contains(&current_mode.borrow()) {
            *current_mode.borrow_mut() = "default".into();
        }
        if let Some(popup) = weak_popup.upgrade() {
            popup.set_items(modes.iter().map(|v| if v == "default" { "Default" } else { v.as_str() }));
            popup.set_selected(modes.iter().position(|v| v == &*current_mode.borrow()).unwrap_or(0));
        }
        *names.borrow_mut() = modes;
        let rows = binding_rows(s, &current_mode.borrow(), &sync_query.borrow());
        if *last.borrow() != rows {
            if let Some(table) = weak_table.upgrade() {
                table.set_rows(rows.clone());
            }
            *last.borrow_mut() = rows;
        }
    }));
    let weak_model = Rc::downgrade(model);
    let selected_mode = mode.clone();
    let controls = AddRemoveControl::new(&ui).on_add(move || {
        if let Some(model) = weak_model.upgrade() {
            binding_sheet(
                ui,
                &model,
                selected_mode.borrow().clone(),
                None,
                layout(L::MoveFocus(Direction::Left)),
            );
        }
    });
    let weak_table = Rc::downgrade(&table);
    let edit_action = edit_binding.clone();
    let edit = IconButton::new(&ui, "pencil", "Edit Shortcut…").on_click(move || {
        if let Some(i) = weak_table.upgrade().and_then(|table| table.selection()) {
            edit_action(i);
        }
    });
    let error = Rc::new(ValidationMessage::new(&ui));
    let weak_error = Rc::downgrade(&error);
    let weak_model = Rc::downgrade(model);
    let weak_table = Rc::downgrade(&table);
    let selected_mode = mode.clone();
    let remove_query = query.clone();
    let remove_binding: Rc<dyn Fn()> = Rc::new(move || {
        if let (Some(model), Some(table)) = (weak_model.upgrade(), weak_table.upgrade()) {
            if let Some(i) = table.selection() {
                let name = selected_mode.borrow().clone();
                let key = matching_bindings(&model.source.borrow(), &name, &remove_query.borrow())
                    .nth(i)
                    .map(|(key, _)| key.clone());
                if let Some(key) = key {
                    Model::submit(
                        &weak_model,
                        Box::new(move |s| {
                            s.keymap_mut(&name)?.remove(&key);
                            Ok(())
                        }),
                        weak_error.clone(),
                    );
                }
            }
        }
    });
    let action = remove_binding.clone();
    let controls = Rc::new(controls.on_remove(move || action()));
    let weak_table = Rc::downgrade(&table);
    let duplicate_table = Rc::downgrade(&table);
    let duplicate_model = Rc::downgrade(model);
    let duplicate_mode = mode.clone();
    let duplicate_query = query.clone();
    let menu = Menu::new(&ui)
        .item(MenuItem::new(&ui, "Edit Shortcut…").on_click(move || {
            if let Some(index) = weak_table.upgrade().and_then(|table| table.selection()) {
                edit_binding(index);
            }
        }))
        .item(MenuItem::new(&ui, "Duplicate").on_click(move || {
            if let (Some(model), Some(table)) = (duplicate_model.upgrade(), duplicate_table.upgrade()) {
                let mode = duplicate_mode.borrow().clone();
                let command = table.selection().and_then(|index| {
                    matching_bindings(&model.source.borrow(), &mode, &duplicate_query.borrow())
                        .nth(index)
                        .map(|(_, command)| command.clone())
                });
                if let Some(command) = command {
                    binding_sheet(ui, &model, mode, None, command);
                }
            }
        }))
        .item(MenuItem::new(&ui, "Remove").on_click(move || remove_binding()));
    table.set_context_menu(menu);
    controls.set_remove_enabled(false);
    edit.set_enabled(false);
    let weak_controls = Rc::downgrade(&controls);
    let weak_edit = WeakView::new(&edit);
    table.set_on_select(move |selection| {
        if let Some(controls) = weak_controls.upgrade() {
            controls.set_remove_enabled(selection.is_some());
        }
        weak_edit.set_enabled(selection.is_some());
    });
    let menu = Menu::new(&ui);
    let mut managed = Vec::new();
    for (title, operation) in [
        ("New Shortcut Set…", KeymapOperation::Create),
        ("Rename Shortcut Set…", KeymapOperation::Rename),
        ("Delete Shortcut Set…", KeymapOperation::Delete),
    ] {
        let weak_model = Rc::downgrade(model);
        let mode = mode.clone();
        let item = MenuItem::new(&ui, title).on_click(move || {
            if let Some(model) = weak_model.upgrade() {
                mode_sheet(ui, &model, mode.borrow().clone(), operation);
            }
        });
        if operation != KeymapOperation::Create {
            managed.push(item.weak());
        }
        menu.add(item);
    }
    let current_mode = mode.clone();
    let menu = menu.on_tracking(move |_| {
        for item in &managed {
            item.set_enabled(*current_mode.borrow() != "default");
        }
    });
    let management = Popup::actions(&ui, "Shortcut Set Actions", menu);
    let combinations = modifier_combinations(&mut f, model);
    let search_model = Rc::downgrade(model);
    let search_table = Rc::downgrade(&table);
    let search = SearchField::new(&ui).placeholder("Search shortcuts").on_change(move |value| {
        *query.borrow_mut() = value.to_lowercase();
        if let (Some(model), Some(table)) = (search_model.upgrade(), search_table.upgrade()) {
            table.set_rows_if_changed(binding_rows(
                &model.source.borrow(),
                &mode.borrow(),
                &query.borrow(),
            ));
        }
    });
    let header = Rc::new(HeaderControls::selector("Shortcut set", popup, search).actions(management));
    let mut page = f.finish(
        SettingsPage::new(&ui, "")
            .content_width(760.0)
            .section(
                Section::new(&ui, "Keyboard shortcuts")
                    .content(table)
                    .footer(error)
                    .footer(Caption::new(&ui, "⌘ Command   ⌥ Option   ⌃ Control   ⇧ Shift")),
            )
            .bottom_bar(HStack::new(&ui).push(controls).spacer(&ui).push(edit))
            .section(Disclosure::new(
                &ui,
                "Advanced: reusable modifier combinations",
                combinations,
            )),
    );
    page.header = Some(header);
    page
}

fn matching_bindings<'a>(
    s: &'a ConfigSource,
    mode: &str,
    query: &'a str,
) -> impl Iterator<Item = (&'a String, &'a WmCommand)> {
    s.keymap(mode)
        .into_iter()
        .flat_map(|map| map.iter())
        .filter(move |(key, command)| {
            query.is_empty()
                || action_name(command).to_lowercase().contains(query)
                || glyphs(key, s).to_lowercase().contains(query)
                || key.to_lowercase().contains(query)
        })
}
fn binding_rows(s: &ConfigSource, mode: &str, query: &str) -> Vec<(String, String)> {
    matching_bindings(s, mode, query)
        .map(|(key, cmd)| (glyphs(key, s), action_name(cmd)))
        .collect()
}

fn save_sheet(model: &Weak<Model>, edit: SourceEdit, message: &Rc<ValidationMessage>) {
    if let Some(model) = model.upgrade() {
        let weak = Rc::downgrade(&model);
        let error = Rc::downgrade(message);
        model.submit_edit(
            edit,
            Box::new(move |result| {
                if let Some(message) = error.upgrade() {
                    message.set_validation(&match &result {
                        Ok(_) => Validation::None,
                        Err(e) => Validation::Error(e.clone()),
                    });
                }
                if let Some(model) = weak.upgrade() {
                    if result.is_ok() {
                        model.close_sheet();
                    } else if let Some(sheet) = model.sheet.borrow().as_ref() {
                        sheet.fit_content();
                    }
                }
            }),
        );
    }
}
fn show_sheet(ui: Ui, model: &Rc<Model>, title: &str, content: impl NativeView) {
    content.min_width(460.0);
    let sheet = Sheet::new(&ui, title, content);
    sheet.fit_content();
    if sheet.show(&model.window.borrow()) {
        *model.sheet.borrow_mut() = Some(sheet);
    }
}
fn sheet_actions(ui: Ui, model: &Rc<Model>) -> Rc<SheetActions> {
    let weak = Rc::downgrade(model);
    let actions = Rc::new(SheetActions::new(&ui));
    actions.set_on_cancel(move || {
        if let Some(model) = weak.upgrade() {
            model.close_sheet();
        }
    });
    actions
}
fn binding_sheet(ui: Ui, model: &Rc<Model>, mode: String, old: Option<String>, command: WmCommand) {
    let cancel = sheet_actions(ui, model);
    let key = Rc::new(RefCell::new(old.clone()));
    let draft = Rc::new(RefCell::new(command.clone()));
    let dirty_key = key.clone();
    let dirty_command = draft.clone();
    let initial_key = old.clone();
    let initial_command = command.clone();
    let weak_cancel = Rc::downgrade(&cancel);
    let changed: Rc<dyn Fn()> = Rc::new(move || {
        if let Some(cancel) = weak_cancel.upgrade() {
            cancel.set_changed(
                *dirty_key.borrow() != initial_key || *dirty_command.borrow() != initial_command,
            );
        }
    });
    let key_changed = changed.clone();
    let key_cb = key.clone();
    let recorder = KeyRecorder::new(&ui).on_change(move |v| {
        *key_cb.borrow_mut() = v.as_ref().and_then(recorded_key);
        key_changed();
    });
    if let Some(old) = &old {
        recorder.set_title(&glyphs(old, &model.source.borrow()));
    }
    let host = Rc::new(PageHost::new(&ui));
    host.set_page(argument_editor(ui, model, &draft, &changed));
    let weak_host = Rc::downgrade(&host);
    let weak_model = Rc::downgrade(model);
    let edited = draft.clone();
    let action_changed = changed.clone();
    let mut descriptors = actions();
    let current = descriptors
        .iter()
        .position(|(_, c)| same_action(c, &command))
        .unwrap_or_else(|| {
            descriptors.push(("Current custom action", command.clone()));
            descriptors.len() - 1
        });
    let templates: Vec<_> = descriptors.iter().map(|(_, c)| c.clone()).collect();
    let popup = Popup::new(&ui)
        .items(descriptors.iter().map(|(title, _)| *title))
        .on_change(move |i| {
            *edited.borrow_mut() = templates[i].clone();
            action_changed();
            if let (Some(host), Some(model)) = (weak_host.upgrade(), weak_model.upgrade()) {
                host.set_page(argument_editor(ui, &model, &edited, &action_changed));
                if let Some(sheet) = model.sheet.borrow().as_ref() {
                    sheet.fit_content();
                }
            }
        });
    popup.set_selected(current);
    let message = Rc::new(ValidationMessage::new(&ui));
    let error = Rc::downgrade(&message);
    let weak_model = Rc::downgrade(model);
    cancel.set_primary_title("Save Shortcut");
    cancel.set_on_done(move || {
        let Some(message) = error.upgrade() else {
            return;
        };
        let Some(key) = key.borrow().clone() else {
            message.set_validation(&Validation::Error("Record a shortcut first".into()));
            if let Some(model) = weak_model.upgrade() {
                if let Some(sheet) = model.sheet.borrow().as_ref() {
                    sheet.fit_content();
                }
            }
            return;
        };
        let cmd = draft.borrow().clone();
        let old = old.clone();
        let mode = mode.clone();
        save_sheet(
            &weak_model,
            Box::new(move |s| {
                let bindings = s.keymap_mut(&mode)?;
                if old.as_ref() != Some(&key) && bindings.contains_key(&key) {
                    return Err("This shortcut is already assigned in this shortcut set".into());
                }
                if let Some(old) = old {
                    bindings.remove(&old);
                }
                bindings.insert(key, cmd);
                Ok(())
            }),
            &message,
        );
    });
    show_sheet(
        ui,
        model,
        "Shortcut",
        VStack::new(&ui)
            .insets(Insets {
                top: 20.0,
                left: 20.0,
                bottom: 20.0,
                right: 20.0,
            })
            .push(SectionTitle::new(&ui, "Keyboard Shortcut"))
            .push(Caption::new(
                &ui,
                "Click the shortcut, then press the keys you want to use.",
            ))
            .push(
                Section::new(&ui, "")
                    .row(SettingsRow::new(&ui, "Shortcut", recorder))
                    .row(SettingsRow::new(&ui, "Action", popup)),
            )
            .push(host)
            .push(message)
            .push(cancel),
    );
}

fn argument_editor(
    ui: Ui,
    model: &Rc<Model>,
    draft: &Rc<RefCell<WmCommand>>,
    changed: &Rc<dyn Fn()>,
) -> Section {
    let command = draft.borrow().clone();
    let mut view = Section::new(&ui, "");
    use WmCommand::{ReactorCommand as RcCommand, Wm};
    match command.clone() {
        RcCommand(Command::Layout(
            L::MoveFocus(d) | L::MoveNode(d) | L::JoinWindow(d) | L::ConsumeOrExpelWindow(d),
        ))
        | RcCommand(Command::Reactor(R::SwitchSpace(d))) => {
            let edited = draft.clone();
            let changed = changed.clone();
            let directions = [Direction::Left, Direction::Right, Direction::Up, Direction::Down];
            let popup = Popup::new(&ui).items(["Left", "Right", "Up", "Down"]).on_change(move |i| {
                let d = directions[i];
                *edited.borrow_mut() = match &command {
                    RcCommand(Command::Layout(L::MoveFocus(_))) => layout(L::MoveFocus(d)),
                    RcCommand(Command::Layout(L::MoveNode(_))) => layout(L::MoveNode(d)),
                    RcCommand(Command::Layout(L::JoinWindow(_))) => layout(L::JoinWindow(d)),
                    RcCommand(Command::Layout(L::ConsumeOrExpelWindow(_))) => {
                        layout(L::ConsumeOrExpelWindow(d))
                    }
                    _ => reactor(R::SwitchSpace(d)),
                };

                changed();
            });
            popup.set_selected(directions.iter().position(|v| *v == d).unwrap_or(0));
            view = view.row(SettingsRow::new(&ui, "Direction", popup));
        }
        RcCommand(Command::Layout(L::ResizeWindowGrow(o) | L::ResizeWindowShrink(o))) => {
            let edited = draft.clone();
            let changed = changed.clone();
            let values = [
                ResizeOrientation::Smart,
                ResizeOrientation::Horizontal,
                ResizeOrientation::Vertical,
            ];
            let popup = Popup::new(&ui).items(["Smart", "Horizontal", "Vertical"]).on_change(move |i| {
                *edited.borrow_mut() =
                    if matches!(command, RcCommand(Command::Layout(L::ResizeWindowGrow(_)))) {
                        layout(L::ResizeWindowGrow(values[i]))
                    } else {
                        layout(L::ResizeWindowShrink(values[i]))
                    };

                changed();
            });
            popup.set_selected(values.iter().position(|v| *v == o).unwrap_or(0));
            view = view.row(SettingsRow::new(&ui, "Orientation", popup));
        }
        Wm(WmCmd::SwitchToWorkspace(target) | WmCmd::MoveWindowToWorkspace(target)) => {
            let names: Vec<_> = (0..model.source.borrow().virtual_workspaces.default_workspace_count)
                .map(|i| super::editors::workspace_name(&model.source.borrow(), i))
                .collect();
            let initial = match target {
                WorkspaceSelector::Index(i) => i,
                WorkspaceSelector::Name(name) => model
                    .source
                    .borrow()
                    .virtual_workspaces
                    .workspace_names
                    .iter()
                    .position(|v| *v == name)
                    .unwrap_or(0),
            };
            let edited = draft.clone();
            let changed = changed.clone();
            let popup = Popup::new(&ui).items(names).on_change(move |i| {
                let target = WorkspaceSelector::Index(i);
                *edited.borrow_mut() = Wm(if matches!(command, Wm(WmCmd::SwitchToWorkspace(_))) {
                    WmCmd::SwitchToWorkspace(target)
                } else {
                    WmCmd::MoveWindowToWorkspace(target)
                });

                changed();
            });
            popup.set_selected(initial);
            view = view.row(SettingsRow::new(&ui, "Workspace", popup));
        }
        RcCommand(Command::Layout(L::SetWorkspaceLayout { mode, workspace })) => {
            let values = super::pages::layouts();
            let edited = draft.clone();
            let changed = changed.clone();
            let modes: Vec<_> = values.iter().map(|(_, v)| *v).collect();
            let selected = modes.iter().position(|v| *v == mode).unwrap_or(0);
            let popup = Popup::new(&ui).items(values.iter().map(|(name, _)| *name)).on_change(move |i| {
                *edited.borrow_mut() = layout(L::SetWorkspaceLayout { workspace, mode: modes[i] });

                changed();
            });
            popup.set_selected(selected);
            view = view.row(SettingsRow::new(&ui, "Layout", popup));
        }
        RcCommand(Command::Reactor(
            R::FocusDisplay(target)
            | R::MoveMouseToDisplay(target)
            | R::MoveWindowToDisplay { selector: target, .. }
            | R::MoveWorkspaceToDisplay { selector: target, .. },
        )) => {
            let values = vec![
                DisplaySelector::Direction(Direction::Left),
                DisplaySelector::Direction(Direction::Right),
                DisplaySelector::Direction(Direction::Up),
                DisplaySelector::Direction(Direction::Down),
            ];
            let mut values = values;
            let mut names = vec!["Left".to_string(), "Right".into(), "Up".into(), "Down".into()];
            for display in model.displays.borrow().iter() {
                values.push(DisplaySelector::Uuid(display.display_uuid.clone()));
                names.push(
                    display
                        .name
                        .clone()
                        .unwrap_or_else(|| format!("Display {}", display.id.as_u32())),
                );
            }
            let selected = values.iter().position(|value| *value == target).unwrap_or_else(|| {
                values.push(target);
                names.push("Current display target".into());
                values.len() - 1
            });
            let edited = draft.clone();
            let changed = changed.clone();
            let popup = Popup::new(&ui).items(names).on_change(move |i| {
                let selector = values[i].clone();
                *edited.borrow_mut() = reactor(match &command {
                    RcCommand(Command::Reactor(R::FocusDisplay(_))) => R::FocusDisplay(selector),
                    RcCommand(Command::Reactor(R::MoveMouseToDisplay(_))) => R::MoveMouseToDisplay(selector),
                    RcCommand(Command::Reactor(R::MoveWorkspaceToDisplay { wrap_around, .. })) => {
                        R::MoveWorkspaceToDisplay {
                            selector,
                            wrap_around: *wrap_around,
                        }
                    }
                    RcCommand(Command::Reactor(R::MoveWindowToDisplay { window_id, .. })) => {
                        R::MoveWindowToDisplay {
                            selector,
                            window_id: *window_id,
                        }
                    }
                    _ => unreachable!(),
                });

                changed();
            });
            popup.set_selected(selected);
            view = view.row(SettingsRow::new(&ui, "Display direction", popup));
        }
        RcCommand(Command::Layout(
            L::ResizeWindowBy { amount } | L::ScrollStrip { delta: amount } | L::AdjustMasterRatio(amount),
        )) => {
            let edited = draft.clone();
            let changed = changed.clone();
            let input = NumberField::new(&ui).value(amount).on_edit(move |v| {
                *edited.borrow_mut() = match &command {
                    RcCommand(Command::Layout(L::ResizeWindowBy { .. })) => {
                        layout(L::ResizeWindowBy { amount: v })
                    }
                    RcCommand(Command::Layout(L::ScrollStrip { .. })) => layout(L::ScrollStrip { delta: v }),
                    _ => layout(L::AdjustMasterRatio(v)),
                };

                changed();
            });
            view = view.row(SettingsRow::new(&ui, "Amount", input));
        }
        RcCommand(Command::Layout(L::AdjustMasterCount { delta })) => {
            let edited = draft.clone();
            let changed = changed.clone();
            view = view.row(SettingsRow::new(
                &ui,
                "Change in window count",
                NumberField::new(&ui).integer().value(delta as f64).on_edit(move |v| {
                    *edited.borrow_mut() = layout(L::AdjustMasterCount { delta: v as i32 });

                    changed();
                }),
            ));
        }
        Wm(WmCmd::BindingMode(name)) => {
            let mut names = vec!["default".to_string()];
            names.extend(model.source.borrow().binding_modes.keys().cloned());
            let selected = names.iter().position(|v| *v == name).unwrap_or(0);
            let choices = names.clone();
            let edited = draft.clone();
            let changed = changed.clone();
            let popup = Popup::new(&ui).items(names).on_change(move |i| {
                *edited.borrow_mut() = Wm(WmCmd::BindingMode(choices[i].clone()));

                changed();
            });
            popup.set_selected(selected);
            view = view.row(SettingsRow::new(&ui, "Keymap", popup));
        }
        Wm(WmCmd::Exec(ExecCmd::String(text))) => {
            let edited = draft.clone();
            let changed = changed.clone();
            view = view.row(SettingsRow::new(
                &ui,
                "Shell command",
                TextField::new(&ui).value(&text).on_change(move |v| {
                    *edited.borrow_mut() = Wm(WmCmd::Exec(ExecCmd::String(v)));

                    changed();
                }),
            ));
        }
        _ if !actions().iter().any(|(_, template)| same_action(template, &command)) => {
            view = view.content(SecondaryLabel::new(
                &ui,
                "Existing arguments are retained. Choose another action to replace them.",
            ));
        }
        _ => {}
    }
    view
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum KeymapOperation {
    Create,
    Rename,
    Delete,
}

fn mode_sheet(ui: Ui, model: &Rc<Model>, old: String, operation: KeymapOperation) {
    if operation != KeymapOperation::Create && old == "default" {
        return;
    }
    let cancel = sheet_actions(ui, model);
    cancel.set_changed(operation == KeymapOperation::Delete);
    let weak_cancel = Rc::downgrade(&cancel);
    let initial = if operation == KeymapOperation::Rename {
        old.clone()
    } else {
        String::new()
    };
    let input = Rc::new(TextField::new(&ui).value(&initial).on_change(move |value| {
        if let Some(cancel) = weak_cancel.upgrade() {
            cancel.set_changed(value != initial);
        }
    }));
    let message = Rc::new(ValidationMessage::new(&ui));
    let weak_input = Rc::downgrade(&input);
    let error = Rc::downgrade(&message);
    let weak_model = Rc::downgrade(model);
    cancel.set_primary_title(if operation == KeymapOperation::Delete {
        "Delete Keymap"
    } else {
        "Save"
    });
    cancel.set_on_done(move || {
        let (Some(input), Some(message)) = (weak_input.upgrade(), error.upgrade()) else {
            return;
        };
        let name = input.get_value().trim().to_string();
        let old = old.clone();
        save_sheet(
            &weak_model,
            Box::new(move |s| match operation {
                KeymapOperation::Create => s.create_keymap(name),
                KeymapOperation::Rename => s.rename_keymap(&old, name),
                KeymapOperation::Delete => s.delete_keymap(&old),
            }),
            &message,
        );
    });
    let mut content = VStack::new(&ui).insets(Insets {
        top: 20.0,
        left: 20.0,
        bottom: 20.0,
        right: 20.0,
    });
    if operation == KeymapOperation::Delete {
        content = content.push(Label::new(
            &ui,
            "Delete this keymap and its shortcuts? References will switch to Default.",
        ));
    } else {
        content = content.push(SettingsRow::new(&ui, "Name", input));
    }
    show_sheet(ui, model, "Keymap", content.push(message).push(cancel));
}
fn modifier_combinations(f: &mut FormBuilder, model: &Rc<Model>) -> VStack {
    let ui = f.ui;
    let name = Rc::new(TextField::new(&ui).placeholder("Name"));
    let modifiers = Rc::new(RefCell::new(String::new()));
    let changed = modifiers.clone();
    let recorder = ModifierRecorder::new(&ui).on_change(move |flags| {
        let mut tokens = Vec::new();
        for (flag, name) in [
            (Modifiers::Control, "Ctrl"),
            (Modifiers::Option, "Alt"),
            (Modifiers::Shift, "Shift"),
            (Modifiers::Command, "Meta"),
        ] {
            if flags.contains(flag) {
                tokens.push(name);
            }
        }
        *changed.borrow_mut() = tokens.join(" + ");
    });
    let message = Rc::new(ValidationMessage::new(&ui));
    let error = Rc::downgrade(&message);
    let weak_model = Rc::downgrade(model);
    let weak_name = Rc::downgrade(&name);
    let add = Button::new(&ui, "Add / Update").on_click(move || {
        if let Some(name) = weak_name.upgrade() {
            let name = name.get_value().trim().to_string();
            let value = modifiers.borrow().clone();
            Model::submit(
                &weak_model,
                Box::new(move |s| {
                    if name.is_empty() || name.contains('+') || value.is_empty() {
                        return Err("Enter a name and record modifiers".into());
                    }
                    s.modifier_combinations.insert(name, value);
                    Ok(())
                }),
                error.clone(),
            );
        }
    });
    let weak_model = Rc::downgrade(model);
    let error = Rc::downgrade(&message);
    let list = Rc::new(
        EditableList::new(&ui, |v: &(String, String)| format!("{} · {}", v.0, v.1))
            .full_length()
            .on_remove(move |i| {
                if let Some(model) = weak_model.upgrade() {
                    let name = model.source.borrow().modifier_combinations.keys().nth(i).cloned();
                    if let Some(name) = name {
                        Model::submit(
                            &weak_model,
                            Box::new(move |s| {
                                s.modifier_combinations.remove(&name);
                                Ok(())
                            }),
                            error.clone(),
                        );
                    }
                }
            }),
    );
    let weak_list = Rc::downgrade(&list);
    f.sync.push(Box::new(move |s| {
        if let Some(list) = weak_list.upgrade() {
            list.set_items(s.modifier_combinations.iter().map(|(k, v)| (k.clone(), v.clone())).collect());
        }
    }));
    VStack::new(&ui)
        .push(list)
        .push(HStack::new(&ui).push(name).push(recorder).push(add))
        .push(message)
}
