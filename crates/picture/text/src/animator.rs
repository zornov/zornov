// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Text animators: a set of property values (move,
//! turn, scale, fade, colour, blur, tracking) applied to the characters,
//! words or lines a selector picks, by how much it picks each one.
//!
//! * A **range selector** picks a stretch of the units: `start`, `end`
//!   and `offset` as fractions of the text (0..1), each a number or keys
//!   over clip time; `shape` (square, ramp_up, ramp_down, triangle,
//!   round, smooth) says how the pick falls off at its edges; `order`
//!   counts the units forwards, backwards, from the centre or at random;
//!   `amount` scales the whole pick (negative pushes the other way).
//!   Keying `offset` from -1 to 1 sweeps the change across the text.
//! * A **wiggly selector** makes the pick wander per unit over time
//!   (`per_second`, `correlation`, `min`, `max`), alone or `mode`
//!   `intersect` (wobble inside the range) / `add`.
//!
//! The values are where a fully picked unit ends up: `position` [x, y]
//! (canvas pixels), `rotation` (degrees), `scale` (factor, or [x, y]),
//! `opacity` (0..1, e.g. 0 to hide what is picked), `colour`, `blur`
//! (pixels), `tracking` (extra pixels between picked units).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::anim::Order;
use crate::geom::Ease;
use crate::paint::UnitPose;
use crate::rgba::Rgba;
use crate::style::TextUnit;

/// A number, or keys `[[t, v], [t, v, "ease"], …]` over seconds into the
/// clip.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Keyed {
    Num(f32),
    Keys(Vec<Vec<Value>>),
}

impl Default for Keyed {
    fn default() -> Self {
        Keyed::Num(0.0)
    }
}

fn ease_named(s: &str) -> Ease {
    serde_json::from_value(Value::String(s.to_owned())).unwrap_or(Ease::Smooth)
}

impl Keyed {
    pub fn at(&self, t: f32) -> f32 {
        match self {
            Keyed::Num(v) => *v,
            Keyed::Keys(ks) => {
                let keys: Vec<(f32, f32, Ease)> = ks
                    .iter()
                    .filter_map(|k| Some((k.first()?.as_f64()? as f32, k.get(1)?.as_f64()? as f32, k.get(2).and_then(Value::as_str).map(ease_named).unwrap_or(Ease::Smooth))))
                    .collect();
                let Some(first) = keys.first() else { return 0.0 };
                if t <= first.0 {
                    return first.1;
                }
                for w in keys.windows(2) {
                    if t <= w[1].0 {
                        let u = (t - w[0].0) / (w[1].0 - w[0].0).max(1e-6);
                        return w[0].1 + (w[1].1 - w[0].1) * w[1].2.apply(u);
                    }
                }
                keys[keys.len() - 1].1
            }
        }
    }

    pub fn is_zero(&self) -> bool {
        matches!(self, Keyed::Num(v) if *v == 0.0)
    }
}

fn one() -> Keyed {
    Keyed::Num(1.0)
}

/// How a range selector's pick falls off.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RangeShape {
    /// All or nothing, with partial units at the edges (smooth sweeps).
    #[default]
    Square,
    /// Rising from nothing at the start to all at the end.
    RampUp,
    RampDown,
    /// Most in the middle of the range.
    Triangle,
    Round,
    /// A soft bell.
    Smooth,
}

/// A range selector.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Range {
    #[serde(default)]
    pub start: Keyed,
    #[serde(default = "one")]
    pub end: Keyed,
    #[serde(default)]
    pub offset: Keyed,
    #[serde(default)]
    pub shape: RangeShape,
    #[serde(default)]
    pub order: Order,
    #[serde(default = "one")]
    pub amount: Keyed,
    /// Ease of the pick at its edges, 0..1 (square shape only).
    #[serde(default)]
    pub smoothness: f32,
}

impl Default for Range {
    fn default() -> Self {
        Range { start: Keyed::Num(0.0), end: Keyed::Num(1.0), offset: Keyed::Num(0.0), shape: RangeShape::Square, order: Order::Forward, amount: Keyed::Num(1.0), smoothness: 0.0 }
    }
}

/// How a wiggly selector combines with the range.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WiggleMode {
    /// The wobble only where the range picks.
    #[default]
    Intersect,
    Add,
}

/// A wiggly selector.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Wiggly {
    #[serde(default = "two")]
    pub per_second: f32,
    /// 0: every unit on its own, 1: all together.
    #[serde(default)]
    pub correlation: f32,
    #[serde(default = "minus_one")]
    pub min: f32,
    #[serde(default = "one_f")]
    pub max: f32,
    #[serde(default)]
    pub seed: u32,
    #[serde(default)]
    pub mode: WiggleMode,
}

fn two() -> f32 {
    2.0
}
fn minus_one() -> f32 {
    -1.0
}
fn one_f() -> f32 {
    1.0
}

/// One animator: what it changes and which units it changes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Animator {
    #[serde(default)]
    pub unit: TextUnit,
    #[serde(default)]
    pub range: Option<Range>,
    #[serde(default)]
    pub wiggle: Option<Wiggly>,
    /// Canvas pixels.
    #[serde(default)]
    pub position: Option<[f32; 2]>,
    /// Degrees.
    #[serde(default)]
    pub rotation: Option<f32>,
    /// A factor, or `[x, y]`.
    #[serde(default)]
    pub scale: Option<Scale>,
    #[serde(default)]
    pub opacity: Option<f32>,
    #[serde(default)]
    pub colour: Option<Rgba>,
    /// Canvas pixels of blur.
    #[serde(default)]
    pub blur: Option<f32>,
    /// Extra canvas pixels after each picked unit.
    #[serde(default)]
    pub tracking: Option<f32>,
}

/// A uniform or per-axis scale.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Scale {
    Both(f32),
    Axes([f32; 2]),
}

impl Scale {
    fn xy(self) -> [f32; 2] {
        match self {
            Scale::Both(s) => [s, s],
            Scale::Axes(a) => a,
        }
    }
}

fn hash(i: u32) -> u32 {
    let mut x = i.wrapping_mul(0x9E37_79B1) ^ 0x85EB_CA6B;
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    x
}

/// Smooth 1D gradient noise, about -0.5..0.5.
fn noise(x: f32, seed: u32) -> f32 {
    let i = x.floor();
    let f = x - i;
    let g = |k: f32| (hash((k as i64 as u32).wrapping_add(seed.wrapping_mul(7919))) as f32 / u32::MAX as f32) * 2.0 - 1.0;
    let u = f * f * (3.0 - 2.0 * f);
    g(i) * f * (1.0 - u) + g(i + 1.0) * (f - 1.0) * u
}

impl Range {
    /// How much unit `index` of `count` is picked at time `t`, 0..1
    /// (times `amount`).
    pub fn pick(&self, t: f32, index: usize, count: usize) -> f32 {
        if count == 0 {
            return 0.0;
        }
        let n = count as f32;
        let slot = match self.order {
            Order::Forward => index as f32,
            Order::Backward => (count - 1 - index) as f32,
            Order::Centre => {
                // Distance from the middle, the middle first.
                let mid = (n - 1.0) * 0.5;
                let mut ranks: Vec<(f32, usize)> = (0..count).map(|j| ((j as f32 - mid).abs(), j)).collect();
                ranks.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
                ranks.iter().position(|r| r.1 == index).unwrap_or(index) as f32
            }
            Order::Random => {
                let mine = hash(index as u32);
                (0..count).filter(|&j| hash(j as u32) < mine || (hash(j as u32) == mine && j < index)).count() as f32
            }
        };
        let off = self.offset.at(t);
        let (mut s, mut e) = (self.start.at(t) + off, self.end.at(t) + off);
        if s > e {
            std::mem::swap(&mut s, &mut e);
        }
        let (lo, hi) = (slot / n, (slot + 1.0) / n);
        let amount = self.amount.at(t);
        let v = match self.shape {
            RangeShape::Square => {
                let cover = ((hi.min(e) - lo.max(s)).max(0.0) * n).clamp(0.0, 1.0);
                if self.smoothness > 0.0 {
                    let k = cover;
                    let sm = k * k * (3.0 - 2.0 * k);
                    k + (sm - k) * self.smoothness.clamp(0.0, 1.0)
                } else {
                    cover
                }
            }
            shape => {
                let u = (lo + hi) * 0.5;
                if e - s <= 1e-6 || u < s || u > e {
                    0.0
                } else {
                    let x = (u - s) / (e - s);
                    match shape {
                        RangeShape::RampUp => x,
                        RangeShape::RampDown => 1.0 - x,
                        RangeShape::Triangle => 1.0 - (2.0 * x - 1.0).abs(),
                        RangeShape::Round => (1.0 - (2.0 * x - 1.0).powi(2)).max(0.0).sqrt(),
                        _ => (((2.0 * x - 1.0) * std::f32::consts::PI).cos() + 1.0) * 0.5,
                    }
                }
            }
        };
        v * amount
    }
}

impl Wiggly {
    fn pick(&self, t: f32, index: usize) -> f32 {
        let spread = (1.0 - self.correlation.clamp(0.0, 1.0)) * 5.3;
        let w = noise(t * self.per_second + index as f32 * spread, self.seed) * 2.0;
        let u = (w.clamp(-1.0, 1.0) + 1.0) * 0.5;
        self.min + (self.max - self.min) * u
    }
}

impl Animator {
    /// How much unit `index` is picked at `t` (any sign).
    pub fn pick(&self, t: f32, index: usize, count: usize) -> f32 {
        let r = match &self.range {
            Some(r) => Some(r.pick(t, index, count)),
            None if self.wiggle.is_none() => Some(Range::default().pick(t, index, count)),
            None => None,
        };
        match (r, &self.wiggle) {
            (Some(r), None) => r,
            (None, Some(w)) => w.pick(t, index),
            (Some(r), Some(w)) => match w.mode {
                WiggleMode::Intersect => r * w.pick(t, index),
                WiggleMode::Add => (r + w.pick(t, index)).clamp(-1.0, 1.0),
            },
            (None, None) => 0.0,
        }
    }

    /// The change to unit `index` of `count` at `t`. `px` is pixels per
    /// canvas pixel of the picture being drawn.
    pub fn pose(&self, t: f32, index: usize, count: usize, px: f32) -> UnitPose {
        let s = self.pick(t, index, count);
        let mut p = UnitPose::REST;
        if s == 0.0 && self.tracking.is_none() {
            return p;
        }
        if let Some(v) = self.position {
            p.offset = [v[0] * s * px, v[1] * s * px];
        }
        if let Some(r) = self.rotation {
            p.rotation = r * s;
        }
        if let Some(sc) = self.scale {
            let k = sc.xy();
            p.scale = [1.0 + (k[0] - 1.0) * s, 1.0 + (k[1] - 1.0) * s];
        }
        if let Some(o) = self.opacity {
            p.opacity = (1.0 + (o - 1.0) * s).clamp(0.0, 1.0);
        }
        if let Some(c) = self.colour
            && s > 0.0
        {
            // The painter mixes towards a colour by its alpha.
            p.colour = Some(Rgba::new(c.r, c.g, c.b, (c.a * s).clamp(0.0, 1.0)));
        }
        if let Some(tr) = self.tracking {
            // Every picked unit before this one pushes it along; the whole
            // line is kept centred.
            let picks: Vec<f32> = (0..count).map(|j| self.pick(t, j, count)).collect();
            let before: f32 = picks[..index.min(count)].iter().sum::<f32>() + picks.get(index).copied().unwrap_or(0.0) * 0.5;
            let total: f32 = picks.iter().sum();
            p.offset[0] += (before - total * 0.5) * tr * px;
        }
        p
    }

    /// Blur of unit `index` in picture pixels.
    pub fn blur(&self, t: f32, index: usize, count: usize, px: f32) -> f32 {
        match self.blur {
            Some(b) => (b * self.pick(t, index, count)).abs() * px,
            None => 0.0,
        }
    }
}

/// Animator presets: sweeps and wobbles editors reach for. Each is an
/// animator whose range `offset` is keyed from -1 to 1 over `duration`
/// seconds starting at `start` (or a wiggle).
pub fn preset(name: &str, unit: TextUnit, start: f32, duration: f32) -> Option<Animator> {
    let sweep = |shape: RangeShape| Range {
        start: Keyed::Num(0.0),
        end: Keyed::Num(1.0),
        offset: Keyed::Keys(vec![vec![Value::from(start), Value::from(-1.0)], vec![Value::from(start + duration), Value::from(1.0), Value::from("linear")]]),
        shape,
        ..Default::default()
    };
    let base = Animator { unit, range: None, wiggle: None, position: None, rotation: None, scale: None, opacity: None, colour: None, blur: None, tracking: None };
    Some(match name {
        // A bright band passing along the letters.
        "highlight_sweep" => Animator { range: Some(sweep(RangeShape::Smooth)), scale: Some(Scale::Both(1.15)), colour: Some(Rgba::hex("#ffd166")), ..base },
        // Letters jump as a wave passes.
        "wave_jump" => Animator { range: Some(sweep(RangeShape::Smooth)), position: Some([0.0, -40.0]), ..base },
        // Out of focus, sharpening one by one.
        "focus_in" => Animator {
            range: Some(Range { start: Keyed::Keys(vec![vec![Value::from(start), Value::from(0.0)], vec![Value::from(start + duration), Value::from(1.0), Value::from("out")]]), end: Keyed::Num(1.0), ..Default::default() }),
            blur: Some(24.0),
            opacity: Some(0.0),
            scale: Some(Scale::Both(1.3)),
            ..base
        },
        // Spread out, coming together.
        "tracking_in" => Animator {
            range: Some(Range { amount: Keyed::Keys(vec![vec![Value::from(start), Value::from(1.0)], vec![Value::from(start + duration), Value::from(0.0), Value::from("out")]]), ..Default::default() }),
            tracking: Some(60.0),
            opacity: Some(0.0),
            ..base
        },
        // Nervous letters.
        "jitter" => Animator { wiggle: Some(Wiggly { per_second: 6.0, correlation: 0.0, min: -1.0, max: 1.0, seed: 3, mode: WiggleMode::Intersect }), position: Some([6.0, 6.0]), rotation: Some(8.0), ..base },
        // Slow drift, each letter on its own.
        "float" => Animator { wiggle: Some(Wiggly { per_second: 0.6, correlation: 0.2, min: -1.0, max: 1.0, seed: 9, mode: WiggleMode::Intersect }), position: Some([0.0, 18.0]), rotation: Some(4.0), ..base },
        // Letters falling away, one by one.
        "drop_out" => Animator {
            range: Some(Range { start: Keyed::Num(0.0), end: Keyed::Keys(vec![vec![Value::from(start), Value::from(0.0)], vec![Value::from(start + duration), Value::from(1.0), Value::from("in")]]), shape: RangeShape::Square, smoothness: 1.0, ..Default::default() }),
            position: Some([0.0, 160.0]),
            rotation: Some(40.0),
            opacity: Some(0.0),
            ..base
        },
        _ => return None,
    })
}

/// Animator preset names.
pub const PRESETS: &[&str] = &["highlight_sweep", "wave_jump", "focus_in", "tracking_in", "jitter", "float", "drop_out"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_pick_the_right_units() {
        let r = Range { start: Keyed::Num(0.0), end: Keyed::Num(0.5), ..Default::default() };
        assert_eq!(r.pick(0.0, 0, 4), 1.0);
        assert_eq!(r.pick(0.0, 1, 4), 1.0);
        assert_eq!(r.pick(0.0, 2, 4), 0.0);
        // Partial units at the edge.
        let r = Range { end: Keyed::Num(0.375), ..Default::default() };
        assert!((r.pick(0.0, 1, 4) - 0.5).abs() < 1e-6);
        // Offset slides the range.
        let r = Range { end: Keyed::Num(0.25), offset: Keyed::Num(0.75), ..Default::default() };
        assert_eq!(r.pick(0.0, 3, 4), 1.0);
        assert_eq!(r.pick(0.0, 0, 4), 0.0);
        // Backward counts from the end.
        let r = Range { end: Keyed::Num(0.25), order: Order::Backward, ..Default::default() };
        assert_eq!(r.pick(0.0, 3, 4), 1.0);
        // Triangle peaks in the middle.
        let r = Range { shape: RangeShape::Triangle, ..Default::default() };
        assert!(r.pick(0.0, 2, 5) > r.pick(0.0, 0, 5));
        // Keys over time.
        let r = Range { end: Keyed::Keys(vec![vec![Value::from(0.0), Value::from(0.0)], vec![Value::from(1.0), Value::from(1.0), Value::from("linear")]]), ..Default::default() };
        assert_eq!(r.pick(0.5, 1, 4), 1.0);
        assert_eq!(r.pick(0.5, 2, 4), 0.0);
    }

    #[test]
    fn animators_move_picked_units_and_track() {
        let a: Animator = serde_json::from_value(serde_json::json!({"unit": "character", "range": {"end": 0.5}, "position": [0, -20], "opacity": 0.0})).unwrap();
        let p = a.pose(0.0, 0, 4, 2.0);
        assert_eq!(p.offset, [0.0, -40.0]);
        assert_eq!(p.opacity, 0.0);
        assert!(a.pose(0.0, 3, 4, 2.0).is_rest());
        let t: Animator = serde_json::from_value(serde_json::json!({"tracking": 10})).unwrap();
        let first = t.pose(0.0, 0, 5, 1.0).offset[0];
        let last = t.pose(0.0, 4, 5, 1.0).offset[0];
        assert!(first < 0.0 && last > 0.0 && (first + last).abs() < 1e-4, "{first} {last}");
        let b: Animator = serde_json::from_value(serde_json::json!({"blur": 10, "range": {"end": 0.5}})).unwrap();
        assert_eq!(b.blur(0.0, 0, 2, 1.5), 15.0);
        assert_eq!(b.blur(0.0, 1, 2, 1.5), 0.0);
    }

    #[test]
    fn wiggles_wander_within_their_bounds() {
        let a: Animator = serde_json::from_value(serde_json::json!({"wiggle": {"per_second": 3}, "rotation": 20})).unwrap();
        let mut seen = Vec::new();
        for k in 0..60 {
            let r = a.pose(k as f32 / 30.0, 2, 6, 1.0).rotation;
            assert!(r.abs() <= 20.0 + 1e-4);
            seen.push(r);
        }
        assert!(seen.iter().any(|r| r.abs() > 1.0));
        for name in PRESETS {
            assert!(preset(name, TextUnit::Character, 0.0, 1.0).is_some(), "{name}");
        }
    }
}
