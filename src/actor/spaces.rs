//! Authoritative native display/space state for Rift.
//!
//! This actor is the only place that should translate macOS display, space, and
//! session lifecycle signals into the snapshot consumed by the reactor. The
//! reactor owns Rift's virtual workspace model, but it must only do so on top of
//! a stable native-space picture. The rules here are therefore intentionally
//! conservative:
//!
//! - Sleep, display churn, and lock/login transitions buffer snapshots instead of
//!   forwarding them immediately.
//! - Only user spaces are allowed to become the reactor's workspace/display
//!   context. Fullscreen and system/login spaces are treated as transient native
//!   state and nulled out before they can rewrite workspace mappings.
//! - When the system finally stabilizes, this actor forwards a single coherent
//!   snapshot plus any synthesized window enter/leave deltas needed to reconcile
//!   the reactor with the post-churn WindowServer state.
//!
//! The core failure mode this prevents is treating unstable lock/wake/login
//! snapshots as authoritative user-space state. That can cause Rift to
//! initialize fresh default workspaces for transient spaces and later remap them
//! onto the real desktop, which looks like "all windows reset to workspace 1".

use dispatchr::queue;
use dispatchr::time::Time;
use objc2_core_foundation::CGSize;
use objc2_foundation::MainThreadMarker;

use crate::actor;
use crate::actor::{reactor, wm_controller};
use crate::common::collections::{HashMap, HashSet};
use crate::sys::dispatch::DispatchExt;
#[cfg(not(test))]
use crate::sys::screen::managed_display_space_ids;
use crate::sys::screen::{CoordinateConverter, ScreenCache, ScreenInfo, SpaceId};
use crate::sys::skylight::DisplayReconfigFlags;
use crate::sys::window_server::WindowServerId;
use crate::sys::{display_churn, window_server};

const REFRESH_DEFAULT_DELAY_NS: i64 = 100_000_000;
const REFRESH_SPACE_SWITCH_DELAY_NS: i64 = 50_000_000;
const REFRESH_RETRY_DELAY_NS: i64 = 100_000_000;
const REFRESH_MAX_RETRIES: u8 = 10;

// OmniWM debounces display changes at 100 ms and then rescans immediately.
// Rift still requires two identical topology samples plus a quiet WindowServer,
// but it should converge on the same order of magnitude rather than waiting
// multiple seconds before even attempting stabilization.
const DISPLAY_CHURN_QUIET_NS: i64 = 100_000_000;
const DISPLAY_STABILIZE_RETRY_NS: i64 = 100_000_000;
const DISPLAY_STABILIZE_MAX_ATTEMPTS: u8 = 10;
const DISPLAY_STABLE_REQUIRED_HITS: u8 = 2;

#[derive(Debug, Clone)]
pub enum Event {
    SystemWillSleep,
    SystemDidWake,
    SessionDidResignActive,
    SessionDidBecomeActive,
    ActiveDisplayChanged,
    ActiveSpaceChanged,
    ScreenRefreshRequested,
    DisplayReconfigured {
        display_id: u32,
        flags: DisplayReconfigFlags,
    },
    DisplayChurnBegin,
    DisplayChurnEnd,
    ScreenParametersChanged(Vec<ScreenInfo>, CoordinateConverter),
    SpaceChanged(Vec<Option<SpaceId>>),
    SpaceInventoryChanged,
    SpaceCreated(SpaceId),
    SpaceDestroyed(SpaceId),
    WindowServerAppeared(WindowServerId, SpaceId),
    WindowServerDestroyed(WindowServerId, SpaceId),
    ProcessScreenRefresh {
        attempt: u8,
    },
    CheckDisplayStabilization {
        expected_epoch: u64,
        attempt: u8,
    },
}

pub type Sender = actor::Sender<Event>;
type Receiver = actor::Receiver<Event>;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct QuarantineStats {
    pub appeared_dropped: u64,
    pub destroyed_dropped: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DisplayTopologyFingerprint(Vec<(String, u64, u64, u64, u64, Option<u64>)>);

#[derive(Debug, Clone)]
struct DisplayTopologyState {
    fingerprint: DisplayTopologyFingerprint,
    hits: u8,
}

/// Forwarded read-only space/display snapshot consumed by the reactor.
#[derive(Debug, Default, Clone)]
pub struct ForwardedSpaceState {
    pub screens: Vec<ScreenInfo>,
    pub fullscreen_spaces: HashSet<SpaceId>,
    pub has_seen_display_set: bool,
    pub active_spaces: HashSet<SpaceId>,
    pub menu_bar_space: Option<SpaceId>,
    pub command_space: Option<SpaceId>,
    pub display_space_ids: HashMap<String, Vec<SpaceId>>,
    pub last_user_space_by_display: HashMap<String, SpaceId>,
    pub space_remaps: Vec<(SpaceId, SpaceId)>,
    pub display_set_changed: bool,
    pub topology_changed: bool,
    pub allow_space_remap: bool,
    pub should_force_refresh_layout: bool,
    pub releases_lifecycle_refresh_quarantine: bool,
    /// Releases the reactor's display-churn gate only after this authoritative
    /// snapshot has been incorporated into its workspace model.
    pub releases_display_churn_refresh_quarantine: bool,
    pub resized_spaces: Vec<(SpaceId, CGSize)>,
    pub topology_window_delta: Option<TopologyWindowDelta>,
    pub active_window_spaces: HashMap<WindowServerId, SpaceId>,
}

impl ForwardedSpaceState {
    pub fn screen_by_space(&self, space: SpaceId) -> Option<&ScreenInfo> {
        self.screens.iter().find(|screen| screen.space == Some(space))
    }

    pub fn iter_known_spaces(&self) -> impl Iterator<Item = SpaceId> + '_ {
        self.screens.iter().filter_map(|screen| screen.space)
    }

    pub fn first_known_space(&self) -> Option<SpaceId> { self.iter_known_spaces().next() }
}

#[derive(Debug, Clone)]
struct PendingScreenParameters {
    screens: Vec<ScreenInfo>,
    converter: CoordinateConverter,
}

#[derive(Debug, Clone)]
pub struct TopologyWindowDelta {
    pub epoch: u64,
    pub flags: DisplayReconfigFlags,
    pub appeared: Vec<(WindowServerId, SpaceId)>,
    pub disappeared: Vec<(WindowServerId, SpaceId)>,
}

impl Default for TopologyWindowDelta {
    fn default() -> Self {
        Self {
            epoch: 0,
            flags: DisplayReconfigFlags::empty(),
            appeared: Vec::new(),
            disappeared: Vec::new(),
        }
    }
}

pub struct AuthorityState {
    pub sleeping: bool,
    pub session_inactive: bool,
    pub display_churn_active: bool,
    pub screens: Vec<ScreenInfo>,
    pub has_seen_display_set: bool,
    pub last_sent_spaces: Option<Vec<Option<SpaceId>>>,
    pub quarantine_stats: QuarantineStats,
    screen_cache: Option<ScreenCache>,
    last_converter: CoordinateConverter,
    refresh_pending: bool,
    display_churn_epoch: u64,
    display_churn_flags: DisplayReconfigFlags,
    display_topology_state: Option<DisplayTopologyState>,
    last_user_space_by_display: HashMap<String, SpaceId>,
    display_space_ids: HashMap<String, Vec<SpaceId>>,
    active_display_uuid: Option<String>,
    awaiting_space_switch_confirmation: bool,
    refresh_deferred_until_stable: bool,
    release_reactor_quarantine_on_next_forward: bool,
    pending_screen_parameters: Option<PendingScreenParameters>,
    pending_spaces: Option<Vec<Option<SpaceId>>>,
    visible_window_spaces: HashMap<WindowServerId, SpaceId>,
    pre_churn_visible_window_spaces: HashMap<WindowServerId, SpaceId>,
    pending_topology_window_delta: Option<TopologyWindowDelta>,
    timers_enabled: bool,
}

impl Default for AuthorityState {
    fn default() -> Self {
        Self {
            sleeping: false,
            session_inactive: false,
            display_churn_active: false,
            screens: Vec::new(),
            has_seen_display_set: false,
            last_sent_spaces: None,
            quarantine_stats: QuarantineStats::default(),
            screen_cache: None,
            last_converter: CoordinateConverter::default(),
            refresh_pending: false,
            display_churn_epoch: 0,
            display_churn_flags: DisplayReconfigFlags::empty(),
            display_topology_state: None,
            last_user_space_by_display: HashMap::default(),
            display_space_ids: HashMap::default(),
            active_display_uuid: None,
            awaiting_space_switch_confirmation: false,
            refresh_deferred_until_stable: false,
            release_reactor_quarantine_on_next_forward: false,
            pending_screen_parameters: None,
            pending_spaces: None,
            visible_window_spaces: HashMap::default(),
            pre_churn_visible_window_spaces: HashMap::default(),
            pending_topology_window_delta: None,
            timers_enabled: true,
        }
    }
}

impl AuthorityState {
    fn runtime() -> Self {
        let mut state = Self::default();
        state.screen_cache = Some(ScreenCache::new(MainThreadMarker::new().unwrap()));
        state
    }
}

pub struct SpacesActor {
    sender: Sender,
    receiver: Receiver,
    reactor_tx: reactor::Sender,
    wm_tx: wm_controller::Sender,
    state: AuthorityState,
}

impl SpacesActor {
    pub fn new(reactor_tx: reactor::Sender, wm_tx: wm_controller::Sender) -> (Self, Sender) {
        Self::new_with_state(reactor_tx, wm_tx, AuthorityState::runtime())
    }

    fn new_with_state(
        reactor_tx: reactor::Sender,
        wm_tx: wm_controller::Sender,
        state: AuthorityState,
    ) -> (Self, Sender) {
        let (sender, receiver) = actor::channel();
        (
            Self {
                sender: sender.clone(),
                receiver,
                reactor_tx,
                wm_tx,
                state,
            },
            sender,
        )
    }

    #[cfg(test)]
    pub fn new_for_tests(
        reactor_tx: reactor::Sender,
        wm_tx: wm_controller::Sender,
    ) -> (Self, Sender) {
        let mut state = AuthorityState::default();
        state.timers_enabled = false;
        Self::new_with_state(reactor_tx, wm_tx, state)
    }

    pub async fn run(mut self) {
        while let Some((span, event)) = self.receiver.recv().await {
            let _guard = span.enter();
            self.handle_event(event);
        }
    }

    fn handle_event(&mut self, event: Event) {
        match event {
            Event::SystemWillSleep => {
                self.state.sleeping = true;
                self.state.release_reactor_quarantine_on_next_forward = false;
                self.reactor_tx.send(reactor::Event::SystemWillSleep);
                if let Some(screen_cache) = self.state.screen_cache.as_mut() {
                    screen_cache.mark_sleeping(true);
                }
            }
            Event::SystemDidWake => {
                self.state.sleeping = false;
                self.reactor_tx.send(reactor::Event::SystemWoke);
                if let Some(screen_cache) = self.state.screen_cache.as_mut() {
                    screen_cache.mark_sleeping(false);
                    screen_cache.mark_dirty();
                }
                self.state.pending_screen_parameters = None;
                self.state.pending_spaces = None;
                if self.state.display_churn_active {
                    let expected_epoch = self.state.display_churn_epoch;
                    self.schedule_display_stabilization_check(expected_epoch);
                }
                // Wake is inherently unstable; discard anything buffered while the
                // machine was asleep and wait for a fresh authoritative rescan.
                self.state.release_reactor_quarantine_on_next_forward = true;
                self.schedule_screen_refresh();
            }
            Event::SessionDidResignActive => {
                self.state.session_inactive = true;
                self.state.release_reactor_quarantine_on_next_forward = false;
                self.reactor_tx.send(reactor::Event::SessionDidResignActive);
            }
            Event::SessionDidBecomeActive => {
                self.state.session_inactive = false;
                self.reactor_tx.send(reactor::Event::SessionDidBecomeActive);
                if let Some(screen_cache) = self.state.screen_cache.as_mut() {
                    screen_cache.mark_dirty();
                }
                self.state.pending_screen_parameters = None;
                self.state.pending_spaces = None;
                if self.state.display_churn_active {
                    let expected_epoch = self.state.display_churn_epoch;
                    self.schedule_display_stabilization_check(expected_epoch);
                }
                // The login window can transiently replace every display's current
                // space. Do not replay buffered lock-screen snapshots into Rift's
                // workspace model; always resample after the user session becomes
                // active again.
                self.state.release_reactor_quarantine_on_next_forward = true;
                self.schedule_screen_refresh();
            }
            Event::ActiveDisplayChanged => {
                self.handle_active_display_changed();
            }
            Event::ActiveSpaceChanged => {
                self.handle_active_space_changed();
            }
            Event::ScreenRefreshRequested => {
                // Preference/system-driven screen changes can transiently report stale
                // geometry; keep using the default delayed refresh path.
                self.schedule_screen_refresh();
            }
            Event::DisplayReconfigured { display_id, flags } => {
                self.handle_display_reconfig_event(display_id, flags);
            }
            Event::DisplayChurnBegin => {
                self.state.display_churn_active = true;
                self.reactor_tx.send(reactor::Event::DisplayChurnBegin);
            }
            Event::DisplayChurnEnd => {
                self.state.display_churn_active = false;
                self.flush_pending_if_stable();
                self.schedule_screen_refresh();
            }
            Event::ScreenParametersChanged(screens, converter) => {
                if self.should_buffer_topology_updates() {
                    self.state.pending_screen_parameters =
                        Some(PendingScreenParameters { screens, converter });
                } else {
                    self.forward_screen_parameters(screens, converter);
                }
            }
            Event::SpaceChanged(spaces) => {
                if self.should_buffer_topology_updates() {
                    self.state.pending_spaces = Some(spaces);
                } else {
                    self.forward_space_snapshot(spaces);
                }
            }
            Event::SpaceInventoryChanged => {
                self.handle_space_inventory_changed();
            }
            Event::SpaceCreated(space) => {
                if !self.should_buffer_topology_updates() && self.classify_space(space).is_some() {
                    self.reactor_tx.send(reactor::Event::SpaceCreated(space));
                }
                self.handle_space_inventory_changed();
            }
            Event::SpaceDestroyed(space) => {
                if !self.should_buffer_topology_updates() && self.classify_space(space).is_some() {
                    self.reactor_tx.send(reactor::Event::SpaceDestroyed(space));
                }
                self.handle_space_inventory_changed();
            }
            Event::WindowServerAppeared(wsid, sid) => {
                if self.should_quarantine_window_space_event() {
                    self.state.quarantine_stats.appeared_dropped += 1;
                } else {
                    self.state.visible_window_spaces.insert(wsid, sid);
                    if let Some(kind) = self.classify_space(sid) {
                        self.reactor_tx.send(reactor::Event::WindowServerAppeared(wsid, sid, kind));
                    }
                }
            }
            Event::WindowServerDestroyed(wsid, sid) => {
                if self.should_quarantine_window_space_event() {
                    self.state.quarantine_stats.destroyed_dropped += 1;
                } else {
                    self.state.visible_window_spaces.remove(&wsid);
                    let current_space = window_server::window_space(wsid);
                    if let Some(kind) = self.classify_space(sid) {
                        if matches!(kind, reactor::SpaceEventKind::User)
                            && let Some(current_space) = current_space
                            && current_space != sid
                        {
                            // WindowServer reports a confirmed move out of the origin
                            // space as a destroy. The selected Space may remain the
                            // same, so explicitly forward the refreshed membership.
                            self.handle_space_inventory_changed();
                            return;
                        }
                        self.reactor_tx
                            .send(reactor::Event::WindowServerDestroyed(wsid, sid, kind));
                    }
                }
            }
            Event::ProcessScreenRefresh { attempt } => {
                self.process_screen_refresh(attempt, true);
            }
            Event::CheckDisplayStabilization { expected_epoch, attempt } => {
                self.attempt_finish_display_churn(expected_epoch, attempt);
            }
        }
    }

    fn handle_active_display_changed(&mut self) {
        #[cfg(not(test))]
        let active_display_uuid = crate::sys::screen::active_menu_bar_display_uuid();
        #[cfg(test)]
        let active_display_uuid: Option<String> = None;

        self.handle_active_display_changed_for(active_display_uuid.as_deref());
    }

    fn handle_active_display_changed_for(&mut self, active_display_uuid: Option<&str>) {
        if self.active_display_matches_state(active_display_uuid) {
            return;
        }

        if self.should_buffer_topology_updates() {
            self.schedule_screen_refresh_after(0, 0);
            return;
        }

        if let Some(active_display_uuid) = active_display_uuid
            && let Some(active_space) = self
                .state
                .screens
                .iter()
                .find(|screen| screen.display_uuid == active_display_uuid)
                .and_then(|screen| screen.space)
        {
            self.state.active_display_uuid = Some(active_display_uuid.to_string());
            self.reactor_tx.send(reactor::Event::ActiveDisplayChanged {
                menu_bar_space: Some(active_space),
                command_space: Some(active_space),
            });
            return;
        }

        if !self.try_forward_authoritative_snapshot(true, true) {
            self.schedule_screen_refresh_after(0, 0);
        }
    }

    fn active_display_matches_state(&self, active_display_uuid: Option<&str>) -> bool {
        active_display_uuid.is_some()
            && active_display_uuid == self.state.active_display_uuid.as_deref()
    }

    fn handle_active_space_changed(&mut self) {
        self.state.awaiting_space_switch_confirmation = true;

        if self.should_buffer_topology_updates() {
            self.schedule_screen_refresh_after(REFRESH_SPACE_SWITCH_DELAY_NS, 0);
            return;
        }

        if !self.try_forward_authoritative_snapshot(false, true) {
            self.schedule_screen_refresh_after(REFRESH_SPACE_SWITCH_DELAY_NS, 0);
        }
    }

    fn handle_space_inventory_changed(&mut self) {
        if self.should_buffer_topology_updates() {
            self.schedule_screen_refresh_after(0, 0);
            return;
        }

        // Space create/destroy and membership changes can leave the visible-space vector
        // unchanged while still changing authoritative per-display space metadata.
        if !self.try_forward_authoritative_snapshot(true, true) {
            self.schedule_screen_refresh_after(REFRESH_RETRY_DELAY_NS, 0);
        }
    }

    fn should_buffer_topology_updates(&self) -> bool {
        self.state.sleeping || self.state.session_inactive || self.state.display_churn_active
    }

    fn should_quarantine_window_space_event(&self) -> bool {
        self.state.sleeping || self.state.session_inactive || self.state.display_churn_active
    }

    fn collect_state(&mut self) -> Option<(Vec<ScreenInfo>, CoordinateConverter)> {
        self.state
            .screen_cache
            .as_mut()
            .and_then(|screen_cache| screen_cache.refresh())
            .or_else(|| {
                #[cfg(test)]
                {
                    Some((self.state.screens.clone(), self.state.last_converter))
                }
                #[cfg(not(test))]
                {
                    None
                }
            })
    }

    fn forward_screen_parameters(
        &mut self,
        screens: Vec<ScreenInfo>,
        converter: CoordinateConverter,
    ) {
        self.state.last_converter = converter;
        let forwarded = self.build_forwarded_state(screens);
        self.state.last_sent_spaces = Some(Self::screen_spaces(&forwarded.screens));
        self.state.awaiting_space_switch_confirmation = false;
        self.wm_tx.send(wm_controller::WmEvent::SpaceStateUpdated(
            forwarded,
            self.state.last_converter,
        ));
    }

    fn forward_space_snapshot(&mut self, spaces: Vec<Option<SpaceId>>) {
        if self.state.last_sent_spaces.as_ref() == Some(&spaces) {
            return;
        }
        if spaces.len() != self.state.screens.len() {
            if !self.try_forward_authoritative_snapshot(true, true) {
                self.schedule_screen_refresh_after(0, 0);
            }
            return;
        }
        let mut screens = self.state.screens.clone();
        for (screen, space) in screens.iter_mut().zip(spaces.iter().copied()) {
            screen.space = space;
        }
        self.state.last_sent_spaces = Some(spaces.clone());
        let forwarded = self.build_forwarded_state(screens);
        self.state.awaiting_space_switch_confirmation = false;
        self.wm_tx.send(wm_controller::WmEvent::SpaceStateUpdated(
            forwarded,
            self.state.last_converter,
        ));
    }

    fn build_forwarded_state(&mut self, screens: Vec<ScreenInfo>) -> ForwardedSpaceState {
        let previous_screens = self.state.screens.clone();
        let fullscreen_spaces: HashSet<SpaceId> = screens
            .iter()
            .filter_map(|screen| screen.space)
            .filter(|space| Self::is_fullscreen_space(*space))
            .collect();
        let mut screens = screens;
        self.preserve_user_spaces_during_fullscreen_transition(&previous_screens, &mut screens);
        self.null_fullscreen_spaces(&mut screens);
        self.null_non_user_spaces(&mut screens);

        let previous_displays: HashSet<String> =
            previous_screens.iter().map(|screen| screen.display_uuid.clone()).collect();
        let new_displays: HashSet<String> =
            screens.iter().map(|screen| screen.display_uuid.clone()).collect();
        let display_set_changed = previous_displays != new_displays;
        let display_order_changed = previous_screens
            .iter()
            .map(|screen| screen.display_uuid.as_str())
            .ne(screens.iter().map(|screen| screen.display_uuid.as_str()));
        let previous_frames: HashMap<_, _> = previous_screens
            .iter()
            .map(|screen| (screen.display_uuid.as_str(), screen.frame))
            .collect();
        let display_geometry_changed = screens.iter().any(|screen| {
            previous_frames
                .get(screen.display_uuid.as_str())
                .is_some_and(|previous| *previous != screen.frame)
        });
        let topology_changed =
            display_set_changed || display_order_changed || display_geometry_changed;
        let should_force_refresh_layout =
            topology_changed && (self.state.has_seen_display_set || !previous_displays.is_empty());

        let previous_sizes: HashMap<_, _> =
            previous_screens.iter().map(|screen| (screen.id, screen.frame.size)).collect();
        let resized_spaces = screens
            .iter()
            .filter_map(|screen| {
                let new_size = screen.frame.size;
                match previous_sizes.get(&screen.id) {
                    Some(previous) => {
                        let width_changed =
                            previous.width.round() as i32 != new_size.width.round() as i32;
                        let height_changed =
                            previous.height.round() as i32 != new_size.height.round() as i32;
                        if width_changed || height_changed {
                            screen.space.map(|space| (space, new_size))
                        } else {
                            None
                        }
                    }
                    None => screen.space.map(|space| (space, new_size)),
                }
            })
            .collect();

        let has_duplicate_spaces = {
            let mut unique_spaces: HashSet<SpaceId> = HashSet::default();
            screens
                .iter()
                .filter_map(|screen| screen.space)
                .any(|space| !unique_spaces.insert(space))
        };
        let allow_space_remap = should_force_refresh_layout
            && !has_duplicate_spaces
            && screens.iter().all(|screen| screen.space.is_some());
        let space_remaps = self.compute_space_remaps(&screens, allow_space_remap);
        let menu_bar_space = self.resolve_menu_bar_space(&screens);
        #[cfg(not(test))]
        let active_display_uuid = crate::sys::screen::active_menu_bar_display_uuid();
        #[cfg(test)]
        let active_display_uuid: Option<String> = None;
        let command_space = self.resolve_command_space(&screens, active_display_uuid.as_deref());
        self.state.active_display_uuid = active_display_uuid
            .filter(|uuid| screens.iter().any(|screen| screen.display_uuid == *uuid))
            .or_else(|| {
                command_space.and_then(|space| {
                    screens
                        .iter()
                        .find(|screen| screen.space == Some(space))
                        .map(|screen| screen.display_uuid.clone())
                })
            });
        #[cfg(test)]
        {
            let mut display_space_ids: HashMap<String, Vec<SpaceId>> = HashMap::default();
            for screen in &screens {
                if let Some(space) = screen.space {
                    display_space_ids.entry(screen.display_uuid.clone()).or_default().push(space);
                }
            }
            self.state.display_space_ids = display_space_ids;
        }
        #[cfg(not(test))]
        {
            self.state.display_space_ids = managed_display_space_ids();
        }

        if !screens.is_empty() {
            self.state.has_seen_display_set = true;
        }
        self.state.visible_window_spaces = self.visible_window_spaces_for_screens(&screens);
        self.state.screens = screens.clone();
        let releases_lifecycle_refresh_quarantine =
            std::mem::take(&mut self.state.release_reactor_quarantine_on_next_forward);
        ForwardedSpaceState {
            screens,
            fullscreen_spaces,
            has_seen_display_set: self.state.has_seen_display_set,
            active_spaces: self.state.screens.iter().filter_map(|screen| screen.space).collect(),
            menu_bar_space,
            command_space,
            display_space_ids: self.state.display_space_ids.clone(),
            last_user_space_by_display: self.state.last_user_space_by_display.clone(),
            space_remaps,
            display_set_changed,
            topology_changed,
            allow_space_remap,
            should_force_refresh_layout,
            releases_lifecycle_refresh_quarantine,
            // Every coherent authoritative snapshot is a valid acknowledgement for
            // the reactor's display-churn gate, including ordinary refreshes after
            // stabilization has already ended.
            releases_display_churn_refresh_quarantine: true,
            resized_spaces,
            topology_window_delta: self.state.pending_topology_window_delta.take(),
            active_window_spaces: self.state.visible_window_spaces.clone(),
        }
    }

    fn screen_spaces(screens: &[ScreenInfo]) -> Vec<Option<SpaceId>> {
        screens.iter().map(|screen| screen.space).collect()
    }

    fn preserve_user_spaces_during_fullscreen_transition(
        &self,
        previous_screens: &[ScreenInfo],
        screens: &mut [ScreenInfo],
    ) {
        let previous_by_display: HashMap<&str, &ScreenInfo> = previous_screens
            .iter()
            .filter_map(|screen| screen.display_uuid_opt().map(|uuid| (uuid, screen)))
            .collect();
        let entering_fullscreen = screens.iter().any(|next| {
            let Some(display_uuid) = next.display_uuid_opt() else {
                return false;
            };
            let Some(new_space) = next.space else {
                return false;
            };
            Self::is_fullscreen_space(new_space)
                && previous_by_display
                    .get(display_uuid)
                    .and_then(|screen| screen.space)
                    .is_some_and(|previous_space| !Self::is_fullscreen_space(previous_space))
        });
        if !entering_fullscreen {
            return;
        }

        let previous_space_owner: HashMap<SpaceId, &str> = previous_screens
            .iter()
            .filter_map(|screen| {
                let display_uuid = screen.display_uuid_opt()?;
                let space = screen.space?;
                (!Self::is_fullscreen_space(space)).then_some((space, display_uuid))
            })
            .collect();

        for next in screens.iter_mut() {
            let Some(display_uuid) = next.display_uuid_opt() else {
                continue;
            };
            let Some(previous) = previous_by_display.get(display_uuid).copied() else {
                continue;
            };
            let Some(new_space) = next.space else {
                continue;
            };
            if Self::is_fullscreen_space(new_space) {
                continue;
            }
            let Some(previous_space) = previous.space else {
                continue;
            };
            if previous_space == new_space || Self::is_fullscreen_space(previous_space) {
                continue;
            }
            let should_rewrite =
                previous_space_owner.get(&new_space).is_some_and(|owner| *owner != display_uuid);
            if !should_rewrite {
                continue;
            }

            next.space = Some(previous_space);
        }
    }

    fn null_fullscreen_spaces(&self, screens: &mut [ScreenInfo]) {
        for screen in screens {
            if screen.space.is_some_and(Self::is_fullscreen_space) {
                screen.space = None;
            }
        }
    }

    fn null_non_user_spaces(&self, screens: &mut [ScreenInfo]) {
        for screen in screens {
            if screen.space.is_some_and(|space| {
                !Self::is_fullscreen_space(space) && !Self::is_user_space(space)
            }) {
                screen.space = None;
            }
        }
    }

    fn compute_space_remaps(
        &mut self,
        screens: &[ScreenInfo],
        allow_space_remap: bool,
    ) -> Vec<(SpaceId, SpaceId)> {
        let mut remaps = Vec::new();
        let mut seen_displays: HashSet<String> = HashSet::default();
        let current_space_owners: HashMap<SpaceId, &str> = screens
            .iter()
            .filter_map(|screen| Some((screen.space?, screen.display_uuid_opt()?)))
            .collect();
        let historical_space_owners: HashMap<SpaceId, String> = self
            .state
            .last_user_space_by_display
            .iter()
            .map(|(display_uuid, &space)| (space, display_uuid.clone()))
            .collect();

        for screen in screens {
            let Some(space) = screen.space else {
                continue;
            };
            let Some(display_uuid) = screen.display_uuid_opt() else {
                continue;
            };
            if !seen_displays.insert(display_uuid.to_string()) {
                continue;
            }

            // During sleep/wake macOS can briefly report the remaining display with
            // the space belonging to a display that has not reappeared yet. Treat
            // that as cross-display contamination: accepting it poisons history and
            // the eventual stable snapshot remaps the built-in display's layout onto
            // the external display (resetting virtual workspaces in the process).
            let target_belongs_to_another_display =
                historical_space_owners.get(&space).is_some_and(|owner| *owner != display_uuid);
            if target_belongs_to_another_display {
                continue;
            }

            if let Some(previous_space) =
                self.state.last_user_space_by_display.get(display_uuid).copied()
                && previous_space != space
            {
                let source_is_now_owned_by_another_display = current_space_owners
                    .get(&previous_space)
                    .is_some_and(|owner| *owner != display_uuid);
                if allow_space_remap && !source_is_now_owned_by_another_display {
                    remaps.push((previous_space, space));
                }
            }

            self.state.last_user_space_by_display.insert(display_uuid.to_string(), space);
        }

        remaps
    }

    fn resolve_command_space(
        &self,
        screens: &[ScreenInfo],
        active_display_uuid: Option<&str>,
    ) -> Option<SpaceId> {
        #[cfg(test)]
        {
            let _ = active_display_uuid;
            Self::resolve_active_display_space(screens, None, None)
                .or_else(|| self.state.screens.iter().find_map(|screen| screen.space))
        }
        #[cfg(not(test))]
        {
            let active_space = crate::sys::screen::get_active_space_number();
            if let Some(space) =
                Self::resolve_active_display_space(screens, active_display_uuid, active_space)
            {
                return Some(space);
            }

            screens.iter().find_map(|screen| screen.space)
        }
    }

    fn resolve_active_display_space(
        screens: &[ScreenInfo],
        active_display_uuid: Option<&str>,
        active_space: Option<SpaceId>,
    ) -> Option<SpaceId> {
        active_display_uuid
            .and_then(|uuid| {
                screens
                    .iter()
                    .find(|screen| screen.display_uuid == uuid)
                    .and_then(|screen| screen.space)
            })
            .or_else(|| {
                active_space
                    .filter(|space| screens.iter().any(|screen| screen.space == Some(*space)))
            })
            .or_else(|| screens.iter().find_map(|screen| screen.space))
    }

    fn resolve_menu_bar_space(&self, screens: &[ScreenInfo]) -> Option<SpaceId> {
        #[cfg(test)]
        {
            screens
                .iter()
                .find_map(|screen| screen.space)
                .or_else(|| self.state.screens.iter().find_map(|screen| screen.space))
        }
        #[cfg(not(test))]
        {
            if let Some(active_space) = crate::sys::screen::get_active_space_number()
                && screens.iter().any(|screen| screen.space == Some(active_space))
            {
                return Some(active_space);
            }

            screens.iter().find_map(|screen| screen.space)
        }
    }

    fn is_fullscreen_space(space: SpaceId) -> bool {
        #[cfg(test)]
        {
            space.get() >= 0x400000000
        }
        #[cfg(not(test))]
        {
            window_server::space_is_fullscreen(space.get())
        }
    }

    fn is_user_space(space: SpaceId) -> bool {
        #[cfg(test)]
        {
            let _ = space;
            true
        }
        #[cfg(not(test))]
        {
            window_server::space_is_user(space.get())
        }
    }

    fn classify_space(&self, space: SpaceId) -> Option<reactor::SpaceEventKind> {
        if Self::is_fullscreen_space(space) {
            Some(reactor::SpaceEventKind::Fullscreen)
        } else {
            Self::is_user_space(space).then_some(reactor::SpaceEventKind::User)
        }
    }

    fn visible_window_spaces_for_screens(
        &self,
        screens: &[ScreenInfo],
    ) -> HashMap<WindowServerId, SpaceId> {
        let mut active_spaces = Vec::new();
        let mut active_space_set = HashSet::default();
        for space in screens.iter().filter_map(|screen| screen.space) {
            if active_space_set.insert(space) {
                active_spaces.push(space);
            }
        }

        if active_spaces.is_empty() {
            return HashMap::default();
        }

        // A global visible-window union is not space-aware and can lag one display
        // behind another. Query every active native space independently, including
        // in tests where the per-space query is overridden.
        let mut visible = HashMap::default();
        for &space in &active_spaces {
            for wsid in window_server::space_window_list_for_connection(&[space.get()], 0, false)
                .into_iter()
                .map(WindowServerId::new)
            {
                Self::record_visible_window_space(
                    &mut visible,
                    &self.state.visible_window_spaces,
                    &active_space_set,
                    wsid,
                    space,
                    window_server::window_space(wsid),
                );
            }
        }

        // The first coherent snapshot after wake/unlock can race WindowServer and
        // temporarily contain no windows. Preserve the last accepted membership
        // only while releasing that lifecycle quarantine. Outside recovery, an
        // empty result is authoritative (and is required to reconcile windows
        // whose destroy notifications were quarantined during display churn).
        if visible.is_empty() && self.state.release_reactor_quarantine_on_next_forward {
            self.state
                .visible_window_spaces
                .iter()
                .filter_map(|(&wsid, &space)| {
                    active_space_set.contains(&space).then_some((wsid, space))
                })
                .collect()
        } else {
            visible
        }
    }

    fn record_visible_window_space(
        visible: &mut HashMap<WindowServerId, SpaceId>,
        previous_visible: &HashMap<WindowServerId, SpaceId>,
        active_spaces: &HashSet<SpaceId>,
        wsid: WindowServerId,
        candidate_space: SpaceId,
        authoritative_space: Option<SpaceId>,
    ) {
        match visible.entry(wsid) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(candidate_space);
            }
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                if *entry.get() == candidate_space {
                    return;
                }

                // `space_window_list_for_connection([space])` is the authoritative
                // source for "window X is visible in active space Y". The extra
                // `window_space(wsid)` lookup is only used to disambiguate the rare
                // case where the same WSID appears in more than one active-space
                // query (for example during native transitions). If that secondary
                // lookup races and returns `None`, preserve the last known active
                // assignment instead of synthesizing a disappearance.
                let resolved = authoritative_space
                    .filter(|space| active_spaces.contains(space))
                    .or_else(|| {
                        previous_visible
                            .get(&wsid)
                            .copied()
                            .filter(|space| active_spaces.contains(space))
                    })
                    .unwrap_or(*entry.get());

                entry.insert(resolved);
            }
        }
    }

    fn synthesize_topology_window_delta(
        &mut self,
        epoch: u64,
        flags: DisplayReconfigFlags,
        screens: &[ScreenInfo],
    ) {
        let current = self.visible_window_spaces_for_screens(screens);
        let previous = std::mem::take(&mut self.state.pre_churn_visible_window_spaces);

        let mut appeared = Vec::new();
        let mut disappeared = Vec::new();

        for (&wsid, &space) in &current {
            match previous.get(&wsid).copied() {
                None => appeared.push((wsid, space)),
                Some(previous_space) if previous_space != space => {
                    disappeared.push((wsid, previous_space));
                    appeared.push((wsid, space));
                }
                Some(_) => {}
            }
        }

        for (&wsid, &space) in &previous {
            if !current.contains_key(&wsid) {
                disappeared.push((wsid, space));
            }
        }

        self.state.pending_topology_window_delta = Some(TopologyWindowDelta {
            epoch,
            flags,
            appeared,
            disappeared,
        });
        self.state.visible_window_spaces = current;
    }

    fn flush_pending_if_stable(&mut self) {
        if self.should_buffer_topology_updates() {
            return;
        }

        let pending_screen_parameters = self.state.pending_screen_parameters.take();
        let pending_spaces = self.state.pending_spaces.take();

        match (pending_screen_parameters, pending_spaces) {
            (Some(pending), Some(spaces)) if pending.screens.len() == spaces.len() => {
                // These two callbacks describe one native snapshot. Merge them before
                // forwarding so the reactor's churn gate cannot observe the topology
                // with stale space IDs and release between two WM events.
                let mut screens = pending.screens;
                for (screen, space) in screens.iter_mut().zip(spaces) {
                    screen.space = space;
                }
                self.forward_screen_parameters(screens, pending.converter);
            }
            (Some(_), Some(_)) => {
                // A topology/space count mismatch is not coherent enough to commit.
                // Resample once the native state is ready instead of forwarding either
                // half of the buffered snapshot.
                if !self.try_forward_authoritative_snapshot(true, true) {
                    self.schedule_screen_refresh_after(0, 0);
                }
            }
            (Some(pending), None) => {
                self.forward_screen_parameters(pending.screens, pending.converter);
            }
            (None, Some(spaces)) => {
                self.forward_space_snapshot(spaces);
            }
            (None, None) => {}
        }
    }

    fn screen_snapshot_is_valid_for_commit(screens: &[ScreenInfo]) -> bool {
        let mut seen_user_spaces: HashSet<SpaceId> = HashSet::default();
        screens.iter().all(|screen| match screen.space {
            Some(space) if !Self::is_fullscreen_space(space) => seen_user_spaces.insert(space),
            _ => true,
        })
    }

    fn screen_snapshot_is_ready_for_authoritative_commit(
        screens: &[ScreenInfo],
        require_complete_spaces: bool,
    ) -> bool {
        !screens.is_empty()
            && (!require_complete_spaces || screens.iter().all(|screen| screen.space.is_some()))
            && Self::screen_snapshot_is_valid_for_commit(screens)
    }

    fn process_screen_refresh(&mut self, attempt: u8, allow_retry: bool) {
        if self.should_buffer_topology_updates() {
            self.state.refresh_deferred_until_stable = true;
            self.state.refresh_pending = false;
            return;
        }

        let Some((screens, converter)) = self.collect_state() else {
            if allow_retry && attempt < REFRESH_MAX_RETRIES {
                self.schedule_screen_refresh_after(REFRESH_RETRY_DELAY_NS, attempt + 1);
                return;
            }
            self.finish_screen_refresh_attempts();
            return;
        };

        if !Self::screen_snapshot_is_ready_for_authoritative_commit(&screens, true) {
            if allow_retry && attempt < REFRESH_MAX_RETRIES {
                self.schedule_screen_refresh_after(REFRESH_RETRY_DELAY_NS, attempt + 1);
                return;
            }
            self.finish_screen_refresh_attempts();
            return;
        }

        let spaces: Vec<Option<SpaceId>> = screens.iter().map(|screen| screen.space).collect();
        if self.state.awaiting_space_switch_confirmation
            && self.state.last_sent_spaces.as_ref() == Some(&spaces)
            && allow_retry
            && attempt < REFRESH_MAX_RETRIES
        {
            self.schedule_screen_refresh_after(REFRESH_SPACE_SWITCH_DELAY_NS, attempt + 1);
            return;
        }

        self.forward_screen_parameters(screens, converter);
        self.state.awaiting_space_switch_confirmation = false;
        self.state.refresh_pending = false;
    }

    fn finish_screen_refresh_attempts(&mut self) {
        self.state.refresh_pending = false;

        // Wake and unlock set this flag before the refresh starts, and it is
        // consumed only by build_forwarded_state after a coherent snapshot is
        // forwarded. Keep trying when the bounded retry sequence expires; if
        // we stop here, the reactor's lifecycle quarantine can never be
        // released because no later snapshot is guaranteed to arrive.
        if self.state.release_reactor_quarantine_on_next_forward {
            self.schedule_screen_refresh_after(REFRESH_RETRY_DELAY_NS, 0);
        }
    }

    fn try_forward_authoritative_snapshot(
        &mut self,
        force: bool,
        require_complete_spaces: bool,
    ) -> bool {
        if self.state.refresh_pending || self.state.display_churn_active {
            return false;
        }

        let Some((screens, converter)) = self.collect_state() else {
            return false;
        };
        if !Self::screen_snapshot_is_ready_for_authoritative_commit(
            &screens,
            require_complete_spaces,
        ) {
            return false;
        }

        let spaces: Vec<Option<SpaceId>> = screens.iter().map(|screen| screen.space).collect();
        if !force && self.state.last_sent_spaces.as_ref() == Some(&spaces) {
            return false;
        }

        self.forward_screen_parameters(screens, converter);
        true
    }

    fn schedule_screen_refresh(&mut self) {
        self.schedule_screen_refresh_after(REFRESH_DEFAULT_DELAY_NS, 0);
    }

    fn schedule_screen_refresh_after(&mut self, delay_ns: i64, attempt: u8) {
        if !self.state.timers_enabled {
            return;
        }

        if attempt == 0 && self.state.display_churn_active {
            self.state.refresh_deferred_until_stable = true;
            return;
        }

        if attempt == 0 {
            if self.state.refresh_pending {
                return;
            }
            self.state.refresh_pending = true;
        } else if !self.state.refresh_pending {
            self.state.refresh_pending = true;
        }

        let sender = self.sender.clone();
        queue::main().after_f_s(
            Time::new_after(Time::NOW, delay_ns),
            (sender, attempt),
            |(sender, attempt)| sender.send(Event::ProcessScreenRefresh { attempt }),
        );
    }

    fn handle_display_reconfig_event(&mut self, _display_id: u32, flags: DisplayReconfigFlags) {
        if !Self::should_begin_display_churn(flags) {
            return;
        }
        let expected_epoch = self.begin_display_churn(flags);
        if let Some(screen_cache) = self.state.screen_cache.as_mut() {
            screen_cache.mark_dirty();
        }
        self.schedule_display_stabilization_check(expected_epoch);
    }

    fn should_begin_display_churn(flags: DisplayReconfigFlags) -> bool {
        // Native macOS space switches can still emit CGDisplay reconfig callbacks on a
        // single physical display. Churn quarantine is only for unstable physical display
        // topology, not ordinary current-space changes which are handled separately.
        // External display setting changes like main-display promotion and desktop shape
        // updates can temporarily destabilize active-space mappings, so they must be
        // buffered like other topology-affecting display reconfigurations.
        flags.intersects(
            DisplayReconfigFlags::ADD
                | DisplayReconfigFlags::REMOVE
                | DisplayReconfigFlags::MOVED
                | DisplayReconfigFlags::SET_MAIN
                | DisplayReconfigFlags::SET_MODE
                | DisplayReconfigFlags::ENABLED
                | DisplayReconfigFlags::DISABLED
                | DisplayReconfigFlags::MIRROR
                | DisplayReconfigFlags::UNMIRROR
                | DisplayReconfigFlags::DESKTOP_SHAPE_CHANGED,
        )
    }

    fn begin_display_churn(&mut self, flags: DisplayReconfigFlags) -> u64 {
        let was_active = self.state.display_churn_active;
        self.state.display_churn_active = true;
        self.state.display_churn_flags |= flags;
        self.state.display_churn_epoch = self.state.display_churn_epoch.wrapping_add(1);
        self.state.display_topology_state = None;
        self.state.last_sent_spaces = None;
        if !was_active {
            self.state.pre_churn_visible_window_spaces = self.state.visible_window_spaces.clone();
        }
        if !was_active {
            let _ = display_churn::begin(flags);
            self.reactor_tx.send(reactor::Event::DisplayChurnBegin);
        } else {
            let _ = display_churn::begin(flags);
        }
        self.state.display_churn_epoch
    }

    fn schedule_display_stabilization_check(&self, expected_epoch: u64) {
        self.schedule_display_stabilization(expected_epoch, 0, DISPLAY_CHURN_QUIET_NS);
    }

    fn schedule_display_stabilization_retry(&self, expected_epoch: u64, attempt: u8) {
        self.schedule_display_stabilization(expected_epoch, attempt, DISPLAY_STABILIZE_RETRY_NS);
    }

    fn schedule_display_stabilization(&self, expected_epoch: u64, attempt: u8, delay_ns: i64) {
        if !self.state.timers_enabled {
            return;
        }

        let sender = self.sender.clone();
        queue::main().after_f_s(
            Time::new_after(Time::NOW, delay_ns),
            (sender, expected_epoch, attempt),
            |(sender, expected_epoch, attempt)| {
                sender.send(Event::CheckDisplayStabilization { expected_epoch, attempt })
            },
        );
    }

    fn retry_display_stabilization(&self, expected_epoch: u64, attempt: u8) -> bool {
        if attempt < DISPLAY_STABILIZE_MAX_ATTEMPTS {
            self.schedule_display_stabilization_retry(expected_epoch, attempt + 1);
            return true;
        }
        false
    }

    fn attempt_finish_display_churn(&mut self, expected_epoch: u64, attempt: u8) {
        if expected_epoch != self.state.display_churn_epoch || !self.state.display_churn_active {
            return;
        }

        let Some((screens, converter)) = self.collect_state() else {
            if !self.retry_display_stabilization(expected_epoch, attempt) {
                self.finish_display_churn(expected_epoch, true);
            }
            return;
        };

        if screens.is_empty() {
            if !self.retry_display_stabilization(expected_epoch, attempt) {
                self.finish_display_churn(expected_epoch, true);
            }
            return;
        }

        let fingerprint = DisplayTopologyFingerprint(
            screens
                .iter()
                .map(|d| {
                    (
                        d.display_uuid.clone(),
                        d.frame.origin.x.to_bits(),
                        d.frame.origin.y.to_bits(),
                        d.frame.size.width.to_bits(),
                        d.frame.size.height.to_bits(),
                        d.space.map(|space| space.get()),
                    )
                })
                .collect(),
        );

        let hits = match self.state.display_topology_state.as_mut() {
            Some(existing) if existing.fingerprint == fingerprint => {
                existing.hits = existing.hits.saturating_add(1);
                existing.hits
            }
            _ => {
                self.state.display_topology_state =
                    Some(DisplayTopologyState { fingerprint, hits: 1 });
                self.schedule_display_stabilization_retry(expected_epoch, attempt + 1);
                return;
            }
        };

        if hits >= DISPLAY_STABLE_REQUIRED_HITS {
            if !Self::screen_snapshot_is_valid_for_commit(&screens) {
                self.state.display_topology_state = None;
                if !self.retry_display_stabilization(expected_epoch, attempt) {
                    self.finish_display_churn(expected_epoch, true);
                }
                return;
            }
            if !window_server::windowserver_quiet_for_us(window_server::WINDOWSERVER_QUIET_US) {
                if !self.retry_display_stabilization(expected_epoch, attempt) {
                    self.finish_display_churn(expected_epoch, true);
                }
                return;
            }
            let flags = self.state.display_churn_flags;
            self.synthesize_topology_window_delta(expected_epoch, flags, &screens);
            self.state.pending_screen_parameters = None;
            self.state.pending_spaces = None;
            // Forward the stabilized snapshot directly; its authoritative state
            // acknowledges the reactor's churn gate when it is incorporated.
            self.forward_screen_parameters(screens, converter);
            self.finish_display_churn(expected_epoch, false);
            return;
        }

        if !self.retry_display_stabilization(expected_epoch, attempt) {
            self.finish_display_churn(expected_epoch, true);
        }
    }

    fn finish_display_churn(&mut self, expected_epoch: u64, schedule_refresh: bool) {
        if expected_epoch != self.state.display_churn_epoch || !self.state.display_churn_active {
            return;
        }
        self.state.display_churn_active = false;
        self.state.display_churn_epoch = self.state.display_churn_epoch.wrapping_add(1);
        self.state.display_churn_flags = DisplayReconfigFlags::empty();
        self.state.display_topology_state = None;
        let _ = display_churn::end();

        if self.state.refresh_deferred_until_stable {
            self.state.refresh_deferred_until_stable = false;
        }
        if schedule_refresh {
            self.schedule_screen_refresh_after(0, 0);
        }
    }
}

#[cfg(test)]
mod tests;
