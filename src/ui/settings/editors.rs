use super::pages::{layouts, optional_number};
use super::*;
use crate::common::config::*;

/// Keep detail controls and their sync closures scoped to the open Settings session.
pub(super) fn show_editor(
    ui: Ui,
    model: &Rc<Model>,
    title: &str,
    page: Page,
    current: &Rc<RefCell<Option<Page>>>,
) {
    let syncing = model.syncing.replace(true);
    for sync in &page.sync {
        sync(&model.source.borrow());
    }
    model.syncing.set(syncing);
    page.view.min_width(480.0);
    page.view.ns_view().layoutSubtreeIfNeeded();
    let height = page.view.ns_view().fittingSize().height.clamp(100.0, 440.0);
    let document = View::flipped(&ui).content(page.view.clone(), Insets {
        top: 0.0,
        left: 0.0,
        bottom: 0.0,
        right: 0.0,
    });
    let scroll = ScrollView::new(&ui, document);
    scroll.fit_width();
    scroll.height(height);
    let weak = Rc::downgrade(model);
    let editor = current.clone();
    let done = Button::new(&ui, "Done").key_equivalent("\r").on_click(move || {
        if let Some(model) = weak.upgrade() {
            if let Some(sheet) = model.sheet.borrow().as_ref() {
                sheet.end();
            }
        }
        *editor.borrow_mut() = None;
    });
    let content = VStack::new(&ui)
        .insets(Insets {
            top: 20.0,
            left: 20.0,
            bottom: 20.0,
            right: 20.0,
        })
        .push(SectionTitle::new(&ui, title))
        .push(scroll)
        .push(
            HStack::new(&ui)
                .push(Caption::new(&ui, "Changes save automatically."))
                .spacer(&ui)
                .push(done),
        );
    let sheet = Sheet::new(&ui, title, content);
    sheet.fit_content();
    if let Some(window) = model.window.borrow().load() {
        sheet.show(&window);
        *model.sheet.borrow_mut() = Some(sheet);
        *current.borrow_mut() = Some(page);
    }
}

/// Workspace records use a native collection and a small attached detail editor.
type WorkspaceEntry = (usize, String, Option<LayoutMode>);

fn workspace_editor(
    ui: Ui,
    model: &Rc<Model>,
    title: &str,
    items: impl Fn(&ConfigSource) -> Vec<WorkspaceEntry> + 'static,
    summary: impl Fn(&WorkspaceEntry, usize) -> String + 'static,
    detail: impl Fn(Ui, &Rc<Model>, usize) -> Page + 'static,
    add: impl Fn(&Rc<Model>, Weak<ValidationMessage>) + 'static,
    remove: impl Fn(&mut ConfigSource, usize) -> Result<(), String> + Send + Copy + 'static,
) -> (Page, Rc<AddRemoveControl>) {
    let mut f = FormBuilder::new(ui, model);
    let current: Rc<RefCell<Option<Page>>> = Rc::new(RefCell::new(None));
    let selected = Rc::new(Cell::new(None));
    let weak = Rc::downgrade(model);
    let editor = current.clone();
    let edit_workspace: Rc<dyn Fn(usize)> = Rc::new(move |index| {
        if let Some(model) = weak.upgrade() {
            let page = detail(ui, &model, index);
            let syncing = model.syncing.replace(true);
            for sync in &page.sync {
                sync(&model.source.borrow());
            }
            model.syncing.set(syncing);
            let weak = Rc::downgrade(&model);
            let current = editor.clone();
            let done = Button::new(&ui, "Done").key_equivalent("\r").on_click(move || {
                if let Some(model) = weak.upgrade() {
                    if let Some(sheet) = model.sheet.borrow().as_ref() {
                        sheet.end();
                    }
                }
                *current.borrow_mut() = None;
            });
            let content = VStack::new(&ui)
                .insets(Insets {
                    top: 16.0,
                    left: 16.0,
                    bottom: 16.0,
                    right: 16.0,
                })
                .push(SectionTitle::new(&ui, &format!("Workspace {}", index + 1)))
                .push(page.view.clone())
                .push(
                    HStack::new(&ui)
                        .push(Caption::new(&ui, "Changes save automatically."))
                        .spacer(&ui)
                        .push(done),
                );
            content.min_width(420.0);
            let sheet = Sheet::new(&ui, "Edit Workspace", content);
            sheet.fit_content();
            if let Some(window) = model.window.borrow().load() {
                sheet.show(&window);
                *model.sheet.borrow_mut() = Some(sheet);
                *editor.borrow_mut() = Some(page);
            }
        }
    });
    let summary = Rc::new(summary);
    let primary = summary.clone();
    let table = Rc::new(
        SettingsList::<WorkspaceEntry>::new(
            &ui,
            move |item| primary(item, 0),
            move |item| summary(item, 1),
        )
        .empty_message("No workspaces")
        .full_length()
        .symbol("rectangle.on.rectangle")
        .on_open(move |index| edit_workspace(index)),
    );
    let message = Rc::new(ValidationMessage::new(&ui));
    let weak = Rc::downgrade(model);
    let error = Rc::downgrade(&message);
    let weak_remove = weak.clone();
    let remove_error = error.clone();
    let remove_selected = selected.clone();
    let actions = Rc::new(
        AddRemoveControl::new(&ui)
            .on_add(move || {
                if let Some(model) = weak.upgrade() {
                    add(&model, error.clone());
                }
            })
            .on_remove(move || {
                if let Some(index) = remove_selected.get() {
                    FormBuilder::submit(
                        &weak_remove,
                        Box::new(move |s| remove(s, index)),
                        remove_error.clone(),
                    );
                }
            }),
    );
    let weak_actions = Rc::downgrade(&actions);
    let weak_model = Rc::downgrade(model);
    let selection = selected.clone();
    table.set_on_select(move |index| {
        selection.set(index);
        if let (Some(actions), Some(model)) = (weak_actions.upgrade(), weak_model.upgrade()) {
            actions.set_remove_enabled(index.is_some_and(|i| {
                i > 0 && i + 1 == model.source.borrow().virtual_workspaces.default_workspace_count
            }));
        }
    });
    let weak_table = Rc::downgrade(&table);
    let weak_actions = Rc::downgrade(&actions);
    let last = RefCell::new(Vec::new());
    f.sync.push(Box::new(move |source| {
        let values = items(source);
        if *last.borrow() != values {
            if let Some(table) = weak_table.upgrade() {
                let index = selected.get().filter(|i| *i < values.len());
                table.set_rows(values.clone());
                table.set_selected(index);
            }
            *last.borrow_mut() = values;
        }
        if let Some(actions) = weak_actions.upgrade() {
            actions.set_remove_enabled(selected.get().is_some_and(|i| {
                i > 0 && i + 1 == source.virtual_workspaces.default_workspace_count
            }));
        }
        if let Some(page) = current.borrow().as_ref() {
            for sync in &page.sync {
                sync(source);
            }
        }
    }));
    (
        f.finish(Section::new(&ui, title).content(table).footer(message)),
        actions,
    )
}

pub(super) fn workspaces(ui: Ui, model: &Rc<Model>) -> Page {
    let mut f = FormBuilder::new(ui, model);
    let mut section = Section::new(&ui, "Virtual Workspaces")
        .row(f.switch(
            "Enabled",
            |s| s.virtual_workspaces.enabled,
            |s, v| s.virtual_workspaces.enabled = v,
        ))
        .row(f.integer(
            "Workspace count",
            |s| s.virtual_workspaces.default_workspace_count as f64,
            |s, v| resize_workspaces(s, v as usize),
        ))
        .row(f.switch(
            "Auto-assign windows",
            |s| s.virtual_workspaces.auto_assign_windows,
            |s, v| s.virtual_workspaces.auto_assign_windows = v,
        ))
        .row(f.switch(
            "Preserve workspace focus",
            |s| s.virtual_workspaces.preserve_focus_per_workspace,
            |s, v| s.virtual_workspaces.preserve_focus_per_workspace = v,
        ))
        .row(f.switch(
            "Switch back when selecting active workspace",
            |s| s.virtual_workspaces.workspace_auto_back_and_forth,
            |s, v| s.virtual_workspaces.workspace_auto_back_and_forth = v,
        ))
        .row(f.switch(
            "Prevent wrapping",
            |s| s.virtual_workspaces.prevent_wrapping,
            |s, v| s.virtual_workspaces.prevent_wrapping = v,
        ))
        .row(f.switch(
            "Reapply rules when titles change",
            |s| s.virtual_workspaces.reapply_app_rules_on_title_change,
            |s, v| s.virtual_workspaces.reapply_app_rules_on_title_change = v,
        ));
    let defaults = workspace_popup(
        &mut f,
        "Default workspace",
        false,
        |s| Some(WorkspaceSelector::Index(s.virtual_workspaces.default_workspace)),
        |s, v| {
            if let Some(WorkspaceSelector::Index(i)) = v {
                s.virtual_workspaces.default_workspace = i;
            }
        },
    );
    section = section.row(defaults);
    let (editor, actions) = workspace_editor(
        ui,
        model,
        "Workspaces",
        |s| {
            (0..s.virtual_workspaces.default_workspace_count)
                .map(|i| {
                    (
                        i,
                        workspace_name(s, i),
                        Some(workspace_layout(s, i).unwrap_or(s.settings.layout.mode)),
                    )
                })
                .collect()
        },
        |(i, name, layout), column| {
            if column == 0 {
                name.clone()
            } else {
                let layout = layout
                    .as_ref()
                    .and_then(|mode| {
                        layouts()
                            .into_iter()
                            .find(|(_, value)| value == mode)
                            .map(|(name, _)| name.to_string())
                    })
                    .unwrap_or_else(|| "Default layout".into());
                if name == &format!("Workspace {}", i + 1) {
                    layout
                } else {
                    format!("Workspace {} · {}", i + 1, layout)
                }
            }
        },
        |ui, model, i| {
            let mut f = FormBuilder::new(ui, model);
            let name = f.text(
                "Name",
                move |s| s.virtual_workspaces.workspace_names.get(i).cloned().unwrap_or_default(),
                move |s, v| {
                    let old = s.virtual_workspaces.workspace_names.get(i).cloned();
                    if s.virtual_workspaces.workspace_names.len() <= i {
                        s.virtual_workspaces.workspace_names.resize(i + 1, String::new());
                    }
                    s.virtual_workspaces.workspace_names[i] = v.clone();
                    if let Some(old) = old.filter(|v| !v.is_empty()) {
                        for rule in &mut s.virtual_workspaces.workspace_rules {
                            if rule.workspace == WorkspaceSelector::Name(old.clone()) {
                                rule.workspace = WorkspaceSelector::Index(i);
                            }
                        }
                        for rule in &mut s.virtual_workspaces.app_rules {
                            if rule.workspace == Some(WorkspaceSelector::Name(old.clone())) {
                                rule.workspace = Some(WorkspaceSelector::Index(i));
                            }
                        }
                    }
                    while s.virtual_workspaces.workspace_names.last().is_some_and(String::is_empty)
                    {
                        s.virtual_workspaces.workspace_names.pop();
                    }
                    Ok(())
                },
            );
            let values = layouts();
            let layout = f.inherited_popup(
                "Layout",
                values,
                move |s| workspace_layout(s, i),
                |s| s.settings.layout.mode,
                move |s, mode| {
                    let name = s.virtual_workspaces.workspace_names.get(i).cloned();
                    s.virtual_workspaces
                        .workspace_rules
                        .retain(|r| !selector_matches(&r.workspace, i, name.as_deref()));
                    if let Some(layout) = mode {
                        s.virtual_workspaces.workspace_rules.push(WorkspaceLayoutRule {
                            workspace: WorkspaceSelector::Index(i),
                            layout,
                        });
                    }
                },
            );
            f.finish(Section::new(&ui, "").row(name).row(layout))
        },
        |model, error| {
            FormBuilder::submit(
                &Rc::downgrade(model),
                Box::new(|s| {
                    if s.virtual_workspaces.default_workspace_count >= MAX_WORKSPACES {
                        return Err(format!("Maximum is {MAX_WORKSPACES} workspaces"));
                    }
                    resize_workspaces(s, s.virtual_workspaces.default_workspace_count + 1);
                    Ok(())
                }),
                error,
            )
        },
        |s, i| {
            if i + 1 != s.virtual_workspaces.default_workspace_count {
                return Err("Remove the last workspace, or change Workspace count.".into());
            }
            if i == 0 {
                return Err("Keep at least one workspace.".into());
            }
            resize_workspaces(s, i);
            Ok(())
        },
    );
    let editor_sync = editor.sync;
    f.sync.extend(editor_sync);
    f.finish(
        SettingsPage::new(&ui, "")
            .section(section)
            .section(editor.view)
            .bottom_bar(HStack::new(&ui).push(actions).spacer(&ui)),
    )
}

fn resize_workspaces(s: &mut ConfigSource, count: usize) {
    let w = &mut s.virtual_workspaces;
    w.default_workspace_count = count;
    // Invalid counts are left for the shared validator, without large allocations.
    if !(1..=MAX_WORKSPACES).contains(&count) {
        return;
    }
    let removed = w.workspace_names.iter().skip(count).cloned().collect::<Vec<_>>();
    w.workspace_names.truncate(count);
    w.default_workspace = w.default_workspace.min(count - 1);
    let removed_selector = |v: &WorkspaceSelector| match v {
        WorkspaceSelector::Index(i) => *i >= count,
        WorkspaceSelector::Name(n) => removed.contains(n),
    };
    w.workspace_rules.retain(|r| !removed_selector(&r.workspace));
    for r in &mut w.app_rules {
        if r.workspace.as_ref().is_some_and(removed_selector) {
            r.workspace = None;
        }
    }
}
fn selector_matches(selector: &WorkspaceSelector, i: usize, name: Option<&str>) -> bool {
    match selector {
        WorkspaceSelector::Index(index) => *index == i,
        WorkspaceSelector::Name(n) => Some(n.as_str()) == name,
    }
}
fn workspace_layout(s: &ConfigSource, i: usize) -> Option<LayoutMode> {
    s.virtual_workspaces
        .workspace_rules
        .iter()
        .find(|r| {
            selector_matches(
                &r.workspace,
                i,
                s.virtual_workspaces.workspace_names.get(i).map(String::as_str),
            )
        })
        .map(|r| r.layout)
}
pub(super) fn workspace_name(s: &ConfigSource, i: usize) -> String {
    s.virtual_workspaces
        .workspace_names
        .get(i)
        .filter(|n| !n.is_empty())
        .cloned()
        .unwrap_or_else(|| format!("Workspace {}", i + 1))
}

fn workspace_popup(
    f: &mut FormBuilder,
    title: &str,
    optional: bool,
    get: impl Fn(&ConfigSource) -> Option<WorkspaceSelector> + 'static,
    set: impl Fn(&mut ConfigSource, Option<WorkspaceSelector>) + Copy + Send + 'static,
) -> SettingsRow {
    let message = Rc::new(ValidationMessage::new(&f.ui));
    let error = Rc::downgrade(&message);
    let model = f.model.clone();
    let input = Rc::new(Popup::new(&f.ui).on_change(move |i| {
        let target = if optional && i == 0 {
            None
        } else {
            Some(WorkspaceSelector::Index(i - usize::from(optional)))
        };
        FormBuilder::submit(
            &model,
            Box::new(move |s| {
                set(s, target);
                Ok(())
            }),
            error.clone(),
        );
    }));
    let weak = Rc::downgrade(&input);
    f.sync.push(Box::new(move |s| {
        if let Some(input) = weak.upgrade() {
            let mut names = Vec::new();
            if optional {
                names.push("Current workspace".into());
            }
            names.extend(
                (0..s.virtual_workspaces.default_workspace_count)
                    .map(|i| format!("{} · {}", i + 1, workspace_name(s, i))),
            );
            input.set_items(&names);
            let selection = match get(s) {
                Some(WorkspaceSelector::Index(i)) => i + usize::from(optional),
                Some(WorkspaceSelector::Name(n)) => s
                    .virtual_workspaces
                    .workspace_names
                    .iter()
                    .position(|v| *v == n)
                    .map(|i| i + usize::from(optional))
                    .unwrap_or(0),
                None => 0,
            };
            input.set_selected(selection);
        }
    }));
    f.row(title, input, message)
}

pub(super) fn rules(ui: Ui, model: &Rc<Model>) -> Page {
    let mut f = FormBuilder::new(ui, model);
    let message = Rc::new(ValidationMessage::new(&ui));
    let current: Rc<RefCell<Option<Page>>> = Rc::new(RefCell::new(None));
    let weak = Rc::downgrade(model);
    let editor = current.clone();
    let edit_rule: Rc<dyn Fn(usize)> = Rc::new(move |index| {
        if let Some(model) = weak.upgrade() {
            let page = rule_detail(ui, &model, index);
            let syncing = model.syncing.replace(true);
            for sync in &page.sync {
                sync(&model.source.borrow());
            }
            model.syncing.set(syncing);
            page.view.min_height(360.0);
            let axis = objc2_app_kit::NSLayoutConstraintOrientation::Vertical;
            page.view.ns_view().setContentHuggingPriority_forOrientation(1.0, axis);
            page.view
                .ns_view()
                .setContentCompressionResistancePriority_forOrientation(1.0, axis);
            let weak = Rc::downgrade(&model);
            let done = Button::new(&ui, "Done").key_equivalent("\r").on_click(move || {
                if let Some(model) = weak.upgrade() {
                    if let Some(sheet) = model.sheet.borrow().as_ref() {
                        sheet.end();
                    }
                }
            });
            let content = VStack::new(&ui).push(page.view.clone()).push(
                HStack::new(&ui)
                    .insets(Insets {
                        top: 0.0,
                        left: 20.0,
                        bottom: 12.0,
                        right: 20.0,
                    })
                    .push(Caption::new(&ui, "Changes save automatically."))
                    .spacer(&ui)
                    .push(done),
            );
            let sheet = Sheet::new(&ui, "Edit App Rule", content);
            sheet.ns_window().setContentSize(CGSize::new(600.0, 620.0));
            if let Some(window) = model.window.borrow().load() {
                sheet.show(&window);
                *model.sheet.borrow_mut() = Some(sheet);
                *editor.borrow_mut() = Some(page);
            }
        }
    });
    let action = edit_rule.clone();
    let app_names = Rc::downgrade(model);
    let table = Rc::new(
        SettingsList::<AppWorkspaceRule>::new(
            &ui,
            move |rule| {
                app_names
                    .upgrade()
                    .and_then(|model| {
                        model
                            .applications
                            .borrow()
                            .iter()
                            .find(|app| rule.app_id.is_some() && app.bundle_id == rule.app_id)
                            .map(|app| app.name.clone())
                            .or_else(|| {
                                model.installed_applications.borrow().as_ref().and_then(|apps| {
                                    apps.iter()
                                        .find(|(_, id)| rule.app_id.as_ref() == Some(id))
                                        .map(|(name, _)| name.clone())
                                })
                            })
                    })
                    .unwrap_or_else(|| rule_summary(rule))
            },
            rule_behavior,
        )
        .empty_message("No app rules")
        .full_length()
        .symbol("app")
        .images(application_icons(|rule: &AppWorkspaceRule| rule.app_id.clone()))
        .on_open(move |index| action(index))
        .reorderable(true)
        .on_reorder({
            let weak = Rc::downgrade(model);
            let error = Rc::downgrade(&message);
            move |from, to| {
                FormBuilder::submit(
                    &weak,
                    Box::new(move |s| {
                        let rules = &mut s.virtual_workspaces.app_rules;
                        if from < rules.len() && to < rules.len() {
                            let rule = rules.remove(from);
                            rules.insert(to, rule);
                        }
                        Ok(())
                    }),
                    error.clone(),
                )
            }
        }),
    );
    let weak_table = Rc::downgrade(&table);
    let names = weak_table.clone();
    let _ = model.requests.send(Request {
        action: Action::RefreshRuntime,
        finish: Box::new(move |_| {
            if let Some(table) = names.upgrade() {
                table.ns_table_view().reloadData();
            }
        }),
    });
    let edit_action = edit_rule.clone();
    let edit = IconButton::new(&ui, "pencil", "Edit Rule…").on_click(move || {
        if let Some(index) = weak_table.upgrade().and_then(|table| table.selection()) {
            edit_action(index);
        }
    });
    let weak = Rc::downgrade(model);
    let next_editor = edit_rule.clone();
    let controls = AddRemoveControl::new(&ui).on_add(move || {
        if let Some(model) = weak.upgrade() {
            add_rule(ui, &model, next_editor.clone());
        }
    });
    let weak = Rc::downgrade(model);
    let weak_table = Rc::downgrade(&table);
    let error = Rc::downgrade(&message);
    let remove_rule: Rc<dyn Fn()> = Rc::new(move || {
        if let Some(index) = weak_table.upgrade().and_then(|table| table.selection()) {
            FormBuilder::submit(
                &weak,
                Box::new(move |s| {
                    if index < s.virtual_workspaces.app_rules.len() {
                        s.virtual_workspaces.app_rules.remove(index);
                    }
                    Ok(())
                }),
                error.clone(),
            );
        }
    });
    let action = remove_rule.clone();
    let controls = Rc::new(controls.on_remove(move || action()));
    let weak_table = Rc::downgrade(&table);
    table.set_context_menu(
        Menu::new(&ui)
            .item(MenuItem::new(&ui, "Edit Rule…").on_click(move || {
                if let Some(index) = weak_table.upgrade().and_then(|table| table.selection()) {
                    edit_rule(index);
                }
            }))
            .item(MenuItem::new(&ui, "Remove").on_click(move || remove_rule())),
    );
    controls.set_remove_enabled(false);
    edit.set_enabled(false);
    let weak_controls = Rc::downgrade(&controls);
    let weak_edit = objc2::rc::Weak::new(edit.ns_button());
    table.set_on_select(move |selection| {
        if let Some(controls) = weak_controls.upgrade() {
            controls.set_remove_enabled(selection.is_some());
        }
        if let Some(edit) = weak_edit.load() {
            edit.setEnabled(selection.is_some());
        }
    });
    let weak_table = Rc::downgrade(&table);
    let last = RefCell::new(Vec::new());
    f.sync.push(Box::new(move |source| {
        let rules = &source.virtual_workspaces.app_rules;
        if *last.borrow() != *rules {
            if let Some(table) = weak_table.upgrade() {
                let selected = table.selection().filter(|index| *index < rules.len());
                table.set_rows(rules.clone());
                table.set_selected(selected);
            }
            *last.borrow_mut() = rules.clone();
        }
        if let Some(page) = current.borrow().as_ref() {
            for sync in &page.sync {
                sync(source);
            }
        }
    }));
    f.finish(SettingsPage::new(&ui, "")
        .subtitle("Choose which windows Rift manages and where they open. Click a rule’s arrow to edit it. Drag rules to change their order.")
        .section(table)
        .bottom_bar(HStack::new(&ui).push(controls).spacer(&ui).push(edit))
        .section(message)
        )
}

fn rule_behavior(rule: &AppWorkspaceRule) -> String {
    let mut parts = vec![if rule.manage == Some(false) {
        "Ignore".into()
    } else if rule.floating {
        "Floating".into()
    } else if rule.manage == Some(true) {
        "Manage".into()
    } else {
        "Automatic".into()
    }];
    if let Some(workspace) = &rule.workspace {
        parts.push(match workspace {
            WorkspaceSelector::Index(index) => format!("Workspace {}", index + 1),
            WorkspaceSelector::Name(name) => name.clone(),
        });
    }
    if rule.focus {
        parts.push("Focus".into());
    }
    parts.join(" · ")
}

type AppMatch = (Option<String>, Option<String>);
fn application_choices(model: &Model) -> Vec<(String, AppMatch)> {
    let mut apps: Vec<_> = model
        .applications
        .borrow()
        .iter()
        .filter(|app| app.window_count > 0)
        .map(|app| {
            (
                app.name.clone(),
                if let Some(id) = &app.bundle_id {
                    (Some(id.clone()), None)
                } else {
                    (None, Some(app.name.clone()))
                },
            )
        })
        .collect();
    if let Some(installed) = model.installed_applications.borrow().as_ref() {
        apps.extend(installed.iter().map(|(name, id)| (name.clone(), (Some(id.clone()), None))));
    }
    apps.sort_by_key(|(name, _)| name.to_lowercase());
    let mut seen = std::collections::BTreeSet::new();
    apps.retain(|(_, target)| seen.insert(target.clone()));
    apps
}

fn application_icons<T>(
    bundle_id: impl Fn(&T) -> Option<String>,
) -> impl Fn(&T) -> Option<objc2::rc::Retained<objc2_app_kit::NSImage>> {
    let icons = RefCell::new(std::collections::HashMap::<
        String,
        Option<objc2::rc::Retained<objc2_app_kit::NSImage>>,
    >::new());
    move |item| {
        let id = bundle_id(item)?;
        if let Some(icon) = icons.borrow().get(&id) {
            return icon.clone();
        }
        let workspace = objc2_app_kit::NSWorkspace::sharedWorkspace();
        let icon = workspace
            .URLForApplicationWithBundleIdentifier(&objc2_foundation::NSString::from_str(&id))
            .and_then(|url| url.path())
            .map(|path| workspace.iconForFile(&path));
        icons.borrow_mut().insert(id, icon.clone());
        icon
    }
}

fn app_picker(
    ui: Ui,
    model: &Rc<Model>,
    initial: AppMatch,
    mut choose: impl FnMut(AppMatch) + 'static,
) -> (Rc<HStack>, Rc<Label>) {
    let title = Rc::new(Label::new(
        &ui,
        &application_choices(model)
            .into_iter()
            .find(|(_, target)| *target == initial)
            .map(|(name, _)| name)
            .or(initial.1)
            .or(initial.0)
            .unwrap_or_else(|| "Any application".into()),
    ));
    let choices = Rc::new(RefCell::new(application_choices(model)));
    let selected_choices = choices.clone();
    let dismissal = Rc::new(RefCell::new(std::rc::Weak::<Popover>::new()));
    let dismiss = dismissal.clone();
    let list = Rc::new(
        SettingsList::new(
            &ui,
            |app: &(String, AppMatch)| app.0.clone(),
            |app| app.1.0.clone().unwrap_or_default(),
        )
        .empty_message("No matching applications")
        .fit_content(240.0)
        .symbol("app")
        .images(application_icons(|app: &(String, AppMatch)| app.1.0.clone()))
        .on_select(move |index| {
            let item = index.and_then(|index| selected_choices.borrow().get(index).cloned());
            if let Some((_, target)) = item {
                choose(target);
                if let Some(popover) = dismiss.borrow().upgrade() {
                    popover.close();
                }
            }
        }),
    );
    list.set_rows(choices.borrow().clone());
    let weak_model = Rc::downgrade(model);
    let weak_list = Rc::downgrade(&list);
    let filtered = choices.clone();
    let search = Rc::new(
        SearchField::new(&ui)
            .placeholder("Find an application")
            .on_change(move |query| {
                if let (Some(model), Some(list)) = (weak_model.upgrade(), weak_list.upgrade()) {
                    let query = query.trim().to_lowercase();
                    let apps: Vec<_> = application_choices(&model)
                        .into_iter()
                        .filter(|(name, target)| {
                            name.to_lowercase().contains(&query)
                                || target
                                    .0
                                    .as_ref()
                                    .is_some_and(|id| id.to_lowercase().contains(&query))
                        })
                        .collect();
                    *filtered.borrow_mut() = apps.clone();
                    list.set_rows(apps);
                    list.set_selected(None);
                }
            }),
    );
    let weak_model = Rc::downgrade(model);
    let weak_list = Rc::downgrade(&list);
    let weak_search = Rc::downgrade(&search);
    let _ = model.requests.send(Request {
        action: Action::RefreshRuntime,
        finish: Box::new(move |_| {
            if let (Some(model), Some(list)) = (weak_model.upgrade(), weak_list.upgrade()) {
                let query = weak_search
                    .upgrade()
                    .map(|search| search.get_value().trim().to_lowercase())
                    .unwrap_or_default();
                let apps: Vec<_> = application_choices(&model)
                    .into_iter()
                    .filter(|(name, target)| {
                        name.to_lowercase().contains(&query)
                            || target
                                .0
                                .as_ref()
                                .is_some_and(|id| id.to_lowercase().contains(&query))
                    })
                    .collect();
                *choices.borrow_mut() = apps.clone();
                list.set_rows(apps);
            }
        }),
    });
    let content = VStack::new(&ui)
        .insets(Insets {
            top: 12.0,
            left: 12.0,
            bottom: 12.0,
            right: 12.0,
        })
        .push(search)
        .push(list);
    content.width(360.0);
    let popover = Rc::new(Popover::new(&ui, content));
    *dismissal.borrow_mut() = Rc::downgrade(&popover);
    let button = Button::new(&ui, "Change…");
    let anchor = objc2::rc::Weak::new(button.ns_view());
    let button = button.on_click(move || {
        if let Some(view) = anchor.load() {
            popover.show(&view);
        }
    });
    (
        Rc::new(HStack::new(&ui).push(title.clone()).spacer(&ui).push(button)),
        title,
    )
}

fn add_rule(ui: Ui, model: &Rc<Model>, edit_rule: Rc<dyn Fn(usize)>) {
    // A native sheet collects the first matcher before inserting a valid rule.
    let validation = Rc::new(ValidationMessage::new(&ui));
    let input_error = Rc::downgrade(&validation);
    let input = Rc::new(TextField::new(&ui).placeholder("com.apple.Safari"));
    let app_name = Rc::new(RefCell::new(None::<String>));
    let selected_name = app_name.clone();
    let weak_model = Rc::downgrade(model);
    let weak_input = Rc::downgrade(&input);
    let button = Button::new(&ui, "Add Rule").key_equivalent("\r").on_click(move || {
        let Some(input) = weak_input.upgrade() else {
            return;
        };
        let value = input.get_value();
        let name = if value.trim().is_empty() {
            selected_name.borrow().clone()
        } else {
            None
        };
        if value.trim().is_empty() && name.is_none() {
            if let Some(message) = input_error.upgrade() {
                message.set_validation(&Validation::Error(
                    "Choose an app or enter a bundle identifier.".into(),
                ));
            }
            if let Some(model) = weak_model.upgrade() {
                if let Some(sheet) = model.sheet.borrow().as_ref() {
                    sheet.fit_content();
                }
            }
            return;
        }
        let Some(model) = weak_model.upgrade() else {
            return;
        };
        let weak = Rc::downgrade(&model);
        let message = input_error.clone();
        let next = edit_rule.clone();
        let _ = model.requests.send(Request {
            action: Action::Edit(Box::new(move |source| {
                source.virtual_workspaces.app_rules.push(AppWorkspaceRule {
                    app_id: (!value.trim().is_empty()).then(|| value.trim().to_string()),
                    app_name: name,
                    ..Default::default()
                });
                Ok(())
            })),
            finish: Box::new(move |result| {
                if let Some(model) = weak.upgrade() {
                    match result {
                        Ok(()) => {
                            if let Some(sheet) = model.sheet.borrow().as_ref() {
                                sheet.end();
                            }
                            let index = model
                                .source
                                .borrow()
                                .virtual_workspaces
                                .app_rules
                                .len()
                                .checked_sub(1);
                            if let Some(index) = index {
                                next(index);
                            }
                        }
                        Err(error) => {
                            if let Some(message) = message.upgrade() {
                                message.set_validation(&Validation::Error(error));
                            }
                            if let Some(sheet) = model.sheet.borrow().as_ref() {
                                sheet.fit_content();
                            }
                        }
                    }
                }
            }),
        });
    });
    let weak_sheet = Rc::downgrade(model);
    let cancel = Button::new(&ui, "Cancel").key_equivalent("\u{1b}").on_click(move || {
        if let Some(sheet) = weak_sheet.upgrade() {
            if let Some(sheet) = sheet.sheet.borrow().as_ref() {
                sheet.end();
            }
        }
    });
    let target = Rc::downgrade(&input);
    let choices = Rc::new(RefCell::new(application_choices(model)));
    let selection_choices = choices.clone();
    let list = Rc::new(
        SettingsList::new(
            &ui,
            |app: &(String, AppMatch)| app.0.clone(),
            |app| app.1.0.clone().unwrap_or_default(),
        )
        .fit_content(260.0)
        .symbol("app")
        .images(application_icons(|app: &(String, AppMatch)| app.1.0.clone()))
        .on_select(move |index| {
            if let Some((_, (id, name))) =
                index.and_then(|index| selection_choices.borrow().get(index).cloned())
            {
                if let Some(input) = target.upgrade() {
                    input.set_value(id.as_deref().unwrap_or_default());
                    *app_name.borrow_mut() = name;
                }
            } else if let Some(input) = target.upgrade() {
                input.set_value("");
                *app_name.borrow_mut() = None;
            }
        }),
    );
    list.set_rows(choices.borrow().clone());
    let weak_model = Rc::downgrade(model);
    let weak_list = Rc::downgrade(&list);
    let filtered = choices.clone();
    let search = Rc::new(
        SearchField::new(&ui)
            .placeholder("Search by app name or bundle identifier")
            .on_change(move |query| {
                if let (Some(model), Some(list)) = (weak_model.upgrade(), weak_list.upgrade()) {
                    let query = query.trim().to_lowercase();
                    let apps: Vec<_> = application_choices(&model)
                        .into_iter()
                        .filter(|(name, target)| {
                            name.to_lowercase().contains(&query)
                                || target
                                    .0
                                    .as_ref()
                                    .is_some_and(|id| id.to_lowercase().contains(&query))
                        })
                        .collect();
                    *filtered.borrow_mut() = apps.clone();
                    list.set_rows(apps);
                    list.set_selected(None);
                }
            }),
    );
    let weak_model = Rc::downgrade(model);
    let weak_list = Rc::downgrade(&list);
    let weak_search = Rc::downgrade(&search);
    let _ = model.requests.send(Request {
        action: Action::RefreshRuntime,
        finish: Box::new(move |_| {
            if let (Some(model), Some(list)) = (weak_model.upgrade(), weak_list.upgrade()) {
                let query = weak_search
                    .upgrade()
                    .map(|search| search.get_value().trim().to_lowercase())
                    .unwrap_or_default();
                let apps: Vec<_> = application_choices(&model)
                    .into_iter()
                    .filter(|(name, target)| {
                        name.to_lowercase().contains(&query)
                            || target
                                .0
                                .as_ref()
                                .is_some_and(|id| id.to_lowercase().contains(&query))
                    })
                    .collect();
                *choices.borrow_mut() = apps.clone();
                list.set_rows(apps);
            }
        }),
    });
    let picker = VStack::new(&ui).push(search).push(list);
    let content = VStack::new(&ui)
        .insets(Insets {
            top: 20.0,
            left: 20.0,
            bottom: 20.0,
            right: 20.0,
        })
        .push(SectionTitle::new(&ui, "Add App Rule"))
        .push(Caption::new(
            &ui,
            "Choose an app, then set how Rift should handle its windows.",
        ))
        .push(picker)
        .push(Disclosure::new(&ui, "Enter a bundle identifier manually", input))
        .push(validation)
        .push(HStack::new(&ui).spacer(&ui).push(cancel).push(button));
    content.min_width(380.0);
    let native = Sheet::new(&ui, "Add Rule", content);
    native.fit_content();
    if let Some(window) = model.window.borrow().load() {
        native.show(&window);
        *model.sheet.borrow_mut() = Some(native);
    }
}

fn rule_summary(r: &AppWorkspaceRule) -> String {
    r.app_name
        .as_ref()
        .or(r.app_id.as_ref())
        .or(r.title_substring.as_ref())
        .or(r.title_regex.as_ref())
        .or(r.ax_subrole.as_ref())
        .or(r.ax_role.as_ref())
        .cloned()
        .unwrap_or("Rule".into())
}
fn rule_detail(ui: Ui, model: &Rc<Model>, i: usize) -> Page {
    let mut f = FormBuilder::new(ui, model);
    let mut matches = Section::new(&ui, "Application")
        .description("Apply this rule to an app, or narrow it to windows with a particular title.");
    let existing = model
        .source
        .borrow()
        .virtual_workspaces
        .app_rules
        .get(i)
        .map(|r| (r.app_id.clone(), r.app_name.clone()))
        .unwrap_or_default();
    let weak = Rc::downgrade(model);
    let message = Rc::new(ValidationMessage::new(&ui));
    let error = Rc::downgrade(&message);
    let (picker_view, picker) = app_picker(ui, model, existing, move |target| {
        FormBuilder::submit(
            &weak,
            Box::new(move |s| {
                let rule =
                    s.virtual_workspaces.app_rules.get_mut(i).ok_or("Rule no longer exists")?;
                rule.app_id = target.0;
                rule.app_name = target.1;
                Ok(())
            }),
            error.clone(),
        );
    });
    let weak_picker = Rc::downgrade(&picker);
    let weak_model = Rc::downgrade(model);
    f.sync.push(Box::new(move |source| {
        if let Some(picker) = weak_picker.upgrade() {
            let target = source
                .virtual_workspaces
                .app_rules
                .get(i)
                .map(|r| (r.app_id.clone(), r.app_name.clone()))
                .unwrap_or_default();
            let name = weak_model
                .upgrade()
                .and_then(|model| {
                    application_choices(&model)
                        .into_iter()
                        .find(|(_, value)| *value == target)
                        .map(|(name, _)| name)
                })
                .or(target.1)
                .or(target.0)
                .unwrap_or_else(|| "Any application".into());
            picker.set_text(&name);
        }
    }));
    matches = matches.row(SettingsRow::new(&ui, "Application", picker_view)).footer(message);
    let advanced = VStack::new(&ui).spacing(8.0);
    for (title, field) in [
        ("Bundle identifier", 0),
        ("Application name", 1),
        ("Window title contains", 2),
        ("Title regex", 3),
        ("Accessibility role", 4),
        ("Accessibility subrole", 5),
    ] {
        let row = f.text(
            title,
            move |s| {
                s.virtual_workspaces
                    .app_rules
                    .get(i)
                    .and_then(|r| match field {
                        0 => r.app_id.clone(),
                        1 => r.app_name.clone(),
                        2 => r.title_substring.clone(),
                        3 => r.title_regex.clone(),
                        4 => r.ax_role.clone(),
                        _ => r.ax_subrole.clone(),
                    })
                    .unwrap_or_default()
            },
            move |s, v| {
                let Some(r) = s.virtual_workspaces.app_rules.get_mut(i) else {
                    return Err("Rule no longer exists".into());
                };
                let value = (!v.trim().is_empty()).then_some(v);
                match field {
                    0 => r.app_id = value,
                    1 => r.app_name = value,
                    2 => r.title_substring = value,
                    3 => r.title_regex = value,
                    4 => r.ax_role = value,
                    _ => r.ax_subrole = value,
                };
                Ok(())
            },
        );
        if field == 2 {
            matches = matches.row(row);
        } else {
            advanced.add(row);
        }
    }
    matches = matches.content(Disclosure::new(&ui, "Advanced matching", advanced));
    let workspace = workspace_popup(
        &mut f,
        "Workspace",
        true,
        move |s| s.virtual_workspaces.app_rules.get(i).and_then(|r| r.workspace.clone()),
        move |s, v| {
            if let Some(r) = s.virtual_workspaces.app_rules.get_mut(i) {
                r.workspace = v;
            }
        },
    );
    let actions = Section::new(&ui, "Window behavior")
        .row(workspace)
        .row(f.switch(
            "Floating",
            move |s| s.virtual_workspaces.app_rules.get(i).is_some_and(|r| r.floating),
            move |s, v| {
                if let Some(r) = s.virtual_workspaces.app_rules.get_mut(i) {
                    r.floating = v;
                    if !v {
                        r.position = None;
                    }
                }
            },
        ))
        .row(f.switch(
            "Focus window",
            move |s| s.virtual_workspaces.app_rules.get(i).is_some_and(|r| r.focus),
            move |s, v| {
                if let Some(r) = s.virtual_workspaces.app_rules.get_mut(i) {
                    r.focus = v;
                }
            },
        ))
        .row(f.popup(
            "Management",
            vec![
                ("Automatic", None),
                ("Manage", Some(true)),
                ("Ignore", Some(false)),
            ],
            move |s| s.virtual_workspaces.app_rules.get(i).and_then(|r| r.manage),
            move |s, v| {
                if let Some(r) = s.virtual_workspaces.app_rules.get_mut(i) {
                    r.manage = v;
                }
            },
        ));
    let note = WrappingLabel::new(
        &ui,
        "Automatic manages normal windows and ignores special windows when appropriate. Manage always includes matching windows; Ignore excludes them.",
    );
    note.ns_text_field().setFont(Some(&Font::caption()));
    note.ns_text_field().setTextColor(Some(&cgs::Color::secondary_label()));
    let actions = actions.footer(note);
    let mut geometry = Section::new(&ui, "Initial size and position").description(
        "Leave blank to use the existing geometry. Position applies to floating windows.",
    );
    for (title, axis) in [
        ("Horizontal position (%)", 0),
        ("Vertical position (%)", 1),
        ("Width (points)", 2),
        ("Height (points)", 3),
    ] {
        let row = f.text(
            title,
            move |s| {
                s.virtual_workspaces
                    .app_rules
                    .get(i)
                    .and_then(|r| match axis {
                        0 => r.position.map(|p| p.x * 100.0),
                        1 => r.position.map(|p| p.y * 100.0),
                        2 => r.size.and_then(|p| p.w),
                        _ => r.size.and_then(|p| p.h),
                    })
                    .map(|v| v.to_string())
                    .unwrap_or_default()
            },
            move |s, v| {
                let value = optional_number(&v)?;
                if let Some(r) = s.virtual_workspaces.app_rules.get_mut(i) {
                    if axis < 2 {
                        r.position = value.map(|v| {
                            let mut p = r.position.unwrap_or(AppRulePosition { x: 0.0, y: 0.0 });
                            if axis == 0 {
                                p.x = v / 100.0;
                            } else {
                                p.y = v / 100.0;
                            }
                            p
                        });
                    } else {
                        let mut size = r.size.unwrap_or(AppRuleSize { w: None, h: None });
                        if axis == 2 {
                            size.w = value;
                        } else {
                            size.h = value;
                        }
                        r.size = (size.w.is_some() || size.h.is_some()).then_some(size);
                    }
                }
                Ok(())
            },
        );
        if axis < 2 {
            f.enabled(&row, move |s| {
                s.virtual_workspaces.app_rules.get(i).is_some_and(|r| r.floating)
            });
        }
        geometry = geometry.row(row);
    }
    f.finish(
        SettingsPage::new(&ui, "")
            .section(matches)
            .section(actions)
            .section(Disclosure::new(&ui, "Set initial size and position", geometry)),
    )
}

pub(super) fn strings(
    f: &mut FormBuilder,
    title: &str,
    get: impl Fn(&ConfigSource) -> Vec<String> + Copy + 'static,
    set: impl Fn(&mut ConfigSource, Vec<String>) + Copy + Send + 'static,
) -> Section {
    // Native list and a text field. No embedded TOML or serialization in the UI.
    let ui = f.ui;
    let message = Rc::new(ValidationMessage::new(&ui));
    let error = Rc::downgrade(&message);
    let weak = f.model.clone();
    let field = Rc::new(TextField::new(&ui).placeholder("Add an entry"));
    let input = Rc::downgrade(&field);
    let add = Button::new(&ui, "Add").on_click(move || {
        if let (Some(model), Some(input)) = (weak.upgrade(), input.upgrade()) {
            let value = input.get_value();
            if value.trim().is_empty() {
                return;
            }
            let mut values = get(&model.source.borrow());
            values.push(value);
            FormBuilder::submit(
                &weak,
                Box::new(move |s| {
                    set(s, values);
                    Ok(())
                }),
                error.clone(),
            );
            input.set_value("");
        }
    });
    let weak = f.model.clone();
    let error = Rc::downgrade(&message);
    let list = Rc::new(
        EditableList::new(&ui, |s: &String| s.clone()).on_remove(move |i| {
            if let Some(model) = weak.upgrade() {
                let mut values = get(&model.source.borrow());
                if i < values.len() {
                    values.remove(i);
                }
                FormBuilder::submit(
                    &weak,
                    Box::new(move |s| {
                        set(s, values);
                        Ok(())
                    }),
                    error.clone(),
                );
            }
        }),
    );
    list.height(120.0);
    let weak_list = Rc::downgrade(&list);
    f.sync.push(Box::new(move |s| {
        if let Some(list) = weak_list.upgrade() {
            list.set_items(get(s));
        }
    }));
    Section::new(&ui, title)
        .content(HStack::new(&ui).push(field).push(add))
        .content(list)
        .footer(message)
}

pub(super) fn display_overrides(
    f: &mut FormBuilder,
    page: SettingsPage,
    model: &Rc<Model>,
) -> SettingsPage {
    let ui = f.ui;
    let mut displays: Vec<_> = model
        .displays
        .borrow()
        .iter()
        .map(|d| {
            (
                d.display_uuid.clone(),
                d.name.clone().unwrap_or_else(|| format!("Display {}", d.id.as_u32())),
            )
        })
        .collect();
    let source = model.source.borrow();
    for uuid in source
        .settings
        .layout
        .scrolling
        .per_display
        .keys()
        .chain(source.settings.layout.gaps.per_display.keys())
    {
        if !displays.iter().any(|(id, _)| id == uuid) {
            displays.push((uuid.clone(), "Disconnected display".into()));
        }
    }
    drop(source);
    if displays.is_empty() {
        return page;
    }
    let current = Rc::new(RefCell::new(None::<Page>));
    let editor = current.clone();
    let weak = Rc::downgrade(model);
    let entries = displays.clone();
    let connected: Vec<_> =
        model.displays.borrow().iter().map(|d| d.display_uuid.clone()).collect();
    let list = SettingsList::new(
        &ui,
        |entry: &(String, String)| entry.1.clone(),
        move |entry: &(String, String)| {
            if connected.contains(&entry.0) {
                "Connected · Customize spacing and scrolling widths".into()
            } else {
                format!("Not connected · Saved settings · {}", entry.0)
            }
        },
    )
    .symbol("display")
    .full_length()
    .on_open(move |index| {
        if let Some(model) = weak.upgrade() {
            let (uuid, name) = &entries[index];
            show_editor(
                ui,
                &model,
                name,
                display_options(ui, &model, uuid.clone()),
                &editor,
            );
        }
    });
    list.set_rows(displays);
    f.sync.push(Box::new(move |source| {
        if let Some(page) = current.borrow().as_ref() {
            for sync in &page.sync {
                sync(source);
            }
        }
    }));
    page.section(
        Section::new(&ui, "Display settings")
            .description(
                "Choose a display to override the spacing and scrolling widths for that screen.",
            )
            .content(list),
    )
}

fn display_options(ui: Ui, model: &Rc<Model>, uuid: String) -> Page {
    let mut f = FormBuilder::new(ui, model);
    let mut rows = Section::new(&ui, "Scrolling layout widths")
        .description("Widths inherit the scrolling layout’s values. Changing a value overrides it for this display.");
    for (title, field) in [
        ("Column width (%)", 0),
        ("Minimum width (%)", 1),
        ("Maximum width (%)", 2),
    ] {
        let id = uuid.clone();
        let edit_id = uuid.clone();
        rows = rows.row(f.text(
            title,
            move |s| {
                s.settings
                    .layout
                    .scrolling
                    .per_display
                    .get(&id)
                    .and_then(|o| match field {
                        0 => o.column_width_ratio,
                        1 => o.min_column_width_ratio,
                        _ => o.max_column_width_ratio,
                    })
                    .unwrap_or_else(|| match field {
                        0 => s.settings.layout.scrolling.column_width_ratio,
                        1 => s.settings.layout.scrolling.min_column_width_ratio,
                        _ => s.settings.layout.scrolling.max_column_width_ratio,
                    })
                    .mul_add(100.0, 0.0)
                    .to_string()
            },
            move |s, v| {
                let inherited = match field {
                    0 => s.settings.layout.scrolling.column_width_ratio,
                    1 => s.settings.layout.scrolling.min_column_width_ratio,
                    _ => s.settings.layout.scrolling.max_column_width_ratio,
                };
                let value = optional_number(&v)?
                    .map(|v| v / 100.0)
                    .filter(|v| (*v - inherited).abs() > 1e-9);
                let o = s.settings.layout.scrolling.per_display.entry(edit_id.clone()).or_default();
                match field {
                    0 => o.column_width_ratio = value,
                    1 => o.min_column_width_ratio = value,
                    _ => o.max_column_width_ratio = value,
                };
                if o.column_width_ratio.is_none()
                    && o.min_column_width_ratio.is_none()
                    && o.max_column_width_ratio.is_none()
                {
                    s.settings.layout.scrolling.per_display.remove(&edit_id);
                }
                Ok(())
            },
        ));
    }
    for outer in [true, false] {
        let group = Rc::new(VStack::new(&ui));
        for axis in 0..if outer { 4 } else { 2 } {
            let id = uuid.clone();
            let edit_id = uuid.clone();
            let row = f.number(
                if outer {
                    ["Top", "Left", "Bottom", "Right"][axis]
                } else {
                    ["Horizontal", "Vertical"][axis]
                },
                1.0,
                move |s| {
                    let effective = s.settings.layout.gaps.effective_for_display(Some(&id));
                    if outer {
                        [
                            effective.outer.top,
                            effective.outer.left,
                            effective.outer.bottom,
                            effective.outer.right,
                        ][axis]
                    } else {
                        [effective.inner.horizontal, effective.inner.vertical][axis]
                    }
                },
                move |s, v| {
                    let base = s.settings.layout.gaps.effective_for_display(Some(&edit_id));
                    let o = s.settings.layout.gaps.per_display.entry(edit_id.clone()).or_default();
                    if outer {
                        let g = o.outer.get_or_insert(base.outer);
                        match axis {
                            0 => g.top = v,
                            1 => g.left = v,
                            2 => g.bottom = v,
                            _ => g.right = v,
                        }
                    } else {
                        let g = o.inner.get_or_insert(base.inner);
                        if axis == 0 {
                            g.horizontal = v;
                        } else {
                            g.vertical = v;
                        }
                    }
                },
            );
            // Preserve Rust callback ownership in the stack.
            group.add(row);
        }
        let id = uuid.clone();
        let edit_id = uuid.clone();
        let weak = f.model.clone();
        let message = Rc::new(ValidationMessage::new(&ui));
        let error = Rc::downgrade(&message);
        let row = Rc::new(
            OverrideRow::new(&ui, group, "Use Default").on_change(move |custom| {
                let id = edit_id.clone();
                FormBuilder::submit(
                    &weak,
                    Box::new(move |s| {
                        let base = s.settings.layout.gaps.effective_for_display(None);
                        let o = s.settings.layout.gaps.per_display.entry(id.clone()).or_default();
                        if outer {
                            o.outer = custom.then_some(base.outer);
                        } else {
                            o.inner = custom.then_some(base.inner);
                        }
                        if o.outer.is_none() && o.inner.is_none() {
                            s.settings.layout.gaps.per_display.remove(&id);
                        }
                        Ok(())
                    }),
                    error.clone(),
                );
            }),
        );
        let weak_row = Rc::downgrade(&row);
        f.sync.push(Box::new(move |s| {
            if let Some(row) = weak_row.upgrade() {
                row.set_overridden(s.settings.layout.gaps.per_display.get(&id).is_some_and(|o| {
                    if outer {
                        o.outer.is_some()
                    } else {
                        o.inner.is_some()
                    }
                }));
            }
        }));
        rows = rows
            .content(SubsectionTitle::new(
                &ui,
                if outer {
                    "Space around windows"
                } else {
                    "Space between windows"
                },
            ))
            .content(row)
            .content(message);
    }
    f.finish(rows)
}

pub(super) fn keyboard(ui: Ui, model: &Rc<Model>) -> Page { super::commands::keyboard(ui, model) }
