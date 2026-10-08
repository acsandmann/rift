//! Searchable settings destinations, without constructing hidden page trees.
#[derive(Clone)]
pub(super) struct Result {
    pub title: &'static str,
    pub location: &'static str,
    pub page: usize,
    pub scope: Option<usize>,
    help: &'static str,
}

#[derive(Clone)]
pub(super) enum Row {
    Heading(&'static str),
    Setting(Result),
}
pub(super) fn grouped(results: &[Result]) -> Vec<Row> {
    let mut rows = Vec::new();
    for page in 0..9 {
        let matches: Vec<_> = results.iter().filter(|result| result.page == page).collect();
        if matches.is_empty() {
            continue;
        }
        rows.push(Row::Heading(
            [
                "General",
                "Layouts",
                "Workspaces",
                "Rules",
                "Keyboard",
                "Mouse & Trackpad",
                "Interface",
                "Advanced",
                "About",
            ][page],
        ));
        rows.extend(matches.into_iter().cloned().map(Row::Setting));
    }
    rows
}
struct Destination {
    title: &'static str,
    key: &'static str,
    aliases: &'static str,
    help: &'static str,
    page: usize,
    scope: Option<usize>,
    location: &'static str,
}
// Schema registration supplies navigation context, not duplicate field lists.
// It is initialized once without constructing any hidden Settings views.
fn catalog() -> &'static [Destination] {
    use crate::common::config::*;
    static CATALOG: std::sync::OnceLock<Vec<Destination>> = std::sync::OnceLock::new();
    CATALOG.get_or_init(|| {
        fn append<T: ConfigSchema>(
            out: &mut Vec<Destination>,
            group: &str,
            page: usize,
            scope: Option<usize>,
            location: &'static str,
        ) {
            for field in T::fields().iter().filter(|field| field.group == group) {
                out.push(Destination {
                    title: field.title.split(" (").next().unwrap_or(field.title),
                    key: field.key,
                    aliases: field.aliases,
                    help: field.help,
                    page,
                    scope,
                    location,
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
        for (scope, location) in [
            "Layouts › Default behavior",
            "Layouts › Traditional",
            "Layouts › BSP",
            "Layouts › Stack",
            "Layouts › Master Stack",
            "Layouts › Scrolling",
            "Layouts › Floating",
        ]
        .into_iter()
        .enumerate()
        {
            append::<BaseLayoutSettings>(&mut out, "", 1, Some(scope), location);
        }
        append::<TraditionalLayoutSettings>(&mut out, "", 1, Some(1), "Layouts › Traditional");
        append::<BspLayoutSettings>(&mut out, "", 1, Some(2), "Layouts › BSP");
        append::<StackSettings>(&mut out, "", 1, Some(3), "Layouts › Stack");
        append::<MasterStackSettings>(&mut out, "", 1, Some(4), "Layouts › Master Stack");
        append::<ScrollingLayoutSettings>(&mut out, "", 1, Some(5), "Layouts › Scrolling");
        append::<OuterGaps>(&mut out, "", 1, Some(7), "Layouts › Spacing");
        append::<InnerGaps>(&mut out, "", 1, Some(7), "Layouts › Spacing");
        out
    })
}

impl Result {
    pub fn description(&self) -> String {
        if !self.help.is_empty() {
            return self.help.lines().map(str::trim).collect::<Vec<_>>().join(" ");
        }
        let text = match self.title {
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
            _ => return self.location.to_string(),
        };
        if self.scope.is_some_and(|scope| (1..=6).contains(&scope)) {
            format!(
                "{} · {text}",
                self.location.strip_prefix("Layouts › ").unwrap_or(self.location)
            )
        } else {
            text.into()
        }
    }
}

pub(super) fn results(query: &str) -> Vec<Result> {
    let query = query.trim().to_lowercase();
    let terms: Vec<_> = query.split_whitespace().collect();
    if terms.is_empty() {
        return Vec::new();
    }
    let mut matches = Vec::new();
    let mut add = |page, scope, location, title, aliases| {
        let title: &'static str = title;
        let mut location: &'static str = location;
        let aliases: &'static str = aliases;
        let fields: Vec<_> = catalog()
            .iter()
            .filter(|field| field.title == title && field.page == page && field.scope == scope)
            .collect();
        let documented = fields
            .iter()
            .copied()
            .find(|field| field.location == location)
            .or_else(|| (fields.len() == 1).then(|| fields[0]));
        if documented.is_none() && fields.len() > 1 {
            return;
        }
        if let Some(field) = documented {
            location = field.location;
        }
        let help = documented.map_or("", |field| field.help);
        let name = title.to_lowercase();
        let context = location.to_lowercase();
        let key = documented.map_or("", |field| field.key);
        let schema_aliases = documented.map_or("", |field| field.aliases);
        let words = format!(
            "{name} {context} {aliases} {key} {schema_aliases} {}",
            help.to_lowercase()
        );
        if !terms.iter().all(|term| words.contains(term)) {
            return;
        }
        let score = if name == query {
            1000
        } else if name.starts_with(&query) {
            600
        } else if name.contains(&query) {
            400
        } else {
            0
        } + terms
            .iter()
            .map(|term| {
                if name.contains(term) {
                    60
                } else if context.contains(term) {
                    20
                } else {
                    5
                }
            })
            .sum::<usize>();
        matches.push((score, Result {
            title,
            location,
            page,
            scope,
            help,
        }));
    };
    for (page, title) in [
        "General",
        "Layouts",
        "Workspaces",
        "Rules",
        "Keyboard",
        "Mouse & Trackpad",
        "Interface",
        "Advanced",
        "About",
    ]
    .into_iter()
    .enumerate()
    {
        add(page, None, "Settings", title, "");
    }
    for (scope, title) in [
        "Default behavior",
        "Traditional",
        "BSP",
        "Stack",
        "Master Stack",
        "Scrolling",
        "Floating",
        "Spacing",
        "Displays",
    ]
    .into_iter()
    .enumerate()
    {
        add(1, Some(scope), "Layouts", title, "layout");
    }
    for (title, aliases) in [
        ("Screen edges", "gaps gap margin padding outer edges spacing"),
        (
            "Set each edge separately",
            "gaps gap top bottom left right margin padding outer",
        ),
        ("Horizontal", "gaps gap between windows inner horizontal spacing"),
        ("Vertical", "gaps gap between windows inner vertical spacing"),
    ] {
        add(1, Some(7), "Layouts › Spacing", title, aliases);
    }
    add(
        1,
        Some(8),
        "Layouts › Displays",
        "Display settings",
        "monitor screen connected per display override gaps spacing widths",
    );
    add(
        2,
        None,
        "Workspaces",
        "Virtual Workspaces",
        "desktops spaces enabled",
    );
    add(
        3,
        None,
        "Rules",
        "App rules",
        "application bundle identifier manage ignore floating window title match workspace assign desktop",
    );
    add(
        4,
        None,
        "Keyboard",
        "Keyboard shortcuts",
        "keys hotkeys keybindings bindings commands",
    );
    add(
        4,
        None,
        "Keyboard",
        "Shortcut set",
        "keymap keybinding mode default",
    );
    add(
        4,
        None,
        "Keyboard",
        "Reusable modifier combinations",
        "command option control shift modifiers",
    );
    for (title, aliases) in [
        (
            "Workspace swipes",
            "trackpad gestures fingers swipe sensitivity haptic",
        ),
        (
            "Scrolling layout gestures",
            "trackpad gestures fingers swipe scrolling niri",
        ),
        ("Drag & Drop", "mouse modifier move resize swap stack drop"),
        ("Pointer movement", "mouse cursor movement"),
    ] {
        add(5, None, "Mouse & Trackpad", title, aliases);
    }
    add(
        6,
        None,
        "Interface",
        "Overview",
        "window previews fade transitions",
    );
    add(
        6,
        None,
        "Interface",
        "Stack Line",
        "stackline indicator color position thickness interaction",
    );
    for (title, aliases) in [
        ("Startup commands", "launch run shell exec"),
        ("Autofocus blacklist", "focus exclude ignore app"),
        (
            "Reload config when edited externally",
            "configuration file automatic reload toml",
        ),
        ("Configuration file", "open config path toml reload"),
    ] {
        add(7, None, "Advanced", title, aliases);
    }
    for (title, aliases) in [
        ("Check for Updates…", "version latest release update"),
        ("Documentation", "docs help guide manual"),
        ("Release Notes", "changelog version changes"),
        ("Sponsor Rift", "donate support sponsorship"),
    ] {
        add(8, None, "About", title, aliases);
    }
    for field in catalog() {
        add(field.page, field.scope, field.location, field.title, field.key);
    }
    matches.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(a.1.title.cmp(b.1.title))
            .then(a.1.location.cmp(b.1.location))
    });
    matches.dedup_by(|a, b| {
        a.1.title == b.1.title
            && a.1.page == b.1.page
            && a.1.scope == b.1.scope
            && a.1.location == b.1.location
    });
    matches.into_iter().map(|(_, result)| result).collect()
}

/// Scope repeated names such as “Enabled” to their documented section.
pub(super) fn section(location: &str) -> &str { location.rsplit(" › ").next().unwrap_or(location) }

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
}
