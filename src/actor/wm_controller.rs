//! The WM Controller handles major events like enabling and disabling the
//! window manager on certain spaces and launching app threads. It also
//! controls hotkey registration.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;

use dispatchr::queue;
use dispatchr::time::Time;
use objc2::rc::Retained;
use objc2_app_kit::{NSApplicationActivationPolicy, NSRunningApplication};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json;
use strum::VariantNames;
use tracing::{debug, error, info, instrument, warn};

use crate::common::config::WorkspaceSelector;
use crate::sys::app::{NSRunningApplicationExt, pid_t};

pub type Sender = actor::Sender<WmEvent>;

type Receiver = actor::Receiver<WmEvent>;

use self::WmCmd::*;
use crate::actor::app::{AppInfo, AppThreadHandle, Request};
use crate::actor::spaces::ForwardedSpaceState;
use crate::actor::{self, config, input, mission_control, reactor};
use crate::model::tx_store::WindowTxStore;
use crate::sys::dispatch::DispatchExt;
use crate::sys::screen::CoordinateConverter;
use crate::{layout_engine as layout, sys};

#[derive(Debug)]
pub enum WmEvent {
    DiscoverRunningApps,
    AppEventsRegistered,
    AppLaunch(pid_t, AppInfo, Option<Retained<NSRunningApplication>>),
    AppGloballyActivated(pid_t),
    AppGloballyDeactivated(pid_t),
    AppTerminated(pid_t),
    AppExited(pid_t, AppThreadHandle),
    SpaceStateUpdated(ForwardedSpaceState, CoordinateConverter),
    PowerStateChanged(bool),
    KeyboardLayoutChanged,
    ConfigUpdated(crate::common::config::Config),
    Command(WmCommand),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum WmCommand {
    Wm(WmCmd),
    ConfiguredLayout(ConfiguredLayoutCommand),
    ReactorCommand(reactor::Command),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ConfiguredLayoutCommand {
    ToggleWindowFloating(rift_protocol::ToggleWindowFloatingOptions),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, strum_macros::VariantNames)]
#[serde(rename_all = "snake_case")]
pub enum WmCmd {
    ToggleSpaceActivated,
    Exec(ExecCmd),
    ReloadConfig,

    NextWorkspace,
    PrevWorkspace,
    SwitchToWorkspace(WorkspaceSelector),
    MoveWindowToWorkspace(WorkspaceSelector),
    CreateWorkspace,
    SwitchToLastWorkspace,

    ShowMissionControlAll,
    ShowMissionControlCurrent,
    DismissMissionControl,
    CloseWindow,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum ExecCmd {
    String(String),
    Array(Vec<String>),
}

static BUILTIN_WM_CMD_VARIANTS: Lazy<Vec<String>> = Lazy::new(|| {
    WmCmd::VARIANTS
        .iter()
        .map(|v| {
            let mut out = String::with_capacity(v.len());
            for (i, ch) in v.chars().enumerate() {
                if ch.is_uppercase() {
                    if i != 0 {
                        out.push('_');
                    }
                    for lc in ch.to_lowercase() {
                        out.push(lc);
                    }
                } else {
                    out.push(ch);
                }
            }
            out
        })
        .collect()
});

impl WmCmd {
    pub fn snake_case_variants() -> &'static [String] { &BUILTIN_WM_CMD_VARIANTS }
}

pub struct Config {
    pub restore_file: PathBuf,
    pub config: crate::common::config::Config,
}

pub struct WmController {
    config: Config,
    config_tx: config::Sender,
    events_tx: reactor::Sender,
    input_tx: input::Sender,
    stack_line_tx: Option<crate::actor::stack_line::Sender>,
    mission_control_tx: Option<mission_control::Sender>,
    window_tx_store: Option<WindowTxStore>,
    receiver: Receiver,
    sender: Sender,
    hotkeys_installed: bool,
    apps: AppLifecycle,
}

// Reserve before spawning; retain the reservation until AX resources are dropped.
#[derive(Default)]
struct AppLifecycle(HashMap<pid_t, (AppThreadHandle, AppPhase)>);

enum AppPhase {
    Active, // Includes initialization.
    Stopping(Option<AppInfo>),
}

impl AppLifecycle {
    fn reserve(
        &mut self,
        pid: pid_t,
        info: AppInfo,
    ) -> Option<(AppThreadHandle, actor::Receiver<Request>)> {
        if let Some((_, phase)) = self.0.get_mut(&pid) {
            if let AppPhase::Stopping(relaunch) = phase {
                *relaunch = Some(info);
            }
            return None;
        }
        let (handle, rx) = AppThreadHandle::channel();
        self.0.insert(pid, (handle.clone(), AppPhase::Active));
        Some((handle, rx))
    }

    fn terminate(&mut self, pid: pid_t) {
        if let Some((handle, phase)) = self.0.get_mut(&pid) {
            *phase = AppPhase::Stopping(None);
            _ = handle.send(Request::Terminate);
        }
    }

    fn exited(&mut self, pid: pid_t, handle: &AppThreadHandle) -> Option<Option<AppInfo>> {
        if !self.0.get(&pid)?.0.same_actor(handle) {
            return None;
        }
        Some(match self.0.remove(&pid)?.1 {
            AppPhase::Stopping(info) => info,
            _ => None,
        })
    }
}

impl WmController {
    pub fn new(
        config: Config,
        config_tx: config::Sender,
        events_tx: reactor::Sender,
        input_tx: input::Sender,
        stack_line_tx: crate::actor::stack_line::Sender,
        mission_control_tx: crate::actor::mission_control::Sender,
        window_tx_store: Option<WindowTxStore>,
    ) -> (Self, actor::Sender<WmEvent>) {
        let (sender, receiver) = actor::channel();
        sys::app::set_application_callback({
            let sender = sender.clone();
            move |pid, info| sender.send(WmEvent::AppLaunch(pid, info, None))
        });
        let this = Self {
            config,
            config_tx,
            events_tx,
            input_tx,
            stack_line_tx: Some(stack_line_tx),
            mission_control_tx: Some(mission_control_tx),
            window_tx_store,
            receiver,
            sender: sender.clone(),
            hotkeys_installed: false,
            apps: AppLifecycle::default(),
        };
        (this, sender)
    }

    pub async fn run(mut self) {
        while let Some((span, event)) = self.receiver.recv().await {
            let _guard = span.enter();
            self.handle_event(event);
        }
    }

    #[instrument(name = "wm_controller::handle_event", skip(self))]
    pub fn handle_event(&mut self, event: WmEvent) {
        debug!("handle_event");
        use reactor::Event;

        use self::WmCommand::*;
        use self::WmEvent::*;

        if matches!(
            event,
            Command(Wm(crate::actor::wm_controller::WmCmd::NextWorkspace))
                | Command(Wm(crate::actor::wm_controller::WmCmd::PrevWorkspace))
                | Command(Wm(crate::actor::wm_controller::WmCmd::SwitchToWorkspace(_)))
                | Command(Wm(crate::actor::wm_controller::WmCmd::SwitchToLastWorkspace))
                | SpaceStateUpdated(..)
        ) && let Some(tx) = &self.mission_control_tx
        {
            tx.send(mission_control::Event::RefreshCurrentWorkspace);
        }

        match event {
            SpaceStateUpdated(space_state, converter) => {
                self.events_tx.send(Event::SpaceStateChanged(space_state.clone()));
                _ = self
                    .input_tx
                    .send(input::Request::SpaceStateUpdated(space_state.clone(), converter));
                if let Some(tx) = &self.stack_line_tx {
                    _ = tx.try_send(crate::actor::stack_line::Event::SpaceStateUpdated(
                        converter,
                        space_state,
                    ));
                }
            }
            AppEventsRegistered => {
                _ = self.input_tx.send(input::Request::SetEventProcessing(false));

                if !self.hotkeys_installed {
                    self.register_hotkeys();
                    self.hotkeys_installed = true;
                }

                let sender = self.sender.clone();
                let input_tx = self.input_tx.clone();
                queue::main().after_f_s(
                    Time::new_after(Time::NOW, 250 * 1000000),
                    (sender, WmEvent::DiscoverRunningApps),
                    |(sender, event)| sender.send(event),
                );

                queue::main().after_f_s(
                    Time::new_after(Time::NOW, (250 + 350) * 1000000),
                    (input_tx, input::Request::SetEventProcessing(true)),
                    |(sender, event)| sender.send(event),
                );
            }
            DiscoverRunningApps => {
                for (pid, info) in sys::app::running_apps(None) {
                    self.new_app(pid, info, None);
                }
            }
            AppLaunch(pid, info, running_app) => {
                self.new_app(pid, info, running_app);
            }
            AppGloballyActivated(pid) => {
                _ = self.input_tx.send(input::Request::EnforceHidden);
                self.events_tx.send(Event::ApplicationGloballyActivated(pid));
            }
            AppGloballyDeactivated(pid) => {
                self.events_tx.send(Event::ApplicationGloballyDeactivated(pid));
            }
            AppTerminated(pid) => {
                sys::app::remove_application_observer(pid);
                self.apps.terminate(pid);
            }
            AppExited(pid, handle) => {
                if let Some(relaunch) = self.apps.exited(pid, &handle) {
                    self.events_tx.send(Event::AppActorExited(pid, handle));
                    if let Some(info) = relaunch {
                        self.new_app(pid, info, None);
                    }
                }
            }
            ConfigUpdated(new_cfg) => {
                let old_keys_ser = serde_json::to_string(&self.config.config.keys).ok();

                self.config.config = new_cfg;

                _ = self.input_tx.send(input::Request::ConfigUpdated(self.config.config.clone()));

                if !self.hotkeys_installed {
                    debug!(
                        "hotkeys not yet installed; deferring hotkey update until AppEventsRegistered"
                    );
                    return;
                }

                if let Some(old_ser) = old_keys_ser {
                    if serde_json::to_string(&self.config.config.keys).ok().as_deref()
                        != Some(&old_ser)
                    {
                        debug!("hotkey bindings changed; reloading hotkeys");
                        self.register_hotkeys();
                    } else {
                        debug!("hotkey bindings unchanged; skipping reload");
                    }
                } else {
                    debug!("could not compare hotkey bindings; reloading hotkeys");
                    self.register_hotkeys();
                }
            }
            PowerStateChanged(is_low_power_mode) => {
                info!("Power state changed: low power mode = {}", is_low_power_mode);
                _ = self.input_tx.send(input::Request::SetLowPowerMode(is_low_power_mode));
            }
            KeyboardLayoutChanged => {
                _ = self.input_tx.send(input::Request::KeyboardLayoutChanged);
            }
            Command(Wm(ReloadConfig)) => self.reload_config(),
            Command(Wm(crate::actor::wm_controller::WmCmd::ToggleSpaceActivated)) => {
                self.events_tx.send(reactor::Event::Command(reactor::Command::Reactor(
                    reactor::ReactorCommand::ToggleSpaceActivated,
                )));
            }
            Command(Wm(NextWorkspace)) => {
                self.events_tx.send(reactor::Event::Command(reactor::Command::Layout(
                    layout::LayoutCommand::NextWorkspace(None),
                )));
            }
            Command(Wm(PrevWorkspace)) => {
                self.events_tx.send(reactor::Event::Command(reactor::Command::Layout(
                    layout::LayoutCommand::PrevWorkspace(None),
                )));
            }
            Command(Wm(SwitchToWorkspace(ws_sel))) => {
                let maybe_index: Option<usize> = match &ws_sel {
                    WorkspaceSelector::Index(i) => Some(*i),
                    WorkspaceSelector::Name(name) => self
                        .config
                        .config
                        .virtual_workspaces
                        .workspace_names
                        .iter()
                        .position(|n| n == name),
                };

                if let Some(workspace_index) = maybe_index {
                    self.events_tx.send(reactor::Event::Command(reactor::Command::Layout(
                        layout::LayoutCommand::SwitchToWorkspace(workspace_index),
                    )));
                } else {
                    tracing::warn!(
                        "Hotkey requested switch to workspace {:?} but it could not be resolved; ignoring",
                        ws_sel
                    );
                }
            }
            Command(Wm(MoveWindowToWorkspace(workspace))) => {
                self.events_tx.send(reactor::Event::Command(reactor::Command::Layout(
                    layout::LayoutCommand::MoveWindowToWorkspace {
                        workspace,
                        follow: false,
                        window_id: None,
                    },
                )));
            }
            Command(Wm(CreateWorkspace)) => {
                self.events_tx.send(reactor::Event::Command(reactor::Command::Layout(
                    layout::LayoutCommand::CreateWorkspace,
                )));
            }
            Command(Wm(SwitchToLastWorkspace)) => {
                self.events_tx.send(reactor::Event::Command(reactor::Command::Layout(
                    layout::LayoutCommand::SwitchToLastWorkspace,
                )));
            }
            Command(Wm(ShowMissionControlAll)) => {
                if let Some(tx) = &self.mission_control_tx {
                    let _ = tx.try_send(mission_control::Event::ShowAll);
                }
            }
            Command(Wm(ShowMissionControlCurrent)) => {
                if let Some(tx) = &self.mission_control_tx {
                    let _ = tx.try_send(mission_control::Event::ShowCurrent);
                }
            }
            Command(Wm(DismissMissionControl)) => {
                if let Some(tx) = &self.mission_control_tx {
                    let _ = tx.try_send(mission_control::Event::Dismiss);
                }
            }
            Command(Wm(CloseWindow)) => {
                self.events_tx.send(reactor::Event::Command(reactor::Command::Reactor(
                    reactor::ReactorCommand::CloseWindow { window_server_id: None },
                )));
            }
            Command(Wm(Exec(cmd))) => {
                self.exec_cmd(cmd);
            }
            Command(ConfiguredLayout(ConfiguredLayoutCommand::ToggleWindowFloating(options))) => {
                self.events_tx.send(reactor::Event::Command(reactor::Command::Layout(
                    layout::LayoutCommand::ToggleWindowFloatingWithOptions(options),
                )));
            }
            Command(ReactorCommand(cmd)) => {
                self.events_tx.send(reactor::Event::Command(cmd));
            }
        }
    }

    fn new_app(
        &mut self,
        pid: pid_t,
        info: AppInfo,
        running_app: Option<Retained<NSRunningApplication>>,
    ) {
        let running_app = match running_app {
            Some(app) => app,
            None => {
                let Some(app) = NSRunningApplication::with_process_id(pid) else {
                    debug!(?pid, "Failed to resolve NSRunningApplication for new app");
                    return;
                };
                app
            }
        };

        if running_app.activationPolicy() != NSApplicationActivationPolicy::Regular
            && info.bundle_id.as_deref() != Some("com.apple.loginwindow")
        {
            sys::app::ensure_activation_policy_observer(running_app.clone(), info.clone());
            debug!(
                pid = ?pid,
                bundle = ?info.bundle_id,
                "App not yet regular; deferring spawn until activation policy changes"
            );

            if running_app.activationPolicy() == NSApplicationActivationPolicy::Regular {
                sys::app::remove_activation_policy_observer(pid);
            } else {
                return;
            }
        }

        if !running_app.isFinishedLaunching() {
            sys::app::ensure_finished_launching_observer(running_app.clone(), info.clone());
            debug!(
                pid = ?pid,
                bundle = ?info.bundle_id,
                "App has not finished launching; deferring spawn until finished"
            );

            if running_app.isFinishedLaunching() {
                sys::app::remove_finished_launching_observer(pid);
            } else {
                return;
            }
        }

        if let Some((handle, rx)) = self.apps.reserve(pid, info.clone()) {
            actor::app::spawn_app_thread(
                pid,
                info,
                self.events_tx.clone(),
                self.window_tx_store.clone(),
                self.sender.clone(),
                handle,
                rx,
            );
        }
    }

    fn register_hotkeys(&mut self) {
        debug!("register_hotkeys");
        let bindings: Vec<(String, WmCommand)> =
            self.config.config.key_specs.iter().cloned().collect();
        _ = self.input_tx.send(input::Request::SetHotkeys(bindings));
    }

    fn reload_config(&self) {
        let (response, _result) = std::sync::mpsc::sync_channel(1);
        let msg = config::Event::ApplyConfig {
            cmd: crate::common::config::ConfigCommand::ReloadConfig,
            response,
        };
        if let Err(e) = self.config_tx.try_send(msg) {
            let error_message = e.to_string();
            error!("Failed to request config reload: {error_message}");
        }
    }

    fn exec_cmd(&self, cmd_args: ExecCmd) {
        std::thread::spawn(move || {
            let cmd_args = cmd_args.as_array();
            let [cmd, args @ ..] = &*cmd_args else {
                error!("Empty argument list passed to exec");
                return;
            };
            let output = std::process::Command::new(cmd).args(args).output();
            let output = match output {
                Ok(o) => o,
                Err(e) => {
                    error!("Failed to execute command {cmd:?}: {e:?}");
                    return;
                }
            };
            if !output.status.success() {
                error!(
                    "Exec command exited with status {}: {cmd:?} {args:?}",
                    output.status
                );
                error!("stdout: {}", String::from_utf8_lossy(&*output.stdout));
                error!("stderr: {}", String::from_utf8_lossy(&*output.stderr));
            }
        });
    }
}

impl ExecCmd {
    fn as_array(&self) -> Cow<'_, [String]> {
        match self {
            ExecCmd::Array(vec) => Cow::Borrowed(&*vec),
            ExecCmd::String(s) => s.split(' ').map(|s| s.to_owned()).collect::<Vec<_>>().into(),
        }
    }
}

#[cfg(test)]
mod app_lifecycle_tests {
    use super::*;

    #[test]
    fn dedupe_failure_termination_and_reuse() {
        for terminated in [false, true] {
            let mut apps = AppLifecycle::default();
            let info = || AppInfo {
                bundle_id: None,
                localized_name: None,
            };
            let (old, mut rx) = apps.reserve(42, info()).unwrap();
            assert!(apps.reserve(42, info()).is_none());
            if terminated {
                apps.terminate(42);
                assert!(matches!(rx.try_recv().unwrap().1, Request::Terminate));
                assert!(apps.reserve(42, info()).is_none());
            }
            assert!(apps.exited(42, &old).is_some());
            let (new, _rx) = apps.reserve(42, info()).unwrap();
            assert!(apps.exited(42, &old).is_none());
            assert!(!new.same_actor(&old));
            assert!(apps.reserve(42, info()).is_none());
        }
    }
}
