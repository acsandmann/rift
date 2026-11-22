//! Interface preferences. Previews are event-driven native views, never windows.
use cgs::app::Application;
use cgs::controls::{Button, NumberSlider, PathField};
use cgs::drawing::VisualEffect;
use cgs::input::KeyRecorder;
use cgs::layout::{HStack, Spacer, VStack};
use cgs::settings::{Section, SettingsPage, ValidationMessage};
use cgs::style::Font;
use cgs::tabs::TabSelector;
use cgs::text::{Caption, Label};
use cgs::view::{NativeControl, NativeView, WeakView};
use cgs::{Insets, Ui};
use objc2_app_kit::*;
use objc2_core_graphics::{CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess};

use super::interface_preview::StackPreview;
use super::*;
use crate::common::config::*;
use crate::sys::hotkey::Hotkey;
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
    let preview_caption = Rc::new(Caption::new(&ui, ""));
    f.sync_when(
        &preview_caption,
        |s| s.settings.ui.menu_bar.clone(),
        |caption, s| {
            let settings = &s.settings.ui.menu_bar;
            caption.set_text(if !settings.enabled {
                "Turn on the indicator to see workspace information in your menu bar."
            } else if settings.current_display_only {
                "Live preview · current display · Development is active"
            } else {
                "Live preview · two displays separated by a divider"
            });
        },
    );
    let preview = Section::form(&ui, "").content(bar).footer(preview_caption);
    let visibility = f.fields(
        Section::new(&ui, "Menu bar"),
        &["enabled", "show_binding_mode"],
        |s| &s.settings.ui.menu_bar,
        |s| &mut s.settings.ui.menu_bar,
    );
    let display = f
        .field(
            "current_display_only",
            |s| &s.settings.ui.menu_bar,
            |s| &mut s.settings.ui.menu_bar,
        )
        .description("Follows the display with the active macOS menu bar.");
    let mode = f.popup(
        "Workspaces to show",
        vec![
            ("All workspaces", MenuBarDisplayMode::All),
            ("Active workspace", MenuBarDisplayMode::Active),
        ],
        |s| s.settings.ui.menu_bar.mode,
        |s, value| s.settings.ui.menu_bar.mode = value,
    );
    f.enabled(&mode, |s| s.settings.ui.menu_bar.enabled);
    let empty = f.field(
        "show_empty",
        |s| &s.settings.ui.menu_bar,
        |s| &mut s.settings.ui.menu_bar,
    );
    let empty_hint = Rc::new(Caption::new(&ui, ""));
    f.sync_when(
        &empty_hint,
        |s| s.settings.ui.menu_bar.mode,
        |hint, s| {
            hint.set_text(if s.settings.ui.menu_bar.mode == MenuBarDisplayMode::All {
                "The active workspace is always included."
            } else {
                "Choose All workspaces to include empty workspaces."
            });
        },
    );
    let empty = empty.description_view(empty_hint);
    f.enabled(&empty, |s| {
        let settings = &s.settings.ui.menu_bar;
        settings.enabled && settings.mode == MenuBarDisplayMode::All
    });
    let workspaces = Section::new(&ui, "Workspaces").row(display).row(mode).row(empty);
    let message = Rc::new(ValidationMessage::new(&ui));
    let weak_model = f.model.clone();
    let error = Rc::downgrade(&message);
    let style = Rc::new(
        TabSelector::choices(&ui, &[("Layout", None), ("Index", None), ("Name", None)]).on_change(
            move |index| {
                Model::submit(
                    &weak_model,
                    Box::new(move |s| {
                        let settings = &mut s.settings.ui.menu_bar;
                        match index {
                            0 => settings.display_style = WorkspaceDisplayStyle::Layout,
                            1 | 2 => {
                                settings.display_style = WorkspaceDisplayStyle::Label;
                                settings.active_label = if index == 1 {
                                    ActiveWorkspaceLabel::Index
                                } else {
                                    ActiveWorkspaceLabel::Name
                                };
                            }
                            _ => {}
                        }
                        Ok(())
                    }),
                    error.clone(),
                );
            },
        ),
    );
    let examples = MenuBarPreview::new(&ui);
    for (index, (display, label)) in [
        (WorkspaceDisplayStyle::Layout, ActiveWorkspaceLabel::Index),
        (WorkspaceDisplayStyle::Label, ActiveWorkspaceLabel::Index),
        (WorkspaceDisplayStyle::Label, ActiveWorkspaceLabel::Name),
    ]
    .into_iter()
    .enumerate()
    {
        let image = examples.choice_image(display, label);
        style.ns_segmented_control().setImage_forSegment(Some(&image), index as isize);
    }
    style.accessibility_label("Show workspaces as: layouts, indexes, or names");
    for (index, tip) in ["Layouts", "Indexes", "Names"].iter().enumerate() {
        style.ns_segmented_control().setToolTip_forSegment(
            Some(&objc2_foundation::NSString::from_str(tip)),
            index as isize,
        );
    }
    f.sync_when(
        &style,
        |s| s.settings.ui.menu_bar.clone(),
        |style, s| {
            let settings = &s.settings.ui.menu_bar;
            let index = if settings.display_style == WorkspaceDisplayStyle::Layout {
                0
            } else if settings.active_label == ActiveWorkspaceLabel::Index {
                1
            } else {
                2
            };
            style.set_selected(index);
            style.ns_segmented_control().setEnabled(settings.enabled);
        },
    );
    let style = f
        .row("Show workspaces as", style, message)
        .description("Layouts show window arrangements. Unnamed workspaces use their index.");
    let fullscreen = f
        .field(
            "show_fullscreen_indicator",
            |s| &s.settings.ui.menu_bar,
            |s| &mut s.settings.ui.menu_bar,
        )
        .description("Adds brackets around fullscreen workspaces in all display styles.");
    let appearance = Section::new(&ui, "Appearance").row(style).row(fullscreen);
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
        SettingsPage::new(&ui, "")
            .section(preview)
            .section(visibility)
            .section(workspaces)
            .section(appearance)
            .section(Section::new(&ui, "Custom images").row(row).footer(Caption::new(
                &ui,
                "Used in the menu that opens when you click the indicator. Does not change the menu bar preview.",
            ))),
    )
}

pub(super) const OVERVIEW_SHORTCUTS: [(OverviewCommand, &str); 5] = [
    (OverviewCommand::ZoomIn, "Zoom In"),
    (OverviewCommand::ZoomOut, "Zoom Out"),
    (OverviewCommand::ActualSize, "Actual Size"),
    (OverviewCommand::FitWorkspace, "Fit Current Workspace"),
    (OverviewCommand::FitAllWorkspaces, "Fit All Workspaces"),
];

pub(super) fn overview(ui: Ui, model: &Rc<Model>) -> Page {
    let mut f = FormBuilder::new(ui, model);
    f.tooltips = true;
    let visibility = f.fields(
        Section::new(&ui, "Workspaces"),
        &[
            "enabled",
            "show_empty_workspaces",
            "window_previews",
            "hide_toolbar",
        ],
        |s| &s.settings.ui.mission_control,
        |s| &mut s.settings.ui.mission_control,
    );
    let permission = Rc::new(Caption::new(&ui, ""));
    f.sync(&permission, |caption, _| {
        caption.set_text(screen_recording_status())
    });
    let status = Rc::downgrade(&permission);
    let check = Button::new(&ui, "Check Screen Recording Access…").on_click(move || {
        if !CGPreflightScreenCaptureAccess() && !CGRequestScreenCaptureAccess() {
            Application::open(
                "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
            );
        }
        if let Some(status) = status.upgrade() {
            status.set_text(screen_recording_status());
        }
    });
    let recording = Section::new(&ui, "Screen Recording")
        .content(VStack::new(&ui).spacing(8.0).push(permission).push(check));
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
    let mut shortcuts = Section::new(&ui, "Keyboard Shortcuts");
    for (action, label) in OVERVIEW_SHORTCUTS {
        let controls = Rc::new(VStack::new(&ui).spacing(4.));
        let message = Rc::new(ValidationMessage::new(&ui));
        let model = Rc::downgrade(model);
        let error = Rc::downgrade(&message);
        let last = RefCell::new(None);
        f.sync(&controls, move |controls, source| {
            let settings = &source.settings.ui.mission_control;
            let mut bindings: Vec<_> = settings
                .keymap()
                .iter()
                .filter(|(_, command)| **command == action)
                .map(|(key, _)| (key.clone(), source.hotkey(key)))
                .collect();
            // Show the familiar Command shortcut first when legacy aliases are present.
            bindings.sort_by_key(|(_, key)| {
                key.as_ref()
                    .is_none_or(|key| key.modifiers != crate::sys::hotkey::Modifiers::META)
            });
            let current = (settings.enabled, bindings);
            if last.borrow().as_ref() == Some(&current) {
                return;
            }
            let mut rows = Vec::new();
            let mut line = HStack::new(&ui).spacing(5.).spacer(&ui);
            for (i, (key, hotkey)) in current.1.iter().enumerate() {
                if i != 0 && i % 4 == 0 {
                    rows.push(Rc::new(line));
                    line = HStack::new(&ui).spacing(5.).spacer(&ui);
                }
                let recorder = overview_recorder(
                    &ui,
                    action,
                    label,
                    Some((key, hotkey.as_ref())),
                    &model,
                    &error,
                    None,
                );
                line.add(recorder.flat_field(&ui));
            }
            if current.1.is_empty() {
                let add = overview_recorder(&ui, action, label, None, &model, &error, None);
                rows.push(Rc::new(line.push(add.flat_field(&ui))));
            } else {
                let pending = Rc::new(HStack::new(&ui).spacer(&ui));
                let add = overview_recorder(
                    &ui,
                    action,
                    label,
                    None,
                    &model,
                    &error,
                    Some(WeakView::new(&pending)),
                );
                pending.add(add.flat_field(&ui));
                pending.set_hidden(true);
                let reveal = pending.clone();
                let plus = Button::new(&ui, "").symbol("plus.circle.fill").borderless().on_click(
                    move || {
                        reveal.set_hidden(false);
                        add.begin_recording();
                    },
                );
                let name = format!("Add a shortcut for {label}");
                plus.accessibility_label(&name);
                plus.tooltip(&name);
                plus.width(20.);
                plus.ns_button().setContentTintColor(Some(&NSColor::secondaryLabelColor()));
                rows.push(Rc::new(line.push(plus)));
                rows.push(pending);
            }
            controls.set_children(rows);
            WeakView::new(controls).set_enabled(settings.enabled);
            last.replace(Some(current));
        });
        shortcuts = shortcuts.row(f.row(label, controls, message));
    }
    f.finish(
        SettingsPage::new(&ui, "")
            .section(visibility)
            .section(recording)
            .section(transitions.row(duration))
            .section(shortcuts)
            .section(Caption::new(&ui, "Enabling Overview requires restarting Rift.")),
    )
}

fn overview_shortcut_edit(
    model: &Weak<Model>,
    error: &Weak<ValidationMessage>,
    action: OverviewCommand,
    old: Option<String>,
) -> impl Fn(Option<String>) + 'static {
    let (model, error) = (model.clone(), error.clone());
    move |new| {
        let old = old.clone();
        Model::submit(
            &model,
            Box::new(move |source| source.edit_overview_shortcut(action, old.as_deref(), new)),
            error.clone(),
        );
    }
}

fn overview_recorder(
    ui: &Ui,
    action: OverviewCommand,
    label: &str,
    binding: Option<(&String, Option<&Hotkey>)>,
    model: &Weak<Model>,
    error: &Weak<ValidationMessage>,
    pending: Option<WeakView>,
) -> Rc<KeyRecorder> {
    use super::commands::{recorded_key, recording_guard, shortcut};
    let old = binding.map(|(key, _)| key.clone());
    let original = binding.and_then(|(_, hotkey)| hotkey.map(shortcut));
    let recorder = Rc::new_cyclic(|weak: &Weak<KeyRecorder>| {
        let (weak, model, error) = (weak.clone(), model.clone(), error.clone());
        let (old, original) = (old.clone(), original.clone());
        let submit = overview_shortcut_edit(&model, &error, action, old.clone());
        let mut capture = recording_guard(model.clone(), error.clone());
        let target = old.clone();
        let validation = error.clone();
        KeyRecorder::new(ui)
            .placeholder("Add Shortcut")
            .capture(move |recording| {
                let result = capture(recording);
                if !recording {
                    report(&validation, &Ok(()));
                }
                if (!recording || result.is_err())
                    && let Some(view) = pending.as_ref().and_then(WeakView::load)
                {
                    view.setHidden(true);
                }
                result
            })
            .conflict(move |value| {
                let result = recorded_key(value)
                    .ok_or_else(|| "This key cannot be assigned as a shortcut".to_owned())
                    .and_then(|key| {
                        model
                            .upgrade()
                            .ok_or_else(|| "Settings has closed".to_owned())?
                            .source
                            .borrow()
                            .validate_overview_shortcut(action, target.as_deref(), Some(&key))
                    });
                report(&error, &result);
                result.err()
            })
            .on_change(move |value| {
                if let Some(recorder) = weak.upgrade() {
                    recorder.set_value(original.clone());
                    if let (None, Some(key)) = (&original, &old) {
                        recorder.set_title(key);
                    }
                }
                if old.is_some() || value.is_some() {
                    submit(value.as_ref().and_then(recorded_key));
                }
            })
    });
    recorder.control_size(NSControlSize::Small);
    recorder.accessibility_label(&format!(
        "{} {label} shortcut",
        if old.is_some() { "Change" } else { "Add" }
    ));
    recorder.set_value(original.clone());
    if let (None, Some(key)) = (&original, &old) {
        recorder.set_title(key);
    }
    recorder
}

fn screen_recording_status() -> &'static str {
    if CGPreflightScreenCaptureAccess() {
        "Screen Recording access is granted. Window previews are available."
    } else {
        "Window previews require Screen Recording access. Allow Rift in System Settings > Privacy & Security > Screen Recording, then reopen Overview. macOS may require restarting Rift."
    }
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
