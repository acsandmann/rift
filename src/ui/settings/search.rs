//! Searchable settings destinations, without constructing hidden page trees.
use super::Category::*;
use super::{CATEGORIES, Category};
use crate::common::config::LayoutMode;

/// A destination with its search text precomputed once.
pub(super) struct Entry {
    pub title: &'static str,
    pub location: String,
    pub page: Category,
    pub layout: Option<LayoutMode>,
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
    pub fn target(&self) -> Option<super::layouts::Target> {
        use super::layouts::Target;
        match section(&self.location) {
            "Spacing" => Some(Target::Spacing),
            "Display overrides" => Some(Target::Displays),
            _ => self.layout.map(Target::Layout),
        }
    }

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
    for &(page, heading, _) in &CATEGORIES {
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
    page: Category,
    layout: Option<LayoutMode>,
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
            layout: self.layout,
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
        "Display overrides" => "Customize spacing and column widths for one display.",
        "Workspaces" => "Add workspaces and customize their names and layouts.",
        _ => return entry.location.clone(),
    };
    if entry.layout.is_some() {
        let layout = entry.location.strip_prefix("Layouts › ").unwrap_or(&entry.location);
        format!("{layout} · {text}")
    } else {
        text.into()
    }
}

/// Destinations without schema fields: pages, sections, and handcrafted editors.
const MANUAL: &[(Category, Option<LayoutMode>, &str, &str, &str)] = &[
    (
        Layouts,
        None,
        "Spacing",
        "Screen edges",
        "gaps gap margin padding outer edges spacing",
    ),
    (
        Layouts,
        None,
        "Spacing",
        "Set each edge separately",
        "gaps gap top bottom left right margin padding outer",
    ),
    (
        Layouts,
        None,
        "Spacing",
        "Horizontal",
        "gaps gap between windows inner horizontal spacing",
    ),
    (
        Layouts,
        None,
        "Spacing",
        "Vertical",
        "gaps gap between windows inner vertical spacing",
    ),
    (
        Layouts,
        None,
        "Display overrides",
        "Display overrides",
        "monitor screen connected per display override gaps spacing widths add display uuid",
    ),
    (
        Workspaces,
        None,
        "Workspaces",
        "Virtual Workspaces",
        "desktops spaces enabled",
    ),
    (
        Rules,
        None,
        "Rules",
        "App rules",
        "application bundle identifier manage ignore floating window title match workspace assign desktop",
    ),
    (
        Keyboard,
        None,
        "Keyboard",
        "Keyboard shortcuts",
        "keys hotkeys keybindings bindings commands",
    ),
    (
        Keyboard,
        None,
        "Keyboard",
        "Shortcut set",
        "keymap keybinding mode default",
    ),
    (
        Keyboard,
        None,
        "Keyboard",
        "Reusable modifier combinations",
        "command option control shift modifiers",
    ),
    (
        Mouse,
        None,
        "Mouse & Trackpad",
        "Workspace swipes",
        "trackpad gestures fingers swipe sensitivity haptic",
    ),
    (
        Mouse,
        None,
        "Mouse & Trackpad",
        "Scrolling layout gestures",
        "trackpad gestures fingers swipe scrolling niri",
    ),
    (
        Mouse,
        None,
        "Mouse & Trackpad",
        "Drag & Drop",
        "mouse modifier move resize swap stack drop",
    ),
    (
        Mouse,
        None,
        "Mouse & Trackpad",
        "Pointer movement",
        "mouse cursor movement",
    ),
    (
        Overview,
        None,
        "Overview",
        "Overview",
        "window previews fade transitions",
    ),
    (
        StackTabs,
        None,
        "Stack Tabs",
        "Stack Tabs",
        "stackline indicator color position thickness interaction",
    ),
    (
        Advanced,
        None,
        "Advanced",
        "Startup commands",
        "launch run shell exec",
    ),
    (
        Advanced,
        None,
        "Advanced",
        "Autofocus blacklist",
        "focus exclude ignore app",
    ),
    (
        Advanced,
        None,
        "Advanced",
        "Reload config when edited externally",
        "configuration file automatic reload toml",
    ),
    (
        Advanced,
        None,
        "Advanced",
        "Configuration file",
        "open config path toml reload",
    ),
    (
        About,
        None,
        "About",
        "Check for Updates…",
        "version latest release update",
    ),
    (About, None, "About", "Documentation", "docs help guide manual"),
    (
        About,
        None,
        "About",
        "Matrix Room",
        "matrix community chat discussion support",
    ),
    (
        About,
        None,
        "About",
        "Release Notes",
        "changelog version changes",
    ),
    (
        About,
        None,
        "About",
        "Sponsor Rift",
        "donate support sponsorship",
    ),
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
            page: Category,
            layout: Option<LayoutMode>,
            location: &str,
        ) {
            for field in T::fields().iter().filter(|field| field.group == group) {
                out.push(Draft {
                    title: field.title.split(" (").next().unwrap_or(field.title),
                    location: location.to_owned(),
                    page,
                    layout,
                    key: field.key,
                    help: field.help,
                    aliases: format!("{} {}", field.aliases, field.title),
                });
            }
        }
        let mut out = Vec::new();
        append::<Settings>(&mut out, "general", General, None, "General › Window behavior");
        append::<Settings>(&mut out, "advanced", Advanced, None, "Advanced");
        append::<Settings>(&mut out, "pointer", Mouse, None, "Mouse & Trackpad › Focus");
        append::<VirtualWorkspaceSettings>(&mut out, "", Workspaces, None, "Workspaces");
        append::<GestureSettings>(
            &mut out,
            "main",
            Mouse,
            None,
            "Mouse & Trackpad › Workspace swipes",
        );
        append::<GestureSettings>(
            &mut out,
            "advanced",
            Mouse,
            None,
            "Mouse & Trackpad › Advanced swipe settings",
        );
        append::<ScrollingGestureSettings>(
            &mut out,
            "",
            Mouse,
            None,
            "Mouse & Trackpad › Scrolling layout gestures",
        );
        append::<DragDropSettings>(&mut out, "", Mouse, None, "Mouse & Trackpad › Drag & Drop");
        append::<MenuBarSettings>(&mut out, "", MenuBar, None, "Menu Bar");
        append::<MissionControlSettings>(&mut out, "", Overview, None, "Overview");
        append::<StackLineSettings>(&mut out, "", StackTabs, None, "Stack Tabs");
        append::<LayoutSettings>(&mut out, "", Layouts, None, "Layouts");
        append::<BaseLayoutSettings>(&mut out, "", Layouts, None, "Layouts › Shared settings");
        for (title, layout) in super::pages::layouts() {
            if *layout != LayoutMode::Floating {
                append::<BaseLayoutSettings>(
                    &mut out,
                    "",
                    Layouts,
                    Some(*layout),
                    &format!("Layouts › {title}"),
                );
            }
        }
        append::<TraditionalLayoutSettings>(
            &mut out,
            "",
            Layouts,
            Some(LayoutMode::Traditional),
            "Layouts › Traditional",
        );
        append::<BspLayoutSettings>(&mut out, "", Layouts, Some(LayoutMode::Bsp), "Layouts › BSP");
        append::<StackSettings>(&mut out, "", Layouts, Some(LayoutMode::Stack), "Layouts › Stack");
        append::<MasterStackSettings>(
            &mut out,
            "",
            Layouts,
            Some(LayoutMode::MasterStack),
            "Layouts › Master Stack",
        );
        append::<ScrollingLayoutSettings>(
            &mut out,
            "",
            Layouts,
            Some(LayoutMode::Scrolling),
            "Layouts › Scrolling",
        );
        append::<OuterGaps>(&mut out, "", Layouts, None, "Spacing");
        append::<InnerGaps>(&mut out, "", Layouts, None, "Spacing");

        let pages = CATEGORIES.iter().map(|&(page, title, _)| (page, None, "Settings", title, ""));
        let layouts = super::pages::layouts()
            .iter()
            .map(|&(title, layout)| (Layouts, Some(layout), "Layouts", title, "layout"));
        for (page, layout, location, title, aliases) in
            pages.chain(layouts).chain(MANUAL.iter().copied())
        {
            // A destination that names a documented field enriches it instead of repeating it.
            let fields: Vec<_> = out
                .iter()
                .enumerate()
                .filter(|(_, field)| {
                    field.title == title && field.page == page && field.layout == layout
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
                    layout,
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
    fn search_finds_settings_and_their_layouts() {
        for (query, title, page, layout) in [
            (
                "column width",
                "Default column width",
                Layouts,
                Some(LayoutMode::Scrolling),
            ),
            (" FFM ", "Focus follows pointer", Mouse, None),
            ("stack offset", "Window offset", Layouts, Some(LayoutMode::Stack)),
            ("screen edges", "Screen edges", Layouts, None),
            ("per display override", "Display overrides", Layouts, None),
        ] {
            let found = results(query);
            let first = found.first().expect("find a setting, not just its section");
            assert_eq!((first.title, first.page, first.layout), (title, page, layout));
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
                seen.insert((
                    entry.title,
                    &entry.location,
                    entry.page,
                    entry.layout.map(|layout| layout.config_choice_index())
                )),
                "duplicate destination {} in {}",
                entry.title,
                entry.location
            );
        }
    }
}
