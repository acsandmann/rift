use super::*;
use crate::common::config::*;
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

pub(super) fn build(ui: Ui, model: &Rc<Model>, id: Category) -> Page {
    match id {
        Category::General => general(ui, model),
        Category::Workspaces => super::editors::workspaces(ui, model),
        Category::Rules => super::editors::rules(ui, model),
        Category::Keyboard => super::commands::keyboard(ui, model),
        Category::Mouse => input(ui, model),
        Category::Interface => interface(ui, model),
        Category::About => FormBuilder::new(ui, model)
            .finish(SettingsPage::new(&ui, "").section(about(ui, &model.env))),
        Category::Advanced => advanced(ui, model),
        Category::Layouts => unreachable!("Layouts owns its inline inspector"),
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

/// The shared new-window position each layout follows unless it overrides it.
pub(super) fn window_position(f: &mut FormBuilder) -> SettingsRow {
    f.inherited_popup(
        "New window position",
        insertion(),
        |s| s.settings.layout.base.window_insertion_point,
        |_| WindowInsertionPoint::default(),
        |s, v| s.settings.layout.base.window_insertion_point = v,
    )
    .tooltip("Where layouts insert new windows, unless a layout chooses its own.")
}

pub(super) fn layout_displays(ui: Ui, model: &Rc<Model>) -> Page {
    let mut f = FormBuilder::new(ui, model);
    let page = super::editors::display_overrides(&mut f, model);
    f.finish(page)
}

pub(super) fn gap_preview(ui: Ui, f: &mut FormBuilder, display: Option<String>) -> impl NativeView {
    gap_preview_sized(ui, f, display, CGSize::new(354.0, 160.0))
}

fn gap_preview_sized(
    ui: Ui,
    f: &mut FormBuilder,
    display: Option<String>,
    size: CGSize,
) -> impl NativeView {
    let preview = Rc::new(LayoutPreview::new(&ui, size));
    let illustration = preview.clone();
    let update: Rc<dyn Fn(&ConfigSource)> = Rc::new(move |s| {
        let size = illustration.canvas_size();
        let gaps = s.settings.layout.gaps.effective_for_display(display.as_deref());
        // A representative desktop. Compress gaps so small ones stay visible
        // and large ones still leave room for windows.
        let scaled = |v: f64| v.clamp(0.0, 200.0).sqrt() * size.width / 140.0;
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

pub(super) fn layout_spacing(ui: Ui, model: &Rc<Model>) -> Page {
    let source = model.source.borrow();
    let gaps = source.settings.layout.gaps.effective_for_display(None);
    let custom = gaps.outer.top != gaps.outer.bottom
        || gaps.outer.top != gaps.outer.left
        || gaps.outer.top != gaps.outer.right
        || gaps.inner.horizontal != gaps.inner.vertical;
    drop(source);
    let mut f = FormBuilder::new(ui, model);
    let preview = gap_preview_sized(ui, &mut f, None, CGSize::new(160.0, 100.0));
    let mut row = |name: &str, outer: bool, axis: Option<usize>| {
        f.gap_cells(
            name,
            true,
            move |source| spacing_gap(source, None, outer, axis),
            move |source, value| set_spacing_gap(source, None, outer, axis, value),
        )
    };
    let simple = Rc::new(
        HStack::new(&ui)
            .push(
                Grid::new(&ui)
                    .spacing(8.0, 8.0)
                    .row(row("Screen edges", true, None))
                    .row(row("Between windows", false, None))
                    .first_baseline()
                    .column_widths(&[LABEL_COLUMN]),
            )
            .push(Spacer::new(&ui)),
    );
    let mut numeric_row = |name: &str, outer: bool, axis: usize| {
        f.gap_cells(
            name,
            false,
            move |source| spacing_gap(source, None, outer, Some(axis)),
            move |source, value| set_spacing_gap(source, None, outer, Some(axis), value),
        )
    };
    let empty = || Box::new(View::new(&ui)) as Box<dyn NativeView>;
    let mut custom_grid = Grid::new(&ui).spacing(8.0, 16.0).row(vec![
        Box::new(SubsectionTitle::new(&ui, "Screen edges")),
        empty(),
        Box::new(SubsectionTitle::new(&ui, "Between windows")),
        empty(),
    ]);
    for (index, name) in EDGES.into_iter().enumerate() {
        let mut cells = numeric_row(name, true, index);
        if let Some(axis) = AXES.get(index) {
            cells.extend(numeric_row(axis, false, index));
        } else {
            cells.extend([empty(), empty()]);
        }
        custom_grid = custom_grid.row(cells);
    }
    let header = custom_grid.ns_grid_view().rowAtIndex(0);
    header.mergeCellsInRange(objc2_foundation::NSRange::new(0, 2));
    header.mergeCellsInRange(objc2_foundation::NSRange::new(2, 2));
    let custom_form = Rc::new(custom_grid.first_baseline());
    simple.set_hidden(custom);
    custom_form.set_hidden(!custom);
    let (weak_simple, weak_custom) = (Rc::downgrade(&simple), Rc::downgrade(&custom_form));
    let mode = SegmentedControl::new(&ui, &["Simple", "Custom"]).on_change(move |index| {
        if let (Some(simple), Some(custom)) = (weak_simple.upgrade(), weak_custom.upgrade()) {
            simple.set_hidden(index != 0);
            custom.set_hidden(index == 0);
        }
    });
    mode.accessibility_label("Spacing controls");
    mode.set_selected(usize::from(custom));
    let editor = ResponsiveStack::new(&ui, 560.0)
        .push(
            VStack::new(&ui)
                .spacing(12.0)
                .push(HStack::new(&ui).push(mode).push(Spacer::new(&ui)))
                .push(simple)
                .push(custom_form),
        )
        .push(Spacer::new(&ui))
        .push(preview);
    lengthen_sliders(editor.ns_view());
    f.finish(editor)
}

/// A ratio as a percentage, without float noise such as `56.99999999999999`.
pub(super) fn percentage_text(ratio: f64) -> String {
    format!("{:.2}", ratio * 100.0)
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

pub(super) fn layout_options(ui: Ui, model: &Rc<Model>, mode: LayoutMode) -> Page {
    let mut f = FormBuilder::new(ui, model);
    f.tooltips = true;
    let subsection = |title: &str| {
        let title = SubsectionTitle::new(&ui, title);
        title
            .ns_text_field()
            .setFont(Some(&objc2_app_kit::NSFont::systemFontOfSize_weight(
                objc2_app_kit::NSFont::smallSystemFontSize(),
                unsafe { objc2_app_kit::NSFontWeightMedium },
            )));
        Section::form(&ui, "").content(title)
    };
    let options = || match mode {
        LayoutMode::MasterStack => subsection("Master area"),
        LayoutMode::Scrolling => subsection("Column sizing"),
        _ => Section::form(&ui, ""),
    };
    // A layout either follows the shared position or overrides it.
    macro_rules! insertion_point {
        ($layout:ident) => {
            f.following_popup(
                "New window position",
                "Shared",
                insertion(),
                |s| s.settings.layout.$layout.base.window_insertion_point,
                |s| s.settings.layout.base.window_insertion_point.unwrap_or_default(),
                |s, v| s.settings.layout.$layout.base.window_insertion_point = v,
            )
            .tooltip("Where this layout inserts new windows. Shared follows New window position in Shared settings.")
        };
    }
    let mut page = VStack::new(&ui).spacing(12.0);
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
            ).tooltip("Automatic fills the available space for a single window. Enter a width-to-height ratio, such as 1.78 for 16:9, to keep it at a fixed shape.")),
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
            .row(f.following_popup(
                "Master arrangement",
                "Automatic",
                arrangement(),
                |s| s.settings.layout.master_stack.master_arrangement,
                natural,
                |s, v| s.settings.layout.master_stack.master_arrangement = v,
            ))
            .row(f.following_popup(
                "Stack arrangement",
                "Automatic",
                arrangement(),
                |s| s.settings.layout.master_stack.stack_arrangement,
                natural,
                |s, v| s.settings.layout.master_stack.stack_arrangement = v,
            ))
            .row(insertion_point!(master_stack))
        }
        LayoutMode::Scrolling => {
            let sizing = options()
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
                .row(width_presets(&mut f));
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
                // Unset means animated, so only turning it off is written.
                .row(f.switch(
                    "Animate navigation",
                    |s| s.settings.layout.scrolling.animate.unwrap_or(true),
                    |s, v| s.settings.layout.scrolling.animate = (!v).then_some(false),
                ));
            page = page.push(navigation);
            subsection("Window placement").row(insertion_point!(scrolling))
        }
        LayoutMode::Floating => return f.finish(page),
    };
    let page = page.push(section);
    align_form(page.ns_view());
    f.finish(page)
}

/// Scrolling’s width presets as chips, in percent of the screen width.
fn width_presets(f: &mut FormBuilder) -> SettingsRow {
    let message = Rc::new(ValidationMessage::new(&f.ui));
    // A chip collection contains distinct controls and structural operations;
    // keep each submission as an ordered transaction rather than coalescing it.
    let write_model = f.model.clone();
    let error = Rc::downgrade(&message);
    let write = move |widths| {
        Model::submit(
            &write_model,
            Box::new(move |source| {
                source.settings.layout.scrolling.preset_column_widths = widths;
                Ok(())
            }),
            error.clone(),
        );
    };
    let model = f.model.clone();
    let chips = Rc::new(
        ValueChips::new(&f.ui, "Width preset", "%", 5.0, 100.0)
            .format(|percent| percentage_text(percent / 100.0))
            .on_edit(move |edit| {
                let Some(model) = model.upgrade() else { return };
                // Untouched presets keep their full precision.
                let mut widths =
                    model.source.borrow().settings.layout.scrolling.preset_column_widths.clone();
                match edit {
                    ChipEdit::Add(percent) => widths.push(percent / 100.0),
                    ChipEdit::Change(index, percent) if index < widths.len() => {
                        widths[index] = percent / 100.0
                    }
                    ChipEdit::Remove(index) if index < widths.len() => {
                        widths.remove(index);
                    }
                    ChipEdit::Move(from, to) if from < widths.len() && to < widths.len() => {
                        let width = widths.remove(from);
                        widths.insert(to, width);
                    }
                    _ => return,
                }
                write(widths);
            }),
    );
    f.sync_when(
        &chips,
        |s| s.settings.layout.scrolling.preset_column_widths.clone(),
        |chips, s| {
            let widths = &s.settings.layout.scrolling.preset_column_widths;
            chips.set_values(&widths.iter().map(|width| width * 100.0).collect::<Vec<_>>());
        },
    );
    f.row("Width presets", chips, message).tooltip(
        "Column widths the preset column width command cycles through, as percentages of the screen width. Drag widths to reorder them; click to adjust, or hover to remove.",
    )
}

/// One comfortable length for sliders on the Layouts page.
pub(super) fn lengthen_sliders(view: &objc2_app_kit::NSView) {
    if let Some(slider) = view.downcast_ref::<objc2_app_kit::NSSlider>() {
        slider.widthAnchor().constraintEqualToConstant(140.0).setActive(true);
    }
    for child in view.subviews() {
        lengthen_sliders(&child);
    }
}

/// The narrowest label column on the Layouts page, so its forms share one control edge.
pub(super) const LABEL_COLUMN: f64 = 180.0;

/// Give every form grid in `view` the same label column, so controls in separate
/// subsections and groups share one predictable leading edge, and give continuous
/// sliders one comfortable length.
pub(super) fn align_form(view: &objc2_app_kit::NSView) {
    lengthen_sliders(view);
    fn grids(view: &objc2_app_kit::NSView, out: &mut Vec<Retained<objc2_app_kit::NSGridView>>) {
        if let Some(grid) = view.downcast_ref::<objc2_app_kit::NSGridView>()
            && grid.numberOfColumns() >= 2
        {
            out.push(objc2::Message::retain(grid));
        }
        for child in view.subviews() {
            grids(&child, out);
        }
    }
    let mut found = Vec::new();
    grids(view, &mut found);
    let width = found
        .iter()
        .flat_map(|grid| {
            (0..grid.numberOfRows()).filter_map(|row| {
                let cell = grid.cellAtColumnIndex_rowIndex(0, row);
                Some(cell.contentView()?.fittingSize().width)
            })
        })
        .fold(LABEL_COLUMN, f64::max);
    for grid in found {
        grid.columnAtIndex(0).setWidth(width);
    }
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
    f.sync_when(
        &path,
        |s| s.settings.ui.menu_bar.layout_folder.clone(),
        |path, s| path.set_value(&s.settings.ui.menu_bar.layout_folder),
    );
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
        Section::new(&ui, "Stack Tabs").description("tabs beside stacked windows."),
        "",
        |s| &s.settings.ui.stack_line,
        |s| &mut s.settings.ui.stack_line,
    );
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
    f.sync_when(
        &recorder,
        |s| {
            (
                match &s.settings.focus_follows_mouse_disable_hotkey {
                    Some(HotkeySpec::Hotkey(key)) => Some(key.clone()),
                    _ => None,
                },
                s.settings.focus_follows_mouse,
            )
        },
        |recorder, s| {
            recorder.set_value(None);
            if let Some(HotkeySpec::Hotkey(key)) = &s.settings.focus_follows_mouse_disable_hotkey {
                recorder.set_title(&key.to_string());
            }
            recorder.set_enabled(s.settings.focus_follows_mouse);
        },
    );
    f.sync_when(
        &modifier_recorder,
        |s| {
            (
                match &s.settings.focus_follows_mouse_disable_hotkey {
                    Some(HotkeySpec::ModifiersOnly { modifiers }) => {
                        from_rift_modifiers(*modifiers)
                    }
                    _ => Modifiers::empty(),
                },
                s.settings.focus_follows_mouse,
            )
        },
        |recorder, s| {
            recorder.set_value(match &s.settings.focus_follows_mouse_disable_hotkey {
                Some(HotkeySpec::ModifiersOnly { modifiers }) => from_rift_modifiers(*modifiers),
                _ => Modifiers::empty(),
            });
            recorder.set_enabled(s.settings.focus_follows_mouse);
        },
    );
    let clear = Button::new(&ui, "Clear").on_click(move || submit(None));
    Section::new(&ui, "Suspend pointer focus while held")
        .description("Record either a shortcut or modifiers. Recording one replaces the other.")
        .row(SettingsRow::new(&ui, "Shortcut", recorder))
        .row(SettingsRow::new(&ui, "Modifiers only", modifier_recorder))
        .content(clear)
        .footer(message)
}
