use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

use rift_protocol::ApplicationData;

/// A rule's application matcher: bundle identifier, or name for apps without one.
pub(super) type AppMatch = (Option<String>, Option<String>);

#[derive(PartialEq)]
pub(super) struct Choice {
    pub name: String,
    pub target: AppMatch,
    pub(super) search: String,
}

/// Running and installed applications, sorted by name and shared with open pickers.
#[derive(Default)]
pub(super) struct Inventory {
    pub choices: Vec<Rc<Choice>>,
    by_bundle: HashMap<String, Rc<Choice>>,
}

/// Applications with windows; only these affect the inventory.
pub(super) fn running(apps: &[ApplicationData]) -> impl Iterator<Item = (&str, Option<&str>)> {
    apps.iter()
        .filter(|app| app.window_count > 0)
        .map(|app| (app.name.as_str(), app.bundle_id.as_deref()))
}

impl Inventory {
    pub fn new(runtime: &[ApplicationData], installed: &[(String, String)]) -> Self {
        let mut choices: Vec<_> = running(runtime)
            .map(|(name, id)| {
                let target = (id.map(str::to_owned), id.is_none().then(|| name.to_owned()));
                (name.to_owned(), target)
            })
            .chain(installed.iter().map(|(name, id)| (name.clone(), (Some(id.clone()), None))))
            .collect();
        choices.sort_by_cached_key(|(name, _)| name.to_lowercase());
        let mut seen = BTreeSet::new();
        let choices: Vec<_> = choices
            .into_iter()
            .filter(|(_, target)| seen.insert(target.clone()))
            .map(|(name, target)| {
                Rc::new(Choice {
                    search: format!("{} {}", name, target.0.as_deref().unwrap_or(""))
                        .to_lowercase(),
                    name,
                    target,
                })
            })
            .collect();
        let by_bundle = choices
            .iter()
            .filter_map(|choice| Some((choice.target.0.clone()?, choice.clone())))
            .collect();
        Self { choices, by_bundle }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize { self.choices.len() }

    pub fn name_for_bundle(&self, bundle_id: &str) -> Option<&str> {
        self.by_bundle.get(bundle_id).map(|choice| choice.name.as_str())
    }

    /// Choices whose name or bundle identifier contains the lowercase `query`.
    pub fn matching(&self, query: &str) -> Vec<Rc<Choice>> {
        self.choices
            .iter()
            .filter(|choice| choice.search.contains(query))
            .cloned()
            .collect()
    }

    /// How a matcher is shown: its application's name, else the raw matcher.
    pub fn display_name(&self, target: &AppMatch) -> String {
        target
            .0
            .as_ref()
            .and_then(|id| self.by_bundle.get(id))
            .filter(|choice| choice.target == *target)
            .or_else(|| self.choices.iter().find(|choice| choice.target == *target))
            .map(|choice| choice.name.clone())
            .or_else(|| target.1.clone())
            .or_else(|| target.0.clone())
            .unwrap_or_else(|| "Any application".into())
    }
}
