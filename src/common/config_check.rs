//! Offline validation for a Rift configuration file.
//!
//! This module intentionally only reads and deserializes the configuration. It
//! must remain safe to call before AppKit, Accessibility, WindowServer, or any
//! of Rift's runtime actors are initialized.

use std::path::Path;

use anyhow::{Context, Result, bail};

use super::config::Config;

/// Parse and semantically validate the configuration at `path` without
/// starting Rift's runtime.
pub fn check(path: &Path) -> Result<()> {
    let config = Config::read_offline(path).with_context(|| {
        format!("could not read or parse configuration file '{}'", path.display())
    })?;
    let issues = config.validate();

    if issues.is_empty() {
        return Ok(());
    }

    let details = issues.iter().map(|issue| format!("  - {issue}")).collect::<Vec<_>>().join("\n");
    bail!(
        "configuration validation failed for '{}':\n{details}",
        path.display()
    );
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::NamedTempFile;

    use super::*;

    fn config_file(contents: &str) -> NamedTempFile {
        let file = NamedTempFile::new().unwrap();
        fs::write(file.path(), contents).unwrap();
        file
    }

    #[test]
    fn accepts_a_valid_configuration_without_starting_runtime() {
        let file = config_file(include_str!("../../rift.default.toml"));

        check(file.path()).unwrap();
    }

    #[test]
    fn rejects_layout_specific_character_hotkeys_without_tis_lookup() {
        let contents =
            include_str!("../../rift.default.toml").replace("\"Alt + Z\"", "\"Alt + €\"");
        let file = config_file(&contents);
        let lookups_before = crate::sys::hotkey::keyboard_layout_lookup_count_for_test();

        let error = format!("{:#}", check(file.path()).unwrap_err());

        assert_eq!(
            crate::sys::hotkey::keyboard_layout_lookup_count_for_test(),
            lookups_before
        );
        assert!(error.contains("Cannot resolve character key '€' offline"));
        assert!(error.contains("active keyboard layout"));
    }

    #[test]
    fn rejects_invalid_named_hotkeys_offline() {
        let contents =
            include_str!("../../rift.default.toml").replace("\"Alt + Z\"", "\"Alt + Banana\"");
        let file = config_file(&contents);

        let error = format!("{:#}", check(file.path()).unwrap_err());

        assert!(error.contains("Could not parse hotkey: Alt + Banana"));
    }

    #[test]
    fn keeps_focus_disable_hotkeys_in_the_offline_scope() {
        let contents = include_str!("../../rift.default.toml").replace(
            "focus_follows_mouse = true",
            "focus_follows_mouse = true\nfocus_follows_mouse_disable_hotkey = \"Alt + Comma\"",
        );
        let file = config_file(&contents);
        let lookups_before = crate::sys::hotkey::keyboard_layout_lookup_count_for_test();

        check(file.path()).unwrap();

        assert_eq!(
            crate::sys::hotkey::keyboard_layout_lookup_count_for_test(),
            lookups_before
        );
    }

    #[test]
    fn reports_missing_files_clearly() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("missing.toml");

        let error = format!("{:#}", check(&path).unwrap_err());

        assert!(error.contains("could not read or parse configuration file"));
        assert!(error.contains("missing.toml"));
    }

    #[test]
    fn reports_toml_parse_failures() {
        let file = config_file("[settings\n");

        let error = format!("{:#}", check(file.path()).unwrap_err());

        assert!(error.contains("could not read or parse configuration file"));
        assert!(error.contains("TOML parse error"));
    }

    #[test]
    fn reports_semantic_validation_failures() {
        let contents = include_str!("../../rift.default.toml")
            .replace("animation_duration = 0.3", "animation_duration = -1.0");
        let file = config_file(&contents);

        let error = format!("{:#}", check(file.path()).unwrap_err());

        assert!(error.contains("configuration validation failed"));
        assert!(error.contains("animation_duration must be non-negative"));
    }
}
