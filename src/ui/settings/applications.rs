pub(super) struct Choice {
    pub name: String,
    pub target: (Option<String>, Option<String>),
    pub search: String,
}

pub(super) fn inventory(
    runtime: &[rift_protocol::ApplicationData],
    installed: &[(String, String)],
) -> Vec<Choice> {
    let mut choices: Vec<_> = runtime
        .iter()
        .filter(|app| app.window_count > 0)
        .map(|app| {
            (
                app.name.clone(),
                (
                    app.bundle_id.clone(),
                    app.bundle_id.is_none().then(|| app.name.clone()),
                ),
            )
        })
        .chain(installed.iter().map(|(name, id)| (name.clone(), (Some(id.clone()), None))))
        .collect();
    choices.sort_by_key(|(name, _)| name.to_lowercase());
    let mut seen = std::collections::BTreeSet::new();
    choices
        .into_iter()
        .filter(|(_, target)| seen.insert(target.clone()))
        .map(|(name, target)| Choice {
            search: format!("{} {}", name, target.0.as_deref().unwrap_or("")).to_lowercase(),
            name,
            target,
        })
        .collect()
}
