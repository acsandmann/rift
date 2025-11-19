//! converting between different binding's geometry types

use objc2_core_foundation as ic;
use serde::{Deserialize, Deserializer, Serialize};
use serde_with::{DeserializeAs, SerializeAs};

pub trait Round {
    fn round(&self) -> Self;
}

impl Round for ic::CGRect {
    fn round(&self) -> Self {
        let min_rounded = self.min().round();
        let max_rounded = self.max().round();
        ic::CGRect {
            origin: min_rounded,
            size: ic::CGSize {
                width: max_rounded.x - min_rounded.x,
                height: max_rounded.y - min_rounded.y,
            },
        }
    }
}

impl Round for ic::CGPoint {
    fn round(&self) -> Self {
        ic::CGPoint {
            x: self.x.round(),
            y: self.y.round(),
        }
    }
}

impl Round for ic::CGSize {
    fn round(&self) -> Self {
        ic::CGSize {
            width: self.width.round(),
            height: self.height.round(),
        }
    }
}

pub trait IsWithin {
    fn is_within(&self, how_much: f64, other: Self) -> bool;
}

impl IsWithin for ic::CGRect {
    fn is_within(&self, how_much: f64, other: Self) -> bool {
        self.origin.is_within(how_much, other.origin) && self.size.is_within(how_much, other.size)
    }
}

impl IsWithin for ic::CGPoint {
    fn is_within(&self, how_much: f64, other: Self) -> bool {
        self.x.is_within(how_much, other.x) && self.y.is_within(how_much, other.y)
    }
}

impl IsWithin for ic::CGSize {
    fn is_within(&self, how_much: f64, other: Self) -> bool {
        self.width.is_within(how_much, other.width) && self.height.is_within(how_much, other.height)
    }
}

impl IsWithin for f64 {
    fn is_within(&self, how_much: f64, other: Self) -> bool { (self - other).abs() < how_much }
}

pub trait SameAs: IsWithin + Sized {
    fn same_as(&self, other: Self) -> bool { self.is_within(0.1, other) }
}

impl SameAs for ic::CGRect {}
impl SameAs for ic::CGPoint {}
impl SameAs for ic::CGSize {}

pub trait CGRectExt {
    fn intersection(&self, other: &Self) -> Self;
    fn contains(&self, point: ic::CGPoint) -> bool;
    fn contains_rect(&self, other: Self) -> bool;
    fn area(&self) -> f64;
}

impl CGRectExt for ic::CGRect {
    fn intersection(&self, other: &Self) -> Self {
        let min_x = f64::max(self.min().x, other.min().x);
        let max_x = f64::min(self.max().x, other.max().x);
        let min_y = f64::max(self.min().y, other.min().y);
        let max_y = f64::min(self.max().y, other.max().y);
        ic::CGRect {
            origin: ic::CGPoint::new(min_x, min_y),
            size: ic::CGSize::new(f64::max(max_x - min_x, 0.), f64::max(max_y - min_y, 0.)),
        }
    }

    fn contains(&self, point: ic::CGPoint) -> bool {
        (self.min().x..=self.max().x).contains(&point.x)
            && (self.min().y..=self.max().y).contains(&point.y)
    }

    fn contains_rect(&self, other: Self) -> bool {
        self.min().x <= other.min().x
            && self.min().y <= other.min().y
            && self.max().x >= other.max().x
            && self.max().y >= other.max().y
    }

    fn area(&self) -> f64 { self.size.width * self.size.height }
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "ic::CGRect")]
pub struct CGRectDef {
    #[serde(with = "CGPointDef")]
    pub origin: ic::CGPoint,
    #[serde(with = "CGSizeDef")]
    pub size: ic::CGSize,
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "ic::CGPoint")]
pub struct CGPointDef {
    pub x: f64,
    pub y: f64,
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "ic::CGSize")]
pub struct CGSizeDef {
    pub width: f64,
    pub height: f64,
}

impl SerializeAs<ic::CGRect> for CGRectDef {
    fn serialize_as<S>(value: &ic::CGRect, serializer: S) -> Result<S::Ok, S::Error>
    where S: serde::Serializer {
        CGRectDef::serialize(value, serializer)
    }
}

impl<'de> DeserializeAs<'de, ic::CGRect> for CGRectDef {
    fn deserialize_as<D>(deserializer: D) -> Result<ic::CGRect, D::Error>
    where D: Deserializer<'de> {
        CGRectDef::deserialize(deserializer)
    }
}
