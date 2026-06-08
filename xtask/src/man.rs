use std::path::Path;

use clap::{Command, CommandFactory};
use documented::{Documented, DocumentedFields};
use rift_wm::common::config::{
    ActiveWorkspaceLabel, AnimationEasing, AppWorkspaceRule, GapOverride, GapSettings,
    GestureSettings, HapticPattern, HorizontalPlacement, InnerGaps, LayoutMode, LayoutSettings,
    MasterStackNewWindowPlacement, MasterStackSettings, MasterStackSide, MenuBarDisplayMode,
    MenuBarSettings, MissionControlSettings, OuterGaps, ScratchpadConfig, ScrollingAlignment,
    ScrollingFocusNavigationStyle, ScrollingGestureSettings, ScrollingLayoutSettings, Settings,
    StackDefaultOrientation, StackLineSettings, StackSettings, UiSettings, VerticalPlacement,
    VirtualWorkspaceSettings, WindowSnappingSettings, WorkspaceDisplayStyle, WorkspaceLayoutRule,
    WorkspaceSelector,
};
use roff::{Roff, bold, italic, roman};

/// Generate manpages for both `rift` and `rift-cli` into `out_dir`.
pub fn generate(out_dir: &str) -> Result<(), String> {
    let gen_dir = Path::new(out_dir);
    if !gen_dir.exists() {
        std::fs::create_dir_all(gen_dir)
            .map_err(|e| format!("Failed to create output directory '{out_dir}': {e}"))?;
    }

    generate_rift(gen_dir)?;
    generate_rift_cli(gen_dir)?;

    println!("Generated manpages to {out_dir}");
    Ok(())
}

fn generate_rift(out_dir: &Path) -> Result<(), String> {
    let mut cmd = rift_wm::cli::rift::Cli::command()
        .name("rift")
        .about("rift: a tiling window manager for macOS");

    let mut buffer: Vec<u8> = Vec::new();
    write_clap_header(&mut buffer, &cmd, "rift manual")?;
    render_command_recursive(&mut cmd, 1, "rift", &mut buffer)?;
    write_configuration_section(&mut buffer)?;
    write_keys_section(&mut buffer)?;
    write_example_section(&mut buffer)?;
    write_environment_section(&mut buffer, true)?;
    write_files_section(&mut buffer)?;
    write_exit_status_section(&mut buffer)?;

    let path = out_dir.join("rift.1");
    std::fs::write(&path, buffer)
        .map_err(|e| format!("Failed to write manpage to '{}': {}", path.display(), e))?;
    Ok(())
}

fn generate_rift_cli(out_dir: &Path) -> Result<(), String> {
    let mut cmd = rift_wm::cli::rift_cli::Cli::command();
    let mut buffer: Vec<u8> = Vec::new();
    write_clap_header(&mut buffer, &cmd, "rift-cli manual")?;
    render_command_recursive(&mut cmd, 1, "rift-cli", &mut buffer)?;
    write_environment_section(&mut buffer, false)?;
    write_exit_status_section(&mut buffer)?;

    let path = out_dir.join("rift-cli.1");
    std::fs::write(&path, buffer)
        .map_err(|e| format!("Failed to write manpage to '{}': {}", path.display(), e))?;
    Ok(())
}

fn write_clap_header(buffer: &mut Vec<u8>, cmd: &Command, manual: &str) -> Result<(), String> {
    let man = clap_mangen::Man::new(cmd.clone()).manual(manual.to_string());
    man.render_title(buffer)
        .map_err(|e| format!("Failed to render title: {e}"))?;
    man.render_name_section(buffer)
        .map_err(|e| format!("Failed to render name section: {e}"))?;
    man.render_synopsis_section(buffer)
        .map_err(|e| format!("Failed to render synopsis section: {e}"))?;
    man.render_description_section(buffer)
        .map_err(|e| format!("Failed to render description section: {e}"))?;
    Ok(())
}

fn render_command_recursive(
    cmd: &mut Command,
    depth: usize,
    prefix: &str,
    buffer: &mut Vec<u8>,
) -> Result<(), String> {
    let mut sect = Roff::new();
    let title = if depth == 1 {
        "OPTIONS".to_string()
    } else {
        format!("SUBCOMMAND: {prefix}")
    };
    sect.control("SH", [title.as_str()]);

    if let Some(about) = cmd.get_long_about().or(cmd.get_about()) {
        sect.text([roman(about.to_string())]);
    }

    // Render usage with the full path as the binary name so it reads as
    // `rift-cli query windows` rather than just `windows`.
    let usage = cmd
        .clone()
        .bin_name(prefix.to_string())
        .render_usage()
        .to_string();
    sect.control("TP", []);
    sect.text([bold(usage)]);

    render_args(cmd, &mut sect);

    // At depth 2+, commands that themselves have subcommands are collapsed
    // into a single flat list (`Commands:`). This keeps tools like rift-cli
    // readable instead of producing one SUBCOMMAND section per nested verb.
    let has_subs = cmd.get_subcommands().next().is_some();
    if depth >= 2 && has_subs {
        let leaves = collect_leaves(cmd, prefix.to_string());
        if !leaves.is_empty() {
            sect.control("PP", []);
            sect.text([bold("Commands:".to_string())]);
            for (path, about) in leaves {
                sect.control("TP", []);
                sect.text([bold(path)]);
                if let Some(about) = about {
                    sect.text([roman(about)]);
                }
            }
        }
        sect.to_writer(buffer)
            .map_err(|e| format!("Failed to render command section: {e}"))?;
        return Ok(());
    }

    sect.to_writer(buffer)
        .map_err(|e| format!("Failed to render command section: {e}"))?;

    for sub in cmd.get_subcommands_mut() {
        let name = sub.get_name();
        if name == "help" {
            continue;
        }
        let child_prefix = format!("{prefix} {name}");
        render_command_recursive(sub, depth + 1, &child_prefix, buffer)?;
    }
    Ok(())
}

fn render_args(cmd: &Command, sect: &mut Roff) {
    for arg in cmd.get_arguments() {
        if arg.is_hide_set() {
            continue;
        }
        if arg.get_id().as_str() == "help" {
            continue;
        }
        sect.control("TP", []);
        let mut opt = String::new();
        if let Some(short) = arg.get_short() {
            opt.push('-');
            opt.push(short);
            if arg.get_long().is_some() {
                opt.push_str(", ");
            }
        }
        if let Some(long) = arg.get_long() {
            opt.push_str("--");
            opt.push_str(long);
        }
        if opt.is_empty() {
            // Positional argument.
            opt.push('<');
            opt.push_str(&arg.get_id().as_str().to_uppercase());
            opt.push('>');
        }
        sect.text([bold(opt)]);
        if let Some(help) = arg.get_long_help().or(arg.get_help()) {
            sect.text([roman(help.to_string())]);
        }
        if let Some(env) = arg.get_env() {
            sect.text([roman(format!(" [env: {}]", env.to_string_lossy()))]);
        }
        if let Some(default) = arg.get_default_values().iter().next() {
            sect.text([roman(format!(" [default: {}]", default.to_string_lossy()))]);
        }
        if arg.is_required_set() {
            sect.text([roman(" [required]")]);
        }
    }
}

fn collect_leaves(cmd: &Command, prefix: String) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    for sub in cmd.get_subcommands() {
        let name = sub.get_name();
        if name == "help" {
            continue;
        }
        let full = if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix} {name}")
        };
        if sub.get_subcommands().next().is_some() {
            out.extend(collect_leaves(sub, full));
        } else {
            let mut signature = full;
            for arg in sub.get_arguments() {
                if arg.is_hide_set() || arg.get_id().as_str() == "help" {
                    continue;
                }
                let token = if let Some(long) = arg.get_long() {
                    if arg.is_required_set() {
                        format!(" --{long} <{}>", arg.get_id().as_str().to_uppercase())
                    } else {
                        format!(" [--{long}]")
                    }
                } else if let Some(short) = arg.get_short() {
                    if arg.is_required_set() {
                        format!(" -{short} <{}>", arg.get_id().as_str().to_uppercase())
                    } else {
                        format!(" [-{short}]")
                    }
                } else if arg.is_required_set() {
                    format!(" <{}>", arg.get_id().as_str().to_uppercase())
                } else {
                    format!(" [{}]", arg.get_id().as_str().to_uppercase())
                };
                signature.push_str(&token);
            }
            let about = sub.get_about().map(|a| a.to_string());
            out.push((signature, about));
        }
    }
    out
}

/// Build a CONFIGURATION section by listing each config struct and its fields.
/// All doc text comes from the `///` comments via [`documented`] derives, so
/// adding a new field automatically makes it appear here.
fn write_configuration_section(buffer: &mut Vec<u8>) -> Result<(), String> {
    let mut sect = Roff::new();
    sect.control("SH", ["CONFIGURATION"]);
    sect.text([
        roman("Rift reads ".to_string()),
        bold("~/.config/rift/config.toml".to_string()),
        roman(
            " on startup. Each table below describes a section of that file. \
             Descriptions are pulled directly from the source's doc comments \
             via the documented crate, so they stay in sync with the code."
                .to_string(),
        ),
    ]);

    // Tables that map to TOML sections, listed in the order they appear in
    // `rift.default.toml`.
    write_struct::<Settings>(&mut sect, "[settings]");
    write_struct::<LayoutSettings>(&mut sect, "[settings.layout]");
    write_struct::<MasterStackSettings>(&mut sect, "[settings.layout.master_stack]");
    write_struct::<ScrollingLayoutSettings>(&mut sect, "[settings.layout.scrolling]");
    write_struct::<ScrollingGestureSettings>(&mut sect, "[settings.layout.scrolling.gestures]");
    write_struct::<StackSettings>(&mut sect, "[settings.layout.stack]");
    write_struct::<GapSettings>(&mut sect, "[settings.layout.gaps]");
    write_struct::<OuterGaps>(&mut sect, "[settings.layout.gaps.outer]");
    write_struct::<InnerGaps>(&mut sect, "[settings.layout.gaps.inner]");
    write_struct::<GapOverride>(&mut sect, "[settings.layout.gaps.per_display.<UUID>]");
    write_struct::<UiSettings>(&mut sect, "[settings.ui]");
    write_struct::<MenuBarSettings>(&mut sect, "[settings.ui.menu_bar]");
    write_struct::<StackLineSettings>(&mut sect, "[settings.ui.stack_line]");
    write_struct::<MissionControlSettings>(&mut sect, "[settings.ui.mission_control]");
    write_struct::<GestureSettings>(&mut sect, "[settings.gestures]");
    write_struct::<WindowSnappingSettings>(&mut sect, "[settings.window_snapping]");
    write_struct::<VirtualWorkspaceSettings>(&mut sect, "[virtual_workspaces]");
    write_struct::<AppWorkspaceRule>(&mut sect, "  app_rules[].<item>");
    write_struct::<WorkspaceLayoutRule>(&mut sect, "  workspace_rules[].<item>");

    // Enums that show up as values in the tables above.
    sect.control("SS", ["Enumerated values"]);
    write_enum::<LayoutMode>(&mut sect, "LayoutMode");
    write_enum::<AnimationEasing>(&mut sect, "AnimationEasing");
    write_enum::<MasterStackSide>(&mut sect, "MasterStackSide");
    write_enum::<MasterStackNewWindowPlacement>(&mut sect, "MasterStackNewWindowPlacement");
    write_enum::<ScrollingAlignment>(&mut sect, "ScrollingAlignment");
    write_enum::<ScrollingFocusNavigationStyle>(&mut sect, "ScrollingFocusNavigationStyle");
    write_enum::<StackDefaultOrientation>(&mut sect, "StackDefaultOrientation");
    write_enum::<HorizontalPlacement>(&mut sect, "HorizontalPlacement");
    write_enum::<VerticalPlacement>(&mut sect, "VerticalPlacement");
    write_enum::<MenuBarDisplayMode>(&mut sect, "MenuBarDisplayMode");
    write_enum::<ActiveWorkspaceLabel>(&mut sect, "ActiveWorkspaceLabel");
    write_enum::<WorkspaceDisplayStyle>(&mut sect, "WorkspaceDisplayStyle");
    write_enum::<HapticPattern>(&mut sect, "HapticPattern");
    write_enum::<ScratchpadConfig>(&mut sect, "ScratchpadConfig");
    write_enum::<WorkspaceSelector>(&mut sect, "WorkspaceSelector");

    sect.to_writer(buffer)
        .map_err(|e| format!("Failed to write configuration section: {e}"))?;
    Ok(())
}

fn write_struct<T: Documented + DocumentedFields>(sect: &mut Roff, heading: &str) {
    sect.control("SS", [heading]);
    sect.text([roman(T::DOCS.trim().to_string())]);

    // If `heading` names a real TOML section in rift.default.toml, inline that
    // chunk so users see a worked example right next to the field listing.
    if let Some(snippet) = toml_section_snippet(heading) {
        sect.control("PP", []);
        sect.text([italic("Example (from rift.default.toml):".to_string())]);
        sect.control("PP", []);
        sect.control("nf", []);
        sect.control("RS", ["4"]);
        for line in snippet.lines() {
            sect.text([roman(line.to_string())]);
            sect.control("br", []);
        }
        sect.control("RE", []);
        sect.control("fi", []);
    }

    for name in T::FIELD_NAMES {
        sect.control("TP", []);
        sect.text([bold((*name).to_string())]);
        if let Ok(doc) = T::get_field_docs(name) {
            sect.text([roman(doc.trim().to_string())]);
        }
    }
}

/// Return the lines of `rift.default.toml` belonging to the table named by
/// `heading` (e.g. `"[settings.layout.gaps]"`), excluding the header line
/// itself. Returns `None` when the heading is not a real TOML section
/// (placeholders like `"  app_rules[].<item>"` or `"[…]"`).
fn toml_section_snippet(heading: &str) -> Option<String> {
    if !heading.starts_with('[') || !heading.ends_with(']') {
        return None;
    }
    let toml = include_str!("../../rift.default.toml");
    let mut collecting = false;
    let mut found = false;
    let mut lines: Vec<&str> = Vec::new();
    for raw in toml.lines() {
        let trimmed = raw.trim_start();
        if trimmed.starts_with('[') && trimmed.contains(']') {
            let header = &trimmed[..=trimmed.find(']')?];
            if header == heading {
                collecting = true;
                found = true;
                continue;
            } else if collecting {
                break;
            }
        }
        if collecting {
            lines.push(raw);
        }
    }
    if !found {
        return None;
    }
    // Trim leading blank lines.
    while lines.first().is_some_and(|l| l.trim().is_empty()) {
        lines.remove(0);
    }
    // Trim trailing blank lines and comment-only blocks (those usually belong
    // to the *following* section's header in the source TOML).
    while lines.last().is_some_and(|l| {
        let t = l.trim();
        t.is_empty() || t.starts_with('#')
    }) {
        lines.pop();
    }
    if lines.is_empty() {
        return None;
    }
    Some(lines.join("\n"))
}

fn write_enum<T: Documented>(sect: &mut Roff, heading: &str) {
    sect.control("SS", [heading]);
    sect.text([roman(T::DOCS.trim().to_string())]);
}

fn write_keys_section(buffer: &mut Vec<u8>) -> Result<(), String> {
    let mut sect = Roff::new();
    sect.control("SH", ["KEYS"]);
    sect.text([roman(
        "The [keys] table maps hotkey specs to commands. A hotkey spec is a \
         string like \"Alt + Shift + H\". Supported modifiers are Alt, Ctrl, \
         Shift, and Meta (the Command \u{2318} key on macOS). Arrow keys may \
         be written as ArrowUp / ArrowDown / ArrowLeft / ArrowRight or simply \
         Up / Down / Left / Right. The [modifier_combinations] table lets \
         you alias modifier groups (for example, comb1 = \"Alt + Shift\") and \
         then use them in [keys] (\"comb1 + H\")."
            .to_string(),
    )]);
    sect.to_writer(buffer)
        .map_err(|e| format!("Failed to write keys section: {e}"))?;
    Ok(())
}

fn write_example_section(buffer: &mut Vec<u8>) -> Result<(), String> {
    let mut sect = Roff::new();
    sect.control("SH", ["EXAMPLE CONFIG"]);
    sect.text([roman(
        "The configuration below is rift's built-in default. It is reproduced \
         verbatim from rift.default.toml and is used as a fallback when no \
         user config exists."
            .to_string(),
    )]);
    sect.control("PP", []);
    sect.control("nf", []);
    sect.control("RS", ["4"]);
    for line in include_str!("../../rift.default.toml").lines() {
        sect.text([roman(line.to_string())]);
        sect.control("br", []);
    }
    sect.control("RE", []);
    sect.control("fi", []);
    sect.to_writer(buffer)
        .map_err(|e| format!("Failed to write example section: {e}"))?;
    Ok(())
}

fn write_environment_section(buffer: &mut Vec<u8>, daemon: bool) -> Result<(), String> {
    let mut sect = Roff::new();
    sect.control("SH", ["ENVIRONMENT"]);
    let common = [
        (
            "RUST_LOG",
            "Tracing/log filter (tracing-subscriber EnvFilter syntax, e.g. \
             \"rift_wm=debug\" or \"info,rift_wm::actor=trace\").",
        ),
        (
            "RUST_BACKTRACE",
            "Standard Rust backtrace control. rift forces this to \"1\" if \
             unset.",
        ),
        (
            "RIFT_BS_NAME",
            "Mach bootstrap name used for the rift IPC server. Override only \
             when running multiple rift instances side-by-side.",
        ),
    ];
    let cli_only = [(
        "RIFT_CLI_PRETTY",
        "If set to anything other than \"0\", rift-cli pretty-prints JSON \
         output. Defaults to compact output.",
    )];
    let daemon_only: [(&str, &str); 9] = [
        (
            "RIFT_EVENT_TYPE",
            "Set on subscriber commands: event name (\"workspace_changed\", \
             \"windows_changed\", \"window_title_changed\", \
             \"stacks_changed\").",
        ),
        (
            "RIFT_WORKSPACE_ID",
            "Set on subscriber commands: workspace identifier for the event.",
        ),
        (
            "RIFT_WORKSPACE_NAME",
            "Set on subscriber commands: workspace name for the event.",
        ),
        (
            "RIFT_WORKSPACE_INDEX",
            "Set on subscriber commands: workspace index (when available).",
        ),
        (
            "RIFT_WINDOW_COUNT",
            "Set on subscriber commands for windows_changed events.",
        ),
        (
            "RIFT_WINDOWS",
            "Set on subscriber commands: comma-separated list of window ids.",
        ),
        (
            "RIFT_WINDOW_TITLE",
            "Set on subscriber commands for window_title_changed events.",
        ),
        (
            "RIFT_PREVIOUS_WINDOW_TITLE",
            "Previous title before a window_title_changed event.",
        ),
        (
            "RIFT_EVENT_JSON",
            "Full JSON payload of the event being delivered.",
        ),
    ];
    for (k, v) in common {
        sect.control("IP", [k]).text([roman(v.to_string())]);
    }
    if daemon {
        for (k, v) in daemon_only {
            sect.control("IP", [k]).text([roman(v.to_string())]);
        }
    } else {
        for (k, v) in cli_only {
            sect.control("IP", [k]).text([roman(v.to_string())]);
        }
    }
    sect.to_writer(buffer)
        .map_err(|e| format!("Failed to write environment section: {e}"))?;
    Ok(())
}

fn write_files_section(buffer: &mut Vec<u8>) -> Result<(), String> {
    let mut sect = Roff::new();
    sect.control("SH", ["FILES"]);
    let files = [
        ("~/.config/rift/config.toml", "User configuration file."),
        ("~/.rift/", "Runtime data directory."),
        ("~/.rift/layout.ron", "Layout restore file (legacy)."),
    ];
    for (path, desc) in files {
        sect.control("IP", [path]).text([roman(desc.to_string())]);
    }
    sect.to_writer(buffer)
        .map_err(|e| format!("Failed to write files section: {e}"))?;
    Ok(())
}

fn write_exit_status_section(buffer: &mut Vec<u8>) -> Result<(), String> {
    let mut sect = Roff::new();
    sect.control("SH", ["EXIT STATUS"]);
    for (code, reason) in [
        ("0", "Successful program execution."),
        ("1", "Unsuccessful program execution."),
        ("101", "The program panicked."),
    ] {
        sect.control("IP", [code]).text([roman(reason.to_string())]);
    }
    sect.to_writer(buffer)
        .map_err(|e| format!("Failed to write exit status section: {e}"))?;
    Ok(())
}
