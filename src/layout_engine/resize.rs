use objc2_core_foundation::CGPoint;
use serde::{Deserialize, Serialize};

/// Corner specification for resize operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResizeCorner {
    #[default]
    None,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl ResizeCorner {
    pub fn affects_left(&self) -> bool { matches!(self, Self::TopLeft | Self::BottomLeft) }

    pub fn affects_right(&self) -> bool {
        matches!(self, Self::TopRight | Self::BottomRight | Self::None)
    }

    pub fn affects_top(&self) -> bool { matches!(self, Self::TopLeft | Self::TopRight) }

    pub fn affects_bottom(&self) -> bool {
        matches!(self, Self::BottomLeft | Self::BottomRight | Self::None)
    }

    pub fn from_cursor_position(cursor: CGPoint, window_center: CGPoint) -> Self {
        let left = cursor.x < window_center.x;
        let top = cursor.y < window_center.y;

        match (left, top) {
            (true, true) => Self::TopLeft,
            (false, true) => Self::TopRight,
            (true, false) => Self::BottomLeft,
            (false, false) => Self::BottomRight,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ResizeMode {
    /// Relative to current size (Hyprland default)
    #[default]
    Relative,
    /// Absolute target size ("exact" in Hyprland)
    Exact,
}

/// A resize value that can be specified as pixels or percentage.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ResizeValue {
    Pixels(f64),
    Percent(f64),
}

impl ResizeValue {
    pub fn parse(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        if trimmed.ends_with('%') {
            let num_str = trimmed.trim_end_matches('%');
            let pct: f64 = num_str.parse().ok()?;
            Some(Self::Percent(pct / 100.0))
        } else {
            let px: f64 = trimmed.parse().ok()?;
            Some(Self::Pixels(px))
        }
    }
}

/// 2-dimensional resize payload with a mode.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct ResizeDelta {
    pub x: ResizeValue,
    pub y: ResizeValue,
    #[serde(default)]
    pub mode: ResizeMode,
}

impl Default for ResizeDelta {
    fn default() -> Self {
        Self {
            x: ResizeValue::Pixels(0.0),
            y: ResizeValue::Pixels(0.0),
            mode: ResizeMode::Relative,
        }
    }
}

impl ResizeDelta {
    pub fn relative(x: ResizeValue, y: ResizeValue) -> Self {
        Self {
            x,
            y,
            mode: ResizeMode::Relative,
        }
    }

    pub fn exact(x: ResizeValue, y: ResizeValue) -> Self {
        Self {
            x,
            y,
            mode: ResizeMode::Exact,
        }
    }

    /// Convert into pixel deltas given current size (for relative) and screen size (for exact %).
    pub fn to_pixel_delta(
        &self,
        current_w: f64,
        current_h: f64,
        screen_w: f64,
        screen_h: f64,
    ) -> (f64, f64) {
        let dx = Self::one_dim_delta(self.x, self.mode, current_w, screen_w);
        let dy = Self::one_dim_delta(self.y, self.mode, current_h, screen_h);
        (dx, dy)
    }

    fn one_dim_delta(value: ResizeValue, mode: ResizeMode, current: f64, screen: f64) -> f64 {
        let target = match (mode, value) {
            (ResizeMode::Relative, ResizeValue::Pixels(px)) => current + px,
            (ResizeMode::Relative, ResizeValue::Percent(pct)) => current * (1.0 + pct),
            (ResizeMode::Exact, ResizeValue::Pixels(px)) => px,
            (ResizeMode::Exact, ResizeValue::Percent(pct)) => screen * pct,
        };
        target - current
    }
}
