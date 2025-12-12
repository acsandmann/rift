//! The WM Controller handles major events like enabling and disabling the
//! window manager on certain spaces and launching app threads. It also
//! controls hotkey registration.

use std::borrow::Cow;
use std::path::PathBuf;

use dispatchr::queue;
use dispatchr::time::Time;
use objc2_app_kit::{NSApplicationActivationPolicy, NSRunningApplication, NSScreen};
use objc2_core_foundation::CGRect;
use objc2_foundation::MainThreadMarker;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json;
use strum::VariantNames;
use tracing::{debug, error, info, instrument, trace, warn};

use crate::common::config::WorkspaceSelector;
use crate::sys::app::{NSRunningApplicationExt, pid_t};

pub type Sender = actor::Sender<WmEvent>;

type Receiver = actor::Receiver<WmEvent>;

use crate::actor::app::AppInfo;
use crate::actor::{self, event_tap, mission_control, reactor};
use crate::common::collections::{HashMap, HashSet};
use crate::model::tx_store::WindowTxStore;
use crate::sys::dispatch::DispatchExt;
use crate::sys::event::Hotkey;
use crate::sys::geometry::CGRectExt;
use crate::sys::screen::{CoordinateConverter, NSScreenExt, ScreenDescriptor, ScreenId, SpaceId};
use crate::sys::window_server::{
    WindowServerId, WindowServerInfo, current_cursor_location, space_window_list_for_connection,
};
use crate::{layout_engine as layout, sys};

#[derive(Debug)]
pub enum WmEvent {
    DiscoverRunningApps,
    AppEventsRegistered,
    AppLaunch(pid_t, AppInfo),
    AppThreadTerminated(pid_t),
    AppGloballyActivated(pid_t),
    AppGloballyDeactivated(pid_t),
    AppTerminated(pid_t),
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
    pub one_space: bool,
    pub restore_file: PathBuf,
    pub config: crate::common::config::Config,
}

pub struct WmController {
    config: Config,
    events_tx: reactor::Sender,
    event_tap_tx: event_tap::Sender,
    stack_line_tx: Option<crate::actor::stack_line::Sender>,
    mission_control_tx: Option<crate::actor::mission_control::Sender>,
    window_tx_store: Option<WindowTxStore>,
    receiver: Receiver,
    sender: Sender,
    starting_space: Option<SpaceId>,
    cur_space: Vec<Option<SpaceId>>,
    cur_screen_id: Vec<ScreenId>,
    cur_display_uuid: Vec<String>,
    cur_frames: Vec<CGRect>,
    disabled_spaces: HashSet<SpaceId>,
    enabled_spaces: HashSet<SpaceId>,
    enabled_displays: HashSet<String>,
    disabled_displays: HashSet<String>,
    last_known_space_by_screen: HashMap<ScreenId, SpaceId>,
    login_window_pid: Option<pid_t>,
    login_window_active: bool,
    spawning_apps: HashSet<pid_t>,
    known_apps: HashSet<pid_t>,
    hotkeys_registered: bool,
    mtm: MainThreadMarker,
    screen_params_received: bool,
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
            starting_space: None,
            cur_space: Vec::new(),
            cur_screen_id: Vec::new(),
            cur_display_uuid: Vec::new(),
            cur_frames: Vec::new(),
            disabled_spaces: HashSet::default(),
            enabled_spaces: HashSet::default(),
            enabled_displays: HashSet::default(),
            disabled_displays: HashSet::default(),
            last_known_space_by_screen: HashMap::default(),
            login_window_pid: None,
            login_window_active: false,
            spawning_apps: HashSet::default(),
            known_apps: HashSet::default(),
            hotkeys_registered: false,
            mtm: MainThreadMarker::new().unwrap(),
            screen_params_received: false,
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

        use self::WmCmd::*;
        use self::WmCommand::*;
        use self::WmEvent::*;

        if matches!(
            event,
            Command(Wm(NextWorkspace))
                | Command(Wm(PrevWorkspace))
                | Command(Wm(SwitchToWorkspace(_)))
                | Command(Wm(SwitchToLastWorkspace))
                | SpaceChanged(_)
        ) && let Some(tx) = &self.mission_control_tx
        {
            tx.send(mission_control::Event::RefreshCurrentWorkspace);
        }

        match event {
            SystemWoke => self.events_tx.send(Event::SystemWoke),
            AppEventsRegistered => {
                _ = self.event_tap_tx.send(event_tap::Request::SetEventProcessing(false));

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
                if !self.screen_params_received {
                    let sender = self.sender.clone();
                    queue::main().after_f_s(
                        Time::new_after(Time::NOW, 200 * 1000000),
                        (sender, WmEvent::DiscoverRunningApps),
                        |(sender, event)| sender.send(event),
                    );
                    return;
                }
                for (pid, info) in sys::app::running_apps(None) {
                    self.new_app(pid, info);
                }
            }
            AppLaunch(pid, info) => {
                self.new_app(pid, info);
            }
            AppGloballyActivated(pid) => {
                _ = self.event_tap_tx.send(event_tap::Request::EnforceHidden);

                if self.login_window_pid == Some(pid) {
                    info!("Login window activated");
                    self.login_window_active = true;
                    self.events_tx
                        .send(Event::SpaceChanged(self.active_spaces(), self.get_windows()));
                }

                self.events_tx.send(Event::ApplicationGloballyActivated(pid));
            }
            AppGloballyDeactivated(pid) => {
                if self.login_window_pid == Some(pid) {
                    info!("Login window deactivated");
                    self.login_window_active = false;
                    let active_spaces = self.active_spaces();
                    info!(
                        ?active_spaces,
                        "Login window deactivated; recomputing active_spaces"
                    );
                    self.events_tx.send(Event::SpaceChanged(active_spaces, self.get_windows()));
                }
                self.events_tx.send(Event::ApplicationGloballyDeactivated(pid));
            }
            AppTerminated(pid) => {
                sys::app::remove_activation_policy_observer(pid);
                if self.known_apps.remove(&pid) {
                    debug!(pid = ?pid, "App terminated; removed from known_apps");
                }
                if self.spawning_apps.remove(&pid) {
                    debug!(pid = ?pid, "App terminated; removed from spawning_apps");
                }
                self.events_tx.send(Event::ApplicationTerminated(pid));
            }
            AppThreadTerminated(pid) => {
                if self.known_apps.remove(&pid) {
                    debug!(pid = ?pid, "App thread terminated; removed from known_apps");
                }
                if self.spawning_apps.remove(&pid) {
                    debug!(pid = ?pid, "App thread terminated; removed from spawning_apps");
                }
            }
            ConfigUpdated(new_cfg) => {
                let old_keys_ser = serde_json::to_string(&self.config.config.keys).ok();

                self.config.config = new_cfg;

                if let Some(old_ser) = old_keys_ser {
                    if serde_json::to_string(&self.config.config.keys).ok().as_deref()
                        != Some(&old_ser)
                    {
                        debug!("hotkey bindings changed; reloading hotkeys");
                        self.unregister_hotkeys();
                        self.register_hotkeys();
                    } else {
                        debug!("hotkey bindings unchanged; skipping reload");
                    }
                } else {
                    debug!("could not compare hotkey bindings; reloading hotkeys");
                    self.unregister_hotkeys();
                    self.register_hotkeys();
                }
            }
            ScreenParametersChanged(screens, converter, spaces) => {
                let default_disable = self.config.config.settings.default_disable;
                let prev_display_uuids: HashSet<String> =
                    self.cur_display_uuid.iter().cloned().collect();
                let new_display_uuids: HashSet<String> =
                    screens.iter().map(|s| s.display_uuid.clone()).collect();
                let displays_changed = prev_display_uuids != new_display_uuids;

                info!(
                    default_disable,
                    prev_displays = ?prev_display_uuids,
                    new_displays = ?new_display_uuids,
                    displays_changed,
                    screen_count = screens.len(),
                    "ScreenParametersChanged received"
                );

                if displays_changed && !default_disable {
                    // When displays change in default-enable mode, drop any remembered
                    // disabled display/space state so surviving displays default to enabled.
                    self.disabled_spaces.clear();
                    self.disabled_displays.clear();
                    info!(
                        "Cleared disabled state due to display set change (default_disable=false)"
                    );
                }
                if !default_disable && screens.len() == 1 {
                    // After sleep/resume with a single display, ensure we default to enabled.
                    self.disabled_spaces.clear();
                    self.disabled_displays.clear();
                    info!("Cleared disabled state for single-display default-enable scenario");
                }

                self.screen_params_received = true;
                self.cur_screen_id = screens.iter().map(|s| s.id).collect();
                self.cur_display_uuid = screens.iter().map(|s| s.display_uuid.clone()).collect();
                self.handle_space_changed(spaces);
                let active_spaces = self.active_spaces();
                info!(
                    ?active_spaces,
                    disabled_spaces = ?self.disabled_spaces,
                    enabled_spaces = ?self.enabled_spaces,
                    disabled_displays = ?self.disabled_displays,
                    enabled_displays = ?self.enabled_displays,
                    "Computed active_spaces after ScreenParametersChanged"
                );
                let frames: Vec<CGRect> = screens.iter().map(|s| s.frame).collect();
                self.cur_frames = frames.clone();
                let snapshots: Vec<reactor::ScreenSnapshot> = screens
                    .into_iter()
                    .zip(active_spaces.iter().copied())
                    .map(|(descriptor, space)| reactor::ScreenSnapshot {
                        screen_id: descriptor.id.as_u32(),
                        frame: descriptor.frame,
                        space,
                        display_uuid: descriptor.display_uuid,
                        name: descriptor.name,
                    })
                    .collect();
                self.events_tx
                    .send(Event::ScreenParametersChanged(snapshots, self.get_windows()));
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
                self.handle_space_changed(spaces.clone());
                let active_spaces = self.active_spaces();
                self.events_tx.send(reactor::Event::ActiveSpacesChanged(active_spaces.clone()));
                self.events_tx.send(reactor::Event::SpaceChanged(spaces, self.get_windows()));
            }
            PowerStateChanged(is_low_power_mode) => {
                info!("Power state changed: low power mode = {}", is_low_power_mode);
            }
            Command(Wm(ToggleSpaceActivated)) => {
                let Some(space) = self.get_focused_space() else {
                    warn!("no focused space found");
                    return;
                };

                let display_uuid = self
                    .cur_space
                    .iter()
                    .position(|s| *s == Some(space))
                    .and_then(|idx| self.cur_display_uuid.get(idx).cloned());

                let default_disable = self.config.config.settings.default_disable;
                let space_currently_enabled = if default_disable {
                    self.enabled_spaces.contains(&space)
                } else {
                    !self.disabled_spaces.contains(&space)
                };

                if space_currently_enabled {
                    if default_disable {
                        self.enabled_spaces.remove(&space);
                        if let Some(ref uuid) = display_uuid {
                            self.enabled_displays.remove(uuid);
                        }
                        debug!("removed space {:?} from enabled_spaces", space);
                    } else {
                        self.disabled_spaces.insert(space);
                        if let Some(ref uuid) = display_uuid {
                            self.disabled_displays.insert(uuid.clone());
                        }
                        debug!("added space {:?} to disabled_spaces", space);
                    }
                } else if default_disable {
                    self.enabled_spaces.insert(space);
                    if let Some(ref uuid) = display_uuid {
                        self.enabled_displays.insert(uuid.clone());
                    }
                    debug!("added space {:?} to enabled_spaces", space);
                } else {
                    self.disabled_spaces.remove(&space);
                    if let Some(ref uuid) = display_uuid {
                        self.disabled_displays.remove(uuid);
                    }
                    debug!("removed space {:?} from disabled_spaces", space);
                }

                let active_spaces = self.active_spaces();
                trace!("active_spaces after toggle = {:?}", active_spaces);
                let current_spaces = self.cur_space.clone();

                self.events_tx.send(reactor::Event::ActiveSpacesChanged(active_spaces.clone()));
                self.events_tx
                    .send(reactor::Event::SpaceChanged(current_spaces, self.get_windows()));

                self.apply_app_rules_to_existing_windows(&[space]);
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
        if info.bundle_id.as_deref() == Some("com.apple.loginwindow") {
            self.login_window_pid = Some(pid);
        }

        if self.known_apps.contains(&pid) || self.spawning_apps.contains(&pid) {
            debug!(pid = ?pid, "Duplicate AppLaunch received; skipping spawn");
            return;
        }

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

        self.spawning_apps.insert(pid);

        actor::app::spawn_app_thread(
            pid,
            info,
            self.events_tx.clone(),
            self.window_tx_store.clone(),
        );

        self.spawning_apps.remove(&pid);
        self.known_apps.insert(pid);
    }

    fn get_focused_space(&self) -> Option<SpaceId> {
        if let Ok(point) = current_cursor_location() {
            if let Some((idx, _)) =
                self.cur_frames.iter().enumerate().find(|(_, f)| f.contains(point))
            {
                if let Some(space_opt) = self.cur_space.get(idx) {
                    if let Some(space) = space_opt {
                        return Some(*space);
                    }
                }
                if let Some(screen_id) = self.cur_screen_id.get(idx) {
                    if let Some(space) = self.last_known_space_by_screen.get(screen_id).copied() {
                        return Some(space);
                    }
                }
            }
        }

        let screen = NSScreen::mainScreen(self.mtm)?;
        let number = screen.get_number().ok()?;
        *self.cur_screen_id.iter().zip(&self.cur_space).find(|(id, _)| **id == number)?.1
    }

    fn handle_space_changed(&mut self, spaces: Vec<Option<SpaceId>>) {
        let previous_spaces = self.cur_space.clone();
        self.cur_space = spaces;

        let active_spaces: HashSet<SpaceId> =
            self.cur_space.iter().copied().flatten().collect::<HashSet<_>>();
        let active_displays: HashSet<String> =
            self.cur_display_uuid.iter().cloned().collect::<HashSet<_>>();
        let active_screen_ids: HashSet<ScreenId> =
            self.cur_screen_id.iter().copied().collect::<HashSet<_>>();

        self.disabled_spaces.retain(|space| active_spaces.contains(space));
        self.enabled_spaces.retain(|space| active_spaces.contains(space));
        self.disabled_displays.retain(|uuid| active_displays.contains(uuid));
        self.enabled_displays.retain(|uuid| active_displays.contains(uuid));
        self.last_known_space_by_screen
            .retain(|screen, _| active_screen_ids.contains(screen));

        debug!(
            "handle_space_changed: previous_spaces={:?}, cur_space={:?}, cur_screen_id.len={}",
            previous_spaces,
            self.cur_space,
            self.cur_screen_id.len()
        );

        let pairs: Vec<(ScreenId, Option<SpaceId>)> =
            self.cur_screen_id.iter().copied().zip(self.cur_space.iter().copied()).collect();

        for (idx, (screen_id, space_opt)) in pairs.into_iter().enumerate() {
            if let Some(new_space) = space_opt {
                let previous_space = previous_spaces
                    .get(idx)
                    .copied()
                    .flatten()
                    .or_else(|| self.last_known_space_by_screen.get(&screen_id).copied());

                if let Some(previous_space) = previous_space {
                    if previous_space != new_space {
                        debug!(
                            "transferring space activation: idx={}, screen_id={:?}, {:?} -> {:?}",
                            idx, screen_id, previous_space, new_space
                        );
                        self.transfer_space_activation(previous_space, new_space);
                    }
                }

                self.last_known_space_by_screen.insert(screen_id, new_space);
            }
        }

        for idx in self.cur_screen_id.len()..self.cur_space.len() {
            if let Some(Some(new_space)) = self.cur_space.get(idx).copied() {
                if let Some(previous_space) = previous_spaces.get(idx).copied().flatten() {
                    if previous_space != new_space {
                        debug!(
                            "transferring space activation (no screen_id): idx={}, {:?} -> {:?}",
                            idx, previous_space, new_space
                        );
                        self.transfer_space_activation(previous_space, new_space);
                    }
                }
            }
        }

        let default_disable = self.config.config.settings.default_disable;
        for (idx, space_opt) in self.cur_space.iter().enumerate() {
            let Some(space) = space_opt else { continue };
            let Some(display_uuid) = self.cur_display_uuid.get(idx) else {
                continue;
            };

            if default_disable {
                if self.enabled_displays.contains(display_uuid) {
                    if self.enabled_spaces.insert(*space) {
                        debug!(
                            "synced space {:?} to enabled_spaces from display {:?}",
                            space, display_uuid
                        );
                    }
                }
            } else {
                if self.disabled_displays.contains(display_uuid) {
                    if self.disabled_spaces.insert(*space) {
                        debug!(
                            "synced space {:?} to disabled_spaces from display {:?}",
                            space, display_uuid
                        );
                    }
                }
            }
        }

        let Some(&Some(space)) = self.cur_space.first() else {
            return;
        };
        if self.starting_space.is_none() {
            self.starting_space = Some(space);
            self.register_hotkeys();
        } else if self.config.one_space {
            if Some(space) == self.starting_space {
                self.register_hotkeys();
            } else {
                self.unregister_hotkeys();
            }
        }
    }

    fn transfer_space_activation(&mut self, old_space: SpaceId, new_space: SpaceId) {
        if self.config.config.settings.default_disable {
            if self.enabled_spaces.remove(&old_space) {
                self.enabled_spaces.insert(new_space);
            }
        } else if self.disabled_spaces.remove(&old_space) {
            self.disabled_spaces.insert(new_space);
        }

        if self.starting_space == Some(old_space) {
            self.starting_space = Some(new_space);
        }
    }

    fn active_spaces(&self) -> Vec<Option<SpaceId>> {
        let mut spaces = self.cur_space.clone();
        for (idx, space) in spaces.iter_mut().enumerate() {
            // Get the display UUID for this index to check display-based activation
            let display_uuid = self.cur_display_uuid.get(idx);
            let display_enabled =
                display_uuid.map(|uuid| self.enabled_displays.contains(uuid)).unwrap_or(false);
            let display_disabled =
                display_uuid.map(|uuid| self.disabled_displays.contains(uuid)).unwrap_or(false);

            let enabled = match space {
                _ if self.login_window_active => false,
                Some(_) if self.config.one_space && *space != self.starting_space => false,
                Some(sp) if self.disabled_spaces.contains(sp) => false,
                _ if display_disabled => false,
                Some(sp) if self.enabled_spaces.contains(sp) => true,
                _ if display_enabled => true,
                _ if self.config.config.settings.default_disable => false,
                _ => true,
            };
            if !enabled {
                *space = None;
            }
        }
        spaces
    }

    fn register_hotkeys(&mut self) {
        debug!("register_hotkeys");
        if self.hotkeys_registered {
            debug!("Hotkeys already registered; refreshing bindings");
        }

        let bindings: Vec<(Hotkey, WmCommand)> = self.config.config.keys.iter().cloned().collect();

        _ = self.event_tap_tx.send(event_tap::Request::SetHotkeys(bindings));

        self.hotkeys_registered = true;
    }

    fn unregister_hotkeys(&mut self) {
        debug!("unregister_hotkeys");
        if self.hotkeys_registered {
            _ = self.event_tap_tx.send(event_tap::Request::SetHotkeys(Vec::new()));
            self.hotkeys_registered = false;
        }
    }

    fn get_windows(&self) -> Vec<WindowServerInfo> {
        let all_windows = sys::window_server::get_visible_windows_with_layer(None);

        let active_space_ids: Vec<SpaceId> =
            self.active_spaces().into_iter().filter_map(|opt| opt).collect();
        let fallback_space_ids: Vec<SpaceId> = self.cur_space.iter().copied().flatten().collect();
        let space_id_values: Vec<u64> = if active_space_ids.is_empty() {
            fallback_space_ids.iter().map(|space| space.get()).collect()
        } else {
            active_space_ids.iter().map(|space| space.get()).collect()
        };

        // If we don't know any current spaces yet, avoid leaking windows across
        // spaces; wait for a valid space list before surfacing anything.
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

    fn apply_app_rules_to_existing_windows(&mut self, target_spaces: &[SpaceId]) {
        use crate::common::collections::HashMap;

        if target_spaces.is_empty() {
            return;
        }

        let space_id_values: Vec<u64> = target_spaces.iter().map(|space| space.get()).collect();
        let allowed_window_ids: HashSet<WindowServerId> =
            space_window_list_for_connection(&space_id_values, 0, false)
                .into_iter()
                .map(WindowServerId::new)
                .collect();

        if allowed_window_ids.is_empty() {
            return;
        }

        let visible_windows = self.get_windows();
        let mut windows_by_pid: HashMap<pid_t, Vec<WindowServerInfo>> = HashMap::default();

        for window in visible_windows {
            if !allowed_window_ids.contains(&window.id) {
                continue;
            }
            windows_by_pid.entry(window.pid).or_default().push(window);
        }

        for (pid, windows) in windows_by_pid {
            if let Some(app_info) = self.get_app_info_for_pid(pid) {
                self.events_tx.send(reactor::Event::ApplyAppRulesToExistingWindows {
                    pid,
                    app_info,
                    windows,
                });
            }
        }
    }

    fn get_app_info_for_pid(&self, pid: pid_t) -> Option<AppInfo> {
        use objc2_app_kit::NSRunningApplication;

        use crate::sys::app::NSRunningApplicationExt;

        NSRunningApplication::with_process_id(pid).map(|app| AppInfo::from(&*app))
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
