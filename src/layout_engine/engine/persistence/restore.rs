use super::matcher::MatchScope;
use super::snapshot::PersistedLayout;
use super::*;

impl PersistedLayout {
    /// The file's entry for a live Space. The disk master can still contain the previous launch's
    /// SpaceIds even though startup already repaired the live engine; display identity bridges
    /// that interval until the next master save.
    fn entry_for_space(&self, engine: &LayoutEngine, space: SpaceId) -> Option<SpaceId> {
        let saved_spaces = self.workspaces.layout_spaces();
        Some(space).filter(|space| saved_spaces.contains(space)).or_else(|| {
            engine
                .display_uuid_for_space(space)
                .and_then(|display| self.display_last_space.get(display).copied())
                .filter(|space| saved_spaces.contains(space))
        })
    }

    /// Pair saved native Spaces with the live ones they restore into. A display the file has no
    /// entry for is left alone, and one saved Space restores to at most one display.
    fn space_pairs(
        &self,
        engine: &LayoutEngine,
        request: RestoreRequest,
        visible_spaces: &[SpaceId],
    ) -> anyhow::Result<Vec<(SpaceId, SpaceId)>> {
        if request.scope != RestoreScope::Displays {
            return Ok(vec![(self.source_space(engine, request)?, request.active_space)]);
        }
        let mut pairs: Vec<(SpaceId, SpaceId)> = Vec::new();
        for &space in visible_spaces {
            if let Some(source) = self.entry_for_space(engine, space)
                && !pairs.iter().any(|(claimed, _)| *claimed == source)
            {
                pairs.push((source, space));
            }
        }
        // A shared catalog pairs by position, so only its active workspaces depend on these pairs.
        if pairs.is_empty() && engine.workspaces.settings.mode != WorkspaceMode::Global {
            anyhow::bail!("saved layout has no entry for any connected display");
        }
        Ok(pairs)
    }

    /// A shared catalog follows workspaces between displays, so ordinals pair across all of it.
    fn catalog_mappings(
        &self,
        engine: &LayoutEngine,
    ) -> anyhow::Result<Vec<(VirtualWorkspaceId, VirtualWorkspaceId)>> {
        let sources: Vec<_> = self.workspaces.catalog().collect();
        let targets: Vec<_> = engine.workspaces.catalog().collect();
        if sources.len() != targets.len() {
            anyhow::bail!("saved and current catalogs have different workspace counts");
        }
        Ok(sources
            .into_iter()
            .zip(targets)
            .filter(|(source, _)| self.workspaces[*source].active_layout().is_some())
            .collect())
    }

    fn source_space(
        &self,
        engine: &LayoutEngine,
        request: RestoreRequest,
    ) -> anyhow::Result<SpaceId> {
        let saved_spaces = self.workspaces.layout_spaces();
        let preferred = match request.source {
            RestoreSource::SavedActiveSpace => (self.saved_active_space())
                .filter(|space| saved_spaces.contains(space))
                .or(saved_spaces.contains(&request.active_space).then_some(request.active_space)),
            RestoreSource::CurrentSpace => self.entry_for_space(engine, request.active_space),
        };

        if let Some(space) = preferred {
            return Ok(space);
        }
        if saved_spaces.is_empty() {
            Err(anyhow::anyhow!("saved layout contains no macOS spaces"))
        } else if saved_spaces.len() == 1 {
            Ok(*saved_spaces.first().expect("one saved space"))
        } else {
            Err(anyhow::anyhow!(
                "cannot choose a source from {} saved macOS spaces; save the layout again to record its active space",
                saved_spaces.len()
            ))
        }
    }

    /// Pair saved workspaces with the live ones they replace. This is the transaction boundary:
    /// every fallible check belongs here, and nothing here mutates anything.
    fn workspace_mappings(
        &self,
        engine: &LayoutEngine,
        request: RestoreRequest,
        source_space: SpaceId,
    ) -> anyhow::Result<Vec<(VirtualWorkspaceId, VirtualWorkspaceId)>> {
        let sources = self.workspaces.workspace_ids(source_space);
        let targets = engine.workspaces.workspace_ids(request.active_space);
        let mappings: Vec<_> = match request.scope {
            RestoreScope::Space | RestoreScope::Displays => {
                // A space restore is all-or-nothing. `zip` silently truncates, which would split
                // layout ownership between the snapshot and the pre-restore state.
                if sources.len() != targets.len() {
                    return Err(anyhow::anyhow!(
                        "saved and current spaces have different workspace counts"
                    ));
                }
                sources.into_iter().zip(targets).collect()
            }
            RestoreScope::Workspace => {
                let target = engine
                    .workspaces
                    .active_workspace(request.active_space)
                    .ok_or_else(|| anyhow::anyhow!("current space has no active workspace"))?;
                let source = match request.source {
                    // A portable workspace file represents what was active when it was saved.
                    RestoreSource::SavedActiveSpace => self
                        .workspaces
                        .active_workspace(source_space)
                        .ok_or_else(|| anyhow::anyhow!("saved space has no active workspace"))?,
                    // A master file represents the complete workspace catalog. Restoring S must
                    // therefore read S's ordinal from the saved native space, regardless of which
                    // workspace happened to be active when Rift last quit.
                    RestoreSource::CurrentSpace => {
                        let index =
                            targets.iter().position(|id| *id == target).ok_or_else(|| {
                                anyhow::anyhow!(
                                    "current active workspace is missing from its native space"
                                )
                            })?;
                        sources.get(index).copied().ok_or_else(|| {
                            anyhow::anyhow!("saved space has no workspace at target index {index}")
                        })?
                    }
                };
                vec![(source, target)]
            }
        };
        if mappings
            .iter()
            .any(|(source, _)| self.workspaces.workspaces[*source].active_layout().is_none())
        {
            return Err(anyhow::anyhow!("saved workspace layout is incomplete"));
        }
        Ok(mappings)
    }
}

impl LayoutEngine {
    /// Everything fallible happens before the first mutation. After that, a window absent from the
    /// file keeps its assignment and floating state, a saved window with no live match leaves no
    /// trace, and anything outside the mapped workspaces is not touched.
    pub fn restore_layout(
        &mut self,
        path: PathBuf,
        request: RestoreRequest,
        visible_spaces: &[SpaceId],
        window_store: &mut WindowStore,
        layout_settings: &LayoutSettings,
    ) -> anyhow::Result<RestoreReport> {
        let mut snapshot = PersistedLayout::read(&path)?;
        let pairs = snapshot.space_pairs(self, request, visible_spaces)?;
        let mappings = if request.scope == RestoreScope::Displays
            && self.workspaces.settings.mode == WorkspaceMode::Global
        {
            snapshot.catalog_mappings(self)?
        } else {
            let mut mappings = Vec::new();
            for &(source, active_space) in &pairs {
                let request = RestoreRequest { active_space, ..request };
                mappings.extend(snapshot.workspace_mappings(self, request, source)?);
            }
            mappings
        };
        let actives: Vec<_> = (pairs.iter())
            .filter_map(|(source, space)| {
                let source_active = snapshot.workspaces.active_workspace(*source)?;
                let (_, active) = mappings.iter().find(|(source, _)| *source == source_active)?;
                Some((*space, *active))
            })
            .collect();
        let targets: Vec<_> = mappings.iter().map(|(_, target)| *target).collect();
        // Candidates still waiting in a replaced workspace lose their slots with its tree.
        self.persistence.pending.retain(|_, saved| !targets.contains(&saved.workspace));
        for &(source, target) in &mappings {
            let saved = snapshot
                .workspaces
                .workspaces
                .remove(source)
                .expect("workspace sources were validated before installation");
            self.native_layouts.remove(&target);
            let display = self.space_display_map.get(&self.workspaces[target].space);
            let display = display.and_then(Option::as_deref);
            let workspace = &mut self.workspaces[target];
            workspace.restore_contents(saved);
            crate::layout_engine::engine::apply_workspace_layout_settings(
                workspace,
                layout_settings,
                display,
            );
        }
        if request.scope != RestoreScope::Workspace {
            for (space, active) in actives {
                if self.workspaces[active].space == space {
                    self.workspaces.set_active_workspace(space, active);
                }
            }
        }

        let mut scoped = 0usize;
        for (window, mut saved) in snapshot.pending {
            let Some((_, target)) = mappings.iter().find(|(source, _)| *source == saved.workspace)
            else {
                continue;
            };
            saved.workspace = *target;
            scoped += 1;
            // A process-local id can collide with a candidate that startup left pending elsewhere.
            if let Some(previous) = self.persistence.pending.insert(window, saved) {
                self.discard_saved_window(window, previous);
            }
        }

        let mut report = RestoreReport {
            workspaces_replaced: mappings.len(),
            ..RestoreReport::default()
        };
        let scope = MatchScope::Workspaces(&targets);
        // Listed up front: a match moves a window into a later target, and it must not be seen
        // again there, where it would take the slot of a same-titled window.
        let live_windows: Vec<_> = (targets.iter())
            .flat_map(|target| {
                window_store.workspace_windows(*target).into_iter().map(|live| (*target, live))
            })
            .collect();
        for (target, live) in live_windows {
            let Some(window) = window_store.window(live) else {
                continue;
            };
            let frame = window.frame_monotonic;
            let evidence = WindowFingerprint::of(live, window);
            let matched = self.reconcile_saved_window(window_store, live, &evidence, scope);
            report.matched += usize::from(matched);
            if !matched && self.floating.is_floating(live) {
                self.floating_positions.store_if_absent(target, live, frame);
            }
            // Rebuild the projection for both outcomes: a matched floating window needs its
            // runtime-only active entry, and a current window absent from the file needs a slot
            // in the replaced tree.
            let owner = window_store.workspace_for_window(live).unwrap_or(target);
            let (from, space) = (self.workspaces[target].space, self.workspaces[owner].space);
            if from != space {
                report.moved.push((live, from, space));
            }
            self.add_window_to_layout(window_store, space, live);
            if self.focused_window == Some(live) {
                self.workspaces.set_last_focused_window(space, owner, Some(live));
            }
        }
        let unmatched: Vec<_> = (self.persistence.pending.iter())
            .filter(|(_, saved)| targets.contains(&saved.workspace))
            .map(|(window, _)| *window)
            .collect();
        self.discard_saved_windows(unmatched);
        report.unmatched = scoped.saturating_sub(report.matched);
        tracing::info!(path = %path.display(), ?report, "Persisted layout restore completed");
        Ok(report)
    }
}
