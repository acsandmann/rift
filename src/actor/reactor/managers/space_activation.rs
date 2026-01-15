use crate::common::collections::{HashMap, HashSet};
use crate::sys::screen::{ScreenId, SpaceId};

/// this is how we decide which macos spaces (and/or displays) are considered active.
///
/// driven by raw input:
/// - current screen -> (space, display_uuid) snapshots
/// - login window activation state
/// - configuration flags (default_disable, one_space)
/// - user "toggle" commands (target space/display context)
#[derive(Debug, Default)]
pub struct SpaceActivationPolicy {
    disabled_spaces: HashSet<SpaceId>,
    enabled_spaces: HashSet<SpaceId>,

    disabled_displays: HashSet<String>,
    enabled_displays: HashSet<String>,

    starting_space: Option<SpaceId>,

    last_known_space_by_screen: HashMap<ScreenId, SpaceId>,

    pub login_window_active: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct SpaceActivationConfig {
    pub default_disable: bool,
    pub one_space: bool,
}

#[derive(Debug, Clone)]
pub struct ScreenActivationInput {
    pub screen_id: ScreenId,
    /// can be None during transitions
    pub space: Option<SpaceId>,
    pub display_uuid: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ToggleSpaceContext {
    pub space: SpaceId,
    pub display_uuid: Option<String>,
}

impl SpaceActivationPolicy {
    pub fn new() -> Self {
        Self {
            disabled_spaces: HashSet::default(),
            enabled_spaces: HashSet::default(),
            disabled_displays: HashSet::default(),
            enabled_displays: HashSet::default(),
            starting_space: None,
            last_known_space_by_screen: HashMap::default(),
            login_window_active: false,
        }
    }

    pub fn set_login_window_active(&mut self, active: bool) { self.login_window_active = active; }

    /// Note: this emits no events; Reactor should call this and then recompute active spaces.
    pub fn on_spaces_updated(
        &mut self,
        cfg: SpaceActivationConfig,
        screens: &[ScreenActivationInput],
    ) {
        // rebuild to prune old activation states
        let active_spaces: HashSet<SpaceId> = screens.iter().filter_map(|s| s.space).collect();
        let active_displays: HashSet<String> =
            screens.iter().filter_map(|s| s.display_uuid.clone()).collect();
        let active_screen_ids: HashSet<ScreenId> = screens.iter().map(|s| s.screen_id).collect();

        self.disabled_spaces.retain(|space| active_spaces.contains(space));
        self.enabled_spaces.retain(|space| active_spaces.contains(space));
        self.disabled_displays.retain(|uuid| active_displays.contains(uuid));
        self.enabled_displays.retain(|uuid| active_displays.contains(uuid));
        self.last_known_space_by_screen.retain(|sid, _| active_screen_ids.contains(sid));

        // transfer activation state since space ids can churn sometimes (why does this happen apple)
        for screen in screens.iter() {
            let Some(new_space) = screen.space else { continue };

            // Capture "previous" space per screen from the last known snapshot (not current inputs).
            // Using current `screens[..].space` here would make `previous_space == new_space` and
            // prevent activation transfer from ever triggering.
            let previous_space = self.last_known_space_by_screen.get(&screen.screen_id).copied();

            if let Some(previous_space) = previous_space
                && previous_space != new_space
            {
                self.transfer_space_activation(cfg, previous_space, new_space);
            }

            self.last_known_space_by_screen.insert(screen.screen_id, new_space);
        }

        // apply display level activation status
        for screen in screens {
            let Some(space) = screen.space else { continue };
            let Some(display_uuid) = screen.display_uuid.as_deref() else {
                continue;
            };

            if cfg.default_disable {
                if self.enabled_displays.contains(display_uuid) {
                    self.enabled_spaces.insert(space);
                }
            } else {
                if self.disabled_displays.contains(display_uuid) {
                    self.disabled_spaces.insert(space);
                }
            }
        }

        if let Some(starting) = self.starting_space {
            if !active_spaces.contains(&starting) {
                self.starting_space = None;
            }
        }

        if self.starting_space.is_none() {
            self.starting_space = screens.first().and_then(|s| s.space);
        }
    }

    /// This mutates the policy state only; Reactor is responsible for recomputing
    /// active spaces and performing any follow-up actions.
    pub fn toggle_space_activated(&mut self, cfg: SpaceActivationConfig, ctx: ToggleSpaceContext) {
        let space_currently_enabled = if cfg.default_disable {
            self.enabled_spaces.contains(&ctx.space)
        } else {
            !self.disabled_spaces.contains(&ctx.space)
        };

        if space_currently_enabled {
            if cfg.default_disable {
                self.enabled_spaces.remove(&ctx.space);
                if let Some(uuid) = ctx.display_uuid.as_ref() {
                    self.enabled_displays.remove(uuid);
                }
            } else {
                self.disabled_spaces.insert(ctx.space);
                if let Some(uuid) = ctx.display_uuid.as_ref() {
                    self.disabled_displays.insert(uuid.clone());
                }
            }
        } else if cfg.default_disable {
            self.enabled_spaces.insert(ctx.space);
            if let Some(uuid) = ctx.display_uuid.as_ref() {
                self.enabled_displays.insert(uuid.clone());
            }
        } else {
            self.disabled_spaces.remove(&ctx.space);
            if let Some(uuid) = ctx.display_uuid.as_ref() {
                self.disabled_displays.remove(uuid);
            }
        }
    }

    pub fn compute_active_spaces(
        &self,
        cfg: SpaceActivationConfig,
        cur_spaces: &[Option<SpaceId>],
        cur_display_uuids: &[Option<String>],
    ) -> Vec<Option<SpaceId>> {
        let mut out: Vec<Option<SpaceId>> = cur_spaces.to_vec();

        for (idx, space_opt) in out.iter_mut().enumerate() {
            let display_uuid = cur_display_uuids.get(idx).and_then(|v| v.as_ref());
            let display_enabled =
                display_uuid.map(|u| self.enabled_displays.contains(u)).unwrap_or(false);
            let display_disabled =
                display_uuid.map(|u| self.disabled_displays.contains(u)).unwrap_or(false);

            // this is the core logic for deciding whats what
            let enabled = match *space_opt {
                _ if self.login_window_active => false,
                Some(space) if cfg.one_space && Some(space) != self.starting_space => false,
                Some(space) if self.disabled_spaces.contains(&space) => false,
                _ if display_disabled => false,
                Some(space) if self.enabled_spaces.contains(&space) => true,
                _ if display_enabled => true,
                _ if cfg.default_disable => false,
                _ => true,
            };

            if !enabled {
                *space_opt = None;
            }
        }

        out
    }

    fn transfer_space_activation(
        &mut self,
        cfg: SpaceActivationConfig,
        old_space: SpaceId,
        new_space: SpaceId,
    ) {
        if cfg.default_disable {
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
}
