use std::path::PathBuf;
use std::process;

use clap::{Parser, Subcommand};
use objc2::MainThreadMarker;
use objc2_application_services::AXUIElement;
use rift_wm::actor::config::ConfigActor;
use rift_wm::actor::config_watcher::ConfigWatcher;
use rift_wm::actor::event_tap::EventTap;
use rift_wm::actor::menu_bar::Menu;
use rift_wm::actor::mission_control::MissionControlActor;
use rift_wm::actor::mission_control_observer::NativeMissionControl;
use rift_wm::actor::notification_center::NotificationCenter;
use rift_wm::actor::process::ProcessActor;
use rift_wm::actor::reactor::{self, Reactor};
use rift_wm::actor::stack_line::StackLine;
use rift_wm::actor::window_notify as window_notify_actor;
use rift_wm::actor::wm_controller::{self, WmController};
use rift_wm::common::config::{Config, config_file, restore_file};
use rift_wm::common::log;
use rift_wm::common::util::execute_startup_commands;
use rift_wm::ipc;
use rift_wm::layout_engine::LayoutEngine;
use rift_wm::model::tx_store::WindowTxStore;
use rift_wm::sys::accessibility::ensure_accessibility_permission;
use rift_wm::sys::executor::Executor;
use rift_wm::sys::screen::{CoordinateConverter, displays_have_separate_spaces};
use rift_wm::sys::service::{ServiceCommands, handle_service_command};
use rift_wm::sys::skylight::{CGSEventType, KnownCGSEvent};
use tokio::join;

embed_plist::embed_info_plist!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/Info.plist"));

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

    /// Path to configuration file to use (overrides default).
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Manage the launchd service for rift
    Service {
        #[command(subcommand)]
        service: ServiceCommands,
    },
}

fn main() {
    sigpipe::reset();
    let opt = Cli::parse();

    if let Some(Commands::Service { service }) = &opt.command {
        match handle_service_command(service) {
            Ok(msg) => {
                println!("{}", msg);
                process::exit(0);
            }
            Err(e) => {
                eprintln!("{}", e);
                process::exit(1);
            }
        }
    }

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
        app.finishLaunching();
        NSApplication::load();
    }

    ensure_accessibility_permission();

    if !displays_have_separate_spaces() {
        eprintln!(
            "Rift detected that the macOS setting \"Displays have separate Spaces\" \
is disabled. Rift currently requires this setting to be enabled. \
Enable it in System Settings > Desktop & Dock (Mission Control) and restart Rift."
        );
        std::process::exit(1);
    }

    let config_path = opt.config.clone().unwrap_or_else(|| config_file());
    let mut config = if config_path.exists() {
        Config::read(&config_path).unwrap()
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
    let (event_tap_tx, event_tap_rx) = rift_wm::actor::channel();
    let (menu_tx, menu_rx) = rift_wm::actor::channel();
    let (stack_line_tx, stack_line_rx) = rift_wm::actor::channel();
    let (wnd_tx, wnd_rx) = rift_wm::actor::channel();
    let window_tx_store = WindowTxStore::new();
    let events_tx = Reactor::spawn(
        config.clone(),
        layout,
        reactor::Record::new(opt.record.as_deref()),
        event_tap_tx.clone(),
        broadcast_tx.clone(),
        menu_tx.clone(),
        stack_line_tx.clone(),
        Some((wnd_tx.clone(), window_tx_store.clone())),
        opt.one,
    );

    let config_tx =
        ConfigActor::spawn_with_path(config.clone(), events_tx.clone(), config_path.clone());

    ConfigWatcher::spawn(config_tx.clone(), config.clone(), config_path.clone());

    let wn_actor = window_notify_actor::WindowNotify::new(
        events_tx.clone(),
        wnd_rx,
        &[
            CGSEventType::Known(KnownCGSEvent::SpaceWindowDestroyed),
            CGSEventType::Known(KnownCGSEvent::SpaceWindowCreated),
            //CGSEventType::Known(KnownCGSEvent::WindowMoved),
            //CGSEventType::Known(KnownCGSEvent::WindowResized),
        ],
        Some(window_tx_store.clone()),
    );

    let events_tx_mach = events_tx.clone();
    let server_state = match ipc::run_mach_server(events_tx_mach, config_tx.clone()) {
        Ok(state) => state,
        Err(err) => {
            eprintln!("{}", err);
            process::exit(1);
        }
    };

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
        restore_file: restore_file(),
        config: config.clone(),
    };
    let (mc_tx, mc_rx) = rift_wm::actor::channel();
    let (_mc_native_tx, mc_native_rx) = rift_wm::actor::channel();
    let (wm_controller, wm_controller_sender) = WmController::new(
        wm_config,
        events_tx.clone(),
        event_tap_tx.clone(),
        stack_line_tx.clone(),
        mc_tx.clone(),
        Some(window_tx_store.clone()),
    );

    let _ = events_tx.send(reactor::Event::RegisterWmSender(wm_controller_sender.clone()));

    let notification_center = NotificationCenter::new(wm_controller_sender.clone());

    let process_actor = ProcessActor::new(wm_controller_sender.clone());

    let event_tap = EventTap::new(
        config.clone(),
        events_tx.clone(),
        event_tap_rx,
        Some(wm_controller_sender.clone()),
    );
    let menu = Menu::new(config.clone(), menu_rx, mtm);
    let stack_line = StackLine::new(
        config.clone(),
        stack_line_rx,
        mtm,
        events_tx.clone(),
        CoordinateConverter::default(),
    );

    let mission_control = MissionControlActor::new(config.clone(), mc_rx, events_tx.clone(), mtm);
    let mission_control_native = NativeMissionControl::new(events_tx.clone(), mc_native_rx);

    println!(
        "NOTICE: by default rift starts in a deactivated state.
        you must activate it by using the toggle_spaces_activated command.
        by default this is bound to Alt+Z but can be changed in the config file."
    );

    unsafe { AXUIElement::new_system_wide().set_messaging_timeout(1.0) };

    let _executor_session = Executor::run_main(mtm, async move {
        join!(
            wm_controller.run(),
            notification_center.watch_for_notifications(),
            event_tap.run(),
            menu.run(),
            stack_line.run(),
            wn_actor.run(),
            mission_control_native.run(),
            mission_control.run(),
            process_actor.run()
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
