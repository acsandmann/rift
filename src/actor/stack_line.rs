use std::collections::hash_map::Entry;
use std::rc::Rc;

use objc2::MainThreadMarker;
use objc2_app_kit::NSCursor;
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use tracing::instrument;

use crate::actor::app::WindowId;
use crate::actor::reactor::{Command, ReactorCommand};
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
    pub window_ids: Vec<WindowId>,
}

#[derive(Debug)]
pub enum Event {
    GroupsUpdated {
        space_id: SpaceId,
        groups: Vec<GroupInfo>,
    },
    ScreenParametersChanged(CoordinateConverter),
    ConfigUpdated(Config),
    MouseDown(CGPoint),
    MouseMoved(CGPoint),
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
    cursor_over_indicator: bool,
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
            cursor_over_indicator: false,
        }
    }

    pub async fn run(mut self) {
        if !self.is_enabled() {
            tracing::debug!("stack line disabled at start; will listen for config changes");
        }

        while let Some((span, event)) = self.rx.recv().await {
            let _guard = span.enter();
            self.handle_event(event);
        }
    }

    fn is_enabled(&self) -> bool { self.config.settings.ui.stack_line.enabled }

    #[instrument(name = "stack_line::handle_event", skip(self))]
    fn handle_event(&mut self, event: Event) {
        if !self.is_enabled()
            && !matches!(
                event,
                Event::ConfigUpdated(_)
                    | Event::ScreenParametersChanged(_)
                    | Event::MouseDown(_)
                    | Event::MouseMoved(_)
            )
        {
            return;
        }
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
            Event::MouseDown(point) => {
                self.handle_mouse_down(point);
            }
            Event::MouseMoved(point) => {
                self.handle_mouse_moved(point);
            }
        }
    }

    fn handle_groups_updated(&mut self, space_id: SpaceId, groups: Vec<GroupInfo>) {
        let sigs: Vec<GroupSig> = groups.iter().map(GroupSig::from_group_info).collect();

        match self.group_sigs_by_space.entry(space_id) {
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
        self.indicators.retain(|&node_id, indicator| match indicator.space_id() {
            Some(indicator_space_id) if indicator_space_id == space_id => {
                if group_nodes.contains(&node_id) {
                    true
                } else {
                    if let Err(err) = indicator.clear() {
                        tracing::warn!(?err, "failed to clear stack line indicator");
                    }
                    false
                }
            }
            _ => true,
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

    fn handle_mouse_down(&mut self, screen_point: CGPoint) {
        if !self.is_enabled() {
            return;
        }

        for (&node_id, indicator) in &self.indicators {
            let frame = indicator.frame();
            let (mx, my) = hit_margins(frame, indicator.recommended_thickness());

            if point_in_hit_area(screen_point, frame, mx, my) {
                let local_point =
                    CGPoint::new(screen_point.x - frame.origin.x, screen_point.y - frame.origin.y);

                if let Some(segment_index) = indicator.check_click(local_point) {
                    tracing::debug!(
                        ?node_id,
                        segment_index,
                        "Detected click on stack line indicator segment"
                    );
                    self.handle_indicator_clicked(node_id, segment_index);
                    return;
                }
            }
        }
    }

    // this is very hacky but we don't use nswindow so we have to roll this ourselves
    fn handle_mouse_moved(&mut self, screen_point: CGPoint) {
        let over_indicator = if self.is_enabled() {
            self.indicators.values().any(|indicator| {
                let frame = indicator.frame();
                let (mx, my) = hit_margins(frame, indicator.recommended_thickness());
                let enter_mul = 1.0;
                let exit_mul = 0.65;

                // enter hitbox is larger than exit
                let (mx, my) = if self.cursor_over_indicator {
                    (mx * exit_mul, my * exit_mul)
                } else {
                    (mx * enter_mul, my * enter_mul)
                };

                point_in_hit_area(screen_point, frame, mx, my)
            })
        } else {
            false
        };

        // the hack
        if over_indicator != self.cursor_over_indicator {
            self.cursor_over_indicator = over_indicator;
            if over_indicator {
                NSCursor::pointingHandCursor().set();
                tracing::trace!("Set pointing hand cursor over indicator");
            } else {
                NSCursor::arrowCursor().set();
                tracing::trace!("Reset to arrow cursor");
            }
        }
    }

    fn handle_indicator_clicked(&mut self, node_id: NodeId, segment_index: usize) {
        if let Some(indicator) = self.indicators.get(&node_id) {
            let window_ids = indicator.window_ids();
            if let Some(window_id) = window_ids.get(segment_index) {
                tracing::debug!(
                    ?node_id,
                    segment_index,
                    ?window_id,
                    "Group indicator clicked - focusing window"
                );
                let _ = self.reactor_tx.send(reactor::Event::Command(Command::Reactor(
                    ReactorCommand::FocusWindow {
                        window_id: *window_id,
                        window_server_id: None,
                    },
                )));
            } else {
                tracing::debug!(
                    ?node_id,
                    segment_index,
                    "Group indicator clicked with invalid segment index"
                );
            }
        } else {
            tracing::debug!(
                ?node_id,
                segment_index,
                "Group indicator clicked but not found in map"
            );
        }
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
            window_ids: group.window_ids,
        };

        let indicator_frame = Self::calculate_indicator_frame(
            group.frame,
            group_kind,
            config.bar_thickness,
            config.horizontal_placement,
            config.vertical_placement,
            config.spacing,
        );

        let node_id = group.node_id;

        if let Some(indicator) = self.indicators.get_mut(&node_id) {
            if let Err(err) = indicator.set_frame(indicator_frame) {
                tracing::warn!(?err, "failed to set stack line indicator frame");
            }
            indicator.set_space_id(group.space_id);
            if let Err(err) = indicator.update(config, group_data.clone()) {
                tracing::warn!(?err, "failed to update stack line indicator");
            }
        } else {
            match GroupIndicatorWindow::new(indicator_frame, config) {
                Ok(indicator) => {
                    indicator.set_space_id(group.space_id);
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
        _horizontal_placement: HorizontalPlacement,
        _vertical_placement: VerticalPlacement,
        spacing: f64,
    ) -> CGRect {
        let min_size = thickness * 2.0;
        let adjusted_width = group_frame.size.width.max(min_size);
        let adjusted_height = group_frame.size.height.max(min_size);

        match group_kind {
            GroupKind::Horizontal => CGRect::new(
                CGPoint::new(group_frame.origin.x, group_frame.origin.y - spacing),
                CGSize::new(adjusted_width, thickness),
            ),
            GroupKind::Vertical => CGRect::new(
                CGPoint::new(group_frame.origin.x - spacing, group_frame.origin.y),
                CGSize::new(thickness, adjusted_height),
            ),
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

fn hit_margins(frame: CGRect, thickness: f64) -> (f64, f64) {
    let base = (thickness * 0.25).clamp(1.0, 5.0);
    let target_short = 14.0;

    if frame.size.width < frame.size.height {
        // vertical: widen hitbox more in X
        let mx =
            (base + ((target_short - frame.size.width as f64) * 0.5).max(0.0)).clamp(1.0, 10.0);
        let my = base.clamp(1.0, 4.0);
        (mx, my)
    } else {
        // horizontal: expand more in Y
        let mx = base.clamp(1.0, 4.0);
        let my =
            (base + ((target_short - frame.size.height as f64) * 0.5).max(0.0)).clamp(1.0, 10.0);
        (mx, my)
    }
}

fn point_in_hit_area(point: CGPoint, frame: CGRect, mx: f64, my: f64) -> bool {
    point.x >= frame.origin.x - mx
        && point.x < frame.origin.x + frame.size.width + mx
        && point.y >= frame.origin.y - my
        && point.y < frame.origin.y + frame.size.height + my
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
        let spacing = 4.0;

        let frame_horizontal = StackLine::calculate_indicator_frame(
            group_frame,
            GroupKind::Horizontal,
            thickness,
            HorizontalPlacement::Top,
            VerticalPlacement::Right,
            spacing,
        );
        assert_eq!(frame_horizontal.origin.x, 100.0);
        assert_eq!(frame_horizontal.origin.y, 200.0 - spacing);
        assert_eq!(frame_horizontal.size.width, 400.0);
        assert_eq!(frame_horizontal.size.height, thickness);

        let frame_vertical = StackLine::calculate_indicator_frame(
            group_frame,
            GroupKind::Vertical,
            thickness,
            HorizontalPlacement::Top,
            VerticalPlacement::Left,
            spacing,
        );
        assert_eq!(frame_vertical.origin.x, 100.0 - spacing);
        assert_eq!(frame_vertical.origin.y, 200.0);
        assert_eq!(frame_vertical.size.width, thickness);
        assert_eq!(frame_vertical.size.height, 300.0);
    }
}
