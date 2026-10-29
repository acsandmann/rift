use cgs::app::Application;
use cgs::collection::SettingsList;
use cgs::controls::{
    Button, ChipEdit, LayoutPreview, NumberField, Popup, PreviewAnimation, PreviewWindow,
    ValueChips,
};
use cgs::image::{ImageView, Symbol};
use cgs::input::{KeyRecorder, ModifierRecorder, Modifiers};
use cgs::layout::{Disclosure, GroupBox, HStack, Spacer, VStack};
use cgs::settings::{Section, SettingsPage, SettingsRow, ValidationMessage};
use cgs::style::Font;
use cgs::tabs::TabSelector;
use cgs::text::{Caption, Label, SubsectionTitle};
use cgs::view::{Metrics, NativeControl, NativeView, WeakView};
use cgs::{CGPoint, CGRect, CGSize, Insets, Ui};
use objc2_app_kit::NSAccessibility;

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
        Category::MenuBar => super::interface::menu_bar(ui, model),
        Category::Overview => super::interface::overview(ui, model),
        Category::StackTabs => super::interface::stack_tabs(ui, model),
        Category::About => FormBuilder::new(ui, model).finish(about(ui, &model.env)),
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

/// Stable window identities let the emblem demonstrate Rift without rebuilding views.
fn about_layout(index: usize) -> [PreviewWindow; 3] {
    let frames = match index {
        1 => [
            (0.0, 0.0, 76.0, 140.0),
            (84.0, 0.0, 68.0, 140.0),
            (160.0, 0.0, 76.0, 140.0),
        ],
        2 => [
            (0.0, 0.0, 114.0, 66.0),
            (122.0, 0.0, 114.0, 66.0),
            (0.0, 74.0, 236.0, 66.0),
        ],
        _ => [
            (0.0, 0.0, 138.0, 140.0),
            (146.0, 0.0, 90.0, 66.0),
            (146.0, 74.0, 90.0, 66.0),
        ],
    };
    std::array::from_fn(|id| {
        let (x, y, w, h) = frames[id];
        PreviewWindow::new(id as u64, CGRect::new(CGPoint::new(x, y), CGSize::new(w, h)))
    })
}

fn about_identity(ui: &Ui) -> VStack {
    let preview = Rc::new(LayoutPreview::illustration(ui, CGSize::new(236.0, 140.0)));
    preview.set_windows(&about_layout(0), PreviewAnimation::Immediate);
    preview.ns_view().setAccessibilityElement(false);
    let weak_preview = Rc::downgrade(&preview);
    let mut layout = 0;
    let emblem = Button::new(ui, "").borderless().on_click(move || {
        if let Some(preview) = weak_preview.upgrade() {
            layout = (layout + 1) % 3;
            preview.set_windows(
                &about_layout(layout),
                PreviewAnimation::Animated(std::time::Duration::from_millis(280)),
            );
        }
    });
    emblem.width(268.0);
    emblem.height(144.0);
    emblem.accessibility_label("Rearrange windows");
    emblem.accessibility_help("Show another Rift layout. Respects Reduce Motion.");
    emblem.tooltip("A place for every window. Click to try another layout.");
    emblem.ns_button().setFocusRingType(objc2_app_kit::NSFocusRingType::Exterior);
    emblem.ns_view().addSubview(preview.ns_view());
    preview
        .ns_view()
        .centerXAnchor()
        .constraintEqualToAnchor(&emblem.ns_view().centerXAnchor())
        .setActive(true);
    preview
        .ns_view()
        .centerYAnchor()
        .constraintEqualToAnchor(&emblem.ns_view().centerYAnchor())
        .setActive(true);
    let centered = |text: &str, font: &objc2_app_kit::NSFont, color: &objc2_app_kit::NSColor| {
        let label = Label::new(ui, text).font(font).color(color).wrapping();
        label.ns_text_field().setAlignment(objc2_app_kit::NSTextAlignment::Center);
        label
    };
    VStack::new(ui)
        .spacing(6.0)
        .push(stage(ui, emblem, None))
        .push(centered(
            "Rift",
            &objc2_app_kit::NSFont::systemFontOfSize_weight(28.0, unsafe {
                objc2_app_kit::NSFontWeightSemibold
            }),
            &cgs::style::Color::label(),
        ))
        .push(centered(
            &format!("Version {}", env!("CARGO_PKG_VERSION")),
            &Font::caption(),
            &cgs::style::Color::secondary_label(),
        ))
        .keep(preview)
}

fn about_updates(ui: &Ui, env: &Rc<Env>) -> Section {
    let status =
        Rc::new(Label::new(ui, "Not checked yet").wrapping().color(&cgs::style::Color::label()));
    status.accessibility_label("Update status");
    let icon = Rc::new(ImageView::symbol(ui, "arrow.triangle.2.circlepath").unwrap());
    icon.width(16.0);
    icon.height(16.0);
    icon.ns_image_view()
        .setContentTintColor(Some(&cgs::style::Color::secondary_label()));
    icon.ns_view().setAccessibilityElement(false);
    let check = Button::new(ui, "Check for Updates…");
    check.accessibility_label("Check for Updates…");
    let weak_button = WeakView::new(&check);
    let weak_status = Rc::downgrade(&status);
    let weak_icon = Rc::downgrade(&icon);
    let weak_env = Rc::downgrade(env);
    let check = check.on_click(move || {
        let Some(env) = weak_env.upgrade() else { return };
        if let Some(status) = weak_status.upgrade() {
            status.set_text("Checking for updates…");
            status.ns_text_field().setTextColor(Some(&cgs::style::Color::label()));
        }
        if let Some(icon) = weak_icon.upgrade() {
            icon.ns_image_view()
                .setImage(Symbol::named("arrow.triangle.2.circlepath").as_deref());
            icon.ns_image_view()
                .setContentTintColor(Some(&cgs::style::Color::secondary_label()));
        }
        weak_button.set_enabled(false);
        let (status, icon, button) = (weak_status.clone(), weak_icon.clone(), weak_button.clone());
        // Pending network work must never retain a closed page or its environment.
        env.send(
            Action::CheckUpdates(Box::new(move |result| {
                let (text, symbol, color) = match result {
                    Ok(updates::Release::Current) => (
                        "Rift is up to date.".to_owned(),
                        "checkmark.circle",
                        cgs::style::Color::secondary_label(),
                    ),
                    Ok(updates::Release::Available(version)) => (
                        format!("Version {version} is available."),
                        "arrow.down.circle",
                        cgs::style::Color::control_accent(),
                    ),
                    Err(error) => (
                        error,
                        "exclamationmark.circle",
                        objc2_app_kit::NSColor::systemOrangeColor(),
                    ),
                };
                if let Some(status) = status.upgrade() {
                    status.set_text(&text);
                }
                if let Some(icon) = icon.upgrade() {
                    icon.ns_image_view().setImage(Symbol::named(symbol).as_deref());
                    icon.ns_image_view().setContentTintColor(Some(&color));
                }
                button.set_enabled(true);
            })),
            Box::new(|_| {}),
        );
    });
    let row = HStack::new(ui)
        .spacing(10.0)
        .push(icon)
        .push(status)
        .push(Spacer::new(ui))
        .push(check);
    row.min_height(24.0);
    Section::new(ui, "Software Update").content(GroupBox::new(ui, row))
}

fn about_resources(ui: &Ui) -> Section {
    const RESOURCES: [(&str, &str, &str); 4] = [
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
        (
            "Matrix Room",
            "bubble.left.and.bubble.right",
            "https://matrix.to/#/#rift:matrix.org",
        ),
        ("Sponsor Rift", "heart", "https://github.com/sponsors/acsandmann"),
    ];
    let resource_group = |rows: Vec<(&'static str, &'static str, &'static str)>| {
        let destinations = rows.clone();
        let list = SettingsList::new(
            ui,
            |item: &(&str, &str, &str)| item.0.to_owned(),
            |_| String::new(),
        )
        .external_links()
        .full_length()
        .symbols(|item| item.1.to_owned())
        .on_open(move |index| {
            if let Some((_, _, url)) = destinations.get(index) {
                Application::open(url);
            }
        });
        list.set_rows(rows);
        list
    };
    Section::new(ui, "Resources").content(
        VStack::new(ui)
            .spacing(12.0)
            .push(resource_group(RESOURCES[..3].to_vec()))
            .push(resource_group(RESOURCES[3..].to_vec())),
    )
}

fn about(ui: Ui, env: &Rc<Env>) -> SettingsPage {
    let caption = |text: &str| {
        Label::new(&ui, text)
            .font(&Font::caption())
            .color(&cgs::style::Color::secondary_label())
    };
    let heart = ImageView::symbol(&ui, "heart.fill").unwrap();
    heart.width(11.0);
    heart.height(11.0);
    heart
        .ns_image_view()
        .setContentTintColor(Some(&objc2_app_kit::NSColor::systemRedColor()));
    heart.accessibility_label("love");
    let credit = HStack::new(&ui)
        .spacing(4.0)
        .push(caption("made with"))
        .push(heart)
        .push(caption("by acsandmann"));
    let (leading, trailing) = (Spacer::new(&ui), Spacer::new(&ui));
    let equal = leading
        .ns_view()
        .widthAnchor()
        .constraintEqualToAnchor(&trailing.ns_view().widthAnchor());
    let credit = HStack::new(&ui).push(leading).push(credit).push(trailing);
    equal.setActive(true);
    let footer = VStack::new(&ui).spacing(4.0).push(credit);
    let (top, bottom) = (Spacer::new(&ui), Spacer::new(&ui));
    top.height(8.0);
    bottom.min_height(8.0);
    let content = VStack::new(&ui)
        .spacing(0.0)
        .push(top)
        .push(
            VStack::new(&ui)
                .spacing(20.0)
                .push(about_identity(&ui))
                .push(
                    VStack::new(&ui)
                        .spacing(18.0)
                        .push(about_updates(&ui, env))
                        .push(about_resources(&ui)),
                )
                .push(footer),
        )
        .push(bottom);
    let page = SettingsPage::new(&ui, "").content_width(648.0).section(content);
    let scroll = page.ns_scroll_view();
    let document = scroll.documentView().unwrap();
    document
        .heightAnchor()
        .constraintGreaterThanOrEqualToAnchor(&scroll.contentView().heightAnchor())
        .setActive(true);
    page
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

/// A screen-shaped spacing preview, 16:10 like a Mac display.
pub(super) const PREVIEW: CGSize = CGSize::new(192.0, 120.0);

/// Spacing on a representative desktop, shared or as `display` sees it, following
/// slider drags live.
pub(super) fn gap_preview(
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
        // Compress gaps so small ones stay visible and large ones still leave room for windows.
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
    f.live_preview = Some(update.clone());
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

/// Shared spacing. Simple sets every edge, and both directions between windows, with
/// one slider each; Custom sets each value. Switching only changes the controls, so
/// custom values stay until a simple slider is moved.
pub(super) fn layout_spacing(ui: Ui, model: &Rc<Model>) -> Page {
    let source = model.source.borrow();
    let gaps = source.settings.layout.gaps.effective_for_display(None);
    let custom = gaps.outer.top != gaps.outer.bottom
        || gaps.outer.top != gaps.outer.left
        || gaps.outer.top != gaps.outer.right
        || gaps.inner.horizontal != gaps.inner.vertical;
    drop(source);
    let mut f = FormBuilder::new(ui, model);
    // Built first, so the sliders preview their drags in it.
    let preview = gap_preview(ui, &mut f, None, PREVIEW);
    let mut simple = Section::form(&ui, "");
    let mut detailed = Section::form(&ui, "");
    for (outer, title, names) in [
        (true, "Screen edges", &EDGES[..]),
        (false, "Between windows", &AXES[..]),
    ] {
        let (slider, input, message) = f.numeric_controls(
            title,
            1.0,
            true,
            Some(Rc::new(|_: &ConfigSource| (0.0, 100.0))),
            move |source| spacing_gap(source, None, outer, None),
            move |source, value| set_spacing_gap(source, None, outer, None, value),
        );
        let value = input.unit_field(&ui, "pt");
        value.width(Metrics::VALUE_FIELD_WIDTH);
        // The bezel hosts the field’s view; keep its Rust owner with the row.
        let control = HStack::new(&ui).spacing(8.0).push(slider.unwrap()).push(value).keep(input);
        simple = simple.row(f.row(title, control, message));
        let mut messages = Vec::new();
        let fields = names
            .iter()
            .enumerate()
            .map(|(axis, name)| {
                let (_, input, message) = f.numeric_controls(
                    &format!("{title}, {name}"),
                    1.0,
                    true,
                    None,
                    move |source| spacing_gap(source, None, outer, Some(axis)),
                    move |source, value| set_spacing_gap(source, None, outer, Some(axis), value),
                );
                messages.push(message);
                (*name, input)
            })
            .collect::<Vec<_>>();
        let first = objc2::Message::retain(fields[0].1.ns_text_field());
        let control = messages.into_iter().fold(
            VStack::new(&ui)
                .spacing(2.0)
                .push(super::editors::field_group(&ui, "pt", fields)),
            |stack, message| stack.push(message),
        );
        detailed = detailed.row(SettingsRow::new(&ui, title, control).label_baseline(&first));
    }
    let (simple, detailed) = (Rc::new(simple), Rc::new(detailed));
    simple.set_hidden(custom);
    detailed.set_hidden(!custom);
    let (weak_simple, weak_detailed) = (Rc::downgrade(&simple), Rc::downgrade(&detailed));
    let mode = TabSelector::new(&ui, &["Simple", "Custom"]).on_change(move |index| {
        if let (Some(simple), Some(detailed)) = (weak_simple.upgrade(), weak_detailed.upgrade()) {
            simple.set_hidden(index != 0);
            detailed.set_hidden(index == 0);
        }
    });
    mode.set_selected(usize::from(custom));
    mode.accessibility_label("Spacing controls");
    let page = VStack::new(&ui)
        .spacing(12.0)
        .push(stage(&ui, preview, Some(Box::new(mode))))
        .push(simple)
        .push(detailed);
    align_form(page.ns_view());
    f.finish(page)
}

/// A spacing preview centered above its editor, like a display arrangement, with an
/// optional control beneath it.
pub(super) fn stage(
    ui: &Ui,
    preview: impl NativeView,
    below: Option<Box<dyn NativeView>>,
) -> VStack {
    let centered = |view: Box<dyn NativeView>| {
        let (leading, trailing) = (Spacer::new(ui), Spacer::new(ui));
        let equal = leading.ns_view().widthAnchor();
        let other = trailing.ns_view().widthAnchor();
        let row = HStack::new(ui).push(leading).push(view).push(trailing);
        // Equal margins once both share the row.
        equal.constraintEqualToAnchor(&other).setActive(true);
        row
    };
    let stage = VStack::new(ui).spacing(10.0).insets(Insets {
        top: 4.0,
        left: 0.0,
        bottom: 4.0,
        right: 0.0,
    });
    let stage = stage.push(centered(Box::new(preview)));
    match below {
        Some(view) => stage.push(centered(view)),
        None => stage,
    }
}

/// A ratio as a percentage, without float noise such as `56.99999999999999`.
pub(super) fn percentage_text(ratio: f64) -> String {
    format!("{:.2}", ratio * 100.0)
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

type Following = (&'static str, &'static str, fn(&str) -> String);
const SHARED_POSITION: Following = ("Use shared setting", "Shared", |value| {
    format!("Shared setting: {value}")
});
const AUTOMATIC: Following = ("Automatic", "Automatic", |value| {
    format!("{value}, to suit the master side")
});

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
                SHARED_POSITION,
                insertion(),
                |s| s.settings.layout.$layout.base.window_insertion_point,
                |s| s.settings.layout.base.window_insertion_point.unwrap_or_default(),
                |s, v| s.settings.layout.$layout.base.window_insertion_point = v,
            )
            .tooltip("Where this layout inserts new windows. Use shared setting follows New window position in Shared settings.")
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
        LayoutMode::Bsp => options().row(insertion_point!(bsp)).row(aspect_ratio(&mut f)),
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
                AUTOMATIC,
                arrangement(),
                |s| s.settings.layout.master_stack.master_arrangement,
                natural,
                |s, v| s.settings.layout.master_stack.master_arrangement = v,
            ))
            .row(f.following_popup(
                "Stack arrangement",
                AUTOMATIC,
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
                .row(
                    f.switch(
                        "Animate navigation",
                        |s| s.settings.layout.scrolling.animate.unwrap_or(true),
                        |s, v| s.settings.layout.scrolling.animate = (!v).then_some(false),
                    )
                    .description("Recommended")
                    .tooltip("Recommended. Motion shows where columns move as the strip scrolls."),
                );
            page = page.push(navigation);
            subsection("Window placement").row(insertion_point!(scrolling))
        }
        LayoutMode::Floating => return f.finish(page),
    };
    let page = page.push(section);
    align_form(page.ns_view());
    f.finish(page)
}

/// BSP’s single-window shape: Automatic fills the space; Custom keeps a width-to-height
/// ratio, entered in its own field rather than mixed with a keyword.
fn aspect_ratio(f: &mut FormBuilder) -> SettingsRow {
    const SIXTEEN_NINE: f64 = 16.0 / 9.0;
    let ui = f.ui;
    let message = Rc::new(ValidationMessage::new(&ui));
    let ratio = |s: &ConfigSource| s.settings.layout.bsp.single_window_aspect_ratio;
    let set = |s: &mut ConfigSource, v| s.settings.layout.bsp.single_window_aspect_ratio = v;
    let change = f.assign(ratio, set, Rc::downgrade(&message));
    let field = Rc::new(NumberField::new(&ui).range(0.1, 10.0).on_change({
        let change = change.clone();
        move |value| change(Some(value))
    }));
    field.accessibility_label("Width-to-height ratio");
    let model = f.model.clone();
    let mode = Rc::new(Popup::new(&ui).sections([["Automatic"], ["Custom"]]).on_change(
        move |index| {
            let custom = model.upgrade().and_then(|model| ratio(&model.source.borrow()));
            change(if index == 0 {
                None
            } else {
                custom.or(Some(SIXTEEN_NINE))
            });
        },
    ));
    mode.accessibility_label("Single window aspect ratio");
    mode.set_item_badge(0, Some("Default"));
    let value = field.unit_field(&ui, ": 1");
    value.width(Metrics::VALUE_FIELD_WIDTH);
    let custom = Rc::new(
        HStack::new(&ui)
            .spacing(8.0)
            .push(value)
            .push(Caption::single_line(&ui, "1.78 is 16:9")),
    );
    let weak = Rc::downgrade(&custom);
    f.sync_when(&mode, ratio, move |mode, s| {
        mode.set_selected(usize::from(ratio(s).is_some()));
        if let Some(custom) = weak.upgrade() {
            custom.set_hidden(ratio(s).is_none());
        }
    });
    f.sync_when(&field, ratio, move |field, s| {
        field.set_value(ratio(s).unwrap_or(SIXTEEN_NINE))
    });
    let control = HStack::new(&ui).spacing(10.0).push(mode).push(custom).keep(field);
    f.row("Single window aspect ratio", control, message).tooltip(
        "Automatic fills the available space with a single window. Custom keeps it at a fixed width-to-height shape.",
    )
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
            .compact()
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

/// Sliders on the Layouts page start at a comfortable length and take any room the
/// form has to spare, so their row spans the pane.
pub(super) fn lengthen_sliders(view: &objc2_app_kit::NSView) {
    if let Some(slider) = view.downcast_ref::<objc2_app_kit::NSSlider>() {
        slider
            .widthAnchor()
            .constraintGreaterThanOrEqualToConstant(140.0)
            .setActive(true);
        slider.setContentHuggingPriority_forOrientation(
            1.0,
            objc2_app_kit::NSLayoutConstraintOrientation::Horizontal,
        );
    }
    for child in view.subviews() {
        lengthen_sliders(&child);
    }
}

/// Whether a form control should span its column: sliders do; popups, switches, chips
/// and fields keep their natural width.
fn stretches(view: &objc2_app_kit::NSView) -> bool {
    view.downcast_ref::<objc2_app_kit::NSSlider>().is_some()
        || view.subviews().iter().any(|child| stretches(&child))
}

/// The narrowest label column on the Layouts page, so its forms share one control edge.
pub(super) const LABEL_COLUMN: f64 = 180.0;

/// Give every form grid in `view` the same label column and trailing control edge, so
/// rows in separate subsections and groups line up, with sliders spanning between.
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
                // Measure the title alone; descriptions beneath it wrap to the column.
                fn title(view: &objc2_app_kit::NSView) -> Option<Retained<objc2_app_kit::NSView>> {
                    if view.downcast_ref::<objc2_app_kit::NSTextField>().is_some() {
                        return Some(objc2::Message::retain(view));
                    }
                    view.subviews().iter().find_map(|child| title(&child))
                }
                let content = grid.cellAtColumnIndex_rowIndex(0, row).contentView()?;
                Some(title(&content).unwrap_or(content).fittingSize().width)
            })
        })
        .fold(LABEL_COLUMN, f64::max);
    for grid in found {
        grid.columnAtIndex(0).setWidth(width);
        for row in 0..grid.numberOfRows() {
            let cell = grid.cellAtColumnIndex_rowIndex(1, row);
            // Controls sit at the trailing edge, as in every other pane.
            let fill = cell.contentView().is_some_and(|view| stretches(&view));
            cell.setXPlacement(if fill {
                objc2_app_kit::NSGridCellPlacement::Fill
            } else {
                objc2_app_kit::NSGridCellPlacement::Trailing
            });
        }
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
        let row = f.schema_field(
            field,
            |s| &s.settings.layout.scrolling.gestures,
            |s| &mut s.settings.layout.scrolling.gestures,
        );
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

    let mut section = Section::new(&ui, "Drag & Drop");

    for field in DragDropSettings::fields() {
        let row = if field.key == "modifier" {
            let row = f.text(
                field.title,
                |s| s.settings.drag_drop.modifier.to_string(),
                |s, value| {
                    s.settings.drag_drop.modifier =
                        value.parse().map_err(|error: anyhow::Error| error.to_string())?;

                    Ok(())
                },
            );

            Some(f.schema_metadata(row, field, |s| &s.settings.drag_drop))
        } else {
            f.schema_field(field, |s| &s.settings.drag_drop, |s| &mut s.settings.drag_drop)
        };

        if let Some(row) = row {
            section = section.row(row);
        }
    }

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
