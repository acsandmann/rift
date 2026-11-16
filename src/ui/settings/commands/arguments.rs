//! Command descriptions and editors driven by generated typed argument accessors.
use super::*;
mod structured;

pub(super) fn layout(cmd: L) -> WmCommand { WmCommand::ReactorCommand(Command::Layout(cmd)) }
pub(super) fn reactor(cmd: R) -> WmCommand { WmCommand::ReactorCommand(Command::Reactor(cmd)) }
pub(super) fn wm(cmd: WmCmd) -> WmCommand { WmCommand::Wm(cmd) }

/// Every action Settings offers: its section, title and default parameters.
pub(super) fn actions() -> &'static [(usize, &'static str, WmCommand)] {
    static ACTIONS: std::sync::LazyLock<Vec<(usize, &'static str, WmCommand)>> =
        std::sync::LazyLock::new(|| {
            let mut actions: Vec<_> =
                L::command_catalog()
                    .into_iter()
                    .map(|(group, label, order, command)| (group, label, order, layout(command)))
                    .chain(R::command_catalog().into_iter().map(
                        |(group, label, order, command)| (group, label, order, reactor(command)),
                    ))
                    .chain(
                        WmCmd::command_catalog().into_iter().map(
                            |(group, label, order, command)| (group, label, order, wm(command)),
                        ),
                    )
                    .collect();
            actions.sort_by_key(|(group, _, order, _)| (*group, *order));
            let mut actions: Vec<_> = actions
                .into_iter()
                .map(|(group, label, _, command)| (group, label, command))
                .collect();
            actions.push((OTHER, "Custom command", WmCommand::Sequence(Vec::new())));
            actions
        });
    &ACTIONS
}
pub(super) fn floating_options(
    cmd: &WmCommand,
) -> Option<rift_protocol::ToggleWindowFloatingOptions> {
    match cmd {
        WmCommand::ConfiguredLayout(ConfiguredLayoutCommand::ToggleWindowFloating(options))
        | WmCommand::ReactorCommand(Command::Layout(L::ToggleWindowFloatingWithOptions(options))) => {
            Some(*options)
        }
        WmCommand::ReactorCommand(Command::Layout(L::ToggleWindowFloating)) => {
            Some(Default::default())
        }
        _ => None,
    }
}

pub(super) fn same_action(a: &WmCommand, b: &WmCommand) -> bool {
    fn kind(command: &WmCommand) -> Option<&'static str> {
        Some(match command {
            WmCommand::Wm(command) => command.command_kind(),
            WmCommand::ReactorCommand(Command::Layout(command)) => command.command_kind(),
            WmCommand::ReactorCommand(Command::Reactor(command)) => command.command_kind(),
            WmCommand::ConfiguredLayout(ConfiguredLayoutCommand::ToggleWindowFloating(_)) => {
                "ToggleWindowFloating"
            }
            WmCommand::ConfiguredLayout(ConfiguredLayoutCommand::SwitchPresetColumnWidth) => {
                "SwitchPresetColumnWidth"
            }
            _ => return None,
        })
    }
    kind(a).is_some_and(|a| Some(a) == kind(b))
}
pub(super) fn template(cmd: &WmCommand) -> Option<usize> {
    actions().iter().position(|(_, _, template)| same_action(template, cmd))
}

/// The script of a shell invocation, which Settings edits as one line.
pub(super) fn shell_script(args: &[String]) -> Option<&str> {
    match args {
        [shell, flag, script] if shell.ends_with("sh") && flag == "-c" => Some(script),
        _ => None,
    }
}

/// A command as its TOML value, the form the config file uses.
pub(super) fn toml_text(cmd: &WmCommand) -> String {
    toml::Value::try_from(cmd).map(|value| value.to_string()).unwrap_or_default()
}
pub(super) fn parse_toml(text: &str) -> Result<WmCommand, String> {
    #[derive(serde::Deserialize)]
    struct Binding {
        command: WmCommand,
    }
    toml::from_str::<Binding>(&format!("command = {text}"))
        .map(|binding| binding.command)
        .map_err(|_| "Enter a command as a TOML value, such as \"toggle_stack\"".into())
}

/// Names the parameter of `cmd` shown after its action title; `None` without parameters.
pub(super) fn argument(
    cmd: &WmCommand,
    s: &ConfigSource,
    displays: &[ScreenInfo],
) -> Option<String> {
    use WmCommand::ReactorCommand as RC;
    if let Some(options) = floating_options(cmd) {
        let mut parts = Vec::new();
        if options.center {
            parts.push("centered".to_string());
        }
        if let Some(size) = options.size {
            parts.push(match size {
                rift_protocol::FloatingWindowSize::Preset(_) => "smart size".into(),
                rift_protocol::FloatingWindowSize::Dimensions { w, h } => format!("{w} × {h}"),
            });
        }
        return Some(parts.join(", "));
    }
    if matches!(
        cmd,
        WmCommand::ConfiguredLayout(ConfiguredLayoutCommand::SwitchPresetColumnWidth)
    ) {
        return None;
    }
    if let Some((value, _)) = command_argument(cmd) {
        return Some(match value {
            Arg::Direction(d) => format!("{d:?}"),
            Arg::Orientation(ResizeOrientation::Smart) => String::new(),
            Arg::Orientation(o) => format!("{o:?}"),
            Arg::Number(n) => n.to_string(),
            Arg::Integer(n) => n.to_string(),
            Arg::Backwards(backwards) => if backwards { "Previous" } else { "Next" }.into(),
            Arg::Keymap(name) => keymap_title(&name).into(),
            Arg::Display(target) => display_name(&target, displays),
            Arg::WorkspaceIndex(i) => s.virtual_workspaces.workspace_name(i),
            Arg::Workspace(WorkspaceSelector::Name(name)) => name,
            Arg::Workspace(WorkspaceSelector::Index(i)) => s
                .virtual_workspaces
                .workspace_names
                .get(i)
                .filter(|name| !name.is_empty())
                .cloned()
                .unwrap_or((i + 1).to_string()),
            Arg::Layout(mode) => {
                let label = super::pages::layout_title(mode);
                match cmd {
                    RC(Command::Layout(L::SetWorkspaceLayout { workspace: Some(i), .. })) => {
                        format!("{label} on {}", s.virtual_workspaces.workspace_name(*i))
                    }
                    _ => label.into(),
                }
            }
        });
    }
    Some(match cmd {
        RC(Command::Reactor(R::SaveLayout { path } | R::RestoreLayout { path, .. })) => {
            path.display().to_string()
        }
        RC(Command::Reactor(R::FocusWindow { window_id, .. })) => {
            format!("Window {}:{}", window_id.pid, window_id.idx)
        }
        RC(Command::Layout(L::SwapWindows(a, b))) => {
            format!("{}:{} ↔ {}:{}", a.pid, a.idx, b.pid, b.idx)
        }
        RC(Command::Layout(L::MoveStackTab { window, target })) => {
            format!("{}:{} → {}:{}", window.pid, window.idx, target.pid, target.idx)
        }
        WmCommand::Wm(WmCmd::Exec(ExecCmd::String(text))) => text.clone(),
        WmCommand::Wm(WmCmd::Exec(ExecCmd::Array(args))) => {
            shell_script(args).map_or_else(|| args.join(" "), str::to_string)
        }
        WmCommand::Sequence(_) => String::new(),
        _ => return None,
    })
}
pub(super) fn display_name(target: &DisplaySelector, displays: &[ScreenInfo]) -> String {
    match target {
        DisplaySelector::Direction(direction) => format!("{direction:?}"),
        DisplaySelector::Index(index) => format!("Display {} (index)", index + 1),
        DisplaySelector::Uuid(uuid) => displays
            .iter()
            .find(|display| display.display_uuid == *uuid)
            .map_or("(disconnected display)".into(), |display| {
                display
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("Display {}", display.id.as_u32()))
            }),
    }
}
pub(super) fn keymap_title(name: &str) -> &str { if name == "default" { "Default" } else { name } }

/// A binding's row title: its action, then its parameters.
pub(super) fn title(cmd: &WmCommand, s: &ConfigSource, displays: &[ScreenInfo]) -> String {
    if let WmCommand::Sequence(steps) = cmd
        && !steps.is_empty()
    {
        return steps
            .iter()
            .map(|step| title(step, s, displays))
            .collect::<Vec<_>>()
            .join(" → ");
    }
    let Some(index) = template(cmd) else {
        return toml_text(cmd);
    };
    match argument(cmd, s, displays) {
        Some(argument) if !argument.is_empty() => format!("{} {argument}", actions()[index].1),
        _ => actions()[index].1.into(),
    }
}

/// A settings selector keeping a current value that
/// `items` doesn't include.
pub(super) fn choice<T: Clone + PartialEq + 'static>(
    ui: Ui,
    mut items: Vec<(String, T)>,
    current: T,
    make: impl Fn(T) -> WmCommand + 'static,
    set: &Setter,
) -> Box<dyn NativeView> {
    let selected = items.iter().position(|(_, value)| *value == current).unwrap_or_else(|| {
        items.push(("Current".into(), current));
        items.len() - 1
    });
    let popup = Style::choices(&ui, items.iter().map(|(name, _)| name));
    popup.set_selected(selected);
    let set = set.clone();
    Box::new(popup.on_change(move |i| set(make(items[i].1.clone()))))
}

pub(super) fn directions(display: bool) -> Vec<(String, DisplaySelector)> {
    [
        Direction::Left,
        Direction::Right,
        Direction::Up,
        Direction::Down,
    ]
    .into_iter()
    .map(|d| {
        let name = match d {
            Direction::Left => "Left",
            Direction::Right => "Right",
            Direction::Up => {
                if display {
                    "Above"
                } else {
                    "Up"
                }
            }
            Direction::Down => {
                if display {
                    "Below"
                } else {
                    "Down"
                }
            }
        };
        let name = if display {
            format!(
                "{name} {}current display",
                if matches!(d, Direction::Left | Direction::Right) {
                    "of "
                } else {
                    ""
                }
            )
        } else {
            name.into()
        };
        (name, DisplaySelector::Direction(d))
    })
    .collect()
}

pub(super) fn has_parameters(cmd: &WmCommand) -> bool {
    if matches!(
        cmd,
        WmCommand::ReactorCommand(Command::Reactor(R::CloseWindow { window_server_id: None }))
            | WmCommand::ReactorCommand(Command::Layout(
                L::NextWorkspace(None) | L::PrevWorkspace(None)
            ))
    ) {
        return false;
    }
    argument(cmd, defaults(), &[]).is_some()
        || toml_text(cmd).starts_with('{')
        || matches!(
            cmd,
            WmCommand::Sequence(_)
                | WmCommand::ConfiguredLayout(ConfiguredLayoutCommand::SwitchPresetColumnWidth)
        )
}

pub(super) fn command_argument(command: &WmCommand) -> Option<(Arg, bool)> {
    match command {
        // This legacy form keeps its full TOML editor, including follow/window targets.
        WmCommand::ReactorCommand(Command::Layout(L::MoveWindowToWorkspace { .. })) => None,
        WmCommand::Wm(command) => command.command_argument(),
        WmCommand::ReactorCommand(Command::Layout(command)) => command.command_argument(),
        WmCommand::ReactorCommand(Command::Reactor(command)) => command.command_argument(),
        WmCommand::ConfiguredLayout(ConfiguredLayoutCommand::SwitchPresetColumnWidth) => {
            Some((Arg::Backwards(false), true))
        }
        _ => None,
    }
}
pub(super) fn with_argument(command: &WmCommand, value: Arg) -> WmCommand {
    match command {
        WmCommand::Wm(command) => command.with_command_argument(value).map(wm),
        WmCommand::ReactorCommand(Command::Layout(command)) => {
            command.with_command_argument(value).map(layout)
        }
        WmCommand::ReactorCommand(Command::Reactor(command)) => {
            command.with_command_argument(value).map(reactor)
        }
        WmCommand::ConfiguredLayout(ConfiguredLayoutCommand::SwitchPresetColumnWidth) => {
            return with_argument(&layout(L::SwitchPresetColumnWidth { backwards: false }), value);
        }
        _ => None,
    }
    .expect("a generated argument must match its command field")
}
pub(super) fn simple_parameter(cmd: &WmCommand) -> bool {
    command_argument(cmd).is_some_and(|(_, simple)| simple)
}

pub(super) fn raw_parameters(cmd: &WmCommand) -> bool {
    has_parameters(cmd)
        && !(simple_parameter(cmd)
            || floating_options(cmd).is_some()
            || matches!(cmd, WmCommand::Wm(WmCmd::Exec(ExecCmd::String(_))))
            || matches!(cmd, WmCommand::Wm(WmCmd::Exec(ExecCmd::Array(args))) if shell_script(args).is_some()))
}

pub(super) fn summary(cmd: &WmCommand, source: &ConfigSource, displays: &[ScreenInfo]) -> String {
    if let Some(options) = floating_options(cmd) {
        let placement = if options.center {
            "Centered"
        } else {
            "In place"
        };
        let size = match options.size {
            None => "Original size".into(),
            Some(rift_protocol::FloatingWindowSize::Preset(_)) => "Smart size".into(),
            Some(rift_protocol::FloatingWindowSize::Dimensions { w, h }) => format!("{w} × {h}"),
        };
        return format!("{placement} · {size}");
    }
    match cmd {
        WmCommand::Sequence(steps) => {
            if steps.is_empty() {
                "Configure command…".into()
            } else {
                format!("{} actions", steps.len())
            }
        }
        WmCommand::Wm(WmCmd::Exec(ExecCmd::String(script))) => {
            if script.trim().is_empty() {
                "Enter command…".into()
            } else {
                script.clone()
            }
        }
        WmCommand::ReactorCommand(Command::Layout(
            L::ResizeWindowGrow(ResizeOrientation::Smart)
            | L::ResizeWindowShrink(ResizeOrientation::Smart),
        )) => "Automatic".into(),
        _ if raw_parameters(cmd) => argument(cmd, source, displays)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "Advanced options…".into()),
        _ => argument(cmd, source, displays).unwrap_or_else(|| {
            if has_parameters(cmd) {
                "Options…".into()
            } else {
                String::new()
            }
        }),
    }
}

pub(super) fn search_query(query: &str) -> String {
    query
        .to_lowercase()
        .replace('⌘', "meta+")
        .replace('⌥', "alt+")
        .replace('⌃', "ctrl+")
        .replace('⇧', "shift+")
        .replace("command", "meta")
        .replace("cmd", "meta")
        .replace("option", "alt")
        .replace("control", "ctrl")
        .replace("workspaces", "workspace")
        .replace("displays", "display")
        .replace("layouts", "layout")
        .replace("above", "up")
        .replace("below", "down")
        .replace('←', "left")
        .replace('→', "right")
        .replace('↑', "up")
        .replace('↓', "down")
}

/// The inline control for `cmd`'s parameters, named `Parameter`. A draft applies each
/// keystroke; saved bindings apply committed values, so typing doesn't rebuild the row.
pub(super) fn parameters(
    ui: Ui,
    model: &Model,
    cmd: &WmCommand,
    draft: bool,
    set: Setter,
    error: Weak<ValidationMessage>,
) -> Option<Box<dyn NativeView>> {
    let s = model.source.borrow();
    if let Some((argument @ (Arg::Number(_) | Arg::Integer(_)), _)) = command_argument(cmd) {
        let (value, integer) = match argument {
            Arg::Number(v) => (v, false),
            Arg::Integer(v) => (v as f64, true),
            _ => unreachable!(),
        };
        let field = Style::number_field(&ui, value, integer);
        let (command, set) = (cmd.clone(), set.clone());
        let apply = move |v| {
            set(with_argument(
                &command,
                if integer {
                    Arg::Integer(v as i32)
                } else {
                    Arg::Number(v)
                },
            ))
        };
        let field = if draft {
            field.on_edit(apply)
        } else {
            field.on_change(apply)
        };
        field.accessibility_label("Parameter");
        compressible(&field);
        return Some(Box::new(field));
    }
    let text =
        |value: &str, placeholder: &str, make: Box<dyn Fn(String) -> Result<WmCommand, String>>| {
            let field = Style::text_field(
                &ui,
                value,
                placeholder,
                raw_parameters(cmd) || matches!(cmd, WmCommand::Wm(WmCmd::Exec(_))),
            );
            let (set, error) = (set.clone(), error.clone());
            let apply = move |text: String| {
                let command = make(text);
                report(&error, &command.as_ref().map(|_| ()).map_err(Clone::clone));
                if let Ok(command) = command {
                    set(command);
                }
            };
            Box::new(if draft {
                field.on_change(apply)
            } else {
                field.on_commit(apply)
            }) as Box<dyn NativeView>
        };
    // Popover forms use the same native controls as inline editing, with a
    // stable label/value column so one option never squeezes another.
    let option_row = |label: &str, control: Box<dyn NativeView>| Style::option(&ui, label, control);
    let control = match cmd.clone() {
        cmd if raw_parameters(&cmd) => {
            if let Some(form) =
                structured::editor(ui, model, &cmd, draft, set.clone(), error.clone())
            {
                return Some(form);
            }
            let value = if cmd == WmCommand::Sequence(Vec::new()) {
                String::new()
            } else {
                toml_text(&cmd)
            };
            Box::new(cgs::settings::AdvancedCommandEditor::new(
                &ui,
                &value,
                draft,
                move |value| {
                    let command = parse_toml(&value);
                    report(&error, &command.as_ref().map(|_| ()).map_err(Clone::clone));
                    if let Ok(command) = command {
                        set(command);
                    }
                },
            )) as Box<dyn NativeView>
        }
        cmd if floating_options(&cmd).is_some() => {
            let options = floating_options(&cmd).unwrap();
            let state = Rc::new(RefCell::new(options));
            let (changed, apply) = (state.clone(), set.clone());
            let center_set: Setter = Rc::new(move |command| {
                changed.borrow_mut().center = floating_options(&command).unwrap().center;
                apply(WmCommand::ConfiguredLayout(
                    ConfiguredLayoutCommand::ToggleWindowFloating(*changed.borrow()),
                ));
            });
            let center = choice(
                ui,
                vec![("In place".into(), false), ("Centered".into(), true)],
                options.center,
                |center| {
                    WmCommand::ConfiguredLayout(ConfiguredLayoutCommand::ToggleWindowFloating(
                        rift_protocol::ToggleWindowFloatingOptions { center, size: None },
                    ))
                },
                &center_set,
            );
            center.accessibility_label("Floating placement");
            use rift_protocol::{FloatingWindowSize as Size, FloatingWindowSizePreset as Preset};
            let dimensions = match options.size {
                Some(size @ Size::Dimensions { .. }) => size,
                _ => Size::Dimensions { w: 640.0, h: 480.0 },
            };
            let Size::Dimensions { w, h } = dimensions else {
                unreachable!()
            };
            let edited = state.clone();
            let field = text(
                &format!("{w} × {h}"),
                "Width × height",
                Box::new(move |value| {
                    let dimensions: Vec<_> = value.split(['x', '×']).map(str::trim).collect();
                    let size = (|| {
                        if dimensions.len() != 2 {
                            return None;
                        }
                        let (w, h) = (
                            dimensions[0].parse::<f64>().ok()?,
                            dimensions[1].parse::<f64>().ok()?,
                        );
                        (w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0)
                            .then_some(Size::Dimensions { w, h })
                    })()
                    .ok_or("Enter a positive width × height, such as 640 × 480")?;
                    edited.borrow_mut().size = Some(size);
                    Ok(WmCommand::ConfiguredLayout(
                        ConfiguredLayoutCommand::ToggleWindowFloating(*edited.borrow()),
                    ))
                }),
            );
            field.accessibility_label("Floating width and height");
            let dimensions_row = Rc::new(option_row("Dimensions", field));
            dimensions_row.set_hidden(!matches!(options.size, Some(Size::Dimensions { .. })));
            let (custom, changed, apply) = (dimensions_row.clone(), state, set.clone());
            let size_set: Setter = Rc::new(move |command| {
                let size = floating_options(&command).unwrap().size;
                changed.borrow_mut().size = size;
                custom.set_hidden(!matches!(size, Some(Size::Dimensions { .. })));
                apply(WmCommand::ConfiguredLayout(
                    ConfiguredLayoutCommand::ToggleWindowFloating(*changed.borrow()),
                ));
            });
            let size = choice(
                ui,
                vec![
                    ("Original size".into(), None),
                    ("Smart size".into(), Some(Size::Preset(Preset::Smart))),
                    ("Custom size".into(), Some(dimensions)),
                ],
                options.size,
                |size| {
                    WmCommand::ConfiguredLayout(ConfiguredLayoutCommand::ToggleWindowFloating(
                        rift_protocol::ToggleWindowFloatingOptions { center: false, size },
                    ))
                },
                &size_set,
            );
            size.accessibility_label("Floating size");
            Box::new(Style::options(
                &ui,
                option_row("Position", center),
                option_row("Size", size),
                dimensions_row,
            )) as Box<dyn NativeView>
        }

        command if command_argument(&command).is_some() => {
            let (current, _) = command_argument(&command).unwrap();
            let mut items: Vec<(String, Arg)> = match &current {
                Arg::Direction(_) => directions(false)
                    .into_iter()
                    .map(|(label, target)| {
                        let DisplaySelector::Direction(d) = target else {
                            unreachable!()
                        };
                        (label, Arg::Direction(d))
                    })
                    .collect(),
                Arg::Orientation(_) => [
                    ResizeOrientation::Smart,
                    ResizeOrientation::Horizontal,
                    ResizeOrientation::Vertical,
                ]
                .into_iter()
                .map(|o| (format!("{o:?}"), Arg::Orientation(o)))
                .collect(),
                Arg::Layout(_) => super::pages::layouts()
                    .iter()
                    .map(|(name, mode)| (name.to_string(), Arg::Layout(*mode)))
                    .collect(),
                Arg::Backwards(_) => vec![
                    ("Next".into(), Arg::Backwards(false)),
                    ("Previous".into(), Arg::Backwards(true)),
                ],
                Arg::Keymap(_) => std::iter::once("default")
                    .chain(s.binding_modes.keys().map(String::as_str))
                    .map(|name| (keymap_title(name).into(), Arg::Keymap(name.into())))
                    .collect(),
                Arg::Workspace(_) | Arg::WorkspaceIndex(_) => {
                    (0..s.virtual_workspaces.default_workspace_count)
                        .map(|i| {
                            let name = s.virtual_workspaces.workspace_name(i);
                            let value = match &current {
                                Arg::Workspace(WorkspaceSelector::Name(_)) => {
                                    Arg::Workspace(WorkspaceSelector::Name(name.clone()))
                                }
                                Arg::Workspace(_) => Arg::Workspace(WorkspaceSelector::Index(i)),
                                _ => Arg::WorkspaceIndex(i),
                            };
                            (format!("{} · {name}", i + 1), value)
                        })
                        .collect()
                }
                Arg::Display(_) => directions(true)
                    .into_iter()
                    .map(|(name, target)| (name, Arg::Display(target)))
                    .chain(model.env.displays.borrow().iter().map(|display| {
                        let target = DisplaySelector::Uuid(display.display_uuid.clone());
                        (
                            format!("Display · {}", display_name(&target, &[display.clone()])),
                            Arg::Display(target),
                        )
                    }))
                    .collect(),
                _ => unreachable!(),
            };
            if !items.iter().any(|(_, value)| *value == current) {
                let label = match &current {
                    Arg::Workspace(WorkspaceSelector::Name(name)) => name.clone(),
                    Arg::Workspace(WorkspaceSelector::Index(i)) | Arg::WorkspaceIndex(i) => {
                        format!("Workspace {}", i + 1)
                    }
                    Arg::Display(target) => display_name(target, &model.env.displays.borrow()),
                    _ => "Current".into(),
                };
                items.push((label, current.clone()));
            }
            choice(
                ui,
                items,
                current,
                move |value| with_argument(&command, value),
                &set,
            )
        }
        WmCommand::Wm(WmCmd::Exec(ExecCmd::String(command))) => text(
            &command,
            "Command",
            Box::new(|command| Ok(wm(WmCmd::Exec(ExecCmd::String(command))))),
        ),
        WmCommand::Wm(WmCmd::Exec(ExecCmd::Array(args))) if shell_script(&args).is_some() => {
            let script = shell_script(&args).unwrap_or_default().to_string();
            text(
                &script,
                "Shell script",
                Box::new(move |script| {
                    Ok(wm(WmCmd::Exec(ExecCmd::Array(vec![
                        args[0].clone(),
                        args[1].clone(),
                        script,
                    ]))))
                }),
            )
        }
        cmd if !has_parameters(&cmd) => return None,
        // Commands without native controls, such as sequences, edit as TOML.
        cmd => {
            let value = if cmd == WmCommand::Sequence(Vec::new()) {
                String::new()
            } else {
                toml_text(&cmd)
            };
            text(
                &value,
                "TOML value, such as [\"toggle_stack\", \"balance\"]",
                Box::new(|text| parse_toml(&text)),
            )
        }
    };
    control.accessibility_label("Parameter");
    compressible(&control);
    Some(control)
}
