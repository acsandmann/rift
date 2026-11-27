//! Native controls generated from config metadata, using the same ConfigActor
//! submission and sheet draft ownership as handcrafted Settings controls.
use cgs::controls::{NumberField, NumberSlider, Popup, Slider, Switch};
use cgs::input::TextField;
use cgs::settings::{Section, SettingsRow, ValidationMessage};
use cgs::tabs::TabSelector;
use cgs::view::NativeView;
use objc2_app_kit::NSUserInterfaceItemIdentification;

use super::*;
use crate::common::config::{ConfigField, ConfigSchema, FieldKind, FieldValue};

impl FormBuilder {
    pub(super) fn schema_section<T: ConfigSchema>(
        &mut self,
        title: &str,
        group: &str,
        get: fn(&ConfigSource) -> &T,
        set: fn(&mut ConfigSource) -> &mut T,
    ) -> Section {
        self.schema_rows(Section::new(&self.ui, title), group, get, set)
    }

    pub(super) fn schema_rows<T: ConfigSchema>(
        &mut self,
        mut section: Section,
        group: &str,
        get: fn(&ConfigSource) -> &T,
        set: fn(&mut ConfigSource) -> &mut T,
    ) -> Section {
        for field in T::fields().iter().filter(|field| field.group == group) {
            if let Some(row) = self.schema_field(field, get, set) {
                section = section.row(row);
            }
        }
        section
    }

    pub(super) fn schema_field<T: ConfigSchema>(
        &mut self,
        field: &'static ConfigField<T>,
        get: fn(&ConfigSource) -> &T,
        set: fn(&mut ConfigSource) -> &mut T,
    ) -> Option<SettingsRow> {
        let symbols: Option<&'static [Option<&'static str>]> = match field.key {
            "horiz_placement" => Some(&[
                Some("rectangle.topthird.inset.filled"),
                Some("rectangle.bottomthird.inset.filled"),
            ]),
            "vert_placement" => Some(&[
                Some("rectangle.leadingthird.inset.filled"),
                Some("rectangle.trailingthird.inset.filled"),
            ]),
            "alignment" => Some(&[]),
            // These two menu-bar choices are short, mutually exclusive alternatives.
            "mode"
                if std::any::TypeId::of::<T>()
                    == std::any::TypeId::of::<crate::common::config::MenuBarSettings>() =>
            {
                Some(&[Some("square.grid.2x2"), Some("square")])
            }
            "active_label"
                if std::any::TypeId::of::<T>()
                    == std::any::TypeId::of::<crate::common::config::MenuBarSettings>() =>
            {
                Some(&[Some("number"), Some("textformat")])
            }
            _ => None,
        };
        self.schema_field_with_segments(field, get, set, symbols)
    }

    /// Use TabSelector for settings choices presented as tabs, matching Stack Tabs'
    /// placement controls. Custom (non-schema) choices can use TabSelector::choices directly.
    /// Symbols are optional for each choice.
    #[allow(dead_code)]
    pub(super) fn schema_segmented_field<T: ConfigSchema>(
        &mut self,
        field: &'static ConfigField<T>,
        get: fn(&ConfigSource) -> &T,
        set: fn(&mut ConfigSource) -> &mut T,
        symbols: &'static [Option<&'static str>],
    ) -> Option<SettingsRow> {
        self.schema_field_with_segments(field, get, set, Some(symbols))
    }

    fn schema_field_with_segments<T: ConfigSchema>(
        &mut self,
        field: &'static ConfigField<T>,
        get: fn(&ConfigSource) -> &T,
        set: fn(&mut ConfigSource) -> &mut T,
        symbols: Option<&'static [Option<&'static str>]>,
    ) -> Option<SettingsRow> {
        let read = field.read?;
        let write = field.write?;
        let message = Rc::new(ValidationMessage::new(&self.ui));
        let error = Rc::downgrade(&message);
        let commit = self.change(
            move |source| read(get(source)),
            move |source, value| write(set(source), value),
            error,
        );
        let draft = self.draft;
        let mut stretch_slider = None;
        let control: Box<dyn NativeView> = match field.kind {
            FieldKind::Bool => {
                let input = Rc::new(
                    Switch::new(&self.ui).on_change(move |value| commit(FieldValue::Bool(value))),
                );
                self.sync_when(
                    &input,
                    move |source| read(get(source)),
                    |input, value| {
                        if let FieldValue::Bool(value) = value {
                            input.set_value(*value);
                        }
                    },
                );
                Box::new(input)
            }
            FieldKind::Number { integer } => {
                let input = NumberField::new(&self.ui);
                let input = if integer { input.integer() } else { input };
                let scale = field.scale;
                let change = move |value| commit(FieldValue::Number(value / scale));
                let input = Rc::new(if draft {
                    input.on_edit(change.clone())
                } else {
                    input.on_change(change.clone())
                });
                let options = self
                    .model
                    .upgrade()
                    .and_then(|model| slider_options(field.key, &model.source.borrow()));
                if let Some((min, max, notches)) = options {
                    let model = self.model.clone();
                    let slider = Slider::new(&self.ui).range(min, max).continuous(true);
                    let slider = match slider_step(field.key) {
                        Some(step) => slider.step(step),
                        None => slider,
                    };
                    let slider = if let Some(count) = notches {
                        // Counts sit beside percentages; match their field width.
                        input.width(70.0);
                        slider.notches(count)
                    } else {
                        slider
                    };
                    let slider = Rc::new(NumberSlider::track(
                        &self.ui,
                        slider,
                        &input,
                        move |value, _| {
                            // Give immediate visual feedback, but save once on release.
                            // Exact text entry remains independent of these convenient ranges.
                            let (min, max, _) = model
                                .upgrade()
                                .and_then(|model| slider_options(field.key, &model.source.borrow()))
                                .unwrap_or((min, max, None));
                            let precision = if max <= 1.0 { 100.0 } else { 10.0 };
                            Some(
                                if value <= min || value >= max {
                                    // Keep exact endpoints even when the bound has finer precision.
                                    value
                                } else if integer {
                                    value.round()
                                } else {
                                    (value * precision).round() / precision
                                }
                                .clamp(min, max),
                            )
                        },
                        change,
                    ));
                    slider.accessibility_label(field.title);
                    input.accessibility_label(field.title);
                    let weak_slider = Rc::downgrade(&slider);
                    self.sync_when(
                        &input,
                        move |source| (read(get(source)), slider_options(field.key, source)),
                        move |input, (value, options)| {
                            if let FieldValue::Number(value) = value {
                                let value = *value * scale;
                                input.set_value(value);
                                if let Some(slider) = weak_slider.upgrade() {
                                    if let Some((min, max, _)) = *options {
                                        slider.set_range(min, max);
                                    }
                                    slider.set_value(value);
                                }
                            }
                        },
                    );
                    stretch_slider = Some(slider.clone());
                    Box::new(NumberSlider::new(&self.ui, slider, input))
                } else {
                    input.accessibility_label(field.title);
                    self.sync_when(
                        &input,
                        move |source| read(get(source)),
                        move |input, value| {
                            if let FieldValue::Number(value) = value {
                                input.set_value(*value * scale);
                            }
                        },
                    );
                    Box::new(input)
                }
            }
            FieldKind::Text => {
                let input = TextField::new(&self.ui);
                let change = move |value| commit(FieldValue::Text(value));
                let input = Rc::new(if draft {
                    input.on_change(change)
                } else {
                    input.on_commit(change)
                });
                self.sync_when(
                    &input,
                    move |source| read(get(source)),
                    |input, value| {
                        if let FieldValue::Text(value) = value {
                            input.set_value(value);
                        }
                    },
                );
                Box::new(input)
            }
            FieldKind::Choice(choices) => {
                if let Some(symbols) = symbols {
                    let segments: Vec<_> = choices
                        .iter()
                        .enumerate()
                        .map(|(index, choice)| (choice.0, symbols.get(index).copied().flatten()))
                        .collect();
                    let input = Rc::new(
                        TabSelector::choices(&self.ui, &segments)
                            .on_change(move |index| commit(FieldValue::Choice(index))),
                    );
                    self.sync_when(
                        &input,
                        move |source| read(get(source)),
                        move |input, value| {
                            if let FieldValue::Choice(value) = value {
                                input.set_selected(*value);
                            }
                        },
                    );
                    Box::new(input)
                } else {
                    let input = Rc::new(
                        Popup::new(&self.ui)
                            .on_change(move |index| commit(FieldValue::Choice(index))),
                    );
                    input.set_items(choices.iter().map(|choice| choice.0));
                    if let FieldValue::Choice(index) = read(get(defaults())) {
                        input.set_item_badge(index, Some("Default"));
                    }
                    self.sync_when(
                        &input,
                        move |source| read(get(source)),
                        move |input, value| {
                            if let FieldValue::Choice(value) = value {
                                input.set_selected(*value);
                            }
                        },
                    );
                    Box::new(input)
                }
            }
            FieldKind::Custom => return None,
        };
        let mut row = self.row(field.title, control, message);
        if let Some(slider) = stretch_slider {
            row = row.stretch_slider(slider.ns_slider());
        }
        if !field.unit.is_empty() {
            row = row.suffix(field.unit);
        }
        Some(self.schema_metadata(row, field, get))
    }

    /// Attach the same docs and dependency rules to a specialized native editor.
    pub(super) fn schema_metadata<T: ConfigSchema>(
        &mut self,
        row: SettingsRow,
        field: &'static ConfigField<T>,
        get: fn(&ConfigSource) -> &T,
    ) -> SettingsRow {
        let kind = std::any::TypeId::of::<T>();
        let redundant = (kind == std::any::TypeId::of::<crate::common::config::Settings>()
            && matches!(
                field.key,
                "animate"
                    | "default_disable"
                    | "mouse_follows_focus"
                    | "mouse_hides_on_focus"
                    | "focus_follows_mouse"
                    | "hot_reload"
            ))
            || (kind == std::any::TypeId::of::<crate::common::config::VirtualWorkspaceSettings>()
                && field.key == "reapply_app_rules_on_title_change")
            || (kind == std::any::TypeId::of::<crate::common::config::GestureSettings>()
                && matches!(field.key, "invert_horizontal_swipe" | "skip_empty" | "fingers"))
            || (kind == std::any::TypeId::of::<crate::common::config::ScrollingGestureSettings>()
                && matches!(field.key, "invert_horizontal" | "fingers"));
        let row = if redundant {
            row
        } else if self.tooltips {
            row.tooltip(field.help)
        } else {
            row.help(field.help)
        };
        row.control_view().setIdentifier(Some(&objc2_foundation::NSString::from_str(
            &setting_identifier::<T>(field.key),
        )));
        if let Some(enabled) = field.enabled {
            self.enabled(&row, move |source| enabled(get(source)));
        }
        row
    }
}

pub(super) fn setting_identifier<T>(key: &str) -> String {
    format!("rift.setting.{}.{}", std::any::type_name::<T>(), key)
}

/// Useful interaction ranges in display units, rather than config limits.
/// Counts have a small set of real choices; distances and sizes stay smooth.
fn slider_options(key: &str, source: &ConfigSource) -> Option<(f64, f64, Option<usize>)> {
    Some(match key {
        "min_column_width_ratio" => (
            0.0,
            source.settings.layout.scrolling.max_column_width_ratio * 100.0,
            None,
        ),
        "max_column_width_ratio" => (
            source.settings.layout.scrolling.min_column_width_ratio * 100.0,
            100.0,
            None,
        ),
        "fingers" => (2.0, 5.0, Some(4)),
        "master_count" => (1.0, 8.0, Some(8)),
        "distance_pct" => (1.0, 100.0, None),
        "drop_zone_fraction" => (10.0, 45.0, None),
        "workspace_switch_threshold" => (5.0, 150.0, None),
        "swipe_vertical_tolerance" | "vertical_tolerance" => (0.0, 1.0, None),
        "thickness" => (20.0, 64.0, None),
        "spacing" | "stack_offset" => (0.0, 100.0, None),
        _ => return None,
    })
}

/// Settings that read best in round steps; their fields still accept exact values.
fn slider_step(key: &str) -> Option<f64> {
    Some(match key {
        "min_column_width_ratio" | "max_column_width_ratio" => 5.0,
        "stack_offset" | "distance_pct" | "drop_zone_fraction" | "workspace_switch_threshold" => {
            5.0
        }
        "thickness" => 4.0,
        "swipe_vertical_tolerance" | "vertical_tolerance" => 0.05,
        _ => return None,
    })
}
