// ported from https://github.com/koekeishiya/yabai/blob/master/src/misc/service.h

use std::env;
use std::fs::{self, File};
use std::io::{self, Write};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use clap::Subcommand;
use nix::unistd::getuid;

const LAUNCHCTL_PATH: &str = "/bin/launchctl";
const RIFT_PLIST: &str = "git.acsandmann.rift";

#[derive(Subcommand)]
pub enum ServiceCommands {
    /// Install the per-user launchd service
    Install,
    /// Uninstall the per-user launchd service
    Uninstall,
    /// Start (or bootstrap) the service
    Start,
    /// Stop (or bootout/kill) the service
    Stop,
    /// Restart the service (kickstart -k)
    Restart,
}

pub fn handle_service_command(cmd: &ServiceCommands) -> Result<&'static str, String> {
    match cmd {
        ServiceCommands::Install => service_install()
            .map(|_| "Service installed.")
            .map_err(|e| format!("Failed to install service: {}", e)),
        ServiceCommands::Uninstall => service_uninstall()
            .map(|_| "Service uninstalled.")
            .map_err(|e| format!("Failed to uninstall service: {}", e)),
        ServiceCommands::Start => service_start()
            .map(|_| "Service started.")
            .map_err(|e| format!("Failed to start service: {}", e)),
        ServiceCommands::Stop => service_stop()
            .map(|_| "Service stopped.")
            .map_err(|e| format!("Failed to stop service: {}", e)),
        ServiceCommands::Restart => service_restart()
            .map(|_| "Service restarted.")
            .map_err(|e| format!("Failed to restart service: {}", e)),
    }
}

fn plist_path() -> io::Result<PathBuf> {
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "HOME not set"))?;
    Ok(home.join("Library").join("LaunchAgents").join(format!("{RIFT_PLIST}.plist")))
}

// Preserve Homebrew's opt symlink rather than canonicalizing into the Cellar.
fn bundle_executable(path: PathBuf) -> PathBuf {
    if let Ok(real) = path.canonicalize() {
        for ancestor in real.ancestors() {
            if ancestor.file_name().is_some_and(|name| name == "Cellar") {
                if let Ok(relative) = real.strip_prefix(ancestor) {
                    let mut parts = relative.components();
                    if parts.next().is_some_and(|part| part.as_os_str() == "rift") {
                        let stable = ancestor
                            .parent()
                            .unwrap()
                            .join("opt/rift/Rift.app/Contents/MacOS/rift");
                        if stable.is_file() {
                            return stable;
                        }
                    }
                }
            }
        }
    }
    path
}

fn find_rift_executable_in_path(path_env: &std::ffi::OsStr) -> io::Result<Option<PathBuf>> {
    let mut current_dir: Option<PathBuf> = None;
    for dir in env::split_paths(path_env) {
        let candidate = dir.join("rift");
        if candidate.is_file() {
            if candidate.is_absolute() {
                return Ok(Some(bundle_executable(candidate)));
            }

            let base = match current_dir.as_ref() {
                Some(dir) => dir,
                None => {
                    current_dir = Some(env::current_dir()?);
                    current_dir.as_ref().expect("just set")
                }
            };
            return Ok(Some(bundle_executable(base.join(candidate))));
        }
    }
    Ok(None)
}

fn find_rift_executable() -> io::Result<PathBuf> {
    if let Some(path_env) = env::var_os("PATH") {
        if let Some(candidate) = find_rift_executable_in_path(&path_env)? {
            return Ok(candidate);
        }
    }

    let exe_path = env::current_exe().map_err(|_| {
        io::Error::new(
            io::ErrorKind::Other,
            "unable to retrieve path of current executable",
        )
    })?;
    let sibling = exe_path.with_file_name("rift");
    if sibling.is_file() {
        return Ok(bundle_executable(sibling));
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!(
            "rift agent executable not found: not present in $PATH and no sibling 'rift' next to current executable ('{}')",
            exe_path.display()
        ),
    ))
}

fn plist_contents() -> io::Result<String> {
    let user =
        env::var("USER").map_err(|_| io::Error::new(io::ErrorKind::Other, "env USER not set"))?;
    let path_env =
        env::var("PATH").map_err(|_| io::Error::new(io::ErrorKind::Other, "env PATH not set"))?;

    let agent_exe = find_rift_executable()?;
    let exe_str = agent_exe
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "non-UTF8 executable path"))?;

    let escape = |value: &str| {
        value
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&apos;")
    };
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{name}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
    </array>
    <key>EnvironmentVariables</key>
    <dict>
        <key>PATH</key>
        <string>{path_env}</string>
        <key>RUST_LOG</key>
        <string>error,warn,info</string>
    </dict>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
        <key>Crashed</key>
        <true/>
    </dict>
    <key>StandardOutPath</key>
    <string>/tmp/rift_{user}.out.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/rift_{user}.err.log</string>
    <key>ProcessType</key>
    <string>Interactive</string>
    <key>LimitLoadToSessionType</key>
    <string>Aqua</string>
    <key>Nice</key>
    <integer>-20</integer>
</dict>
</plist>
"#,
        name = RIFT_PLIST,
        exe = escape(exe_str),
        path_env = escape(&path_env),
        user = escape(&user)
    );

    Ok(plist)
}
/*<key>MachServices</key>
<dict>
    <key>{name}</key>
    <true/>
</dict> */

fn ensure_parent_dir(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn write_file_atomic(path: &Path, contents: &str) -> io::Result<()> {
    ensure_parent_dir(path)?;
    let mut f = File::create(path)?;
    f.write_all(contents.as_bytes())?;
    Ok(())
}

fn run_launchctl(args: &[&str], suppress_output: bool) -> io::Result<i32> {
    let mut cmd = Command::new(LAUNCHCTL_PATH);
    cmd.args(args);
    if suppress_output {
        cmd.stdout(Stdio::null()).stderr(Stdio::null());
    }
    let status = cmd.status()?;

    if let Some(code) = status.code() {
        Ok(code)
    } else {
        let sig = status.signal().unwrap_or_default();
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!("launchctl terminated by signal {}", sig),
        ))
    }
}

fn service_is_loaded(target: &str) -> io::Result<bool> {
    match run_launchctl(&["print", target], true)? {
        0 => Ok(true),    // Includes a loaded job whose process is stopped.
        113 => Ok(false), // launchctl: could not find service.
        code => Err(io::Error::other(format!(
            "launchctl print {target} failed (exit {code})"
        ))),
    }
}

fn launchctl(args: &[&str]) -> io::Result<()> {
    match run_launchctl(args, false)? {
        0 => Ok(()),
        code => Err(io::Error::other(format!(
            "launchctl {} failed (exit {code})",
            args.join(" ")
        ))),
    }
}

fn bootout(target: &str) -> io::Result<()> {
    launchctl(&["bootout", target])?;
    let deadline = Instant::now() + Duration::from_secs(3);
    while service_is_loaded(target)? {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("{target} is still loaded"),
            ));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    Ok(())
}

pub fn service_install_internal(plist_path: &Path) -> io::Result<()> {
    let plist = plist_contents()?;
    crate::ui::onboarding::register_service_install()?;
    write_file_atomic(plist_path, &plist)?;
    Ok(())
}

fn pending_plist_update(plist_path: &Path) -> io::Result<Option<String>> {
    // Nix/Home Manager own these symlinks and their service arguments.
    if fs::symlink_metadata(plist_path).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return Ok(None);
    }
    let desired = plist_contents()?;
    match fs::read_to_string(plist_path) {
        Ok(existing) if existing == desired => Ok(None),
        Ok(_) => Ok(Some(desired)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Some(desired)),
        Err(err) => Err(err),
    }
}

pub fn service_install() -> io::Result<()> {
    let plist_path = plist_path()?;
    if plist_path.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("service file '{}' is already installed", plist_path.display()),
        ));
    }
    service_install_internal(&plist_path)
}

pub fn service_uninstall() -> io::Result<()> {
    let plist_path = plist_path()?;
    if !plist_path.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("service file '{}' is not installed", plist_path.display()),
        ));
    }
    if service_is_loaded(&format!("gui/{}/{}", getuid(), RIFT_PLIST))? {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "service is still running; stop it first with `rift service stop` before uninstalling",
        ));
    }
    fs::remove_file(plist_path)?;
    Ok(())
}

pub fn service_start() -> io::Result<()> { start_or_restart(false) }

pub fn service_restart() -> io::Result<()> { start_or_restart(true) }

fn start_or_restart(restart: bool) -> io::Result<()> {
    let path = plist_path()?;
    if restart && !path.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("service file '{}' is not installed", path.display()),
        ));
    }
    let domain = format!("gui/{}", getuid());
    let target = format!("{domain}/{RIFT_PLIST}");
    let path_str = path.to_str().ok_or_else(|| io::Error::other("non-UTF8 LaunchAgent path"))?;
    let mut loaded = service_is_loaded(&target)?;
    if (!path.is_file() || env::var_os("USER").is_some() && env::var_os("PATH").is_some())
        && let Some(desired) = pending_plist_update(&path)?
    {
        // Keep the old plist if removal fails so a retry still detects the update.
        if loaded {
            bootout(&target)?;
            loaded = false;
        }
        write_file_atomic(&path, &desired)?;
    }
    if !loaded {
        launchctl(&["enable", &target])?;
        launchctl(&["bootstrap", &domain, path_str])?;
    }
    if restart && loaded {
        launchctl(&["kickstart", "-k", &target])
    } else {
        launchctl(&["kickstart", &target])
    }
}

pub fn service_stop() -> io::Result<()> {
    let path = plist_path()?;
    if !path.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("service file '{}' is not installed", path.display()),
        ));
    }
    let target = format!("gui/{}/{}", getuid(), RIFT_PLIST);
    if service_is_loaded(&target)? {
        bootout(&target)?;
    }
    launchctl(&["disable", &target])
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs as unix_fs;

    use super::*;

    #[test]
    fn service_refresh_preserves_externally_managed_plist() {
        let tmp = tempfile::tempdir().unwrap();
        let managed = tmp.path().join("managed.plist");
        let agent = tmp.path().join("agent.plist");
        let contents = "externally managed service configuration";
        fs::write(&managed, contents).unwrap();
        unix_fs::symlink(&managed, &agent).unwrap();

        assert!(pending_plist_update(&agent).unwrap().is_none());
        assert_eq!(fs::read_to_string(&managed).unwrap(), contents);
        assert!(fs::symlink_metadata(&agent).unwrap().file_type().is_symlink());
    }

    #[test]
    fn find_rift_executable_prefers_stable_symlink_path() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();

        let real = dir.join("rift-real");
        fs::write(&real, b"#!/bin/sh\nexit 0\n").unwrap();
        let mut perms = fs::metadata(&real).unwrap().permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o755);
            fs::set_permissions(&real, perms).unwrap();
        }

        let link = dir.join("rift");
        unix_fs::symlink(&real, &link).unwrap();

        let found = find_rift_executable_in_path(dir.as_os_str())
            .unwrap()
            .expect("expected to find rift in PATH");
        assert_eq!(found, link);
    }
}
