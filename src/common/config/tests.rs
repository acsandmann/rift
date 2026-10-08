use std::path::PathBuf;

use serde::Deserialize;

use super::parse::migrate_legacy_resize_bindings;
use super::*;
use crate::actor::reactor;
use crate::actor::wm_controller::WmCommand;
use crate::common::collections::HashMap;
use crate::layout_engine::{LayoutCommand, ResizeOrientation};

#[test]
fn scrolling_display_widths_merge_and_validate() {
    let settings: ScrollingLayoutSettings = toml::from_str(
        r#"
            column_width_ratio = 0.7
            min_column_width_ratio = 0.3
            max_column_width_ratio = 0.9
            [per_display."display-a"]
            column_width_ratio = 0.5
            "#,
    )
    .unwrap();
    assert_eq!(settings.preset_column_widths, vec![1.0 / 3.0, 0.5, 2.0 / 3.0]);
    assert_eq!(settings.widths_for_display(Some("display-a")), (0.5, 0.3, 0.9));
    assert_eq!(settings.widths_for_display(Some("other")), (0.7, 0.3, 0.9));
    assert_eq!(settings.widths_for_display(None), (0.7, 0.3, 0.9));
    assert!(settings.validate().is_empty());
    assert!(settings.effective_for_display(Some("display-a")).per_display.is_empty());

    let mut invalid = settings;
    invalid.per_display.get_mut("display-a").unwrap().max_column_width_ratio = Some(0.4);
    assert!(
        invalid
            .validate()
            .iter()
            .any(|issue| issue.contains("per_display[display-a].column_width_ratio"))
    );
}

#[test]
fn layout_insertion_point_supports_global_default_and_per_mode_override() {
    let settings: LayoutSettings = toml::from_str(
        r#"
                window_insertion_point = "end_of_tree"

                [traditional]
                window_insertion_point = "next_to_selection"
                equalize_nodes = true

                [scrolling]
                animate = false
            "#,
    )
    .unwrap();

    assert_eq!(
        settings.window_insertion_point_for(LayoutMode::Traditional),
        WindowInsertionPoint::NextToSelection
    );
    assert_eq!(
        settings.window_insertion_point_for(LayoutMode::Bsp),
        WindowInsertionPoint::EndOfTree
    );
    assert!(settings.traditional.equalize_nodes);
    assert_eq!(settings.scrolling.animate, Some(false));
}

#[test]
fn virtual_workspace_prevent_wrapping_defaults_to_false_and_accepts_suggested_alias() {
    let defaults: VirtualWorkspaceSettings = toml::from_str("").unwrap();
    assert!(!defaults.prevent_wrapping);

    let settings: VirtualWorkspaceSettings = toml::from_str("prevent_wrapping_around = true").unwrap();
    assert!(settings.prevent_wrapping);
}

#[test]
fn app_rules_parse_placement_size_and_focus() {
    let settings: VirtualWorkspaceSettings = toml::from_str(
        r#"
                app_rules = [{
                    app_id = "com.example.Tool",
                    floating = true,
                    position = { x = 0.4, y = 0.7 },
                    size = { w = 640, h = 480 },
                    focus = true
                }]
            "#,
    )
    .unwrap();

    let rule = &settings.app_rules[0];
    assert_eq!(rule.position, Some(AppRulePosition { x: 0.4, y: 0.7 }));
    assert_eq!(rule.size, Some(AppRuleSize { w: Some(640.0), h: Some(480.0) }));
    assert!(rule.focus);
    assert!(settings.validate().is_empty());

    let height_only: VirtualWorkspaceSettings = toml::from_str(
        r#"
                app_rules = [{
                    app_id = "com.example.Panel",
                    size = { h = 320 }
                }]
            "#,
    )
    .unwrap();
    assert_eq!(
        height_only.app_rules[0].size,
        Some(AppRuleSize { w: None, h: Some(320.0) })
    );
    assert!(height_only.validate().is_empty());
}

#[test]
fn app_rule_geometry_validation_rejects_invalid_values() {
    let mut settings = VirtualWorkspaceSettings::default();
    settings.app_rules.push(AppWorkspaceRule {
        app_id: Some("com.example.Tool".into()),
        workspace: None,
        floating: false,
        position: Some(AppRulePosition { x: -0.1, y: 1.1 }),
        size: Some(AppRuleSize {
            w: Some(0.0),
            h: Some(f64::NAN),
        }),
        focus: false,
        manage: Some(true),
        app_name: None,
        title_regex: None,
        title_substring: None,
        ax_role: None,
        ax_subrole: None,
    });

    let issues = settings.validate();
    assert!(issues.iter().any(|issue| issue.contains("between 0 and 1")));
    assert!(issues.iter().any(|issue| issue.contains("only applies")));
    assert!(issues.iter().any(|issue| issue.contains("finite positive")));
}

#[test]
fn app_rule_validation_reports_invalid_regex_and_ignored_effects() {
    let mut settings = VirtualWorkspaceSettings::default();
    settings.app_rules.push(AppWorkspaceRule {
        app_id: Some("com.example.Tool".into()),
        workspace: Some(WorkspaceSelector::Index(1)),
        floating: true,
        focus: true,
        manage: Some(false),
        title_regex: Some("[".into()),
        ..Default::default()
    });

    let issues = settings.validate();
    assert!(issues.iter().any(|issue| issue.contains("invalid title_regex")));
    assert!(issues.iter().any(|issue| issue.contains("effects are ignored")));
}

#[test]
fn horizontal_mouse_warp_config_parsing() {
    let missing: Settings = toml::from_str("").unwrap();
    assert_eq!(missing.horizontal_mouse_warp, None);
    for (value, expected) in [
        ("top-to-bottom", HorizontalMouseWarp::TopToBottom),
        ("bottom-to-top", HorizontalMouseWarp::BottomToTop),
    ] {
        let settings: Settings = toml::from_str(&format!("horizontal_mouse_warp = \"{value}\" ")).unwrap();
        assert_eq!(settings.horizontal_mouse_warp, Some(expected));
    }
    assert!(toml::from_str::<Settings>("horizontal_mouse_warp = \"sideways\"").is_err());
}

#[test]
fn resize_command_config_supports_legacy_and_oriented_forms() {
    #[derive(Deserialize)]
    struct TestConfig {
        keys: HashMap<String, WmCommand>,
    }

    let mut document: toml::Value = toml::from_str(
        r#"
            [keys]
            legacy = "resize_window_grow"
            vertical = { resize_window_shrink = "vertical" }
            smart = { resize_window_grow = "smart" }
            "#,
    )
    .unwrap();
    assert!(migrate_legacy_resize_bindings(&mut document));
    let config: TestConfig = document.try_into().unwrap();

    assert_eq!(
        config.keys["legacy"],
        WmCommand::ReactorCommand(reactor::Command::Layout(LayoutCommand::ResizeWindowGrow(
            ResizeOrientation::Horizontal
        )))
    );
    assert_eq!(
        config.keys["vertical"],
        WmCommand::ReactorCommand(reactor::Command::Layout(LayoutCommand::ResizeWindowShrink(
            ResizeOrientation::Vertical
        )))
    );
    assert_eq!(
        config.keys["smart"],
        WmCommand::ReactorCommand(reactor::Command::Layout(LayoutCommand::ResizeWindowGrow(
            ResizeOrientation::Smart
        )))
    );
}

#[test]
fn menu_bar_layout_folder_defaults_and_expands_home() {
    let settings: MenuBarSettings = toml::from_str("").unwrap();

    assert_eq!(settings.layout_folder, PathBuf::from("~/.config/rift/layouts"));
    assert_eq!(
        settings.resolved_layout_folder(),
        dirs::home_dir().unwrap().join(".config/rift/layouts")
    );
}

#[test]
fn menu_bar_layout_folder_preserves_absolute_paths() {
    let settings: MenuBarSettings = toml::from_str("layout_folder = \"/tmp/rift-layouts\"").unwrap();

    assert_eq!(
        settings.resolved_layout_folder(),
        PathBuf::from("/tmp/rift-layouts")
    );
}

#[test]
fn test_modifier_combinations_in_config() {
    let toml = r#"
            [settings]
            animate = false

            [modifier_combinations]
            comb1 = "Alt + Shift"
            leader = "Ctrl + Alt"

            [keys]
            "comb1 + C" = "toggle_space_activated"
            "leader + Tab" = "next_workspace"
            "Alt + H" = { move_focus = "left" }
        "#;

    let cfg = Config::parse(toml).unwrap();
    // We expect keys to be parsed into hotkeys
    assert!(!cfg.keys.is_empty());
}

#[test]
fn mouse_settings_defaults_and_variants_parse() {
    let defaults: DragDropSettings = toml::from_str("").unwrap();
    assert_eq!(defaults, DragDropSettings::default());

    let settings: DragDropSettings = toml::from_str(
        r#"
                enabled = false
                modifier = "ctrl"
                action1 = "none"
                action2 = "move"
                drop_action = "stack"
                drop_zone_fraction = 0.45
                preview = false
            "#,
    )
    .unwrap();
    assert_eq!(settings.modifier, MouseModifier::Ctrl);
    assert_eq!(settings.action1, MouseAction::None);
    assert_eq!(settings.action2, MouseAction::Move);
    assert_eq!(settings.drop_action, MouseDropAction::Stack);

    for (alias, expected) in [
        ("command", MouseModifier::Cmd),
        ("option", MouseModifier::Alt),
        ("control", MouseModifier::Ctrl),
    ] {
        let parsed: DragDropSettings = toml::from_str(&format!("modifier = \"{alias}\"")).unwrap();
        assert_eq!(parsed.modifier, expected);
    }
}

#[test]
fn legacy_window_snapping_is_removed_before_deserialization() {
    let cfg = Config::parse(
        r#"
                [settings.window_snapping]
                drag_swap_fraction = 0.3
                [keys]
            "#,
    )
    .unwrap();
    assert_eq!(cfg.settings.drag_drop, DragDropSettings::default());
}

#[test]
fn drag_drop_settings_take_precedence_over_legacy_window_snapping() {
    let cfg = Config::parse(
        r#"
                [settings.window_snapping]
                drag_swap_fraction = 0.3
                [settings.drag_drop]
                modifier = "alt"
                drop_action = "stack"
                [keys]
            "#,
    )
    .unwrap();
    assert_eq!(cfg.settings.drag_drop.modifier, MouseModifier::Alt);
    assert_eq!(cfg.settings.drag_drop.drop_action, MouseDropAction::Stack);
}

#[test]
fn invalid_drop_zone_fraction_has_clear_validation_error() {
    let mut cfg = Config::default();
    cfg.settings.drag_drop.drop_zone_fraction = 0.09;
    assert!(
        cfg.validate()
            .iter()
            .any(|issue| issue.contains("drag_drop.drop_zone_fraction"))
    );
    cfg.settings.drag_drop.drop_zone_fraction = 0.46;
    assert!(
        cfg.validate()
            .iter()
            .any(|issue| issue.contains("drag_drop.drop_zone_fraction"))
    );
}

#[test]
fn serde_round_trip_preserves_binding_mode_specs() {
    let cfg = Config::default();
    assert!(!cfg.binding_mode_specs[0].1.is_empty());

    let json = serde_json::to_string(&cfg).unwrap();
    let round_tripped: Config = serde_json::from_str(&json).unwrap();

    assert_eq!(round_tripped.binding_mode_specs, cfg.binding_mode_specs);
}

#[test]
fn keys_only_config_still_has_only_the_default_binding_set() {
    let config = Config::parse(
        r#"
                [settings]
                [keys]
                "A" = "reload_config"
            "#,
    )
    .unwrap();
    assert_eq!(config.binding_mode_specs.len(), 1);
    assert_eq!(config.binding_mode_specs[0].0, "default");
    assert!(config.binding_mode_specs[0].1.iter().any(|(spec, _)| spec == "A"));
}

#[test]
fn custom_binding_modes_parse_and_expand_modifier_combinations() {
    let config = Config::parse(
        r#"
                [settings]
                [modifier_combinations]
                nav = "Alt + Shift"
                [keys]
                "Alt + R" = { binding_mode = "resize" }
                [binding_modes.resize]
                "nav + N" = { binding_mode = "default" }
            "#,
    )
    .unwrap();
    assert_eq!(config.binding_mode_specs[0].0, "default");
    assert_eq!(config.binding_mode_specs[1].0, "resize");
    assert!(config.binding_mode_specs[1]
            .1
            .iter()
            .any(|(spec, command)| spec == "Alt + Shift + N"
                && matches!(command, WmCommand::Wm(crate::actor::wm_controller::WmCmd::BindingMode(target)) if target == "default")));
}

#[test]
fn binding_mode_config_rejects_bad_targets_reserved_default_and_bad_hotkeys() {
    let missing = r#"
            [settings]
            [keys]
            "Alt + R" = { binding_mode = "does-not-exist" }
            [binding_modes.resize]
            "Escape" = { binding_mode = "default" }
        "#;
    assert!(Config::parse(&missing).unwrap_err().to_string().contains("does-not-exist"));

    let reserved = r#"
            [settings]
            [keys]
            [binding_modes.default]
        "#;
    assert!(Config::parse(&reserved).unwrap_err().to_string().contains("reserved"));

    let malformed = r#"
            [settings]
            [keys]
            "NotARealHotkey" = { binding_mode = "default" }
        "#;
    assert!(Config::parse(&malformed).unwrap_err().to_string().contains("hotkey"));
}

#[test]
fn config_validation_rejects_duplicate_binding_mode_names() {
    let mut json = serde_json::to_value(Config::default()).unwrap();
    json["binding_mode_specs"] = serde_json::json!([["default", []], ["default", []]]);
    let config: Config = serde_json::from_value(json).unwrap();
    assert!(config.validate().iter().any(|issue| issue.contains("unique")));
}

#[test]
fn default_document_is_the_canonical_template() {
    let document = ConfigDocument::from_source(ConfigDocument::default().source().unwrap()).unwrap();
    assert_eq!(document.to_string(), include_str!("../../../rift.default.toml"));
}

#[test]
fn scalar_edit_preserves_all_other_source_bytes_and_rolls_back_invalid_edits() {
    let original = include_str!("../../../rift.default.toml")
        .replace(
            "animation_duration = 0.3",
            "animation_duration = 0.3 # my timing",
        )
        .replace("\n[keys]\n", "\n# custom key documentation\n[keys]\n");
    let mut document = ConfigDocument::parse(&original).unwrap();
    let runtime = document.update(|source| source.settings.animation_duration = 0.2).unwrap();
    assert_eq!(runtime.settings.animation_duration, 0.2);
    assert_eq!(
        document.to_string(),
        original.replace("animation_duration = 0.3", "animation_duration = 0.2")
    );
    let saved = document.to_string();
    assert!(document.update(|source| source.settings.animation_duration = -1.0).is_err());
    assert_eq!(document.to_string(), saved);
}

#[test]
fn typed_collections_keep_source_aliases_and_inline_commands() {
    let mut document = ConfigDocument::default();
    document
        .update(|source| {
            source.virtual_workspaces.workspace_names = vec!["coding".into(), "mail".into()];
            source.settings.layout.gaps.per_display.insert("display-a".into(), GapOverride {
                inner: Some(InnerGaps { horizontal: 8.0, vertical: 8.0 }),
                outer: None,
            });
            source.virtual_workspaces.app_rules.push(AppWorkspaceRule {
                app_id: Some("com.example.Tool".into()),
                floating: true,
                position: Some(AppRulePosition { x: 0.4, y: 0.7 }),
                ..Default::default()
            });
            source.virtual_workspaces.workspace_rules.push(WorkspaceLayoutRule {
                workspace: WorkspaceSelector::Index(1),
                layout: LayoutMode::Scrolling,
            });
            source.keys.insert("comb1 + Q".into(), source.keys["Alt + H"].clone());
            source.binding_modes.insert(
                "nav".into(),
                std::collections::BTreeMap::from([("H".into(), source.keys["Alt + H"].clone())]),
            );
        })
        .unwrap();
    let text = document.to_string();
    assert!(text.contains("workspace_names = [\n\t\"coding\",\n\t\"mail\"\n]"));
    assert!(
        text.contains("\"comb1 + Q\" = { move_focus = \"left\" }"),
        "{text}"
    );
    assert!(text.contains("\"Alt + H\" = { move_focus = \"left\" }"));
    assert!(text.find("[keys]").unwrap() < text.find("[binding_modes.nav]").unwrap());
    assert!(
        text.find("[settings.layout.gaps.per_display.display-a.inner]").unwrap()
            < text.find("[settings.ui.mission_control]").unwrap()
    );
    let runtime = Config::parse(&text).unwrap();
    assert_eq!(runtime.virtual_workspaces.app_rules.len(), 1);
    assert_eq!(runtime.virtual_workspaces.workspace_rules.len(), 1);
    assert!(runtime.binding_mode_specs[0].1.iter().any(|(key, _)| key == "Alt + Shift + Q"));
    document
        .update(|source| {
            source.keys.remove("comb1 + Q");
            source.binding_modes.remove("nav");
            source.virtual_workspaces.app_rules.clear();
        })
        .unwrap();
    assert!(!document.to_string().contains("\"comb1 + Q\" ="));
}

#[test]
fn document_save_preserves_symlink_and_reloadable_source() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("nested/config.toml");
    let mut document = ConfigDocument::default();
    document.save(&target).unwrap();
    let link = dir.path().join("config.toml");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    document.update(|source| source.settings.animate = true).unwrap();
    document.save(&link).unwrap();
    assert!(link.is_symlink());
    assert_eq!(std::fs::read_to_string(&target).unwrap(), document.to_string());
    assert!(Config::read(&link).unwrap().settings.animate);
}

#[test]
fn commented_optional_setting_uses_example_anchor_and_resets_to_omission() {
    let mut document = ConfigDocument::default();
    document.update(|source| source.settings.default_disable = false).unwrap();
    let text = document.to_string();
    assert!(text.contains("#default_disable = false\ndefault_disable = false\n"));
    assert!(
        text.find("default_disable = false\n").unwrap() < text.find("focus_follows_mouse = true").unwrap()
    );
    document.update(|source| source.settings.default_disable = true).unwrap();
    assert_eq!(document.to_string(), include_str!("../../../rift.default.toml"));
}

#[test]
fn serde_without_binding_mode_specs_reconstructs_from_keys() {
    let cfg = Config::default();
    let mut json = serde_json::to_value(&cfg).unwrap();
    json.as_object_mut().unwrap().remove("binding_mode_specs");

    let round_tripped: Config = serde_json::from_value(json).unwrap();

    assert_eq!(
        round_tripped.binding_mode_specs[0].1.len(),
        round_tripped.keys.len()
    );
    assert!(!round_tripped.binding_mode_specs[0].1.is_empty());
}

#[test]
fn workspace_resize_cleans_removed_assignments_and_preserves_valid_rules() {
    let mut settings = VirtualWorkspaceSettings::default();
    settings.workspace_names = vec!["one".into(), "two".into(), "three".into()];
    settings.default_workspace = 2;
    settings.workspace_rules = vec![
        WorkspaceLayoutRule {
            workspace: WorkspaceSelector::Name("three".into()),
            layout: LayoutMode::Stack,
        },
        WorkspaceLayoutRule {
            workspace: WorkspaceSelector::Index(0),
            layout: LayoutMode::Bsp,
        },
    ];
    settings.app_rules = vec![AppWorkspaceRule {
        workspace: Some(WorkspaceSelector::Index(2)),
        ..Default::default()
    }];
    settings.resize(2);
    assert_eq!(settings.workspace_names, ["one", "two"]);
    assert_eq!(settings.default_workspace, 1);
    assert_eq!(settings.workspace_rules.len(), 1);
    assert_eq!(
        settings.workspace_rules[0].workspace,
        WorkspaceSelector::Index(0)
    );
    assert_eq!(settings.app_rules[0].workspace, None);
    settings.resize(MAX_WORKSPACES + 1);
    assert_eq!(settings.workspace_names, ["one", "two"]);
    assert!(!settings.validate().is_empty());
}

#[test]
fn keymap_rename_and_delete_update_both_command_encodings_and_self_references() {
    use rift_protocol::ReactorCommand;

    use crate::actor::wm_controller::WmCmd;
    let mut source = ConfigSource {
        settings: Config::default().settings,
        keys: Default::default(),
        binding_modes: Default::default(),
        virtual_workspaces: Default::default(),
        modifier_combinations: Default::default(),
    };
    source.create_keymap("edit".into()).unwrap();
    source.keys.insert("A".into(), WmCommand::Wm(WmCmd::BindingMode("edit".into())));
    source.keymap_mut("edit").unwrap().insert(
        "B".into(),
        WmCommand::ReactorCommand(reactor::Command::Reactor(ReactorCommand::BindingMode(
            "edit".into(),
        ))),
    );
    source.rename_keymap("edit", "renamed".into()).unwrap();
    assert!(source.keymap("edit").is_none());
    assert!(
        matches!(source.keys.get("A"), Some(WmCommand::Wm(WmCmd::BindingMode(name))) if name == "renamed")
    );
    assert!(
        matches!(source.keymap("renamed").unwrap().get("B"), Some(WmCommand::ReactorCommand(reactor::Command::Reactor(ReactorCommand::BindingMode(name)))) if name == "renamed")
    );
    source.keys.insert("C".into(), source.keymap("renamed").unwrap()["B"].clone());
    source.delete_keymap("renamed").unwrap();
    assert!(source.keymap("renamed").is_none());
    assert!(
        matches!(source.keys.get("A"), Some(WmCommand::Wm(WmCmd::BindingMode(name))) if name == "default")
    );
    assert!(
        matches!(source.keys.get("C"), Some(WmCommand::ReactorCommand(reactor::Command::Reactor(ReactorCommand::BindingMode(name)))) if name == "default")
    );
}
