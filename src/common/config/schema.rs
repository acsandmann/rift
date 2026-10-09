//! Compile-time Settings metadata. Doc comments remain the source of help text;
//! config serialization and validation stay independent of UI generation.
pub use rift_config_derive::{ConfigEnum, ConfigSchema};

/// Native popup label and its documentation; variant identity is the typed index.
pub type EnumChoice = (&'static str, &'static str);
#[derive(Clone, Copy)]
pub enum FieldKind {
    Bool,
    Number {
        integer: bool,
    },
    Text,
    Choice(&'static [EnumChoice]),
    /// Specialized UI, such as inherited values, rules, or visual editors.
    Custom,
}
#[derive(Clone, PartialEq)]
pub enum FieldValue {
    Bool(bool),
    Number(f64),
    Text(String),
    Choice(usize),
}
pub struct ConfigField<T> {
    pub key: &'static str,
    pub title: &'static str,
    pub help: &'static str,
    pub group: &'static str,
    pub aliases: &'static str,
    pub unit: &'static str,
    pub scale: f64,
    pub kind: FieldKind,
    pub enabled: Option<fn(&T) -> bool>,
    pub read: Option<fn(&T) -> FieldValue>,
    pub write: Option<fn(&mut T, FieldValue) -> Result<(), String>>,
}
pub trait ConfigSchema: Sized + 'static {
    fn fields() -> &'static [ConfigField<Self>];
    fn field(key: &str) -> Option<&'static ConfigField<Self>> {
        Self::fields().iter().find(|field| field.key == key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(ConfigSchema)]
    #[setting(group = "default")]
    struct Example {
        /// First paragraph.
        ///
        /// Second paragraph.
        #[setting(label = "Motion", group = "behavior")]
        animate: bool,
        #[setting(enabled_by = "animate", unit = "pt", scale = 100.0, order = 0)]
        window_count: usize,
        #[setting(ignore)]
        internal: Vec<String>,
        /// Uses a dedicated editor.
        #[setting(custom)]
        rules: Vec<String>,
    }

    #[test]
    fn schema_exposes_docs_and_typed_edits_without_exposing_ignored_fields() {
        let mut value = Example {
            animate: false,
            window_count: 1,
            internal: vec!["private".into()],
            rules: vec![],
        };
        assert_eq!(Example::fields()[0].key, "window_count");
        let motion = Example::field("animate").unwrap();
        assert_eq!(
            (motion.title, motion.group, motion.help),
            ("Motion", "behavior", "First paragraph.\n\nSecond paragraph.")
        );
        assert!(Example::field("internal").is_none());
        let custom = Example::field("rules").unwrap();
        assert!(custom.read.is_none() && custom.write.is_none());
        let count = Example::field("window_count").unwrap();
        assert_eq!(count.title, "Window count");
        assert_eq!(count.group, "default");
        assert_eq!((count.unit, count.scale), ("pt", 100.0));
        assert!(!count.enabled.unwrap()(&value));
        motion.write.unwrap()(&mut value, FieldValue::Bool(true)).unwrap();
        assert!(count.enabled.unwrap()(&value));
        count.write.unwrap()(&mut value, FieldValue::Number(3.0)).unwrap();
        assert_eq!(value.window_count, 3);
        for bad in [-1.0, 1.5, f64::NAN, f64::INFINITY] {
            assert!(count.write.unwrap()(&mut value, FieldValue::Number(bad)).is_err());
            assert_eq!(
                value.window_count, 3,
                "invalid writes leave the config untouched"
            );
        }
        assert!(motion.write.unwrap()(&mut value, FieldValue::Text("true".into())).is_err());
        assert_eq!(value.internal, ["private"]);
        assert!(value.rules.is_empty());
    }

    #[test]
    fn generated_enum_edits_preserve_existing_config_serialization() {
        use crate::common::config::{ActiveWorkspaceLabel, MenuBarSettings};
        let mut config = MenuBarSettings::default();
        let field = MenuBarSettings::field("active_label").unwrap();
        let FieldKind::Choice(choices) = field.kind else {
            panic!("expected native popup choices")
        };
        let index = choices.iter().position(|choice| choice.0 == "Name").unwrap();
        field.write.unwrap()(&mut config, FieldValue::Choice(index)).unwrap();
        assert_eq!(config.active_label, ActiveWorkspaceLabel::Name);
        assert_eq!(serde_json::to_value(&config).unwrap()["active_label"], "name");
        assert!(field.write.unwrap()(&mut config, FieldValue::Choice(choices.len())).is_err());
        assert_eq!(config.active_label, ActiveWorkspaceLabel::Name);
    }
}
