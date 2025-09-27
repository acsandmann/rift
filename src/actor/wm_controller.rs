//! The WM Controller handles major events like enabling and disabling the
//! window manager on certain spaces and launching app threads. It also
//! controls hotkey registration.

use std::borrow::Cow;
use std::path::PathBuf;

use accessibility_sys::pid_t;
use objc2_app_kit::NSScreen;
use objc2_core_foundation::CGRect;
use objc2_foundation::MainThreadMarker;
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, instrument};

use crate::common::config::WorkspaceSelector;

pub type Sender = actor::Sender<WmEvent>;

type Receiver = actor::Receiver<WmEvent>;

use crate::actor::app::AppInfo;
use crate::actor::{self, mission_control, mouse, reactor};
use crate::common::collections::HashSet;
use crate::sys::event::HotkeyManager;
use crate::sys::screen::{CoordinateConverter, NSScreenExt, ScreenId, SpaceId};
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
    SpaceChanged(Vec<Option<SpaceId>>),
    ScreenParametersChanged(
        Vec<CGRect>,
        Vec<ScreenId>,
        CoordinateConverter,
        Vec<Option<SpaceId>>,
    ),
    SessionActivated,
    SessionDeactivated,
    SystemWoke,
    PowerStateChanged(bool),
    Command(WmCommand),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WmCommand {
    Wm(WmCmd),
    ReactorCommand(reactor::Command),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ExecCmd {
    String(String),
    Array(Vec<String>),
}

pub struct Config {
    pub one_space: bool,
    pub restore_file: PathBuf,
    pub config: crate::common::config::Config,
}

pub struct WmController {
    config: Config,
    events_tx: reactor::Sender,
    mouse_tx: mouse::Sender,
    stack_line_tx: Option<crate::actor::stack_line::Sender>,
    mission_control_tx: Option<crate::actor::mission_control::Sender>,
    receiver: Receiver,
    sender: Sender,
    starting_space: Option<SpaceId>,
    cur_space: Vec<Option<SpaceId>>,
    cur_screen_id: Vec<ScreenId>,
    disabled_spaces: HashSet<SpaceId>,
    enabled_spaces: HashSet<SpaceId>,
    login_window_pid: Option<pid_t>,
    login_window_active: bool,
    hotkeys: Option<HotkeyManager>,
    mtm: MainThreadMarker,
    screen_params_received: bool,
}

impl WmController {
    pub fn new(
        config: Config,
        events_tx: reactor::Sender,
        mouse_tx: mouse::Sender,
        stack_line_tx: crate::actor::stack_line::Sender,
        mission_control_tx: crate::actor::mission_control::Sender,
    ) -> (Self, actor::Sender<WmEvent>) {
        let (sender, receiver) = actor::channel();
        let this = Self {
            config,
            events_tx,
            mouse_tx,
            stack_line_tx: Some(stack_line_tx),
            mission_control_tx: Some(mission_control_tx),
            receiver,
            sender: sender.clone(),
            starting_space: None,
            cur_space: Vec::new(),
            cur_screen_id: Vec::new(),
            disabled_spaces: HashSet::default(),
            enabled_spaces: HashSet::default(),
            login_window_pid: None,
            login_window_active: false,
            hotkeys: None,
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
        match event {
            SessionActivated => self.on_session_activated(),
            SessionDeactivated => self.on_session_deactivated(),
            SystemWoke => self.events_tx.send(Event::SystemWoke),
            AppEventsRegistered => {
                _ = self.mouse_tx.send(mouse::Request::SetEventProcessing(false));

                let sender = self.sender.clone();
                let mouse_tx = self.mouse_tx.clone();
                std::thread::spawn(move || {
                    use std::time::Duration;

                    use crate::sys::executor::Executor;
                    use crate::sys::timer::Timer;

                    Executor::run(async move {
                        Timer::sleep(Duration::from_millis(250)).await;
                        let _ = sender.send(WmEvent::DiscoverRunningApps);

                        Timer::sleep(Duration::from_millis(350)).await;
                        _ = mouse_tx.send(mouse::Request::SetEventProcessing(true));
                    });
                });
            }
            DiscoverRunningApps => {
                if !self.screen_params_received {
                    let sender = self.sender.clone();
                    std::thread::spawn(move || {
                        use std::time::Duration;
                        std::thread::sleep(Duration::from_millis(200));
                        let _ = sender.send(WmEvent::DiscoverRunningApps);
                    });
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
                _ = self.mouse_tx.send(mouse::Request::EnforceHidden);

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
                    self.events_tx
                        .send(Event::SpaceChanged(self.active_spaces(), self.get_windows()));
                }
                self.events_tx.send(Event::ApplicationGloballyDeactivated(pid));
            }
            AppTerminated(pid) => {
                self.events_tx.send(Event::ApplicationTerminated(pid));
            }
            ScreenParametersChanged(frames, ids, converter, spaces) => {
                self.screen_params_received = true;
                self.cur_screen_id = ids;
                self.handle_space_changed(spaces);
                self.events_tx.send(Event::ScreenParametersChanged(
                    frames.clone(),
                    self.active_spaces(),
                    self.get_windows(),
                ));
                _ = self.mouse_tx.send(mouse::Request::ScreenParametersChanged(frames, converter));
                if let Some(tx) = &self.stack_line_tx {
                    _ = tx.try_send(crate::actor::stack_line::Event::ScreenParametersChanged(
                        converter,
                    ));
                }
            }
            SpaceChanged(spaces) => {
                self.handle_space_changed(spaces);
                self.events_tx
                    .send(Event::SpaceChanged(self.active_spaces(), self.get_windows()));
            }
            PowerStateChanged(is_low_power_mode) => {
                info!("Power state changed: low power mode = {}", is_low_power_mode);
            }
            Command(Wm(ToggleSpaceActivated)) => {
                let Some(space) = self.get_focused_space() else {
                    return;
                };

                let space_currently_enabled = if self.config.config.settings.default_disable {
                    self.enabled_spaces.contains(&space)
                } else {
                    !self.disabled_spaces.contains(&space)
                };

                if !space_currently_enabled {
                    if self.config.config.settings.default_disable {
                        self.enabled_spaces.insert(space);
                    } else {
                        self.disabled_spaces.remove(&space);
                    }

                    self.events_tx.send(reactor::Event::SpaceChanged(
                        self.active_spaces(),
                        self.get_windows(),
                    ));

                    self.apply_app_rules_to_existing_windows();
                } else {
                    self.apply_app_rules_to_existing_windows();
                }
            }
            Command(Wm(NextWorkspace)) => {
                self.dismiss_mission_control();
                self.events_tx.send(reactor::Event::Command(reactor::Command::Layout(
                    layout::LayoutCommand::NextWorkspace(None),
                )));
            }
            Command(Wm(PrevWorkspace)) => {
                self.dismiss_mission_control();
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
                    self.dismiss_mission_control();
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
                        layout::LayoutCommand::MoveWindowToWorkspace(workspace_index),
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
                self.dismiss_mission_control();
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

    fn dismiss_mission_control(&self) {
        if let Some(tx) = &self.mission_control_tx {
            let _ = tx.try_send(mission_control::Event::Dismiss);
        }
    }

    fn new_app(&mut self, pid: pid_t, info: AppInfo) {
        if info.bundle_id.as_deref() == Some("com.apple.loginwindow") {
            self.login_window_pid = Some(pid);
        }
        actor::app::spawn_app_thread(pid, info, self.events_tx.clone());
    }

    fn get_focused_space(&self) -> Option<SpaceId> {
        let screen = NSScreen::mainScreen(self.mtm)?;
        let number = screen.get_number().ok()?;
        *self.cur_screen_id.iter().zip(&self.cur_space).find(|(id, _)| **id == number)?.1
    }

    fn handle_space_changed(&mut self, spaces: Vec<Option<SpaceId>>) {
        self.cur_space = spaces;
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

    fn active_spaces(&self) -> Vec<Option<SpaceId>> {
        let mut spaces = self.cur_space.clone();
        for space in &mut spaces {
            let enabled = match space {
                _ if self.login_window_active => false,
                Some(_) if self.config.one_space && *space != self.starting_space => false,
                Some(sp) if self.disabled_spaces.contains(sp) => false,
                Some(sp) if self.enabled_spaces.contains(sp) => true,
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
        let mgr = HotkeyManager::new(self.sender.clone());
        for (key, cmd) in &self.config.config.keys {
            mgr.register_wm(key.modifiers, key.key_code, cmd.clone());
        }
        self.hotkeys = Some(mgr);
    }

    fn unregister_hotkeys(&mut self) {
        debug!("unregister_hotkeys");
        self.hotkeys = None;
    }

    fn on_session_deactivated(&mut self) {
        if self.login_window_active {
            return;
        }

        info!("Session deactivated; disabling WM");
        self.login_window_active = true;
        self.events_tx.send(reactor::Event::SpaceChanged(
            self.active_spaces(),
            self.get_windows(),
        ));
    }

    fn on_session_activated(&mut self) {
        if !self.login_window_active {
            return;
        }

        info!("Session activated; re-enabling WM");
        self.login_window_active = false;
        self.events_tx.send(reactor::Event::SpaceChanged(
            self.active_spaces(),
            self.get_windows(),
        ));
    }

    fn get_windows(&self) -> Vec<WindowServerInfo> {
        #[cfg(not(test))]
        {
            let all_windows = sys::window_server::get_visible_windows_with_layer(None);

            let space_ids: Vec<u64> = self
                .active_spaces()
                .into_iter()
                .filter_map(|opt| opt.map(|s| s.get()))
                .collect();

            if space_ids.is_empty() {
                return all_windows;
            }

            let allowed_window_ids: HashSet<u32> =
                sys::window_server::space_window_list_for_connection(&space_ids, 0, false)
                    .into_iter()
                    .collect();

            all_windows
                .into_iter()
                .filter(|info| allowed_window_ids.contains(&info.id.as_u32()))
                .collect()
        }
        #[cfg(test)]
        {
            vec![]
        }
    }

    fn apply_app_rules_to_existing_windows(&mut self) {
        use crate::common::collections::HashMap;

        let visible_windows = self.get_windows();
        let mut windows_by_pid: HashMap<pid_t, Vec<WindowServerInfo>> = HashMap::default();

        for window in visible_windows {
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
