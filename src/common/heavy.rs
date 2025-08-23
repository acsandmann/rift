// TODO: this cannot be the best way to do this

use std::path::PathBuf;
use parking_lot::Mutex;

use once_cell::sync::Lazy;

use crate::common::collections::HashMap;
use crate::sys::app::pid_t;

const KNOWN_HEAVY: &[&str] = &[
    "com.hnc.discord",
    "com.discordapp.",
    "com.slack.",
    "com.apple.MobileSMS", // imsg is a crazy offender for some reason
    "com.microsoft.teams",
    "us.zoom.xos",
    "com.tinyspeck.slackmacgap",
    "com.microsoft.VSCode",
    "com.github.atom",
    "com.jetbrains.",
    "com.sublimetext.",
    "com.adobe.dreamweaver",
    "com.figma.Desktop",
    "com.adobe.Photoshop",
    "com.adobe.AfterEffects",
    "com.adobe.Premiere",
    "com.sketch.Sketch",
    "com.bohemiancoding.sketch3",
    "com.google.Chrome",
    "org.mozilla.firefox",
    "com.opera.Opera",
    "com.brave.Browser",
    "com.microsoft.edgemac",
    "com.spotify.client",
    "com.apple.TV",
    "com.netflix.Netflix",
    "com.twitch.desktop",
    "com.whatsapp.desktop",
    "com.tdesktop.Telegram",
    "com.notion.id",
    "com.postmanlabs.mac",
    "com.obsidian.obsidian",
];

static HEAVY_CACHE: Lazy<Mutex<HashMap<pid_t, bool>>> =
    Lazy::new(|| Mutex::new(HashMap::default()));

/// heuristic to slow down fps for apps that might not take well to lots of ax calls
pub fn is_heavy(pid: pid_t, bundle_id: &str, bundle_path: &PathBuf) -> bool {
    if KNOWN_HEAVY.iter().any(|k| bundle_id.starts_with(k)) {
        return true;
    }

    let mut cache = HEAVY_CACHE.lock();
    if let Some(&cached) = cache.get(&pid) {
        return cached;
    }

    let mut heavy = false;

    let frameworks_dir = bundle_path.join("Contents").join("Frameworks");
    if frameworks_dir.exists() {
        let electron_fw = frameworks_dir.join("Electron Framework.framework");
        let cef_fw = frameworks_dir.join("Chromium Embedded Framework.framework");
        let qt_web = frameworks_dir.join("QtWebEngineCore.framework");

        heavy |= electron_fw.exists();
        heavy |= cef_fw.exists();
        heavy |= qt_web.exists();
    }

    if !heavy {
        let resources_dir = bundle_path.join("Contents").join("Resources");
        let web_indicators = ["app.asar", "main.js", "index.html", "app"];

        for indicator in &web_indicators {
            heavy |= resources_dir.join(indicator).exists();
        }
    }

    cache.insert(pid, heavy);
    heavy
}
