use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Orientation {
    Horizontal,
    Vertical,
}

impl Default for Orientation {
    fn default() -> Self { Self::Horizontal }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// Directional distance in screen coordinates, where Y increases downward.
pub trait DirectionalDistance {
    fn distance_in_direction(self, target: Self, direction: Direction) -> Option<f64>;
}

impl DirectionalDistance for (f64, f64) {
    fn distance_in_direction(self, target: Self, direction: Direction) -> Option<f64> {
        let distance = match direction {
            Direction::Left => self.0 - target.0,
            Direction::Right => target.0 - self.0,
            Direction::Up => self.1 - target.1,
            Direction::Down => target.1 - self.1,
        };
        (distance >= 0.0).then_some(distance)
    }
}

impl Direction {
    pub fn orientation(self) -> Orientation {
        match self {
            Self::Left | Self::Right => Orientation::Horizontal,
            Self::Up | Self::Down => Orientation::Vertical,
        }
    }

    pub const fn opposite(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
            Self::Up => Self::Down,
            Self::Down => Self::Up,
        }
    }

    pub fn step(self, index: usize, len: usize) -> usize {
        match self {
            Self::Left => (index + len - 1) % len,
            Self::Right => (index + 1) % len,
            Self::Up | Self::Down => 0,
        }
    }
}

impl From<String> for Direction {
    fn from(value: String) -> Self {
        match value.as_str() {
            "left" => Self::Left,
            "right" => Self::Right,
            "up" => Self::Up,
            "down" => Self::Down,
            _ => panic!("Invalid direction string: {value}"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResizeOrientation {
    #[default]
    Horizontal,
    Vertical,
    Smart,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutKind {
    #[default]
    Horizontal,
    Vertical,
    HorizontalStack,
    VerticalStack,
}

impl LayoutKind {
    pub const fn from(orientation: Orientation) -> Self {
        match orientation {
            Orientation::Horizontal => Self::Horizontal,
            Orientation::Vertical => Self::Vertical,
        }
    }

    pub const fn stack_with_offset(orientation: Orientation) -> Self {
        match orientation {
            Orientation::Horizontal => Self::HorizontalStack,
            Orientation::Vertical => Self::VerticalStack,
        }
    }

    pub const fn is_stacked(self) -> bool {
        matches!(self, Self::HorizontalStack | Self::VerticalStack)
    }

    pub const fn orientation(self) -> Orientation {
        match self {
            Self::Horizontal | Self::HorizontalStack => Orientation::Horizontal,
            Self::Vertical | Self::VerticalStack => Orientation::Vertical,
        }
    }

    pub const fn is_group(self) -> bool { self.is_stacked() }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutMode {
    #[default]
    Traditional,
    Bsp,
    Stack,
    MasterStack,
    Scrolling,
    Floating,
}

impl fmt::Display for LayoutMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Traditional => "traditional",
            Self::Bsp => "bsp",
            Self::Stack => "stack",
            Self::MasterStack => "master_stack",
            Self::Scrolling => "scrolling",
            Self::Floating => "floating",
        })
    }
}
