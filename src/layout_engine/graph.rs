use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Orientation {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl Direction {
    pub fn orientation(self) -> Orientation {
        match self {
            Direction::Left | Direction::Right => Orientation::Horizontal,
            Direction::Up | Direction::Down => Orientation::Vertical,
        }
    }

    pub fn opposite(self) -> Direction {
        match self {
            Direction::Left => Direction::Right,
            Direction::Right => Direction::Left,
            Direction::Up => Direction::Down,
            Direction::Down => Direction::Up,
        }
    }
}

impl From<String> for Direction {
    fn from(s: String) -> Self {
        match s.as_str() {
            "left" => Direction::Left,
            "right" => Direction::Right,
            "up" => Direction::Up,
            "down" => Direction::Down,
            _ => panic!("Invalid direction string: {}", s),
        }
    }
}

impl Direction {
    pub fn step(&self, i: usize, len: usize) -> usize {
        match *self {
            Direction::Left => (i + len - 1) % len,
            Direction::Right => (i + 1) % len,
            _ => 0,
        }
    }
}

#[allow(unused)]
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutKind {
    #[default]
    Horizontal,
    Vertical,
    HorizontalStack,
    VerticalStack,
}

impl LayoutKind {
    pub fn from(orientation: Orientation) -> Self {
        match orientation {
            Orientation::Horizontal => LayoutKind::Horizontal,
            Orientation::Vertical => LayoutKind::Vertical,
        }
    }

    pub fn stack_with_offset(orientation: Orientation) -> Self {
        match orientation {
            Orientation::Horizontal => LayoutKind::HorizontalStack,
            Orientation::Vertical => LayoutKind::VerticalStack,
        }
    }

    pub fn is_stacked(self) -> bool {
        matches!(self, LayoutKind::HorizontalStack | LayoutKind::VerticalStack)
    }

    pub fn orientation(self) -> Orientation {
        use LayoutKind::*;
        match self {
            Horizontal => Orientation::Horizontal,
            Vertical => Orientation::Vertical,
            HorizontalStack => Orientation::Horizontal,
            VerticalStack => Orientation::Vertical,
        }
    }

    pub fn is_group(self) -> bool {
        matches!(self, LayoutKind::HorizontalStack | LayoutKind::VerticalStack)
    }
}
