use objc2_core_foundation::CGSize;
use serde::{Deserialize, Serialize};

use super::{LayoutId, LayoutSystem};
use crate::common::collections::HashMap;

/// One logical arrangement follows its workspace between viewports.
#[derive(Clone, Serialize, Debug, Default)]
pub(crate) struct WorkspaceLayoutState {
    layout: Option<LayoutId>,
    active_size: Size,
    // Only populated while migrating an old multi-size snapshot. Retire these
    // trees once the owning layout system is available.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    obsolete: Vec<LayoutId>,
}

impl<'de> Deserialize<'de> for WorkspaceLayoutState {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Snapshot {
            #[serde(default)]
            layout: Option<LayoutId>,
            active_size: Size,
            #[serde(default)]
            configurations: HashMap<Size, LayoutId>,
            #[serde(default, rename = "last_saved")]
            legacy: Option<LayoutId>,
            #[serde(default)]
            obsolete: Vec<LayoutId>,
        }
        let saved = Snapshot::deserialize(deserializer)?;
        if saved.layout.is_none()
            && !saved.configurations.is_empty()
            && !saved.configurations.contains_key(&saved.active_size)
        {
            return Err(serde::de::Error::custom(
                "no configuration for its active display size",
            ));
        }
        let layout = saved
            .layout
            .or_else(|| saved.configurations.get(&saved.active_size).copied())
            .or(saved.legacy);
        let mut obsolete: Vec<_> = saved
            .configurations
            .into_values()
            .chain(saved.legacy)
            .chain(saved.obsolete)
            .filter(|id| Some(*id) != layout)
            .collect();
        obsolete.sort_unstable();
        obsolete.dedup();
        Ok(Self {
            layout,
            active_size: saved.active_size,
            obsolete,
        })
    }
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
    pub(crate) fn active(&self) -> Option<LayoutId> { self.layout }

    pub(crate) fn active_size(&self) -> Option<CGSize> {
        self.layout
            .map(|_| CGSize::new(self.active_size.width.into(), self.active_size.height.into()))
    }

    pub(crate) fn all_layouts(&self) -> impl Iterator<Item = LayoutId> + '_ {
        self.layout.into_iter().chain(self.obsolete.iter().copied())
    }

    pub(crate) fn ensure_active(&mut self, size: CGSize, tree: &mut impl LayoutSystem) {
        for old in self.obsolete.drain(..) {
            tree.remove_layout(old);
        }
        self.layout.get_or_insert_with(|| tree.create_layout());
        self.active_size = size.into();
    }

    pub(crate) fn replace(&mut self, layout: LayoutId) {
        if self.layout.is_none() {
            self.active_size = Size::from(CGSize::new(1000.0, 1000.0));
        }
        self.layout = Some(layout);
        self.obsolete.clear();
    }

    pub(crate) fn validate(&self, tree: &impl LayoutSystem) -> Result<(), String> {
        for layout in self.all_layouts() {
            if !tree.contains_layout(layout) {
                return Err(format!("references missing layout {layout:?}"));
            }
        }
        Ok(())
    }
}
