use std::ops::Range;

use super::*;
use crate::common::config::{Color, *};
use crate::layout_engine::Orientation;

/// Popup choices for every layout mode.
pub(super) fn layouts() -> &'static [(&'static str, LayoutMode)] {
    static LAYOUTS: std::sync::LazyLock<Vec<(&'static str, LayoutMode)>> =
        std::sync::LazyLock::new(|| {
            LayoutMode::CONFIG_CHOICES
                .iter()
                .enumerate()
                .map(|(index, choice)| (choice.0, LayoutMode::from_config_choice(index).unwrap()))
                .collect()
        });
    &LAYOUTS
}

pub(super) fn layout_title(mode: LayoutMode) -> &'static str {
    LayoutMode::CONFIG_CHOICES[mode.config_choice_index()].0
}

fn insertion() -> Vec<(&'static str, WindowInsertionPoint)> {
    vec![
        ("Next to selection", WindowInsertionPoint::NextToSelection),
        ("End of layout", WindowInsertionPoint::EndOfTree),
    ]
}
fn bool_choices() -> Vec<(&'static str, bool)> { vec![("Enabled", true), ("Disabled", false)] }
fn arrangement() -> Vec<(&'static str, Orientation)> {
    vec![
        ("Horizontal", Orientation::Horizontal),
        ("Vertical", Orientation::Vertical),
    ]
}

pub(super) fn build(ui: Ui, model: &Rc<Model>, id: usize) -> Page {
    match id {
        0 => general(ui, model),
        1 => layout(ui, model),
        2 => super::editors::workspaces(ui, model),
        3 => super::editors::rules(ui, model),
        4 => super::commands::keyboard(ui, model),
        5 => input(ui, model),
        6 => interface(ui, model),
        8 => FormBuilder::new(ui, model)
            .finish(SettingsPage::new(&ui, "").section(about(ui, &model.env))),
        _ => advanced(ui, model),
    }
}

fn general(ui: Ui, model: &Rc<Model>) -> Page {
    let mut f = FormBuilder::new(ui, model);
    let section = f.schema_section(
        "Window behavior",
        "general",
        |s| &s.settings,
        |s| &mut s.settings,
    );
    f.finish(SettingsPage::new(&ui, "").section(section))
}

fn about(ui: Ui, env: &Rc<Env>) -> VStack {
    let status = Rc::new(Label::new(&ui, "").color(&cgs::Color::secondary_label()).wrapping());
    status.set_hidden(true);
    let weak_status = Rc::downgrade(&status);
    let weak_env = Rc::downgrade(env);
    let check = Button::new(&ui, "Check for Updates…");
    let weak_button = WeakView::new(&check);
    // Keep only weak view references in the pending request so closing Settings
    // releases the page even while the bounded network request is finishing.
    let check = check.on_click(move || {
        let Some(env) = weak_env.upgrade() else {
            return;
        };
        if let Some(status) = weak_status.upgrade() {
            status.set_hidden(false);
            status.set_text("Checking for updates…");
        }
        weak_button.set_enabled(false);
        let status = weak_status.clone();
        let button = weak_button.clone();
        env.send(
            Action::CheckUpdates(Box::new(move |result| {
                if let Some(status) = status.upgrade() {
                    status.set_text(&result.unwrap_or_else(|e| e));
                }
                button.set_enabled(true);
            })),
            Box::new(|_| {}),
        );
    });
    let identity = HStack::new(&ui).spacing(16.0);
    if let Some(icon) = Application::shared(&ui).icon() {
        let icon = ImageView::new(&ui, &icon);
        icon.width(64.0);
        icon.height(64.0);
        identity.add(icon);
    }
    identity.add(
        VStack::new(&ui)
            .spacing(4.0)
            .push(Label::new(&ui, "Rift").font(&Font::title()))
            .push(Caption::new(&ui, "A window manager for macOS"))
            .push(Caption::new(
                &ui,
                &format!("Version {}", env!("CARGO_PKG_VERSION")),
            )),
    );
    const RESOURCES: [(&str, &str, &str); 3] = [
        (
            "Documentation",
            "book",
            "https://acsandmann.github.io/rift-docs/",
        ),
        (
            "Release Notes",
            "clock.arrow.circlepath",
            "https://github.com/acsandmann/rift/releases",
        ),
        ("Sponsor Rift", "heart", "https://github.com/sponsors/acsandmann"),
    ];
    let resources = SettingsList::new(
        &ui,
        |item: &(&str, &str, &str)| item.0.to_string(),
        |_| String::new(),
    )
    .full_length()
    .navigation()
    .trailing_summary()
    .symbols(|item| item.1.to_string())
    .on_open(|index| {
        if let Some(item) = RESOURCES.get(index) {
            Application::open(item.2);
        }
    });
    resources.set_rows(RESOURCES.to_vec());
    VStack::new(&ui)
        .spacing(16.0)
        .push(identity)
        .push(
            Section::new(&ui, "")
                .row(SettingsRow::new(&ui, "Software updates", check))
                .footer(status),
        )
        .push(Section::new(&ui, "Resources").content(resources))
}

pub(super) const LAYOUT_PAGES: [(&str, &str); 9] = [
    ("Default behavior", "gearshape"),
    ("Traditional", "rectangle.split.2x2"),
    ("BSP", "rectangle.split.2x1"),
    ("Stack", "square.3.layers.3d"),
    ("Master Stack", "sidebar.left"),
    ("Scrolling", "rectangle.split.3x1"),
    ("Floating", "macwindow"),
    ("Spacing", "arrow.up.left.and.arrow.down.right"),
    ("Displays", "display"),
];
/// How the Layouts overview and toolbar menu group `LAYOUT_PAGES`.
pub(super) const LAYOUT_SECTIONS: [(&str, Range<usize>); 3] =
    [("Default", 0..1), ("Layouts", 1..7), ("Global", 7..9)];

fn layout(ui: Ui, model: &Rc<Model>) -> Page {
    let f = FormBuilder::new(ui, model);
    let mut page = SettingsPage::new(&ui, "Layouts");
    for (title, range) in LAYOUT_SECTIONS {
        let env = Rc::downgrade(&model.env);
        let start = range.start;
        let list = SettingsList::new(
            &ui,
            |index: &usize| LAYOUT_PAGES[*index].0.into(),
            |_| String::new(),
        )
        .full_length()
        .navigation()
        .trailing_summary()
        .symbols(|index| LAYOUT_PAGES[*index].1.into())
        .on_open(move |index| {
            if let Some(env) = env.upgrade() {
                env.navigate(scope_page(start + index));
            }
        });
        list.set_rows(range.collect());
        page = page.section(Section::new(&ui, title).content(list));
    }
    f.finish(page)
}

pub(super) fn layout_scope(ui: Ui, model: &Rc<Model>, index: usize) -> Page {
    match index {
        0 => layout_defaults(ui, model),
        1..=6 => layout_options(ui, model, LayoutMode::from_config_choice(index - 1).unwrap()),
        7 => layout_spacing(ui, model),
        _ => {
            let mut f = FormBuilder::new(ui, model);
            let page = super::editors::display_overrides(&mut f, SettingsPage::new(&ui, ""), model);
            f.finish(page)
        }
    }
}

fn layout_defaults(ui: Ui, model: &Rc<Model>) -> Page {
    let mut f = FormBuilder::new(ui, model);
    let section = Section::form(&ui, "Default behavior")
        .description("Used by workspaces that don’t have their own layout.")
        .row(f.field("mode", |s| &s.settings.layout, |s| &mut s.settings.layout))
        .row(f.inherited_popup(
            "New window position",
            insertion(),
            |s| s.settings.layout.base.window_insertion_point,
            |_| WindowInsertionPoint::default(),
            |s, v| s.settings.layout.base.window_insertion_point = v,
        ));
    f.finish(SettingsPage::new(&ui, "").section(section))
}

pub(super) fn gap_preview(ui: Ui, f: &mut FormBuilder, display: Option<String>) -> impl NativeView {
    // Match the grid's 128 + 120 + 82 point columns and two 12 point gaps.
    // A wider canvas was compressed by the editor while its window geometry
    // still used the original width, making equal edge insets look unequal.
    let preview = Rc::new(LayoutPreview::new(&ui, CGSize::new(354.0, 160.0)));
    let illustration = preview.clone();
    let update: Rc<dyn Fn(&ConfigSource)> = Rc::new(move |s| {
        let size = illustration.canvas_size();
        let gaps = s.settings.layout.gaps.effective_for_display(display.as_deref());
        // A representative desktop, scaled to keep even large gaps legible.
        let scaled = |v: f64| v.clamp(0.0, 200.0) * 0.25;
        let left = scaled(gaps.outer.left);
        let top = scaled(gaps.outer.top);
        let width = (size.width - left - scaled(gaps.outer.right)).max(24.0);
        let height = (size.height - top - scaled(gaps.outer.bottom)).max(24.0);
        let horizontal = scaled(gaps.inner.horizontal);
        let vertical = scaled(gaps.inner.vertical);
        let half = ((width - horizontal) / 2.0).max(8.0);
        let right = left + half + horizontal;
        let half_height = ((height - vertical) / 2.0).max(8.0);
        let window = |id, x, y, height| {
            PreviewWindow::new(id, CGRect::new(CGPoint::new(x, y), CGSize::new(half, height)))
        };
        illustration.set_windows(
            &[
                window(0, left, top, height),
                window(1, right, top, half_height),
                window(2, right, top + half_height + vertical, half_height),
            ],
            PreviewAnimation::default(),
        );
    });
    f.gap_preview = Some(update.clone());
    f.sync.push(Box::new(move |source| update(source)));
    preview.accessibility_label("Preview of screen edges and spacing between windows");
    preview
}

/// Screen edges, in the order spacing editors list them.
pub(super) const EDGES: [&str; 4] = ["Top", "Right", "Bottom", "Left"];
/// Directions between windows.
pub(super) const AXES: [&str; 2] = ["Horizontal", "Vertical"];

/// An outer edge (`EDGES`) or inner axis (`AXES`) of the effective spacing.
/// `None` reads the first and writes every one.
pub(super) fn spacing_gap(
    source: &ConfigSource,
    display: Option<&str>,
    outer: bool,
    axis: Option<usize>,
) -> f64 {
    let gaps = source.settings.layout.gaps.effective_for_display(display);
    let axis = axis.unwrap_or(0);
    if outer {
        [
            gaps.outer.top,
            gaps.outer.right,
            gaps.outer.bottom,
            gaps.outer.left,
        ][axis]
    } else {
        [gaps.inner.horizontal, gaps.inner.vertical][axis]
    }
}

/// Writes global spacing, or a display override seeded from the effective values.
pub(super) fn set_spacing_gap(
    source: &mut ConfigSource,
    display: Option<&str>,
    outer: bool,
    axis: Option<usize>,
    value: f64,
) {
    let gaps = &mut source.settings.layout.gaps;
    let mut effective = gaps.effective_for_display(display);
    let slots = if outer {
        let o = &mut effective.outer;
        vec![&mut o.top, &mut o.right, &mut o.bottom, &mut o.left]
    } else {
        let i = &mut effective.inner;
        vec![&mut i.horizontal, &mut i.vertical]
    };
    for (index, slot) in slots.into_iter().enumerate() {
        if axis.is_none_or(|axis| axis == index) {
            *slot = value;
        }
    }
    match (display, outer) {
        (Some(display), true) => {
            gaps.per_display.entry(display.to_owned()).or_default().outer = Some(effective.outer)
        }
        (Some(display), false) => {
            gaps.per_display.entry(display.to_owned()).or_default().inner = Some(effective.inner)
        }
        (None, true) => gaps.outer = effective.outer,
        (None, false) => gaps.inner = effective.inner,
    }
}

fn layout_spacing(ui: Ui, model: &Rc<Model>) -> Page {
    let screen = model.env.window.borrow().display_id().map(crate::sys::screen::ScreenId::new);
    let source = model.source.borrow();
    let display = model
        .env
        .displays
        .borrow()
        .iter()
        .find(|display| Some(display.id) == screen)
        .filter(|display| {
            source.settings.layout.gaps.per_display.contains_key(&display.display_uuid)
                || source.settings.layout.scrolling.per_display.contains_key(&display.display_uuid)
        })
        .map(|display| {
            (
                display.display_uuid.clone(),
                display
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("Display {}", display.id.as_u32())),
            )
        });
    let uuid: Option<String> = display.as_ref().map(|(uuid, _)| uuid.clone());
    let gaps = source.settings.layout.gaps.effective_for_display(uuid.as_deref());
    let custom = gaps.outer.top != gaps.outer.bottom
        || gaps.outer.top != gaps.outer.left
        || gaps.outer.top != gaps.outer.right
        || gaps.inner.horizontal != gaps.inner.vertical;
    drop(source);
    let mut f = FormBuilder::new(ui, model);
    let preview = gap_preview(ui, &mut f, uuid.as_deref().map(str::to_owned));
    let form = |grid: Grid| grid.column_widths(&[128.0, 120.0, 82.0]).first_baseline();
    let mut row = |name: &str, outer: bool, axis: Option<usize>| {
        let (read, write) = (uuid.clone(), uuid.clone());
        f.gap_cells(
            name,
            move |source| spacing_gap(source, read.as_deref(), outer, axis),
            move |source, value| set_spacing_gap(source, write.as_deref(), outer, axis, value),
        )
    };
    let simple = Rc::new(form(
        Grid::new(&ui).spacing(8.0, 12.0).row(row("Screen edges", true, None)).row(row(
            "Between windows",
            false,
            None,
        )),
    ));
    let mut edges = Grid::new(&ui).spacing(8.0, 12.0);
    for (axis, name) in EDGES.into_iter().enumerate() {
        edges = edges.row(row(name, true, Some(axis)));
    }
    let mut between = Grid::new(&ui).spacing(8.0, 12.0);
    for (axis, name) in AXES.into_iter().enumerate() {
        between = between.row(row(name, false, Some(axis)));
    }
    let custom_form = Rc::new(
        VStack::new(&ui)
            .spacing(10.0)
            .push(SubsectionTitle::new(&ui, "Screen edges"))
            .push(form(edges))
            .push(SubsectionTitle::new(&ui, "Between windows"))
            .push(form(between)),
    );
    simple.set_hidden(custom);
    custom_form.set_hidden(!custom);
    let (weak_simple, weak_custom) = (Rc::downgrade(&simple), Rc::downgrade(&custom_form));
    let mode = SegmentedControl::new(&ui, &["Simple", "Custom"]).on_change(move |index| {
        if let (Some(simple), Some(custom)) = (weak_simple.upgrade(), weak_custom.upgrade()) {
            simple.set_hidden(index != 0);
            custom.set_hidden(index == 0);
        }
    });
    mode.set_selected(usize::from(custom));
    let editor = VStack::new(&ui)
        .spacing(10.0)
        .push(preview)
        .push(HStack::new(&ui).push(mode).push(Spacer::new(&ui)))
        .push(simple)
        .push(custom_form);
    let mut page = SettingsPage::new(&ui, "Spacing").content_spacing(10.0);
    if let Some((_, name)) = display {
        let mut scope = HStack::new(&ui).spacing(6.0);
        if let Some(symbol) = ImageView::symbol(&ui, "display") {
            symbol.width(14.0);
            symbol.height(14.0);
            scope = scope.push(symbol);
        }
        page = page
            .section(scope.push(SecondaryLabel::new(&ui, &name)))
            .subtitle("Editing this display’s spacing override.");
    } else {
        page = page.subtitle("Applied to every layout.");
    }
    f.finish(page.section(editor))
}

/// A ratio as a percentage, without float noise such as `56.99999999999999`.
pub(super) fn percentage_text(ratio: f64) -> String {
    format!("{:.2}", ratio * 100.0)
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

fn layout_options(ui: Ui, model: &Rc<Model>, mode: LayoutMode) -> Page {
    let mut f = FormBuilder::new(ui, model);
    let subsection = |title: &str| Section::form(&ui, "").content(SubsectionTitle::new(&ui, title));
    let options = || {
        subsection(match mode {
            LayoutMode::Stack => "Arrangement",
            LayoutMode::MasterStack => "Master area",
            LayoutMode::Scrolling => "Column sizing",
            _ => "Window arrangement",
        })
    };
    macro_rules! insertion_point {
        ($layout:ident) => {
            f.inherited_popup(
                "New window position",
                insertion(),
                |s| s.settings.layout.$layout.base.window_insertion_point,
                |s| s.settings.layout.base.window_insertion_point.unwrap_or_default(),
                |s, v| s.settings.layout.$layout.base.window_insertion_point = v,
            )
        };
    }
    let mut page = VStack::new(&ui)
        .spacing(10.0)
        .push(SectionTitle::new(&ui, layout_title(mode)))
        .push(Caption::new(
            &ui,
            LayoutMode::CONFIG_CHOICES[mode.config_choice_index()].1,
        ));
    let section = match mode {
        LayoutMode::Traditional => f
            .fields(
                options(),
                &["equalize_nodes"],
                |s| &s.settings.layout.traditional,
                |s| &mut s.settings.layout.traditional,
            )
            .row(insertion_point!(traditional)),
        LayoutMode::Bsp => options()
            .row(insertion_point!(bsp))
            .row(f.text(
                "Single window aspect ratio",
                |s| {
                    s.settings
                        .layout
                        .bsp
                        .single_window_aspect_ratio
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "Automatic".into())
                },
                |s, v| {
                    s.settings.layout.bsp.single_window_aspect_ratio =
                        if v.trim().eq_ignore_ascii_case("automatic") {
                            None
                        } else {
                            optional_number(&v)?
                        };
                    Ok(())
                },
            ))
            .footer(WrappingLabel::new(
                &ui,
                "Automatic fills the available space when only one window is open. Enter a width-to-height ratio, such as 1.78 for 16:9, to keep that window at a fixed shape.",
            )),
        LayoutMode::Stack => f
            .fields(
                options(),
                &["stack_offset", "default_orientation"],
                |s| &s.settings.layout.stack,
                |s| &mut s.settings.layout.stack,
            )
            .row(insertion_point!(stack)),
        LayoutMode::MasterStack => {
            let master = options().row(
                f.percentage(
                    "Master width",
                    |_| (5.0, 95.0),
                    |s| s.settings.layout.master_stack.master_ratio,
                    |s, v| s.settings.layout.master_stack.master_ratio = v,
                )
                .suffix("%"),
            );
            page = page.push(f.fields(
                master,
                &["master_count", "master_side"],
                |s| &s.settings.layout.master_stack,
                |s| &mut s.settings.layout.master_stack,
            ));
            let natural = |s: &ConfigSource| match s.settings.layout.master_stack.master_side {
                MasterStackSide::Left | MasterStackSide::Right => Orientation::Vertical,
                MasterStackSide::Top | MasterStackSide::Bottom => Orientation::Horizontal,
            };
            f.fields(
                subsection("Arrangement"),
                &["new_window_placement"],
                |s| &s.settings.layout.master_stack,
                |s| &mut s.settings.layout.master_stack,
            )
            .row(f.inherited_popup(
                "Master arrangement",
                arrangement(),
                |s| s.settings.layout.master_stack.master_arrangement,
                natural,
                |s, v| s.settings.layout.master_stack.master_arrangement = v,
            ))
            .row(f.inherited_popup(
                "Stack arrangement",
                arrangement(),
                |s| s.settings.layout.master_stack.stack_arrangement,
                natural,
                |s, v| s.settings.layout.master_stack.stack_arrangement = v,
            ))
            .row(insertion_point!(master_stack))
        }
        LayoutMode::Scrolling => {
            let sizing = options()
                .description("Column widths are percentages of the screen width.")
                .row(
                    f.percentage(
                        "Default column width",
                        |s| {
                            let scrolling = &s.settings.layout.scrolling;
                            (
                                scrolling.min_column_width_ratio * 100.0,
                                scrolling.max_column_width_ratio * 100.0,
                            )
                        },
                        |s| s.settings.layout.scrolling.column_width_ratio,
                        |s, v| s.settings.layout.scrolling.column_width_ratio = v,
                    )
                    .suffix("%"),
                )
                .row(f.text(
                    "Width presets (%)",
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
                        let widths = v
                            .split(',')
                            .enumerate()
                            .map(|(index, value)| {
                                let value = value.trim().trim_end_matches('%').trim();
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
                ));
            page = page.push(f.fields(
                sizing,
                &[
                    "preserve_window_sizes",
                    "min_column_width_ratio",
                    "max_column_width_ratio",
                ],
                |s| &s.settings.layout.scrolling,
                |s| &mut s.settings.layout.scrolling,
            ));
            let navigation = f
                .fields(
                    subsection("Navigation"),
                    &["alignment", "focus_navigation_style"],
                    |s| &s.settings.layout.scrolling,
                    |s| &mut s.settings.layout.scrolling,
                )
                .row(f.inherited_popup(
                    "Animate navigation",
                    bool_choices(),
                    |s| s.settings.layout.scrolling.animate,
                    |_| true,
                    |s, v| s.settings.layout.scrolling.animate = v,
                ));
            page = page.push(navigation);
            subsection("Window placement").row(insertion_point!(scrolling))
        }
        LayoutMode::Floating => {
            return f.finish(SettingsPage::new(&ui, "").section(page.push(Caption::new(
                &ui,
                "Floating does not have any additional layout options.",
            ))));
        }
    };
    f.finish(SettingsPage::new(&ui, "").section(page.push(section)))
}

fn input(ui: Ui, model: &Rc<Model>) -> Page {
    let mut f = FormBuilder::new(ui, model);
    let mut page = SettingsPage::new(&ui, "");
    let section = f.schema_section("Focus", "pointer", |s| &s.settings, |s| &mut s.settings);
    page = page.section(section);
    let section = f.schema_section(
        "Workspace swipes",
        "main",
        |s| &s.settings.gestures,
        |s| &mut s.settings.gestures,
    );
    let tuning = f.schema_section(
        "",
        "advanced",
        |s| &s.settings.gestures,
        |s| &mut s.settings.gestures,
    );
    page = page.section(section.content(Disclosure::new(&ui, "Advanced swipe settings", tuning)));
    let mut section = Section::new(&ui, "Scrolling layout gestures")
        .description("These gestures navigate the Scrolling layout strip.");
    let mut tuning = Section::new(&ui, "");
    for field in ScrollingGestureSettings::fields() {
        let row = if field.key == "animate" {
            let row = f.inherited_popup(
                field.title,
                bool_choices(),
                |s| s.settings.layout.scrolling.gestures.animate,
                |s| s.settings.layout.scrolling.animate.unwrap_or(true),
                |s, v| s.settings.layout.scrolling.gestures.animate = v,
            );
            Some(f.schema_metadata(row, field, |s| &s.settings.layout.scrolling.gestures))
        } else {
            f.schema_field(
                field,
                |s| &s.settings.layout.scrolling.gestures,
                |s| &mut s.settings.layout.scrolling.gestures,
            )
        };
        if let Some(row) = row {
            if matches!(field.key, "vertical_tolerance" | "workspace_switch_threshold") {
                tuning = tuning.row(row);
            } else {
                section = section.row(row);
            }
        }
    }
    page =
        page.section(section.content(Disclosure::new(&ui, "Advanced scrolling settings", tuning)));
    let section = f.schema_section(
        "Drag & Drop",
        "",
        |s| &s.settings.drag_drop,
        |s| &mut s.settings.drag_drop,
    );
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
    page = page.section(Disclosure::new(
        &ui,
        "Advanced focus settings",
        focus_suspend(&mut f),
    ));
    f.finish(page)
}

fn interface(ui: Ui, model: &Rc<Model>) -> Page {
    type ColorField = (
        &'static str,
        fn(&StackLineSettings) -> Color,
        fn(&mut StackLineSettings) -> &mut Color,
    );
    const COLORS: [ColorField; 3] = [
        ("Selected color", |s| s.selected_color, |s| &mut s.selected_color),
        (
            "Unselected color",
            |s| s.unselected_color,
            |s| &mut s.unselected_color,
        ),
        ("Border color", |s| s.border_color, |s| &mut s.border_color),
    ];
    let mut f = FormBuilder::new(ui, model);
    let mut page = SettingsPage::new(&ui, "");
    let section = f.schema_section(
        "Menu Bar",
        "",
        |s| &s.settings.ui.menu_bar,
        |s| &mut s.settings.ui.menu_bar,
    );
    let message = Rc::new(ValidationMessage::new(&ui));
    let error = Rc::downgrade(&message);
    let weak_model = f.model.clone();
    let path = Rc::new(PathField::new(&ui).directories().on_change(move |path| {
        Model::submit(
            &weak_model,
            Box::new(move |s| {
                s.settings.ui.menu_bar.layout_folder = path;
                Ok(())
            }),
            error.clone(),
        )
    }));
    f.sync(&path, |path, s| {
        path.set_value(&s.settings.ui.menu_bar.layout_folder)
    });
    let field = MenuBarSettings::field("layout_folder").unwrap();
    let row = f.row(field.title, path, message);
    let row = f.schema_metadata(row, field, |s| &s.settings.ui.menu_bar);
    page = page.section(section.row(row));
    let section = f.schema_rows(
        Section::new(&ui, "Overview").description("Requires restarting Rift."),
        "",
        |s| &s.settings.ui.mission_control,
        |s| &mut s.settings.ui.mission_control,
    );
    page = page.section(section);
    let mut section = f.schema_rows(
        Section::new(&ui, "Stack Line").description("Experimental window indicators."),
        "",
        |s| &s.settings.ui.stack_line,
        |s| &mut s.settings.ui.stack_line,
    );
    for (title, get, set) in COLORS {
        let row = f.color(
            title,
            move |s| get(&s.settings.ui.stack_line),
            move |s, v| *set(&mut s.settings.ui.stack_line) = v,
        );
        f.enabled(&row, |s| s.settings.ui.stack_line.enabled);
        section = section.row(row);
    }
    page = page.section(section);
    f.finish(page)
}

fn advanced(ui: Ui, model: &Rc<Model>) -> Page {
    let mut f = FormBuilder::new(ui, model);
    let path = model.env.config_path.to_string_lossy().into_owned();
    let (open, reveal) = (path.clone(), path.clone());
    let message = Rc::new(ValidationMessage::new(&ui));
    let error = Rc::downgrade(&message);
    let env = Rc::downgrade(&model.env);
    let actions = HStack::new(&ui)
        .push(Button::new(&ui, "Open…").on_click(move || {
            Application::open(&open);
        }))
        .push(Button::new(&ui, "Reveal in Finder").on_click(move || Application::reveal(&reveal)))
        .push(Button::new(&ui, "Reload").on_click(move || {
            if let Some(env) = env.upgrade() {
                let error = error.clone();
                env.send(Action::Reload, Box::new(move |result| report(&error, &result)));
            }
        }));
    let location = Label::new(&ui, &path).wrapping().max_lines(1).selectable();
    location.tooltip(&path);
    let file = f
        .fields(
            Section::new(&ui, "Configuration file").content(location),
            &["hot_reload"],
            |s| &s.settings,
            |s| &mut s.settings,
        )
        .content(actions)
        .footer(message);
    let startup = super::editors::strings(
        &mut f,
        "Startup commands",
        |s| s.settings.run_on_start.clone(),
        |s, values| s.settings.run_on_start = values,
    );
    let blacklist = super::editors::strings(
        &mut f,
        "Autofocus blacklist",
        |s| s.settings.auto_focus_blacklist.clone(),
        |s, values| s.settings.auto_focus_blacklist = values,
    );
    f.finish(SettingsPage::new(&ui, "").section(file).section(startup).section(blacklist))
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
        let message = Rc::new(ValidationMessage::new(&self.ui));
        let error = Rc::downgrade(&message);
        let model = self.model.clone();
        let input = Rc::new(ColorWell::new(&self.ui).on_change(move |[r, g, b, a]| {
            let color = Color::new(r, g, b, a);
            Model::submit(
                &model,
                Box::new(move |s| {
                    set(s, color);
                    Ok(())
                }),
                error.clone(),
            );
        }));
        self.sync(&input, move |input, s| {
            let c = get(s);
            input.set_value([c.r, c.g, c.b, c.a]);
        });
        self.row(title, input, message)
    }
}

fn focus_suspend(f: &mut FormBuilder) -> Section {
    use super::commands::{from_rift_modifiers, recorded_key, to_rift_modifiers};
    use crate::sys::hotkey::{Hotkey, HotkeySpec};
    let ui = f.ui;
    let message = Rc::new(ValidationMessage::new(&ui));
    let submit = {
        let model = f.model.clone();
        let error = Rc::downgrade(&message);
        move |value: Option<HotkeySpec>| {
            Model::submit(
                &model,
                Box::new(move |s| {
                    s.settings.focus_follows_mouse_disable_hotkey = value;
                    Ok(())
                }),
                error.clone(),
            )
        }
    };
    let record = submit.clone();
    let recorder = Rc::new(KeyRecorder::new(&ui).on_change(move |value| {
        record(
            value
                .as_ref()
                .and_then(recorded_key)
                .and_then(|v| v.parse::<Hotkey>().ok())
                .map(HotkeySpec::Hotkey),
        );
    }));
    let record = submit.clone();
    let modifier_recorder = Rc::new(ModifierRecorder::new(&ui).on_change(move |flags| {
        record((!flags.is_empty()).then(|| HotkeySpec::ModifiersOnly {
            modifiers: to_rift_modifiers(flags),
        }));
    }));
    f.sync(&recorder, |recorder, s| {
        recorder.set_value(None);
        if let Some(HotkeySpec::Hotkey(key)) = &s.settings.focus_follows_mouse_disable_hotkey {
            recorder.set_title(&key.to_string());
        }
        recorder.set_enabled(s.settings.focus_follows_mouse);
    });
    f.sync(&modifier_recorder, |recorder, s| {
        recorder.set_value(match &s.settings.focus_follows_mouse_disable_hotkey {
            Some(HotkeySpec::ModifiersOnly { modifiers }) => from_rift_modifiers(*modifiers),
            _ => Modifiers::empty(),
        });
        recorder.set_enabled(s.settings.focus_follows_mouse);
    });
    let clear = Button::new(&ui, "Clear").on_click(move || submit(None));
    Section::new(&ui, "Suspend pointer focus while held")
        .description("Record either a shortcut or modifiers. Recording one replaces the other.")
        .row(SettingsRow::new(&ui, "Shortcut", recorder))
        .row(SettingsRow::new(&ui, "Modifiers only", modifier_recorder))
        .content(clear)
        .footer(message)
}
