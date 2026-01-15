//! The WM Controller handles major events like enabling and disabling the
//! window manager on certain spaces and launching app threads. It also
//! controls hotkey registration.

use std::borrow::Cow;
use std::path::PathBuf;

use dispatchr::queue;
use dispatchr::time::Time;
use objc2_app_kit::{NSApplicationActivationPolicy, NSRunningApplication};
use objc2_core_foundation::CGRect;
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
use crate::actor::app::AppInfo;
use crate::actor::{self, event_tap, mission_control, reactor};
use crate::common::collections::HashSet;
use crate::model::tx_store::WindowTxStore;
use crate::sys::dispatch::DispatchExt;
use crate::sys::event::Hotkey;
use crate::sys::screen::{CoordinateConverter, ScreenDescriptor, SpaceId};
use crate::sys::window_server::WindowServerInfo;
use crate::{layout_engine as layout, sys};

#[derive(Debug)]
pub enum WmEvent {
    DiscoverRunningApps,
    AppEventsRegistered,
    AppLaunch(pid_t, AppInfo),
    AppGloballyActivated(pid_t),
    AppGloballyDeactivated(pid_t),
    AppTerminated(pid_t),
    DisplayChurnBegin,
    DisplayChurnEnd,
    SpaceChanged(Vec<Option<SpaceId>>),
    ScreenParametersChanged(Vec<ScreenDescriptor>, CoordinateConverter, Vec<Option<SpaceId>>),
    SystemWoke,
    PowerStateChanged(bool),
    ConfigUpdated(crate::common::config::Config),
    Command(WmCommand),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum WmCommand {
    Wm(WmCmd),
    ReactorCommand(reactor::Command),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, strum_macros::VariantNames)]
#[serde(rename_all = "snake_case")]
pub enum WmCmd {
    ToggleSpaceActivated,
    Exec(ExecCmd),

    NextWorkspace,
    PrevWorkspace,
    SwitchToWorkspace(WorkspaceSelector),
    MoveWindowToWorkspace(WorkspaceSelector),
    CreateWorkspace,
    SwitchToLastWorkspace,

    ShowMissionControlAll,
    ShowMissionControlCurrent,
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

impl WmCommand {
    pub fn builtin_candidates() -> &'static [String] { WmCmd::snake_case_variants() }
}

pub struct Config {
    pub restore_file: PathBuf,
    pub config: crate::common::config::Config,
}

pub struct WmController {
    config: Config,
    events_tx: reactor::Sender,
    event_tap_tx: event_tap::Sender,
    stack_line_tx: Option<crate::actor::stack_line::Sender>,
    mission_control_tx: Option<mission_control::Sender>,
    window_tx_store: Option<WindowTxStore>,
    receiver: Receiver,
    sender: Sender,
    hotkeys_installed: bool,
}

impl WmController {
    pub fn new(
        config: Config,
        events_tx: reactor::Sender,
        event_tap_tx: event_tap::Sender,
        stack_line_tx: crate::actor::stack_line::Sender,
        mission_control_tx: crate::actor::mission_control::Sender,
        window_tx_store: Option<WindowTxStore>,
    ) -> (Self, actor::Sender<WmEvent>) {
        let (sender, receiver) = actor::channel();
        sys::app::set_activation_policy_callback({
            let sender = sender.clone();
            move |pid, info| sender.send(WmEvent::AppLaunch(pid, info))
        });
        sys::app::set_finished_launching_callback({
            let sender = sender.clone();
            move |pid, info| sender.send(WmEvent::AppLaunch(pid, info))
        });
        let this = Self {
            config,
            events_tx,
            event_tap_tx,
            stack_line_tx: Some(stack_line_tx),
            mission_control_tx: Some(mission_control_tx),
            window_tx_store,
            receiver,
            sender: sender.clone(),
            hotkeys_installed: false,
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
                | SpaceChanged(_)
        ) && let Some(tx) = &self.mission_control_tx
        {
            tx.send(mission_control::Event::RefreshCurrentWorkspace);
        }

        match event {
            SystemWoke => self.events_tx.send(Event::SystemWoke),
            DisplayChurnBegin => self.events_tx.send(Event::DisplayChurnBegin),
            DisplayChurnEnd => self.events_tx.send(Event::DisplayChurnEnd),
            AppEventsRegistered => {
                _ = self.event_tap_tx.send(event_tap::Request::SetEventProcessing(false));

                if !self.hotkeys_installed {
                    self.register_hotkeys();
                    self.hotkeys_installed = true;
                }

                let sender = self.sender.clone();
                let event_tap_tx = self.event_tap_tx.clone();
                queue::main().after_f_s(
                    Time::new_after(Time::NOW, 250 * 1000000),
                    (sender, WmEvent::DiscoverRunningApps),
                    |(sender, event)| sender.send(event),
                );

                queue::main().after_f_s(
                    Time::new_after(Time::NOW, (250 + 350) * 1000000),
                    (event_tap_tx, event_tap::Request::SetEventProcessing(true)),
                    |(sender, event)| sender.send(event),
                );
            }
            DiscoverRunningApps => {
                for (pid, info) in sys::app::running_apps(None) {
                    self.new_app(pid, info);
                }
            }
            AppLaunch(pid, info) => {
                self.new_app(pid, info);
            }
            AppGloballyActivated(pid) => {
                _ = self.event_tap_tx.send(event_tap::Request::EnforceHidden);
                self.events_tx.send(Event::ApplicationGloballyActivated(pid));
            }
            AppGloballyDeactivated(pid) => {
                self.events_tx.send(Event::ApplicationGloballyDeactivated(pid));
            }
            AppTerminated(pid) => {
                sys::app::remove_activation_policy_observer(pid);
                self.events_tx.send(Event::ApplicationTerminated(pid));
            }
            ConfigUpdated(new_cfg) => {
                let old_keys_ser = serde_json::to_string(&self.config.config.keys).ok();

                self.config.config = new_cfg;

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
            ScreenParametersChanged(screens, converter, spaces) => {
                let frames: Vec<CGRect> = screens.iter().map(|s| s.frame).collect();
                let snapshots: Vec<reactor::ScreenSnapshot> = screens
                    .into_iter()
                    .zip(spaces.iter().copied())
                    .map(|(descriptor, space)| reactor::ScreenSnapshot {
                        screen_id: descriptor.id.as_u32(),
                        frame: descriptor.frame,
                        space,
                        display_uuid: descriptor.display_uuid,
                        name: descriptor.name,
                    })
                    .collect();

                self.events_tx.send(Event::ScreenParametersChanged(
                    snapshots,
                    self.get_windows_for_spaces(&spaces),
                ));

                _ = self
                    .event_tap_tx
                    .send(event_tap::Request::ScreenParametersChanged(frames, converter));
                if let Some(tx) = &self.stack_line_tx {
                    _ = tx.try_send(crate::actor::stack_line::Event::ScreenParametersChanged(
                        converter,
                    ));
                }
            }
            SpaceChanged(spaces) => {
                self.events_tx.send(reactor::Event::SpaceChanged(
                    spaces.clone(),
                    self.get_windows_for_spaces(&spaces),
                ));
            }
            PowerStateChanged(is_low_power_mode) => {
                info!("Power state changed: low power mode = {}", is_low_power_mode);
            }
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
            Command(Wm(MoveWindowToWorkspace(ws_sel))) => {
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
                        layout::LayoutCommand::MoveWindowToWorkspace {
                            workspace: workspace_index,
                            window_id: None,
                        },
                    )));
                } else {
                    tracing::warn!(
                        "Hotkey requested move window to workspace {:?} but it could not be resolved; ignoring",
                        ws_sel
                    );
                }
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
            Command(Wm(Exec(cmd))) => {
                self.exec_cmd(cmd);
            }
            Command(ReactorCommand(cmd)) => {
                self.events_tx.send(reactor::Event::Command(cmd));
            }
        }
    }

    fn new_app(&mut self, pid: pid_t, info: AppInfo) {
        let Some(running_app) = NSRunningApplication::with_process_id(pid) else {
            debug!(pid = ?pid, "Failed to resolve NSRunningApplication for new app");
            return;
        };

        if running_app.activationPolicy() != NSApplicationActivationPolicy::Regular
            && info.bundle_id.as_deref() != Some("com.apple.loginwindow")
        {
            sys::app::ensure_activation_policy_observer(pid, info.clone());
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
            sys::app::ensure_finished_launching_observer(pid, info.clone());
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

        actor::app::spawn_app_thread(
            pid,
            info,
            self.events_tx.clone(),
            self.window_tx_store.clone(),
        );
    }

    fn register_hotkeys(&mut self) {
        debug!("register_hotkeys");
        let bindings: Vec<(Hotkey, WmCommand)> = self.config.config.keys.iter().cloned().collect();
        _ = self.event_tap_tx.send(event_tap::Request::SetHotkeys(bindings));
    }

    fn get_windows_for_spaces(&self, spaces: &[Option<SpaceId>]) -> Vec<WindowServerInfo> {
        // IMPORTANT: `spaces` is a raw snapshot from system events (one per screen).
        // It does *not* encode whether a space is currently "active"/"activated" per our
        // activation policy (default_disable, ToggleSpaceActivated, login-window suppression).
        //
        // The reactor owns the activation policy and should decide which spaces are active.
        // At the WM→reactor boundary, we must only send windows for spaces that are actually
        // active in the current configuration.
        //
        // We can't safely reconstruct that policy here without duplicating reactor behavior,
        // so treat a missing/unknown space id as inactive and filter it out.
        let all_windows = sys::window_server::get_visible_windows_with_layer(None);

        let space_id_values: Vec<u64> = spaces.iter().copied().flatten().map(|s| s.get()).collect();

        // If we don't know any current spaces yet, avoid leaking windows across spaces.
        if space_id_values.is_empty() {
            return Vec::new();
        }

        let allowed_window_ids: HashSet<u32> =
            sys::window_server::space_window_list_for_connection(&space_id_values, 0, false)
                .into_iter()
                .collect();

        if allowed_window_ids.is_empty() {
            if !all_windows.is_empty() {
                tracing::trace!(
                    ?space_id_values,
                    "space window list empty during screen update; skipping update"
                );
            }
            return Vec::new();
        }

        all_windows
            .into_iter()
            .filter(|info| allowed_window_ids.contains(&info.id.as_u32()))
            .collect()
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
