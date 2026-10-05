use std::path::Path;

use objc2::runtime::AnyObject;
use objc2_foundation::{NSDictionary, NSString};

/// Read bundle metadata without launching apps or retaining NSBundle's global cache.
pub(super) fn installed() -> Vec<(String, String)> {
    let mut apps = Vec::new();
    objc2::rc::autoreleasepool(|_| {
        for root in ["/Applications", "/System/Applications"] {
            scan(Path::new(root), 4, &mut apps);
        }
        if let Some(home) = std::env::var_os("HOME") {
            scan(&Path::new(&home).join("Applications"), 4, &mut apps);
        }
    });
    apps.sort_by_key(|(name, _)| name.to_lowercase());
    let mut seen = std::collections::BTreeSet::new();
    apps.retain(|(_, id)| seen.insert(id.clone()));
    apps
}

fn scan(directory: &Path, depth: usize, apps: &mut Vec<(String, String)>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|extension| {
            extension
                .to_str()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
        }) {
            if let Some(app) = bundle(&path) {
                apps.push(app);
            }
        } else if depth > 0 && entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            scan(&path, depth - 1, apps);
        }
    }
}

#[allow(deprecated)]
fn bundle(path: &Path) -> Option<(String, String)> {
    let file = NSString::from_str(&path.join("Contents/Info.plist").to_string_lossy());
    let info = unsafe { NSDictionary::<NSString, AnyObject>::dictionaryWithContentsOfFile(&file) }?;
    let text = |key: &str| {
        info.objectForKey(&NSString::from_str(key))
            .and_then(|value| value.downcast::<NSString>().ok())
            .map(|value| value.to_string())
            .filter(|value| !value.trim().is_empty())
    };
    let id = text("CFBundleIdentifier")?;
    let name = text("CFBundleDisplayName")
        .or_else(|| text("CFBundleName"))
        .unwrap_or_else(|| path.file_stem().unwrap_or_default().to_string_lossy().into_owned());
    Some((name, id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_nested_apps_and_reads_identifiers_without_including_helpers() {
        let root = tempfile::tempdir().unwrap();
        let write = |relative: &str, name: &str, id: &str| {
            let contents = root.path().join(relative).join("Contents");
            std::fs::create_dir_all(&contents).unwrap();
            std::fs::write(contents.join("Info.plist"), format!(r#"<?xml version="1.0"?><plist version="1.0"><dict><key>CFBundleDisplayName</key><string>{name}</string><key>CFBundleIdentifier</key><string>{id}</string></dict></plist>"#)).unwrap();
        };
        write("Example.app", "Friendly App", "dev.example.app");
        write("Utilities/Utility.app", "Utility", "dev.example.utility");
        write(
            "Example.app/Contents/Library/LoginItems/Helper.app",
            "Helper",
            "dev.example.helper",
        );
        std::fs::create_dir_all(root.path().join("Incomplete.app")).unwrap();
        let mut apps = Vec::new();
        objc2::rc::autoreleasepool(|_| scan(root.path(), 4, &mut apps));
        apps.sort();
        assert_eq!(apps, [
            ("Friendly App".into(), "dev.example.app".into()),
            ("Utility".into(), "dev.example.utility".into())
        ]);
    }
}
