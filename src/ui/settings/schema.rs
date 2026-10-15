//! Native controls generated from config metadata, using the same ConfigActor
//! submission and sheet draft ownership as handcrafted Settings controls.
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
        let control: Box<dyn NativeView> = match field.kind {
            FieldKind::Bool => {
                let input = Rc::new(
                    Switch::new(&self.ui).on_change(move |value| commit(FieldValue::Bool(value))),
                );
                self.sync_when(
                    &input,
                    move |source| read(get(source)),
                    move |input, source| {
                        if let FieldValue::Bool(value) = read(get(source)) {
                            input.set_value(value);
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
                    let weak_input = Rc::downgrade(&input);
                    let app = Application::shared(&self.ui);
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
                    let slider = Rc::new(slider.on_change(move |value| {
                        // Give immediate visual feedback, but save once on release.
                        // Exact text entry remains independent of these convenient ranges.
                        let (min, max, _) = model
                            .upgrade()
                            .and_then(|model| slider_options(field.key, &model.source.borrow()))
                            .unwrap_or((min, max, None));
                        let precision = if max <= 1.0 { 100.0 } else { 10.0 };
                        let value = if value <= min || value >= max {
                            // Keep exact endpoints even when the bound has finer precision.
                            value
                        } else if integer {
                            value.round()
                        } else {
                            (value * precision).round() / precision
                        }
                        .clamp(min, max);
                        if let Some(input) = weak_input.upgrade() {
                            input.set_value(value);
                        }
                        if !app.is_mouse_dragging() {
                            change(value);
                        }
                    }));
                    slider.accessibility_label(field.title);
                    input.accessibility_label(field.title);
                    let weak_slider = Rc::downgrade(&slider);
                    self.sync_when(
                        &input,
                        move |source| (read(get(source)), slider_options(field.key, source)),
                        move |input, source| {
                            if let FieldValue::Number(value) = read(get(source)) {
                                input.set_value(value * scale);
                                if let Some(slider) = weak_slider.upgrade() {
                                    if let Some((min, max, _)) = slider_options(field.key, source) {
                                        slider.set_range(min, max);
                                    }
                                    slider.set_value(value * scale);
                                }
                            }
                        },
                    );
                    Box::new(NumberSlider::new(&self.ui, slider, input))
                } else {
                    input.accessibility_label(field.title);
                    self.sync_when(
                        &input,
                        move |source| read(get(source)),
                        move |input, source| {
                            if let FieldValue::Number(value) = read(get(source)) {
                                input.set_value(value * scale);
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
                    move |input, source| {
                        if let FieldValue::Text(value) = read(get(source)) {
                            input.set_value(&value);
                        }
                    },
                );
                Box::new(input)
            }
            FieldKind::Choice(choices) => {
                let input = Rc::new(
                    Popup::new(&self.ui).on_change(move |index| commit(FieldValue::Choice(index))),
                );
                input.set_items(choices.iter().map(|choice| choice.0));
                if let FieldValue::Choice(index) = read(get(defaults())) {
                    input.set_item_badge(index, Some("Default"));
                }
                self.sync_when(
                    &input,
                    move |source| read(get(source)),
                    move |input, source| {
                        if let FieldValue::Choice(value) = read(get(source)) {
                            input.set_selected(value);
                        }
                    },
                );
                Box::new(input)
            }
            FieldKind::Custom => return None,
        };
        let mut row = self.row(field.title, control, message);
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
        let row = if self.tooltips {
            row.tooltip(field.help)
        } else {
            row.help(field.help)
        };
        if let Some(enabled) = field.enabled {
            self.enabled(&row, move |source| enabled(get(source)));
        }
        row
    }
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
        "fade_duration_ms" => (0.0, 1000.0, None),
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
        "fade_duration_ms" => 50.0,
        "swipe_vertical_tolerance" | "vertical_tolerance" => 0.05,
        _ => return None,
    })
}
