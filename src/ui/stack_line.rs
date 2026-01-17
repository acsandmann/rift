use std::cell::RefCell;
use std::ptr;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2_app_kit::NSStatusWindowLevel;
use objc2_core_foundation::{CFType, CGPoint, CGRect, CGSize};
use objc2_core_graphics::CGContext;
use objc2_quartz_core::{CALayer, CATransaction};
use tracing::warn;

use crate::actor::app::WindowId;
use crate::common::config::{HorizontalPlacement, VerticalPlacement};
use crate::sys::cgs_window::{CgsWindow, CgsWindowError};
use crate::sys::screen::SpaceId;
use crate::sys::skylight::{
    CFRelease, G_CONNECTION, SLSFlushWindowContentRegion, SLWindowContextCreate,
};

unsafe extern "C" {
    fn CGContextFlush(ctx: *mut CGContext);
    fn CGContextClearRect(ctx: *mut CGContext, rect: CGRect);
    fn CGContextSaveGState(ctx: *mut CGContext);
    fn CGContextRestoreGState(ctx: *mut CGContext);
    fn CGContextTranslateCTM(ctx: *mut CGContext, tx: f64, ty: f64);
    fn CGContextScaleCTM(ctx: *mut CGContext, sx: f64, sy: f64);
}

#[derive(Debug, Clone, Copy)]
pub struct Color {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub a: f64,
}

impl Color {
    pub fn new(r: f64, g: f64, b: f64, a: f64) -> Self { Self { r, g, b, a } }

    pub fn blue() -> Self { Self::new(0.0, 0.5, 1.0, 1.0) }

    pub fn light_gray() -> Self { Self::new(0.8, 0.8, 0.8, 1.0) }

    pub fn gray() -> Self { Self::new(0.6, 0.6, 0.6, 1.0) }

    pub fn to_nscolor(&self) -> Retained<objc2_app_kit::NSColor> {
        objc2_app_kit::NSColor::colorWithRed_green_blue_alpha(self.r, self.g, self.b, self.a)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct IndicatorConfig {
    pub bar_thickness: f64,
    pub selected_color: Color,
    pub unselected_color: Color,
    pub border_color: Color,
    pub border_width: f64,
    pub horizontal_placement: HorizontalPlacement,
    pub vertical_placement: VerticalPlacement,
    pub spacing: f64,
}

impl Default for IndicatorConfig {
    fn default() -> Self {
        Self {
            bar_thickness: 6.0,
            selected_color: Color { r: 0.0, g: 0.5, b: 1.0, a: 0.9 },
            unselected_color: Color { r: 0.5, g: 0.5, b: 0.5, a: 0.4 },
            border_color: Color { r: 0.3, g: 0.3, b: 0.3, a: 0.6 },
            border_width: 0.3,
            horizontal_placement: HorizontalPlacement::Top,
            vertical_placement: VerticalPlacement::Right,
            spacing: 4.0,
        }
    }
}

impl From<&crate::common::config::StackLineSettings> for IndicatorConfig {
    fn from(config: &crate::common::config::StackLineSettings) -> Self {
        Self {
            bar_thickness: config.thickness,
            selected_color: Color::blue(),
            unselected_color: Color::light_gray(),
            border_color: Color::gray(),
            border_width: 0.5,
            horizontal_placement: config.horiz_placement,
            vertical_placement: config.vert_placement,
            spacing: config.spacing,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GroupKind {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone)]
pub struct GroupDisplayData {
    pub group_kind: GroupKind,
    pub total_count: usize,
    pub selected_index: usize,
    pub window_ids: Vec<WindowId>,
}

pub type SegmentClickCallback = Rc<dyn Fn(usize)>;

struct IndicatorState {
    config: IndicatorConfig,
    group_data: Option<GroupDisplayData>,
    background_layer: Option<Retained<CALayer>>,
    separator_layers: Vec<Retained<CALayer>>,
    selected_layer: Option<Retained<CALayer>>,
    click_callback: Option<SegmentClickCallback>,
    space_id: Option<SpaceId>,
}

impl IndicatorState {
    fn new(config: IndicatorConfig) -> Self {
        Self {
            config,
            group_data: None,
            background_layer: None,
            separator_layers: Vec::new(),
            selected_layer: None,
            click_callback: None,
            space_id: None,
        }
    }
}

pub struct GroupIndicatorWindow {
    frame: RefCell<CGRect>,
    root_layer: Retained<CALayer>,
    cgs_window: CgsWindow,
    state: RefCell<IndicatorState>,
}

impl GroupIndicatorWindow {
    pub fn new(frame: CGRect, config: IndicatorConfig) -> Result<Self, CgsWindowError> {
        let root_layer = CALayer::layer();
        root_layer.setFrame(CGRect::new(
            CGPoint::new(0.0, 0.0),
            CGSize::new(frame.size.width, frame.size.height),
        ));

        let cgs_window = CgsWindow::new(frame)?;
        if let Err(err) = cgs_window.set_opacity(false) {
            warn!(error=?err, "failed to set stack line window opacity");
        }
        if let Err(err) = cgs_window.set_alpha(1.0) {
            warn!(error=?err, "failed to set stack line window alpha");
        }
        if let Err(err) = cgs_window.set_level(NSStatusWindowLevel as i32) {
            warn!(error=?err, "failed to set stack line window level");
        }

        Ok(Self {
            frame: RefCell::new(frame),
            root_layer,
            cgs_window,
            state: RefCell::new(IndicatorState::new(config)),
        })
    }

    pub fn update(
        &self,
        config: IndicatorConfig,
        group_data: GroupDisplayData,
    ) -> Result<(), CgsWindowError> {
        let old_selected = self.state.borrow().group_data.as_ref().map(|d| d.selected_index);

        {
            let mut state = self.state.borrow_mut();
            state.config = config;
            state.group_data = Some(group_data.clone());
        }

        self.update_layers();

        if let Some(old_index) = old_selected {
            if old_index != group_data.selected_index {
                self.animate_selection_change(group_data.selected_index);
            }
        }

        self.present();
        self.cgs_window.order_above(None)
    }

    pub fn clear(&self) -> Result<(), CgsWindowError> {
        self.clear_layers();
        self.state.borrow_mut().group_data = None;
        self.state.borrow_mut().space_id = None;
        self.present();
        self.cgs_window.order_out()
    }

    pub fn space_id(&self) -> Option<SpaceId> { self.state.borrow().space_id }

    pub fn set_space_id(&self, space_id: SpaceId) {
        self.state.borrow_mut().space_id = Some(space_id);
    }

    pub fn set_frame(&self, frame: CGRect) -> Result<(), CgsWindowError> {
        self.cgs_window.set_shape(frame)?;
        self.root_layer.setFrame(CGRect::new(
            CGPoint::new(0.0, 0.0),
            CGSize::new(frame.size.width, frame.size.height),
        ));
        *self.frame.borrow_mut() = frame;
        Ok(())
    }

    pub fn recommended_thickness(&self) -> f64 { self.state.borrow().config.bar_thickness }

    pub fn frame(&self) -> CGRect { *self.frame.borrow() }

    pub fn set_click_callback(&self, callback: SegmentClickCallback) {
        self.state.borrow_mut().click_callback = Some(callback);
    }

    pub fn group_data(&self) -> Option<GroupDisplayData> { self.state.borrow().group_data.clone() }

    pub fn window_ids(&self) -> Vec<WindowId> {
        self.group_data().map(|d| d.window_ids).unwrap_or_default()
    }

    pub fn click_segment(&self, segment_index: usize) -> Result<(), CgsWindowError> {
        let Some(group_data) = self.group_data() else {
            return Ok(());
        };
        if segment_index >= group_data.total_count {
            return Ok(());
        }
        let mut updated = group_data;
        updated.selected_index = segment_index;
        let config = self.state.borrow().config;
        self.update(config, updated)
    }

    pub fn check_click(&self, window_point: CGPoint) -> Option<usize> {
        let state = self.state.borrow();
        let Some(group_data) = &state.group_data else {
            return None;
        };
        let bounds = self.bounds();
        Self::segment_at_point_static(window_point, group_data, &bounds)
    }

    pub fn segment_at_point(&self, point: CGPoint, group_data: &GroupDisplayData) -> Option<usize> {
        let bounds = self.bounds();
        Self::segment_at_point_static(point, group_data, &bounds)
    }

    fn bounds(&self) -> CGRect {
        let frame = *self.frame.borrow();
        CGRect::new(CGPoint::new(0.0, 0.0), frame.size)
    }

    fn clear_layers(&self) {
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        unsafe { self.root_layer.setSublayers(None) };
        CATransaction::commit();

        let mut state = self.state.borrow_mut();
        state.background_layer = None;
        state.separator_layers.clear();
        state.selected_layer = None;
    }

    fn update_layers(&self) {
        let Some(group_data) = self.group_data() else {
            self.clear_layers();
            return;
        };

        let bounds = self.bounds();

        CATransaction::begin();
        CATransaction::setDisableActions(true);
        self.ensure_separator_layers(group_data.total_count);
        self.update_background_layer(bounds, group_data.group_kind);

        let config = {
            let state = self.state.borrow();
            state.config
        };
        let adjusted_bounds = self.calculate_adjusted_bounds(bounds, config, group_data.group_kind);
        self.update_separator_layers(&group_data, adjusted_bounds);

        self.update_selected_layer(&group_data, bounds);
        CATransaction::commit();
    }

    fn ensure_separator_layers(&self, total_count: usize) {
        let needed_count = if total_count > 1 { total_count - 1 } else { 0 };

        let mut state = self.state.borrow_mut();

        while state.separator_layers.len() > needed_count {
            if let Some(layer) = state.separator_layers.pop() {
                layer.removeFromSuperlayer();
            }
        }

        while state.separator_layers.len() < needed_count {
            let layer = CALayer::layer();
            self.root_layer.addSublayer(&layer);
            state.separator_layers.push(layer);
        }
    }

    /// Calculate adjusted bounds with proper corner handling only
    fn calculate_adjusted_bounds(
        &self,
        bounds: CGRect,
        config: IndicatorConfig,
        group_kind: GroupKind,
    ) -> CGRect {
        let corner_radius = 8.0;
        let min_corner_offset = corner_radius * 0.7;

        match group_kind {
            GroupKind::Horizontal => {
                let corner_offset = if corner_radius > 0.0 {
                    min_corner_offset
                } else {
                    0.0
                };
                CGRect::new(
                    CGPoint::new(bounds.origin.x + corner_offset, bounds.origin.y),
                    CGSize::new(
                        (bounds.size.width - 2.0 * corner_offset).max(20.0),
                        config.bar_thickness,
                    ),
                )
            }
            GroupKind::Vertical => {
                let corner_offset = if corner_radius > 0.0 {
                    min_corner_offset
                } else {
                    0.0
                };
                CGRect::new(
                    CGPoint::new(bounds.origin.x, bounds.origin.y + corner_offset),
                    CGSize::new(
                        config.bar_thickness,
                        (bounds.size.height - 2.0 * corner_offset).max(20.0),
                    ),
                )
            }
        }
    }

    fn update_background_layer(&self, bounds: CGRect, group_kind: GroupKind) {
        let config = {
            let state = self.state.borrow();
            state.config
        };

        let adjusted_bounds = self.calculate_adjusted_bounds(bounds, config, group_kind);
        let corner_radius = 8.0;

        if corner_radius > 0.0 {
            self.update_background_layer_with_rounded_corners(
                adjusted_bounds,
                config,
                corner_radius,
                group_kind,
            );
        } else {
            self.update_background_layer_standard(adjusted_bounds, config);
        }
    }

    fn update_background_layer_standard(&self, bounds: CGRect, config: IndicatorConfig) {
        let mut state = self.state.borrow_mut();

        let background_layer = if let Some(existing) = &state.background_layer {
            existing.clone()
        } else {
            let layer = CALayer::layer();
            self.root_layer.addSublayer(&layer);
            state.background_layer = Some(layer.clone());
            layer
        };

        background_layer.setFrame(CGRect::new(
            CGPoint::new(bounds.origin.x, bounds.origin.y),
            CGSize::new(bounds.size.width, bounds.size.height),
        ));

        let bg_color = config.unselected_color.to_nscolor();
        background_layer.setBackgroundColor(Some(&bg_color.CGColor()));

        background_layer.setBorderWidth(config.border_width);
        let border_color = config.border_color.to_nscolor();
        background_layer.setBorderColor(Some(&border_color.CGColor()));
    }

    fn update_background_layer_with_rounded_corners(
        &self,
        bounds: CGRect,
        config: IndicatorConfig,
        corner_radius: f64,
        _group_kind: GroupKind,
    ) {
        let mut state = self.state.borrow_mut();

        if let Some(existing) = &state.background_layer {
            existing.removeFromSuperlayer();
        }

        let background_layer = CALayer::layer();
        background_layer.setFrame(bounds);

        let bg_color = config.unselected_color.to_nscolor();
        background_layer.setBackgroundColor(Some(&bg_color.CGColor()));
        let effective_corner_radius =
            (corner_radius * 0.4).min(bounds.size.width / 2.0).min(bounds.size.height / 2.0);
        background_layer.setCornerRadius(effective_corner_radius);

        background_layer.setShadowOpacity(0.15);
        background_layer.setShadowOffset(CGSize::new(0.0, 1.0));
        background_layer.setShadowRadius(2.0);

        let shadow_color = objc2_app_kit::NSColor::blackColor();
        background_layer.setShadowColor(Some(&shadow_color.CGColor()));

        background_layer.setBorderWidth(0.3);
        let border_color = config.border_color.to_nscolor();
        background_layer.setBorderColor(Some(&border_color.CGColor()));

        self.root_layer.insertSublayer_atIndex(&background_layer, 0);
        state.background_layer = Some(background_layer);
    }

    fn update_separator_layers(&self, group_data: &GroupDisplayData, bounds: CGRect) {
        if group_data.total_count <= 1 {
            return;
        }

        let segment_length = match group_data.group_kind {
            GroupKind::Horizontal => bounds.size.width / group_data.total_count as f64,
            GroupKind::Vertical => bounds.size.height / group_data.total_count as f64,
        };

        let state = self.state.borrow();
        let config = state.config;
        for (index, layer) in state.separator_layers.iter().enumerate() {
            let separator_pos = (index + 1) as f64 * segment_length;

            let (sep_x, sep_y, sep_width, sep_height) = match group_data.group_kind {
                GroupKind::Horizontal => (
                    bounds.origin.x + separator_pos - 0.5,
                    bounds.origin.y,
                    1.0,
                    bounds.size.height,
                ),
                GroupKind::Vertical => (
                    bounds.origin.x,
                    bounds.origin.y + separator_pos - 0.5,
                    bounds.size.width,
                    1.0,
                ),
            };

            layer.setFrame(CGRect::new(
                CGPoint::new(sep_x, sep_y),
                CGSize::new(sep_width, sep_height),
            ));

            let separator_color = config.border_color.to_nscolor();
            layer.setBackgroundColor(Some(&separator_color.CGColor()));

            if layer.superlayer().is_none() {
                self.root_layer.addSublayer(layer);
            }
        }
    }

    fn update_selected_layer(&self, group_data: &GroupDisplayData, bounds: CGRect) {
        if group_data.total_count == 0 {
            return;
        }

        let config = {
            let state = self.state.borrow();
            state.config
        };
        let adjusted_bounds = self.calculate_adjusted_bounds(bounds, config, group_data.group_kind);
        let corner_radius = 8.0;

        if corner_radius > 0.0 {
            self.update_selected_layer_with_rounded_corners(
                group_data,
                adjusted_bounds,
                config,
                corner_radius,
            );
        } else {
            self.update_selected_layer_standard(group_data, adjusted_bounds, config);
        }
    }

    fn update_selected_layer_standard(
        &self,
        group_data: &GroupDisplayData,
        bounds: CGRect,
        config: IndicatorConfig,
    ) {
        let mut state = self.state.borrow_mut();

        let selected_layer = if let Some(existing) = &state.selected_layer {
            existing.clone()
        } else {
            let layer = CALayer::layer();
            self.root_layer.addSublayer(&layer);
            state.selected_layer = Some(layer.clone());
            layer
        };

        let segment_frame =
            Self::calculate_segment_frame(group_data, bounds, group_data.selected_index);

        selected_layer.setFrame(segment_frame);

        let selected_color = config.selected_color.to_nscolor();
        selected_layer.setBackgroundColor(Some(&selected_color.CGColor()));
    }

    fn update_selected_layer_with_rounded_corners(
        &self,
        group_data: &GroupDisplayData,
        bounds: CGRect,
        config: IndicatorConfig,
        corner_radius: f64,
    ) {
        let mut state = self.state.borrow_mut();

        if let Some(existing) = &state.selected_layer {
            existing.removeFromSuperlayer();
        }

        let selected_layer = CALayer::layer();

        let segment_frame =
            Self::calculate_segment_frame(group_data, bounds, group_data.selected_index);
        selected_layer.setFrame(segment_frame);

        let selected_color = config.selected_color.to_nscolor();
        selected_layer.setBackgroundColor(Some(&selected_color.CGColor()));
        let effective_corner_radius = (corner_radius * 0.4)
            .min(segment_frame.size.width / 2.0)
            .min(segment_frame.size.height / 2.0);
        selected_layer.setCornerRadius(effective_corner_radius);

        selected_layer.setShadowOpacity(0.25);
        selected_layer.setShadowOffset(CGSize::new(0.0, 1.5));
        selected_layer.setShadowRadius(3.0);

        let shadow_color = objc2_app_kit::NSColor::blackColor();
        selected_layer.setShadowColor(Some(&shadow_color.CGColor()));

        selected_layer.setBorderWidth(0.5);
        let border_color = objc2_app_kit::NSColor::colorWithRed_green_blue_alpha(
            (config.selected_color.r + 0.1).min(1.0),
            (config.selected_color.g + 0.1).min(1.0),
            (config.selected_color.b + 0.1).min(1.0),
            1.0,
        );
        selected_layer.setBorderColor(Some(&border_color.CGColor()));

        self.root_layer.addSublayer(&selected_layer);
        state.selected_layer = Some(selected_layer);
    }

    fn animate_selection_change(&self, to_index: usize) {
        let state = self.state.borrow();
        let Some(selected_layer) = state.selected_layer.clone() else {
            return;
        };
        let Some(group_data) = state.group_data.clone() else {
            return;
        };
        let config = state.config;
        drop(state);

        let bounds = self.bounds();
        let adjusted_bounds = self.calculate_adjusted_bounds(bounds, config, group_data.group_kind);

        let to_frame = Self::calculate_segment_frame(&group_data, adjusted_bounds, to_index);

        selected_layer.setFrame(to_frame);
    }

    fn calculate_segment_frame(
        group_data: &GroupDisplayData,
        bar: CGRect,
        segment_index: usize,
    ) -> CGRect {
        let segment_length = match group_data.group_kind {
            GroupKind::Horizontal => bar.size.width / group_data.total_count as f64,
            GroupKind::Vertical => bar.size.height / group_data.total_count as f64,
        };

        let (seg_x, seg_y, seg_width, seg_height) = match group_data.group_kind {
            GroupKind::Horizontal => {
                let seg_start = (bar.origin.x + (segment_index as f64 * segment_length)).round();
                let seg_end =
                    (bar.origin.x + ((segment_index + 1) as f64 * segment_length)).round();
                let actual_width = seg_end - seg_start;
                (seg_start, bar.origin.y, actual_width, bar.size.height)
            }
            GroupKind::Vertical => {
                let seg_end =
                    bar.origin.y + bar.size.height - (segment_index as f64 * segment_length);
                let seg_start =
                    bar.origin.y + bar.size.height - ((segment_index + 1) as f64 * segment_length);
                let seg_start_rounded = seg_start.round();
                let seg_end_rounded = seg_end.round();
                let actual_height = seg_end_rounded - seg_start_rounded;
                (bar.origin.x, seg_start_rounded, bar.size.width, actual_height)
            }
        };

        CGRect::new(CGPoint::new(seg_x, seg_y), CGSize::new(seg_width, seg_height))
    }

    fn segment_at_point_static(
        point: CGPoint,
        group_data: &GroupDisplayData,
        bounds: &CGRect,
    ) -> Option<usize> {
        let bar = *bounds;

        if point.x < bar.origin.x
            || point.x >= bar.origin.x + bar.size.width
            || point.y < bar.origin.y
            || point.y >= bar.origin.y + bar.size.height
        {
            return None;
        }

        if group_data.total_count == 0 {
            return None;
        }

        let segment_length = match group_data.group_kind {
            GroupKind::Horizontal => bar.size.width / group_data.total_count as f64,
            GroupKind::Vertical => bar.size.height / group_data.total_count as f64,
        };

        let segment_index = match group_data.group_kind {
            GroupKind::Horizontal => {
                let relative_x = point.x - bar.origin.x;
                (relative_x / segment_length).floor() as usize
            }
            GroupKind::Vertical => {
                let relative_y_from_top = (bar.origin.y + bar.size.height) - point.y;
                (relative_y_from_top / segment_length).floor() as usize
            }
        };

        if segment_index < group_data.total_count {
            Some(segment_index)
        } else {
            None
        }
    }

    fn present(&self) {
        let frame = *self.frame.borrow();
        let ctx: *mut CGContext = unsafe {
            SLWindowContextCreate(
                *G_CONNECTION,
                self.cgs_window.id(),
                ptr::null_mut() as *mut CFType,
            )
        };
        if ctx.is_null() {
            return;
        }

        unsafe {
            let clear = CGRect::new(CGPoint::new(0.0, 0.0), frame.size);
            CGContextClearRect(ctx, clear);
            CGContextSaveGState(ctx);
            CGContextTranslateCTM(ctx, 0.0, frame.size.height);
            CGContextScaleCTM(ctx, 1.0, -1.0);
            self.root_layer.renderInContext(&*ctx);
            CGContextRestoreGState(ctx);
            CGContextFlush(ctx);
            SLSFlushWindowContentRegion(*G_CONNECTION, self.cgs_window.id(), ptr::null_mut());
            CFRelease(ctx as *mut CFType);
        }
    }
}
