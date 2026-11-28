//! Short-lived Setup Assistant. No window-manager actors are initialized here.
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use cgs::controls::Button;
use cgs::layout::{Divider, HStack, Spacer, VStack};
use cgs::settings::Section;
use cgs::text::Label;
use cgs::window::Window;
use cgs::{CGSize, Ui};
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSColor, NSWindowStyleMask,
    NSWindowTitleVisibility, NSWorkspace,
};
use objc2_foundation::{NSString, NSURL};
use tokio::sync::mpsc::unbounded_channel;

use crate::sys::accessibility::ax_is_trusted;

fn state_dir() -> PathBuf {
    dirs::home_dir()
        .expect("home directory")
        .join("Library/Application Support/Rift")
}

/// Evidence independent of executable location and release version.
fn existing(home: &Path, config: &Path) -> bool {
    config.exists()
        || home.join(".config/rift/config.toml").exists()
        || home.join(".rift").exists()
        || home.join("Library/LaunchAgents/git.acsandmann.rift.plist").exists()
        || home.join("Library/LaunchAgents/com.rift.plist").exists()
}

pub fn first_launch(config: &Path) -> bool {
    first_launch_at(&dirs::home_dir().unwrap(), config, &state_dir())
        && !std::env::var("USER").is_ok_and(|u| {
            !state_dir().join("new-install").exists()
                && std::fs::metadata(format!("/tmp/rift_{u}.out.log")).is_ok_and(|m| m.len() > 0)
        })
}

fn first_launch_at(home: &Path, config: &Path, state: &Path) -> bool {
    !state.join("presented").exists()
        && !state.join("completed").exists()
        && if state.join("new-install").exists() {
            !config.exists()
                && !home.join(".config/rift/config.toml").exists()
                && !home.join(".rift").exists()
        } else {
            !existing(home, config)
        }
}

/// Remember a fresh service installation before its launch agent becomes legacy evidence.
pub fn register_service_install() -> std::io::Result<()> {
    if first_launch(&crate::common::config::config_file()) {
        std::fs::create_dir_all(state_dir())?;
        std::fs::write(state_dir().join("new-install"), b"1")?;
    }
    Ok(())
}

/// Advisory lock released by the kernel even if the UI crashes.
fn lock() -> std::io::Result<Option<File>> { lock_at(&state_dir()) }

fn lock_at(dir: &Path) -> std::io::Result<Option<File>> {
    std::fs::create_dir_all(dir)?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join("setup.lock"))?;
    if unsafe { nix::libc::flock(file.as_raw_fd(), nix::libc::LOCK_EX | nix::libc::LOCK_NB) } != 0 {
        return Ok(None);
    }
    Ok(Some(file))
}

pub fn launch(config: &Path, intro: bool, recovery: bool) -> anyhow::Result<Option<bool>> {
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command.arg("--onboarding").arg("--config").arg(config);
    if intro {
        command.arg("--setup-intro");
    }
    if recovery {
        command.arg("--permission-recovery");
    }
    Ok(match command.status()?.code() {
        Some(0) => Some(false),
        Some(4) => Some(true),
        _ => None,
    })
}

fn open(url: &str) {
    if let Some(url) = NSURL::URLWithString(&NSString::from_str(url)) {
        NSWorkspace::sharedWorkspace().openURL(&url);
    }
}

pub fn run(intro: bool, recovery: bool) -> ! {
    let result = (|| -> anyhow::Result<()> {
        let Some(_lock) = lock()? else {
            std::process::exit(2);
        };
        // Do not mark presentation when no graphical login session is available.
        anyhow::ensure!(
            std::env::var_os("SSH_CONNECTION").is_none(),
            "Setup requires a local graphical session; run rift --setup locally"
        );
        let mtm = MainThreadMarker::new().expect("main thread");
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
        app.finishLaunching();
        anyhow::ensure!(
            objc2_app_kit::NSScreen::mainScreen(mtm).is_some(),
            "No graphical display available"
        );
        if !recovery {
            std::fs::write(state_dir().join("presented"), b"1")?;
        }
        crate::sys::executor::Executor::run_main(mtm, async move {
            let ui = Ui::new(mtm);
            cgs::app::Application::shared(&ui).install_close_window_command();
            let (send, mut receive) = unbounded_channel::<i32>();
            let mut introduction = if intro && !recovery {
                super::onboarding_intro::present(&ui, &app).await
            } else {
                None
            };
            let closed = send.clone();
            let window = Window::new(&ui)
                .title("Set Up Rift")
                .size(CGSize::new(560., 420.))
                .on_close(move || {
                    let _ = closed.send(-1);
                });
            window.ns_window().setTitleVisibility(NSWindowTitleVisibility::Hidden);
            window.ns_window().setTitlebarAppearsTransparent(true);
            window.ns_window().setStyleMask(
                NSWindowStyleMask::Titled
                    | NSWindowStyleMask::Closable
                    | NSWindowStyleMask::FullSizeContentView,
            );
            window.center();
            let helper = super::onboarding_permissions::helper(&ui).map(Rc::new);
            // Poll on a worker; all native presentation and preferences reads stay on main.
            let poll = send.clone();
            std::thread::spawn(move || {
                while !poll.is_closed() {
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    if poll.send(0).is_err() {
                        break;
                    }
                }
            });
            loop {
                let trusted = ax_is_trusted();
                let spaces = separate_spaces_enabled();
                if trusted {
                    if let Some(panel) = &helper {
                        panel.hide();
                    }
                }
                let ready = trusted && spaces;
                if ready && recovery {
                    std::process::exit(0);
                }
                window.ns_window().setContentSize(CGSize::new(
                    560.,
                    if ready {
                        240.
                    } else if !trusted && !spaces {
                        520.
                    } else {
                        380.
                    },
                ));
                let mut content = VStack::new(&ui)
                    .spacing(20.)
                    .insets(cgs::Insets {
                        top: 48.,
                        left: 32.,
                        right: 32.,
                        bottom: 20.,
                    })
                    .push(
                        Label::new(
                            &ui,
                            if ready {
                                "Rift is ready"
                            } else {
                                "Set Up Rift"
                            },
                        )
                        .font(&cgs::style::Font::title()),
                    );
                if ready {
                    content = content.push(body(&ui, "Open Settings from the Rift menu bar icon to configure layouts, workspaces, shortcuts, and window behavior."));
                }
                if !trusted {
                    let floating = helper.clone();
                    let actions = HStack::new(&ui).spacing(12.)
                        .push(Button::new(&ui, "Open Accessibility Settings").push().on_click(move || {
                            open("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility");
                            if let Some(panel) = &floating { panel.show(); }
                        }))
                        .push(Button::new(&ui, "Show Rift in Finder").push().on_click(|| {
                            if let Ok(exe) = std::env::current_exe().and_then(std::fs::canonicalize) {
                                if let Some(bundle) = exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")) {
                                    let _ = std::process::Command::new("open").arg("-R").arg(bundle).spawn();
                                }
                            }
                        }));
                    content = content.push(Section::new(&ui, "Accessibility")
                        .content(body(&ui, "Allow Rift to move and resize windows. Enable Rift in the Accessibility list. If it’s missing, drag the app into the list or use the + button."))
                        .content(actions));
                }
                if !spaces {
                    content = content.push(Section::new(&ui, "Separate Spaces")
                        .content(body(&ui, "Turn on “Displays have separate Spaces” in Desktop & Dock → Mission Control. macOS may require you to log out before this takes effect."))
                        .content(Button::new(&ui, "Open Desktop & Dock").push().on_click(|| {
                            open("x-apple.systempreferences:com.apple.Desktop-Settings.extension");
                        })));
                }
                content = content.push(Spacer::new(&ui));
                let mut actions = HStack::new(&ui).spacing(12.).push(Spacer::new(&ui));
                if ready {
                    let settings = send.clone();
                    actions = actions.push(Button::new(&ui, "Open Settings…").push().on_click(
                        move || {
                            let _ = settings.send(4);
                        },
                    ));
                    let done = send.clone();
                    actions = actions.push(
                        Button::new(&ui, "Done").push().key_equivalent("\r").on_click(move || {
                            let _ = done.send(1);
                        }),
                    );
                } else {
                    let cancel = send.clone();
                    actions = actions.push(
                        Button::new(&ui, "Cancel").push().key_equivalent("\u{1b}").on_click(
                            move || {
                                let _ = cancel.send(-1);
                            },
                        ),
                    );
                }
                content = content.push(Divider::new(&ui)).push(actions);
                window.set_content(content);
                window.show();
                if let Some(introduction) = introduction.take() {
                    introduction.dismiss(&app).await;
                }
                app.activate();
                loop {
                    match receive.recv().await {
                        Some(-1) | None => std::process::exit(2),
                        Some(action @ (1 | 4)) if ready => {
                            if let Err(error) = complete() {
                                cgs::window::Alert::new(
                                    &ui,
                                    "Could not finish setup",
                                    &error.to_string(),
                                )
                                .button("OK")
                                .run_modal();
                                continue;
                            }
                            // The parent starts the daemon first; only the explicit button opens Settings.
                            std::process::exit(if action == 4 { 4 } else { 0 });
                        }
                        _ if ax_is_trusted() != trusted || separate_spaces_enabled() != spaces => {
                            break;
                        }
                        _ => {}
                    }
                }
            }
        });
        Ok(())
    })();
    if let Err(error) = result {
        eprintln!("Setup: {error}");
        std::process::exit(1);
    }
    std::process::exit(0)
}

fn complete() -> std::io::Result<()> { std::fs::write(state_dir().join("completed"), b"1") }

// Read the preference without opening a private WindowServer connection in the UI process.
fn separate_spaces_enabled() -> bool {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    unsafe {
        let defaults: *mut AnyObject = msg_send![class!(NSUserDefaults), standardUserDefaults];
        let domain: *mut AnyObject =
            msg_send![defaults, persistentDomainForName: &*NSString::from_str("com.apple.spaces")];
        if domain.is_null() {
            return true;
        }
        let value: *mut AnyObject =
            msg_send![domain, objectForKey: &*NSString::from_str("spans-displays")];
        value.is_null() || !msg_send![value, boolValue]
    }
}

fn body(ui: &Ui, text: &str) -> Label {
    Label::new(ui, text)
        .font(&cgs::style::Font::body())
        .color(&NSColor::secondaryLabelColor())
        .wrapping()
}
