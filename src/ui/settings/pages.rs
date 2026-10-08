use super::*;
use crate::common::config::{Color, *};
use crate::layout_engine::Orientation;

pub(super) fn layouts() -> Vec<(&'static str, LayoutMode)> {
    LayoutMode::CONFIG_CHOICES
        .iter()
        .enumerate()
        .map(|(index, choice)| (choice.0, LayoutMode::from_config_choice(index).unwrap()))
        .collect()
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
        4 => super::editors::keyboard(ui, model),
        5 => input(ui, model),
        6 => interface(ui, model),
        8 => FormBuilder::new(ui, model).finish(SettingsPage::new(&ui, "").section(about(ui, model))),
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
fn about(ui: Ui, model: &Rc<Model>) -> VStack {
    let status = Rc::new(Label::new(&ui, "").color(&cgs::Color::secondary_label()).wrapping());
    status.set_hidden(true);
    let weak_status = Rc::downgrade(&status);
    let weak_model = Rc::downgrade(model);
    let check = Button::new(&ui, "Check for Updates…");
    let weak_button = objc2::rc::Weak::new(check.ns_button());
    // Keep only weak view references in the pending request so closing Settings
    // releases the page even while the bounded network request is finishing.
    let check = check.on_click(move || {
        if let Some(status) = weak_status.upgrade() {
            status.set_hidden(false);
            status.set_text("Checking for updates…");
        }
        if let Some(button) = weak_button.load() {
            button.setEnabled(false);
        }
        let Some(model) = weak_model.upgrade() else {
            return;
        };
        let status = weak_status.clone();
        let button = weak_button.clone();
        let _ = model.requests.send(Request {
            action: Action::CheckUpdates(Box::new(move |result| {
                if let Some(status) = status.upgrade() {
                    status.set_text(&result.unwrap_or_else(|e| e));
                }
                if let Some(button) = button.load() {
                    button.setEnabled(true);
                }
            })),
            finish: Box::new(|_| {}),
        });
    });
    let identity = HStack::new(&ui).spacing(16.0);
    if let Some(icon) = NSApplication::sharedApplication(ui.mtm()).applicationIconImage() {
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
    let destinations = vec![
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
    let links = destinations.clone();
    let resources = SettingsList::new(
        &ui,
        |item: &(&str, &str, &str)| item.0.to_string(),
        |_| String::new(),
    )
    .full_length()
    .navigation()
    .trailing_summary()
    .symbols(|item| item.1.to_string())
    .on_open(move |index| {
        if let Some(item) = links.get(index) {
            let _ = std::process::Command::new("open").arg(item.2).spawn();
        }
    });
    resources.set_rows(destinations);
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

fn layout(ui: Ui, model: &Rc<Model>) -> Page {
    let f = FormBuilder::new(ui, model);
    let mut page = SettingsPage::new(&ui, "Layouts");
    for (title, range) in [("Default", 0..1), ("Layouts", 1..7), ("Global", 7..9)] {
        let weak = Rc::downgrade(model);
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
            if let Some(model) = weak.upgrade() {
                if let Some(navigate) = model.navigate.borrow().as_ref() {
                    navigate(9 + start + index);
                }
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
        1..=6 => layout_options(ui, model, layouts()[index - 1].1),
        7 => layout_spacing(ui, model),
        _ => {
            let mut f = FormBuilder::new(ui, model);
            let page = SettingsPage::new(&ui, "");
            let page = super::editors::display_overrides(&mut f, page, model);
            f.finish(page)
        }
    }
}

fn layout_defaults(ui: Ui, model: &Rc<Model>) -> Page {
    let mut f = FormBuilder::new(ui, model);
    let mut page = SettingsPage::new(&ui, "");
    let section = Section::form(&ui, "Default behavior")
        .description("Used by workspaces that don’t have their own layout.")
        .row(
            f.schema_field(
                LayoutSettings::field("mode").unwrap(),
                |s| &s.settings.layout,
                |s| &mut s.settings.layout,
            )
            .unwrap(),
        )
        .row(f.inherited_popup(
            "New window position",
            insertion(),
            |s| s.settings.layout.base.window_insertion_point,
            |_| WindowInsertionPoint::default(),
            |s, v| s.settings.layout.base.window_insertion_point = v,
        ));
    page = page.section(section);
    f.finish(page)
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
        let edge = |v: f64| v.clamp(0.0, 200.0) * 0.25;
        let left = edge(gaps.outer.left);
        let top = edge(gaps.outer.top);
        let width = (size.width - left - edge(gaps.outer.right)).max(24.0);
        let height = (size.height - top - edge(gaps.outer.bottom)).max(24.0);
        let horizontal = gaps.inner.horizontal.clamp(0.0, 200.0) * 0.25;
        let vertical = gaps.inner.vertical.clamp(0.0, 200.0) * 0.25;
        let half = ((width - horizontal) / 2.0).max(8.0);
        let right = left + half + horizontal;
        let half_height = ((height - vertical) / 2.0).max(8.0);
        illustration.set_windows(
            &[
                PreviewWindow::new(
                    0,
                    CGRect::new(CGPoint::new(left, top), CGSize::new(half, height)),
                ),
                PreviewWindow::new(
                    1,
                    CGRect::new(CGPoint::new(right, top), CGSize::new(half, half_height)),
                ),
                PreviewWindow::new(
                    2,
                    CGRect::new(
                        CGPoint::new(right, top + half_height + vertical),
                        CGSize::new(half, half_height),
                    ),
                ),
            ],
            PreviewAnimation::default(),
        );
    });
    f.gap_preview = Some(update.clone());
    f.sync.push(Box::new(move |source| update(source)));
    preview.accessibility_label("Preview of screen edges and spacing between windows");
    preview
}

fn set_spacing_gap(
    source: &mut ConfigSource,
    display: Option<&str>,
    outer: bool,
    axis: Option<usize>,
    value: f64,
) {
    let mut gaps = source.settings.layout.gaps.effective_for_display(display);
    if outer {
        if let Some(axis) = axis {
            match axis {
                0 => gaps.outer.top = value,
                1 => gaps.outer.right = value,
                2 => gaps.outer.bottom = value,
                _ => gaps.outer.left = value,
            }
        } else {
            gaps.outer = OuterGaps {
                top: value,
                right: value,
                bottom: value,
                left: value,
            };
        }
    } else if let Some(axis) = axis {
        if axis == 0 {
            gaps.inner.horizontal = value;
        } else {
            gaps.inner.vertical = value;
        }
    } else {
        gaps.inner.horizontal = value;
        gaps.inner.vertical = value;
    }
    if let Some(display) = display {
        let entry = source.settings.layout.gaps.per_display.entry(display.to_owned()).or_default();
        if outer {
            entry.outer = Some(gaps.outer);
        } else {
            entry.inner = Some(gaps.inner);
        }
    } else if outer {
        source.settings.layout.gaps.outer = gaps.outer;
    } else {
        source.settings.layout.gaps.inner = gaps.inner;
    }
}

fn layout_spacing(ui: Ui, model: &Rc<Model>) -> Page {
    use crate::sys::screen::NSScreenExt;
    let screen = model
        .window
        .borrow()
        .load()
        .and_then(|window| window.screen())
        .and_then(|screen| screen.get_number().ok());
    let source = model.source.borrow();
    let display = model
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
    let uuid = display.as_ref().map(|(uuid, _)| uuid.clone());
    let gaps = source.settings.layout.gaps.effective_for_display(uuid.as_deref());
    let custom = gaps.outer.top != gaps.outer.bottom
        || gaps.outer.top != gaps.outer.left
        || gaps.outer.top != gaps.outer.right
        || gaps.inner.horizontal != gaps.inner.vertical;
    drop(source);
    let mut f = FormBuilder::new(ui, model);
    let preview = gap_preview(ui, &mut f, uuid.clone());
    let form = |grid: Grid| {
        grid.ns_grid_view().columnAtIndex(0).setWidth(128.0);
        grid.ns_grid_view().columnAtIndex(1).setWidth(120.0);
        grid.ns_grid_view().columnAtIndex(2).setWidth(82.0);
        grid.ns_grid_view()
            .setRowAlignment(objc2_app_kit::NSGridRowAlignment::FirstBaseline);
        grid
    };
    let mut row = |name: &str, outer: bool, axis: Option<usize>| {
        let read = uuid.clone();
        let write = uuid.clone();
        f.gap_cells(
            name,
            move |source| {
                let gaps = source.settings.layout.gaps.effective_for_display(read.as_deref());
                if outer {
                    [
                        gaps.outer.top,
                        gaps.outer.right,
                        gaps.outer.bottom,
                        gaps.outer.left,
                    ][axis.unwrap_or(0)]
                } else {
                    [gaps.inner.horizontal, gaps.inner.vertical][axis.unwrap_or(0)]
                }
            },
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
    for (axis, name) in ["Top", "Right", "Bottom", "Left"].into_iter().enumerate() {
        edges = edges.row(row(name, true, Some(axis)));
    }
    let between = form(
        Grid::new(&ui)
            .spacing(8.0, 12.0)
            .row(row("Horizontal", false, Some(0)))
            .row(row("Vertical", false, Some(1))),
    );
    let custom_form = Rc::new(
        VStack::new(&ui)
            .spacing(10.0)
            .push(SubsectionTitle::new(&ui, "Screen edges"))
            .push(form(edges))
            .push(SubsectionTitle::new(&ui, "Between windows"))
            .push(between),
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

fn percentage_text(ratio: f64) -> String {
    format!("{:.2}", ratio * 100.0)
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

fn layout_description(mode: LayoutMode) -> &'static str {
    LayoutMode::CONFIG_CHOICES[mode.config_choice_index()].1
}

fn layout_options(ui: Ui, model: &Rc<Model>, mode: LayoutMode) -> Page {
    let mut f = FormBuilder::new(ui, model);
    let options = || {
        Section::form(&ui, "").content(SubsectionTitle::new(&ui, match mode {
            LayoutMode::Stack => "Arrangement",
            LayoutMode::MasterStack => "Master area",
            LayoutMode::Scrolling => "Column sizing",
            _ => "Window arrangement",
        }))
    };
    let mut page = VStack::new(&ui)
        .spacing(10.0)
        .push(SectionTitle::new(
            &ui,
            layouts().into_iter().find(|(_, value)| *value == mode).unwrap().0,
        ))
        .push(Caption::new(&ui, layout_description(mode)));
    let section = match mode {
        LayoutMode::Traditional => options()
            .row(
                f.schema_field(
                    TraditionalLayoutSettings::field("equalize_nodes").unwrap(),
                    |s| &s.settings.layout.traditional,
                    |s| &mut s.settings.layout.traditional,
                )
                .unwrap(),
            )
            .row(f.inherited_popup(
                "New window position",
                insertion(),
                |s| s.settings.layout.traditional.base.window_insertion_point,
                |s| s.settings.layout.base.window_insertion_point.unwrap_or_default(),
                |s, v| s.settings.layout.traditional.base.window_insertion_point = v,
            )),
        LayoutMode::Bsp => options()
            .row(f.inherited_popup(
                "New window position",
                insertion(),
                |s| s.settings.layout.bsp.base.window_insertion_point,
                |s| s.settings.layout.base.window_insertion_point.unwrap_or_default(),
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
            )),
        LayoutMode::Stack => options()
            .row(
                f.schema_field(
                    StackSettings::field("stack_offset").unwrap(),
                    |s| &s.settings.layout.stack,
                    |s| &mut s.settings.layout.stack,
                )
                .unwrap(),
            )
            .row(
                f.schema_field(
                    StackSettings::field("default_orientation").unwrap(),
                    |s| &s.settings.layout.stack,
                    |s| &mut s.settings.layout.stack,
                )
                .unwrap(),
            )
            .row(f.inherited_popup(
                "New window position",
                insertion(),
                |s| s.settings.layout.stack.base.window_insertion_point,
                |s| s.settings.layout.base.window_insertion_point.unwrap_or_default(),
                |s, v| s.settings.layout.stack.base.window_insertion_point = v,
            )),
        LayoutMode::MasterStack => {
            let section = options()
                .row(
                    f.percentage(
                        "Master width",
                        |_| (5.0, 95.0),
                        |s| s.settings.layout.master_stack.master_ratio,
                        |s, v| s.settings.layout.master_stack.master_ratio = v,
                    )
                    .suffix("%"),
                )
                .row(
                    f.schema_field(
                        MasterStackSettings::field("master_count").unwrap(),
                        |s| &s.settings.layout.master_stack,
                        |s| &mut s.settings.layout.master_stack,
                    )
                    .unwrap(),
                )
                .row(
                    f.schema_field(
                        MasterStackSettings::field("master_side").unwrap(),
                        |s| &s.settings.layout.master_stack,
                        |s| &mut s.settings.layout.master_stack,
                    )
                    .unwrap(),
                );
            page = page.push(section);
            Section::form(&ui, "")
                .content(SubsectionTitle::new(&ui, "Arrangement"))
                .row(
                    f.schema_field(
                        MasterStackSettings::field("new_window_placement").unwrap(),
                        |s| &s.settings.layout.master_stack,
                        |s| &mut s.settings.layout.master_stack,
                    )
                    .unwrap(),
                )
                .row(f.inherited_popup(
                    "Master arrangement",
                    arrangement(),
                    |s| s.settings.layout.master_stack.master_arrangement,
                    |s| match s.settings.layout.master_stack.master_side {
                        MasterStackSide::Left | MasterStackSide::Right => Orientation::Vertical,
                        MasterStackSide::Top | MasterStackSide::Bottom => Orientation::Horizontal,
                    },
                    |s, v| s.settings.layout.master_stack.master_arrangement = v,
                ))
                .row(f.inherited_popup(
                    "Stack arrangement",
                    arrangement(),
                    |s| s.settings.layout.master_stack.stack_arrangement,
                    |s| match s.settings.layout.master_stack.master_side {
                        MasterStackSide::Left | MasterStackSide::Right => Orientation::Vertical,
                        MasterStackSide::Top | MasterStackSide::Bottom => Orientation::Horizontal,
                    },
                    |s, v| s.settings.layout.master_stack.stack_arrangement = v,
                ))
                .row(f.inherited_popup(
                    "New window position",
                    insertion(),
                    |s| s.settings.layout.master_stack.base.window_insertion_point,
                    |s| s.settings.layout.base.window_insertion_point.unwrap_or_default(),
                    |s, v| s.settings.layout.master_stack.base.window_insertion_point = v,
                ))
        }
        LayoutMode::Scrolling => {
            let section = options()
                .description("Column widths are percentages of the screen width.")
                .row(
                    f.percentage(
                        "Default column width",
                        |s| {
                            (
                                s.settings.layout.scrolling.min_column_width_ratio * 100.0,
                                s.settings.layout.scrolling.max_column_width_ratio * 100.0,
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
                                value
                                    .parse::<f64>()
                                    .map(|v| v / 100.0)
                                    .map_err(|_| "Enter percentages separated by commas".to_string())
                            })
                            .collect::<Result<_, _>>()?;
                        s.settings.layout.scrolling.preset_column_widths = widths;
                        Ok(())
                    },
                ))
                .row(
                    f.schema_field(
                        ScrollingLayoutSettings::field("preserve_window_sizes").unwrap(),
                        |s| &s.settings.layout.scrolling,
                        |s| &mut s.settings.layout.scrolling,
                    )
                    .unwrap(),
                )
                .row(
                    f.schema_field(
                        ScrollingLayoutSettings::field("min_column_width_ratio").unwrap(),
                        |s| &s.settings.layout.scrolling,
                        |s| &mut s.settings.layout.scrolling,
                    )
                    .unwrap()
                    .suffix("%"),
                )
                .row(
                    f.schema_field(
                        ScrollingLayoutSettings::field("max_column_width_ratio").unwrap(),
                        |s| &s.settings.layout.scrolling,
                        |s| &mut s.settings.layout.scrolling,
                    )
                    .unwrap()
                    .suffix("%"),
                );
            page = page.push(section);
            let section = Section::form(&ui, "")
                .content(SubsectionTitle::new(&ui, "Navigation"))
                .row(
                    f.schema_field(
                        ScrollingLayoutSettings::field("alignment").unwrap(),
                        |s| &s.settings.layout.scrolling,
                        |s| &mut s.settings.layout.scrolling,
                    )
                    .unwrap(),
                )
                .row(
                    f.schema_field(
                        ScrollingLayoutSettings::field("focus_navigation_style").unwrap(),
                        |s| &s.settings.layout.scrolling,
                        |s| &mut s.settings.layout.scrolling,
                    )
                    .unwrap(),
                )
                .row(f.inherited_popup(
                    "Animate navigation",
                    bool_choices(),
                    |s| s.settings.layout.scrolling.animate,
                    |_| true,
                    |s, v| s.settings.layout.scrolling.animate = v,
                ));
            page = page.push(section);
            Section::form(&ui, "")
                .content(SubsectionTitle::new(&ui, "Window placement"))
                .row(f.inherited_popup(
                    "New window position",
                    insertion(),
                    |s| s.settings.layout.scrolling.base.window_insertion_point,
                    |s| s.settings.layout.base.window_insertion_point.unwrap_or_default(),
                    |s, v| s.settings.layout.scrolling.base.window_insertion_point = v,
                ))
        }
        LayoutMode::Floating => {
            return f.finish(SettingsPage::new(&ui, "").section(page.push(Caption::new(
                &ui,
                "Floating does not have any additional layout options.",
            ))));
        }
    };
    let section = if mode == LayoutMode::Bsp {
        section.footer(WrappingLabel::new(&ui,
            "Automatic fills the available space when only one window is open. Enter a width-to-height ratio, such as 1.78 for 16:9, to keep that window at a fixed shape."))
    } else {
        section
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
    page = page.section(section.content(Disclosure::new(&ui, "Advanced scrolling settings", tuning)));
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
    let weak_path = Rc::downgrade(&path);
    f.sync.push(Box::new(move |s| {
        if let Some(path) = weak_path.upgrade() {
            path.set_value(&s.settings.ui.menu_bar.layout_folder);
        }
    }));
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
    let section = f.schema_rows(
        Section::new(&ui, "Stack Line").description("Experimental window indicators."),
        "",
        |s| &s.settings.ui.stack_line,
        |s| &mut s.settings.ui.stack_line,
    );
    let section = section.row({
        let row = f.color(
            "Selected color",
            |s| s.settings.ui.stack_line.selected_color,
            |s, v| s.settings.ui.stack_line.selected_color = v,
        );
        f.enabled(&row, |s| s.settings.ui.stack_line.enabled);
        row
    });
    let section = section.row({
        let row = f.color(
            "Unselected color",
            |s| s.settings.ui.stack_line.unselected_color,
            |s, v| s.settings.ui.stack_line.unselected_color = v,
        );
        f.enabled(&row, |s| s.settings.ui.stack_line.enabled);
        row
    });
    let section = section.row({
        let row = f.color(
            "Border color",
            |s| s.settings.ui.stack_line.border_color,
            |s, v| s.settings.ui.stack_line.border_color = v,
        );
        f.enabled(&row, |s| s.settings.ui.stack_line.enabled);
        row
    });
    page = page.section(section);
    f.finish(page)
}
fn advanced(ui: Ui, model: &Rc<Model>) -> Page {
    let mut f = FormBuilder::new(ui, model);
    let mut page = SettingsPage::new(&ui, "");
    let path = model.config_path.clone();
    let open = path.clone();
    let reveal = path.clone();
    let actions = HStack::new(&ui)
        .push(Button::new(&ui, "Open…").on_click(move || {
            let _ = std::process::Command::new("open").arg(&open).spawn();
        }))
        .push(Button::new(&ui, "Reveal in Finder").on_click(move || {
            let _ = std::process::Command::new("open").arg("-R").arg(&reveal).spawn();
        }));
    let message = Rc::new(ValidationMessage::new(&ui));
    let error = Rc::downgrade(&message);
    let weak_model = Rc::downgrade(model);
    let reload = Button::new(&ui, "Reload").on_click(move || {
        if let Some(model) = weak_model.upgrade() {
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
                }),
            });
        }
    });
    let location = Label::new(&ui, &path.to_string_lossy()).wrapping();
    location.ns_text_field().setSelectable(true);
    location.ns_text_field().setMaximumNumberOfLines(1);
    location.ns_view().setToolTip(Some(&objc2_foundation::NSString::from_str(
        &path.to_string_lossy(),
    )));
    page = page.section(
        Section::new(&ui, "Configuration file")
            .content(location)
            .row(
                f.schema_field(
                    crate::common::config::Settings::field("hot_reload").unwrap(),
                    |s| &s.settings,
                    |s| &mut s.settings,
                )
                .unwrap(),
            )
            .content(actions.push(reload))
            .footer(message),
    );
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
                Model::submit(
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
        Model::submit(
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
        let value = (!flags.is_empty()).then(|| HotkeySpec::ModifiersOnly { modifiers: modifiers(flags) });
        Model::submit(
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
        Model::submit(
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
