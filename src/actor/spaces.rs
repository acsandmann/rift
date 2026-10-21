//! Authoritative native display/space state for Rift.
//!
//! This actor is the only place that should translate macOS display, space, and
//! session lifecycle signals into the snapshot consumed by the reactor. The
//! reactor owns Rift's virtual workspace model, but it must only do so on top of
//! a stable native-space picture. The rules here are therefore intentionally
//! conservative:
//!
//! - Sleep, display churn, and lock/login transitions retain the last accepted
//!   snapshot until a fresh coherent sample settles.
//! - Only user spaces are allowed to become the reactor's workspace/display
//!   context. Fullscreen and system/login spaces are treated as transient native
//!   state and nulled out before they can rewrite workspace mappings.
//! - When the system finally stabilizes, this actor forwards a single coherent
//!   snapshot with full window membership for the reactor to reconcile once.
//!
//! The core failure mode this prevents is treating unstable lock/wake/login
//! snapshots as authoritative user-space state. That can cause Rift to
//! initialize fresh default workspaces for transient spaces and later remap them
//! onto the real desktop, which looks like "all windows reset to workspace 1".

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use dispatchr::queue;
use dispatchr::time::Time;
use objc2_core_foundation::CGSize;
use objc2_foundation::MainThreadMarker;

use crate::actor;
use crate::actor::{reactor, wm_controller};
use crate::common::collections::{HashMap, HashSet};
use crate::sys::dispatch::DispatchExt;
use crate::sys::screen::{CoordinateConverter, ScreenCache, ScreenInfo, SpaceId};
use crate::sys::skylight::DisplayReconfigFlags;
use crate::sys::window_server;
use crate::sys::window_server::WindowServerId;

const REFRESH_DEFAULT_DELAY_NS: i64 = 100_000_000;
const REFRESH_SPACE_SWITCH_DELAY_NS: i64 = 50_000_000;
const REFRESH_RETRY_DELAY_NS: i64 = 100_000_000;
pub(crate) const REFRESH_MAX_RETRIES: u8 = 10;
const REFRESH_RECOVERY_DELAY_NS: i64 = 1_000_000_000;

// Recovery needs two matching physical/Space samples and a quiet WindowServer.
const DISPLAY_STABLE_REQUIRED_HITS: u8 = 2;

/// Published before invalidation is queued. A reactor must not accept an older
/// snapshot or mutate windows while this revision is ahead of its own.
/// Each actor pair owns its record; tests and independent instances cannot interfere.
#[derive(Debug, Clone, Default)]
pub struct TopologyGuard(Arc<AtomicU64>);

impl TopologyGuard {
    pub fn invalidated_revision(&self) -> u64 { self.0.load(Ordering::Acquire) }

    fn invalidate(&self, revision: u64) { self.0.store(revision, Ordering::Release); }
}

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
    SpaceInventoryChanged,
    ReconcileWindowSpaces,
    SpaceCreated(SpaceId),
    SpaceDestroyed(SpaceId),
    WindowServerAppeared(WindowServerId, SpaceId),
    WindowServerDestroyed(WindowServerId, SpaceId),
    ProcessScreenRefresh {
        revision: u64,
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

/// One observation from the native boundary; settling never rereads half a sample.
#[derive(Clone)]
struct NativeSnapshot {
    screens: Vec<ScreenInfo>,
    converter: CoordinateConverter,
    inventory: HashMap<String, Vec<SpaceId>>,
    quiet: bool,
}

#[derive(Debug, Clone)]
struct DisplayTopologyState {
    screens: Vec<ScreenInfo>,
    inventory: HashMap<String, Vec<SpaceId>>,
    hits: u8,
}

/// Forwarded read-only space/display snapshot consumed by the reactor.
#[derive(Debug, Default, Clone)]
pub struct ForwardedSpaceState {
    pub revision: u64,
    pub authoritative: bool,
    /// False when membership was carried forward from an inconclusive query.
    pub membership_complete: bool,
    pub screens: Vec<ScreenInfo>,
    pub fullscreen_spaces: HashSet<SpaceId>,
    pub active_spaces: HashSet<SpaceId>,
    pub menu_bar_space: Option<SpaceId>,
    pub command_space: Option<SpaceId>,
    pub display_space_ids: HashMap<String, Vec<SpaceId>>,
    pub last_user_space_by_display: HashMap<String, SpaceId>,
    pub space_remaps: Vec<(SpaceId, SpaceId)>,
    pub display_set_changed: bool,
    pub should_force_refresh_layout: bool,
    pub resized_spaces: Vec<(SpaceId, CGSize)>,
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

pub struct AuthorityState {
    pub sleeping: bool,
    pub session_inactive: bool,
    pub display_churn_active: bool,
    pub screens: Vec<ScreenInfo>,
    pub last_sent_spaces: Option<Vec<Option<SpaceId>>>,
    pub quarantine_stats: QuarantineStats,
    screen_cache: Option<ScreenCache>,
    refresh_pending: bool,
    display_churn_flags: DisplayReconfigFlags,
    display_topology_state: Option<DisplayTopologyState>,
    last_user_space_by_display: HashMap<String, SpaceId>,
    // Last trustworthy user-Space catalog per display, retained while absent.
    // Classify while IDs are live; querying a destroyed Space's type is unreliable.
    // Forwarded snapshots carry only the current inventory.
    last_known_display_space_ids: HashMap<String, Vec<SpaceId>>,
    active_display_uuid: Option<String>,
    awaiting_space_switch_confirmation: bool,
    recovering_membership: bool,
    visible_window_spaces: HashMap<WindowServerId, SpaceId>,
    revision: u64,
    last_forwarded: Option<ForwardedSpaceState>,
    timers_enabled: bool,
    topology_guard: TopologyGuard,
    churn_started: Option<Instant>,
}

impl Default for AuthorityState {
    fn default() -> Self {
        Self {
            sleeping: false,
            session_inactive: false,
            display_churn_active: false,
            screens: Vec::new(),
            last_sent_spaces: None,
            quarantine_stats: QuarantineStats::default(),
            screen_cache: None,
            refresh_pending: false,
            display_churn_flags: DisplayReconfigFlags::empty(),
            display_topology_state: None,
            last_user_space_by_display: HashMap::default(),
            last_known_display_space_ids: HashMap::default(),
            active_display_uuid: None,
            awaiting_space_switch_confirmation: false,
            recovering_membership: false,
            visible_window_spaces: HashMap::default(),
            revision: 0,
            last_forwarded: None,
            timers_enabled: true,
            topology_guard: TopologyGuard::default(),
            churn_started: None,
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

    pub fn topology_guard(&self) -> TopologyGuard { self.state.topology_guard.clone() }

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
                self.invalidate_topology();
                self.state.recovering_membership = false;
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

                // Only a fresh stable sample can release the sleep quarantine.
                self.state.recovering_membership = true;
                self.schedule_screen_refresh();
            }
            Event::SessionDidResignActive => {
                if self.state.session_inactive {
                    return;
                }
                self.state.session_inactive = true;
                self.invalidate_topology();
                self.state.recovering_membership = false;
            }
            Event::SessionDidBecomeActive => {
                if !self.state.session_inactive {
                    return;
                }
                self.state.session_inactive = false;
                self.reactor_tx.send(reactor::Event::SessionDidBecomeActive);
                if let Some(screen_cache) = self.state.screen_cache.as_mut() {
                    screen_cache.mark_dirty();
                }

                // The login window can transiently replace every display's current
                // space. Do not replay buffered lock-screen snapshots into Rift's
                // workspace model; always resample after the user session becomes
                // active again.
                self.state.recovering_membership = true;
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
                if let Some(screen_cache) = self.state.screen_cache.as_mut() {
                    screen_cache.mark_dirty();
                }
                self.schedule_screen_refresh();
            }
            Event::DisplayReconfigured { display_id, flags } => {
                self.handle_display_reconfig_event(display_id, flags);
            }
            Event::ReconcileWindowSpaces => {
                // A native move may be the first evidence of display churn. Refresh
                // physical screens too, rather than waiting for its late callback.
                if let Some(cache) = self.state.screen_cache.as_mut() {
                    cache.mark_dirty();
                }
                self.handle_space_inventory_changed();
            }
            Event::SpaceInventoryChanged => {
                self.handle_space_inventory_changed();
            }
            Event::SpaceCreated(_) | Event::SpaceDestroyed(_) => {
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
                            self.handle_event(Event::ReconcileWindowSpaces);
                            return;
                        }
                        self.reactor_tx
                            .send(reactor::Event::WindowServerDestroyed(wsid, sid, kind));
                    }
                }
            }
            Event::ProcessScreenRefresh { revision, attempt } => {
                if revision == self.state.revision {
                    let snapshot = if self.lifecycle_quarantined() {
                        None
                    } else {
                        self.collect_state()
                    };
                    self.process_screen_refresh(attempt, snapshot);
                }
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

        if !self.try_forward_authoritative_snapshot(true, 0) {
            self.schedule_screen_refresh_after(0, 0);
        }
    }

    fn active_display_matches_state(&self, active_display_uuid: Option<&str>) -> bool {
        active_display_uuid.is_some()
            && active_display_uuid == self.state.active_display_uuid.as_deref()
    }

    fn handle_active_space_changed(&mut self) {
        // Revoke queued geometry at the signal, before the native query settles.
        // This is presentation invalidation, not physical display churn.
        self.state.awaiting_space_switch_confirmation = true;
        if self.topology_is_authoritative() {
            self.invalidate_topology();
            self.schedule_screen_refresh_after(REFRESH_SPACE_SWITCH_DELAY_NS, 0);
            return;
        }

        if self.should_buffer_topology_updates() {
            self.schedule_screen_refresh_after(REFRESH_SPACE_SWITCH_DELAY_NS, 0);
            return;
        }

        if !self.try_forward_authoritative_snapshot(false, 0) {
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
        if !self.try_forward_authoritative_snapshot(true, 0) {
            self.schedule_screen_refresh_after(REFRESH_RETRY_DELAY_NS, 0);
        }
    }

    // Sleep and session inactivity end only on wake or unlock; churn clears itself.
    fn lifecycle_quarantined(&self) -> bool { self.state.sleeping || self.state.session_inactive }

    fn should_buffer_topology_updates(&self) -> bool {
        self.lifecycle_quarantined() || self.state.display_churn_active
    }

    fn topology_is_authoritative(&self) -> bool {
        !self.should_buffer_topology_updates()
            && self
                .state
                .last_forwarded
                .as_ref()
                .is_some_and(|snapshot| snapshot.revision == self.state.revision)
    }

    fn should_quarantine_window_space_event(&self) -> bool { !self.topology_is_authoritative() }

    fn collect_state(&mut self) -> Option<NativeSnapshot> {
        let resample_geometry =
            self.state.display_churn_active || !self.topology_is_authoritative();
        let cache = self.state.screen_cache.as_mut()?;
        if resample_geometry {
            cache.mark_dirty();
        }
        let (screens, converter) = cache.refresh()?;
        Some(NativeSnapshot {
            screens,
            converter,
            inventory: crate::sys::screen::managed_display_space_ids(),
            quiet: window_server::windowserver_quiet_for_us(window_server::WINDOWSERVER_QUIET_US),
        })
    }

    fn inventory_matches_screens(
        screens: &[ScreenInfo],
        inventory: &HashMap<String, Vec<SpaceId>>,
    ) -> bool {
        let mut owners = HashSet::default();
        // Inventories are read independently from current spaces. An incomplete
        // query is not evidence that a desktop was destroyed.
        screens.iter().all(|screen| {
            inventory
                .get(&screen.display_uuid)
                .is_some_and(|spaces| screen.space.is_none_or(|space| spaces.contains(&space)))
        }) && inventory.values().flatten().all(|space| owners.insert(*space))
    }

    fn build_forwarded_state(
        &mut self,
        screens: Vec<ScreenInfo>,
        inventory: HashMap<String, Vec<SpaceId>>,
    ) -> ForwardedSpaceState {
        let previous_screens = self.state.screens.clone();
        let fullscreen_spaces: HashSet<SpaceId> = screens
            .iter()
            .filter_map(|screen| screen.space)
            .filter(|space| Self::is_fullscreen_space(*space))
            .collect();
        let mut screens = screens;
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
        let should_force_refresh_layout = topology_changed && !previous_screens.is_empty();

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

        let space_remaps = self.compute_space_remaps(&screens, &inventory);
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

        // Full native inventories distinguish identity loss from switching the
        // current Space. This can be visible before a display callback arrives.
        let should_force_refresh_layout = should_force_refresh_layout
            || self.state.last_forwarded.as_ref().is_some_and(|previous| {
                previous.display_space_ids.iter().any(|(display, spaces)| {
                    spaces.iter().any(|space| {
                        !inventory.get(display).is_some_and(|current| current.contains(space))
                    })
                })
            });
        let (membership, membership_complete) = self.visible_window_spaces_for_screens(&screens);
        self.state.visible_window_spaces = membership;
        self.state.screens = screens.clone();
        self.state.recovering_membership = false;
        let mut forwarded = ForwardedSpaceState {
            revision: self.state.revision,
            authoritative: true,
            membership_complete,
            screens,
            fullscreen_spaces,
            active_spaces: self.state.screens.iter().filter_map(|screen| screen.space).collect(),
            menu_bar_space,
            command_space,
            display_space_ids: inventory,
            last_user_space_by_display: self.state.last_user_space_by_display.clone(),
            space_remaps,
            display_set_changed,
            should_force_refresh_layout,
            resized_spaces,
            active_window_spaces: self.state.visible_window_spaces.clone(),
        };
        if self.state.last_forwarded.as_ref().is_none_or(|previous| {
            previous.screens != forwarded.screens
                || previous.fullscreen_spaces != forwarded.fullscreen_spaces
                || previous.display_space_ids != forwarded.display_space_ids
                || previous.active_window_spaces != forwarded.active_window_spaces
                || previous.membership_complete != forwarded.membership_complete
        }) {
            self.state.revision = self.state.revision.wrapping_add(1);
            forwarded.revision = self.state.revision;
            // Revoke queued app-actor frames before publishing a changed presentation,
            // including ordinary native Space switches without physical churn.
            self.state.topology_guard.invalidate(self.state.revision);
        }
        self.state.last_forwarded = Some(forwarded.clone());
        forwarded
    }

    fn invalidate_topology(&mut self) {
        self.state.revision = self.state.revision.wrapping_add(1);
        self.state.display_topology_state = None;
        self.state.refresh_pending = false;
        self.state.topology_guard.invalidate(self.state.revision);
        self.reactor_tx.send(reactor::Event::TopologyInvalidated(self.state.revision));
    }

    fn fullscreen_snapshot_crosses_display_ownership(&self, screens: &[ScreenInfo]) -> bool {
        let entering_fullscreen = screens.iter().any(|next| {
            next.space.is_some_and(Self::is_fullscreen_space)
                && self.state.screens.iter().any(|previous| {
                    previous.display_uuid == next.display_uuid
                        && previous.space.is_some_and(|space| !Self::is_fullscreen_space(space))
                })
        });
        entering_fullscreen
            && screens.iter().any(|next| {
                next.space.is_some_and(|space| {
                    !Self::is_fullscreen_space(space)
                        && self.state.screens.iter().any(|previous| {
                            previous.display_uuid != next.display_uuid
                                && previous.space == Some(space)
                        })
                })
            })
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

    fn space_replacements_are_ambiguous(&self, inventory: &HashMap<String, Vec<SpaceId>>) -> bool {
        let history = &self.state.last_known_display_space_ids;
        inventory.iter().any(|(display, current)| {
            let Some(previous) = history.get(display) else {
                return false;
            };
            let removed = previous
                .iter()
                .filter(|space| !inventory.values().any(|spaces| spaces.contains(space)))
                .take(2)
                .count();
            let added = current
                .iter()
                .filter(|space| {
                    !history.values().any(|spaces| spaces.contains(space))
                        && Self::is_user_space(**space)
                })
                .take(2)
                .count();
            removed > 0 && added > 0 && (removed != 1 || added != 1)
        })
    }

    fn compute_space_remaps(
        &mut self,
        screens: &[ScreenInfo],
        inventory: &HashMap<String, Vec<SpaceId>>,
    ) -> Vec<(SpaceId, SpaceId)> {
        let history = &self.state.last_known_display_space_ids;
        let live: HashSet<_> = inventory.values().flatten().copied().collect();
        let known: HashSet<_> = history.values().flatten().copied().collect();
        let mut remaps = Vec::new();
        let mut accepted = Vec::new();
        for screen in screens {
            let Some(display) = screen.display_uuid_opt() else {
                continue;
            };
            let Some(current) = inventory.get(display) else {
                continue;
            };
            // A partial return can temporarily show another display's desktop.
            // Neither learn that ownership nor use it as replacement evidence.
            if current.iter().any(|space| {
                history.iter().any(|(owner, spaces)| owner != display && spaces.contains(space))
            }) {
                continue;
            }
            if let Some(previous) = history.get(display) {
                if previous.iter().any(|space| {
                    inventory
                        .iter()
                        .any(|(owner, spaces)| owner != display && spaces.contains(space))
                }) {
                    continue;
                }
                let mut removed = previous.iter().copied().filter(|space| !live.contains(space));
                let mut added = current
                    .iter()
                    .copied()
                    .filter(|space| !known.contains(space) && Self::is_user_space(*space));
                if let (Some(old), None, Some(new), None) =
                    (removed.next(), removed.next(), added.next(), added.next())
                {
                    // A unique same-display inventory delta also identifies an inactive
                    // replacement. Multiple vanished/new IDs have no trustworthy pairing.
                    remaps.push((old, new));
                }
            }
            accepted.push(display.to_owned());
            if let Some(space) = screen.space {
                self.state.last_user_space_by_display.insert(display.to_owned(), space);
            }
        }
        for display in accepted {
            self.state.last_known_display_space_ids.insert(
                display.clone(),
                inventory[&display]
                    .iter()
                    .copied()
                    .filter(|space| known.contains(space) || Self::is_user_space(*space))
                    .collect(),
            );
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
            !Self::is_fullscreen_space(space)
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
    ) -> (HashMap<WindowServerId, SpaceId>, bool) {
        let mut active_spaces = Vec::new();
        let mut active_space_set = HashSet::default();
        for space in screens.iter().filter_map(|screen| screen.space) {
            if active_space_set.insert(space) {
                active_spaces.push(space);
            }
        }

        if active_spaces.is_empty() {
            return (HashMap::default(), true);
        }

        // A global visible-window union is not space-aware and can lag one display
        // behind another. Query every active native space independently, including
        // in tests where the per-space query is overridden.
        let mut visible = HashMap::default();
        let mut complete = true;
        for &space in &active_spaces {
            let Some(ids) =
                window_server::try_space_window_list_for_connection(&[space.get()], 0, false)
            else {
                complete = false;
                visible.extend(
                    self.state
                        .visible_window_spaces
                        .iter()
                        .filter_map(|(&id, &known)| (known == space).then_some((id, known))),
                );
                continue;
            };
            for wsid in ids.into_iter().map(WindowServerId::new) {
                let conflicting_space = visible
                    .get(&wsid)
                    .filter(|&&previous| previous != space)
                    .and_then(|_| window_server::window_space(wsid));
                Self::record_visible_window_space(
                    &mut visible,
                    &self.state.visible_window_spaces,
                    &active_space_set,
                    wsid,
                    space,
                    conflicting_space,
                );
            }
        }

        // The first coherent snapshot after wake/unlock can race WindowServer and
        // temporarily contain no windows. Carry forward accepted membership
        // on that first observation. Outside recovery, an
        // empty result is authoritative (and is required to reconcile windows
        // whose destroy notifications were quarantined during display churn).
        if visible.is_empty() && self.state.recovering_membership {
            (
                self.state
                    .visible_window_spaces
                    .iter()
                    .filter_map(|(&wsid, &space)| {
                        active_space_set.contains(&space).then_some((wsid, space))
                    })
                    .collect(),
                false,
            )
        } else {
            (visible, complete)
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

                // Conflicting per-space results are transitional and therefore
                // ambiguous. Preserve a still-active accepted assignment until one
                // per-space query becomes unique. `window_space(wsid)` can return
                // several user spaces in an unstable order during display churn, so
                // it is only a fallback when there is no accepted active assignment.
                let resolved = previous_visible
                    .get(&wsid)
                    .copied()
                    .filter(|space| active_spaces.contains(space))
                    .or_else(|| authoritative_space.filter(|space| active_spaces.contains(space)))
                    .unwrap_or(*entry.get());

                entry.insert(resolved);
            }
        }
    }

    fn screen_snapshot_is_valid_for_commit(screens: &[ScreenInfo]) -> bool {
        let mut seen_user_spaces: HashSet<SpaceId> = HashSet::default();
        let mut seen_displays = HashSet::default();
        !screens.is_empty()
            && screens.iter().all(|screen| {
                let frame = screen.frame;
                screen.space.is_some()
                    && screen.display_uuid_opt().is_some_and(|uuid| seen_displays.insert(uuid))
                    && [
                        frame.origin.x,
                        frame.origin.y,
                        frame.size.width,
                        frame.size.height,
                        screen.backing_scale,
                    ]
                    .iter()
                    .all(|value| value.is_finite())
                    && frame.size.width > 0.0
                    && frame.size.height > 0.0
                    && screen.backing_scale > 0.0
                    && match screen.space {
                        Some(space) if !Self::is_fullscreen_space(space) => {
                            seen_user_spaces.insert(space)
                        }
                        _ => true,
                    }
            })
    }

    fn process_screen_refresh(&mut self, attempt: u8, snapshot: Option<NativeSnapshot>) {
        self.state.refresh_pending = false;
        if self.lifecycle_quarantined() {
            return; // Wake/unlock schedules a fresh sample.
        }
        if self.reconcile_snapshot(true, attempt, snapshot) {
            return;
        }
        if attempt < REFRESH_MAX_RETRIES {
            self.schedule_screen_refresh_after(REFRESH_RETRY_DELAY_NS, attempt + 1);
        } else if !self.topology_is_authoritative() {
            tracing::warn!(
                revision = self.state.revision,
                "Display snapshot still unavailable; retaining last layout and retrying in one second"
            );
            self.schedule_screen_refresh_after(REFRESH_RECOVERY_DELAY_NS, 0);
        }
    }

    fn try_forward_authoritative_snapshot(&mut self, force: bool, attempt: u8) -> bool {
        if self.state.refresh_pending || self.lifecycle_quarantined() {
            return false;
        }
        let snapshot = self.collect_state();
        self.reconcile_snapshot(force, attempt, snapshot)
    }

    fn reconcile_snapshot(
        &mut self,
        force: bool,
        attempt: u8,
        snapshot: Option<NativeSnapshot>,
    ) -> bool {
        let Some(NativeSnapshot {
            screens,
            converter,
            mut inventory,
            quiet,
        }) = snapshot
        else {
            self.reject_native_snapshot();
            return false;
        };
        // Native Space enumeration order carries no identity or geometry authority.
        for spaces in inventory.values_mut() {
            spaces.sort_unstable();
        }
        if !Self::screen_snapshot_is_valid_for_commit(&screens)
            || !Self::inventory_matches_screens(&screens, &inventory)
            || self.fullscreen_snapshot_crosses_display_ownership(&screens)
            || self.space_replacements_are_ambiguous(&inventory)
        {
            self.reject_native_snapshot();
            return false;
        }
        // Detect churn even when native membership gets here before the display callback.
        let physical_topology_changed = self.state.screens.len() != screens.len()
            || screens.iter().any(|screen| {
                !self.state.screens.iter().any(|old| {
                    old.display_uuid == screen.display_uuid
                        && old.frame == screen.frame
                        && old.backing_scale == screen.backing_scale
                        && old.id == screen.id
                })
            });
        let identity_lost = self.state.last_forwarded.as_ref().is_some_and(|previous| {
            previous
                .display_space_ids
                .values()
                .flatten()
                .any(|old| !inventory.values().any(|spaces| spaces.contains(old)))
        });
        if !self.state.screens.is_empty()
            && (physical_topology_changed || identity_lost)
            && !self.state.display_churn_active
        {
            self.begin_display_churn(DisplayReconfigFlags::empty());
        }
        let recovering = self.state.display_churn_active || self.state.recovering_membership;
        if recovering && !self.observe_stable_topology(&screens, &inventory, quiet) {
            return false;
        }
        let spaces: Vec<_> = screens.iter().map(|screen| screen.space).collect();
        if (!force
            || (self.state.awaiting_space_switch_confirmation && attempt < REFRESH_MAX_RETRIES))
            && self.state.last_sent_spaces.as_ref() == Some(&spaces)
        {
            return false;
        }
        let forwarded = self.build_forwarded_state(screens, inventory);
        self.state.last_sent_spaces =
            Some(forwarded.screens.iter().map(|screen| screen.space).collect());
        self.state.awaiting_space_switch_confirmation = false;
        let membership_complete = forwarded.membership_complete;
        self.wm_tx.send(wm_controller::WmEvent::SpaceStateUpdated(forwarded, converter));
        if self.state.display_churn_active {
            tracing::info!(revision = self.state.revision,
                elapsed_ms = self.state.churn_started.take().map(|at| at.elapsed().as_millis()),
                flags = ?self.state.display_churn_flags,
                appeared_dropped = self.state.quarantine_stats.appeared_dropped,
                destroyed_dropped = self.state.quarantine_stats.destroyed_dropped,
                "Display reconciliation finished");
            self.state.display_churn_active = false;
            self.state.display_churn_flags = DisplayReconfigFlags::empty();
        }
        self.state.display_topology_state = None;
        if !membership_complete {
            let (delay, next) = if attempt < REFRESH_MAX_RETRIES {
                (REFRESH_RETRY_DELAY_NS, attempt + 1)
            } else {
                (REFRESH_RECOVERY_DELAY_NS, 0)
            };
            self.schedule_screen_refresh_after(delay, next);
        }
        true
    }

    fn reject_native_snapshot(&mut self) {
        // Native queries can fail before the physical-display callback arrives.
        // Retain the accepted model, but revoke writes until a complete sample settles.
        // Repeated failures share one revision and the existing bounded retry batches.
        if !self.state.display_churn_active {
            self.begin_display_churn(DisplayReconfigFlags::empty());
        } else {
            self.state.display_topology_state = None;
        }
    }

    fn schedule_screen_refresh(&mut self) {
        self.schedule_screen_refresh_after(REFRESH_DEFAULT_DELAY_NS, 0);
    }

    fn schedule_screen_refresh_after(&mut self, delay_ns: i64, attempt: u8) {
        if self.state.refresh_pending {
            return;
        }
        self.state.refresh_pending = true;
        if !self.state.timers_enabled {
            return;
        }
        let sender = self.sender.clone();
        let revision = self.state.revision;
        queue::main().after_f_s(
            Time::new_after(Time::NOW, delay_ns),
            (sender, revision, attempt),
            |(sender, revision, attempt)| {
                sender.send(Event::ProcessScreenRefresh { revision, attempt })
            },
        );
    }

    fn handle_display_reconfig_event(&mut self, _display_id: u32, flags: DisplayReconfigFlags) {
        if !Self::should_begin_display_churn(flags) {
            return;
        }
        self.begin_display_churn(flags);
        if let Some(screen_cache) = self.state.screen_cache.as_mut() {
            screen_cache.mark_dirty();
        }
        self.schedule_screen_refresh();
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

    fn begin_display_churn(&mut self, flags: DisplayReconfigFlags) {
        let was_active = self.state.display_churn_active;
        self.state.display_churn_active = true;
        self.state.display_churn_flags |= flags;
        self.state.display_topology_state = None;
        self.state.last_sent_spaces = None;
        if !was_active {
            self.state.churn_started = Some(Instant::now());
            tracing::info!(
                revision = self.state.revision,
                ?flags,
                "Display reconciliation started"
            );
        }
        self.invalidate_topology();
    }

    fn observe_stable_topology(
        &mut self,
        screens: &[ScreenInfo],
        inventory: &HashMap<String, Vec<SpaceId>>,
        quiet: bool,
    ) -> bool {
        // Compare the full sample, including inactive native desktops. Enumeration
        // order alone is irrelevant; physical identity, scale and geometry are not.
        let mut screens = screens.to_vec();
        screens.sort_by(|a, b| a.display_uuid.cmp(&b.display_uuid));
        let hits = match self.state.display_topology_state.as_mut() {
            Some(existing) if existing.screens == screens && existing.inventory == *inventory => {
                existing.hits = existing.hits.saturating_add(1);
                existing.hits
            }
            _ => {
                self.state.display_topology_state = Some(DisplayTopologyState {
                    screens,
                    inventory: inventory.clone(),
                    hits: 1,
                });
                1
            }
        };
        hits >= DISPLAY_STABLE_REQUIRED_HITS && quiet
    }
}

#[cfg(test)]
mod tests;
