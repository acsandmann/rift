use objc2_core_foundation::CGSize;
use serde::{Deserialize, Serialize};

use super::{LayoutId, LayoutSystem};
use crate::common::collections::HashMap;

/// Display-size configurations belong to their workspace; layout systems own the arrangements.
#[derive(Clone, Serialize, Deserialize, Debug, Default)]
pub(crate) struct WorkspaceLayoutState {
    pub(crate) configurations: HashMap<Size, LayoutId>,
    active_size: Size,
    #[serde(default, rename = "last_saved", skip_serializing)]
    legacy_last_saved: Option<LayoutId>,
}

#[derive(
    Serialize, Deserialize, Default, Clone, Copy, Eq, PartialEq, Hash, Ord, PartialOrd, Debug,
)]
pub(crate) struct Size {
    width: i32,
    height: i32,
}

impl From<CGSize> for Size {
    fn from(value: CGSize) -> Self {
        Self {
            width: value.width.round() as i32,
            height: value.height.round() as i32,
        }
    }
}

impl WorkspaceLayoutState {
    pub(crate) fn active(&self) -> Option<LayoutId> {
        self.configurations.get(&self.active_size).copied()
    }

    pub(crate) fn active_size(&self) -> Option<CGSize> {
        self.active()
            .map(|_| CGSize::new(self.active_size.width.into(), self.active_size.height.into()))
    }

    pub(crate) fn all_layouts(&self) -> impl Iterator<Item = LayoutId> + '_ {
        self.configurations.values().copied().chain(self.legacy_last_saved)
    }

    pub(crate) fn ensure_active(&mut self, size: CGSize, tree: &mut impl LayoutSystem) {
        let size = Size::from(size);
        if self.active_size == size
            && self.configurations.len() == 1
            && self.legacy_last_saved.is_none()
        {
            return;
        }
        let layout =
            self.active().or(self.legacy_last_saved).unwrap_or_else(|| tree.create_layout());
        // One logical arrangement follows the workspace between viewports. Old snapshots
        // can contain divergent per-size trees; their active tree is authoritative.
        for old in self.all_layouts().collect::<std::collections::BTreeSet<_>>() {
            if old != layout {
                tree.remove_layout(old);
            }
        }
        self.configurations.clear();
        self.active_size = size;
        self.configurations.insert(size, layout);
        self.legacy_last_saved = None;
    }

    pub(crate) fn replace(&mut self, layout: LayoutId) {
        if self.configurations.is_empty() {
            self.active_size = Size::from(CGSize::new(1000.0, 1000.0));
        }
        self.configurations.clear();
        self.configurations.insert(self.active_size, layout);
        self.legacy_last_saved = None;
    }

    pub(crate) fn validate(&self, tree: &impl LayoutSystem) -> Result<(), String> {
        if self.configurations.is_empty() && self.legacy_last_saved.is_none() {
            // Never shown yet; `ensure_active` creates the first layout when it is.
            return Ok(());
        }
        if self.configurations.is_empty() {
            return Err("no layout configurations".into());
        }
        if self.active().is_none() {
            return Err("no configuration for its active display size".into());
        }
        for layout in self.all_layouts() {
            if !tree.contains_layout(layout) {
                return Err(format!("references missing layout {layout:?}"));
            }
        }
        Ok(())
    }
}
