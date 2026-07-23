use std::cmp::Ordering;

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
    // A direct Rift window identity owns exactly one restoration candidate. Never let a reused
    // WindowServer id, title, or size score redirect it to another saved window's position.
    let direct = candidates.iter().find(|candidate| candidate.window == live);
    let server_id_match =
        direct.is_none().then(|| fingerprint.window_server_id).flatten().and_then(
            |window_server_id| {
                candidates
                    .iter()
                    .filter(|candidate| {
                        candidate.fingerprint.window_server_id == Some(window_server_id)
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
    let selected = direct.map(|candidate| candidate.window).or(server_id_match).or_else(|| {
        let compatible = |candidate: &&RestoreCandidate<'_>| {
            candidate.fingerprint.app_compatible_with(fingerprint)
        };
        let same_space: Vec<_> = candidates
            .iter()
            .filter(compatible)
            .filter(|candidate| candidate.location.is_none_or(|(space, _)| space == live_space))
            .collect();
        let pool = if same_space.is_empty() {
            candidates.iter().filter(compatible).collect::<Vec<_>>()
        } else {
            same_space
        };
        pool.into_iter()
            .max_by(|a, b| compare_fallback(a, b, fingerprint))
            .map(|candidate| candidate.window)
    })?;

    let selected_fingerprint =
        candidates.iter().find(|candidate| candidate.window == selected)?.fingerprint;
    let title_matches =
        selected_fingerprint.title.is_some() && selected_fingerprint.title == fingerprint.title;
    let size_delta = (selected_fingerprint.width - fingerprint.width).abs()
        + (selected_fingerprint.height - fingerprint.height).abs();
    // Bundle identity narrows the search pool but does not identify a particular window. Require
    // window-specific evidence as well, otherwise the first new window from an application can
    // consume any unrelated saved spot from that same application.
    if !exact_identity && !title_matches && size_delta > 8.0 {
        return None;
    }

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

fn compare_fallback(
    a: &RestoreCandidate<'_>,
    b: &RestoreCandidate<'_>,
    live: &WindowFingerprint,
) -> Ordering {
    let score = |saved: &WindowFingerprint| {
        let title = (saved.title.is_some() && saved.title == live.title) as u8;
        let size_delta = (saved.width - live.width).abs() + (saved.height - live.height).abs();
        let app = (saved.app_id.is_some() && saved.app_id == live.app_id) as u8;
        (title, size_delta, app)
    };
    let (title_a, size_a, app_a) = score(a.fingerprint);
    let (title_b, size_b, app_b) = score(b.fingerprint);
    title_a
        .cmp(&title_b)
        .then_with(|| size_b.partial_cmp(&size_a).unwrap_or(Ordering::Equal))
        .then_with(|| app_a.cmp(&app_b))
        .then_with(|| b.window.cmp(&a.window))
}
