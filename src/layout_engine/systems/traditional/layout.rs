use objc2_core_foundation::CGRect;
use serde::{Deserialize, Serialize};

use super::stack::StackLayoutResult;
use super::window::Window;
use crate::actor::app::WindowId;
use crate::layout_engine::LayoutKind;
use crate::model::selection::TreeEvent;
use crate::model::tree::{NodeId, NodeMap};
use crate::sys::geometry::Round;

#[derive(Default, Serialize, Deserialize)]
pub(crate) struct Layout {
    pub(crate) info: slotmap::SecondaryMap<NodeId, LayoutInfo>,
}

#[allow(unused)]
#[derive(Default, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct LayoutInfo {
    pub(crate) size: f32,
    pub(crate) total: f32,
    kind: LayoutKind,
    last_ungrouped_kind: LayoutKind,
    #[serde(default)]
    is_fullscreen: bool,
}

impl Layout {
    pub(crate) fn handle_event(&mut self, map: &NodeMap, event: TreeEvent) {
        match event {
            TreeEvent::AddedToForest(node) => {
                self.info.insert(node, LayoutInfo::default());
            }
            TreeEvent::AddedToParent(node) => {
                let parent = node.parent(map).unwrap();
                self.info[node].size = 1.0;
                self.info[parent].total += 1.0;
            }
            TreeEvent::Copied { src, dest, .. } => {
                self.info.insert(dest, self.info[src].clone());
            }
            TreeEvent::RemovingFromParent(node) => {
                self.info[node.parent(map).unwrap()].total -= self.info[node].size;
            }
            TreeEvent::RemovedFromForest(node) => {
                self.info.remove(node);
            }
        }
    }

    pub(crate) fn assume_size_of(&mut self, new: NodeId, old: NodeId, map: &NodeMap) {
        assert_eq!(new.parent(map), old.parent(map));
        let parent = new.parent(map).unwrap();
        self.info[parent].total -= self.info[new].size;
        self.info[new].size = core::mem::replace(&mut self.info[old].size, 0.0);
    }

    pub(crate) fn set_kind(&mut self, node: NodeId, kind: LayoutKind) {
        self.info[node].kind = kind;
        if !kind.is_group() {
            self.info[node].last_ungrouped_kind = kind;
        }
    }

    pub(crate) fn kind(&self, node: NodeId) -> LayoutKind { self.info[node].kind }

    pub(crate) fn proportion(&self, map: &NodeMap, node: NodeId) -> Option<f64> {
        let Some(parent) = node.parent(map) else { return None };
        Some(f64::from(self.info[node].size) / f64::from(self.info[parent].total))
    }

    pub(crate) fn take_share(&mut self, map: &NodeMap, node: NodeId, from: NodeId, share: f32) {
        assert_eq!(node.parent(map), from.parent(map));
        let share = share.min(self.info[from].size);
        let share = share.max(-self.info[node].size);
        self.info[from].size -= share;
        self.info[node].size += share;
    }

    pub(crate) fn set_fullscreen(&mut self, node: NodeId, is_fullscreen: bool) {
        self.info[node].is_fullscreen = is_fullscreen;
    }

    pub(crate) fn toggle_fullscreen(&mut self, node: NodeId) -> bool {
        self.info[node].is_fullscreen = !self.info[node].is_fullscreen;
        self.info[node].is_fullscreen
    }

    pub(crate) fn debug(&self, node: NodeId, is_container: bool) -> String {
        let info = &self.info[node];
        if is_container {
            format!("{:?} [size {} total={}]", info.kind, info.size, info.total)
        } else {
            format!("[size {}]", info.size)
        }
    }

    fn is_focused_in_subtree(&self, map: &NodeMap, window: &Window, node: NodeId) -> bool {
        if window.at(node).is_some() {
            if let Some(parent) = node.parent(map) {
                return parent.first_child(map) == Some(node);
            }
        }
        for child in node.children(map) {
            if self.is_focused_in_subtree(map, window, child) {
                return true;
            }
        }
        false
    }

    fn calculate_always_visible_stack_layout(
        &self,
        container_rect: CGRect,
        window_count: usize,
        stack_offset: f64,
        is_horizontal: bool,
    ) -> StackLayoutResult {
        if window_count == 0 {
            return StackLayoutResult::new(container_rect, 0, stack_offset, is_horizontal);
        }
        let total_offset_space = (window_count - 1) as f64 * stack_offset;
        let min_window_dimension = 100.0;
        let (available_width, available_height) = if is_horizontal {
            (
                (container_rect.size.width - total_offset_space).max(min_window_dimension),
                container_rect.size.height,
            )
        } else {
            (
                container_rect.size.width,
                (container_rect.size.height - total_offset_space).max(min_window_dimension),
            )
        };
        let effective_width = available_width.max(min_window_dimension);
        let effective_height = available_height.max(min_window_dimension);
        StackLayoutResult {
            container_rect,
            _window_count: window_count,
            stack_offset,
            is_horizontal,
            window_width: effective_width,
            window_height: effective_height,
        }
    }

    pub(crate) fn get_sizes_with_gaps(
        &self,
        map: &NodeMap,
        window: &Window,
        root: NodeId,
        screen: CGRect,
        stack_offset: f64,
        gaps: &crate::common::config::GapSettings,
    ) -> Vec<(WindowId, CGRect)> {
        use objc2_core_foundation::{CGPoint, CGSize};
        let mut sizes = vec![];
        let tiling_area = if gaps.outer.top == 0.0
            && gaps.outer.left == 0.0
            && gaps.outer.bottom == 0.0
            && gaps.outer.right == 0.0
        {
            screen
        } else {
            CGRect {
                origin: CGPoint {
                    x: screen.origin.x + gaps.outer.left,
                    y: screen.origin.y + gaps.outer.top,
                },
                size: CGSize {
                    width: (screen.size.width - gaps.outer.left - gaps.outer.right).max(0.0),
                    height: (screen.size.height - gaps.outer.top - gaps.outer.bottom).max(0.0),
                },
            }
            .round()
        };
        self.apply_with_gaps(
            map,
            window,
            root,
            tiling_area,
            screen,
            &mut sizes,
            stack_offset,
            gaps,
        );
        sizes
    }

    fn apply_with_gaps(
        &self,
        map: &NodeMap,
        window: &Window,
        node: NodeId,
        rect: CGRect,
        screen: CGRect,
        sizes: &mut Vec<(WindowId, CGRect)>,
        stack_offset: f64,
        gaps: &crate::common::config::GapSettings,
    ) {
        let info = &self.info[node];
        let rect = if info.is_fullscreen { screen } else { rect };
        if let Some(wid) = window.at(node) {
            debug_assert!(
                node.children(map).next().is_none(),
                "non-leaf node with window id"
            );
            sizes.push((wid, rect));
            return;
        }
        use LayoutKind::*;
        match info.kind {
            HorizontalStack | VerticalStack => {
                let children: Vec<_> = node.children(map).collect();
                if children.is_empty() {
                    return;
                }
                let is_horizontal = matches!(info.kind, HorizontalStack);
                let layout = self.calculate_always_visible_stack_layout(
                    rect,
                    children.len(),
                    stack_offset,
                    is_horizontal,
                );
                let focused_idx = children
                    .iter()
                    .position(|&c| self.is_focused_in_subtree(map, window, c))
                    .unwrap_or(0);
                for (idx, &child) in children.iter().enumerate() {
                    let frame = if idx == focused_idx {
                        layout.get_focused_frame_for_index(idx, focused_idx)
                    } else {
                        layout.get_frame_for_index(idx)
                    };
                    self.apply_with_gaps(
                        map,
                        window,
                        child,
                        frame,
                        screen,
                        sizes,
                        stack_offset,
                        gaps,
                    );
                }
            }
            Horizontal => {
                self.layout_axis(map, window, node, rect, screen, sizes, stack_offset, gaps, true)
            }
            Vertical => {
                self.layout_axis(map, window, node, rect, screen, sizes, stack_offset, gaps, false)
            }
        }
    }

    fn layout_axis(
        &self,
        map: &NodeMap,
        window: &Window,
        node: NodeId,
        rect: CGRect,
        screen: CGRect,
        sizes: &mut Vec<(WindowId, CGRect)>,
        stack_offset: f64,
        gaps: &crate::common::config::GapSettings,
        horizontal: bool,
    ) {
        use objc2_core_foundation::{CGPoint, CGSize};
        let children: Vec<_> = node.children(map).collect();
        if children.is_empty() {
            return;
        }
        let min_size = 0.05;
        let expected_total = children.len() as f32;
        let mut needs_normalization = false;
        let mut actual_total = 0.0;
        for &child in &children {
            let sz = self.info[child].size;
            actual_total += sz;
            if sz < min_size {
                needs_normalization = true;
            }
        }
        if (actual_total - expected_total).abs() > 0.01 || needs_normalization {
            let share = 1.0;
            unsafe {
                let info = &mut *(&self.info as *const _
                    as *mut slotmap::SecondaryMap<NodeId, LayoutInfo>);
                for &child in &children {
                    info[child].size = share;
                }
                info[node].total = children.len() as f32;
            }
        }
        debug_assert!({
            let sum_children: f32 = children.iter().map(|c| self.info[*c].size).sum();
            (sum_children - self.info[node].total).abs() < 0.01
        });
        let total = self.info[node].total;
        let inner_gap = if horizontal {
            gaps.inner.horizontal
        } else {
            gaps.inner.vertical
        };
        let axis_len = if horizontal {
            rect.size.width
        } else {
            rect.size.height
        };
        let total_gap = (children.len().saturating_sub(1)) as f64 * inner_gap;
        let usable_axis = if inner_gap == 0.0 {
            axis_len
        } else {
            (axis_len - total_gap).max(0.0)
        };
        let mut offset = if horizontal {
            rect.origin.x
        } else {
            rect.origin.y
        };
        for (i, &child) in children.iter().enumerate() {
            let ratio = f64::from(self.info[child].size) / f64::from(total);
            let seg_len = usable_axis * ratio;
            let child_rect = if horizontal {
                CGRect {
                    origin: CGPoint { x: offset, y: rect.origin.y },
                    size: CGSize {
                        width: seg_len,
                        height: rect.size.height,
                    },
                }
            } else {
                CGRect {
                    origin: CGPoint { x: rect.origin.x, y: offset },
                    size: CGSize {
                        width: rect.size.width,
                        height: seg_len,
                    },
                }
            }
            .round();
            self.apply_with_gaps(map, window, child, child_rect, screen, sizes, stack_offset, gaps);
            offset += seg_len;
            if i < children.len() - 1 {
                offset += inner_gap;
            }
        }
    }
}
