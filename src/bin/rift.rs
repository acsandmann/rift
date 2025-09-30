use std::path::PathBuf;

use clap::Parser;
use objc2::MainThreadMarker;
use rift_wm::actor::config::ConfigActor;
use rift_wm::actor::config_watcher::ConfigWatcher;
use rift_wm::actor::menu_bar::Menu;
use rift_wm::actor::mission_control::MissionControlActor;
use rift_wm::actor::mouse::Mouse;
use rift_wm::actor::notification_center::NotificationCenter;
use rift_wm::actor::reactor::{self, Reactor};
use rift_wm::actor::stack_line::StackLine;
use rift_wm::actor::swipe::Swipe;
use rift_wm::actor::window_notify as window_notify_actor;
use rift_wm::actor::wm_controller::{self, WmController};
use rift_wm::common::config::{Config, config_file, restore_file};
use rift_wm::common::log;
use rift_wm::common::util::execute_startup_commands;
use rift_wm::ipc;
use rift_wm::layout_engine::LayoutEngine;
use rift_wm::sys::accessibility::ensure_accessibility_permission;
use rift_wm::sys::executor::Executor;
use rift_wm::sys::screen::CoordinateConverter;
use rift_wm::sys::skylight::{CGSEventType, KnownCGSEvent};
use tokio::join;

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

    let mtm = MainThreadMarker::new().unwrap();
    {
        use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
        let app = NSApplication::sharedApplication(mtm);
        let _ = app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        unsafe {
            let _: () = objc2::msg_send![&*app, finishLaunching];
        }
        unsafe { NSApplication::load() };
    }

    ensure_accessibility_permission();

    let mut config = if config_file().exists() {
        Config::read(&config_file()).unwrap()
    } else {
        Config::default()
    };
    config.settings.animate &= !opt.no_animate;
    config.settings.default_disable |= opt.default_disable;

    if opt.validate {
        LayoutEngine::load(restore_file()).unwrap();
        return;
    }

    execute_startup_commands(&config.settings.run_on_start);

    let (broadcast_tx, broadcast_rx) = rift_wm::actor::channel();

    let layout = if opt.restore {
        LayoutEngine::load(restore_file()).unwrap()
    } else {
        LayoutEngine::new(
            &config.virtual_workspaces,
            &config.settings.layout,
            Some(broadcast_tx.clone()),
        )
    };
    let (mouse_tx, mouse_rx) = rift_wm::actor::channel();
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

    let config_tx = ConfigActor::spawn(config.clone(), events_tx.clone());

    ConfigWatcher::spawn(config_tx.clone());

    let (_wnd_tx, wnd_rx) = rift_wm::actor::channel();
    let wn_actor = window_notify_actor::WindowNotify::new(events_tx.clone(), wnd_rx, &[
        CGSEventType::Known(KnownCGSEvent::WindowDestroyed),
        CGSEventType::Known(KnownCGSEvent::WindowCreated),
    ]);

    let events_tx_mach = events_tx.clone();
    let server_state = ipc::run_mach_server(events_tx_mach, config_tx.clone());

    let mach_bridge_rx = broadcast_rx;

    let server_state_for_bridge = server_state.clone();
    std::thread::spawn(move || {
        let mut rx = mach_bridge_rx;
        let server_state = server_state_for_bridge;
        loop {
            match rx.blocking_recv() {
                Some((_span, event)) => {
                    let state = server_state.read();
                    state.publish(event);
                }
                None => {
                    break;
                }
            }
        }
    });

    let wm_config = wm_controller::Config {
        one_space: opt.one,
        restore_file: restore_file(),
        config: config.clone(),
    };
    let (mc_tx, mc_rx) = rift_wm::actor::channel();
    let (wm_controller, wm_controller_sender) = WmController::new(
        wm_config,
        events_tx.clone(),
        mouse_tx.clone(),
        stack_line_tx.clone(),
        mc_tx.clone(),
    );
    let notification_center = NotificationCenter::new(wm_controller_sender.clone());

    let mouse = Mouse::new(config.clone(), events_tx.clone(), mouse_rx);
    let menu = Menu::new(config.clone(), menu_rx, mtm);
    let stack_line = StackLine::new(
        config.clone(),
        stack_line_rx,
        mtm,
        events_tx.clone(),
        CoordinateConverter::default(),
    );

    let mission_control = MissionControlActor::new(config.clone(), mc_rx, events_tx.clone(), mtm);

    println!(
        "NOTICE: by default rift starts in a deactivated state.
        you must activate it by using the toggle_spaces_activated command.
        by default this is bound to Alt+Z but can be changed in the config file."
    );

    let (_swipe_tx, swipe_rx) = rift_wm::actor::channel();
    let swipe = Swipe::new(config.clone(), wm_controller_sender.clone(), swipe_rx);

    let _executor_session = Executor::start(async move {
        if let Some(swipe) = swipe {
            join!(
                wm_controller.run(),
                notification_center.watch_for_notifications(),
                mouse.run(),
                menu.run(),
                stack_line.run(),
                wn_actor.run(),
                swipe.run(),
                mission_control.run(),
            );
        } else {
            join!(
                wm_controller.run(),
                notification_center.watch_for_notifications(),
                mouse.run(),
                menu.run(),
                stack_line.run(),
                wn_actor.run(),
                mission_control.run(),
            );
        }
    });

    let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
    unsafe {
        let _: () = objc2::msg_send![&*app, run];
    }
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
