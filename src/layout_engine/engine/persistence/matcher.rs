use super::*;

pub(super) type WorkspaceLocation = (SpaceId, VirtualWorkspaceId);

pub(super) struct RestoreCandidate<'a> {
    pub(super) window: WindowId,
    pub(super) fingerprint: &'a WindowFingerprint,
    pub(super) location: Option<WorkspaceLocation>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct MatchDecision {
    pub(super) selected: WindowId,
    pub(super) exact_identity: bool,
    pub(super) duplicate_identities: Vec<WindowId>,
}

/// Select a restoration candidate without mutating engine state.
///
/// Keeping ranking pure makes matching deterministic and prevents a rejected low-confidence
/// candidate from partially changing trees, floating state, or pending identities.
pub(super) fn choose_match(
    live: WindowId,
    live_space: SpaceId,
    fingerprint: &WindowFingerprint,
    preferred_location: Option<WorkspaceLocation>,
    candidates: &[RestoreCandidate<'_>],
) -> Option<MatchDecision> {
    // WindowId is process-local, so it is direct evidence only while stronger saved identity does
    // not contradict it. If WindowServer identity disagrees, let the genuine server-id candidate
    // win rather than trusting an id that may have been reused since the file was written.
    let direct = candidates.iter().find(|candidate| {
        candidate.window == live
            && candidate.fingerprint.direct_identity_compatible_with(fingerprint)
    });
    let server_id_match =
        direct.is_none().then(|| fingerprint.window_server_id).flatten().and_then(
            |window_server_id| {
                candidates
                    .iter()
                    .filter(|candidate| {
                        candidate.fingerprint.window_server_id == Some(window_server_id)
                            && candidate.fingerprint.server_identity_compatible_with(fingerprint)
                    })
                    .max_by(|a, b| {
                        let rank = |candidate: &RestoreCandidate<'_>| {
                            (
                                candidate.window == live,
                                candidate.location == preferred_location,
                                candidate.location.is_some_and(|(space, _)| space == live_space),
                            )
                        };
                        rank(a).cmp(&rank(b)).then_with(|| b.window.cmp(&a.window))
                    })
                    .map(|candidate| candidate.window)
            },
        );

    let exact_identity = direct.is_some() || server_id_match.is_some();
    let selected = direct
        .map(|candidate| candidate.window)
        .or(server_id_match)
        .or_else(|| choose_fallback(fingerprint, candidates))?;

    let mut duplicate_identities = if direct.is_none() && server_id_match.is_some() {
        fingerprint.window_server_id.map_or_else(Vec::new, |window_server_id| {
            candidates
                .iter()
                .filter(|candidate| {
                    candidate.window != selected
                        && candidate.fingerprint.window_server_id == Some(window_server_id)
                })
                .map(|candidate| candidate.window)
                .collect()
        })
    } else {
        Vec::new()
    };
    duplicate_identities.sort_unstable();

    Some(MatchDecision {
        selected,
        exact_identity,
        duplicate_identities,
    })
}

fn choose_fallback(
    live: &WindowFingerprint,
    candidates: &[RestoreCandidate<'_>],
) -> Option<WindowId> {
    // A known bundle id plus a non-empty title identifies a restarted application's window when
    // that pair occurs only once. Size is layout output, so it must not veto that association.
    let matching: Vec<_> = candidates
        .iter()
        .filter(|candidate| {
            candidate.fingerprint.app_id.is_some()
                && candidate.fingerprint.app_id == live.app_id
                && candidate.fingerprint.title.is_some()
                && candidate.fingerprint.title == live.title
        })
        .collect();
    if matching.len() == 1 {
        return Some(matching[0].window);
    }

    // Duplicate titles do occur within an application. In that case size is useful only as a
    // disambiguation signal: accept the uniquely closest saved frame and reject an equal-distance
    // tie instead of assigning a saved slot arbitrarily.
    let mut closest = None;
    let mut closest_delta = f64::INFINITY;
    let mut tied = false;
    for candidate in matching {
        let delta = (candidate.fingerprint.width - live.width).abs()
            + (candidate.fingerprint.height - live.height).abs();
        if delta < closest_delta {
            closest = Some(candidate.window);
            closest_delta = delta;
            tied = false;
        } else if delta == closest_delta {
            tied = true;
        }
    }
    (!tied).then_some(closest).flatten()
}
