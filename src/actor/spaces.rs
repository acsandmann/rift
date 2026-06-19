use dispatchr::queue;
use dispatchr::time::Time;
use objc2_core_foundation::CGSize;

use crate::actor;
use crate::actor::{reactor, wm_controller};
use crate::common::collections::{HashMap, HashSet};
use crate::sys::dispatch::DispatchExt;
#[cfg(not(test))]
use crate::sys::geometry::CGRectExt;
use crate::sys::screen::{CoordinateConverter, ScreenCache, ScreenInfo, SpaceId};
#[cfg(not(test))]
use crate::sys::screen::managed_display_space_ids;
use crate::sys::skylight::DisplayReconfigFlags;
use crate::sys::window_server::WindowServerId;
#[cfg(not(test))]
use crate::sys::window_server::WindowServerInfo;
use crate::sys::{display_churn, window_server};
use objc2_foundation::MainThreadMarker;

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
    pub command_space: Option<SpaceId>,
    pub display_space_ids: HashMap<String, Vec<SpaceId>>,
    pub last_user_space_by_display: HashMap<String, SpaceId>,
    pub space_remaps: Vec<(SpaceId, SpaceId)>,
    pub display_set_changed: bool,
    pub topology_changed: bool,
    pub allow_space_remap: bool,
    pub should_force_refresh_layout: bool,
    pub resized_spaces: Vec<(SpaceId, CGSize)>,
    pub topology_window_delta: Option<TopologyWindowDelta>,
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
    awaiting_space_switch_confirmation: bool,
    refresh_deferred_until_stable: bool,
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
            awaiting_space_switch_confirmation: false,
            refresh_deferred_until_stable: false,
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
    pub fn new(
        reactor_tx: reactor::Sender,
        wm_tx: wm_controller::Sender,
    ) -> (Self, Sender) {
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
                if let Some(screen_cache) = self.state.screen_cache.as_mut() {
                    screen_cache.mark_sleeping(true);
                }
            }
            Event::SystemDidWake => {
                self.state.sleeping = false;
                if let Some(screen_cache) = self.state.screen_cache.as_mut() {
                    screen_cache.mark_sleeping(false);
                }
                if self.state.display_churn_active {
                    let expected_epoch = self.state.display_churn_epoch;
                    self.schedule_display_stabilization_check(expected_epoch);
                }
                // Wake is inherently unstable; prefer the delayed refresh path.
                self.schedule_screen_refresh();
                self.flush_pending_if_stable();
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
                self.reactor_tx.send(reactor::Event::DisplayChurnEnd);
                self.flush_pending_if_stable();
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
                if self.classify_space(space).is_some() {
                    self.reactor_tx.send(reactor::Event::SpaceCreated(space));
                }
                self.handle_space_inventory_changed();
            }
            Event::SpaceDestroyed(space) => {
                if self.classify_space(space).is_some() {
                    self.reactor_tx.send(reactor::Event::SpaceDestroyed(space));
                }
                self.handle_space_inventory_changed();
            }
            Event::WindowServerAppeared(wsid, sid) => {
                self.state.visible_window_spaces.insert(wsid, sid);
                if self.should_quarantine_window_space_event() {
                    self.state.quarantine_stats.appeared_dropped += 1;
                } else if let Some(kind) = self.classify_space(sid) {
                    self.reactor_tx
                        .send(reactor::Event::WindowServerAppeared(wsid, sid, kind));
                }
            }
            Event::WindowServerDestroyed(wsid, sid) => {
                self.state.visible_window_spaces.remove(&wsid);
                if self.should_quarantine_window_space_event() {
                    self.state.quarantine_stats.destroyed_dropped += 1;
                } else if let Some(kind) = self.classify_space(sid) {
                    if matches!(kind, reactor::SpaceEventKind::User)
                        && let Some(current_space) = window_server::window_space(wsid)
                        && current_space != sid
                    {
                        return;
                    }
                    self.reactor_tx
                        .send(reactor::Event::WindowServerDestroyed(wsid, sid, kind));
                }
            }
            Event::ProcessScreenRefresh { attempt } => {
                self.process_screen_refresh(attempt, true);
            }
            Event::CheckDisplayStabilization {
                expected_epoch,
                attempt,
            } => {
                self.attempt_finish_display_churn(expected_epoch, attempt);
            }
        }
    }

    fn handle_active_display_changed(&mut self) {
        if self.should_buffer_topology_updates() {
            self.schedule_screen_refresh_after(0, 0);
            return;
        }

        if !self.try_forward_authoritative_snapshot(true, true) {
            self.schedule_screen_refresh_after(0, 0);
        }
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
        self.state.sleeping || self.state.display_churn_active
    }

    fn should_quarantine_window_space_event(&self) -> bool {
        self.state.sleeping || self.state.display_churn_active
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
        self.wm_tx
            .send(wm_controller::WmEvent::SpaceStateUpdated(
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
        self.wm_tx
            .send(wm_controller::WmEvent::SpaceStateUpdated(
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

        let previous_sizes: HashMap<_, _> = previous_screens
            .iter()
            .map(|screen| (screen.id, screen.frame.size))
            .collect();
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
        let command_space = self.resolve_command_space(&screens);
        #[cfg(test)]
        {
            let mut display_space_ids: HashMap<String, Vec<SpaceId>> = HashMap::default();
            for screen in &screens {
                if let Some(space) = screen.space {
                    display_space_ids
                        .entry(screen.display_uuid.clone())
                        .or_default()
                        .push(space);
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

        ForwardedSpaceState {
            screens,
            fullscreen_spaces,
            has_seen_display_set: self.state.has_seen_display_set,
            active_spaces: self.state.screens.iter().filter_map(|screen| screen.space).collect(),
            command_space,
            display_space_ids: self.state.display_space_ids.clone(),
            last_user_space_by_display: self.state.last_user_space_by_display.clone(),
            space_remaps,
            display_set_changed,
            topology_changed,
            allow_space_remap,
            should_force_refresh_layout,
            resized_spaces,
            topology_window_delta: self.state.pending_topology_window_delta.take(),
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
            let should_rewrite = previous_space_owner
                .get(&new_space)
                .is_some_and(|owner| *owner != display_uuid);
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

    fn compute_space_remaps(
        &mut self,
        screens: &[ScreenInfo],
        allow_space_remap: bool,
    ) -> Vec<(SpaceId, SpaceId)> {
        let mut remaps = Vec::new();
        let mut seen_displays: HashSet<String> = HashSet::default();

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

            if allow_space_remap
                && let Some(previous_space) = self
                    .state
                    .last_user_space_by_display
                    .get(display_uuid)
                    .copied()
                && previous_space != space
            {
                remaps.push((previous_space, space));
            }

            self.state
                .last_user_space_by_display
                .insert(display_uuid.to_string(), space);
        }

        remaps
    }

    fn resolve_command_space(&self, screens: &[ScreenInfo]) -> Option<SpaceId> {
        #[cfg(test)]
        {
            screens
                .iter()
                .find_map(|screen| screen.space)
                .or_else(|| self.state.screens.iter().find_map(|screen| screen.space))
        }
        #[cfg(not(test))]
        {
            if let Ok(point) = window_server::current_cursor_location()
                && let Some(space) = screens
                    .iter()
                    .find(|screen| screen.frame.contains(point))
                    .and_then(|screen| screen.space)
            {
                return Some(space);
            }

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

    fn classify_space(&self, space: SpaceId) -> Option<reactor::SpaceEventKind> {
        if Self::is_fullscreen_space(space) {
            Some(reactor::SpaceEventKind::Fullscreen)
        } else {
            #[cfg(test)]
            {
                Some(reactor::SpaceEventKind::User)
            }
            #[cfg(not(test))]
            {
                if window_server::space_is_user(space.get()) {
                    Some(reactor::SpaceEventKind::User)
                } else {
                    None
                }
            }
        }
    }

    fn visible_window_spaces_for_screens(
        &self,
        screens: &[ScreenInfo],
    ) -> HashMap<WindowServerId, SpaceId> {
        #[cfg(test)]
        {
            self.state
                .visible_window_spaces
                .iter()
                .filter_map(|(&wsid, &space)| {
                    screens.iter().any(|screen| screen.space == Some(space)).then_some((wsid, space))
                })
                .collect()
        }
        #[cfg(not(test))]
        {
            let active_spaces: HashSet<SpaceId> = screens.iter().filter_map(|screen| screen.space).collect();
            window_server::get_visible_windows_with_layer(None)
                .into_iter()
                .filter_map(|info| {
                    let space = self.resolve_space_for_window_info(screens, &info)?;
                    active_spaces.contains(&space).then_some((info.id, space))
                })
                .collect()
        }
    }

    #[cfg(not(test))]
    fn resolve_space_for_window_info(
        &self,
        screens: &[ScreenInfo],
        info: &WindowServerInfo,
    ) -> Option<SpaceId> {
        if let Some(space) = window_server::window_space(info.id)
            && screens.iter().any(|screen| screen.space == Some(space))
        {
            return Some(space);
        }

        let frame = info.frame;
        let center = objc2_core_foundation::CGPoint::new(
            frame.origin.x + frame.size.width / 2.0,
            frame.origin.y + frame.size.height / 2.0,
        );
        screens
            .iter()
            .find(|screen| screen.frame.contains(center))
            .and_then(|screen| screen.space)
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

        if let Some(pending) = self.state.pending_screen_parameters.take() {
            self.forward_screen_parameters(pending.screens, pending.converter);
        }

        if let Some(pending) = self.state.pending_spaces.take() {
            self.forward_space_snapshot(pending);
        }
    }

    fn screen_snapshot_is_valid_for_commit(screens: &[ScreenInfo]) -> bool {
        let mut seen_user_spaces: HashSet<SpaceId> = HashSet::default();
        screens.iter().all(|screen| match screen.space {
            Some(space) if !Self::is_fullscreen_space(space) => seen_user_spaces.insert(space),
            _ => true,
        })
    }

    fn process_screen_refresh(&mut self, attempt: u8, allow_retry: bool) {
        if self.state.display_churn_active {
            self.state.refresh_deferred_until_stable = true;
            self.state.refresh_pending = false;
            return;
        }

        let Some((screens, converter)) = self.collect_state() else {
            if allow_retry && attempt < REFRESH_MAX_RETRIES {
                self.schedule_screen_refresh_after(REFRESH_RETRY_DELAY_NS, attempt + 1);
                return;
            }
            self.state.refresh_pending = false;
            return;
        };

        if screens.is_empty() {
            if allow_retry && attempt < REFRESH_MAX_RETRIES {
                self.schedule_screen_refresh_after(REFRESH_RETRY_DELAY_NS, attempt + 1);
                return;
            }
            self.state.refresh_pending = false;
            return;
        }

        if screens.iter().any(|screen| screen.space.is_none())
            && allow_retry
            && attempt < REFRESH_MAX_RETRIES
        {
            self.schedule_screen_refresh_after(REFRESH_RETRY_DELAY_NS, attempt + 1);
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
        if screens.is_empty() {
            return false;
        }
        if require_complete_spaces && screens.iter().any(|screen| screen.space.is_none()) {
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
        let expected_epoch = self.begin_display_churn(flags);
        if let Some(screen_cache) = self.state.screen_cache.as_mut() {
            screen_cache.mark_dirty();
        }
        self.schedule_display_stabilization_check(expected_epoch);
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
                sender.send(Event::CheckDisplayStabilization {
                    expected_epoch,
                    attempt,
                })
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
                self.state.display_topology_state = Some(DisplayTopologyState {
                    fingerprint,
                    hits: 1,
                });
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
        self.reactor_tx.send(reactor::Event::DisplayChurnEnd);

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
