use super::pages::{layouts, optional_number};
use super::*;
use crate::common::config::*;

/// Workspace records use a native collection and a small attached detail editor.
type WorkspaceEntry = (usize, String, Option<LayoutMode>);

/// Retain only the open editor, so its controls stay synchronized with ConfigActor.
fn record_editor(
    ui: Ui,
    model: &Rc<Model>,
    title: &str,
    build: impl FnOnce(&Rc<Model>) -> Page,
    scrolling: bool,
) {
    let parent = model.env.window.borrow().clone();
    if !parent.is_open() {
        return;
    }
    let draft = model.draft();
    let page = Rc::new(build(&draft));
    page.synchronize(&draft);
    *draft.draft.as_ref().unwrap().page.borrow_mut() = Some(page.clone());
    let weak = Rc::downgrade(model);
    let cancel = Rc::new(SheetActions::new(&ui));
    cancel.set_on_cancel(move || {
        if let Some(model) = weak.upgrade() {
            model.close_sheet();
        }
    });
    cancel.set_changed(draft.draft.as_ref().is_some_and(|d| *draft.source.borrow() != d.base));
    let dirty_cancel = Rc::downgrade(&cancel);
    *draft.draft.as_ref().unwrap().changed.borrow_mut() = Some(Box::new(move |dirty| {
        if let Some(cancel) = dirty_cancel.upgrade() {
            cancel.set_changed(dirty);
        }
    }));
    let message = Rc::new(ValidationMessage::new(&ui));
    let error = Rc::downgrade(&message);
    let weak = Rc::downgrade(model);
    let weak_draft = Rc::downgrade(&draft);
    cancel.set_on_done(move || {
        // Commit the field editor before reading the draft, including numeric fields.
        let (Some(model), Some(draft)) = (weak.upgrade(), weak_draft.upgrade()) else {
            return;
        };
        if model.sheet.borrow().as_ref().is_some_and(|sheet| !sheet.end_editing()) {
            return;
        }
        let value = draft.source.borrow().clone();
        let base = draft.draft.as_ref().unwrap().base.clone();
        if value == base {
            model.close_sheet();
            return;
        }
        let weak = Rc::downgrade(&model);
        let error = error.clone();
        model.submit_edit(
            Box::new(move |source| {
                if *source != base {
                    return Err(
                        "Settings changed elsewhere. Cancel and reopen this editor to try again."
                            .into(),
                    );
                }
                *source = value;
                Ok(())
            }),
            Box::new(move |result| {
                if let Some(model) = weak.upgrade() {
                    match result {
                        Ok(()) => {
                            model.close_sheet();
                        }
                        Err(text) => {
                            if let Some(error) = error.upgrade() {
                                error.set_validation(&Validation::Error(text));
                            }
                        }
                    }
                }
            }),
        );
    });
    let body: Rc<dyn NativeView> = if scrolling {
        let scroll = ScrollView::new(&ui, page.view.clone());
        scroll.fit_width();
        scroll.height(430.0);
        Rc::new(scroll)
    } else {
        page.view.clone()
    };
    let content = VStack::new(&ui)
        .insets(Insets {
            top: 24.0,
            left: 24.0,
            bottom: 20.0,
            right: 24.0,
        })
        .spacing(16.0)
        .push(SectionTitle::new(&ui, title))
        .push(body)
        .push(message)
        .push(cancel);
    content.width(if scrolling { 540.0 } else { 420.0 });
    let sheet = Sheet::new(&ui, title, content);
    sheet.fit_content();
    model.sheet.borrow_mut().take();
    *model.sheet_model.borrow_mut() = Some(draft);
    sheet.show(&parent);
    *model.sheet.borrow_mut() = Some(sheet);
}

pub(super) fn workspaces(ui: Ui, model: &Rc<Model>) -> Page {
    let mut f = FormBuilder::new(ui, model);
    let mut section = f.schema_section(
        "Workspace behavior",
        "",
        |s| &s.virtual_workspaces,
        |s| &mut s.virtual_workspaces,
    );
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
    let selected = Rc::new(Cell::new(None));
    let weak = Rc::downgrade(model);
    let edit_workspace: Rc<dyn Fn(usize)> = Rc::new(move |index| {
        if let Some(model) = weak.upgrade() {
            record_editor(
                ui,
                &model,
                &workspace_name(&model.source.borrow(), index),
                |draft| {
                    if index == draft.source.borrow().virtual_workspaces.default_workspace_count {
                        draft.source.borrow_mut().virtual_workspaces.resize(index + 1);
                    }
                    workspace_detail(ui, draft, index)
                },
                false,
            );
        }
    });
    let open_added = edit_workspace.clone();
    let table = Rc::new(
        SettingsList::<WorkspaceEntry>::new(
            &ui,
            |item| item.1.clone(),
            |(i, name, layout): &WorkspaceEntry| {
                let layout = layout.map(super::pages::layout_title).unwrap_or("Default layout");
                if name == &format!("Workspace {}", i + 1) {
                    layout.to_string()
                } else {
                    format!("Workspace {} · {}", i + 1, layout)
                }
            },
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
                    let index = model.source.borrow().virtual_workspaces.default_workspace_count;
                    if index >= MAX_WORKSPACES {
                        if let Some(error) = error.upgrade() {
                            error.set_validation(&Validation::Error(format!(
                                "Use at most {MAX_WORKSPACES} workspaces."
                            )));
                        }
                    } else {
                        open_added(index);
                    }
                }
            })
            .on_remove(move || {
                if let Some(index) = remove_selected.get() {
                    Model::submit(
                        &weak_remove,
                        Box::new(move |s| {
                            let i = index;
                            if i + 1 != s.virtual_workspaces.default_workspace_count {
                                return Err("Only the last workspace can be removed.".into());
                            }
                            if i == 0 {
                                return Err("Keep at least one workspace.".into());
                            }
                            s.virtual_workspaces.resize(i);
                            Ok(())
                        }),
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
    f.sync.push(Box::new(move |source| {
        let s = source;
        let values: Vec<WorkspaceEntry> = {
            (0..s.virtual_workspaces.default_workspace_count)
                .map(|i| {
                    (
                        i,
                        workspace_name(s, i),
                        Some(workspace_layout(s, i).unwrap_or(s.settings.layout.mode)),
                    )
                })
                .collect()
        };
        if let Some(table) = weak_table.upgrade() {
            let index = selected.get().filter(|i| *i < values.len());
            table.set_rows_if_changed(values);
            table.set_selected(index);
        }
        if let Some(actions) = weak_actions.upgrade() {
            actions.set_remove_enabled(selected.get().is_some_and(|i| {
                i > 0 && i + 1 == source.virtual_workspaces.default_workspace_count
            }));
        }
    }));
    f.finish(
        SettingsPage::new(&ui, "")
            .content_width(760.0)
            .section(section)
            .section(Section::new(&ui, "Workspaces").content(table).footer(message))
            .bottom_bar(HStack::new(&ui).push(actions).spacer(&ui)),
    )
}

fn workspace_detail(ui: Ui, model: &Rc<Model>, i: usize) -> Page {
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
            while s.virtual_workspaces.workspace_names.last().is_some_and(String::is_empty) {
                s.virtual_workspaces.workspace_names.pop();
            }
            Ok(())
        },
    );
    let values = layouts().to_vec();
    let layout = f.inherited_popup(
        "Layout",
        values,
        move |s| workspace_layout(s, i),
        |s| s.settings.layout.mode,
        move |s, mode| {
            let name = s.virtual_workspaces.workspace_names.get(i).cloned();
            // Display bindings stay; only the layout moves to the new rule.
            s.virtual_workspaces.workspace_rules.retain_mut(|r| {
                if !selector_matches(&r.workspace, i, name.as_deref()) {
                    return true;
                }
                r.layout = None;
                r.display.is_some()
            });
            if let Some(layout) = mode {
                s.virtual_workspaces.workspace_rules.push(WorkspaceLayoutRule {
                    workspace: WorkspaceSelector::Index(i),
                    layout: Some(layout),
                    display: None,
                });
            }
        },
    );
    f.finish(Section::new(&ui, "").row(name).row(layout))
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
        .rev()
        .filter(|r| {
            selector_matches(
                &r.workspace,
                i,
                s.virtual_workspaces.workspace_names.get(i).map(String::as_str),
            )
        })
        .find_map(|r| r.layout)
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
        Model::submit(
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
    let visible = Rc::new(RefCell::new(Vec::<usize>::new()));
    let query = Rc::new(RefCell::new(String::new()));
    let filter = Rc::new(Cell::new(0usize));
    let message = Rc::new(ValidationMessage::new(&ui));
    let weak = Rc::downgrade(model);
    let edit_rule: Rc<dyn Fn(usize)> = Rc::new(move |index| {
        if let Some(model) = weak.upgrade() {
            let title = model
                .source
                .borrow()
                .virtual_workspaces
                .app_rules
                .get(index)
                .map(|rule| rule_name(&model, rule))
                .unwrap_or_else(|| "App Rule".into());
            record_editor(ui, &model, &title, |draft| rule_detail(ui, draft, index), true);
        }
    });
    let edit_record = edit_rule;
    let next_editor = edit_record.clone();
    let indices = visible.clone();
    let edit_rule: Rc<dyn Fn(usize)> = Rc::new(move |row| {
        if let Some(&index) = indices.borrow().get(row) {
            edit_record(index);
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
                    .map(|model| rule_name(&model, rule))
                    .unwrap_or_else(|| rule_summary(rule))
            },
            rule_behavior,
        )
        .full_length()
        .empty_message("No matching rules")
        .symbol("app")
        .images(application_icons(|rule: &AppWorkspaceRule| rule.app_id.clone()))
        .on_open(move |index| action(index))
        .reorderable(true)
        .on_reorder({
            let weak = Rc::downgrade(model);
            let error = Rc::downgrade(&message);
            let indices = visible.clone();
            move |from, to| {
                let indices = indices.borrow();
                let (Some(&from), Some(&to)) = (indices.get(from), indices.get(to)) else {
                    return;
                };
                Model::submit(
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
    if model.env.installed_applications.borrow().is_none() {
        let _ = model.env.requests.send(Request {
            action: Action::RefreshRuntime,
            finish: Box::new(move |_| {
                if let Some(table) = names.upgrade() {
                    table.reload();
                }
            }),
        });
    }
    let edit_action = edit_rule.clone();
    let edit = IconButton::new(&ui, "pencil", "Edit Rule…").on_click(move || {
        if let Some(index) = weak_table.upgrade().and_then(|table| table.selection()) {
            edit_action(index);
        }
    });
    let weak = Rc::downgrade(model);
    let controls = AddRemoveControl::new(&ui).on_add(move || {
        if let Some(model) = weak.upgrade() {
            add_rule(ui, &model, next_editor.clone());
        }
    });
    let weak = Rc::downgrade(model);
    let weak_table = Rc::downgrade(&table);
    let error = Rc::downgrade(&message);
    let indices = visible.clone();
    let remove_rule: Rc<dyn Fn()> = Rc::new(move || {
        if let Some(index) = weak_table
            .upgrade()
            .and_then(|table| table.selection())
            .and_then(|row| indices.borrow().get(row).copied())
        {
            Model::submit(
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
    let weak_edit = WeakView::new(&edit);
    table.set_on_select(move |selection| {
        if let Some(controls) = weak_controls.upgrade() {
            controls.set_remove_enabled(selection.is_some());
        }
        weak_edit.set_enabled(selection.is_some());
    });
    let weak_table = Rc::downgrade(&table);
    let weak_model = Rc::downgrade(model);
    let indices = visible.clone();
    let search_query = query.clone();
    let selected_filter = filter.clone();
    let refresh: Rc<dyn Fn()> = Rc::new(move || {
        let (Some(model), Some(table)) = (weak_model.upgrade(), weak_table.upgrade()) else {
            return;
        };
        let source = model.source.borrow();
        let query = search_query.borrow();
        let matches: Vec<_> = source
            .virtual_workspaces
            .app_rules
            .iter()
            .enumerate()
            .filter(|(_, rule)| {
                let kind = match selected_filter.get() {
                    0 => true,
                    index => match &rule.workspace {
                        Some(WorkspaceSelector::Index(workspace)) => *workspace == index - 1,
                        Some(WorkspaceSelector::Name(name)) => {
                            *name == workspace_name(&source, index - 1)
                        }
                        None => false,
                    },
                };
                kind && (query.is_empty()
                    || format!(
                        "{} {} {} {rule:?}",
                        rule_name(&model, rule),
                        rule_summary(rule),
                        rule_behavior(rule)
                    )
                    .to_lowercase()
                    .contains(query.as_str()))
            })
            .collect();
        *indices.borrow_mut() = matches.iter().map(|(index, _)| *index).collect();
        table.set_rows_if_changed(matches.into_iter().map(|(_, rule)| rule.clone()).collect());
    });
    let update = refresh.clone();
    f.sync.push(Box::new(move |_| update()));
    let update = refresh.clone();
    let search = SearchField::new(&ui).placeholder("Search rules").on_change(move |value| {
        *query.borrow_mut() = value.to_lowercase();
        update();
    });
    let tint = Rc::new(RefCell::new(Weak::<HeaderControls>::new()));
    let menu = Menu::new(&ui);
    let mut filter_items = Vec::new();
    let mut choices = vec!["All workspaces".to_owned()];
    choices.extend(
        (0..model.source.borrow().virtual_workspaces.default_workspace_count)
            .map(|index| workspace_name(&model.source.borrow(), index)),
    );
    for (index, title) in choices.into_iter().enumerate() {
        let filter = filter.clone();
        let update = refresh.clone();
        let tint = tint.clone();
        let item = MenuItem::new(&ui, &title).on_click(move || {
            filter.set(index);
            if let Some(header) = tint.borrow().upgrade() {
                header.set_filtered(index != 0);
            }
            update();
        });
        filter_items.push(item.weak());
        menu.add(item);
    }
    let menu = menu.on_tracking(move |_| {
        for (index, item) in filter_items.iter().enumerate() {
            item.set_checked(filter.get() == index);
        }
    });
    let header = Rc::new(HeaderControls::new(
        &ui,
        "Filter rules",
        "line.3.horizontal.decrease",
        menu,
        search,
    ));
    *tint.borrow_mut() = Rc::downgrade(&header);
    let mut page = f.finish(
        SettingsPage::new(&ui, "")
            .content_width(760.0)
            .subtitle(
                "Choose which windows Rift manages and where they open. Drag rules to change their order.",
            )
            .section(table)
            .bottom_bar(HStack::new(&ui).push(controls).spacer(&ui).push(edit))
            .section(message),
    );
    page.header = Some(header);
    page
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

use super::applications::{AppMatch, Choice};

/// One application search/list used by both matcher editors.
fn application_picker(
    ui: Ui,
    model: &Rc<Model>,
    height: f64,
    mut choose: impl FnMut(Option<AppMatch>) + 'static,
) -> VStack {
    let list = Rc::new(
        SettingsList::new(
            &ui,
            |app: &Rc<Choice>| app.name.clone(),
            |app| app.target.0.clone().unwrap_or_default(),
        )
        .empty_message("No matching applications")
        .fit_content(height)
        .symbol("app")
        .images(application_icons(|app: &Rc<Choice>| app.target.0.clone()))
        .on_select_item(move |app| choose(app.map(|app| app.target.clone()))),
    );
    list.set_rows(model.env.inventory.borrow().matching(""));
    let weak_env = Rc::downgrade(&model.env);
    let weak_list = Rc::downgrade(&list);
    let search = Rc::new(
        SearchField::new(&ui)
            .placeholder("Search by app name or bundle identifier")
            .on_change(move |query| {
                if let (Some(env), Some(list)) = (weak_env.upgrade(), weak_list.upgrade()) {
                    list.set_selected(None);
                    list.set_rows(env.inventory.borrow().matching(&query.trim().to_lowercase()));
                }
            }),
    );
    if model.env.installed_applications.borrow().is_none() {
        let weak_env = Rc::downgrade(&model.env);
        let weak_list = Rc::downgrade(&list);
        let weak_search = Rc::downgrade(&search);
        model.env.send(
            Action::RefreshRuntime,
            Box::new(move |_| {
                if let (Some(env), Some(list), Some(search)) =
                    (weak_env.upgrade(), weak_list.upgrade(), weak_search.upgrade())
                {
                    list.set_selected(None);
                    list.set_rows(
                        env.inventory.borrow().matching(&search.get_value().trim().to_lowercase()),
                    );
                }
            }),
        );
    }
    VStack::new(&ui).push(search).push(list)
}

fn application_icons<T>(bundle_id: impl Fn(&T) -> Option<String>) -> impl Fn(&T) -> Option<Image> {
    move |item| Application::icon_for_bundle(&bundle_id(item)?)
}

fn app_picker(
    ui: Ui,
    model: &Rc<Model>,
    initial: AppMatch,
    mut choose: impl FnMut(AppMatch) + 'static,
) -> (Rc<HStack>, Rc<Label>) {
    let title = Rc::new(Label::new(
        &ui,
        &model.env.inventory.borrow().display_name(&initial),
    ));
    let dismissal = Rc::new(RefCell::new(Weak::<Popover>::new()));
    let dismiss = dismissal.clone();
    let content = application_picker(ui, model, 240.0, move |target| {
        if let Some(target) = target {
            choose(target);
            if let Some(popover) = dismiss.borrow().upgrade() {
                popover.close();
            }
        }
    })
    .insets(uniform_insets(12.0));
    content.width(360.0);
    let popover = Rc::new(Popover::new(&ui, content));
    *dismissal.borrow_mut() = Rc::downgrade(&popover);
    let button = Button::new(&ui, "Change…");
    let anchor = WeakView::new(&button);
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
    let weak_sheet = Rc::downgrade(model);
    let cancel = Rc::new(SheetActions::new(&ui));
    cancel.set_on_cancel(move || {
        if let Some(model) = weak_sheet.upgrade() {
            model.close_sheet();
        }
    });
    let weak_cancel = Rc::downgrade(&cancel);
    let input = Rc::new(TextField::new(&ui).placeholder("com.apple.Safari").on_change(
        move |value| {
            if let Some(cancel) = weak_cancel.upgrade() {
                cancel.set_changed(!value.trim().is_empty());
            }
        },
    ));
    let app_name = Rc::new(RefCell::new(None::<String>));
    let selected_name = app_name.clone();
    let weak_model = Rc::downgrade(model);
    let weak_input = Rc::downgrade(&input);
    cancel.set_primary_title("Add Rule");
    cancel.set_on_done(move || {
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
            if let Some(model) = weak_model.upgrade()
                && let Some(sheet) = model.sheet.borrow().as_ref()
            {
                sheet.fit_content();
            }
            return;
        }
        let Some(model) = weak_model.upgrade() else {
            return;
        };
        let weak = Rc::downgrade(&model);
        let message = input_error.clone();
        let next = edit_rule.clone();
        let _ = model.env.requests.send(Request {
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
    let target = Rc::downgrade(&input);
    let weak_cancel = Rc::downgrade(&cancel);
    let picker = application_picker(ui, model, 260.0, move |choice| {
        if let Some(cancel) = weak_cancel.upgrade() {
            cancel.set_changed(choice.is_some());
        }
        if let Some(input) = target.upgrade() {
            let (id, name) = choice.unwrap_or_default();
            input.set_value(id.as_deref().unwrap_or_default());
            *app_name.borrow_mut() = name;
        }
    });
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
        .push(cancel);
    content.min_width(380.0);
    let native = Sheet::new(&ui, "Add Rule", content);
    native.fit_content();
    if native.show(&model.env.window.borrow()) {
        *model.sheet.borrow_mut() = Some(native);
    }
}

fn rule_name(model: &Model, rule: &AppWorkspaceRule) -> String {
    rule.app_id
        .as_deref()
        .and_then(|id| model.env.inventory.borrow().name_for_bundle(id).map(str::to_owned))
        .unwrap_or_else(|| rule_summary(rule))
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
    let mut matches = Section::new(&ui, "Matching conditions")
        .description("Match an application or window title.");
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
        Model::submit(
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
                .map(|model| model.env.inventory.borrow().display_name(&target))
                .unwrap_or_default();
            picker.set_text(&name);
        }
    }));
    matches = matches.row(SettingsRow::new(&ui, "Application", picker_view)).footer(message);
    let mut advanced = Section::new(&ui, "");
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
            advanced = advanced.row(row);
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
        ).help("Automatic manages normal windows and ignores special windows when appropriate. Manage always includes matching windows; Ignore excludes them."));
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
                        0 => r.position.map(|p| p.x),
                        1 => r.position.map(|p| p.y),
                        2 => r.size.and_then(|p| p.w),
                        _ => r.size.and_then(|p| p.h),
                    })
                    .map(|v| {
                        if axis < 2 {
                            super::pages::percentage_text(v)
                        } else {
                            v.to_string()
                        }
                    })
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
        VStack::new(&ui)
            .spacing(Metrics::SECTION_SPACING)
            .push(matches)
            .push(actions)
            .push(Disclosure::new(&ui, "Set initial size and position", geometry)),
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
            Model::submit(
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
        EditableList::new(&ui, |s: &String| s.clone())
            .full_length()
            .on_remove(move |i| {
                if let Some(model) = weak.upgrade() {
                    let mut values = get(&model.source.borrow());
                    if i < values.len() {
                        values.remove(i);
                    }
                    Model::submit(
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
        .env
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
    let customized: Vec<_> = source
        .settings
        .layout
        .scrolling
        .per_display
        .keys()
        .chain(source.settings.layout.gaps.per_display.keys())
        .cloned()
        .collect();
    drop(source);
    if displays.is_empty() {
        return page;
    }
    let weak = Rc::downgrade(model);
    let entries = displays.clone();
    let connected: Vec<_> =
        model.env.displays.borrow().iter().map(|d| d.display_uuid.clone()).collect();
    let list = SettingsList::new(
        &ui,
        |entry: &(String, String)| entry.1.clone(),
        move |entry: &(String, String)| {
            if connected.contains(&entry.0) {
                if customized.contains(&entry.0) {
                    "Connected · Custom settings".into()
                } else {
                    "Connected · Default".into()
                }
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
            record_editor(
                ui,
                &model,
                name,
                |draft| display_options(ui, draft, uuid.clone()),
                true,
            );
        }
    });
    list.set_rows(displays);
    page.section(
        Section::new(&ui, "Display settings")
            .description("Choose a display to customize its spacing.")
            .content(list),
    )
}

fn display_options(ui: Ui, model: &Rc<Model>, uuid: String) -> Page {
    let mut f = FormBuilder::new(ui, model);
    let mut rows = Section::new(&ui, "Scrolling layout widths").description(
        "Widths inherit the scrolling layout’s values. Changing a value overrides it for this display.",
    );
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
                super::pages::percentage_text(
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
                        .unwrap_or(match field {
                            0 => s.settings.layout.scrolling.column_width_ratio,
                            1 => s.settings.layout.scrolling.min_column_width_ratio,
                            _ => s.settings.layout.scrolling.max_column_width_ratio,
                        }),
                )
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
    rows = rows.content(super::pages::gap_preview(ui, &mut f, Some(uuid.clone())));
    for outer in [true, false] {
        let group = Rc::new(VStack::new(&ui));
        for axis in 0..if outer { 4 } else { 2 } {
            let id = uuid.clone();
            let edit_id = uuid.clone();
            let row = f
                .gap(
                    if outer {
                        super::pages::EDGES[axis]
                    } else {
                        super::pages::AXES[axis]
                    },
                    move |s| super::pages::spacing_gap(s, Some(&id), outer, Some(axis)),
                    move |s, v| {
                        super::pages::set_spacing_gap(s, Some(&edit_id), outer, Some(axis), v);
                    },
                )
                .suffix("pt");
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
                Model::submit(
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
