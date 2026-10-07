//! Searchable settings destinations, without constructing hidden page trees.
#[derive(Clone)]
pub(super) struct Result {
    pub title: &'static str,
    pub location: &'static str,
    pub page: usize,
    pub scope: Option<usize>,
}

#[derive(Clone)]
pub(super) enum Row { Heading(&'static str), Setting(Result) }
pub(super) fn grouped(results: &[Result]) -> Vec<Row> {
    let mut rows = Vec::new();
    for page in 0..9 {
        let matches: Vec<_> = results.iter().filter(|result| result.page == page).collect();
        if matches.is_empty() { continue; }
        rows.push(Row::Heading(["General", "Layouts", "Workspaces", "Rules", "Keyboard", "Mouse & Trackpad", "Interface", "Advanced", "About"][page]));
        rows.extend(matches.into_iter().cloned().map(Row::Setting));
    }
    rows
}
impl Result {
    pub fn description(&self) -> String {
        let text = match self.title {
            "Default column width" => "Choose how wide new scrolling columns are.",
            "Width presets" => "Choose the widths used when cycling column sizes.",
            "Focus follows pointer" => "Focus a window when you move the pointer over it.",
            "Screen edges" => "Set the space between windows and screen edges.",
            "Horizontal" => "Set the horizontal space between windows.",
            "Vertical" => "Set the vertical space between windows.",
            "Master width" => "Choose how much space primary windows use.",
            "Master windows" => "Choose the number of primary windows.",
            "Default workspace" => "Choose the workspace used for new windows.",
            "Preserve workspace focus" => "Remember the focused window in each workspace.",
            "Virtual Workspaces" => "Organize your windows into separate workspaces.",
            "Auto-assign windows" => "Automatically place windows in a workspace.",
            "Prevent wrapping" => "Stop workspace navigation at the first and last workspace.",
            "Reapply rules when titles change" => "Check workspace rules again when a window title changes.",
            "Switch back when selecting active workspace" => "Return to your previous workspace when you select the active one.",
            "Workspace swipes" => "Switch workspaces using trackpad gestures.",
            "Show empty workspaces" => "Include workspaces without windows in the menu bar.",
            "Workspaces to show" => "Choose which workspaces appear in the menu bar.",
            "Active workspace label" => "Choose the label shown for the active workspace.",
            "Show menu bar indicator" => "Show Rift and your workspaces in the menu bar.",
            "Window offset" => "Set how far stacked windows overlap.",
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
            format!("{} · {text}", self.location.strip_prefix("Layouts › ").unwrap_or(self.location))
        } else { text.into() }
    }
}

pub(super) fn results(query: &str) -> Vec<Result> {
    let query = query.trim().to_lowercase();
    let terms: Vec<_> = query.split_whitespace().collect();
    if terms.is_empty() { return Vec::new(); }
    let mut matches = Vec::new();
    let mut add = |page, scope, location, title, aliases| {
        let title: &'static str = title;
        let location: &'static str = location;
        let aliases: &'static str = aliases;
        let name = title.to_lowercase();
        let context = location.to_lowercase();
        let words = format!("{name} {context} {aliases}");
        if !terms.iter().all(|term| words.contains(term)) { return; }
        let score = if name == query { 1000 } else if name.starts_with(&query) { 600 }
            else if name.contains(&query) { 400 } else { 0 }
            + terms.iter().map(|term| if name.contains(term) { 60 } else if context.contains(term) { 20 } else { 5 }).sum::<usize>();
        matches.push((score, Result { title, location, page, scope }));
    };
    for (page, title) in ["General", "Layouts", "Workspaces", "Rules", "Keyboard", "Mouse & Trackpad", "Interface", "Advanced", "About"].into_iter().enumerate() {
        add(page, None, "Settings", title, "");
    }
    for (scope, title) in ["Default behavior", "Traditional", "BSP", "Stack", "Master Stack", "Scrolling", "Floating", "Spacing", "Displays"].into_iter().enumerate() {
        add(1, Some(scope), "Layouts", title, "layout");
    }
    for (title, aliases) in [
        ("Animate window changes", "animation transitions motion"),
        ("Duration", "animation speed seconds"),
        ("Frame rate", "animation fps performance"),
        ("Animation easing", "transition curve"),
        ("Start with tiling disabled", "floating startup automatic tiling"),
    ] { add(0, None, "General › Window behavior", title, aliases); }
    for (scope, location, fields) in [
        (0, "Layouts › Default behavior", &["Default layout", "New window position"][..]),
        (1, "Layouts › Traditional", &["Keep windows equally sized", "New window position"][..]),
        (2, "Layouts › BSP", &["Single window aspect ratio", "New window position"][..]),
        (3, "Layouts › Stack", &["Window offset", "Orientation", "New window position"][..]),
        (4, "Layouts › Master Stack", &["Master width", "Master windows", "Master side", "New windows", "Master arrangement", "Stack arrangement", "New window position"][..]),
        (5, "Layouts › Scrolling", &["Default column width", "Width presets", "Preserve window sizes", "Minimum width", "Maximum width", "Alignment", "Focus navigation", "Animate navigation", "New window position"][..]),
    ] {
        for title in fields { add(1, Some(scope), location, *title, if scope == 5 && title.to_lowercase().contains("width") { "column width sizing percentages tiling" } else { "tiling arrange windows" }); }
    }
    for (title, aliases) in [
        ("Screen edges", "gaps gap margin padding outer edges spacing"),
        ("Set each edge separately", "gaps gap top bottom left right margin padding outer"),
        ("Horizontal", "gaps gap between windows inner horizontal spacing"),
        ("Vertical", "gaps gap between windows inner vertical spacing"),
    ] { add(1, Some(7), "Layouts › Spacing", title, aliases); }
    add(1, Some(8), "Layouts › Displays", "Display settings", "monitor screen connected per display override gaps spacing widths");
    for (title, aliases) in [
        ("Virtual Workspaces", "desktops spaces enabled"),
        ("Default workspace", "startup desktop space"),
        ("Auto-assign windows", "automatically assign apps workspaces"),
        ("Preserve workspace focus", "remember active focus"),
        ("Switch back when selecting active workspace", "back and forth previous desktop"),
        ("Prevent wrapping", "cycle last first workspace"),
        ("Reapply rules when titles change", "application title updated match workspace"),
        ("Workspaces", "names rename layout desktop space count number add remove"),
    ] { add(2, None, "Workspaces", title, aliases); }
    add(3, None, "Rules", "App rules", "application bundle identifier manage ignore floating window title match workspace assign desktop");
    add(4, None, "Keyboard", "Keyboard shortcuts", "keys hotkeys keybindings bindings commands" );
    add(4, None, "Keyboard", "Shortcut set", "keymap keybinding mode default" );
    add(4, None, "Keyboard", "Reusable modifier combinations", "command option control shift modifiers" );
    for (title, aliases) in [
        ("Focus follows pointer", "ffm focus follows mouse hover autofocus"),
        ("Move pointer to focused window", "mouse follows focus cursor warp"),
        ("Hide pointer after focusing", "hide mouse cursor"),
        ("Workspace swipes", "trackpad gestures fingers swipe sensitivity haptic"),
        ("Scrolling layout gestures", "trackpad gestures fingers swipe scrolling niri"),
        ("Drag & Drop", "mouse modifier move resize swap stack drop"),
        ("Pointer movement", "mouse cursor movement"),
    ] { add(5, None, "Mouse & Trackpad", title, aliases); }
    for (title, aliases) in [
        ("Show menu bar indicator", "menubar status workspace icon"),
        ("Show empty workspaces", "menu bar empty spaces"),
        ("Workspaces to show", "menu bar all active display"),
        ("Active workspace label", "menu bar name number index"),
        ("Display style", "menu bar layout style"),
        ("Layout folder", "menu bar directory icons"),
        ("Overview", "window previews fade transitions"),
        ("Stack Line", "stackline indicator color position thickness interaction"),
    ] { add(6, None, "Interface", title, aliases); }
    for (title, aliases) in [
        ("Startup commands", "launch run shell exec"),
        ("Autofocus blacklist", "focus exclude ignore app"),
        ("Reload config when edited externally", "configuration file automatic reload toml"),
        ("Configuration file", "open config path toml reload"),
    ] { add(7, None, "Advanced", title, aliases); }
    for (title, aliases) in [
        ("Check for Updates…", "version latest release update"),
        ("Documentation", "docs help guide manual"),
        ("Release Notes", "changelog version changes"),
        ("Sponsor Rift", "donate support sponsorship"),
    ] { add(8, None, "About", title, aliases); }
    matches.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.title.cmp(b.1.title)).then(a.1.location.cmp(b.1.location)));
    matches.dedup_by(|a, b| a.1.title == b.1.title && a.1.page == b.1.page && a.1.scope == b.1.scope);
    matches.into_iter().map(|(_, result)| result).collect()
}


pub(super) fn reveal(root: &objc2_app_kit::NSView, title: &str) {
    use objc2::Message;
    use objc2_app_kit::{NSAccessibility, NSControl, NSTextField, NSView};
    fn find(view: &NSView, title: &str, labels: bool) -> Option<objc2::rc::Retained<NSView>> {
        if let Some(control) = view.downcast_ref::<NSControl>() {
            let name = control.accessibilityLabel().map(|value| value.to_string());
            let matches = name.as_deref().is_some_and(|name| name == title || name.starts_with(&format!("{title} (")))
                || (labels && view.downcast_ref::<NSTextField>().is_some_and(|field| field.stringValue().to_string() == title));
            if matches { return Some(view.retain()); }
        }
        view.subviews().iter().find_map(|child| find(&child, title, labels))
    }
    if let Some(view) = find(root, title, false).or_else(|| find(root, title, true)) {
        view.scrollRectToVisible(view.bounds());
        if let Some(control) = view.downcast_ref::<NSControl>() {
            if control.isEnabled() && view.downcast_ref::<NSTextField>().is_none_or(|field| field.isEditable()) {
                if let Some(window) = view.window() { window.makeFirstResponder(Some(&view)); }
            }
        }
    }
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
        assert!(results("column width unrelatedword").is_empty(), "match every query word");
        assert!(results("   ").is_empty());
    }
}
