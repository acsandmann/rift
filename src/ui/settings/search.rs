//! Searchable settings destinations, without constructing hidden page trees.
use super::CATEGORIES;
use super::pages::LAYOUT_PAGES;

/// A destination with its search text precomputed once.
pub(super) struct Entry {
    pub title: &'static str,
    pub location: String,
    pub page: usize,
    pub scope: Option<usize>,
    pub description: String,
    name: String,
    context: String,
    haystack: String,
}

#[derive(Clone, Copy)]
pub(super) enum Row {
    Heading(&'static str),
    Setting(&'static Entry),
}

impl Entry {
    pub fn page_id(&self) -> usize { self.scope.map_or(self.page, super::scope_page) }

    fn score(&self, query: &str, terms: &[&str]) -> usize {
        let exact = if self.name == query {
            1000
        } else if self.name.starts_with(query) {
            600
        } else if self.name.contains(query) {
            400
        } else {
            0
        };
        exact
            + terms
                .iter()
                .map(|term| {
                    if self.name.contains(term) {
                        60
                    } else if self.context.contains(term) {
                        20
                    } else {
                        5
                    }
                })
                .sum::<usize>()
    }
}

pub(super) fn grouped(results: &[&'static Entry]) -> Vec<Row> {
    let mut rows = Vec::new();
    for (page, (heading, _)) in CATEGORIES.iter().enumerate() {
        let mut matches = results.iter().filter(|entry| entry.page == page).peekable();
        if matches.peek().is_some() {
            rows.push(Row::Heading(heading));
            rows.extend(matches.map(|entry| Row::Setting(entry)));
        }
    }
    rows
}

struct Draft {
    title: &'static str,
    location: String,
    page: usize,
    scope: Option<usize>,
    key: &'static str,
    help: &'static str,
    aliases: String,
}

impl Draft {
    fn finish(self) -> Entry {
        let description = describe(&self);
        let name = self.title.to_lowercase();
        let context = self.location.to_lowercase();
        let haystack =
            format!("{name} {context} {} {} {}", self.aliases, self.key, self.help).to_lowercase();
        Entry {
            title: self.title,
            location: self.location,
            page: self.page,
            scope: self.scope,
            description,
            name,
            context,
            haystack,
        }
    }
}

fn describe(entry: &Draft) -> String {
    if !entry.help.is_empty() {
        return entry.help.lines().map(str::trim).collect::<Vec<_>>().join(" ");
    }
    let text = match entry.title {
        "Screen edges" => "Set the space between windows and screen edges.",
        "Virtual Workspaces" => "Organize your windows into separate workspaces.",
        "Workspace swipes" => "Switch workspaces using trackpad gestures.",
        "New window position" => "Choose where newly opened windows appear.",
        "Default layout" => "Choose the layout used by default.",
        "App rules" => "Choose how Rift handles an application's windows.",
        "Workspace rules" => "Assign matching windows to a workspace.",
        "Keyboard shortcuts" => "View, add, and edit keyboard shortcuts.",
        "Display settings" | "Displays" => "Customize spacing for a connected display.",
        "Workspaces" => "Add workspaces and customize their names and layouts.",
        _ => return entry.location.clone(),
    };
    if entry.scope.is_some_and(|scope| (1..=6).contains(&scope)) {
        let layout = entry.location.strip_prefix("Layouts › ").unwrap_or(&entry.location);
        format!("{layout} · {text}")
    } else {
        text.into()
    }
}

/// Destinations without schema fields: pages, sections, and handcrafted editors.
const MANUAL: &[(usize, Option<usize>, &str, &str, &str)] = &[
    (
        1,
        Some(7),
        "Layouts › Spacing",
        "Screen edges",
        "gaps gap margin padding outer edges spacing",
    ),
    (
        1,
        Some(7),
        "Layouts › Spacing",
        "Set each edge separately",
        "gaps gap top bottom left right margin padding outer",
    ),
    (
        1,
        Some(7),
        "Layouts › Spacing",
        "Horizontal",
        "gaps gap between windows inner horizontal spacing",
    ),
    (
        1,
        Some(7),
        "Layouts › Spacing",
        "Vertical",
        "gaps gap between windows inner vertical spacing",
    ),
    (
        1,
        Some(8),
        "Layouts › Displays",
        "Display settings",
        "monitor screen connected per display override gaps spacing widths",
    ),
    (
        2,
        None,
        "Workspaces",
        "Virtual Workspaces",
        "desktops spaces enabled",
    ),
    (
        3,
        None,
        "Rules",
        "App rules",
        "application bundle identifier manage ignore floating window title match workspace assign desktop",
    ),
    (
        4,
        None,
        "Keyboard",
        "Keyboard shortcuts",
        "keys hotkeys keybindings bindings commands",
    ),
    (
        4,
        None,
        "Keyboard",
        "Shortcut set",
        "keymap keybinding mode default",
    ),
    (
        4,
        None,
        "Keyboard",
        "Reusable modifier combinations",
        "command option control shift modifiers",
    ),
    (
        5,
        None,
        "Mouse & Trackpad",
        "Workspace swipes",
        "trackpad gestures fingers swipe sensitivity haptic",
    ),
    (
        5,
        None,
        "Mouse & Trackpad",
        "Scrolling layout gestures",
        "trackpad gestures fingers swipe scrolling niri",
    ),
    (
        5,
        None,
        "Mouse & Trackpad",
        "Drag & Drop",
        "mouse modifier move resize swap stack drop",
    ),
    (
        5,
        None,
        "Mouse & Trackpad",
        "Pointer movement",
        "mouse cursor movement",
    ),
    (
        6,
        None,
        "Interface",
        "Overview",
        "window previews fade transitions",
    ),
    (
        6,
        None,
        "Interface",
        "Stack Line",
        "stackline indicator color position thickness interaction",
    ),
    (7, None, "Advanced", "Startup commands", "launch run shell exec"),
    (
        7,
        None,
        "Advanced",
        "Autofocus blacklist",
        "focus exclude ignore app",
    ),
    (
        7,
        None,
        "Advanced",
        "Reload config when edited externally",
        "configuration file automatic reload toml",
    ),
    (
        7,
        None,
        "Advanced",
        "Configuration file",
        "open config path toml reload",
    ),
    (
        8,
        None,
        "About",
        "Check for Updates…",
        "version latest release update",
    ),
    (8, None, "About", "Documentation", "docs help guide manual"),
    (8, None, "About", "Release Notes", "changelog version changes"),
    (8, None, "About", "Sponsor Rift", "donate support sponsorship"),
];

// Schema registration supplies navigation context, not duplicate field lists.
// It is initialized once without constructing any hidden Settings views.
fn catalog() -> &'static [Entry] {
    use crate::common::config::*;
    static CATALOG: std::sync::OnceLock<Vec<Entry>> = std::sync::OnceLock::new();
    CATALOG.get_or_init(|| {
        fn append<T: ConfigSchema>(
            out: &mut Vec<Draft>,
            group: &str,
            page: usize,
            scope: Option<usize>,
            location: &str,
        ) {
            for field in T::fields().iter().filter(|field| field.group == group) {
                out.push(Draft {
                    title: field.title.split(" (").next().unwrap_or(field.title),
                    location: location.to_owned(),
                    page,
                    scope,
                    key: field.key,
                    help: field.help,
                    aliases: field.aliases.to_owned(),
                });
            }
        }
        let mut out = Vec::new();
        append::<Settings>(&mut out, "general", 0, None, "General › Window behavior");
        append::<Settings>(&mut out, "advanced", 7, None, "Advanced");
        append::<Settings>(&mut out, "pointer", 5, None, "Mouse & Trackpad › Focus");
        append::<VirtualWorkspaceSettings>(&mut out, "", 2, None, "Workspaces");
        append::<GestureSettings>(&mut out, "main", 5, None, "Mouse & Trackpad › Workspace swipes");
        append::<GestureSettings>(
            &mut out,
            "advanced",
            5,
            None,
            "Mouse & Trackpad › Advanced swipe settings",
        );
        append::<ScrollingGestureSettings>(
            &mut out,
            "",
            5,
            None,
            "Mouse & Trackpad › Scrolling layout gestures",
        );
        append::<DragDropSettings>(&mut out, "", 5, None, "Mouse & Trackpad › Drag & Drop");
        append::<MenuBarSettings>(&mut out, "", 6, None, "Interface › Menu Bar");
        append::<MissionControlSettings>(&mut out, "", 6, None, "Interface › Overview");
        append::<StackLineSettings>(&mut out, "", 6, None, "Interface › Stack Line");
        append::<LayoutSettings>(&mut out, "", 1, Some(0), "Layouts › Default behavior");
        for (scope, (title, _)) in LAYOUT_PAGES[..7].iter().enumerate() {
            append::<BaseLayoutSettings>(
                &mut out,
                "",
                1,
                Some(scope),
                &format!("Layouts › {title}"),
            );
        }
        append::<TraditionalLayoutSettings>(&mut out, "", 1, Some(1), "Layouts › Traditional");
        append::<BspLayoutSettings>(&mut out, "", 1, Some(2), "Layouts › BSP");
        append::<StackSettings>(&mut out, "", 1, Some(3), "Layouts › Stack");
        append::<MasterStackSettings>(&mut out, "", 1, Some(4), "Layouts › Master Stack");
        append::<ScrollingLayoutSettings>(&mut out, "", 1, Some(5), "Layouts › Scrolling");
        append::<OuterGaps>(&mut out, "", 1, Some(7), "Layouts › Spacing");
        append::<InnerGaps>(&mut out, "", 1, Some(7), "Layouts › Spacing");

        let pages = CATEGORIES
            .iter()
            .enumerate()
            .map(|(page, (title, _))| (page, None, "Settings", *title, ""));
        let scopes = LAYOUT_PAGES
            .iter()
            .enumerate()
            .map(|(scope, (title, _))| (1, Some(scope), "Layouts", *title, "layout"));
        for (page, scope, location, title, aliases) in
            pages.chain(scopes).chain(MANUAL.iter().copied())
        {
            // A destination that names a documented field enriches it instead of repeating it.
            let fields: Vec<_> = out
                .iter()
                .enumerate()
                .filter(|(_, field)| {
                    field.title == title && field.page == page && field.scope == scope
                })
                .map(|(index, _)| index)
                .collect();
            let documented = fields
                .iter()
                .copied()
                .find(|&index| out[index].location == location)
                .or_else(|| (fields.len() == 1).then(|| fields[0]));
            match documented {
                Some(index) => {
                    let field = &mut out[index];
                    field.aliases.push(' ');
                    field.aliases.push_str(aliases);
                }
                None if fields.is_empty() => out.push(Draft {
                    title,
                    location: location.to_owned(),
                    page,
                    scope,
                    key: "",
                    help: "",
                    aliases: aliases.to_owned(),
                }),
                None => {}
            }
        }
        out.into_iter().map(Draft::finish).collect()
    })
}

pub(super) fn results(query: &str) -> Vec<&'static Entry> {
    let query = query.trim().to_lowercase();
    let terms: Vec<_> = query.split_whitespace().collect();
    if terms.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<_> = catalog()
        .iter()
        .filter(|entry| terms.iter().all(|term| entry.haystack.contains(term)))
        .map(|entry| (entry.score(&query, &terms), entry))
        .collect();
    hits.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(a.1.title.cmp(b.1.title))
            .then(a.1.location.cmp(&b.1.location))
    });
    hits.into_iter().map(|(_, entry)| entry).collect()
}

/// Scope repeated names such as “Enabled” to their documented section.
pub(super) fn section(location: &str) -> &str {
    location.rsplit(" › ").next().unwrap_or(location)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn search_finds_settings_and_their_scopes() {
        for (query, title, page, scope) in [
            ("column width", "Default column width", 1, Some(5)),
            (" FFM ", "Focus follows pointer", 5, None),
            ("stack offset", "Window offset", 1, Some(3)),
        ] {
            let found = results(query);
            let first = found.first().expect("find a setting, not just its section");
            assert_eq!((first.title, first.page, first.scope), (title, page, scope));
        }
        assert!(results("gaps").iter().any(|result| result.title == "Screen edges"));
        assert!(
            results("column width unrelatedword").is_empty(),
            "match every query word"
        );
        assert!(results("   ").is_empty());
    }

    #[test]
    fn catalog_lists_each_destination_once() {
        let mut seen = std::collections::HashSet::new();
        for entry in catalog() {
            assert!(
                seen.insert((entry.title, &entry.location, entry.page, entry.scope)),
                "duplicate destination {} in {}",
                entry.title,
                entry.location
            );
        }
    }
}
