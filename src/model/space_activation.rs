use crate::common::collections::HashSet;
use crate::sys::screen::{ScreenInfo, SpaceId};

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

    pub login_window_active: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct SpaceActivationConfig {
    pub default_disable: bool,
    pub one_space: bool,
}

#[derive(Debug, Clone)]
pub struct ToggleSpaceContext {
    pub space: SpaceId,
    pub display_uuid: Option<String>,
}

impl SpaceActivationPolicy {
    pub fn new() -> Self { Self::default() }

    pub fn set_login_window_active(&mut self, active: bool) { self.login_window_active = active; }

    /// Note: this emits no events; Reactor should call this and then recompute active spaces.
    pub fn on_spaces_updated(&mut self, cfg: SpaceActivationConfig, screens: &[ScreenInfo]) {
        // Display preferences belong to durable UUIDs, never a reusable live ScreenId.
        // Native Space identity is transferred only by an authoritative remap.
        for screen in screens {
            let Some(space) = screen.space else { continue };
            let Some(display_uuid) = screen.display_uuid_opt() else {
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
            if !screens.iter().any(|screen| screen.space == Some(starting)) {
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

    pub fn on_space_remapped(&mut self, old_space: SpaceId, new_space: SpaceId) {
        if self.enabled_spaces.remove(&old_space) {
            self.enabled_spaces.insert(new_space);
        }
        if self.disabled_spaces.remove(&old_space) {
            self.disabled_spaces.insert(new_space);
        }
        if self.starting_space == Some(old_space) {
            self.starting_space = Some(new_space);
        }
    }
}

#[cfg(test)]
mod tests {
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};

    use super::*;
    use crate::sys::screen::ScreenId;

    fn input(screen_id: u32, space: Option<u64>, display_uuid: Option<&str>) -> ScreenInfo {
        ScreenInfo {
            is_builtin: false,
            backing_scale: 1.0,
            id: ScreenId::new(screen_id),
            frame: CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(0.0, 0.0)),
            display_uuid: display_uuid.unwrap_or_default().to_string(),
            name: None,
            space: space.map(SpaceId::new),
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
    fn missing_uuid_preserves_activation_for_both_default_policies() {
        for default_disable in [false, true] {
            let mut policy = SpaceActivationPolicy::new();
            let cfg = SpaceActivationConfig {
                default_disable,
                one_space: false,
            };
            let space = SpaceId::new(1);
            policy.on_spaces_updated(cfg, &[input(1, Some(1), Some("display-a"))]);
            policy.toggle_space_activated(cfg, ToggleSpaceContext {
                space,
                display_uuid: Some("display-a".into()),
            });
            for uuid in [None, Some("display-a")] {
                policy.on_spaces_updated(cfg, &[input(1, Some(1), uuid)]);
                assert_eq!(
                    policy.compute_active_spaces(cfg, &[Some(space)], &[uuid.map(str::to_owned)]),
                    vec![default_disable.then_some(space)],
                    "default_disable={default_disable}, uuid={uuid:?}"
                );
            }
        }
    }

    #[test]
    fn reused_screen_id_does_not_transfer_display_or_space_preferences() {
        for default_disable in [false, true] {
            let mut policy = SpaceActivationPolicy::new();
            let cfg = SpaceActivationConfig {
                default_disable,
                one_space: false,
            };
            let old = SpaceId::new(1);
            let new = SpaceId::new(2);
            policy.on_spaces_updated(cfg, &[input(1, Some(1), Some("display-a"))]);
            policy.toggle_space_activated(cfg, ToggleSpaceContext {
                space: old,
                display_uuid: Some("display-a".into()),
            });
            policy.on_spaces_updated(cfg, &[input(1, Some(2), Some("display-b"))]);
            assert_eq!(
                policy.compute_active_spaces(cfg, &[Some(new)], &[Some("display-b".into())]),
                vec![(!default_disable).then_some(new)]
            );
            policy.on_spaces_updated(cfg, &[input(9, Some(1), Some("display-a"))]);
            assert_eq!(
                policy.compute_active_spaces(cfg, &[Some(old)], &[Some("display-a".into())]),
                vec![default_disable.then_some(old)]
            );
        }
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
    fn enabled_display_applies_to_its_current_and_other_native_spaces_only() {
        let mut policy = SpaceActivationPolicy::new();
        let cfg = SpaceActivationConfig {
            default_disable: true,
            one_space: false,
        };
        policy.on_spaces_updated(cfg, &[input(1, Some(1), Some("display-a"))]);
        policy.toggle_space_activated(cfg, ToggleSpaceContext {
            space: SpaceId::new(1),
            display_uuid: Some("display-a".into()),
        });
        assert_eq!(
            policy.compute_active_spaces(
                cfg,
                &[
                    Some(SpaceId::new(1)),
                    Some(SpaceId::new(2)),
                    Some(SpaceId::new(3))
                ],
                &[
                    Some("display-a".into()),
                    Some("display-a".into()),
                    Some("display-b".into())
                ]
            ),
            vec![Some(SpaceId::new(1)), Some(SpaceId::new(2)), None]
        );
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
    fn native_space_replacement_transfers_activation_for_both_default_policies() {
        for default_disable in [false, true] {
            let mut policy = SpaceActivationPolicy::new();
            let cfg = SpaceActivationConfig {
                default_disable,
                one_space: false,
            };
            let old = SpaceId::new(1);
            let new = SpaceId::new(2);
            policy.on_spaces_updated(cfg, &[input(1, Some(1), Some("display-a"))]);
            // No display preference may mask whether the native-space state transfers.
            policy
                .toggle_space_activated(cfg, ToggleSpaceContext { space: old, display_uuid: None });
            policy.on_space_remapped(old, new);
            policy.on_spaces_updated(cfg, &[input(1, Some(2), Some("display-a"))]);
            assert_eq!(
                policy.compute_active_spaces(cfg, &[Some(old), Some(new)], &[None, None]),
                vec![
                    (!default_disable).then_some(old),
                    default_disable.then_some(new)
                ],
                "default_disable={default_disable}"
            );
        }
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
    fn display_activation_survives_disconnect_and_reconnect_with_new_native_ids() {
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

        policy.on_spaces_updated(cfg, &[
            input(1, Some(1), Some("display-a")),
            input(4, Some(40), Some("display-b")),
            input(2, Some(50), Some("projector")),
        ]);
        assert_eq!(
            policy.compute_active_spaces(
                cfg,
                &[
                    Some(SpaceId::new(1)),
                    Some(SpaceId::new(40)),
                    Some(SpaceId::new(50))
                ],
                &[
                    Some("display-a".into()),
                    Some("display-b".into()),
                    Some("projector".into())
                ]
            ),
            vec![Some(SpaceId::new(1)), Some(SpaceId::new(40)), None]
        );
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
