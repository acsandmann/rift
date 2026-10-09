use std::io::Write;
use std::path::Path;

use serde::Serialize;
use toml_edit::{DocumentMut, Item, Table, TableLike};

use super::parse::parse_config_file;
use super::{Config, ConfigSource};

/// Editable source. Runtime normalization never changes this document.
#[derive(Clone)]
pub struct ConfigDocument {
    document: DocumentMut,
}

impl Default for ConfigDocument {
    fn default() -> Self {
        Self {
            document: include_str!("../../../rift.default.toml").parse().unwrap(),
        }
    }
}

impl std::fmt::Display for ConfigDocument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { self.document.fmt(f) }
}

/// A validated edit that has not been committed yet.
pub struct Edited {
    pub document: ConfigDocument,
    pub config: Config,
    pub source: ConfigSource,
}

impl ConfigDocument {
    pub fn parse(text: &str) -> anyhow::Result<Self> { Ok(Self::load(text)?.0) }

    /// Parse and validate once, returning the runtime config alongside the document.
    pub fn load(text: &str) -> anyhow::Result<(Self, Config)> {
        let config = Config::parse(text)?;
        Ok((Self { document: text.parse()? }, config))
    }

    pub fn read(path: &Path) -> anyhow::Result<Self> {
        Self::parse(&std::fs::read_to_string(path)?)
    }

    pub fn source(&self) -> anyhow::Result<ConfigSource> {
        Ok(parse_config_file(&self.to_string())?)
    }

    pub fn runtime(&self) -> anyhow::Result<Config> { Config::parse(&self.to_string()) }

    /// Generate from typed source using the handwritten default as the style template.
    #[cfg(test)]
    pub fn from_source(source: ConfigSource) -> anyhow::Result<Self> {
        let mut document = Self::default();
        document.update(|current| *current = source)?;
        Ok(document)
    }

    /// Transactional typed edits, including map insertion/removal and ordered rule lists.
    /// Only semantic differences are patched; omitted defaults remain omitted.
    pub fn update(&mut self, edit: impl FnOnce(&mut ConfigSource)) -> anyhow::Result<Config> {
        let edited = self.edited(|source| {
            edit(source);
            Ok(())
        })?;
        *self = edited.document;
        Ok(edited.config)
    }

    /// Apply and validate `edit` against a copy, leaving `self` untouched.
    pub fn edited(
        &self,
        edit: impl FnOnce(&mut ConfigSource) -> anyhow::Result<()>,
    ) -> anyhow::Result<Edited> {
        let mut source = self.source()?;
        let before = toml::Value::try_from(&source)?;
        edit(&mut source)?;
        let after = toml::Value::try_from(&source)?;
        let mut document = self.clone();
        patch(
            document.document.as_table_mut(),
            before.as_table().unwrap(),
            after.as_table().unwrap(),
            "",
            None,
        )?;
        let config = document.validated()?;
        Ok(Edited { document, config, source })
    }

    fn validated(&self) -> anyhow::Result<Config> {
        let config = self.runtime()?;
        let issues = config.validate();
        anyhow::ensure!(issues.is_empty(), "{}", issues.join("; "));
        Ok(config)
    }

    /// Adapter for the existing CLI's dotted setting keys. UI callers use `update`.
    pub(crate) fn set(&mut self, key: &str, value: &serde_json::Value) -> anyhow::Result<Config> {
        let mut candidate = self.clone();
        let mut parts = key.split('.').peekable();
        let mut table: &mut dyn TableLike = candidate.document.as_table_mut();
        while let Some(part) = parts.next() {
            anyhow::ensure!(!part.is_empty(), "Empty config path component");
            if parts.peek().is_none() {
                if value.is_null() {
                    remove_value(table, part);
                } else {
                    let mut new = value.serialize(toml_edit::ser::ValueSerializer::new())?;
                    if let Some(old) = table.get(part).and_then(Item::as_value) {
                        *new.decor_mut() = old.decor().clone();
                    }
                    insert_value(table, part, new);
                }
                break;
            }
            if !table.contains_key(part) {
                table.insert(part, Item::Table(Table::new()));
            }
            table = table
                .get_mut(part)
                .and_then(Item::as_table_like_mut)
                .ok_or_else(|| anyhow::anyhow!("Invalid config path: {key}"))?;
        }
        let runtime = candidate.validated()?;
        *self = candidate;
        Ok(runtime)
    }

    /// Atomic replacement in the target directory, including symlink targets.
    /// Documents are validated when created or edited, so saving does not reparse.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let parent =
            target.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        if let Ok(metadata) = std::fs::metadata(&target) {
            file.as_file().set_permissions(metadata.permissions())?;
        }
        file.write_all(self.to_string().as_bytes())?;
        file.as_file().sync_all()?;
        file.persist(target)?;
        Ok(())
    }
}

fn patch(
    table: &mut dyn TableLike,
    before: &toml::map::Map<String, toml::Value>,
    after: &toml::map::Map<String, toml::Value>,
    path: &str,
    position: Option<isize>,
) -> anyhow::Result<()> {
    for key in before.keys().chain(after.keys().filter(|key| !before.contains_key(*key))) {
        if before.get(key) == after.get(key) {
            continue;
        }
        // Supported legacy spelling must not leave two competing values behind.
        let actual = if key == "prevent_wrapping" && table.contains_key("prevent_wrapping_around") {
            "prevent_wrapping_around"
        } else {
            key
        };
        let Some(new) = after.get(key) else {
            remove_value(table, actual);
            continue;
        };
        if (path == "settings" && key == "default_disable" && new.as_bool() == Some(true))
            || (path == "virtual_workspaces"
                && key == "default_workspace"
                && new.as_integer() == Some(0))
        {
            remove_value(table, actual);
            continue;
        }
        if let toml::Value::Table(new_table) = new
            && path != "keys"
            && !(path.starts_with("binding_modes."))
        {
            if table.get(actual).is_none() {
                let mut child = Table::new();
                if let Some(position) = table
                    .iter()
                    .filter_map(|(_, item)| item.as_table()?.position())
                    .max()
                    .or(position)
                {
                    child.set_position(position);
                }
                table.insert(actual, Item::Table(child));
            }
            let child_position = table
                .get(actual)
                .and_then(Item::as_table)
                .and_then(Table::position)
                .or(position);
            if let Some(child) = table.get_mut(actual).and_then(Item::as_table_like_mut) {
                let empty = toml::map::Map::new();
                patch(
                    child,
                    before.get(key).and_then(toml::Value::as_table).unwrap_or(&empty),
                    new_table,
                    &if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    },
                    child_position,
                )?;
                continue;
            }
        }
        let mut value = new.serialize(toml_edit::ser::ValueSerializer::new())?;
        if let Some(array) = value.as_array_mut() {
            let old_array = table.get(actual).and_then(Item::as_array);
            let old_values = before.get(key).and_then(toml::Value::as_array);
            let new_values = new.as_array().unwrap();
            for (index, element) in array.iter_mut().enumerate() {
                if let Some(old_index) = old_values
                    .and_then(|values| values.iter().position(|old| old == &new_values[index]))
                    && let Some(old) = old_array.and_then(|array| array.get(old_index))
                {
                    *element = old.clone();
                } else if matches!(
                    key.as_str(),
                    "workspace_names" | "app_rules" | "workspace_rules"
                ) {
                    element.decor_mut().set_prefix("\n\t");
                }
            }
            if matches!(key.as_str(), "workspace_names" | "app_rules" | "workspace_rules")
                && !array.is_empty()
            {
                array.set_trailing("\n");
            }
        }
        if let Some(old) = table.get(actual).and_then(Item::as_value) {
            *value.decor_mut() = old.decor().clone();
        } else {
            value.decor_mut().set_prefix(" ");
        }
        insert_value(table, actual, value);
    }
    Ok(())
}

// Commented examples provide insertion anchors, never semantic values.
fn insert_value(table: &mut dyn TableLike, key: &str, value: toml_edit::Value) {
    if let Some(existing) = table.get_mut(key) {
        *existing = Item::Value(value);
        return;
    }
    let anchor = table.iter().find_map(|(name, _)| {
        let prefix = table.key(name)?.leaf_decor().prefix()?.as_str()?;
        let mut end = 0;
        for line in prefix.split_inclusive('\n') {
            end += line.len();
            if line.trim_start().strip_prefix('#').is_some_and(|comment| {
                comment
                    .trim_start()
                    .strip_prefix(key)
                    .is_some_and(|rest| rest.trim_start().starts_with('='))
            }) {
                return Some((
                    name.to_string(),
                    prefix[..end].to_string(),
                    prefix[end..].to_string(),
                ));
            }
        }
        None
    });
    if let Some((anchor, leading, remaining)) = anchor {
        let tail: Vec<_> = table
            .iter()
            .skip_while(|(name, _)| *name != anchor)
            .map(|(name, item)| (table.key(name).unwrap().clone(), item.clone()))
            .collect();
        for (key, _) in &tail {
            table.remove(key.get());
        }
        table.insert(key, Item::Value(value));
        table.key_mut(key).unwrap().leaf_decor_mut().set_prefix(leading);
        for (mut old_key, item) in tail {
            if old_key.get() == anchor {
                old_key.leaf_decor_mut().set_prefix(remaining.clone());
            }
            table.entry_format(&old_key).or_insert(item);
        }
        return;
    }
    table.insert(key, Item::Value(value));
}

fn remove_value(table: &mut dyn TableLike, key: &str) {
    let prefix = table
        .key(key)
        .and_then(|key| key.leaf_decor().prefix())
        .and_then(|prefix| prefix.as_str())
        .unwrap_or("")
        .to_string();
    let next = table
        .iter()
        .skip_while(|(name, _)| *name != key)
        .nth(1)
        .map(|(name, _)| name.to_string());
    table.remove(key);
    if let Some(next) = next
        && !prefix.is_empty()
    {
        let mut key = table.key_mut(&next).unwrap();
        let suffix = key.leaf_decor().prefix().and_then(|prefix| prefix.as_str()).unwrap_or("");
        let combined = format!("{prefix}{suffix}");
        key.leaf_decor_mut().set_prefix(combined);
    }
}
