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

    known_user_spaces: HashSet<SpaceId>,

    starting_space: Option<SpaceId>,

    last_known_space_by_screen: HashMap<ScreenId, SpaceId>,
    last_known_display_by_screen: HashMap<ScreenId, String>,

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
            known_user_spaces: HashSet::default(),
            starting_space: None,
            last_known_space_by_screen: HashMap::default(),
            last_known_display_by_screen: HashMap::default(),
            login_window_active: false,
        }
    }

    pub fn set_login_window_active(&mut self, active: bool) { self.login_window_active = active; }

    #[allow(dead_code)]
    pub fn on_space_created(&mut self, space: SpaceId) {
        self.known_user_spaces.insert(space);
    }

    #[allow(dead_code)]
    pub fn on_space_destroyed(&mut self, space: SpaceId) {
        self.known_user_spaces.remove(&space);
    }

    /// Note: this emits no events; Reactor should call this and then recompute active spaces.
    pub fn on_spaces_updated(
        &mut self,
        cfg: SpaceActivationConfig,
        screens: &[ScreenActivationInput],
    ) {
        // rebuild to prune old activation states
        let active_spaces: HashSet<SpaceId> = screens.iter().filter_map(|s| s.space).collect();
        let active_screen_ids: HashSet<ScreenId> = screens.iter().map(|s| s.screen_id).collect();

        self.last_known_space_by_screen.retain(|sid, _| active_screen_ids.contains(sid));
        self.last_known_display_by_screen
            .retain(|sid, _| active_screen_ids.contains(sid));

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
                // Only transfer activation when the previous space id is no longer known
                // (e.g. space id churn on reconnect), not for normal space switches.
                if !self.known_user_spaces.contains(&previous_space) {
                    self.transfer_space_activation(cfg, previous_space, new_space);
                }
            }

            self.known_user_spaces.insert(new_space);
            self.last_known_space_by_screen.insert(screen.screen_id, new_space);
        }

        // transfer activation across display UUID churns (e.g. wake/reconnect)
        for screen in screens.iter() {
            let Some(new_display) = screen.display_uuid.as_deref() else {
                continue;
            };
            if let Some(previous_display) =
                self.last_known_display_by_screen.get(&screen.screen_id).cloned()
            {
                if previous_display != new_display {
                    self.transfer_display_activation(cfg, &previous_display, new_display);
                }
            }
            self.last_known_display_by_screen
                .insert(screen.screen_id, new_display.to_string());
        }

        let mut active_displays: HashSet<String> = HashSet::default();
        for screen in screens {
            if let Some(display_uuid) = screen.display_uuid.as_deref() {
                active_displays.insert(display_uuid.to_string());
                if let Some(prev) = self.last_known_display_by_screen.get(&screen.screen_id) {
                    active_displays.insert(prev.clone());
                }
            } else if let Some(prev) = self.last_known_display_by_screen.get(&screen.screen_id) {
                active_displays.insert(prev.clone());
            }
        }

        self.disabled_displays.retain(|uuid| active_displays.contains(uuid));
        self.enabled_displays.retain(|uuid| active_displays.contains(uuid));

        // apply display level activation status
        for screen in screens {
            let Some(space) = screen.space else { continue };
            let display_uuid = screen.display_uuid.as_deref().or_else(|| {
                self.last_known_display_by_screen.get(&screen.screen_id).map(|v| v.as_str())
            });
            let Some(display_uuid) = display_uuid else { continue };

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
            }
        } else if cfg.default_disable {
            self.enabled_spaces.insert(ctx.space);
            if let Some(uuid) = ctx.display_uuid.as_ref() {
                self.enabled_displays.insert(uuid.clone());
            }
        } else {
            self.disabled_spaces.remove(&ctx.space);
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
            let (display_enabled, display_disabled) = if cfg.default_disable {
                (
                    display_uuid.map(|u| self.enabled_displays.contains(u)).unwrap_or(false),
                    display_uuid.map(|u| self.disabled_displays.contains(u)).unwrap_or(false),
                )
            } else {
                (false, false)
            };

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

    fn transfer_display_activation(
        &mut self,
        cfg: SpaceActivationConfig,
        old_display: &str,
        new_display: &str,
    ) {
        if cfg.default_disable {
            if self.enabled_displays.remove(old_display) {
                self.enabled_displays.insert(new_display.to_string());
            }
        } else if self.disabled_displays.remove(old_display) {
            self.disabled_displays.insert(new_display.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(
        screen_id: u32,
        space: Option<u64>,
        display_uuid: Option<&str>,
    ) -> ScreenActivationInput {
        ScreenActivationInput {
            screen_id: ScreenId::new(screen_id),
            space: space.map(SpaceId::new),
            display_uuid: display_uuid.map(|v| v.to_string()),
        }
    }

    #[test]
    fn toggle_space_activation_default_disable_round_trip() {
        let mut policy = SpaceActivationPolicy::new();
        let cfg = SpaceActivationConfig {
            default_disable: true,
            one_space: false,
        };
        let ctx = ToggleSpaceContext {
            space: SpaceId::new(1),
            display_uuid: Some("display-a".to_string()),
        };

        policy.toggle_space_activated(cfg, ctx.clone());
        assert!(policy.enabled_spaces.contains(&SpaceId::new(1)));
        assert!(policy.enabled_displays.contains("display-a"));

        policy.toggle_space_activated(cfg, ctx);
        assert!(!policy.enabled_spaces.contains(&SpaceId::new(1)));
        assert!(!policy.enabled_displays.contains("display-a"));
    }

    #[test]
    fn preserves_display_state_when_uuid_missing() {
        let mut policy = SpaceActivationPolicy::new();
        let cfg = SpaceActivationConfig {
            default_disable: true,
            one_space: false,
        };

        policy.on_spaces_updated(cfg, &[input(1, Some(1), Some("display-a"))]);
        policy.toggle_space_activated(cfg, ToggleSpaceContext {
            space: SpaceId::new(1),
            display_uuid: Some("display-a".to_string()),
        });

        policy.on_spaces_updated(cfg, &[input(1, Some(1), None)]);

        assert!(policy.enabled_displays.contains("display-a"));
        assert!(policy.enabled_spaces.contains(&SpaceId::new(1)));
    }

    #[test]
    fn preserves_disabled_space_state_when_uuid_missing_default_enable() {
        let mut policy = SpaceActivationPolicy::new();
        let cfg = SpaceActivationConfig {
            default_disable: false,
            one_space: false,
        };

        policy.on_spaces_updated(cfg, &[input(1, Some(1), Some("display-a"))]);
        policy.toggle_space_activated(cfg, ToggleSpaceContext {
            space: SpaceId::new(1),
            display_uuid: Some("display-a".to_string()),
        });

        policy.on_spaces_updated(cfg, &[input(1, Some(1), None)]);

        assert!(policy.disabled_spaces.contains(&SpaceId::new(1)));
    }

    #[test]
    fn transfers_enabled_display_on_uuid_change_default_disable() {
        let mut policy = SpaceActivationPolicy::new();
        let cfg = SpaceActivationConfig {
            default_disable: true,
            one_space: false,
        };

        policy.on_spaces_updated(cfg, &[input(1, Some(1), Some("display-a"))]);
        policy.toggle_space_activated(cfg, ToggleSpaceContext {
            space: SpaceId::new(1),
            display_uuid: Some("display-a".to_string()),
        });

        policy.on_spaces_updated(cfg, &[input(1, Some(1), Some("display-b"))]);

        assert!(!policy.enabled_displays.contains("display-a"));
        assert!(policy.enabled_displays.contains("display-b"));
    }

    #[test]
    fn one_space_config_disables_non_starting_spaces() {
        let mut policy = SpaceActivationPolicy::new();
        let cfg = SpaceActivationConfig {
            default_disable: false,
            one_space: true,
        };

        policy.on_spaces_updated(cfg, &[
            input(1, Some(1), Some("display-a")),
            input(2, Some(2), Some("display-b")),
        ]);

        let active =
            policy.compute_active_spaces(cfg, &[Some(SpaceId::new(1)), Some(SpaceId::new(2))], &[
                Some("display-a".to_string()),
                Some("display-b".to_string()),
            ]);

        assert_eq!(active, vec![Some(SpaceId::new(1)), None]);
    }

    #[test]
    fn disabled_space_does_not_block_other_spaces_default_enable() {
        let mut policy = SpaceActivationPolicy::new();
        let cfg = SpaceActivationConfig {
            default_disable: false,
            one_space: false,
        };

        policy.on_spaces_updated(cfg, &[input(1, Some(1), Some("display-a"))]);
        policy.toggle_space_activated(cfg, ToggleSpaceContext {
            space: SpaceId::new(1),
            display_uuid: Some("display-a".to_string()),
        });

        let active =
            policy.compute_active_spaces(cfg, &[Some(SpaceId::new(1)), Some(SpaceId::new(2))], &[
                Some("display-a".to_string()),
                Some("display-a".to_string()),
            ]);

        assert_eq!(active, vec![None, Some(SpaceId::new(2))]);
    }

    #[test]
    fn enabled_display_allows_space_when_default_disabled() {
        let mut policy = SpaceActivationPolicy::new();
        let cfg = SpaceActivationConfig {
            default_disable: true,
            one_space: false,
        };

        policy.on_spaces_updated(cfg, &[input(1, Some(1), Some("display-a"))]);
        policy.toggle_space_activated(cfg, ToggleSpaceContext {
            space: SpaceId::new(1),
            display_uuid: Some("display-a".to_string()),
        });

        let active = policy
            .compute_active_spaces(cfg, &[Some(SpaceId::new(1))], &[Some("display-a".to_string())]);

        assert_eq!(active, vec![Some(SpaceId::new(1))]);
    }

    #[test]
    fn enabled_display_applies_to_other_spaces_on_same_display() {
        let mut policy = SpaceActivationPolicy::new();
        let cfg = SpaceActivationConfig {
            default_disable: true,
            one_space: false,
        };

        policy.on_spaces_updated(cfg, &[input(1, Some(1), Some("display-a"))]);
        policy.toggle_space_activated(cfg, ToggleSpaceContext {
            space: SpaceId::new(1),
            display_uuid: Some("display-a".to_string()),
        });

        let active = policy
            .compute_active_spaces(cfg, &[Some(SpaceId::new(2))], &[Some("display-a".to_string())]);

        assert_eq!(active, vec![Some(SpaceId::new(2))]);
    }

    #[test]
    fn login_window_disables_all_spaces() {
        let mut policy = SpaceActivationPolicy::new();
        policy.set_login_window_active(true);
        let cfg = SpaceActivationConfig {
            default_disable: false,
            one_space: false,
        };

        policy.on_spaces_updated(cfg, &[input(1, Some(1), Some("display-a"))]);
        let active = policy
            .compute_active_spaces(cfg, &[Some(SpaceId::new(1))], &[Some("display-a".to_string())]);

        assert_eq!(active, vec![None]);
    }

    #[test]
    fn disabled_space_persists_across_space_switches_default_enable() {
        let mut policy = SpaceActivationPolicy::new();
        let cfg = SpaceActivationConfig {
            default_disable: false,
            one_space: false,
        };

        policy.on_spaces_updated(cfg, &[input(1, Some(1), Some("display-a"))]);
        policy.toggle_space_activated(cfg, ToggleSpaceContext {
            space: SpaceId::new(1),
            display_uuid: Some("display-a".to_string()),
        });

        policy.on_spaces_updated(cfg, &[input(1, Some(2), Some("display-a"))]);
        let active = policy
            .compute_active_spaces(cfg, &[Some(SpaceId::new(2))], &[Some("display-a".to_string())]);
        assert_eq!(active, vec![Some(SpaceId::new(2))]);

        policy.on_spaces_updated(cfg, &[input(1, Some(1), Some("display-a"))]);
        let active = policy
            .compute_active_spaces(cfg, &[Some(SpaceId::new(1))], &[Some("display-a".to_string())]);
        assert_eq!(active, vec![None]);
    }

    #[test]
    fn transfer_space_activation_on_space_id_churn_default_disable() {
        let mut policy = SpaceActivationPolicy::new();
        let cfg = SpaceActivationConfig {
            default_disable: true,
            one_space: false,
        };

        policy.on_spaces_updated(cfg, &[input(1, Some(1), Some("display-a"))]);
        policy.toggle_space_activated(cfg, ToggleSpaceContext {
            space: SpaceId::new(1),
            display_uuid: Some("display-a".to_string()),
        });

        policy.on_space_destroyed(SpaceId::new(1));
        policy.on_spaces_updated(cfg, &[input(1, Some(2), Some("display-a"))]);

        assert!(!policy.enabled_spaces.contains(&SpaceId::new(1)));
        assert!(policy.enabled_spaces.contains(&SpaceId::new(2)));
    }

    #[test]
    fn transfer_space_activation_on_space_id_churn_default_enable() {
        let mut policy = SpaceActivationPolicy::new();
        let cfg = SpaceActivationConfig {
            default_disable: false,
            one_space: false,
        };

        policy.on_spaces_updated(cfg, &[input(1, Some(1), Some("display-a"))]);
        policy.toggle_space_activated(cfg, ToggleSpaceContext {
            space: SpaceId::new(1),
            display_uuid: Some("display-a".to_string()),
        });

        policy.on_space_destroyed(SpaceId::new(1));
        policy.on_spaces_updated(cfg, &[input(1, Some(2), Some("display-a"))]);

        assert!(!policy.disabled_spaces.contains(&SpaceId::new(1)));
        assert!(policy.disabled_spaces.contains(&SpaceId::new(2)));
    }

    #[test]
    fn starting_space_clears_when_missing() {
        let mut policy = SpaceActivationPolicy::new();
        let cfg = SpaceActivationConfig {
            default_disable: false,
            one_space: true,
        };

        policy.on_spaces_updated(cfg, &[input(1, Some(1), Some("display-a"))]);
        assert_eq!(policy.starting_space, Some(SpaceId::new(1)));

        policy.on_spaces_updated(cfg, &[input(1, Some(2), Some("display-a"))]);
        assert_eq!(policy.starting_space, Some(SpaceId::new(2)));
    }

    #[test]
    fn prune_display_state_when_screen_removed() {
        let mut policy = SpaceActivationPolicy::new();
        let cfg = SpaceActivationConfig {
            default_disable: true,
            one_space: false,
        };

        policy.on_spaces_updated(cfg, &[
            input(1, Some(1), Some("display-a")),
            input(2, Some(2), Some("display-b")),
        ]);
        policy.toggle_space_activated(cfg, ToggleSpaceContext {
            space: SpaceId::new(1),
            display_uuid: Some("display-a".to_string()),
        });
        policy.toggle_space_activated(cfg, ToggleSpaceContext {
            space: SpaceId::new(2),
            display_uuid: Some("display-b".to_string()),
        });

        policy.on_spaces_updated(cfg, &[input(1, Some(1), Some("display-a"))]);

        assert!(policy.enabled_displays.contains("display-a"));
        assert!(!policy.enabled_displays.contains("display-b"));
    }

    #[test]
    fn missing_space_values_are_ignored() {
        let mut policy = SpaceActivationPolicy::new();
        let cfg = SpaceActivationConfig {
            default_disable: false,
            one_space: false,
        };

        policy.on_spaces_updated(cfg, &[input(1, None, Some("display-a"))]);
        let active = policy.compute_active_spaces(cfg, &[None], &[Some("display-a".to_string())]);

        assert_eq!(active, vec![None]);
    }
}
