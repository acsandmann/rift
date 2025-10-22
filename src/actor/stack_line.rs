use std::collections::hash_map::Entry;
use std::rc::Rc;

use objc2::MainThreadMarker;
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use tracing::instrument;

use crate::actor::{self, reactor};
use crate::common::collections::HashMap;
use crate::common::config::{Config, HorizontalPlacement, VerticalPlacement};
use crate::layout_engine::LayoutKind;
use crate::model::tree::NodeId;
use crate::sys::screen::{CoordinateConverter, SpaceId};
use crate::ui::stack_line::{GroupDisplayData, GroupIndicatorWindow, GroupKind, IndicatorConfig};

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
    ScreenParametersChanged(CoordinateConverter),
    ConfigUpdated(Config),
}

pub struct StackLine {
    config: Config,
    rx: Receiver,
    #[allow(dead_code)]
    mtm: MainThreadMarker,
    indicators: HashMap<NodeId, GroupIndicatorWindow>,
    #[allow(dead_code)]
    reactor_tx: reactor::Sender,
    coordinate_converter: CoordinateConverter,
    group_sigs_by_space: HashMap<SpaceId, Vec<GroupSig>>,
}

pub type Sender = actor::Sender<Event>;
pub type Receiver = actor::Receiver<Event>;

impl StackLine {
    pub fn new(
        config: Config,
        rx: Receiver,
        mtm: MainThreadMarker,
        reactor_tx: reactor::Sender,
        coordinate_converter: CoordinateConverter,
    ) -> Self {
        Self {
            config,
            rx,
            mtm,
            indicators: HashMap::default(),
            reactor_tx,
            coordinate_converter,
            group_sigs_by_space: HashMap::default(),
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
            Event::ScreenParametersChanged(converter) => {
                self.handle_screen_parameters_changed(converter);
            }
            Event::ConfigUpdated(config) => {
                self.handle_config_updated(config);
            }
        }
    }

    fn handle_groups_updated(&mut self, _space_id: SpaceId, groups: Vec<GroupInfo>) {
        let sigs: Vec<GroupSig> = groups.iter().map(GroupSig::from_group_info).collect();

        match self.group_sigs_by_space.entry(_space_id) {
            Entry::Occupied(mut prev) => {
                if prev.get() == &sigs {
                    return;
                }
                let _ = prev.insert(sigs);
            }
            Entry::Vacant(v) => {
                let _ = v.insert(sigs);
            }
        };

        let group_nodes: std::collections::HashSet<NodeId> =
            groups.iter().map(|g| g.node_id).collect();
        self.indicators.retain(|&node_id, indicator| {
            // TODO: Also check if they r on the same space when we track that
            if group_nodes.contains(&node_id) {
                true
            } else {
                if let Err(err) = indicator.clear() {
                    tracing::warn!(?err, "failed to clear stack line indicator");
                }
                false
            }
        });

        for group in groups {
            self.update_or_create_indicator(group);
        }
    }

    fn handle_screen_parameters_changed(&mut self, converter: CoordinateConverter) {
        self.coordinate_converter = converter;
        tracing::debug!("Updated coordinate converter for group indicators");
    }

    fn handle_config_updated(&mut self, config: Config) {
        let old_enabled = self.is_enabled();
        self.config = config;
        let new_enabled = self.is_enabled();

        if old_enabled && !new_enabled {
            for indicator in self.indicators.values() {
                if let Err(err) = indicator.clear() {
                    tracing::warn!(
                        ?err,
                        "failed to clear stack line indicator during config update"
                    );
                }
            }
            self.indicators.clear();
            self.group_sigs_by_space.clear();
        } else if new_enabled {
            let new_config = self.indicator_config();
            for (node_id, indicator) in &self.indicators {
                if let Some(group_data) = indicator.group_data() {
                    if let Err(err) = indicator.update(new_config, group_data) {
                        tracing::warn!(
                            ?err,
                            ?node_id,
                            "failed to update stack line indicator with new config"
                        );
                    }
                }
            }
        }

        tracing::debug!("Updated stack line configuration");
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

        let config = self.indicator_config();
        let group_data = GroupDisplayData {
            group_kind,
            total_count: group.total_count,
            selected_index: group.selected_index,
        };

        let indicator_frame = Self::calculate_indicator_frame(
            group.frame,
            group_kind,
            config.bar_thickness,
            config.horizontal_placement,
            config.vertical_placement,
        );

        let node_id = group.node_id;

        if let Some(indicator) = self.indicators.get_mut(&node_id) {
            if let Err(err) = indicator.set_frame(indicator_frame) {
                tracing::warn!(?err, "failed to set stack line indicator frame");
            }
            if let Err(err) = indicator.update(config, group_data.clone()) {
                tracing::warn!(?err, "failed to update stack line indicator");
            }
        } else {
            match GroupIndicatorWindow::new(indicator_frame, config) {
                Ok(indicator) => {
                    let indicator =
                        self.attach_indicator(node_id, indicator, config, group_data.clone());
                    self.indicators.insert(node_id, indicator);
                }
                Err(err) => {
                    tracing::warn!(?err, "failed to create stack line indicator window");
                    return;
                }
            }
        }

        tracing::debug!(
            ?group.frame,
            ?indicator_frame,
            "Positioned indicator"
        );
    }

    fn attach_indicator(
        &mut self,
        node_id: NodeId,
        indicator: GroupIndicatorWindow,
        config: IndicatorConfig,
        group_data: GroupDisplayData,
    ) -> GroupIndicatorWindow {
        let self_ptr: *mut StackLine = self as *mut _;
        indicator.set_click_callback(Rc::new(move |segment_index| {
            unsafe {
                // safety: `self_ptr` remains valid while the actor lives.
                let this: &mut StackLine = &mut *self_ptr;
                this.handle_indicator_clicked(node_id, segment_index);
            }
        }));

        if let Err(err) = indicator.update(config, group_data.clone()) {
            tracing::warn!(?err, "failed to initialize stack line indicator");
        }

        indicator
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
                    group_frame.origin,
                    CGSize::new(group_frame.size.width, thickness),
                ),
                HorizontalPlacement::Bottom => CGRect::new(
                    CGPoint::new(
                        group_frame.origin.x,
                        group_frame.origin.y + group_frame.size.height - thickness,
                    ),
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

#[derive(Debug, Clone, PartialEq)]
struct GroupSig {
    node_id: NodeId,
    kind: LayoutKind,
    x_q2: i64,
    y_q2: i64,
    w_q2: i64,
    h_q2: i64,
    total: usize,
    selected_index: usize,
}

impl GroupSig {
    fn from_group_info(g: &GroupInfo) -> GroupSig {
        let quant = |v: f64| -> i64 { (v * 2.0).round() as i64 };
        GroupSig {
            node_id: g.node_id,
            kind: g.container_kind,
            x_q2: quant(g.frame.origin.x),
            y_q2: quant(g.frame.origin.y),
            w_q2: quant(g.frame.size.width),
            h_q2: quant(g.frame.size.height),
            total: g.total_count,
            selected_index: g.selected_index,
        }
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
        assert_eq!(frame_top.origin.y, 200.0);
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
        assert_eq!(frame_bottom.origin.y, 200.0 + 300.0 - thickness);
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
