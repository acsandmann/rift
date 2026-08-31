// ported from https://github.com/koekeishiya/yabai/blob/master/src/misc/service.h

use std::env;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, Write};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use clap::Subcommand;
use nix::unistd::getuid;

const LAUNCHCTL_PATH: &str = "/bin/launchctl";
const RIFT_PLIST: &str = "git.acsandmann.rift";
/// The agent binary launchd starts. `rift-cli` has to look for it by name;
/// `rift` is it.
const RIFT_AGENT_BIN: &str = "rift";

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

fn find_rift_executable_in_path(path_env: &std::ffi::OsStr) -> io::Result<Option<PathBuf>> {
    let mut current_dir: Option<PathBuf> = None;
    for dir in env::split_paths(path_env) {
        let candidate = dir.join(RIFT_AGENT_BIN);
        if candidate.is_file() {
            if candidate.is_absolute() {
                return Ok(Some(candidate));
            }

            let base = match current_dir.as_ref() {
                Some(dir) => dir,
                None => {
                    current_dir = Some(env::current_dir()?);
                    current_dir.as_ref().expect("just set")
                }
            };
            return Ok(Some(base.join(candidate)));
        }
    }
    Ok(None)
}

/// The rift agent binary the launchd plist should point at.
///
/// `current_exe` comes first because `rift service` runs inside the agent: the
/// binary to launch is this one, and searching `$PATH` instead can name a
/// different install — running a dev build's `rift service start` would write a
/// plist pointing at Homebrew's rift and then "restart" that. macOS reports the
/// path the binary was invoked through rather than resolving it, so a Homebrew
/// invocation still records the stable `bin/rift` symlink rather than the
/// Cellar path a version bump moves out from under it.
///
/// `rift-cli service` is not the agent, so it falls back to looking for one.
fn find_rift_executable() -> io::Result<PathBuf> {
    let exe_path = env::current_exe().map_err(|_| {
        io::Error::new(
            io::ErrorKind::Other,
            "unable to retrieve path of current executable",
        )
    })?;

    if exe_path.file_name() == Some(OsStr::new(RIFT_AGENT_BIN)) {
        return Ok(exe_path);
    }

    let sibling = exe_path.with_file_name(RIFT_AGENT_BIN);
    if sibling.is_file() {
        return Ok(sibling);
    }

    if let Some(path_env) = env::var_os("PATH") {
        if let Some(candidate) = find_rift_executable_in_path(&path_env)? {
            return Ok(candidate);
        }
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!(
            "rift agent executable not found: no sibling 'rift' next to current executable ('{}') and none in $PATH",
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

    let plist = format!(
        r#"<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">
<plist version=\"1.0\">
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
        exe = exe_str,
        path_env = path_env,
        user = user
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

fn spawn_launchctl(args: &[&str]) -> io::Result<()> {
    let mut cmd = Command::new(LAUNCHCTL_PATH);
    cmd.args(args);
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    let _child = cmd.spawn()?;
    Ok(())
}

/// Every launchd label rift is known to run under.
///
/// `rift service` owns the first one, but a Homebrew install is started by
/// `brew services` under Homebrew's own label instead — so looking only at
/// rift's label reports a perfectly healthy install as "not installed".
const KNOWN_LABELS: &[&str] = &[RIFT_PLIST, "homebrew.mxcl.rift-plus", "homebrew.mxcl.rift"];

/// What launchd is doing with rift, for `rift status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceStatus {
    /// The label rift is loaded under, if launchd is running it at all.
    pub running: Option<String>,
    /// Whether the plist `rift service` manages is installed.
    pub own_plist_installed: bool,
}

pub fn service_state() -> io::Result<ServiceStatus> {
    Ok(ServiceStatus {
        running: KNOWN_LABELS
            .iter()
            .find(|label| job_is_running(label))
            .map(|label| (*label).to_string()),
        own_plist_installed: plist_path()?.is_file(),
    })
}

fn job_is_running(label: &str) -> bool {
    let service_target = format!("gui/{}/{}", getuid(), label);
    run_launchctl(&["print", &service_target], true).is_ok_and(|code| code == 0)
}

fn service_is_running() -> io::Result<bool> { Ok(job_is_running(RIFT_PLIST)) }

pub fn service_install_internal(plist_path: &Path) -> io::Result<()> {
    let plist = plist_contents()?;
    write_file_atomic(plist_path, &plist)?;
    Ok(())
}

fn ensure_plist_up_to_date(plist_path: &Path) -> io::Result<bool> {
    let desired = plist_contents()?;
    match fs::read_to_string(plist_path) {
        Ok(existing) if existing == desired => Ok(false),
        Ok(_) => {
            write_file_atomic(plist_path, &desired)?;
            Ok(true)
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            write_file_atomic(plist_path, &desired)?;
            Ok(true)
        }
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
    if service_is_running()? {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "service is still running; stop it first with `rift service stop` before uninstalling",
        ));
    }
    fs::remove_file(plist_path)?;
    Ok(())
}

pub fn service_start() -> io::Result<()> {
    let plist_path = plist_path()?;
    if !plist_path.is_file() {
        service_install_internal(&plist_path).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!(
                    "service file '{}' could not be installed: {}",
                    plist_path.display(),
                    e
                ),
            )
        })?;
    }

    let uid = getuid();
    let service_target = format!("gui/{}/{}", uid, RIFT_PLIST);
    let domain_target = format!("gui/{}", uid);

    let plist_changed = if env::var_os("USER").is_some() && env::var_os("PATH").is_some() {
        ensure_plist_up_to_date(&plist_path)?
    } else {
        false
    };

    let is_bootstrapped = run_launchctl(&["print", &service_target], true).unwrap_or(1);
    if is_bootstrapped != 0 {
        let _ = run_launchctl(&["enable", &service_target], true);

        let _ = spawn_launchctl(&["bootstrap", &domain_target, plist_path.to_str().unwrap()]);
        let _ = std::thread::sleep(std::time::Duration::from_millis(150));
        let code = run_launchctl(&["kickstart", &service_target], false)?;
        if code == 0 {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::Other,
                format!("kickstart after bootstrap failed (exit {})", code),
            ))
        }
    } else {
        let code = run_launchctl(&["kickstart", &service_target], false)?;
        if code == 0 {
            return Ok(());
        }

        if plist_changed {
            let _ = run_launchctl(&["bootout", &domain_target, plist_path.to_str().unwrap()], true);
            let _ = spawn_launchctl(&["bootstrap", &domain_target, plist_path.to_str().unwrap()]);
            let _ = std::thread::sleep(std::time::Duration::from_millis(150));
            let code2 = run_launchctl(&["kickstart", &service_target], false)?;
            if code2 == 0 {
                Ok(())
            } else {
                Err(io::Error::new(
                    io::ErrorKind::Other,
                    format!(
                        "kickstart failed (exit {}), reload+kickstart failed (exit {})",
                        code, code2
                    ),
                ))
            }
        } else {
            Err(io::Error::new(
                io::ErrorKind::Other,
                format!("kickstart failed (exit {})", code),
            ))
        }
    }
}

pub fn service_restart() -> io::Result<()> {
    let plist_path = plist_path()?;
    if !plist_path.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("service file '{}' is not installed", plist_path.display()),
        ));
    }

    let uid = getuid();
    let service_target = format!("gui/{}/{}", uid, RIFT_PLIST);
    let code = run_launchctl(&["kickstart", "-k", &service_target], false)?;
    if code == 0 {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!("kickstart -k failed (exit {})", code),
        ))
    }
}

pub fn service_stop() -> io::Result<()> {
    let plist_path = plist_path()?;
    if !plist_path.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("service file '{}' is not installed", plist_path.display()),
        ));
    }

    let uid = getuid();
    let service_target = format!("gui/{}/{}", uid, RIFT_PLIST);
    let domain_target = format!("gui/{}", uid);

    let is_bootstrapped = run_launchctl(&["print", &service_target], true).unwrap_or(1);

    if is_bootstrapped != 0 {
        let code = run_launchctl(&["kill", "SIGTERM", &service_target], false)?;
        if code == 0 {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::Other,
                format!("kill SIGTERM failed (exit {})", code),
            ))
        }
    } else {
        let code1 =
            run_launchctl(&["bootout", &domain_target, plist_path.to_str().unwrap()], false)?;
        let code2 = run_launchctl(&["disable", &service_target], false)?;

        if code1 == 0 && code2 == 0 {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::Other,
                format!("bootout exit {}, disable exit {}", code1, code2),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs as unix_fs;

    use super::*;

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
