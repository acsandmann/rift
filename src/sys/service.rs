// ported from https://github.com/koekeishiya/yabai/blob/master/src/misc/service.h

use std::env;
use std::fs::{self, File};
use std::io::{self, Write};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use nix::unistd::getuid;

const LAUNCHCTL_PATH: &str = "/bin/launchctl";
const RIFT_PLIST: &str = "com.acsandmann.rift";

fn plist_path() -> io::Result<PathBuf> {
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "HOME not set"))?;
    Ok(home.join("Library").join("LaunchAgents").join(format!("{RIFT_PLIST}.plist")))
}

fn plist_contents() -> io::Result<String> {
    let user =
        env::var("USER").map_err(|_| io::Error::new(io::ErrorKind::Other, "env USER not set"))?;
    let path_env =
        env::var("PATH").map_err(|_| io::Error::new(io::ErrorKind::Other, "env PATH not set"))?;
    let exe_path = env::current_exe().map_err(|_| {
        io::Error::new(io::ErrorKind::Other, "unable to retrieve path of executable")
    })?;

    let preferred_agent_path = exe_path.with_file_name("rift");
    if !preferred_agent_path.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "rift agent executable '{}' not found next to current executable; \
place the 'rift' binary adjacent to the CLI or install the agent first",
                preferred_agent_path.display()
            ),
        ));
    }

    let exe_str = preferred_agent_path
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "non-UTF8 executable path"))?;

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

pub fn service_install_internal(plist_path: &Path) -> io::Result<()> {
    let plist = plist_contents()?;
    write_file_atomic(plist_path, &plist)?;
    Ok(())
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

    let is_bootstrapped = run_launchctl(&["print", &service_target], true).unwrap_or(1);
    if is_bootstrapped != 0 {
        let _ = run_launchctl(&["enable", &service_target], false);

        let code = run_launchctl(
            &["bootstrap", &domain_target, plist_path.to_str().unwrap()],
            false,
        )?;
        if code == 0 {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::Other,
                format!("bootstrap failed (exit {})", code),
            ))
        }
    } else {
        let code = run_launchctl(&["kickstart", &service_target], false)?;
        if code == 0 {
            Ok(())
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
