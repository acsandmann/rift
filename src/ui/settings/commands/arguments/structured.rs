//! Generated field metadata supplies the shape; edits replace one value without
//! reconstructing a command or dropping arguments the UI did not change.
use cgs::settings::{CommandForm, FormEdit, FormField, FormValue};
use serde_json::Value;

use super::*;

enum Kind {
    Text,
    Number(String, bool),
    Boolean,
    Choice(Vec<Value>),
}
struct Field {
    path: String,
    label: String,
    kind: Kind,
}
fn label(name: &str) -> String {
    match name {
        "pid" => "Process ID".into(),
        "idx" => "Window index".into(),
        "window_id" | "window_server_id" => "System window ID".into(),
        "path" => "File".into(),
        "selector" => "Display".into(),
        "scope" => "Apply to".into(),
        "source" => "Restore from".into(),
        _ => {
            let mut name = name.replace('_', " ");
            if let Some(first) = name.get_mut(..1) {
                first.make_ascii_uppercase();
            }
            name
        }
    }
}
fn json(value: impl serde::Serialize) -> Value {
    serde_json::to_value(value).expect("serializable command argument")
}

pub(super) fn editor(
    ui: Ui,
    model: &Model,
    command: &WmCommand,
    draft: bool,
    set: Setter,
    error: Weak<ValidationMessage>,
) -> Option<Box<dyn NativeView>> {
    let schema = match command {
        WmCommand::Wm(command) => command.command_fields(),
        WmCommand::ReactorCommand(Command::Layout(command)) => command.command_fields(),
        WmCommand::ReactorCommand(Command::Reactor(command)) => command.command_fields(),
        _ => return None,
    };
    if schema.is_empty() {
        return None;
    }
    let document = json(command);
    let (tag, _payload) = document.as_object()?.iter().next()?;
    let source = model.source.borrow();
    let mut fields = Vec::new();
    let mut controls = Vec::new();
    for (name, ty) in schema {
        let path = if *name == "0" && schema.len() == 1 {
            format!("/{tag}")
        } else {
            format!("/{tag}/{name}")
        };
        let value = document.pointer(&path)?;
        let ty = ty.replace(' ', "");
        if ty == "WindowId" {
            let prefix = if *name == "window_id" {
                String::new()
            } else if name.parse::<usize>().is_ok() {
                format!("Window {}", name.parse::<usize>().ok()? + 1)
            } else {
                label(name)
            };
            for (key, kind, title) in [("pid", "i32", "process ID"), ("idx", "u32", "index")] {
                let path = format!("{path}/{key}");
                let title = if prefix.is_empty() {
                    label(key)
                } else {
                    format!("{prefix} {title}")
                };
                controls.push(FormField {
                    label: title.clone(),
                    value: FormValue::Text {
                        value: document.pointer(&path)?.to_string(),
                        placeholder: String::new(),
                    },
                });
                fields.push(Field {
                    path,
                    label: title,
                    kind: Kind::Number(kind.into(), false),
                });
            }
            continue;
        }
        let optional = ty.starts_with("Option<");
        let base = if optional {
            ty.strip_prefix("Option<")?.strip_suffix('>')?
        } else {
            &ty
        };
        let title = label(name);
        let choices: Option<Vec<(String, Value)>> = match base {
            "bool" if optional => Some(vec![
                ("Default".into(), Value::Null),
                ("Enabled".into(), json(true)),
                ("Disabled".into(), json(false)),
            ]),
            "RestoreScope" => Some(
                rift_protocol::RestoreScope::CONFIG_CHOICES
                    .iter()
                    .enumerate()
                    .map(|(i, (title, _))| {
                        (
                            title.to_string(),
                            json(rift_protocol::RestoreScope::from_config_choice(i).unwrap()),
                        )
                    })
                    .collect(),
            ),
            "RestoreSource" => Some(
                rift_protocol::RestoreSource::CONFIG_CHOICES
                    .iter()
                    .enumerate()
                    .map(|(i, (title, _))| {
                        (
                            title.to_string(),
                            json(rift_protocol::RestoreSource::from_config_choice(i).unwrap()),
                        )
                    })
                    .collect(),
            ),
            "LayoutMode" => Some(
                super::super::super::pages::layouts()
                    .iter()
                    .map(|(title, mode)| (title.to_string(), json(mode)))
                    .collect(),
            ),
            "WorkspaceSelector" | "usize" if *name == "workspace" => {
                let named = value.is_string();
                let mut items = Vec::new();
                if optional {
                    items.push(("Current workspace".into(), Value::Null));
                }
                items.extend(
                    (0..source.virtual_workspaces.default_workspace_count).map(|index| {
                        let name = source.virtual_workspaces.workspace_name(index);
                        (
                            format!("{} · {name}", index + 1),
                            if named { json(name) } else { json(index) },
                        )
                    }),
                );
                Some(items)
            }
            "DisplaySelector" => {
                let mut items: Vec<_> = directions(true)
                    .into_iter()
                    .map(|(name, target)| (name, json(target)))
                    .collect();
                items.extend(model.env.displays.borrow().iter().map(|display| {
                    let target = DisplaySelector::Uuid(display.display_uuid.clone());
                    (display_name(&target, &[display.clone()]), json(target))
                }));
                Some(items)
            }
            _ => None,
        };
        let (control, kind) = if let Some(mut choices) = choices {
            if !choices.iter().any(|(_, candidate)| candidate == value) {
                let title = if base == "DisplaySelector" {
                    display_name(
                        &serde_json::from_value(value.clone()).ok()?,
                        &model.env.displays.borrow(),
                    )
                } else {
                    value.as_str().map(str::to_string).unwrap_or_else(|| value.to_string())
                };
                choices.push((title, value.clone()));
            }
            let selected = choices.iter().position(|(_, candidate)| candidate == value)?;
            (
                FormValue::Choice {
                    labels: choices.iter().map(|(label, _)| label.clone()).collect(),
                    selected,
                },
                Kind::Choice(choices.into_iter().map(|(_, value)| value).collect()),
            )
        } else {
            match base {
                "bool" => (FormValue::Boolean(value.as_bool()?), Kind::Boolean),
                "PathBuf" => (FormValue::Path(value.as_str()?.into()), Kind::Text),
                "String" => (
                    FormValue::Text {
                        value: value.as_str()?.into(),
                        placeholder: String::new(),
                    },
                    Kind::Text,
                ),
                "i32" | "u32" | "usize" | "f64" => (
                    FormValue::Text {
                        value: if value.is_null() {
                            String::new()
                        } else {
                            value.to_string()
                        },
                        placeholder: if optional {
                            "Automatic".into()
                        } else {
                            String::new()
                        },
                    },
                    Kind::Number(base.into(), optional),
                ),
                _ => return None,
            }
        };
        controls.push(FormField {
            label: title.clone(),
            value: control,
        });
        fields.push(Field { path, label: title, kind });
    }
    let document = Rc::new(RefCell::new(document));
    let invalid = RefCell::new(vec![None::<String>; fields.len()]);
    Some(Box::new(CommandForm::new(
        &ui,
        controls,
        draft,
        move |index, edit| {
            let field = &fields[index];
            let parsed = match (&field.kind, edit) {
                (Kind::Text, FormEdit::Text(value)) => Ok(Value::String(value)),
                (Kind::Boolean, FormEdit::Boolean(value)) => Ok(Value::Bool(value)),
                (Kind::Choice(values), FormEdit::Choice(index)) => {
                    values.get(index).cloned().ok_or(())
                }
                (Kind::Number(kind, optional), FormEdit::Text(value)) => {
                    let value = value.trim();
                    if *optional && value.is_empty() {
                        Ok(Value::Null)
                    } else {
                        match kind.as_str() {
                            "i32" => value.parse::<i32>().map(json).map_err(|_| ()),
                            "u32" => value.parse::<u32>().map(json).map_err(|_| ()),
                            "usize" => value.parse::<usize>().map(json).map_err(|_| ()),
                            _ => value
                                .parse::<f64>()
                                .ok()
                                .filter(|v| v.is_finite())
                                .map(json)
                                .ok_or(()),
                        }
                    }
                }
                _ => Err(()),
            };
            let unresolved = {
                let mut invalid = invalid.borrow_mut();
                invalid[index] = parsed
                    .as_ref()
                    .err()
                    .map(|_| format!("Enter a valid value for {}.", field.label));
                invalid.iter().flatten().next().cloned()
            };
            let result = parsed
                .map_err(|_| format!("Enter a valid value for {}.", field.label))
                .and_then(|value| {
                    let mut document = document.borrow_mut();
                    *document
                        .pointer_mut(&field.path)
                        .ok_or("This command argument is no longer available")? = value;
                    serde_json::from_value::<WmCommand>(document.clone())
                        .map_err(|_| format!("Enter a valid value for {}.", field.label))
                });
            let result = if let Some(error) = unresolved {
                Err(error)
            } else {
                result
            };
            report(&error, &result.as_ref().map(|_| ()).map_err(Clone::clone));
            if let Ok(command) = result {
                set(command);
            }
        },
    )))
}
