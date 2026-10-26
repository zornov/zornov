// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Parameter values: points, colours and the dynamic [`Value`] used by
//! effects, masks, grades, generators and keyframes.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A 2D point or vector in canvas units (pixels at canvas resolution unless
/// a parameter says otherwise).
#[derive(Clone, Copy, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct Vec2 {
    /// Horizontal component.
    pub x: f64,
    /// Vertical component (down is positive, matching image rows).
    pub y: f64,
}

impl Vec2 {
    /// Creates a vector.
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
    /// `(0, 0)`.
    pub const ZERO: Self = Self::new(0.0, 0.0);
    /// `(1, 1)`.
    pub const ONE: Self = Self::new(1.0, 1.0);
}

/// An RGBA colour. Components are 0..=1, colour channels are sRGB-encoded
/// (what a colour picker shows) and alpha is straight, not premultiplied.
///
/// Colours are stored encoded because that is what users pick and what
/// JSON readers expect; keyframe interpolation converts to linear light
/// first (see [`Rgba::to_linear`]) so fades between colours do not dip.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rgba {
    /// Red, sRGB-encoded.
    pub r: f64,
    /// Green, sRGB-encoded.
    pub g: f64,
    /// Blue, sRGB-encoded.
    pub b: f64,
    /// Straight alpha.
    pub a: f64,
}

impl Rgba {
    /// Creates a colour from sRGB-encoded components.
    pub const fn new(r: f64, g: f64, b: f64, a: f64) -> Self {
        Self { r, g, b, a }
    }
    /// Opaque black.
    pub const BLACK: Self = Self::new(0.0, 0.0, 0.0, 1.0);
    /// Opaque white.
    pub const WHITE: Self = Self::new(1.0, 1.0, 1.0, 1.0);
    /// Fully transparent black.
    pub const TRANSPARENT: Self = Self::new(0.0, 0.0, 0.0, 0.0);

    /// Converts the colour channels to linear light (IEC 61966-2-1 sRGB curve).
    pub fn to_linear(self) -> [f64; 4] {
        [srgb_to_linear(self.r), srgb_to_linear(self.g), srgb_to_linear(self.b), self.a]
    }

    /// Builds a colour from linear-light channels and straight alpha.
    pub fn from_linear(c: [f64; 4]) -> Self {
        Self::new(linear_to_srgb(c[0]), linear_to_srgb(c[1]), linear_to_srgb(c[2]), c[3])
    }
}

impl Default for Rgba {
    fn default() -> Self {
        Self::BLACK
    }
}

fn srgb_to_linear(v: f64) -> f64 {
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

fn linear_to_srgb(v: f64) -> f64 {
    if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.max(0.0).powf(1.0 / 2.4) - 0.055 }
}

/// A dynamically typed parameter value.
///
/// Effect, mask, grade, transition and generator parameters are declared by
/// the crates that implement them (`fx`, `colour`, `render`), so the model
/// stores them by name as `Value`s rather than as fixed structs. Only
/// [`Value::Scalar`], [`Value::Point`], [`Value::Colour`] and
/// [`Value::Angle`] can be keyframed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Value {
    /// On/off switch.
    Bool(bool),
    /// A plain number.
    Scalar(f64),
    /// A 2D point or size.
    Point(Vec2),
    /// A colour, interpolated in linear light.
    Colour(Rgba),
    /// An angle in degrees; tracks may interpolate along the shortest path.
    Angle(f64),
    /// Free text.
    Text(String),
    /// One option out of a fixed list, by its name.
    Choice(String),
    /// A list of `[x, y]` control points, e.g. a tone curve.
    Curve(Vec<[f64; 2]>),
}

impl Value {
    /// Whether values of this kind can carry a keyframe track.
    pub fn is_animatable(&self) -> bool {
        matches!(self, Value::Scalar(_) | Value::Point(_) | Value::Colour(_) | Value::Angle(_))
    }

    /// Whether `self` and `other` are the same kind of value.
    pub fn same_kind(&self, other: &Value) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }

    /// The number as `f64`, for scalars and angles.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Scalar(v) | Value::Angle(v) => Some(*v),
            _ => None,
        }
    }

    /// The point, for point values.
    pub fn as_point(&self) -> Option<Vec2> {
        match self {
            Value::Point(p) => Some(*p),
            _ => None,
        }
    }

    /// The colour, for colour values.
    pub fn as_colour(&self) -> Option<Rgba> {
        match self {
            Value::Colour(c) => Some(*c),
            _ => None,
        }
    }

    /// The flag, for booleans.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// The string, for text and choice values.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Text(s) | Value::Choice(s) => Some(s),
            _ => None,
        }
    }

    /// Splits an animatable value into interpolation components.
    /// Colours are converted to linear light here.
    pub(crate) fn components(&self) -> Option<([f64; 4], usize)> {
        Some(match self {
            Value::Scalar(v) | Value::Angle(v) => ([*v, 0.0, 0.0, 0.0], 1),
            Value::Point(p) => ([p.x, p.y, 0.0, 0.0], 2),
            Value::Colour(c) => (c.to_linear(), 4),
            _ => return None,
        })
    }

    /// Rebuilds a value of the same kind as `self` from components.
    pub(crate) fn with_components(&self, c: [f64; 4]) -> Value {
        match self {
            Value::Scalar(_) => Value::Scalar(c[0]),
            Value::Angle(_) => Value::Angle(c[0]),
            Value::Point(_) => Value::Point(Vec2::new(c[0], c[1])),
            Value::Colour(_) => Value::Colour(Rgba::from_linear(c)),
            other => other.clone(),
        }
    }
}

impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Value::Scalar(v)
    }
}
impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Value::Bool(v)
    }
}
impl From<Vec2> for Value {
    fn from(v: Vec2) -> Self {
        Value::Point(v)
    }
}
impl From<Rgba> for Value {
    fn from(v: Rgba) -> Self {
        Value::Colour(v)
    }
}

/// Named parameters, sorted by name so files diff cleanly.
pub type ParamMap = BTreeMap<String, Value>;

/// Free-form JSON fields kept for forward compatibility: anything a newer
/// version wrote that this version does not understand is stored here on
/// load and written back unchanged on save.
pub type Extra = serde_json::Map<String, serde_json::Value>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_round_trip() {
        for v in [0.0, 0.01, 0.2, 0.5, 0.9, 1.0] {
            let c = Rgba::new(v, v, v, 1.0);
            let back = Rgba::from_linear(c.to_linear());
            assert!((back.r - v).abs() < 1e-9);
        }
    }

    #[test]
    fn value_json_shape() {
        let v = Value::Point(Vec2::new(1.0, 2.0));
        let s = serde_json::to_string(&v).unwrap();
        assert_eq!(s, r#"{"point":{"x":1.0,"y":2.0}}"#);
        assert_eq!(serde_json::from_str::<Value>(&s).unwrap(), v);
    }
}
