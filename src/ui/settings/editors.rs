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
                if *source != *base {
                    return Err(
                        "Settings changed elsewhere. Cancel and reopen this editor to try again."
                            .into(),
                    );
                }
                *source = value.as_ref().clone();
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
                        Arc::make_mut(&mut draft.source.borrow_mut())
                            .virtual_workspaces
                            .resize(index + 1);
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
        move |s, mode| set_workspace_layout(s, i, mode),
    );
    f.finish(Section::new(&ui, "").row(name).row(layout))
}

fn selector_matches(selector: &WorkspaceSelector, i: usize, name: Option<&str>) -> bool {
    match selector {
        WorkspaceSelector::Index(index) => *index == i,
        WorkspaceSelector::Name(n) => Some(n.as_str()) == name,
    }
}
/// Give workspace `i` its own layout, or let it follow the default with `None`.
pub(super) fn set_workspace_layout(s: &mut ConfigSource, i: usize, mode: Option<LayoutMode>) {
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
}

/// The layout workspace `i` sets for itself, if any.
pub(super) fn workspace_layout(s: &ConfigSource, i: usize) -> Option<LayoutMode> {
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
    if model.env.installed_applications.borrow().is_none() {
        model.env.send(Action::RefreshRuntime, Box::new(|_| {}));
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
    let names = Rc::downgrade(&table);
    let inventory_refresh = Rc::downgrade(&refresh);
    model.env.application_views.borrow_mut().push(Box::new(move || {
        let (Some(table), Some(refresh)) = (names.upgrade(), inventory_refresh.upgrade()) else {
            return false;
        };
        refresh();
        table.reload();
        true
    }));
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
    let refreshing = Rc::new(Cell::new(false));
    let selecting = refreshing.clone();
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
        .on_select_item(move |app| {
            if !selecting.get() {
                choose(app.map(|app| app.target.clone()));
            }
        }),
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
    let weak_env = Rc::downgrade(&model.env);
    let weak_list = Rc::downgrade(&list);
    let weak_search = Rc::downgrade(&search);
    model.env.application_views.borrow_mut().push(Box::new(move || {
        let (Some(env), Some(list), Some(search)) =
            (weak_env.upgrade(), weak_list.upgrade(), weak_search.upgrade())
        else {
            return false;
        };
        let rows = env.inventory.borrow().matching(&search.get_value().trim().to_lowercase());
        let selected = list
            .selected_row()
            .and_then(|selected| rows.iter().position(|app| app.target == selected.target));
        // Native reload/selection notifications must not change the user's matcher.
        refreshing.set(true);
        list.set_rows_if_changed(rows);
        list.set_selected(selected);
        refreshing.set(false);
        true
    }));
    if model.env.installed_applications.borrow().is_none() {
        model.env.send(Action::RefreshRuntime, Box::new(|_| {}));
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
    // A popover sizes to its content, and the search field and list have no width of their own.
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
            control: None,
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
    // The sheet fits its content; give the application list room to read.
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

/// A connected display’s name.
pub(super) fn display_name(displays: &[ScreenInfo], uuid: &str) -> Option<String> {
    let display = displays.iter().find(|display| display.display_uuid == uuid)?;
    Some(
        display
            .name
            .clone()
            .unwrap_or_else(|| format!("Display {}", display.id.as_u32())),
    )
}

/// Whether `uuid` has any saved spacing or Scrolling width overrides.
fn has_overrides(source: &ConfigSource, uuid: &str) -> bool {
    let layout = &source.settings.layout;
    layout.gaps.per_display.contains_key(uuid) || layout.scrolling.per_display.contains_key(uuid)
}

/// Inline display overrides: a display popup, then that display’s spacing and Scrolling
/// column widths. Connected displays come first; disconnected ones stay listed while
/// they have saved overrides, and Add Display… accepts any display’s UUID, so settings
/// can be prepared before it’s connected. Each display’s editor is built when chosen.
pub(super) fn display_inspector(ui: Ui, model: &Rc<Model>) -> Page {
    struct State {
        ui: Ui,
        model: Weak<Model>,
        popup: Rc<Popup>,
        host: Rc<PageHost>,
        status: Caption,
        adding: Rc<VStack>,
        uuid: Rc<TextField>,
        /// (uuid, connected) for each popup choice, before Add Display….
        choices: RefCell<Vec<(String, bool)>>,
        /// Displays added by UUID this session, listed until they have saved settings.
        added: RefCell<Vec<String>>,
        selected: RefCell<Option<String>>,
        pages: RefCell<std::collections::HashMap<String, Page>>,
    }
    impl State {
        fn refresh(&self, source: &ConfigSource) {
            let Some(model) = self.model.upgrade() else { return };
            let displays = model.env.displays.borrow();
            let layout = &source.settings.layout;
            let mut choices: Vec<_> =
                displays.iter().map(|d| (d.display_uuid.clone(), true)).collect();
            let mut saved: Vec<_> = layout
                .gaps
                .per_display
                .keys()
                .chain(layout.scrolling.per_display.keys())
                .chain(self.added.borrow().iter())
                .filter(|uuid| !displays.iter().any(|d| d.display_uuid == **uuid))
                .map(|uuid| (uuid.clone(), false))
                .collect();
            saved.sort();
            saved.dedup();
            let title = |(uuid, _): &(String, bool)| {
                display_name(&displays, uuid).unwrap_or_else(|| {
                    format!(
                        "Display {}",
                        uuid.get(..8).map_or(uuid.clone(), |id| format!("{id}…"))
                    )
                })
            };
            self.popup.set_sections([
                choices.iter().map(title).collect::<Vec<_>>(),
                saved.iter().map(title).collect(),
                vec!["Add Display…".into()],
            ]);
            drop(displays);
            choices.extend(saved);
            for (index, (uuid, _)) in choices.iter().enumerate() {
                self.popup
                    .set_item_badge(index, has_overrides(source, uuid).then_some("Customized"));
            }
            let current = self
                .selected
                .borrow()
                .clone()
                .filter(|uuid| choices.iter().any(|c| c.0 == *uuid));
            let current = current.or_else(|| choices.first().map(|c| c.0.clone()));
            *self.choices.borrow_mut() = choices;
            self.show(current);
        }

        fn show(&self, uuid: Option<String>) {
            let Some(model) = self.model.upgrade() else { return };
            if *self.selected.borrow() != uuid
                && !super::layouts::finish_editing(self.host.ns_view())
            {
                return self.select_current();
            }
            self.host.set_hidden(uuid.is_none());
            *self.selected.borrow_mut() = uuid.clone();
            self.select_current();
            let Some(uuid) = uuid else {
                self.status.set_text(
                    "No displays are connected. Add one by its UUID to prepare its settings.",
                );
                return self.status.set_hidden(false);
            };
            let connected =
                self.choices.borrow().iter().any(|(id, connected)| *id == uuid && *connected);
            self.status
                .set_text("Not connected. These settings apply when this display connects.");
            self.status.set_hidden(connected);
            let mut pages = self.pages.borrow_mut();
            let page = pages
                .entry(uuid.clone())
                .or_insert_with(|| display_options(self.ui, &model, uuid));
            page.synchronize(&model);
            self.host.set_cached_page(page.view.clone());
        }

        /// Point the popup back at the shown display, such as after Add Display….
        fn select_current(&self) {
            let selected = self.selected.borrow();
            if let Some(index) =
                self.choices.borrow().iter().position(|c| Some(&c.0) == selected.as_ref())
            {
                self.popup.set_selected(index);
            }
        }

        fn add(&self) {
            let uuid = self.uuid.get_value().trim().to_owned();
            let Some(model) = self.model.upgrade() else { return };
            if uuid.is_empty() {
                return;
            }
            if !self.added.borrow().contains(&uuid) {
                self.added.borrow_mut().push(uuid.clone());
            }
            self.uuid.set_value("");
            self.adding.set_hidden(true);
            *self.selected.borrow_mut() = Some(uuid);
            self.refresh(&model.source.borrow());
        }
    }
    let state = Rc::new_cyclic(|weak: &Weak<State>| {
        let owner = weak.clone();
        let popup = Rc::new(Popup::new(&ui).on_change(move |index| {
            let Some(state) = owner.upgrade() else { return };
            let uuid = state.choices.borrow().get(index).map(|choice| choice.0.clone());
            match uuid {
                Some(uuid) => state.show(Some(uuid)),
                // Add Display…: keep showing the current display while a UUID is entered.
                None => {
                    state.select_current();
                    state.adding.set_hidden(false);
                    state.uuid.focus();
                }
            }
        }));
        popup.accessibility_label("Display");
        let uuid = Rc::new(TextField::new(&ui).placeholder("Display UUID"));
        uuid.accessibility_label("Display UUID");
        let owner = weak.clone();
        let add = Button::new(&ui, "Add").push().key_equivalent("\r").on_click(move || {
            if let Some(state) = owner.upgrade() {
                state.add();
            }
        });
        let owner = weak.clone();
        let cancel =
            Button::new(&ui, "Cancel").push().key_equivalent("\u{1b}").on_click(move || {
                if let Some(state) = owner.upgrade() {
                    state.adding.set_hidden(true);
                }
            });
        let adding = Rc::new(
            VStack::new(&ui)
                .spacing(4.0)
                .push(uuid.clone())
                .push(HStack::new(&ui).push(Spacer::new(&ui)).push(cancel).push(add))
                .push({
                    let hint =
                        Caption::new(&ui, "Find a display’s UUID with rift-cli query displays.");
                    hint.ns_text_field().setAlignment(objc2_app_kit::NSTextAlignment::Right);
                    hint
                }),
        );
        adding.set_hidden(true);
        State {
            ui,
            model: Rc::downgrade(model),
            popup,
            host: Rc::new(PageHost::content_sized(&ui)),
            status: {
                let status = Caption::new(&ui, "");
                status.ns_text_field().setAlignment(objc2_app_kit::NSTextAlignment::Right);
                status
            },
            adding,
            uuid,
            choices: RefCell::default(),
            added: RefCell::default(),
            selected: RefCell::default(),
            pages: RefCell::default(),
        }
    });
    let mut f = FormBuilder::new(ui, model);
    let weak = Rc::downgrade(&state);
    f.sync.push(Box::new(move |source| {
        if let Some(state) = weak.upgrade() {
            state.refresh(source);
        }
    }));
    let control = VStack::new(&ui)
        .spacing(6.0)
        .push(HStack::new(&ui).push(Spacer::new(&ui)).push(state.popup.clone()))
        .push(state.adding.clone())
        .push(objc2::Message::retain(state.status.ns_view()));
    let row = SettingsRow::new(&ui, "Display", control).label_baseline(state.popup.ns_view());
    let page = VStack::new(&ui)
        .spacing(10.0)
        .push(Section::form(&ui, "").row(row))
        .push(state.host.clone())
        .keep(state);
    super::pages::align_form(page.ns_view());
    f.finish(page)
}

/// One display’s overrides. Empty fields show the inherited value in gray; entering a
/// value overrides it, and clearing it, or Use Shared, inherits again. Spacing groups
/// and each Scrolling width inherit independently, as the configuration stores them.
fn display_options(ui: Ui, model: &Rc<Model>, uuid: String) -> Page {
    use super::pages::{AXES, EDGES, set_spacing_gap, spacing_gap};
    let mut f = FormBuilder::new(ui, model);
    let mut section = Section::form(&ui, "");
    for outer in [true, false] {
        let names: &[&str] = if outer { &EDGES } else { &AXES };
        let message = Rc::new(ValidationMessage::new(&ui));
        let id = uuid.clone();
        let overridden = move |s: &ConfigSource| {
            s.settings.layout.gaps.per_display.get(&id).is_some_and(|o| {
                if outer {
                    o.outer.is_some()
                } else {
                    o.inner.is_some()
                }
            })
        };
        let fields = names
            .iter()
            .enumerate()
            .map(|(axis, name)| {
                let (get_id, set_id) = (uuid.clone(), uuid.clone());
                let overridden = overridden.clone();
                (
                    *name,
                    inherited_field(
                        &mut f,
                        &format!(
                            "{}, {name}",
                            if outer {
                                "Screen edges"
                            } else {
                                "Between windows"
                            }
                        ),
                        "pt",
                        1.0,
                        Rc::downgrade(&message),
                        move |s| {
                            (
                                overridden(s)
                                    .then(|| spacing_gap(s, Some(&get_id), outer, Some(axis))),
                                spacing_gap(s, None, outer, Some(axis)),
                            )
                        },
                        move |s, value| match value {
                            Some(value) => {
                                set_spacing_gap(s, Some(&set_id), outer, Some(axis), value)
                            }
                            None => clear_gaps(s, &set_id, outer),
                        },
                    ),
                )
            })
            .collect();
        let id = uuid.clone();
        section = section.row(override_group(
            &mut f,
            (
                if outer {
                    "Screen edges"
                } else {
                    "Between windows"
                },
                "Shared settings",
                "Use Shared",
            ),
            "pt",
            fields,
            message,
            overridden,
            move |s| clear_gaps(s, &id, outer),
        ));
    }
    const WIDTHS: [&str; 3] = ["Default", "Minimum", "Maximum"];
    fn width_mut(o: &mut ScrollingWidthOverride, field: usize) -> &mut Option<f64> {
        match field {
            0 => &mut o.column_width_ratio,
            1 => &mut o.min_column_width_ratio,
            _ => &mut o.max_column_width_ratio,
        }
    }
    fn inherited_width(s: &ScrollingLayoutSettings, field: usize) -> f64 {
        [
            s.column_width_ratio,
            s.min_column_width_ratio,
            s.max_column_width_ratio,
        ][field]
    }
    let message = Rc::new(ValidationMessage::new(&ui));
    let fields = (0..3)
        .map(|field| {
            let (get_id, set_id) = (uuid.clone(), uuid.clone());
            (
                WIDTHS[field],
                inherited_field(
                    &mut f,
                    &format!("{} column width", WIDTHS[field]),
                    "%",
                    100.0,
                    Rc::downgrade(&message),
                    move |s| {
                        let scrolling = &s.settings.layout.scrolling;
                        let own = scrolling.per_display.get(&get_id).and_then(|o| {
                            [
                                o.column_width_ratio,
                                o.min_column_width_ratio,
                                o.max_column_width_ratio,
                            ][field]
                        });
                        (own, inherited_width(scrolling, field))
                    },
                    move |s, value| {
                        let scrolling = &mut s.settings.layout.scrolling;
                        let inherited = inherited_width(scrolling, field);
                        let o = scrolling.per_display.entry(set_id.clone()).or_default();
                        *width_mut(o, field) = value.filter(|v| (*v - inherited).abs() > 1e-9);
                        if (0..3).all(|field| width_mut(o, field).is_none()) {
                            scrolling.per_display.remove(&set_id);
                        }
                    },
                ),
            )
        })
        .collect();
    let id = uuid.clone();
    let clear_id = uuid.clone();
    section = section.row(override_group(
        &mut f,
        (
            "Scrolling column widths",
            "the Scrolling layout",
            "Use Layout Widths",
        ),
        "%",
        fields,
        message,
        move |s| s.settings.layout.scrolling.per_display.contains_key(&id),
        move |s| {
            s.settings.layout.scrolling.per_display.remove(&clear_id);
        },
    ));
    let preview = super::pages::gap_preview(ui, &mut f, Some(uuid.clone()), super::pages::PREVIEW);
    let page = VStack::new(&ui)
        .spacing(10.0)
        .push(super::pages::stage(&ui, preview, None))
        .push(section)
        .push(Caption::new(
            &ui,
            "Empty fields inherit. Enter a value to customize it for this display.",
        ));
    super::pages::align_form(page.ns_view());
    f.finish(page)
}

fn clear_gaps(s: &mut ConfigSource, uuid: &str, outer: bool) {
    let gaps = &mut s.settings.layout.gaps;
    if let Some(o) = gaps.per_display.get_mut(uuid) {
        if outer {
            o.outer = None
        } else {
            o.inner = None
        }
        if o.outer.is_none() && o.inner.is_none() {
            gaps.per_display.remove(uuid);
        }
    }
}

/// Labelled fields in one row, captioned beneath like an inspector, with a Use Shared
/// button while the group is overridden.
fn override_group(
    f: &mut FormBuilder,
    (title, source, revert): (&str, &str, &str),
    unit: &str,
    fields: Vec<(&str, Rc<NumberField>)>,
    message: Rc<ValidationMessage>,
    overridden: impl Fn(&ConfigSource) -> bool + Clone + 'static,
    clear: impl Fn(&mut ConfigSource) + Send + Clone + 'static,
) -> SettingsRow {
    let ui = f.ui;
    let model = f.model.clone();
    let error = Rc::downgrade(&message);
    let reset = Rc::new(
        Button::new(&ui, "")
            .symbol("arrow.uturn.backward.circle.fill")
            .borderless()
            .on_click(move || {
                let clear = clear.clone();
                let edit = move |s: &mut ConfigSource| {
                    clear(s);
                    Ok(())
                };
                Model::submit(&model, Box::new(edit), error.clone());
            }),
    );
    reset
        .ns_button()
        .setContentTintColor(Some(&objc2_app_kit::NSColor::secondaryLabelColor()));
    reset.accessibility_label(&format!("{revert} for {}", title.to_lowercase()));
    reset.tooltip(&format!("{revert}: follow {source} again on this display"));
    // Under the label, whether this display follows the shared values or its own.
    let status = Rc::new(Caption::single_line(&ui, ""));
    let first = objc2::Message::retain(fields[0].1.ns_text_field());
    let weak_reset = Rc::downgrade(&reset);
    let source = source.to_owned();
    let current = overridden.clone();
    f.sync_when(
        &status,
        move |s| current(s),
        move |status, s| {
            let custom = overridden(s);
            status.set_text(&if custom {
                "Custom for this display".to_owned()
            } else {
                format!("From {source}")
            });
            if let Some(reset) = weak_reset.upgrade() {
                reset.set_hidden(!custom);
            }
        },
    );
    let control = field_group(&ui, unit, fields).push(reset);
    f.row(title, control, message).description_view(status).label_baseline(&first)
}

/// Compact fields in a row, each captioned beneath, as in an inspector.
pub(super) fn field_group(ui: &Ui, unit: &str, fields: Vec<(&str, Rc<NumberField>)>) -> HStack {
    fields.into_iter().fold(
        HStack::new(ui)
            .spacing(8.0)
            .alignment(objc2_app_kit::NSLayoutAttribute::FirstBaseline),
        |row, (name, field)| {
            let value = field.unit_field(ui, unit);
            value.width(56.0);
            row.push(
                VStack::new(ui)
                    .spacing(3.0)
                    .alignment(objc2_app_kit::NSLayoutAttribute::CenterX)
                    .push(value)
                    .push(Caption::single_line(ui, name))
                    .keep(field),
            )
        },
    )
}

/// A numeric field showing `get`’s inherited value (second) as a placeholder unless it’s
/// overridden (first). Values are shown multiplied by `scale`.
fn inherited_field(
    f: &mut FormBuilder,
    name: &str,
    unit: &str,
    scale: f64,
    error: Weak<ValidationMessage>,
    get: impl Fn(&ConfigSource) -> (Option<f64>, f64) + 'static,
    set: impl Fn(&mut ConfigSource, Option<f64>) + Send + Clone + 'static,
) -> Rc<NumberField> {
    let get = Rc::new(get);
    let current = get.clone();
    let change = f.assign(move |s| current(s).0, set, error);
    let input = Rc::new(
        NumberField::new(&f.ui).on_change_optional(move |value| change(value.map(|v| v / scale))),
    );
    // Assistive apps read an empty field’s placeholder: the inherited value.
    input.accessibility_label(name);
    input.tooltip(&format!("In {unit}. Leave empty to inherit."));
    let current = get.clone();
    f.sync_when(
        &input,
        move |s| current(s),
        move |input, s| match get(s) {
            (Some(value), _) => input.set_value(value * scale),
            // At most two decimals, without float noise.
            (None, inherited) => {
                input.set_inherited(&super::pages::percentage_text(inherited * scale / 100.0))
            }
        },
    );
    input
}
