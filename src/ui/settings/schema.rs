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
        let model = self.model.clone();
        let commit = move |value: FieldValue| {
            if model.upgrade().is_some_and(|model| read(get(&model.source.borrow())) == value) {
                return;
            }
            Self::submit(
                &model,
                Box::new(move |source| write(set(source), value)),
                error.clone(),
            );
        };
        let draft = self.model.upgrade().is_some_and(|model| model.draft_base.is_some());
        let control: Box<dyn NativeView> = match field.kind {
            FieldKind::Bool => {
                let input = Rc::new(
                    Switch::new(&self.ui).on_change(move |value| commit(FieldValue::Bool(value))),
                );
                let weak = Rc::downgrade(&input);
                self.sync.push(Box::new(move |source| {
                    if let (Some(input), FieldValue::Bool(value)) =
                        (weak.upgrade(), read(get(source)))
                    {
                        input.set_value(value);
                    }
                }));
                Box::new(input)
            }
            FieldKind::Number { integer } => {
                let input = NumberField::new(&self.ui);
                let input = if integer { input.integer() } else { input };
                let scale = field.scale;
                let change = move |value| commit(FieldValue::Number(value / scale));
                let input = Rc::new(if draft {
                    input.on_edit(change)
                } else {
                    input.on_change(change)
                });
                input.width(100.0);
                let weak = Rc::downgrade(&input);
                self.sync.push(Box::new(move |source| {
                    if let (Some(input), FieldValue::Number(value)) =
                        (weak.upgrade(), read(get(source)))
                    {
                        input.set_value(value * scale);
                    }
                }));
                Box::new(input)
            }
            FieldKind::Text => {
                let input = TextField::new(&self.ui);
                let change = move |value| commit(FieldValue::Text(value));
                let input = Rc::new(if draft {
                    input.on_change(change)
                } else {
                    input.on_commit(change)
                });
                let weak = Rc::downgrade(&input);
                self.sync.push(Box::new(move |source| {
                    if let (Some(input), FieldValue::Text(value)) =
                        (weak.upgrade(), read(get(source)))
                    {
                        input.set_value(&value);
                    }
                }));
                Box::new(input)
            }
            FieldKind::Choice(choices) => {
                let input = Rc::new(
                    Popup::new(&self.ui).on_change(move |index| commit(FieldValue::Choice(index))),
                );
                input.set_items(choices.iter().map(|choice| choice.0));
                let weak = Rc::downgrade(&input);
                self.sync.push(Box::new(move |source| {
                    if let (Some(input), FieldValue::Choice(value)) =
                        (weak.upgrade(), read(get(source)))
                    {
                        input.set_selected(value);
                    }
                }));
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
        let row = row.help(field.help);
        if let Some(enabled) = field.enabled {
            self.enabled(&row, move |source| enabled(get(source)));
        }
        row
    }
}
