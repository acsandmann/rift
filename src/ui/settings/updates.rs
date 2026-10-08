//! A user-initiated release check. No timers or background checks.
pub type Completion = Box<dyn FnOnce(Result<String, String>)>;

pub fn start() -> tokio::sync::oneshot::Receiver<Result<String, String>> {
    let (send, receive) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let _ = send.send(latest());
    });
    receive
}

fn latest() -> Result<String, String> {
    let response = std::process::Command::new("/usr/bin/curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--max-time",
            "10",
            "--header",
            "Accept: application/vnd.github+json",
            "--user-agent",
            "Rift-update-check",
            "https://api.github.com/repos/acsandmann/rift/releases/latest",
        ])
        .output()
        .map_err(|_| "Couldn’t connect. Check your internet connection and try again.".to_string())?;
    if !response.status.success() {
        return Err("Couldn’t check for updates. Check your connection and try again.".into());
    }
    let release: serde_json::Value = serde_json::from_slice(&response.stdout)
        .map_err(|_| "The release service returned an unexpected response.".to_string())?;
    let tag = release["tag_name"]
        .as_str()
        .ok_or_else(|| "No release version was returned.".to_string())?;
    let latest = tag.trim_start_matches('v');
    let running = env!("CARGO_PKG_VERSION");
    if latest == running {
        Ok("Rift is up to date.".into())
    } else {
        Ok(format!("Latest release: {latest}. You’re running {running}."))
    }
}
