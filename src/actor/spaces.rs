//! Authoritative native display/space state for Rift.
//!
//! This actor is the only place that translates macOS display, space, and session
//! lifecycle signals into the snapshot consumed by the reactor. The reactor owns
//! Rift's virtual workspace model on top of that snapshot, so the rules here are
//! conservative:
//!
//! - Nothing is forwarded during sleep, lock/login, or display churn. Churn begins
//!   on a topology-changing reconfiguration (or a sample showing one) and ends
//!   once two identical samples are seen while the WindowServer is quiet. Sleep
//!   and lock end on wake or unlock, which always take a fresh sample.
//! - Only user spaces become the reactor's workspace/display context. Fullscreen
//!   and system/login spaces are nulled out before they can rewrite mappings.
//! - A stabilized churn is forwarded as one snapshot plus the window enter/leave
//!   delta between the last authoritative membership and the new one. The
//!   pre-churn membership is kept until a complete sample can be diffed against
//!   it, so a failed per-space query never makes windows look removed.
//! - Space identity is reported, not interpreted: each snapshot carries every
//!   display's full native inventory, from which the reactor tells a recreated
//!   desktop from a switch to another one.
//!
//! The failure this prevents is treating an unstable lock/wake/login snapshot as
//! user-space state: Rift would initialize default workspaces for the transient
//! spaces and later remap them onto the real desktop ("all windows reset to
//! workspace 1").

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

const REFRESH_DELAY_NS: i64 = 100_000_000;
const REFRESH_SPACE_SWITCH_DELAY_NS: i64 = 50_000_000;
/// Quick retries before backing off, doubling the delay up to `REFRESH_MAX_BACKOFF`.
const REFRESH_MAX_RETRIES: u8 = 10;
const REFRESH_MAX_BACKOFF: u8 = 5;

// Two identical topology samples plus a quiet WindowServer end display churn.
const DISPLAY_STABILIZE_DELAY_NS: i64 = 100_000_000;
const DISPLAY_STABILIZE_MAX_ATTEMPTS: u8 = 10;
const DISPLAY_STABLE_REQUIRED_HITS: u8 = 2;
// Writes made during a display change can be lost or overtaken by the window server's own
// moves, such as putting windows back where they were the last time these displays were
// attached, for a few seconds after it. Once displays have been quiet for each of these, the
// reactor checks windows really are placed.
const DISPLAY_SETTLED_NS: [i64; 3] = [1_000_000_000, 3_000_000_000, 8_000_000_000];

/// Reconfiguration flags that begin display churn. Native space switches can emit
/// callbacks on a single display without any of these; main-display promotion and
/// desktop shape changes reshuffle active spaces, so they count.
const CHURN_FLAGS: DisplayReconfigFlags = DisplayReconfigFlags::ADD
    .union(DisplayReconfigFlags::REMOVE)
    .union(DisplayReconfigFlags::MOVED)
    .union(DisplayReconfigFlags::SET_MAIN)
    .union(DisplayReconfigFlags::SET_MODE)
    .union(DisplayReconfigFlags::ENABLED)
    .union(DisplayReconfigFlags::DISABLED)
    .union(DisplayReconfigFlags::MIRROR)
    .union(DisplayReconfigFlags::UNMIRROR)
    .union(DisplayReconfigFlags::DESKTOP_SHAPE_CHANGED);

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
        attempt: u8,
    },
    CheckDisplayStabilization {
        expected_epoch: u64,
        attempt: u8,
    },
    CheckDisplaysSettled {
        expected_epoch: u64,
    },
}

pub type Sender = actor::Sender<Event>;
type Receiver = actor::Receiver<Event>;

/// Forwarded read-only space/display snapshot consumed by the reactor.
///
/// `screens` holds only user spaces: a screen showing a fullscreen or system space
/// has `space: None`. The spaces on screens are the reactor's active native spaces.
#[derive(Debug, Default, Clone)]
pub struct ForwardedSpaceState {
    pub revision: u64,
    pub authoritative: bool,
    /// False when membership was carried forward from an inconclusive query.
    pub membership_complete: bool,
    pub screens: Vec<ScreenInfo>,
    pub fullscreen_spaces: HashSet<SpaceId>,
    pub menu_bar_space: Option<SpaceId>,
    pub command_space: Option<SpaceId>,
    /// Every native space of every display, current or not.
    pub display_space_ids: HashMap<String, Vec<SpaceId>>,
    pub display_set_changed: bool,
    /// Display set, order, geometry, or native space inventory changed.
    pub should_force_refresh_layout: bool,
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

    pub fn is_space_on_screen(&self, space: SpaceId) -> bool {
        self.screen_by_space(space).is_some()
    }

    /// Whether displays, their spaces' identities, or window membership changed in a
    /// way that needs reconciliation, as opposed to a plain space switch.
    pub fn topology_changed(&self) -> bool {
        self.display_set_changed
            || self.should_force_refresh_layout
            || !self.resized_spaces.is_empty()
            || self.topology_window_delta.is_some()
    }
}

#[derive(Debug, Clone, Default)]
pub struct TopologyWindowDelta {
    pub flags: DisplayReconfigFlags,
    pub appeared: Vec<(WindowServerId, SpaceId)>,
    pub disappeared: Vec<(WindowServerId, SpaceId)>,
}

type TopologyFingerprint = Vec<(String, [u64; 4], Option<SpaceId>)>;

#[derive(Default)]
pub struct AuthorityState {
    pub sleeping: bool,
    pub session_inactive: bool,
    pub display_churn_active: bool,
    pub screens: Vec<ScreenInfo>,
    pub last_sent_spaces: Option<Vec<Option<SpaceId>>>,
    /// Whether any non-empty display set was forwarded, so later ones are changes.
    has_seen_display_set: bool,
    screen_cache: Option<ScreenCache>,
    last_converter: CoordinateConverter,
    /// The attempt of the scheduled refresh, if one is scheduled.
    refresh_pending: Option<u8>,
    display_churn_epoch: u64,
    /// The last sampled topology and how many consecutive times it was seen.
    display_topology_state: Option<(TopologyFingerprint, u8)>,
    active_display_uuid: Option<String>,
    awaiting_space_switch_confirmation: bool,
    /// Set by wake/unlock until the first snapshot after it is forwarded.
    recovering_membership: bool,
    visible_window_spaces: HashMap<WindowServerId, SpaceId>,
    /// The membership accepted before display churn began and the churn's flags,
    /// kept until a complete membership sample is diffed against them.
    churn_baseline: Option<(HashMap<WindowServerId, SpaceId>, DisplayReconfigFlags)>,
    revision: u64,
    last_forwarded: Option<ForwardedSpaceState>,
    timers_disabled: bool,
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
        let state = AuthorityState {
            screen_cache: Some(ScreenCache::new(MainThreadMarker::new().unwrap())),
            ..AuthorityState::default()
        };
        Self::new_with_state(reactor_tx, wm_tx, state)
    }

    fn new_with_state(
        reactor_tx: reactor::Sender,
        wm_tx: wm_controller::Sender,
        state: AuthorityState,
    ) -> (Self, Sender) {
        let (sender, receiver) = actor::channel();
        let actor = Self {
            sender: sender.clone(),
            receiver,
            reactor_tx,
            wm_tx,
            state,
        };
        (actor, sender)
    }

    #[cfg(test)]
    pub fn new_for_tests(
        reactor_tx: reactor::Sender,
        wm_tx: wm_controller::Sender,
    ) -> (Self, Sender) {
        let state = AuthorityState {
            timers_disabled: true,
            ..AuthorityState::default()
        };
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
                self.enter_lifecycle_quarantine();
                if let Some(cache) = self.state.screen_cache.as_mut() {
                    cache.mark_sleeping(true);
                }
            }
            Event::SystemDidWake => {
                self.state.sleeping = false;
                self.reactor_tx.send(reactor::Event::SystemWoke);
                if let Some(cache) = self.state.screen_cache.as_mut() {
                    cache.mark_sleeping(false);
                }
                self.leave_lifecycle_quarantine();
            }
            Event::SessionDidResignActive => {
                if !self.state.session_inactive {
                    self.state.session_inactive = true;
                    self.enter_lifecycle_quarantine();
                }
            }
            Event::SessionDidBecomeActive => {
                if self.state.session_inactive {
                    self.state.session_inactive = false;
                    self.reactor_tx.send(reactor::Event::SessionDidBecomeActive);
                    self.leave_lifecycle_quarantine();
                }
            }
            Event::ActiveDisplayChanged => {
                self.handle_active_display_changed_for(
                    crate::sys::screen::active_menu_bar_display_uuid().as_deref(),
                );
            }
            Event::ActiveSpaceChanged => {
                self.state.awaiting_space_switch_confirmation = true;
                if !self.try_forward_authoritative_snapshot(false) {
                    self.schedule_screen_refresh_after(REFRESH_SPACE_SWITCH_DELAY_NS, 0);
                }
            }
            Event::ScreenRefreshRequested => {
                // Preference/system-driven changes can transiently report stale geometry.
                self.mark_screens_dirty();
                self.schedule_screen_refresh_after(REFRESH_DELAY_NS, 0);
            }
            Event::DisplayReconfigured { flags, .. } => {
                if flags.intersects(CHURN_FLAGS) {
                    let epoch = self.begin_display_churn(flags);
                    self.mark_screens_dirty();
                    self.schedule_display_stabilization(epoch, 0);
                }
            }
            Event::ReconcileWindowSpaces => {
                // A native move may be the first evidence of display churn. Refresh
                // physical screens too, rather than waiting for its late callback.
                self.mark_screens_dirty();
                self.handle_space_inventory_changed();
            }
            Event::SpaceInventoryChanged => self.handle_space_inventory_changed(),
            Event::SpaceCreated(space) | Event::SpaceDestroyed(space) => {
                if !self.topology_unstable() && classify_space(space).is_some() {
                    self.reactor_tx.send(if matches!(event, Event::SpaceCreated(_)) {
                        reactor::Event::SpaceCreated(space)
                    } else {
                        reactor::Event::SpaceDestroyed(space)
                    });
                }
                self.handle_space_inventory_changed();
            }
            Event::WindowServerAppeared(wsid, sid) => {
                if !self.topology_is_authoritative() {
                    return;
                }
                self.state.visible_window_spaces.insert(wsid, sid);
                if let Some(kind) = classify_space(sid) {
                    self.reactor_tx.send(reactor::Event::WindowServerAppeared(wsid, sid, kind));
                }
            }
            Event::WindowServerDestroyed(wsid, sid) => {
                if !self.topology_is_authoritative() {
                    return;
                }
                self.state.visible_window_spaces.remove(&wsid);
                let current_space = window_server::window_space(wsid);
                let Some(kind) = classify_space(sid) else { return };
                if matches!(kind, reactor::SpaceEventKind::User)
                    && current_space.is_some_and(|current| current != sid)
                {
                    // WindowServer reports a confirmed move out of the origin space as
                    // a destroy. The selected Space may remain the same, so explicitly
                    // forward the refreshed membership.
                    self.handle_event(Event::ReconcileWindowSpaces);
                    return;
                }
                self.reactor_tx.send(reactor::Event::WindowServerDestroyed(wsid, sid, kind));
            }
            Event::ProcessScreenRefresh { attempt } => self.process_screen_refresh(attempt),
            Event::CheckDisplayStabilization { expected_epoch, attempt } => {
                self.attempt_finish_display_churn(expected_epoch, attempt);
            }
            Event::CheckDisplaysSettled { expected_epoch } => {
                if expected_epoch == self.state.display_churn_epoch
                    && self.topology_is_authoritative()
                {
                    self.reactor_tx.send(reactor::Event::DisplaysSettled);
                }
            }
        }
    }

    fn enter_lifecycle_quarantine(&mut self) {
        self.invalidate_topology();
        self.state.recovering_membership = false;
    }

    /// Wake and unlock are inherently unstable, and the login window can replace
    /// every display's current space, so wait for a fresh authoritative rescan.
    fn leave_lifecycle_quarantine(&mut self) {
        self.mark_screens_dirty();
        if self.state.display_churn_active {
            self.schedule_display_stabilization(self.state.display_churn_epoch, 0);
        }
        self.state.recovering_membership = true;
        self.schedule_screen_refresh_after(REFRESH_DELAY_NS, 0);
    }

    fn mark_screens_dirty(&mut self) {
        if let Some(cache) = self.state.screen_cache.as_mut() {
            cache.mark_dirty();
        }
    }

    fn handle_active_display_changed_for(&mut self, active_display_uuid: Option<&str>) {
        if active_display_uuid.is_some()
            && active_display_uuid == self.state.active_display_uuid.as_deref()
        {
            return;
        }
        if !self.topology_unstable()
            && let Some(uuid) = active_display_uuid
            && let Some(space) = self
                .state
                .screens
                .iter()
                .find(|screen| screen.display_uuid == uuid)
                .and_then(|screen| screen.space)
        {
            self.state.active_display_uuid = Some(uuid.to_string());
            self.reactor_tx.send(reactor::Event::ActiveDisplayChanged {
                menu_bar_space: Some(space),
                command_space: Some(space),
            });
            return;
        }
        if !self.try_forward_authoritative_snapshot(true) {
            self.schedule_screen_refresh_after(0, 0);
        }
    }

    /// Space create/destroy and membership changes can leave the visible spaces
    /// unchanged while still changing per-display space metadata.
    fn handle_space_inventory_changed(&mut self) {
        if self.topology_unstable() {
            self.schedule_screen_refresh_after(0, 0);
        } else if !self.try_forward_authoritative_snapshot(true) {
            self.schedule_screen_refresh_after(REFRESH_DELAY_NS, 0);
        }
    }

    // Sleep and session inactivity end only on wake or unlock; churn clears itself.
    fn lifecycle_quarantined(&self) -> bool { self.state.sleeping || self.state.session_inactive }

    fn topology_unstable(&self) -> bool {
        self.lifecycle_quarantined() || self.state.display_churn_active
    }

    /// Whether the last forwarded snapshot still describes the native topology.
    /// Window enter/leave events are dropped otherwise; the next snapshot carries
    /// the membership they would have changed.
    fn topology_is_authoritative(&self) -> bool {
        !self.topology_unstable()
            && self
                .state
                .last_forwarded
                .as_ref()
                .is_some_and(|snapshot| snapshot.revision == self.state.revision)
    }

    fn collect_state(&mut self) -> Option<(Vec<ScreenInfo>, CoordinateConverter)> {
        match self.state.screen_cache.as_mut() {
            Some(cache) => cache.refresh(),
            None if cfg!(test) => Some((self.state.screens.clone(), self.state.last_converter)),
            None => None,
        }
    }

    /// Forwards a full snapshot. Returns whether its window membership is complete.
    fn forward_screen_parameters(
        &mut self,
        screens: Vec<ScreenInfo>,
        converter: CoordinateConverter,
    ) -> bool {
        self.state.last_converter = converter;
        let forwarded = self.build_forwarded_state(screens);
        self.state.last_sent_spaces =
            Some(forwarded.screens.iter().map(|screen| screen.space).collect());
        self.state.awaiting_space_switch_confirmation = false;
        let membership_complete = forwarded.membership_complete;
        self.wm_tx.send(wm_controller::WmEvent::SpaceStateUpdated(
            forwarded,
            self.state.last_converter,
        ));
        membership_complete
    }

    fn build_forwarded_state(&mut self, mut screens: Vec<ScreenInfo>) -> ForwardedSpaceState {
        let previous_screens = std::mem::take(&mut self.state.screens);
        let fullscreen_spaces: HashSet<SpaceId> = screens
            .iter()
            .filter_map(|screen| screen.space)
            .filter(|space| is_fullscreen_space(*space))
            .collect();
        preserve_user_spaces_during_fullscreen_transition(&previous_screens, &mut screens);
        for screen in &mut screens {
            if screen
                .space
                .is_some_and(|space| classify_space(space).is_none_or(is_fullscreen))
            {
                screen.space = None;
            }
        }

        let display_set_changed = displays(&previous_screens) != displays(&screens);
        let previous_frames: HashMap<_, _> = previous_screens
            .iter()
            .map(|screen| (screen.display_uuid.as_str(), screen.frame))
            .collect();
        let topology_changed = display_set_changed
            || previous_screens
                .iter()
                .map(|screen| &screen.display_uuid)
                .ne(screens.iter().map(|screen| &screen.display_uuid))
            || screens.iter().any(|screen| {
                previous_frames
                    .get(screen.display_uuid.as_str())
                    .is_some_and(|previous| *previous != screen.frame)
            });
        // The very first display set is not a change.
        let refresh_layout = topology_changed && self.state.has_seen_display_set;
        self.state.has_seen_display_set |= !screens.is_empty();

        let previous_sizes: HashMap<_, _> =
            previous_screens.iter().map(|screen| (screen.id, screen.frame.size)).collect();
        let resized_spaces = screens
            .iter()
            .filter(|screen| {
                previous_sizes.get(&screen.id).is_none_or(|previous| {
                    previous.width.round() as i32 != screen.frame.size.width.round() as i32
                        || previous.height.round() as i32 != screen.frame.size.height.round() as i32
                })
            })
            .filter_map(|screen| Some((screen.space?, screen.frame.size)))
            .collect();

        let active_display_uuid = crate::sys::screen::active_menu_bar_display_uuid();
        let active_space = crate::sys::screen::get_active_space_number();
        let menu_bar_space = resolve_active_display_space(&screens, None, active_space);
        let command_space =
            resolve_active_display_space(&screens, active_display_uuid.as_deref(), active_space);
        self.state.active_display_uuid = active_display_uuid
            .filter(|uuid| screens.iter().any(|screen| screen.display_uuid == *uuid))
            .or_else(|| {
                let space = command_space?;
                let screen = screens.iter().find(|screen| screen.space == Some(space))?;
                Some(screen.display_uuid.clone())
            });

        let display_space_ids = display_space_ids(&screens);
        // Full native inventories distinguish identity loss from switching the
        // current Space. This can be visible before a display callback arrives.
        let inventory_lost_space = self.state.last_forwarded.as_ref().is_some_and(|previous| {
            previous.display_space_ids.iter().any(|(display, spaces)| {
                let current = display_space_ids.get(display);
                spaces
                    .iter()
                    .any(|space| !current.is_some_and(|current| current.contains(space)))
            })
        });

        let (membership, membership_complete) = self.visible_window_spaces_for_screens(&screens);
        // An incomplete sample cannot show which windows left, so the baseline waits for
        // a complete one, even after the churn itself has ended.
        let topology_window_delta = membership_complete
            .then(|| self.state.churn_baseline.take())
            .flatten()
            .map(|(baseline, flags)| topology_window_delta(&baseline, &membership, flags));
        self.state.visible_window_spaces = membership;
        self.state.screens = screens.clone();
        self.state.recovering_membership = false;
        let mut forwarded = ForwardedSpaceState {
            revision: self.state.revision,
            authoritative: true,
            membership_complete,
            screens,
            fullscreen_spaces,
            menu_bar_space,
            command_space,
            display_space_ids,
            display_set_changed,
            should_force_refresh_layout: refresh_layout || inventory_lost_space,
            resized_spaces,
            topology_window_delta,
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
        }
        self.state.last_forwarded = Some(forwarded.clone());
        forwarded
    }

    fn invalidate_topology(&mut self) {
        self.state.revision = self.state.revision.wrapping_add(1);
        self.state.display_topology_state = None;
        self.reactor_tx.send(reactor::Event::TopologyInvalidated(self.state.revision));
    }

    /// Native membership of every window on the screens' spaces, and whether every
    /// per-space query succeeded.
    fn visible_window_spaces_for_screens(
        &self,
        screens: &[ScreenInfo],
    ) -> (HashMap<WindowServerId, SpaceId>, bool) {
        let previous = &self.state.visible_window_spaces;
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
        // behind another, so query every active native space independently.
        let mut visible = HashMap::default();
        let mut complete = true;
        for &space in &active_spaces {
            let Some(ids) =
                window_server::try_space_window_list_for_connection(&[space.get()], 0, false)
            else {
                complete = false;
                visible.extend(previous.iter().filter(|(_, known)| **known == space));
                continue;
            };
            for wsid in ids.into_iter().map(WindowServerId::new) {
                let conflicting_space = visible
                    .get(&wsid)
                    .filter(|&&known| known != space)
                    .and_then(|_| window_server::window_space(wsid));
                Self::record_visible_window_space(
                    &mut visible,
                    previous,
                    &active_space_set,
                    wsid,
                    space,
                    conflicting_space,
                );
            }
        }

        // The first coherent snapshot after wake/unlock can race WindowServer and
        // temporarily contain no windows; carry accepted membership forward then.
        // Otherwise an empty result is authoritative (and needed to reconcile
        // windows whose destroy notifications were quarantined during churn).
        if visible.is_empty() && self.state.recovering_membership {
            let carried = previous
                .iter()
                .filter(|(_, space)| active_space_set.contains(*space))
                .map(|(&wsid, &space)| (wsid, space))
                .collect();
            return (carried, false);
        }
        (visible, complete)
    }

    /// Conflicting per-space results are transitional and therefore ambiguous.
    /// Preserve a still-active accepted assignment until one per-space query becomes
    /// unique. `window_space(wsid)` can return several user spaces in an unstable
    /// order during display churn, so it is only a fallback when there is no
    /// accepted active assignment.
    fn record_visible_window_space(
        visible: &mut HashMap<WindowServerId, SpaceId>,
        previous_visible: &HashMap<WindowServerId, SpaceId>,
        active_spaces: &HashSet<SpaceId>,
        wsid: WindowServerId,
        candidate_space: SpaceId,
        authoritative_space: Option<SpaceId>,
    ) {
        let Some(&current) = visible.get(&wsid) else {
            visible.insert(wsid, candidate_space);
            return;
        };
        if current != candidate_space {
            let active = |space: &SpaceId| active_spaces.contains(space);
            let resolved = (previous_visible.get(&wsid).copied().filter(active))
                .or(authoritative_space.filter(active))
                .unwrap_or(current);
            visible.insert(wsid, resolved);
        }
    }

    fn process_screen_refresh(&mut self, attempt: u8) {
        if !self.state.timers_disabled && self.state.refresh_pending != Some(attempt) {
            return; // Superseded or cancelled.
        }
        self.state.refresh_pending = None;
        if self.topology_unstable() {
            return;
        }
        let next = attempt.saturating_add(1);
        let screens = self
            .collect_state()
            .filter(|(screens, _)| snapshot_is_ready_for_commit(screens));
        let Some((screens, converter)) = screens else {
            self.schedule_screen_refresh_after(retry_delay(attempt), next);
            return;
        };

        if self.invalidated() && !self.observe_stable_topology(&screens) {
            self.schedule_screen_refresh_after(retry_delay(attempt), next);
            return;
        }

        let spaces: Vec<_> = screens.iter().map(|screen| screen.space).collect();
        if self.state.awaiting_space_switch_confirmation
            && self.state.last_sent_spaces.as_ref() == Some(&spaces)
            && attempt < REFRESH_MAX_RETRIES
        {
            self.schedule_screen_refresh_after(REFRESH_SPACE_SWITCH_DELAY_NS, next);
            return;
        }

        // Incomplete membership is retried until a query succeeds; the reactor keeps
        // the last accepted ownership meanwhile.
        if !self.forward_screen_parameters(screens, converter) {
            self.schedule_screen_refresh_after(retry_delay(attempt), next);
        }
    }

    /// Whether the last forwarded snapshot predates an invalidation, so a new one
    /// needs a stable sample.
    fn invalidated(&self) -> bool {
        self.state.revision > self.state.last_forwarded.as_ref().map_or(0, |s| s.revision)
    }

    /// Samples and forwards the current topology now if nothing makes it unsafe.
    /// Returns false when the caller should fall back to a scheduled refresh.
    fn try_forward_authoritative_snapshot(&mut self, force: bool) -> bool {
        // A refresh still in its quick retries is waiting out transient state; one
        // backing off is not, and should not delay a fresh event.
        if self.state.refresh_pending.is_some_and(|attempt| attempt <= REFRESH_MAX_RETRIES)
            || self.topology_unstable()
            || self.state.recovering_membership
            || self.invalidated()
        {
            return false;
        }
        let Some((screens, converter)) = self.collect_state() else {
            return false;
        };
        if !snapshot_is_ready_for_commit(&screens) {
            return false;
        }

        // Membership can change before the display callback arrives. If the
        // refresh already sees physical churn, use the stabilization flow.
        let physical_topology_changed =
            self.state.screens.len() != screens.len()
                || screens.iter().any(|screen| {
                    !self.state.screens.iter().any(|old| {
                        old.display_uuid == screen.display_uuid && old.frame == screen.frame
                    })
                });
        if !self.state.screens.is_empty() && physical_topology_changed {
            let epoch = self.begin_display_churn(DisplayReconfigFlags::empty());
            self.schedule_display_stabilization(epoch, 0);
            return false;
        }
        let spaces: Vec<_> = screens.iter().map(|screen| screen.space).collect();
        if !force && self.state.last_sent_spaces.as_ref() == Some(&spaces) {
            return false;
        }
        // A complete sample is what a backing-off refresh was waiting for.
        self.state.refresh_pending = None;
        if !self.forward_screen_parameters(screens, converter) {
            self.schedule_screen_refresh_after(REFRESH_DELAY_NS, 1);
        }
        true
    }

    /// Schedules a refresh. A new chain (`attempt == 0`) is not started while one is
    /// pending or while churn stabilization owns resampling. Only the timer of the
    /// last scheduled attempt runs; an earlier one finds itself superseded.
    fn schedule_screen_refresh_after(&mut self, delay_ns: i64, attempt: u8) {
        if self.state.timers_disabled
            || (attempt == 0
                && (self.state.display_churn_active || self.state.refresh_pending.is_some()))
        {
            return;
        }
        self.state.refresh_pending = Some(attempt);
        self.after(delay_ns, Event::ProcessScreenRefresh { attempt });
    }

    fn begin_display_churn(&mut self, flags: DisplayReconfigFlags) -> u64 {
        match &mut self.state.churn_baseline {
            // A churn not yet reconciled by a complete sample keeps its older baseline.
            Some((_, pending_flags)) => *pending_flags |= flags,
            None => {
                // Live membership may already have changed before the display callback.
                let baseline = self.state.last_forwarded.as_ref().map_or_else(
                    || self.state.visible_window_spaces.clone(),
                    |state| state.active_window_spaces.clone(),
                );
                self.state.churn_baseline = Some((baseline, flags));
            }
        }
        // One physical change sends several callbacks. Each restarts stabilization, but
        // the reactor only needs to hear that the topology is no longer authoritative once.
        if !self.state.display_churn_active {
            self.invalidate_topology();
        } else {
            self.state.display_topology_state = None;
        }
        self.state.display_churn_active = true;
        self.state.display_churn_epoch = self.state.display_churn_epoch.wrapping_add(1);
        self.state.last_sent_spaces = None;
        self.state.display_churn_epoch
    }

    fn schedule_display_stabilization(&self, expected_epoch: u64, attempt: u8) {
        if !self.state.timers_disabled {
            self.after(DISPLAY_STABILIZE_DELAY_NS, Event::CheckDisplayStabilization {
                expected_epoch,
                attempt,
            });
        }
    }

    fn after(&self, delay_ns: i64, event: Event) {
        queue::main().after_f_s(
            Time::new_after(Time::NOW, delay_ns),
            (self.sender.clone(), event),
            |(sender, event)| sender.send(event),
        );
    }

    /// Records a topology sample; true once it has repeated enough while the
    /// WindowServer is quiet.
    fn observe_stable_topology(&mut self, screens: &[ScreenInfo]) -> bool {
        let fingerprint: TopologyFingerprint = screens
            .iter()
            .map(|screen| {
                let frame = screen.frame;
                let bits = [
                    frame.origin.x,
                    frame.origin.y,
                    frame.size.width,
                    frame.size.height,
                ]
                .map(f64::to_bits);
                (screen.display_uuid.clone(), bits, screen.space)
            })
            .collect();
        let hits = match &mut self.state.display_topology_state {
            Some((seen, hits)) if *seen == fingerprint => {
                *hits = hits.saturating_add(1);
                *hits
            }
            state => {
                *state = Some((fingerprint, 1));
                1
            }
        };
        hits >= DISPLAY_STABLE_REQUIRED_HITS
            && window_server::windowserver_quiet_for_us(window_server::WINDOWSERVER_QUIET_US)
    }

    fn attempt_finish_display_churn(&mut self, expected_epoch: u64, attempt: u8) {
        // A topology seen while sleeping/locked cannot establish authority.
        // Wake/unlock already schedules a fresh stabilization check.
        if expected_epoch != self.state.display_churn_epoch
            || !self.state.display_churn_active
            || self.lifecycle_quarantined()
        {
            return;
        }

        if let Some((screens, converter)) = self.collect_state()
            && !screens.is_empty()
            && self.observe_stable_topology(&screens)
        {
            if snapshot_is_ready_for_commit(&screens) {
                // Publish the normalized topology and its single membership sample together.
                let membership_complete = self.forward_screen_parameters(screens, converter);
                self.finish_display_churn(!membership_complete);
                return;
            }
            self.state.display_topology_state = None;
        }

        if attempt < DISPLAY_STABILIZE_MAX_ATTEMPTS {
            self.schedule_display_stabilization(expected_epoch, attempt + 1);
        } else {
            self.finish_display_churn(true);
        }
    }

    fn finish_display_churn(&mut self, schedule_refresh: bool) {
        self.state.display_churn_active = false;
        self.state.display_churn_epoch = self.state.display_churn_epoch.wrapping_add(1);
        self.state.display_topology_state = None;

        if schedule_refresh {
            self.schedule_screen_refresh_after(0, 0);
        }
        if !self.state.timers_disabled {
            let expected_epoch = self.state.display_churn_epoch;
            for delay in DISPLAY_SETTLED_NS {
                self.after(delay, Event::CheckDisplaysSettled { expected_epoch });
            }
        }
    }
}

/// The delay before retry `attempt`: quick at first, then backing off so a
/// WindowServer that stays busy is not sampled at full rate indefinitely.
fn retry_delay(attempt: u8) -> i64 {
    REFRESH_DELAY_NS << attempt.saturating_sub(REFRESH_MAX_RETRIES).min(REFRESH_MAX_BACKOFF)
}

/// Windows that left or entered a space between `before` and `after`. A window that
/// changed spaces does both.
fn topology_window_delta(
    before: &HashMap<WindowServerId, SpaceId>,
    after: &HashMap<WindowServerId, SpaceId>,
    flags: DisplayReconfigFlags,
) -> TopologyWindowDelta {
    let mut delta = TopologyWindowDelta { flags, ..Default::default() };
    for (&wsid, &space) in after {
        match before.get(&wsid) {
            Some(&previous) if previous == space => {}
            Some(&previous) => {
                delta.disappeared.push((wsid, previous));
                delta.appeared.push((wsid, space));
            }
            None => delta.appeared.push((wsid, space)),
        }
    }
    for (&wsid, &space) in before {
        if !after.contains_key(&wsid) {
            delta.disappeared.push((wsid, space));
        }
    }
    delta
}

fn is_fullscreen_space(space: SpaceId) -> bool { window_server::space_is_fullscreen(space.get()) }

fn is_fullscreen(kind: reactor::SpaceEventKind) -> bool {
    matches!(kind, reactor::SpaceEventKind::Fullscreen)
}

/// The kind of a space Rift tracks, or None for system/login spaces.
fn classify_space(space: SpaceId) -> Option<reactor::SpaceEventKind> {
    if is_fullscreen_space(space) {
        Some(reactor::SpaceEventKind::Fullscreen)
    } else {
        window_server::space_is_user(space.get()).then_some(reactor::SpaceEventKind::User)
    }
}

fn displays(screens: &[ScreenInfo]) -> HashSet<&str> {
    screens.iter().map(|screen| screen.display_uuid.as_str()).collect()
}

#[cfg(not(test))]
fn display_space_ids(_screens: &[ScreenInfo]) -> HashMap<String, Vec<SpaceId>> {
    crate::sys::screen::managed_display_space_ids()
}

#[cfg(test)]
fn display_space_ids(screens: &[ScreenInfo]) -> HashMap<String, Vec<SpaceId>> {
    let mut ids: HashMap<String, Vec<SpaceId>> = HashMap::default();
    for screen in screens {
        if let Some(space) = screen.space {
            ids.entry(screen.display_uuid.clone()).or_default().push(space);
        }
    }
    ids
}

/// The space of the active display, else the active space if on a screen, else
/// the first screen's space.
fn resolve_active_display_space(
    screens: &[ScreenInfo],
    active_display_uuid: Option<&str>,
    active_space: Option<SpaceId>,
) -> Option<SpaceId> {
    active_display_uuid
        .and_then(|uuid| screens.iter().find(|screen| screen.display_uuid == uuid)?.space)
        .or_else(|| {
            active_space.filter(|space| screens.iter().any(|screen| screen.space == Some(*space)))
        })
        .or_else(|| screens.iter().find_map(|screen| screen.space))
}

/// A snapshot can be committed only with screens, a space on each, and no user
/// space shown twice.
fn snapshot_is_ready_for_commit(screens: &[ScreenInfo]) -> bool {
    let mut seen_user_spaces = HashSet::default();
    !screens.is_empty()
        && screens.iter().all(|screen| screen.space.is_some())
        && screens.iter().all(|screen| match screen.space {
            Some(space) if !is_fullscreen_space(space) => seen_user_spaces.insert(space),
            _ => true,
        })
}

/// Entering fullscreen on one display can make macOS briefly report another
/// display's user space on a display that kept its own. Restore the previous
/// space on such displays.
fn preserve_user_spaces_during_fullscreen_transition(
    previous_screens: &[ScreenInfo],
    screens: &mut [ScreenInfo],
) {
    let user_space =
        |screen: &ScreenInfo| screen.space.filter(|space| !is_fullscreen_space(*space));
    let previous_by_display: HashMap<&str, &ScreenInfo> = previous_screens
        .iter()
        .filter_map(|screen| Some((screen.display_uuid_opt()?, screen)))
        .collect();
    let entering_fullscreen = screens.iter().any(|next| {
        next.space.is_some_and(is_fullscreen_space)
            && next
                .display_uuid_opt()
                .and_then(|display| previous_by_display.get(display))
                .is_some_and(|previous| user_space(previous).is_some())
    });
    if !entering_fullscreen {
        return;
    }
    let previous_owner: HashMap<SpaceId, &str> = previous_screens
        .iter()
        .filter_map(|screen| Some((user_space(screen)?, screen.display_uuid_opt()?)))
        .collect();
    for next in screens.iter_mut() {
        let Some(display) = next.display_uuid_opt() else {
            continue;
        };
        let Some(previous_space) = previous_by_display.get(display).and_then(|s| user_space(s))
        else {
            continue;
        };
        let Some(new_space) = user_space(next) else { continue };
        if new_space != previous_space
            && previous_owner.get(&new_space).is_some_and(|owner| *owner != display)
        {
            next.space = Some(previous_space);
        }
    }
}

#[cfg(test)]
mod tests;
