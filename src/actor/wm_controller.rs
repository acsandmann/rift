//! The WM Controller handles major events like enabling and disabling the
//! window manager on certain spaces and launching app threads. It also
//! controls hotkey registration.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use dispatchr::queue;
use dispatchr::time::Time;
use objc2_app_kit::{NSApplicationActivationPolicy, NSRunningApplication};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use strum::VariantNames;
use tracing::{debug, error, info, instrument, warn};

use crate::common::config::WorkspaceSelector;
use crate::sys::app::{NSRunningApplicationExt, pid_t};

pub type Sender = actor::Sender<WmEvent>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppDiscoverySource {
    Process,
    WindowServer,
}

fn should_spawn_app(policy: NSApplicationActivationPolicy, source: AppDiscoverySource) -> bool {
    match policy {
        NSApplicationActivationPolicy::Regular => true,
        NSApplicationActivationPolicy::Accessory => source == AppDiscoverySource::WindowServer,
        NSApplicationActivationPolicy::Prohibited => false,
        _ => false,
    }
}

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
    /// Match the actors to the running apps (`AppLifecycle::sync`): an app's retry fell due.
    SyncApps,
    AppEventsRegistered,
    AppLaunch(pid_t, AppInfo, AppDiscoverySource),
    AppGloballyActivated(pid_t),
    AppGloballyDeactivated(pid_t),
    AppTerminated(pid_t),
    /// An app terminated whose pid could not be read: each tracked app that is
    /// no longer running is treated as terminated.
    AppsMayHaveTerminated,
    AppExited(pid_t, AppThreadHandle),
    SpaceStateUpdated(ForwardedSpaceState, CoordinateConverter),
    PowerStateChanged(bool),
    KeyboardLayoutChanged,
    OverviewChanged,
    OverviewActivationTarget(reactor::OverviewTarget),
    ConfigUpdated(crate::common::config::Config),
    Command(WmCommand),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum WmCommand {
    Wm(WmCmd),
    ConfiguredLayout(ConfiguredLayoutCommand),
    ReactorCommand(reactor::Command),
    Sequence(Vec<WmCommand>),
}

impl WmCommand {
    pub fn steps(&self) -> Box<dyn Iterator<Item = &WmCommand> + '_> {
        match self {
            WmCommand::Sequence(commands) => Box::new(commands.iter().flat_map(WmCommand::steps)),
            command => Box::new(std::iter::once(command)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ConfiguredLayoutCommand {
    ToggleWindowFloating(rift_protocol::ToggleWindowFloatingOptions),
    SwitchPresetColumnWidth,
}

#[derive(
    Debug,
    Clone,
    Serialize,
    Deserialize,
    PartialEq,
    Eq,
    strum_macros::VariantNames,
    rift_config_derive::CommandCatalog,
)]
#[serde(rename_all = "snake_case")]
pub enum WmCmd {
    #[command(group = 2, label = "Toggle tiling", order = 40)]
    ToggleSpaceActivated,
    #[command(
        label = "Run command",
        order = 45,
        sample = "Self::Exec(ExecCmd::String(String::new()))"
    )]
    Exec(ExecCmd),
    #[command(order = 46)]
    ReloadConfig,
    #[command(
        label = "Switch to keymap",
        order = 44,
        sample = "Self::BindingMode(\"default\".into())"
    )]
    BindingMode(String),

    #[command(group = 2, order = 31)]
    NextWorkspace,
    #[command(group = 2, label = "Previous workspace", order = 32)]
    PrevWorkspace,
    #[command(
        group = 2,
        order = 29,
        sample = "Self::SwitchToWorkspace(WorkspaceSelector::Index(0))"
    )]
    SwitchToWorkspace(WorkspaceSelector),
    #[command(
        group = 2,
        order = 30,
        sample = "Self::MoveWindowToWorkspace(WorkspaceSelector::Index(0))"
    )]
    MoveWindowToWorkspace(WorkspaceSelector),
    #[command(group = 2, order = 34)]
    CreateWorkspace,
    #[command(group = 2, label = "Last workspace", order = 33)]
    SwitchToLastWorkspace,

    #[command(group = 5, label = "Show all workspaces", order = 41)]
    ShowMissionControlAll,
    #[command(group = 5, label = "Show current workspace", order = 42)]
    ShowMissionControlCurrent,
    #[command(group = 5, label = "Dismiss overview", order = 43)]
    DismissMissionControl,
    #[command(group = 0, order = 11)]
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
    apps: AppLifecycle,
    /// Whether `DiscoverRunningApps` has run: `sync_apps` attaches nothing
    /// before it, since rift's app events must be registered first.
    discovered: bool,
    /// When `sync_apps` last ran.
    synced_at: Option<Instant>,
}

// Reserve before spawning; retain the reservation until AX resources are dropped.
#[derive(Default)]
struct AppLifecycle {
    actors: HashMap<pid_t, (AppThreadHandle, AppPhase)>,
    /// Running apps whose actor gave up, and when to try each again.
    retries: HashMap<pid_t, Retry>,
}

/// An app that refused AX requests while its actor started.
#[derive(Clone, Copy, Debug)]
struct Retry {
    failures: u32,
    at: Instant,
}

/// The wait before attaching an app again after its first failure; it doubles
/// with each failure in a row, up to `RETRY_LONGEST`.
const RETRY_FIRST: Duration = Duration::from_secs(2);
const RETRY_LONGEST: Duration = Duration::from_secs(300);
/// `sync_apps` runs at most this often, except when a retry falls due or an
/// app may have terminated.
const SYNC_INTERVAL: Duration = Duration::from_secs(1);

/// What `AppLifecycle::sync` found to do.
struct AppSync<T> {
    /// Running apps with no actor to attach.
    attach: Vec<(pid_t, T)>,
    /// Actors whose app no longer runs, to terminate.
    ended: Vec<pid_t>,
}

enum AppPhase {
    Active, // Includes initialization.
    Stopping(Option<(AppInfo, AppDiscoverySource)>),
}

impl AppLifecycle {
    fn reserve(
        &mut self,
        pid: pid_t,
        info: AppInfo,
        source: AppDiscoverySource,
    ) -> Option<(AppThreadHandle, actor::Receiver<Request>)> {
        if let Some((_, phase)) = self.actors.get_mut(&pid) {
            if let AppPhase::Stopping(relaunch) = phase {
                let source = if relaunch
                    .as_ref()
                    .is_some_and(|(_, previous)| *previous == AppDiscoverySource::WindowServer)
                {
                    AppDiscoverySource::WindowServer
                } else {
                    source
                };
                *relaunch = Some((info, source));
            }
            return None;
        }
        let (handle, rx) = AppThreadHandle::channel();
        self.actors.insert(pid, (handle.clone(), AppPhase::Active));
        Some((handle, rx))
    }

    /// The active apps for which `running` is false.
    fn ended(&self, running: impl Fn(pid_t) -> bool) -> Vec<pid_t> {
        let mut pids: Vec<pid_t> = self
            .actors
            .iter()
            .filter(|(pid, (_, phase))| matches!(phase, AppPhase::Active) && !running(**pid))
            .map(|(pid, _)| *pid)
            .collect();
        pids.sort_unstable();
        pids
    }

    fn terminate(&mut self, pid: pid_t) {
        if let Some((handle, phase)) = self.actors.get_mut(&pid) {
            *phase = AppPhase::Stopping(None);
            _ = handle.send(Request::Terminate);
        }
    }

    fn exited(
        &mut self,
        pid: pid_t,
        handle: &AppThreadHandle,
    ) -> Option<Option<(AppInfo, AppDiscoverySource)>> {
        if !self.actors.get(&pid)?.0.same_actor(handle) {
            return None;
        }
        if handle.is_attached() {
            // It attached; a later failure is a first failure.
            self.retries.remove(&pid);
        }
        Some(match self.actors.remove(&pid)?.1 {
            AppPhase::Stopping(info) => info,
            _ => None,
        })
    }

    fn is_attached(&self, pid: pid_t) -> bool { self.actors.contains_key(&pid) }

    /// Whether `handle` is `pid`'s actor and was not asked to stop: when it
    /// exits, it gave up on its own.
    fn is_active(&self, pid: pid_t, handle: &AppThreadHandle) -> bool {
        self.actors
            .get(&pid)
            .is_some_and(|(h, phase)| h.same_actor(handle) && matches!(phase, AppPhase::Active))
    }

    /// Note that `pid`'s actor gave up at `now` while the app ran on, and
    /// return how long to wait before attaching it again.
    fn attach_failed(&mut self, pid: pid_t, now: Instant) -> Duration {
        let failures = self.retries.get(&pid).map_or(0, |retry| retry.failures) + 1;
        let delay = RETRY_FIRST.saturating_mul(1 << (failures - 1).min(16)).min(RETRY_LONGEST);
        self.retries.insert(pid, Retry { failures, at: now + delay });
        delay
    }

    /// Match the actors to the running apps. Rift learns of apps from one-time
    /// events (a launch, a termination, a window appearing), and an app that
    /// misses one stays as it was: an app whose actor gave up while the app
    /// was busy keeps its windows wherever the last rift left them, unmanaged.
    /// Comparing what runs with what is attached finds it, whatever was missed.
    ///
    /// `attachable` are the running apps an actor may attach to; those with
    /// none are attached, apart from rift itself and apps whose retry is not
    /// due at `now`. Actors whose process is not `running` have ended.
    fn sync<T>(
        &mut self,
        attachable: Vec<(pid_t, T)>,
        running: impl Fn(pid_t) -> bool,
        own: pid_t,
        now: Instant,
    ) -> AppSync<T> {
        self.retries.retain(|pid, _| running(*pid));
        let attach = attachable
            .into_iter()
            .filter(|(pid, _)| {
                *pid != own
                    && !self.is_attached(*pid)
                    && self.retries.get(pid).is_none_or(|retry| retry.at <= now)
            })
            .collect();
        AppSync {
            attach,
            ended: self.ended(running),
        }
    }
}

impl WmController {
    pub fn new(
        config: Config,
        config_tx: config::Sender,
        events_tx: reactor::Sender,
        input_tx: input::Sender,
        stack_line_tx: crate::actor::stack_line::Sender,
        mission_control_tx: Option<crate::actor::mission_control::Sender>,
        window_tx_store: Option<WindowTxStore>,
    ) -> (Self, actor::Sender<WmEvent>) {
        let (sender, receiver) = actor::channel();
        sys::app::set_application_callback({
            let sender = sender.clone();
            move |pid, info| sender.send(WmEvent::AppLaunch(pid, info, AppDiscoverySource::Process))
        });
        let this = Self {
            config,
            config_tx,
            events_tx,
            input_tx,
            stack_line_tx: Some(stack_line_tx),
            mission_control_tx,
            window_tx_store,
            receiver,
            sender: sender.clone(),
            apps: AppLifecycle::default(),
            discovered: false,
            synced_at: None,
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

        // Apps come and go, or the user turns to another app or Space: the
        // moments to check that every running app has its actor.
        let sync = matches!(
            &event,
            SpaceStateUpdated(..) | AppGloballyActivated(_) | AppGloballyDeactivated(_)
        );

        match event {
            OverviewActivationTarget(target) => {
                if let Some(tx) = &self.mission_control_tx {
                    tx.send(mission_control::Event::ActivationTarget(target));
                }
            }
            OverviewChanged => {
                if let Some(tx) = &self.mission_control_tx {
                    tx.send(mission_control::Event::RefreshCurrentWorkspace);
                }
            }
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
                _ = self.input_tx.send(input::Request::EnableHotkeys);
                _ = self.input_tx.send(input::Request::SetEventProcessing(false));

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
                    self.new_app(pid, info, AppDiscoverySource::Process);
                }
                self.discovered = true;
            }
            SyncApps => self.sync_apps(true),
            AppLaunch(pid, info, source) => {
                self.new_app(pid, info, source);
            }
            AppGloballyActivated(pid) => {
                _ = self.input_tx.send(input::Request::EnforceHidden);
                self.events_tx.send(Event::ApplicationGloballyActivated(pid));
                self.retry_unattached_app(pid);
            }
            AppGloballyDeactivated(pid) => {
                self.events_tx.send(Event::ApplicationGloballyDeactivated(pid));
            }
            AppTerminated(pid) => {
                sys::app::remove_application_observer(pid);
                self.apps.terminate(pid);
            }
            AppsMayHaveTerminated => self.sync_apps(true),
            AppExited(pid, handle) => {
                let gave_up = self.apps.is_active(pid, &handle);
                if let Some(relaunch) = self.apps.exited(pid, &handle) {
                    self.events_tx.send(Event::AppActorExited(pid, handle));
                    if let Some((info, source)) = relaunch {
                        self.new_app(pid, info, source);
                    } else if gave_up && sys::app::is_running(pid) {
                        let delay = self.apps.attach_failed(pid, Instant::now());
                        warn!(pid, ?delay, "Could not attach a running app; trying again");
                        queue::main().after_f_s(
                            Time::new_after(Time::NOW, delay.as_nanos() as i64),
                            (self.sender.clone(), WmEvent::SyncApps),
                            |(sender, event)| sender.send(event),
                        );
                    }
                }
            }
            ConfigUpdated(new_cfg) => {
                if let Some(tx) = &self.mission_control_tx {
                    tx.send(mission_control::Event::Configure(
                        new_cfg.settings.ui.mission_control.clone(),
                        new_cfg.virtual_workspaces.mode
                            == crate::common::config::WorkspaceMode::Global,
                    ));
                }
                if !new_cfg.settings.ui.mission_control.enabled {
                    self.mission_control_tx.take();
                    self.input_tx.send(input::Request::ReleaseMissionControl);
                }
                self.config.config = new_cfg;

                _ = self.input_tx.send(input::Request::ConfigUpdated(self.config.config.clone()));
            }
            PowerStateChanged(is_low_power_mode) => {
                info!("Power state changed: low power mode = {}", is_low_power_mode);
                _ = self.input_tx.send(input::Request::SetLowPowerMode(is_low_power_mode));
            }
            KeyboardLayoutChanged => {
                _ = self.input_tx.send(input::Request::KeyboardLayoutChanged);
            }
            Command(Wm(ReloadConfig)) => self.reload_config(),
            Command(Wm(BindingMode(target))) => {
                _ = self.input_tx.send(input::Request::SetBindingMode(target));
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
            // Deprecated Current spelling forwards to the canonical Overview.
            Command(Wm(ShowMissionControlAll | ShowMissionControlCurrent)) => {
                if let Some(tx) = &self.mission_control_tx {
                    let _ = tx.try_send(mission_control::Event::ShowAll);
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
            Command(ConfiguredLayout(ConfiguredLayoutCommand::SwitchPresetColumnWidth)) => {
                self.events_tx.send(reactor::Event::Command(reactor::Command::Layout(
                    layout::LayoutCommand::SwitchPresetColumnWidth { backwards: false },
                )));
            }
            Command(ReactorCommand(cmd)) => {
                self.events_tx.send(reactor::Event::Command(cmd));
            }
            Command(Sequence(commands)) => {
                for command in commands {
                    self.handle_event(Command(command));
                }
            }
        }
        if sync {
            self.sync_apps(false);
        }
    }

    /// Match the actors to the running apps (`AppLifecycle::sync`). `now`
    /// runs it whenever called; otherwise at most once per `SYNC_INTERVAL`.
    fn sync_apps(&mut self, now: bool) {
        let at = Instant::now();
        if !now
            && self
                .synced_at
                .is_some_and(|last| at.saturating_duration_since(last) < SYNC_INTERVAL)
        {
            return;
        }
        self.synced_at = Some(at);
        let attachable = if self.discovered {
            sys::app::regular_apps().collect()
        } else {
            Vec::new()
        };
        let own = std::process::id() as pid_t;
        let sync = self.apps.sync(attachable, sys::app::is_running, own, at);
        for pid in sync.ended {
            debug!(?pid, "App no longer running; terminating its actor");
            sys::app::remove_application_observer(pid);
            self.apps.terminate(pid);
        }
        for (pid, info) in sync.attach {
            info!(pid, bundle = ?info.bundle_id, "Attaching a running app that has no actor");
            self.new_app(pid, info, AppDiscoverySource::Process);
        }
    }

    /// Attaching fails while an app refuses AX requests; retry when the user activates it.
    fn retry_unattached_app(&mut self, pid: pid_t) {
        if self.apps.is_attached(pid) {
            return;
        }
        let Some(running_app) = NSRunningApplication::with_process_id(pid) else {
            return;
        };
        self.new_app(pid, AppInfo::from(&*running_app), AppDiscoverySource::Process);
    }

    fn new_app(&mut self, pid: pid_t, info: AppInfo, source: AppDiscoverySource) {
        // Leave Rift's own UI to AppKit; managing it through AX causes focus feedback loops.
        if pid == std::process::id() as pid_t {
            return;
        }

        let Some(running_app) = NSRunningApplication::with_process_id(pid) else {
            debug!(?pid, "Failed to resolve NSRunningApplication for new app");
            return;
        };

        let policy = running_app.activationPolicy();
        if policy == NSApplicationActivationPolicy::Prohibited {
            return;
        }
        if !should_spawn_app(policy, source) {
            if policy != NSApplicationActivationPolicy::Accessory {
                return;
            }
            sys::app::ensure_activation_policy_observer(pid, running_app.clone(), info.clone());
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
            let override_handler = (source == AppDiscoverySource::WindowServer).then(|| {
                let sender = self.sender.clone();
                std::sync::Arc::new(move |pid, info| {
                    sender.send(WmEvent::AppLaunch(pid, info, AppDiscoverySource::WindowServer));
                }) as std::sync::Arc<dyn Fn(pid_t, AppInfo) + Send + Sync>
            });
            sys::app::ensure_finished_launching_observer(
                pid,
                running_app.clone(),
                info.clone(),
                override_handler,
            );
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

        if let Some((handle, rx)) = self.apps.reserve(pid, info.clone(), source) {
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
    fn discovery_source_controls_activation_policy_admission() {
        use AppDiscoverySource::{Process, WindowServer};
        let regular = NSApplicationActivationPolicy::Regular;
        let accessory = NSApplicationActivationPolicy::Accessory;
        let prohibited = NSApplicationActivationPolicy::Prohibited;
        for (policy, source, expected) in [
            (regular, Process, true),
            (regular, WindowServer, true),
            (accessory, Process, false),
            (accessory, WindowServer, true),
            (prohibited, Process, false),
            (prohibited, WindowServer, false),
        ] {
            assert_eq!(should_spawn_app(policy, source), expected);
        }
    }

    #[test]
    fn only_active_apps_that_stopped_running_have_ended() {
        let mut apps = AppLifecycle::default();
        let info = || AppInfo {
            bundle_id: None,
            localized_name: None,
        };
        for pid in [1, 2, 3] {
            let _ = apps.reserve(pid, info(), AppDiscoverySource::Process);
        }
        apps.terminate(3);
        assert_eq!(apps.ended(|pid| pid == 1), vec![2], "3 is stopping already");
        assert!(apps.ended(|_| true).is_empty());
    }

    fn info() -> AppInfo {
        AppInfo {
            bundle_id: None,
            localized_name: None,
        }
    }

    /// What `sync` attaches when `running` run, all of them attachable.
    fn attach(apps: &mut AppLifecycle, running: &[pid_t], own: pid_t, now: Instant) -> Vec<pid_t> {
        let attachable = running.iter().map(|&pid| (pid, ())).collect();
        let sync = apps.sync(attachable, |pid| running.contains(&pid), own, now);
        sync.attach.into_iter().map(|(pid, _)| pid).collect()
    }

    /// An app whose actor gave up while the app kept running is attached
    /// again without being activated.
    #[test]
    fn an_app_that_could_not_be_attached_is_attached_again_without_activation() {
        let own = 1;
        let mut apps = AppLifecycle::default();
        let start = Instant::now();

        let (handle, _rx) = apps.reserve(42, info(), AppDiscoverySource::Process).unwrap();
        assert!(
            apps.is_active(42, &handle),
            "it gave up; it was not asked to stop"
        );
        assert!(apps.exited(42, &handle).is_some());
        let first = apps.attach_failed(42, start);

        assert!(
            attach(&mut apps, &[own, 42], own, start + first / 2).is_empty(),
            "not before its retry is due"
        );
        assert_eq!(attach(&mut apps, &[own, 42], own, start + first), vec![42]);

        // Still busy: it waits longer before the next attempt.
        let retried = start + first;
        let (handle, _rx) = apps.reserve(42, info(), AppDiscoverySource::Process).unwrap();
        assert!(apps.exited(42, &handle).is_some());
        let second = apps.attach_failed(42, retried);
        assert!(second > first);
        assert!(attach(&mut apps, &[42], own, retried + first).is_empty());

        // Attached at last: nothing more to do for it.
        let (handle, _rx) = apps.reserve(42, info(), AppDiscoverySource::Process).unwrap();
        handle.mark_attached();
        assert!(attach(&mut apps, &[42], own, retried + second).is_empty());

        // Should that actor end while the app runs, it starts over from the first wait.
        assert!(apps.exited(42, &handle).is_some());
        assert_eq!(apps.attach_failed(42, retried + second), first);
    }

    #[test]
    fn running_apps_with_no_actor_are_attached() {
        let own = 1;
        let mut apps = AppLifecycle::default();
        let now = Instant::now();
        // 7's launch was missed; 8 has its actor; rift itself is never attached.
        let _ = apps.reserve(8, info(), AppDiscoverySource::Process);
        assert_eq!(attach(&mut apps, &[own, 7, 8], own, now), vec![7]);

        // An app asked to stop did not fail to attach.
        let (handle, _rx) = apps.reserve(9, info(), AppDiscoverySource::Process).unwrap();
        apps.terminate(9);
        assert!(!apps.is_active(9, &handle));

        // A failed app that quit is forgotten, so its pid starts fresh if reused.
        let (handle, _rx) = apps.reserve(10, info(), AppDiscoverySource::Process).unwrap();
        assert!(apps.exited(10, &handle).is_some());
        apps.attach_failed(10, now);
        assert!(attach(&mut apps, &[], own, now).is_empty());
        assert_eq!(attach(&mut apps, &[10], own, now), vec![10]);
    }

    #[test]
    fn sync_ends_actors_whose_app_quit_and_attaches_only_what_may_be() {
        let own = 1;
        let mut apps = AppLifecycle::default();
        let now = Instant::now();
        let _ = apps.reserve(2, info(), AppDiscoverySource::Process);
        let _ = apps.reserve(3, info(), AppDiscoverySource::WindowServer);
        // 2 quit; 3 runs though not attachable (an accessory app found by its
        // window); 4 runs and may be attached.
        let sync = apps.sync(vec![(4, ())], |pid| pid == 3 || pid == 4, own, now);
        assert_eq!(sync.ended, vec![2]);
        assert_eq!(
            sync.attach.into_iter().map(|(pid, _)| pid).collect::<Vec<_>>(),
            vec![4]
        );
    }

    #[test]
    fn dedupe_failure_termination_and_reuse() {
        for terminated in [false, true] {
            let mut apps = AppLifecycle::default();
            let info = || AppInfo {
                bundle_id: None,
                localized_name: None,
            };
            let (old, mut rx) = apps.reserve(42, info(), AppDiscoverySource::Process).unwrap();
            assert!(apps.reserve(42, info(), AppDiscoverySource::Process).is_none());
            if terminated {
                apps.terminate(42);
                assert!(matches!(rx.try_recv().unwrap().1, Request::Terminate));
                assert!(apps.reserve(42, info(), AppDiscoverySource::Process).is_none());
            }
            assert!(apps.exited(42, &old).is_some());
            let (new, _rx) = apps.reserve(42, info(), AppDiscoverySource::Process).unwrap();
            assert!(apps.exited(42, &old).is_none());
            assert!(!new.same_actor(&old));
            assert!(apps.reserve(42, info(), AppDiscoverySource::Process).is_none());
        }
    }
}
