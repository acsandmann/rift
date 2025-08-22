use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use objc2::MainThreadMarker;
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use tracing::instrument;

use crate::actor::{self, reactor};
use crate::common::config::{Config, HorizontalPlacement, VerticalPlacement};
use crate::layout_engine::LayoutKind;
use crate::model::tree::NodeId;
use crate::sys::screen::{CoordinateConverter, SpaceId};
use crate::ui::stack_line::{GroupDisplayData, GroupIndicatorNSView, GroupKind, IndicatorConfig};

#[derive(Debug, Clone)]
pub struct GroupInfo {
    pub node_id: NodeId,
    pub space_id: SpaceId,
    pub container_kind: LayoutKind,
    pub frame: CGRect,
    pub total_count: usize,
    pub selected_index: usize,
}

#[derive(Debug)]
pub enum Event {
    GroupsUpdated {
        space_id: SpaceId,
        groups: Vec<GroupInfo>,
    },
    GroupSelectionChanged {
        node_id: NodeId,
        selected_index: usize,
    },
    ScreenParametersChanged(CoordinateConverter),
}

pub struct StackLine {
    config: Arc<Config>,
    rx: Receiver,
    mtm: MainThreadMarker,
    indicators: HashMap<NodeId, GroupIndicatorNSView>,
    #[allow(dead_code)]
    reactor_tx: reactor::Sender,
    coordinate_converter: CoordinateConverter,
}

pub type Sender = actor::Sender<Event>;
pub type Receiver = actor::Receiver<Event>;

impl StackLine {
    pub fn new(
        config: Arc<Config>,
        rx: Receiver,
        mtm: MainThreadMarker,
        reactor_tx: reactor::Sender,
        coordinate_converter: CoordinateConverter,
    ) -> Self {
        Self {
            config,
            rx,
            mtm,
            indicators: HashMap::new(),
            reactor_tx,
            coordinate_converter,
        }
    }

    pub async fn run(mut self) {
        if !self.is_enabled() {
            return;
        }

        while let Some((span, event)) = self.rx.recv().await {
            let _guard = span.enter();
            self.handle_event(event);
        }
    }

    fn is_enabled(&self) -> bool { self.config.settings.ui.stack_line.enabled }

    #[instrument(name = "stack_line::handle_event", skip(self))]
    fn handle_event(&mut self, event: Event) {
        match event {
            Event::GroupsUpdated { space_id, groups } => {
                self.handle_groups_updated(space_id, groups);
            }
            Event::GroupSelectionChanged { node_id, selected_index } => {
                self.handle_selection_changed(node_id, selected_index);
            }
            Event::ScreenParametersChanged(converter) => {
                self.handle_screen_parameters_changed(converter);
            }
        }
    }

    fn handle_groups_updated(&mut self, _space_id: SpaceId, groups: Vec<GroupInfo>) {
        let group_nodes: std::collections::HashSet<NodeId> =
            groups.iter().map(|g| g.node_id).collect();
        self.indicators.retain(|&node_id, indicator| {
            // TODO: Also check if they r on the same space when we track that
            if group_nodes.contains(&node_id) {
                true
            } else {
                indicator.clear();
                false
            }
        });

        for group in groups {
            self.update_or_create_indicator(group);
        }
    }

    fn handle_selection_changed(&mut self, node_id: NodeId, selected_index: usize) {
        if let Some(indicator) = self.indicators.get_mut(&node_id) {
            if let Some(mut group_data) = indicator.group_data() {
                group_data.selected_index = selected_index;
                indicator.update(group_data);
            }
        }
    }

    fn handle_screen_parameters_changed(&mut self, converter: CoordinateConverter) {
        self.coordinate_converter = converter;
        tracing::debug!("Updated coordinate converter for group indicators");
    }

    // TODO: make this work
    fn handle_indicator_clicked(&mut self, node_id: NodeId, segment_index: usize) {
        // TODO: Send event to reactor when indicators are clicked
        // For now just log the click
        tracing::debug!(?node_id, segment_index, "Group indicator clicked");
        // self.reactor_tx.send(reactor::Event::GroupIndicatorClicked { node_id, segment_index });
    }

    fn update_or_create_indicator(&mut self, group: GroupInfo) {
        let group_kind = match group.container_kind {
            LayoutKind::HorizontalStack => GroupKind::Horizontal,
            LayoutKind::VerticalStack => GroupKind::Vertical,
            _ => {
                tracing::warn!(?group.container_kind, "Unexpected container kind for group");
                return;
            }
        };

        let group_data = GroupDisplayData {
            group_kind,
            total_count: group.total_count,
            selected_index: group.selected_index,
        };

        let node_id = group.node_id;
        let needs_creation = !self.indicators.contains_key(&node_id);

        if needs_creation {
            let mut indicator = GroupIndicatorNSView::new(group.frame, self.mtm);
            indicator.update(group_data);

            indicator.ensure_host_window(self.mtm);

            let self_ptr: *mut StackLine = self as *mut _;
            indicator.set_click_callback(Rc::new(move |segment_index| {
                unsafe {
                    // safety: `self_ptr` remains valid while the actor lives.
                    let this: &mut StackLine = &mut *self_ptr;
                    this.handle_indicator_clicked(node_id, segment_index);
                }
            }));

            self.indicators.insert(node_id, indicator);
        } else {
            if let Some(existing) = self.indicators.get_mut(&node_id) {
                existing.update(group_data);
            }
        }

        if let Some(indicator) = self.indicators.get(&node_id) {
            self.position_indicator(indicator, group.frame);
        }
    }

    fn position_indicator(&self, indicator: &GroupIndicatorNSView, group_frame: CGRect) {
        let config = self.indicator_config();

        let Some(group_data) = indicator.group_data() else {
            tracing::warn!("Cannot position indicator without group data");
            return;
        };

        let cocoa_group_frame = match self.coordinate_converter.convert_rect(group_frame) {
            Some(frame) => frame,
            None => {
                tracing::warn!("Failed to convert group frame coordinates");
                return;
            }
        };

        let indicator_frame = Self::calculate_indicator_frame(
            cocoa_group_frame,
            group_data.group_kind,
            config.bar_thickness,
            config.horizontal_placement,
            config.vertical_placement,
        );

        indicator.set_frame(indicator_frame);

        tracing::debug!(
            ?group_frame,
            ?cocoa_group_frame,
            ?indicator_frame,
            "Positioned indicator"
        );
    }

    // TODO: We should just pass in the coordinates from the layout calculation.
    fn calculate_indicator_frame(
        group_frame: CGRect,
        group_kind: GroupKind,
        thickness: f64,
        horizontal_placement: HorizontalPlacement,
        vertical_placement: VerticalPlacement,
    ) -> CGRect {
        match group_kind {
            GroupKind::Horizontal => match horizontal_placement {
                HorizontalPlacement::Top => CGRect::new(
                    CGPoint::new(
                        group_frame.origin.x,
                        group_frame.origin.y + group_frame.size.height - thickness,
                    ),
                    CGSize::new(group_frame.size.width, thickness),
                ),
                HorizontalPlacement::Bottom => CGRect::new(
                    group_frame.origin,
                    CGSize::new(group_frame.size.width, thickness),
                ),
            },
            GroupKind::Vertical => match vertical_placement {
                VerticalPlacement::Left => CGRect::new(
                    group_frame.origin,
                    CGSize::new(thickness, group_frame.size.height),
                ),
                VerticalPlacement::Right => CGRect::new(
                    CGPoint::new(
                        group_frame.origin.x + group_frame.size.width - thickness,
                        group_frame.origin.y,
                    ),
                    CGSize::new(thickness, group_frame.size.height),
                ),
            },
        }
    }

    fn indicator_config(&self) -> IndicatorConfig {
        IndicatorConfig::from(&self.config.settings.ui.stack_line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_group_info_fields() {
        assert_eq!(LayoutKind::VerticalStack.is_group(), true);
        assert_eq!(LayoutKind::HorizontalStack.is_group(), true);
        assert_eq!(LayoutKind::Horizontal.is_group(), false);
    }

    #[test]
    fn test_calculate_indicator_frame() {
        let group_frame = CGRect::new(CGPoint::new(100.0, 200.0), CGSize::new(400.0, 300.0));
        let thickness = 6.0;

        let frame_top = StackLine::calculate_indicator_frame(
            group_frame,
            GroupKind::Horizontal,
            thickness,
            HorizontalPlacement::Top,
            VerticalPlacement::Right,
        );
        assert_eq!(frame_top.origin.x, 100.0);
        assert_eq!(frame_top.origin.y, 200.0 + 300.0 - thickness);
        assert_eq!(frame_top.size.width, 400.0);
        assert_eq!(frame_top.size.height, thickness);

        let frame_bottom = StackLine::calculate_indicator_frame(
            group_frame,
            GroupKind::Horizontal,
            thickness,
            HorizontalPlacement::Bottom,
            VerticalPlacement::Right,
        );
        assert_eq!(frame_bottom.origin.x, 100.0);
        assert_eq!(frame_bottom.origin.y, 200.0);
        assert_eq!(frame_bottom.size.width, 400.0);
        assert_eq!(frame_bottom.size.height, thickness);

        let frame_left = StackLine::calculate_indicator_frame(
            group_frame,
            GroupKind::Vertical,
            thickness,
            HorizontalPlacement::Top,
            VerticalPlacement::Left,
        );
        assert_eq!(frame_left.origin.x, 100.0);
        assert_eq!(frame_left.origin.y, 200.0);
        assert_eq!(frame_left.size.width, thickness);
        assert_eq!(frame_left.size.height, 300.0);

        let frame_right = StackLine::calculate_indicator_frame(
            group_frame,
            GroupKind::Vertical,
            thickness,
            HorizontalPlacement::Top,
            VerticalPlacement::Right,
        );
        assert_eq!(frame_right.origin.x, 100.0 + 400.0 - thickness);
        assert_eq!(frame_right.origin.y, 200.0);
        assert_eq!(frame_right.size.width, thickness);
        assert_eq!(frame_right.size.height, 300.0);
    }
}
