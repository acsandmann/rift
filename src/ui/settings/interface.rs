//! Interface preferences. Previews are event-driven native views, never windows.
use objc2_app_kit::*;

use super::interface_preview::StackPreview;
use super::*;
use crate::common::config::*;
use crate::ui::menu_bar::MenuBarPreview;

pub(super) fn menu_bar(ui: Ui, model: &Rc<Model>) -> Page {
    let mut f = FormBuilder::new(ui, model);
    f.tooltips = true;
    let indicator = Rc::new(MenuBarPreview::new(&ui));
    indicator.height(24.0);
    let width = indicator.ns_view().widthAnchor().constraintEqualToConstant(100.0);
    width.setActive(true);
    let disabled = Rc::new(Caption::single_line(&ui, "Rift indicator is hidden"));
    let bar = HStack::new(&ui)
        .spacing(10.0)
        .insets(Insets {
            top: 12.0,
            left: 12.0,
            bottom: 12.0,
            right: 12.0,
        })
        .push(Label::new(&ui, "").font(&NSFont::systemFontOfSize(16.0)))
        .push(Label::new(&ui, "Finder").font(&NSFont::systemFontOfSize(12.0)))
        .push(Spacer::new(&ui))
        .push(disabled.clone())
        .push(indicator.clone())
        .push(Label::new(&ui, "9:41").font(&NSFont::systemFontOfSize(12.0)));
    let bar = VisualEffect::new(&ui, NSVisualEffectMaterial::HeaderView, bar);
    bar.ns_visual_effect_view()
        .setBlendingMode(NSVisualEffectBlendingMode::WithinWindow);
    bar.ns_visual_effect_view().setState(NSVisualEffectState::Active);
    let disabled_copy = disabled.clone();
    f.sync_when(
        &indicator,
        |s| s.settings.ui.menu_bar.clone(),
        move |view, s| {
            let settings = &s.settings.ui.menu_bar;
            view.update(settings);
            width.setConstant(if settings.enabled { view.width() } else { 0.0 });
            disabled_copy.set_hidden(settings.enabled);
        },
    );
    bar.ns_view().setWantsLayer(true);
    if let Some(layer) = bar.ns_view().layer() {
        layer.setCornerRadius(10.0);
        layer.setMasksToBounds(true);
    }
    let preview = Section::form(&ui, "").content(bar).footer(Caption::new(
        &ui,
        "Example: Development workspace · resize binding mode",
    ));
    let visibility = f.fields(
        Section::new(&ui, "Visibility"),
        &["enabled", "show_empty", "show_binding_mode"],
        |s| &s.settings.ui.menu_bar,
        |s| &mut s.settings.ui.menu_bar,
    );
    let appearance = f.fields(
        Section::new(&ui, "Workspaces"),
        &["mode", "active_label", "display_style"],
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
        );
    }));
    f.sync_when(
        &path,
        |s| s.settings.ui.menu_bar.layout_folder.clone(),
        |path, s| {
            path.set_value(&s.settings.ui.menu_bar.layout_folder);
        },
    );
    let row = f.row("Layout folder", path, message);
    let row = f
        .schema_metadata(row, MenuBarSettings::field("layout_folder").unwrap(), |s| {
            &s.settings.ui.menu_bar
        })
        .wide();
    f.finish(
        SettingsPage::new(&ui, "").section(preview).section(visibility).section(
            appearance.row(row).footer(Caption::new(
                &ui,
                "Custom layout images are used by the workspace menu.",
            )),
        ),
    )
}

pub(super) fn overview(ui: Ui, model: &Rc<Model>) -> Page {
    let mut f = FormBuilder::new(ui, model);
    f.tooltips = true;
    let visibility = f.fields(
        Section::new(&ui, "Workspaces"),
        &["enabled", "show_empty_workspaces", "window_previews"],
        |s| &s.settings.ui.mission_control,
        |s| &mut s.settings.ui.mission_control,
    );
    let transitions = f.fields(
        Section::new(&ui, "Transitions"),
        &["fade_enabled"],
        |s| &s.settings.ui.mission_control,
        |s| &mut s.settings.ui.mission_control,
    );
    let duration = f.field(
        "fade_duration_ms",
        |s| &s.settings.ui.mission_control,
        |s| &mut s.settings.ui.mission_control,
    );
    f.enabled(&duration, |s| {
        s.settings.ui.mission_control.enabled && s.settings.ui.mission_control.fade_enabled
    });
    f.finish(
        SettingsPage::new(&ui, "")
            .section(visibility)
            .section(transitions.row(duration))
            .section(Caption::new(
                &ui,
                "Changes are saved now and take effect after restarting Rift.",
            )),
    )
}

pub(super) fn stack_tabs(ui: Ui, model: &Rc<Model>) -> Page {
    let mut f = FormBuilder::new(ui, model);
    f.tooltips = true;
    let preview = Rc::new(StackPreview::new(&ui));
    preview.height(280.0);
    preview.accessibility_label("Stack Tabs preview. Select an example window using its tab.");
    f.sync_when(
        &preview,
        |s| s.settings.ui.stack_line.clone(),
        |view, s| view.update(&s.settings.ui.stack_line),
    );
    let caption = Rc::new(Caption::new(&ui, ""));
    f.sync_when(
        &caption,
        |s| s.settings.ui.stack_line.enabled,
        |caption, s| {
            caption.set_text(if s.settings.ui.stack_line.enabled {
                "Example macOS apps. Select or close a tab."
            } else {
                "Tabs are hidden. Example windows remain stacked."
            });
        },
    );
    let interaction = f.fields(
        Section::new(&ui, "Behavior"),
        &["enabled", "hover"],
        |s| &s.settings.ui.stack_line,
        |s| &mut s.settings.ui.stack_line,
    );
    let reset_preview = Rc::downgrade(&preview);
    let reset = Button::new(&ui, "Reset preview").borderless().on_click(move || {
        if let Some(preview) = reset_preview.upgrade() {
            preview.reset();
        }
    });
    reset.ns_button().setControlSize(NSControlSize::Small);
    reset.ns_button().setFont(Some(&Font::caption()));
    let footer = HStack::new(&ui).spacing(12.0).push(caption).push(Spacer::new(&ui)).push(reset);
    let weak_preview = Rc::downgrade(&preview);
    f.live_preview = Some(Rc::new(move |source| {
        if let Some(preview) = weak_preview.upgrade() {
            preview.update(&source.settings.ui.stack_line);
        }
    }));
    let placement = f.fields(
        Section::new(&ui, "Placement & size"),
        &["horiz_placement", "vert_placement"],
        |s| &s.settings.ui.stack_line,
        |s| &mut s.settings.ui.stack_line,
    );
    let mut placement = placement;
    for (key, title, min, max, get, set) in [
        (
            "thickness",
            "Thickness",
            20.0,
            64.0,
            (|s: &ConfigSource| s.settings.ui.stack_line.thickness) as fn(&ConfigSource) -> f64,
            (|s: &mut ConfigSource, v| s.settings.ui.stack_line.thickness = v)
                as fn(&mut ConfigSource, f64),
        ),
        (
            "spacing",
            "Spacing",
            0.0,
            100.0,
            (|s: &ConfigSource| s.settings.ui.stack_line.spacing) as fn(&ConfigSource) -> f64,
            (|s: &mut ConfigSource, v| s.settings.ui.stack_line.spacing = v)
                as fn(&mut ConfigSource, f64),
        ),
    ] {
        let (slider, input, message) =
            f.numeric_controls(title, 1.0, false, Some(Rc::new(move |_| (min, max))), get, set);
        let slider = slider.unwrap();
        if key == "thickness" {
            slider.set_step(4.0);
        }
        let row = f.row(title, NumberSlider::with_unit(&ui, slider, input, "pt"), message);
        placement = placement.row(f.schema_metadata(
            row,
            StackLineSettings::field(key).unwrap(),
            |s| &s.settings.ui.stack_line,
        ));
    }
    f.finish(
        SettingsPage::new(&ui, "")
            .section(Section::form(&ui, "").content(preview).footer(footer))
            .section(interaction)
            .section(placement),
    )
}
