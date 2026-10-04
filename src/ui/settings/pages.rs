use super::*;
use crate::common::config::{Color, *};
use crate::layout_engine::Orientation;

pub(super) fn layouts() -> Vec<(&'static str, LayoutMode)> {
    vec![
        ("Traditional", LayoutMode::Traditional),
        ("BSP", LayoutMode::Bsp),
        ("Stack", LayoutMode::Stack),
        ("Master Stack", LayoutMode::MasterStack),
        ("Scrolling", LayoutMode::Scrolling),
        ("Floating", LayoutMode::Floating),
    ]
}
fn insertion() -> Vec<(&'static str, Option<WindowInsertionPoint>)> {
    vec![
        ("Use Default", None),
        ("Next to selection", Some(WindowInsertionPoint::NextToSelection)),
        ("End of layout", Some(WindowInsertionPoint::EndOfTree)),
    ]
}
fn optional_bool() -> Vec<(&'static str, Option<bool>)> {
    vec![
        ("Use Default", None),
        ("Enabled", Some(true)),
        ("Disabled", Some(false)),
    ]
}
fn arrangement() -> Vec<(&'static str, Option<Orientation>)> {
    vec![
        ("Use Default", None),
        ("Horizontal", Some(Orientation::Horizontal)),
        ("Vertical", Some(Orientation::Vertical)),
    ]
}

pub(super) fn build(ui: Ui, model: &Rc<Model>, id: usize) -> Page {
    match id {
        0 => general(ui, model),
        1 => layout(ui, model),
        2 => super::editors::workspaces(ui, model),
        3 => super::editors::rules(ui, model),
        4 => super::editors::keyboard(ui, model),
        5 => input(ui, model),
        6 => interface(ui, model),
        _ => advanced(ui, model),
    }
}

fn general(ui: Ui, model: &Rc<Model>) -> Page {
    let mut f = FormBuilder::new(ui, model);
    let mut page = SettingsPage::new(&ui, "General");
    let mut section = Section::new(&ui, "Window behavior").row(f.switch(
        "Animate window changes",
        |s| s.settings.animate,
        |s, v| s.settings.animate = v,
    ));
    let row = f.number(
        "Duration (seconds)",
        1.0,
        |s| s.settings.animation_duration,
        |s, v| s.settings.animation_duration = v,
    );
    f.enabled(&row, |s| s.settings.animate);
    section = section.row(row);
    let row = f.number(
        "Frame rate",
        1.0,
        |s| s.settings.animation_fps,
        |s, v| s.settings.animation_fps = v,
    );
    f.enabled(&row, |s| s.settings.animate);
    section = section.row(row);
    let row = f.popup(
        "Animation easing",
        vec![
            ("Ease In Out", AnimationEasing::EaseInOut),
            ("Linear", AnimationEasing::Linear),
            ("Ease In Sine", AnimationEasing::EaseInSine),
            ("Ease Out Sine", AnimationEasing::EaseOutSine),
            ("Ease In Out Sine", AnimationEasing::EaseInOutSine),
            ("Ease In Quad", AnimationEasing::EaseInQuad),
            ("Ease Out Quad", AnimationEasing::EaseOutQuad),
            ("Ease In Out Quad", AnimationEasing::EaseInOutQuad),
            ("Ease In Cubic", AnimationEasing::EaseInCubic),
            ("Ease Out Cubic", AnimationEasing::EaseOutCubic),
            ("Ease In Out Cubic", AnimationEasing::EaseInOutCubic),
            ("Ease In Quart", AnimationEasing::EaseInQuart),
            ("Ease Out Quart", AnimationEasing::EaseOutQuart),
            ("Ease In Out Quart", AnimationEasing::EaseInOutQuart),
            ("Ease In Quint", AnimationEasing::EaseInQuint),
            ("Ease Out Quint", AnimationEasing::EaseOutQuint),
            ("Ease In Out Quint", AnimationEasing::EaseInOutQuint),
            ("Ease In Expo", AnimationEasing::EaseInExpo),
            ("Ease Out Expo", AnimationEasing::EaseOutExpo),
            ("Ease In Out Expo", AnimationEasing::EaseInOutExpo),
            ("Ease In Circ", AnimationEasing::EaseInCirc),
            ("Ease Out Circ", AnimationEasing::EaseOutCirc),
            ("Ease In Out Circ", AnimationEasing::EaseInOutCirc),
        ],
        |s| s.settings.animation_easing,
        |s, v| s.settings.animation_easing = v,
    );
    f.enabled(&row, |s| s.settings.animate);
    let section = section.row(row).row(f.switch(
        "Start with tiling disabled",
        |s| s.settings.default_disable,
        |s, v| s.settings.default_disable = v,
    ));
    page = page.section(section);
    let section = Section::new(&ui, "Focus")
        .row(f.switch(
            "Focus follows pointer",
            |s| s.settings.focus_follows_mouse,
            |s, v| s.settings.focus_follows_mouse = v,
        ))
        .row(f.switch(
            "Move pointer to focused window",
            |s| s.settings.mouse_follows_focus,
            |s, v| s.settings.mouse_follows_focus = v,
        ));
    let row = f.switch(
        "Hide pointer after focusing",
        |s| s.settings.mouse_hides_on_focus,
        |s, v| s.settings.mouse_hides_on_focus = v,
    );
    f.enabled(&row, |s| s.settings.mouse_follows_focus);
    page = page.section(section.row(row));
    let section = Section::new(&ui, "Configuration").row(f.switch(
        "Reload config when edited externally",
        |s| s.settings.hot_reload,
        |s, v| s.settings.hot_reload = v,
    ));
    page = page.section(section);
    f.finish(page)
}
fn layout(ui: Ui, model: &Rc<Model>) -> Page {
    let mut f = FormBuilder::new(ui, model);
    let mut page = SettingsPage::new(&ui, "Layouts");
    let section = Section::new(&ui, "Defaults")
        .row(f.popup(
            "Default layout",
            layouts(),
            |s| s.settings.layout.mode,
            |s, v| s.settings.layout.mode = v,
        ))
        .row(f.popup(
            "New window position",
            insertion(),
            |s| s.settings.layout.base.window_insertion_point,
            |s, v| s.settings.layout.base.window_insertion_point = v,
        ));
    page = page.section(section);
    let choices: Vec<_> = layouts().into_iter().map(|(_, mode)| mode).collect();
    let initial = choices
        .iter()
        .position(|mode| *mode == model.source.borrow().settings.layout.mode)
        .unwrap_or(0);
    let selected = Rc::new(Cell::new(initial));
    let host = Rc::new(PageHost::new(&ui));
    let panes: Rc<RefCell<Vec<Option<Page>>>> =
        Rc::new(RefCell::new((0..choices.len()).map(|_| None).collect()));
    show_layout(ui, model, &host, &panes, initial);
    let weak_model = Rc::downgrade(model);
    let weak_host = Rc::downgrade(&host);
    let weak_panes = Rc::downgrade(&panes);
    let selection = selected.clone();
    let labels = [
        "Traditional",
        "BSP",
        "Stack",
        "Master",
        "Scrolling",
        "Floating",
    ];
    let selector = SegmentedControl::new(&ui, &labels);
    selector
        .ns_segmented_control()
        .setControlSize(objc2_app_kit::NSControlSize::Small);
    selector.accessibility_label("Layout options");
    selector.set_selected(initial);
    let popup = Popup::new(&ui).items(layouts().iter().map(|(label, _)| *label));
    popup.accessibility_label("Layout options");
    popup.set_selected(initial);
    let segments = objc2::rc::Weak::new(selector.ns_segmented_control());
    let menu = objc2::rc::Weak::new(popup.ns_popup_button());
    let navigate: Rc<dyn Fn(usize)> = Rc::new(move |index| {
        if let (Some(model), Some(host), Some(panes)) =
            (weak_model.upgrade(), weak_host.upgrade(), weak_panes.upgrade())
        {
            selection.set(index);
            if let Some(control) = segments.load() {
                control.setSelectedSegment(index as isize);
            }
            if let Some(control) = menu.load() {
                control.selectItemAtIndex(index as isize);
            }
            show_layout(ui, &model, &host, &panes, index);
        }
    });
    let action = navigate.clone();
    let selector = selector.on_change(move |index| action(index));
    let popup = popup.on_change(move |index| navigate(index));
    let navigation = Section::new(&ui, "Layout Settings")
        .content(ResponsiveView::new(
            &ui,
            selector,
            HStack::new(&ui).push(Label::new(&ui, "Options for:")).push(popup),
        ))
        .content(host);
    page = page.section(navigation);
    f.sync.push(Box::new(move |source| {
        if let Some(pane) = &panes.borrow()[selected.get()] {
            for sync in &pane.sync {
                sync(source);
            }
        }
    }));
    let section = Section::subsection(&ui, "Outer gaps")
        .description("Leave blank to set each side separately.")
        .row(f.text(
            "All sides (points)",
            |s| {
                let g = &s.settings.layout.gaps.outer;
                if g.top == g.left && g.top == g.bottom && g.top == g.right {
                    g.top.to_string()
                } else {
                    String::new()
                }
            },
            |s, value| {
                if let Some(v) = optional_number(&value)? {
                    s.settings.layout.gaps.outer = OuterGaps {
                        top: v,
                        left: v,
                        bottom: v,
                        right: v,
                    };
                }
                Ok(())
            },
        ));
    let sides = VStack::new(&ui)
        .push(f.number(
            "Top (points)",
            1.0,
            |s| s.settings.layout.gaps.outer.top,
            |s, v| s.settings.layout.gaps.outer.top = v,
        ))
        .push(f.number(
            "Left (points)",
            1.0,
            |s| s.settings.layout.gaps.outer.left,
            |s, v| s.settings.layout.gaps.outer.left = v,
        ))
        .push(f.number(
            "Bottom (points)",
            1.0,
            |s| s.settings.layout.gaps.outer.bottom,
            |s, v| s.settings.layout.gaps.outer.bottom = v,
        ))
        .push(f.number(
            "Right (points)",
            1.0,
            |s| s.settings.layout.gaps.outer.right,
            |s, v| s.settings.layout.gaps.outer.right = v,
        ));
    page = page.section(section.content(Disclosure::new(&ui, "Customize sides", sides)));
    let section = Section::subsection(&ui, "Inner gaps")
        .row(f.number(
            "Horizontal (points)",
            1.0,
            |s| s.settings.layout.gaps.inner.horizontal,
            |s, v| s.settings.layout.gaps.inner.horizontal = v,
        ))
        .row(f.number(
            "Vertical (points)",
            1.0,
            |s| s.settings.layout.gaps.inner.vertical,
            |s, v| s.settings.layout.gaps.inner.vertical = v,
        ));
    page = page.section(section);
    page = super::editors::display_overrides(&mut f, page, model);
    f.finish(page)
}
fn percentage_text(ratio: f64) -> String {
    format!("{:.2}", ratio * 100.0)
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

fn show_layout(
    ui: Ui,
    model: &Rc<Model>,
    host: &PageHost,
    panes: &RefCell<Vec<Option<Page>>>,
    index: usize,
) {
    let mut panes = panes.borrow_mut();
    let pane = panes[index].get_or_insert_with(|| layout_options(ui, model, layouts()[index].1));
    let syncing = model.syncing.replace(true);
    for sync in &pane.sync {
        sync(&model.source.borrow());
    }
    model.syncing.set(syncing);
    host.set_page(pane.view.clone());
}

fn layout_options(ui: Ui, model: &Rc<Model>, mode: LayoutMode) -> Page {
    let mut f = FormBuilder::new(ui, model);
    let section = match mode {
        LayoutMode::Traditional => Section::subsection(&ui, "Traditional")
            .row(f.switch(
                "Equalize splits",
                |s| s.settings.layout.traditional.equalize_nodes,
                |s, v| s.settings.layout.traditional.equalize_nodes = v,
            ))
            .row(f.popup(
                "New window position",
                insertion(),
                |s| s.settings.layout.traditional.base.window_insertion_point,
                |s, v| s.settings.layout.traditional.base.window_insertion_point = v,
            )),
        LayoutMode::Bsp => Section::subsection(&ui, "BSP")
            .row(f.popup(
                "New window position",
                insertion(),
                |s| s.settings.layout.bsp.base.window_insertion_point,
                |s, v| s.settings.layout.bsp.base.window_insertion_point = v,
            ))
            .row(f.text(
                "Single window aspect ratio",
                |s| {
                    s.settings
                        .layout
                        .bsp
                        .single_window_aspect_ratio
                        .map(|v| v.to_string())
                        .unwrap_or_default()
                },
                |s, v| {
                    s.settings.layout.bsp.single_window_aspect_ratio = optional_number(&v)?;
                    Ok(())
                },
            )),
        LayoutMode::Stack => Section::subsection(&ui, "Stack")
            .row(f.number(
                "Window offset (points)",
                1.0,
                |s| s.settings.layout.stack.stack_offset,
                |s, v| s.settings.layout.stack.stack_offset = v,
            ))
            .row(f.popup(
                "Orientation",
                vec![
                    ("Perpendicular", StackDefaultOrientation::Perpendicular),
                    ("Same", StackDefaultOrientation::Same),
                    ("Horizontal", StackDefaultOrientation::Horizontal),
                    ("Vertical", StackDefaultOrientation::Vertical),
                ],
                |s| s.settings.layout.stack.default_orientation,
                |s, v| s.settings.layout.stack.default_orientation = v,
            ))
            .row(f.popup(
                "New window position",
                insertion(),
                |s| s.settings.layout.stack.base.window_insertion_point,
                |s, v| s.settings.layout.stack.base.window_insertion_point = v,
            )),
        LayoutMode::MasterStack => Section::subsection(&ui, "Master Stack")
            .row(
                f.number(
                    "Master width",
                    100.0,
                    |s| s.settings.layout.master_stack.master_ratio,
                    |s, v| s.settings.layout.master_stack.master_ratio = v,
                )
                .suffix("%"),
            )
            .row(f.integer(
                "Master windows",
                |s| s.settings.layout.master_stack.master_count as f64,
                |s, v| s.settings.layout.master_stack.master_count = v as usize,
            ))
            .row(f.popup(
                "Master side",
                vec![
                    ("Left", MasterStackSide::Left),
                    ("Right", MasterStackSide::Right),
                    ("Top", MasterStackSide::Top),
                    ("Bottom", MasterStackSide::Bottom),
                ],
                |s| s.settings.layout.master_stack.master_side,
                |s, v| s.settings.layout.master_stack.master_side = v,
            ))
            .row(f.popup(
                "New windows",
                vec![
                    ("Master", MasterStackNewWindowPlacement::Master),
                    ("Stack", MasterStackNewWindowPlacement::Stack),
                    ("Focused", MasterStackNewWindowPlacement::Focused),
                ],
                |s| s.settings.layout.master_stack.new_window_placement,
                |s, v| s.settings.layout.master_stack.new_window_placement = v,
            ))
            .row(f.popup(
                "Master arrangement",
                arrangement(),
                |s| s.settings.layout.master_stack.master_arrangement,
                |s, v| s.settings.layout.master_stack.master_arrangement = v,
            ))
            .row(f.popup(
                "Stack arrangement",
                arrangement(),
                |s| s.settings.layout.master_stack.stack_arrangement,
                |s, v| s.settings.layout.master_stack.stack_arrangement = v,
            ))
            .row(f.popup(
                "New window position",
                insertion(),
                |s| s.settings.layout.master_stack.base.window_insertion_point,
                |s, v| s.settings.layout.master_stack.base.window_insertion_point = v,
            )),
        LayoutMode::Scrolling => Section::subsection(&ui, "Scrolling")
            .row(
                f.number(
                    "Default column width",
                    100.0,
                    |s| s.settings.layout.scrolling.column_width_ratio,
                    |s, v| s.settings.layout.scrolling.column_width_ratio = v,
                )
                .suffix("%"),
            )
            .row(f.text(
                "Preset widths",
                |s| {
                    s.settings
                        .layout
                        .scrolling
                        .preset_column_widths
                        .iter()
                        .map(|v| percentage_text(*v))
                        .collect::<Vec<_>>()
                        .join(", ")
                },
                |s, v| {
                    let old = &s.settings.layout.scrolling.preset_column_widths;
                    let widths =
                        v.split(',')
                            .enumerate()
                            .map(|(index, value)| {
                                let value = value.trim();
                                // Keep the original precision for displayed values the user did not edit.
                                if let Some(original) =
                                    old.get(index).filter(|v| percentage_text(**v) == value)
                                {
                                    return Ok(*original);
                                }
                                value.parse::<f64>().map(|v| v / 100.0).map_err(|_| {
                                    "Enter percentages separated by commas".to_string()
                                })
                            })
                            .collect::<Result<_, _>>()?;
                    s.settings.layout.scrolling.preset_column_widths = widths;
                    Ok(())
                },
            ))
            .row(f.switch(
                "Preserve window sizes",
                |s| s.settings.layout.scrolling.preserve_window_sizes,
                |s, v| s.settings.layout.scrolling.preserve_window_sizes = v,
            ))
            .row(
                f.number(
                    "Minimum width",
                    100.0,
                    |s| s.settings.layout.scrolling.min_column_width_ratio,
                    |s, v| s.settings.layout.scrolling.min_column_width_ratio = v,
                )
                .suffix("%"),
            )
            .row(
                f.number(
                    "Maximum width",
                    100.0,
                    |s| s.settings.layout.scrolling.max_column_width_ratio,
                    |s, v| s.settings.layout.scrolling.max_column_width_ratio = v,
                )
                .suffix("%"),
            )
            .row(f.popup(
                "Alignment",
                vec![
                    ("Left", ScrollingAlignment::Left),
                    ("Center", ScrollingAlignment::Center),
                    ("Right", ScrollingAlignment::Right),
                ],
                |s| s.settings.layout.scrolling.alignment,
                |s, v| s.settings.layout.scrolling.alignment = v,
            ))
            .row(f.popup(
                "Focus navigation",
                vec![
                    ("Niri", ScrollingFocusNavigationStyle::Niri),
                    ("Anchored", ScrollingFocusNavigationStyle::Anchored),
                ],
                |s| s.settings.layout.scrolling.focus_navigation_style,
                |s, v| s.settings.layout.scrolling.focus_navigation_style = v,
            ))
            .row(f.popup(
                "Animate navigation",
                optional_bool(),
                |s| s.settings.layout.scrolling.animate,
                |s, v| s.settings.layout.scrolling.animate = v,
            ))
            .row(f.popup(
                "New window position",
                insertion(),
                |s| s.settings.layout.scrolling.base.window_insertion_point,
                |s, v| s.settings.layout.scrolling.base.window_insertion_point = v,
            )),
        LayoutMode::Floating => {
            Section::subsection(&ui, "Floating").description("No layout-specific options.")
        }
    };
    f.finish(section)
}

fn input(ui: Ui, model: &Rc<Model>) -> Page {
    let mut f = FormBuilder::new(ui, model);
    let mut page = SettingsPage::new(&ui, "Mouse & Trackpad");
    let section = Section::new(&ui, "Workspace swipes")
        .row(f.switch(
            "Enabled",
            |s| s.settings.gestures.enabled,
            |s, v| s.settings.gestures.enabled = v,
        ))
        .row(f.switch(
            "Consume macOS workspace swipe",
            |s| s.settings.gestures.consume_dock_swipe,
            |s, v| s.settings.gestures.consume_dock_swipe = v,
        ))
        .row(f.switch(
            "Invert direction",
            |s| s.settings.gestures.invert_horizontal_swipe,
            |s, v| s.settings.gestures.invert_horizontal_swipe = v,
        ))
        .row(f.switch(
            "Skip empty workspaces",
            |s| s.settings.gestures.skip_empty,
            |s, v| s.settings.gestures.skip_empty = v,
        ))
        .row(f.switch(
            "Haptic feedback",
            |s| s.settings.gestures.haptics_enabled,
            |s, v| s.settings.gestures.haptics_enabled = v,
        ))
        .row(f.integer(
            "Fingers",
            |s| s.settings.gestures.fingers as f64,
            |s, v| s.settings.gestures.fingers = v as usize,
        ));
    let tuning = VStack::new(&ui)
        .push(f.number(
            "Distance (%)",
            100.0,
            |s| s.settings.gestures.distance_pct,
            |s, v| s.settings.gestures.distance_pct = v,
        ))
        .push(f.number(
            "Vertical tolerance",
            1.0,
            |s| s.settings.gestures.swipe_vertical_tolerance,
            |s, v| s.settings.gestures.swipe_vertical_tolerance = v,
        ))
        .push(f.popup(
            "Haptic pattern",
            vec![
                ("Generic", HapticPattern::Generic),
                ("Alignment", HapticPattern::Alignment),
                ("Level change", HapticPattern::LevelChange),
            ],
            |s| s.settings.gestures.haptic_pattern,
            |s, v| s.settings.gestures.haptic_pattern = v,
        ));
    page = page.section(section.content(Disclosure::new(&ui, "Advanced swipe settings", tuning)));
    let section = Section::new(&ui, "Scrolling layout gestures")
        .description("These gestures navigate the Scrolling layout strip.")
        .row(f.switch(
            "Enabled",
            |s| s.settings.layout.scrolling.gestures.enabled,
            |s, v| s.settings.layout.scrolling.gestures.enabled = v,
        ))
        .row(f.switch(
            "Invert horizontal direction",
            |s| s.settings.layout.scrolling.gestures.invert_horizontal,
            |s, v| s.settings.layout.scrolling.gestures.invert_horizontal = v,
        ))
        .row(f.integer(
            "Fingers",
            |s| s.settings.layout.scrolling.gestures.fingers as f64,
            |s, v| s.settings.layout.scrolling.gestures.fingers = v as usize,
        ))
        .row(f.switch(
            "Continue into workspace swipe",
            |s| s.settings.layout.scrolling.gestures.propagate_to_workspace_swipe,
            |s, v| s.settings.layout.scrolling.gestures.propagate_to_workspace_swipe = v,
        ))
        .row(f.popup(
            "Animate gestures",
            optional_bool(),
            |s| s.settings.layout.scrolling.gestures.animate,
            |s, v| s.settings.layout.scrolling.gestures.animate = v,
        ))
        .row(f.number(
            "Vertical tolerance",
            1.0,
            |s| s.settings.layout.scrolling.gestures.vertical_tolerance,
            |s, v| s.settings.layout.scrolling.gestures.vertical_tolerance = v,
        ))
        .row(f.number(
            "Workspace switch threshold (%)",
            100.0,
            |s| s.settings.layout.scrolling.gestures.workspace_switch_threshold,
            |s, v| s.settings.layout.scrolling.gestures.workspace_switch_threshold = v,
        ));
    page = page.section(section);
    let section = Section::new(&ui, "Drag & Drop")
        .row(f.switch(
            "Enabled",
            |s| s.settings.drag_drop.enabled,
            |s, v| s.settings.drag_drop.enabled = v,
        ))
        .row(f.popup(
            "Modifier",
            vec![
                ("⌘ Command", MouseModifier::Cmd),
                ("⌥ Option", MouseModifier::Alt),
                ("⇧ Shift", MouseModifier::Shift),
                ("⌃ Control", MouseModifier::Ctrl),
                ("Fn", MouseModifier::Fn),
            ],
            |s| s.settings.drag_drop.modifier,
            |s, v| s.settings.drag_drop.modifier = v,
        ))
        .row(f.popup(
            "Primary button",
            vec![
                ("Move window", MouseAction::Move),
                ("Pass through", MouseAction::None),
            ],
            |s| s.settings.drag_drop.action1,
            |s, v| s.settings.drag_drop.action1 = v,
        ))
        .row(f.popup(
            "Secondary button",
            vec![
                ("Move window", MouseAction::Move),
                ("Pass through", MouseAction::None),
            ],
            |s| s.settings.drag_drop.action2,
            |s, v| s.settings.drag_drop.action2 = v,
        ))
        .row(f.popup(
            "Drop in center",
            vec![
                ("Swap", MouseDropAction::Swap),
                ("Stack", MouseDropAction::Stack),
            ],
            |s| s.settings.drag_drop.drop_action,
            |s, v| s.settings.drag_drop.drop_action = v,
        ))
        .row(f.number(
            "Center drop zone (%)",
            100.0,
            |s| s.settings.drag_drop.drop_zone_fraction,
            |s, v| s.settings.drag_drop.drop_zone_fraction = v,
        ))
        .row(f.switch(
            "Show drop preview",
            |s| s.settings.drag_drop.preview,
            |s, v| s.settings.drag_drop.preview = v,
        ))
        .row(f.switch(
            "Haptic feedback",
            |s| s.settings.drag_drop.haptics_enabled,
            |s, v| s.settings.drag_drop.haptics_enabled = v,
        ));
    page = page.section(section);
    let section = Section::new(&ui, "Pointer movement").row(f.popup(
        "Horizontal pointer warp",
        vec![
            ("Disabled", None),
            ("Top to bottom", Some(HorizontalMouseWarp::TopToBottom)),
            ("Bottom to top", Some(HorizontalMouseWarp::BottomToTop)),
        ],
        |s| s.settings.horizontal_mouse_warp,
        |s, v| s.settings.horizontal_mouse_warp = v,
    ));
    page = page.section(section);
    page = page.section(focus_suspend(&mut f));
    f.finish(page)
}
fn interface(ui: Ui, model: &Rc<Model>) -> Page {
    let mut f = FormBuilder::new(ui, model);
    let mut page = SettingsPage::new(&ui, "Interface");
    let section = Section::new(&ui, "Menu Bar")
        .row(f.switch(
            "Show menu bar indicator",
            |s| s.settings.ui.menu_bar.enabled,
            |s, v| s.settings.ui.menu_bar.enabled = v,
        ))
        .row(f.switch(
            "Show empty workspaces",
            |s| s.settings.ui.menu_bar.show_empty,
            |s, v| s.settings.ui.menu_bar.show_empty = v,
        ))
        .row(f.popup(
            "Workspaces to show",
            vec![
                ("All", MenuBarDisplayMode::All),
                ("Active", MenuBarDisplayMode::Active),
            ],
            |s| s.settings.ui.menu_bar.mode,
            |s, v| s.settings.ui.menu_bar.mode = v,
        ))
        .row(f.popup(
            "Active workspace label",
            vec![
                ("Index", ActiveWorkspaceLabel::Index),
                ("Name", ActiveWorkspaceLabel::Name),
            ],
            |s| s.settings.ui.menu_bar.active_label,
            |s, v| s.settings.ui.menu_bar.active_label = v,
        ))
        .row(f.popup(
            "Display style",
            vec![
                ("Layout", WorkspaceDisplayStyle::Layout),
                ("Label", WorkspaceDisplayStyle::Label),
            ],
            |s| s.settings.ui.menu_bar.display_style,
            |s, v| s.settings.ui.menu_bar.display_style = v,
        ));
    let message = Rc::new(ValidationMessage::new(&ui));
    let error = Rc::downgrade(&message);
    let weak_model = f.model.clone();
    let path = Rc::new(PathField::new(&ui).directories().on_change(move |path| {
        FormBuilder::submit(
            &weak_model,
            Box::new(move |s| {
                s.settings.ui.menu_bar.layout_folder = path;
                Ok(())
            }),
            error.clone(),
        )
    }));
    let weak_path = Rc::downgrade(&path);
    f.sync.push(Box::new(move |s| {
        if let Some(path) = weak_path.upgrade() {
            path.set_value(&s.settings.ui.menu_bar.layout_folder);
        }
    }));
    page = page.section(section.row(f.row("Layout folder", path, message)));
    let section = Section::new(&ui, "Overview")
        .description("Changing whether Overview is enabled requires restarting Rift.")
        .row(f.switch(
            "Enabled (restart required)",
            |s| s.settings.ui.mission_control.enabled,
            |s, v| s.settings.ui.mission_control.enabled = v,
        ))
        .row(f.switch(
            "Show empty workspaces",
            |s| s.settings.ui.mission_control.show_empty_workspaces,
            |s, v| s.settings.ui.mission_control.show_empty_workspaces = v,
        ))
        .row(f.switch(
            "Window previews",
            |s| s.settings.ui.mission_control.window_previews,
            |s, v| s.settings.ui.mission_control.window_previews = v,
        ))
        .row(f.switch(
            "Fade transitions",
            |s| s.settings.ui.mission_control.fade_enabled,
            |s, v| s.settings.ui.mission_control.fade_enabled = v,
        ))
        .row(f.number(
            "Fade duration (milliseconds)",
            1.0,
            |s| s.settings.ui.mission_control.fade_duration_ms,
            |s, v| s.settings.ui.mission_control.fade_duration_ms = v,
        ));
    page = page.section(section);
    let section = Section::new(&ui, "Stack Line")
        .description("Experimental")
        .row(f.switch(
            "Enabled",
            |s| s.settings.ui.stack_line.enabled,
            |s, v| s.settings.ui.stack_line.enabled = v,
        ))
        .row(f.popup(
            "Interaction",
            vec![
                ("Hover", StackLineHoverMode::Hover),
                ("Click", StackLineHoverMode::Click),
            ],
            |s| s.settings.ui.stack_line.hover,
            |s, v| s.settings.ui.stack_line.hover = v,
        ))
        .row(f.popup(
            "Horizontal placement",
            vec![
                ("Top", HorizontalPlacement::Top),
                ("Bottom", HorizontalPlacement::Bottom),
            ],
            |s| s.settings.ui.stack_line.horiz_placement,
            |s, v| s.settings.ui.stack_line.horiz_placement = v,
        ))
        .row(f.popup(
            "Vertical placement",
            vec![
                ("Left", VerticalPlacement::Left),
                ("Right", VerticalPlacement::Right),
            ],
            |s| s.settings.ui.stack_line.vert_placement,
            |s, v| s.settings.ui.stack_line.vert_placement = v,
        ))
        .row(f.number(
            "Thickness (points)",
            1.0,
            |s| s.settings.ui.stack_line.thickness,
            |s, v| s.settings.ui.stack_line.thickness = v,
        ))
        .row(f.number(
            "Spacing (points)",
            1.0,
            |s| s.settings.ui.stack_line.spacing,
            |s, v| s.settings.ui.stack_line.spacing = v,
        ));
    let section = section.row(f.color(
        "Selected color",
        |s| s.settings.ui.stack_line.selected_color,
        |s, v| s.settings.ui.stack_line.selected_color = v,
    ));
    let section = section.row(f.color(
        "Unselected color",
        |s| s.settings.ui.stack_line.unselected_color,
        |s, v| s.settings.ui.stack_line.unselected_color = v,
    ));
    let section = section.row(f.color(
        "Border color",
        |s| s.settings.ui.stack_line.border_color,
        |s, v| s.settings.ui.stack_line.border_color = v,
    ));
    page = page.section(section);
    f.finish(page)
}
fn advanced(ui: Ui, model: &Rc<Model>) -> Page {
    let mut f = FormBuilder::new(ui, model);
    let mut page = SettingsPage::new(&ui, "Advanced");
    page = page.section(super::editors::strings(
        &mut f,
        "Startup commands",
        |s| s.settings.run_on_start.clone(),
        |s, v| s.settings.run_on_start = v,
    ));
    page = page.section(super::editors::strings(
        &mut f,
        "Autofocus blacklist",
        |s| s.settings.auto_focus_blacklist.clone(),
        |s, v| s.settings.auto_focus_blacklist = v,
    ));
    let path = model.config_path.clone();
    let open = path.clone();
    let reveal = path.clone();
    let actions = HStack::new(&ui)
        .push(Button::new(&ui, "Open Config File").on_click(move || {
            let _ = std::process::Command::new("open").arg(&open).spawn();
        }))
        .push(Button::new(&ui, "Reveal in Finder").on_click(move || {
            let _ = std::process::Command::new("open").arg("-R").arg(&reveal).spawn();
        }));
    let message = Rc::new(ValidationMessage::new(&ui));
    let error = Rc::downgrade(&message);
    let weak_model = Rc::downgrade(model);
    let reload = Button::new(&ui, "Reload From Disk").on_click(move || {
        if let Some(model) = weak_model.upgrade() {
            let weak_model = Rc::downgrade(&model);
            let error = error.clone();
            let _ = model.requests.send(Request {
                action: Action::Reload,
                finish: Box::new(move |result| {
                    if let Some(message) = error.upgrade() {
                        message.set_validation(&match &result {
                            Ok(_) => Validation::None,
                            Err(e) => Validation::Error(e.clone()),
                        });
                    }
                    if let (Some(model), Ok(source)) = (weak_model.upgrade(), result) {
                        *model.source.borrow_mut() = source;
                    }
                }),
            });
        }
    });
    page = page.section(
        Section::new(&ui, "Configuration file")
            .description(&path.to_string_lossy())
            .content(actions.push(reload))
            .footer(message),
    );
    f.finish(page)
}

pub(super) fn optional_number(value: &str) -> Result<Option<f64>, String> {
    if value.trim().is_empty() {
        return Ok(None);
    }
    value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .map(Some)
        .ok_or_else(|| "Enter a number, or leave blank to use Default".into())
}

impl FormBuilder {
    fn color(
        &mut self,
        title: &str,
        get: impl Fn(&ConfigSource) -> Color + 'static,
        set: impl Fn(&mut ConfigSource, Color) + Send + Copy + 'static,
    ) -> SettingsRow {
        use objc2_app_kit::{NSColor, NSColorSpace};
        let message = Rc::new(ValidationMessage::new(&self.ui));
        let error = Rc::downgrade(&message);
        let model = self.model.clone();
        let input = Rc::new(ColorWell::new(&self.ui).on_change(move |v| {
            if let Some(v) = v.colorUsingColorSpace(&NSColorSpace::sRGBColorSpace()) {
                let color = Color::new(
                    v.redComponent(),
                    v.greenComponent(),
                    v.blueComponent(),
                    v.alphaComponent(),
                );
                Self::submit(
                    &model,
                    Box::new(move |s| {
                        set(s, color);
                        Ok(())
                    }),
                    error.clone(),
                );
            }
        }));
        let weak = Rc::downgrade(&input);
        self.sync.push(Box::new(move |s| {
            if let Some(input) = weak.upgrade() {
                let c = get(s);
                input.set_value(&NSColor::colorWithSRGBRed_green_blue_alpha(c.r, c.g, c.b, c.a));
            }
        }));
        self.row(title, input, message)
    }
}

fn focus_suspend(f: &mut FormBuilder) -> Section {
    use crate::sys::hotkey::{Hotkey, HotkeySpec, Modifiers as RiftModifiers};
    fn modifiers(flags: Modifiers) -> RiftModifiers {
        let mut value = RiftModifiers::empty();
        for (flag, bits) in [
            (Modifiers::Control, RiftModifiers::CONTROL),
            (Modifiers::Option, RiftModifiers::ALT),
            (Modifiers::Shift, RiftModifiers::SHIFT),
            (Modifiers::Command, RiftModifiers::META),
        ] {
            if flags.contains(flag) {
                value.insert(bits);
            }
        }
        value
    }
    let ui = f.ui;
    let message = Rc::new(ValidationMessage::new(&ui));
    let error = Rc::downgrade(&message);
    let model = f.model.clone();
    let recorder = Rc::new(KeyRecorder::new(&ui).on_change(move |value| {
        let value = value
            .as_ref()
            .and_then(super::commands::recorded_key)
            .and_then(|v| v.parse::<Hotkey>().ok())
            .map(HotkeySpec::Hotkey);
        FormBuilder::submit(
            &model,
            Box::new(move |s| {
                s.settings.focus_follows_mouse_disable_hotkey = value;
                Ok(())
            }),
            error.clone(),
        );
    }));
    let error = Rc::downgrade(&message);
    let model = f.model.clone();
    let modifier_recorder = Rc::new(ModifierRecorder::new(&ui).on_change(move |flags| {
        let value =
            (!flags.is_empty()).then(|| HotkeySpec::ModifiersOnly { modifiers: modifiers(flags) });
        FormBuilder::submit(
            &model,
            Box::new(move |s| {
                s.settings.focus_follows_mouse_disable_hotkey = value;
                Ok(())
            }),
            error.clone(),
        );
    }));
    let weak = Rc::downgrade(&recorder);
    let weak_modifiers = Rc::downgrade(&modifier_recorder);
    f.sync.push(Box::new(move |s| {
        if let Some(recorder) = weak.upgrade() {
            recorder.set_value(None);
            if let Some(HotkeySpec::Hotkey(key)) = &s.settings.focus_follows_mouse_disable_hotkey {
                recorder
                    .ns_button()
                    .setTitle(&objc2_foundation::NSString::from_str(&key.to_string()));
            }
            recorder.set_enabled(s.settings.focus_follows_mouse);
        }
        if let Some(recorder) = weak_modifiers.upgrade() {
            let mut flags = Modifiers::empty();
            if let Some(HotkeySpec::ModifiersOnly { modifiers }) =
                &s.settings.focus_follows_mouse_disable_hotkey
            {
                for (bits, flag) in [
                    (RiftModifiers::CONTROL, Modifiers::Control),
                    (RiftModifiers::ALT, Modifiers::Option),
                    (RiftModifiers::SHIFT, Modifiers::Shift),
                    (RiftModifiers::META, Modifiers::Command),
                ] {
                    if modifiers.intersects(bits) {
                        flags |= flag;
                    }
                }
            }
            recorder.set_value(flags);
            recorder.set_enabled(s.settings.focus_follows_mouse);
        }
    }));
    let model = f.model.clone();
    let error = Rc::downgrade(&message);
    let clear = Button::new(&ui, "Clear").on_click(move || {
        FormBuilder::submit(
            &model,
            Box::new(|s| {
                s.settings.focus_follows_mouse_disable_hotkey = None;
                Ok(())
            }),
            error.clone(),
        )
    });
    Section::new(&ui, "Suspend pointer focus while held")
        .description("Record either a shortcut or modifiers. Recording one replaces the other.")
        .row(SettingsRow::new(&ui, "Shortcut", recorder))
        .row(SettingsRow::new(&ui, "Modifiers only", modifier_recorder))
        .content(clear)
        .footer(message)
}
