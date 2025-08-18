use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use objc2::MainThreadMarker;
use rift_wm::actor::menu_bar::Menu;
use rift_wm::actor::mouse::{self, Mouse};
use rift_wm::actor::notification_center::NotificationCenter;
use rift_wm::actor::reactor::{self, Reactor};
use rift_wm::actor::stack_line::StackLine;
use rift_wm::actor::wm_controller::{self, WmController};
use rift_wm::common::config::{Config, config_file, restore_file};
use rift_wm::common::log;
use rift_wm::layout_engine::LayoutEngine;
use rift_wm::server;
use rift_wm::sys::executor::Executor;
use rift_wm::sys::screen::CoordinateConverter;
use rift_wm::sys::window_notify::{self, take_receiver};
use tokio::join;
use tracing::{error, trace};

/// Execute startup commands from configuration
fn execute_startup_commands(commands: &[String]) {
    if commands.is_empty() {
        return;
    }

    trace!("Executing {} startup commands", commands.len());

    for (i, command) in commands.iter().enumerate() {
        trace!("Executing startup command {}: {}", i + 1, command);

        // Parse the command properly handling quotes
        let parts = parse_command(command);
        if parts.is_empty() {
            error!("Empty startup command at index {}", i);
            continue;
        }

        let (cmd, args) = parts.split_first().unwrap();

        let cmd_owned = cmd.to_string();
        let args_owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let command_str = command.clone();

        std::thread::spawn(move || {
            let output = std::process::Command::new(&cmd_owned).args(&args_owned).output();

            match output {
                Ok(output) => {
                    if output.status.success() {
                        trace!("Startup command completed successfully: {}", command_str);
                    } else {
                        error!(
                            "Startup command failed with status {}: {}",
                            output.status, command_str
                        );
                        if !output.stderr.is_empty() {
                            error!("stderr: {}", String::from_utf8_lossy(&output.stderr));
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to execute startup command '{}': {}", command_str, e);
                }
            }
        });
    }
}

fn parse_command(command: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current_part = String::new();
    let mut in_quotes = false;
    let mut chars = command.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '\'' | '"' => {
                if in_quotes {
                    in_quotes = false;
                } else {
                    in_quotes = true;
                }
            }
            ' ' | '\t' if !in_quotes => {
                if !current_part.is_empty() {
                    parts.push(current_part.clone());
                    current_part.clear();
                }
            }
            '\\' if in_quotes => {
                if let Some(next_ch) = chars.next() {
                    match next_ch {
                        'n' => current_part.push('\n'),
                        't' => current_part.push('\t'),
                        'r' => current_part.push('\r'),
                        '\\' => current_part.push('\\'),
                        '\'' => current_part.push('\''),
                        '"' => current_part.push('"'),
                        _ => {
                            current_part.push('\\');
                            current_part.push(next_ch);
                        }
                    }
                } else {
                    current_part.push('\\');
                }
            }
            _ => {
                current_part.push(ch);
            }
        }
    }

    if !current_part.is_empty() {
        parts.push(current_part);
    }

    parts
}

#[derive(Parser)]
struct Cli {
    /// Only run the window manager on the current space.
    #[arg(long)]
    one: bool,

    /// Disable new spaces by default.
    ///
    /// Ignored if --one is used.
    #[arg(long)]
    default_disable: bool,

    /// Disable animations.
    #[arg(long)]
    no_animate: bool,

    /// Check whether the restore file can be loaded without actually starting
    /// the window manager.
    #[arg(long)]
    validate: bool,

    /// Restore the configuration saved with the save_and_exit command. This is
    /// only useful within the same session.
    #[arg(long)]
    restore: bool,

    /// Record reactor events to the specified file path. Overwrites the file if
    /// exists.
    #[arg(long)]
    record: Option<PathBuf>,
}

fn main() {
    let opt: Cli = Parser::parse();

    if std::env::var_os("RUST_BACKTRACE").is_none() {
        // SAFETY: We are single threaded at this point.
        unsafe { std::env::set_var("RUST_BACKTRACE", "1") };
    }
    log::init_logging();
    install_panic_hook();

    let _ = window_notify::init(window_notify::CGSEventType::WINDOW_DESTROYED as u32);

    let mut config = if config_file().exists() {
        Config::read(&config_file()).unwrap()
    } else {
        Config::default()
    };
    config.settings.animate &= !opt.no_animate;
    config.settings.default_disable |= opt.default_disable;
    let config = Arc::new(config);

    if opt.validate {
        LayoutEngine::load(restore_file()).unwrap();
        return;
    }

    execute_startup_commands(&config.settings.run_on_start);

    let (broadcast_tx, _broadcast_rx) = tokio::sync::broadcast::channel(256);

    let layout = if opt.restore {
        LayoutEngine::load(restore_file()).unwrap()
    } else {
        LayoutEngine::new(
            &config.virtual_workspaces,
            &config.settings.layout,
            Some(broadcast_tx.clone()),
        )
    };
    let (mouse_tx, mouse_rx) = mouse::channel();
    let (menu_tx, menu_rx) = rift_wm::actor::channel();
    let (stack_line_tx, stack_line_rx) = rift_wm::actor::channel();
    let events_tx = Reactor::spawn(
        config.clone(),
        layout,
        reactor::Record::new(opt.record.as_deref()),
        mouse_tx.clone(),
        broadcast_tx.clone(),
        menu_tx.clone(),
        stack_line_tx.clone(),
    );

    {
        let mut rx = take_receiver(window_notify::CGSEventType::WINDOW_DESTROYED as u32);
        let events_tx_clone = events_tx.clone();
        std::thread::spawn(move || {
            loop {
                match rx.blocking_recv() {
                    Some(event) => {
                        if let Some(window_id) = event.window_id {
                            let _ = events_tx_clone.send((
                                tracing::Span::current(),
                                rift_wm::actor::reactor::Event::WindowServerDestroyed(
                                    rift_wm::sys::window_server::WindowServerId::new(window_id),
                                ),
                            ));
                        }
                    }
                    None => {
                        // The sender has been dropped, exit the loop.
                        break;
                    }
                }
            }
        });
    }

    let events_tx_mach = events_tx.clone();
    std::thread::spawn(move || {
        server::run_mach_server(events_tx_mach);
    });

    let mach_bridge_rx = broadcast_tx.subscribe();
    std::thread::spawn(move || {
        let mut rx = mach_bridge_rx;
        loop {
            match rx.blocking_recv() {
                Ok(event) => {
                    crate::server::forward_broadcast_event(event);
                }
                Err(_) => {
                    break;
                }
            }
        }
    });

    let mtm = MainThreadMarker::new().unwrap();
    {
        use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
        let app = NSApplication::sharedApplication(mtm);
        let _ = app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        unsafe {
            let _: () = objc2::msg_send![&*app, finishLaunching];
        }
    }
    let wm_config = wm_controller::Config {
        one_space: opt.one,
        restore_file: restore_file(),
        config: config.clone(),
    };
    let (wm_controller, wm_controller_sender) = WmController::new(
        wm_config,
        events_tx.clone(),
        mouse_tx.clone(),
        stack_line_tx.clone(),
    );
    let notification_center = NotificationCenter::new(wm_controller_sender.clone());

    let mouse = Mouse::new(config.clone(), events_tx.clone(), mouse_rx);
    let menu = Menu::new(config.clone(), menu_rx, mtm);
    let stack_line = StackLine::new(
        config.clone(),
        stack_line_rx,
        mtm,
        events_tx,
        CoordinateConverter::default(),
    );

    Executor::run(async move {
        join!(
            wm_controller.run(),
            notification_center.watch_for_notifications(),
            mouse.run(),
            menu.run(),
            stack_line.run(),
        );
    });
}

#[cfg(panic = "unwind")]
fn install_panic_hook() {
    // Abort on panic instead of propagating panics to the main thread.
    // See Cargo.toml for why we don't use panic=abort everywhere.
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        original_hook(info);
        std::process::abort();
    }));
}

#[cfg(not(panic = "unwind"))]
fn install_panic_hook() {}
