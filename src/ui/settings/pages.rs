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
        8 => {
            FormBuilder::new(ui, model).finish(SettingsPage::new(&ui, "").section(about(ui, model)))
        }
        _ => advanced(ui, model),
    }
}

fn general(ui: Ui, model: &Rc<Model>) -> Page {
    let mut f = FormBuilder::new(ui, model);
    let mut page = SettingsPage::new(&ui, "");
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
    f.finish(page)
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
        .spacing(24.0)
        .push(identity)
        .push(
            Section::new(&ui, "")
                .row(
                    SettingsRow::new(&ui, "Software updates", check)
                        .description("Find out if a newer release is available."),
                )
                .footer(status),
        )
        .push(Section::new(&ui, "Resources").content(resources))
}

fn layout(ui: Ui, model: &Rc<Model>) -> Page {
    layout_search_scope(ui, model, 0)
}

pub(super) fn layout_search_scope(ui: Ui, model: &Rc<Model>, initial: usize) -> Page {
    let mut f = FormBuilder::new(ui, model);
    let host = Rc::new(PageHost::new(&ui));
    let current = Rc::new(RefCell::new(Rc::new(layout_scope(ui, model, initial))));
    current.borrow().synchronize(model);
    host.set_page(current.borrow().view.clone());
    let selected = Rc::new(Cell::new(initial));
    let weak_model = Rc::downgrade(model);
    let weak_host = Rc::downgrade(&host);
    let active = current.clone();
    let selection = selected.clone();
    let entries = [
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
    let browser = Sidebar::with_children(&ui, vec![
        SidebarItem { id: 100, title: "Default".into(), symbol: String::new() },
        SidebarItem { id: 101, title: "Layouts".into(), symbol: String::new() },
        SidebarItem { id: 102, title: "Global".into(), symbol: String::new() },
    ], move |group| {
        let range = match group { 100 => 0..1, 101 => 1..7, _ => 7..9 };
        range.map(|id| {
            let (title, symbol) = entries[id];
            SidebarItem { id, title: title.into(), symbol: symbol.into() }
        }).collect()
    }).on_select(move |index| {
        if selection.get() == index { return; }
        if let (Some(model), Some(host)) = (weak_model.upgrade(), weak_host.upgrade()) {
            let page = Rc::new(layout_scope(ui, &model, index));
            page.synchronize(&model);
            host.set_page(page.view.clone());
            *active.borrow_mut() = page;
            selection.set(index);
        }
    });
    browser.group_parents();
    browser.set_selected([1, 3, 4, 5, 6, 7, 8, 10, 11][initial]);
    let split = MasterDetail::new(&ui, browser, host);
    let item = split.ns_split_view_controller().splitViewItems().objectAtIndex(0);
    item.setMinimumThickness(170.0);
    item.setMaximumThickness(200.0);
    let weak_model = Rc::downgrade(model);
    f.sync.push(Box::new(move |_| {
        if let Some(model) = weak_model.upgrade() { current.borrow().synchronize(&model); }
    }));
    f.finish(split)
}

fn layout_scope(ui: Ui, model: &Rc<Model>, index: usize) -> Page {
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
        .row(f.popup(
            "Default layout",
            layouts(),
            |s| s.settings.layout.mode,
            |s, v| s.settings.layout.mode = v,
        ))
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
    use objc2_quartz_core::{CALayer, CATransaction};
    use objc2_app_kit::{NSColor, NSWorkspace};
    let root = CALayer::layer();
    root.setGeometryFlipped(true);
    root.setCornerRadius(12.0);
    root.setBackgroundColor(Some(&NSColor::controlBackgroundColor().CGColor()));
    root.setBorderColor(Some(&NSColor::separatorColor().CGColor()));
    root.setBorderWidth(1.0);
    let windows: Vec<_> = (0..3).map(|_| {
        let layer = CALayer::layer();
        layer.setCornerRadius(6.0);
        layer.setBackgroundColor(Some(&NSColor::controlAccentColor().colorWithAlphaComponent(0.18).CGColor()));
        layer.setBorderColor(Some(&NSColor::controlAccentColor().colorWithAlphaComponent(0.4).CGColor()));
        layer.setBorderWidth(1.0);
        root.addSublayer(&layer);
        layer
    }).collect();
    let initialized = Cell::new(false);
    let update: Rc<dyn Fn(&ConfigSource)> = Rc::new(move |s| {
        let gaps = s.settings.layout.gaps.effective_for_display(display.as_deref());
        // A representative desktop, scaled to keep even large gaps legible.
        let edge = |v: f64| 10.0 + v.clamp(0.0, 200.0) * 0.35;
        let left = edge(gaps.outer.left);
        let top = edge(gaps.outer.top);
        let width = (420.0 - left - edge(gaps.outer.right)).max(24.0);
        let height = (220.0 - top - edge(gaps.outer.bottom)).max(24.0);
        let horizontal = gaps.inner.horizontal.clamp(0.0, 200.0) * 0.35;
        let vertical = gaps.inner.vertical.clamp(0.0, 200.0) * 0.35;
        let half = ((width - horizontal) / 2.0).max(8.0);
        let right = left + half + horizontal;
        let half_height = ((height - vertical) / 2.0).max(8.0);
        CATransaction::begin();
        CATransaction::setDisableActions(!initialized.replace(true) || NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion());
        CATransaction::setAnimationDuration(0.16);
        windows[0].setFrame(CGRect::new(CGPoint::new(left, top), CGSize::new(half, height)));
        windows[1].setFrame(CGRect::new(CGPoint::new(right, top), CGSize::new(half, half_height)));
        windows[2].setFrame(CGRect::new(CGPoint::new(right, top + half_height + vertical), CGSize::new(half, half_height)));
        CATransaction::commit();
    });
    f.gap_preview = Some(update.clone());
    f.sync.push(Box::new(move |source| update(source)));
    let preview = LayerHost::new(&ui, &root);
    preview.width(420.0);
    preview.height(220.0);
    preview.accessibility_label("Preview of screen edges and spacing between windows");
    preview
}

fn layout_spacing(ui: Ui, model: &Rc<Model>) -> Page {
    let mut f = FormBuilder::new(ui, model);
    let preview = gap_preview(ui, &mut f, None);
    let source = model.source.borrow();
    let outer = &source.settings.layout.gaps.outer;
    let inner = &source.settings.layout.gaps.inner;
    let custom = outer.top != outer.bottom || outer.top != outer.left || outer.top != outer.right || inner.horizontal != inner.vertical;
    drop(source);
    let simple = Rc::new(Section::new(&ui, "").row(f.gap(
        "Screen edges (points)",
        |s| s.settings.layout.gaps.outer.top,
        |s, v| s.settings.layout.gaps.outer = OuterGaps { top: v, bottom: v, left: v, right: v },
    )));
    let mut edges = Section::new(&ui, "");
    for (axis, name) in ["Top (points)", "Bottom (points)", "Right (points)", "Left (points)"].into_iter().enumerate() {
        edges = edges.row(f.gap(name,
            move |s| { let g = &s.settings.layout.gaps.outer; [g.top, g.bottom, g.right, g.left][axis] },
            move |s, v| { let g = &mut s.settings.layout.gaps.outer; match axis { 0 => g.top = v, 1 => g.bottom = v, 2 => g.right = v, _ => g.left = v } },
        ));
    }
    let edges = Rc::new(edges);
    let between = Rc::new(Section::new(&ui, "Between windows")
        .row(f.gap("Horizontal (points)", |s| s.settings.layout.gaps.inner.horizontal, |s, v| s.settings.layout.gaps.inner.horizontal = v))
        .row(f.gap("Vertical (points)", |s| s.settings.layout.gaps.inner.vertical, |s, v| s.settings.layout.gaps.inner.vertical = v)));
    let uniform_between = Rc::new(Section::new(&ui, "").row(f.gap(
        "Between windows (points)",
        |s| s.settings.layout.gaps.inner.horizontal,
        |s, v| { s.settings.layout.gaps.inner.horizontal = v; s.settings.layout.gaps.inner.vertical = v; },
    )));
    between.set_hidden(!custom);
    uniform_between.set_hidden(custom);
    let (weak_between, weak_uniform) = (Rc::downgrade(&between), Rc::downgrade(&uniform_between));
    simple.set_hidden(custom);
    edges.set_hidden(!custom);
    let (weak_simple, weak_edges) = (Rc::downgrade(&simple), Rc::downgrade(&edges));
    let weak_model = Rc::downgrade(model);
    let mode = SegmentedControl::new(&ui, &["Simple", "Custom"]).on_change(move |index| {
        if let (Some(simple), Some(edges)) = (weak_simple.upgrade(), weak_edges.upgrade()) {
            simple.set_hidden(index != 0);
            edges.set_hidden(index == 0);
        }
        if let (Some(between), Some(uniform)) = (weak_between.upgrade(), weak_uniform.upgrade()) {
            between.set_hidden(index == 0);
            uniform.set_hidden(index != 0);
        }
        if index == 0 {
            FormBuilder::submit(&weak_model, Box::new(|s| {
                let v = s.settings.layout.gaps.outer.top;
                s.settings.layout.gaps.outer = OuterGaps { top: v, bottom: v, left: v, right: v };
                s.settings.layout.gaps.inner.vertical = s.settings.layout.gaps.inner.horizontal;
                Ok(())
            }), Weak::new());
        }
    });
    mode.set_selected(usize::from(custom));
    let page = SettingsPage::new(&ui, "Spacing").subtitle("Applied to every layout. Values are in points.")
        .section(preview).section(mode).section(Section::form(&ui, "Screen edges").content(simple).content(edges))
        .section(uniform_between).section(between);
    f.finish(page)
}

fn percentage_text(ratio: f64) -> String {
    format!("{:.2}", ratio * 100.0)
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

fn layout_description(mode: LayoutMode) -> &'static str {
    match mode {
        LayoutMode::Traditional => "Arrange windows in adjustable rows and columns.",
        LayoutMode::Bsp => "Split available space as windows are added.",
        LayoutMode::Stack => "Overlap windows while keeping each one visible.",
        LayoutMode::MasterStack => "Keep primary windows large with the rest beside them.",
        LayoutMode::Scrolling => "Arrange windows in a horizontally scrolling strip.",
        LayoutMode::Floating => "Move and resize windows without automatic tiling.",
    }
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
        .push(SectionTitle::new(&ui, layouts().into_iter().find(|(_, value)| *value == mode).unwrap().0))
        .push(Caption::new(&ui, layout_description(mode)));
    let section = match mode {
        LayoutMode::Traditional => options()
            .row(f.switch(
                "Keep windows equally sized",
                |s| s.settings.layout.traditional.equalize_nodes,
                |s, v| s.settings.layout.traditional.equalize_nodes = v,
            ))
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
                ));
            page = page.push(section);
            Section::form(&ui, "").content(SubsectionTitle::new(&ui, "Arrangement"))
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
                );
            page = page.push(section);
            let section = Section::form(&ui, "").content(SubsectionTitle::new(&ui, "Navigation"))
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
                .row(f.inherited_popup(
                    "Animate navigation",
                    bool_choices(),
                    |s| s.settings.layout.scrolling.animate,
                    |_| true,
                    |s, v| s.settings.layout.scrolling.animate = v,
                ));
            page = page.push(section);
            Section::form(&ui, "").content(SubsectionTitle::new(&ui, "Window placement")).row(f.inherited_popup(
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
        .row(f.inherited_popup(
            "Animate gestures",
            bool_choices(),
            |s| s.settings.layout.scrolling.gestures.animate,
            |s| s.settings.layout.scrolling.animate.unwrap_or(true),
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
    let mut page = SettingsPage::new(&ui, "");
    let section = Section::new(&ui, "Menu Bar")
        .row(f.switch(
            "Show menu bar indicator",
            |s| s.settings.ui.menu_bar.enabled,
            |s, v| s.settings.ui.menu_bar.enabled = v,
        ))
        .row({
            let row = f.switch(
                "Show empty workspaces",
                |s| s.settings.ui.menu_bar.show_empty,
                |s, v| s.settings.ui.menu_bar.show_empty = v,
            );
            f.enabled(&row, |s| s.settings.ui.menu_bar.enabled);
            row
        })
        .row({
            let row = f.popup(
                "Workspaces to show",
                vec![
                    ("All", MenuBarDisplayMode::All),
                    ("Active", MenuBarDisplayMode::Active),
                ],
                |s| s.settings.ui.menu_bar.mode,
                |s, v| s.settings.ui.menu_bar.mode = v,
            );
            f.enabled(&row, |s| s.settings.ui.menu_bar.enabled);
            row
        })
        .row({
            let row = f.popup(
                "Active workspace label",
                vec![
                    ("Index", ActiveWorkspaceLabel::Index),
                    ("Name", ActiveWorkspaceLabel::Name),
                ],
                |s| s.settings.ui.menu_bar.active_label,
                |s, v| s.settings.ui.menu_bar.active_label = v,
            );
            f.enabled(&row, |s| s.settings.ui.menu_bar.enabled);
            row
        })
        .row({
            let row = f.popup(
                "Display style",
                vec![
                    ("Layout", WorkspaceDisplayStyle::Layout),
                    ("Label", WorkspaceDisplayStyle::Label),
                ],
                |s| s.settings.ui.menu_bar.display_style,
                |s, v| s.settings.ui.menu_bar.display_style = v,
            );
            f.enabled(&row, |s| s.settings.ui.menu_bar.enabled);
            row
        });
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
    let row = f.row("Layout folder", path, message);
    f.enabled(&row, |s| s.settings.ui.menu_bar.enabled);
    page = page.section(section.row(row));
    let section = Section::new(&ui, "Overview")
        .description("Requires restarting Rift.")
        .row(f.switch(
            "Enabled",
            |s| s.settings.ui.mission_control.enabled,
            |s, v| s.settings.ui.mission_control.enabled = v,
        ))
        .row({
            let row = f.switch(
                "Show empty workspaces",
                |s| s.settings.ui.mission_control.show_empty_workspaces,
                |s, v| s.settings.ui.mission_control.show_empty_workspaces = v,
            );
            f.enabled(&row, |s| s.settings.ui.mission_control.enabled);
            row
        })
        .row({
            let row = f.switch(
                "Window previews",
                |s| s.settings.ui.mission_control.window_previews,
                |s, v| s.settings.ui.mission_control.window_previews = v,
            );
            f.enabled(&row, |s| s.settings.ui.mission_control.enabled);
            row
        })
        .row({
            let row = f.switch(
                "Fade transitions",
                |s| s.settings.ui.mission_control.fade_enabled,
                |s, v| s.settings.ui.mission_control.fade_enabled = v,
            );
            f.enabled(&row, |s| s.settings.ui.mission_control.enabled);
            row
        })
        .row({
            let row = f.number(
                "Fade duration (milliseconds)",
                1.0,
                |s| s.settings.ui.mission_control.fade_duration_ms,
                |s, v| s.settings.ui.mission_control.fade_duration_ms = v,
            );
            f.enabled(&row, |s| s.settings.ui.mission_control.enabled);
            row
        });
    page = page.section(section);
    let section = Section::new(&ui, "Stack Line")
        .description("Experimental")
        .row(f.switch(
            "Enabled",
            |s| s.settings.ui.stack_line.enabled,
            |s, v| s.settings.ui.stack_line.enabled = v,
        ))
        .row({
            let row = f.popup(
                "Interaction",
                vec![
                    ("Hover", StackLineHoverMode::Hover),
                    ("Click", StackLineHoverMode::Click),
                ],
                |s| s.settings.ui.stack_line.hover,
                |s, v| s.settings.ui.stack_line.hover = v,
            );
            f.enabled(&row, |s| s.settings.ui.stack_line.enabled);
            row
        })
        .row({
            let row = f.popup(
                "Horizontal placement",
                vec![
                    ("Top", HorizontalPlacement::Top),
                    ("Bottom", HorizontalPlacement::Bottom),
                ],
                |s| s.settings.ui.stack_line.horiz_placement,
                |s, v| s.settings.ui.stack_line.horiz_placement = v,
            );
            f.enabled(&row, |s| s.settings.ui.stack_line.enabled);
            row
        })
        .row({
            let row = f.popup(
                "Vertical placement",
                vec![
                    ("Left", VerticalPlacement::Left),
                    ("Right", VerticalPlacement::Right),
                ],
                |s| s.settings.ui.stack_line.vert_placement,
                |s, v| s.settings.ui.stack_line.vert_placement = v,
            );
            f.enabled(&row, |s| s.settings.ui.stack_line.enabled);
            row
        })
        .row({
            let row = f.number(
                "Thickness (points)",
                1.0,
                |s| s.settings.ui.stack_line.thickness,
                |s, v| s.settings.ui.stack_line.thickness = v,
            );
            f.enabled(&row, |s| s.settings.ui.stack_line.enabled);
            row
        })
        .row({
            let row = f.number(
                "Spacing (points)",
                1.0,
                |s| s.settings.ui.stack_line.spacing,
                |s, v| s.settings.ui.stack_line.spacing = v,
            );
            f.enabled(&row, |s| s.settings.ui.stack_line.enabled);
            row
        });
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
    page = page.section(
        Section::new(&ui, "Configuration file")
            .description(&path.to_string_lossy())
            .row(f.switch(
                "Reload config when edited externally",
                |s| s.settings.hot_reload,
                |s, v| s.settings.hot_reload = v,
            ))
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
