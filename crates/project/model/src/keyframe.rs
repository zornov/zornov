// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Keyframe tracks and interpolation.
//!
//! A [`Track`] is a list of keys sorted by time. Times are relative to the
//! start of the clip that owns the track, so moving a clip moves its
//! animation with it. Between two keys the *left* key's [`Interp`] decides
//! the curve. Before the first key the first value holds; after the last
//! key the last value holds.
//!
//! Point tracks (a clip's position) can also bend in space: a key's
//! [`Keyframe::path`] handles turn the straight line to the next key into
//! a Bézier curve, travelled at an even speed along its length.

use crate::modifier::{self, Modifier};
use crate::time::Micros;
use crate::value::Value;
use serde::{Deserialize, Serialize};

/// How a value travels from one key to the next.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Interp {
    /// Keep this key's value until the next key, then jump.
    Hold,
    /// Straight line.
    #[default]
    Linear,
    /// Cubic Bézier timing curve shaped by this key's `ease_out` and the
    /// next key's `ease_in` handles.
    Ease,
    /// Smooth curve through neighbouring keys (Catmull-Rom tangents),
    /// clamped so it never overshoots the key values.
    Auto,
    /// A named timing curve from this key's [`Keyframe::curve`]: elastic,
    /// bounce, back, expo, steps…
    Curve,
}

/// Which end of a move a [`Curve`] shapes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CurveEnd {
    /// The departure (a spring winds up, a bounce leaves).
    In,
    /// The arrival (the common choice: springs and bounces land).
    #[default]
    Out,
    /// Both ends, meeting in the middle.
    InOut,
}

/// Timing curves beyond a single Bézier (the curves motion designers know
/// from common animation tools).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum Curve {
    /// `u^power` shapes: 2 quad, 3 cubic, 4 quart, 5 quint.
    Power {
        #[serde(default = "two")]
        power: f64,
        #[serde(default)]
        end: CurveEnd,
    },
    /// Exponential: very slow, then very fast.
    Expo {
        #[serde(default)]
        end: CurveEnd,
    },
    /// Circular.
    Circ {
        #[serde(default)]
        end: CurveEnd,
    },
    /// Sine.
    Sine {
        #[serde(default)]
        end: CurveEnd,
    },
    /// Goes past the target by `overshoot` (1.70 is the classic) and back.
    Back {
        #[serde(default = "back")]
        overshoot: f64,
        #[serde(default)]
        end: CurveEnd,
    },
    /// A spring: rings around the target and settles. `amplitude` ≥ 1
    /// swings further, a smaller `period` (0.1..1) rings faster.
    Elastic {
        #[serde(default = "one")]
        amplitude: f64,
        #[serde(default = "period")]
        period: f64,
        #[serde(default)]
        end: CurveEnd,
    },
    /// Drops and bounces on the target like a ball.
    Bounce {
        #[serde(default)]
        end: CurveEnd,
    },
    /// Jumps in `count` even steps (stop-motion, ticking counters).
    Steps {
        #[serde(default = "four")]
        count: u32,
    },
}

fn one() -> f64 {
    1.0
}
fn two() -> f64 {
    2.0
}
fn back() -> f64 {
    1.70158
}
fn period() -> f64 {
    0.3
}
fn four() -> u32 {
    4
}

impl Default for Curve {
    fn default() -> Self {
        Curve::Power { power: 3.0, end: CurveEnd::Out }
    }
}

impl Curve {
    /// Progress (a fraction of the change, which may leave 0..=1 on the
    /// way) at time fraction `u` in 0..=1. Exactly 0 at 0 and 1 at 1.
    pub fn apply(self, u: f64) -> f64 {
        let u = u.clamp(0.0, 1.0);
        if u <= 0.0 {
            return 0.0;
        }
        if u >= 1.0 {
            return 1.0;
        }
        let (out, end): (&dyn Fn(f64) -> f64, CurveEnd) = match self {
            Curve::Power { power, end } => {
                let p = power.clamp(0.1, 20.0);
                (&move |u: f64| 1.0 - (1.0 - u).powf(p), end)
            }
            Curve::Expo { end } => (&|u: f64| (1.0 - 2f64.powf(-10.0 * u)) / (1.0 - 2f64.powf(-10.0)), end),
            Curve::Circ { end } => (&|u: f64| (1.0 - (u - 1.0) * (u - 1.0)).max(0.0).sqrt(), end),
            Curve::Sine { end } => (&|u: f64| (u * std::f64::consts::FRAC_PI_2).sin(), end),
            Curve::Back { overshoot, end } => {
                let s = overshoot.clamp(0.0, 10.0);
                (&move |u: f64| {
                    let v = u - 1.0;
                    1.0 + (s + 1.0) * v * v * v + s * v * v
                }, end)
            }
            Curve::Elastic { amplitude, period, end } => {
                let a = amplitude.max(1.0);
                let p = period.clamp(0.05, 2.0);
                let shift = p / std::f64::consts::TAU * (1.0 / a).asin();
                (&move |u: f64| 1.0 + a * 2f64.powf(-10.0 * u) * ((u - shift) * std::f64::consts::TAU / p).sin(), end)
            }
            Curve::Bounce { end } => (&bounce_out, end),
            Curve::Steps { count } => return (u * count.max(1) as f64).floor() / count.max(1) as f64,
        };
        match end {
            CurveEnd::Out => out(u),
            CurveEnd::In => 1.0 - out(1.0 - u),
            CurveEnd::InOut => {
                if u < 0.5 {
                    0.5 * (1.0 - out(1.0 - 2.0 * u))
                } else {
                    0.5 + 0.5 * out(2.0 * u - 1.0)
                }
            }
        }
    }
}

fn bounce_out(u: f64) -> f64 {
    const N: f64 = 7.5625;
    const D: f64 = 2.75;
    if u < 1.0 / D {
        N * u * u
    } else if u < 2.0 / D {
        let u = u - 1.5 / D;
        N * u * u + 0.75
    } else if u < 2.5 / D {
        let u = u - 2.25 / D;
        N * u * u + 0.9375
    } else {
        let u = u - 2.625 / D;
        N * u * u + 0.984375
    }
}

/// Bézier handles bending a point track's path between keys, as offsets
/// from the key's point in the track's units (canvas pixels for position).
#[derive(Clone, Copy, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct PathHandles {
    /// Handle arriving at this key from the previous one.
    #[serde(default, rename = "in")]
    pub inn: [f64; 2],
    /// Handle leaving this key towards the next one.
    #[serde(default)]
    pub out: [f64; 2],
}

/// A Bézier handle for [`Interp::Ease`], in units of the segment: `x` is a
/// fraction of the segment's duration (clamped to 0..=1), `y` a fraction of
/// its value change (values outside 0..=1 overshoot on purpose).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Handle {
    /// Influence along time.
    pub x: f64,
    /// Influence along value.
    pub y: f64,
}

impl Default for Handle {
    /// A gentle ease (the classic ease-in-out shape when used on both ends).
    fn default() -> Self {
        Self { x: 0.42, y: 0.0 }
    }
}

/// One key on a [`Track`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Keyframe {
    /// Time relative to the clip start.
    pub time: Micros,
    /// Value at this key; must be animatable and the same kind as the
    /// other keys on the track.
    pub value: Value,
    /// Curve towards the next key.
    #[serde(default)]
    pub interp: Interp,
    /// Handle arriving at this key (used when the previous key eases).
    #[serde(default)]
    pub ease_in: Handle,
    /// Handle leaving this key (used when this key eases).
    #[serde(default)]
    pub ease_out: Handle,
    /// The timing curve towards the next key when `interp` is
    /// [`Interp::Curve`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub curve: Option<Curve>,
    /// For point tracks: how the path bends into and out of this key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathHandles>,
}

impl Keyframe {
    /// A key with the given interpolation and default handles.
    pub fn new(time: Micros, value: impl Into<Value>, interp: Interp) -> Self {
        Self { time, value: value.into(), interp, ease_in: Handle::default(), ease_out: Handle::default(), curve: None, path: None }
    }

    /// A key that moves on to the next along `curve`.
    pub fn with_curve(time: Micros, value: impl Into<Value>, curve: Curve) -> Self {
        Self { curve: Some(curve), ..Self::new(time, value, Interp::Curve) }
    }
}

/// A keyframe track for one property.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct Track {
    /// Keys sorted by strictly increasing time.
    pub keys: Vec<Keyframe>,
    /// For angle tracks: turn the short way round (e.g. 350° → 10° goes
    /// through 0°, not back through 180°).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub shortest_angle: bool,
    /// Procedural layers over the keys (loop, wobble, step…), in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modifiers: Vec<Modifier>,
}

/// Why a track is malformed.
#[derive(Clone, Debug, PartialEq)]
pub enum TrackError {
    /// The track has no keys.
    Empty,
    /// Key times are not strictly increasing.
    Unsorted,
    /// A key holds a value that cannot be animated.
    NotAnimatable,
    /// A modifier's settings can't run (the message says why).
    BadModifier(String),
    /// Keys hold different kinds of values.
    MixedKinds,
}

impl Track {
    /// A track from keys (sorted here for convenience).
    pub fn new(mut keys: Vec<Keyframe>) -> Self {
        keys.sort_by_key(|k| k.time);
        Self { keys, shortest_angle: false, modifiers: Vec::new() }
    }

    /// Checks the track's structural rules.
    pub fn validate(&self) -> Result<(), TrackError> {
        let first = self.keys.first().ok_or(TrackError::Empty)?;
        if !first.value.is_animatable() {
            return Err(TrackError::NotAnimatable);
        }
        for w in self.keys.windows(2) {
            if w[1].time <= w[0].time {
                return Err(TrackError::Unsorted);
            }
        }
        if self.keys.iter().any(|k| !k.value.same_kind(&first.value)) {
            return Err(TrackError::MixedKinds);
        }
        if let Some(p) = self.modifiers.iter().find_map(Modifier::problem) {
            return Err(TrackError::BadModifier(p));
        }
        Ok(())
    }

    /// Inserts `key`, replacing any key at the same time.
    pub fn set_key(&mut self, key: Keyframe) {
        match self.keys.binary_search_by_key(&key.time, |k| k.time) {
            Ok(i) => self.keys[i] = key,
            Err(i) => self.keys.insert(i, key),
        }
    }

    /// Removes the key at exactly `time`, returning it.
    pub fn remove_key(&mut self, time: Micros) -> Option<Keyframe> {
        let i = self.keys.binary_search_by_key(&time, |k| k.time).ok()?;
        Some(self.keys.remove(i))
    }

    /// The key at exactly `time`.
    pub fn key_at(&self, time: Micros) -> Option<&Keyframe> {
        let i = self.keys.binary_search_by_key(&time, |k| k.time).ok()?;
        Some(&self.keys[i])
    }

    /// Moves every key by `delta` (used when a clip's start is trimmed so
    /// the animation stays attached to the same media frames).
    pub fn shift(&mut self, delta: Micros) {
        for k in &mut self.keys {
            k.time += delta;
        }
    }

    /// The value at clip-relative time `t`, or `None` for an empty track:
    /// the keys, then the [`Modifier`]s.
    pub fn eval(&self, t: Micros) -> Option<Value> {
        if self.modifiers.is_empty() {
            return self.eval_keys(t);
        }
        let (first, last) = (self.keys.first()?.time, self.keys.last()?.time);
        // Time modifiers pick the moment; spans counts how many key spans
        // a repeat with offset has carried the value on by.
        let (mut at, mut spans) = (t, 0i64);
        for m in self.modifiers.iter().filter(|m| m.is_time()) {
            let (t2, n) = modifier::remap_time(m, at, first, last);
            at = t2;
            spans += n;
        }
        let v = self.eval_keys(at)?;
        let Some((mut c, dims)) = v.components() else { return Some(v) };
        if spans != 0 {
            let (a, _) = self.keys[0].value.components()?;
            let (b, _) = self.keys[self.keys.len() - 1].value.components()?;
            for d in 0..dims {
                c[d] += spans as f64 * (b[d] - a[d]);
            }
        }
        for m in self.modifiers.iter().filter(|m| !m.is_time()) {
            modifier::apply_value(m, &mut c, dims, t, self);
        }
        Some(v.with_components(c))
    }

    /// Whether the track's value can change over time (more than one key,
    /// or a modifier).
    pub fn varies(&self) -> bool {
        self.keys.len() > 1 || !self.modifiers.is_empty()
    }

    /// The value the keys alone give at `t`.
    pub(crate) fn eval_keys(&self, t: Micros) -> Option<Value> {
        let keys = &self.keys;
        let first = keys.first()?;
        let last = keys.last()?;
        if t <= first.time || keys.len() == 1 {
            return Some(first.value.clone());
        }
        if t >= last.time {
            return Some(last.value.clone());
        }
        // Index of the segment's left key.
        let i = match keys.binary_search_by_key(&t, |k| k.time) {
            Ok(i) => return Some(keys[i].value.clone()),
            Err(i) => i - 1,
        };
        let (k0, k1) = (&keys[i], &keys[i + 1]);
        // The keys around the segment are all it reads (a smooth curve's
        // tangents look one key further each way): a track of thousands of
        // keys is not turned into numbers whole for every look.
        let lo = i.saturating_sub(1);
        let comps = self.unwrapped_components(lo, (i + 2).min(keys.len() - 1))?;
        let (c0, dims) = comps.at(i);
        let (c1, _) = comps.at(i + 1);
        let u = (t - k0.time) as f64 / (k1.time - k0.time) as f64;
        if let Some(bend) = self.bend(i, &comps) {
            // A curved path: the timing curve gives the distance travelled.
            let p = match k0.interp {
                Interp::Hold => return Some(k0.value.clone()),
                Interp::Linear => u,
                Interp::Ease => bezier_progress(u, k0.ease_out, k1.ease_in),
                Interp::Curve => k0.curve.unwrap_or_default().apply(u),
                // Even speed along the path, easing off the first key and
                // into the last.
                Interp::Auto => hermite(0.0, 1.0, if i == 0 { 0.0 } else { 1.0 }, if i + 2 == keys.len() { 0.0 } else { 1.0 }, u),
            };
            let at = bend.at(p);
            return Some(k0.value.with_components([at[0], at[1], 0.0, 0.0]));
        }
        let out = match k0.interp {
            Interp::Hold => return Some(k0.value.clone()),
            Interp::Linear => lerp4(c0, c1, u),
            Interp::Ease => lerp4(c0, c1, bezier_progress(u, k0.ease_out, k1.ease_in)),
            Interp::Curve => lerp4(c0, c1, k0.curve.unwrap_or_default().apply(u)),
            Interp::Auto => {
                let mut out = [0.0; 4];
                for (d, slot) in out.iter_mut().enumerate().take(dims) {
                    let tangents = clamped_tangents(keys, &comps, d, i);
                    let h = (k1.time - k0.time) as f64;
                    *slot = hermite(c0[d], c1[d], tangents.0 * h, tangents.1 * h, u);
                }
                out
            }
        };
        Some(k0.value.with_components(out))
    }

    /// The curved path from key `i` to the next, when either end has path
    /// handles and the track holds points.
    fn bend(&self, i: usize, comps: &Window) -> Option<Bend> {
        let (k0, k1) = (&self.keys[i], &self.keys[i + 1]);
        if comps.at(i).1 != 2 || !matches!(k0.value, Value::Point(_)) {
            return None;
        }
        let out = k0.path.map(|p| p.out).unwrap_or_default();
        let inn = k1.path.map(|p| p.inn).unwrap_or_default();
        if out == [0.0, 0.0] && inn == [0.0, 0.0] {
            return None;
        }
        let (a, b) = (comps.at(i).0, comps.at(i + 1).0);
        Some(Bend::new([[a[0], a[1]], [a[0] + out[0], a[1] + out[1]], [b[0] + inn[0], b[1] + inn[1]], [b[0], b[1]]]))
    }

    /// Components for keys `lo..=hi` (at most four); angle tracks with
    /// `shortest_angle` are unwrapped so consecutive keys differ by at most
    /// half a turn (which walks every key before them).
    fn unwrapped_components(&self, lo: usize, hi: usize) -> Option<Window> {
        let unwrap = self.shortest_angle && matches!(self.keys[0].value, Value::Angle(_));
        let mut w = Window { lo, comps: [([0.0; 4], 0); 4] };
        let mut prev: Option<[f64; 4]> = None;
        for (j, k) in self.keys.iter().enumerate().take(hi + 1).skip(if unwrap { 0 } else { lo }) {
            let (mut c, n) = k.value.components()?;
            if unwrap && let Some(p) = prev {
                let mut d = (c[0] - p[0]).rem_euclid(360.0);
                if d > 180.0 {
                    d -= 360.0;
                }
                c[0] = p[0] + d;
            }
            prev = Some(c);
            if j >= lo {
                w.comps[j - lo] = (c, n);
            }
        }
        Some(w)
    }
}

/// A cubic Bézier path walked by distance, not by its parameter, so a
/// clip moves along it at the speed its timing curve says.
struct Bend {
    pts: [[f64; 2]; 4],
    /// Length travelled at each of the samples along the parameter.
    lengths: [f64; Bend::SAMPLES + 1],
}

impl Bend {
    const SAMPLES: usize = 48;

    fn new(pts: [[f64; 2]; 4]) -> Self {
        let mut lengths = [0.0; Self::SAMPLES + 1];
        let mut prev = pts[0];
        for (j, l) in lengths.iter_mut().enumerate().skip(1) {
            let p = Self::point(&pts, j as f64 / Self::SAMPLES as f64);
            *l = (p[0] - prev[0]).hypot(p[1] - prev[1]);
            prev = p;
        }
        for j in 1..=Self::SAMPLES {
            lengths[j] += lengths[j - 1];
        }
        Self { pts, lengths }
    }

    fn point(p: &[[f64; 2]; 4], s: f64) -> [f64; 2] {
        let m = 1.0 - s;
        let (a, b, c, d) = (m * m * m, 3.0 * m * m * s, 3.0 * m * s * s, s * s * s);
        [a * p[0][0] + b * p[1][0] + c * p[2][0] + d * p[3][0], a * p[0][1] + b * p[1][1] + c * p[2][1] + d * p[3][1]]
    }

    /// The point a fraction `f` of the way along (past either end it
    /// carries on along the end's direction, so overshooting curves
    /// overshoot along the path).
    fn at(&self, f: f64) -> [f64; 2] {
        let total = self.lengths[Self::SAMPLES];
        if total <= 1e-12 {
            return self.pts[0];
        }
        let beyond = |from: [f64; 2], towards: [f64; 2], d: f64| {
            let (dx, dy) = (towards[0] - from[0], towards[1] - from[1]);
            let n = dx.hypot(dy).max(1e-12);
            [from[0] + dx / n * d, from[1] + dy / n * d]
        };
        if f <= 0.0 {
            let ahead = Self::point(&self.pts, 1.0 / Self::SAMPLES as f64);
            return beyond(self.pts[0], [2.0 * self.pts[0][0] - ahead[0], 2.0 * self.pts[0][1] - ahead[1]], -f * total);
        }
        if f >= 1.0 {
            let behind = Self::point(&self.pts, 1.0 - 1.0 / Self::SAMPLES as f64);
            return beyond(self.pts[3], [2.0 * self.pts[3][0] - behind[0], 2.0 * self.pts[3][1] - behind[1]], (f - 1.0) * total);
        }
        let want = f * total;
        let j = self.lengths.partition_point(|&l| l < want).clamp(1, Self::SAMPLES);
        let (l0, l1) = (self.lengths[j - 1], self.lengths[j]);
        let s = ((j - 1) as f64 + if l1 > l0 { (want - l0) / (l1 - l0) } else { 0.0 }) / Self::SAMPLES as f64;
        Self::point(&self.pts, s)
    }
}

fn lerp4(a: [f64; 4], b: [f64; 4], u: f64) -> [f64; 4] {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * u)
}

/// Progress (value fraction) at time fraction `u` along a cubic Bézier from
/// (0,0) to (1,1) with control points from the two handles.
fn bezier_progress(u: f64, out: Handle, inn: Handle) -> f64 {
    let x1 = out.x.clamp(0.0, 1.0);
    let y1 = out.y;
    let x2 = 1.0 - inn.x.clamp(0.0, 1.0);
    let y2 = 1.0 - inn.y;
    let bez = |p1: f64, p2: f64, s: f64| {
        let m = 1.0 - s;
        3.0 * m * m * s * p1 + 3.0 * m * s * s * p2 + s * s * s
    };
    // x(s) is monotonic because x1, x2 lie in 0..=1, so bisection always
    // finds the unique s; 50 halvings is far below a microsecond.
    let (mut lo, mut hi) = (0.0f64, 1.0f64);
    for _ in 0..50 {
        let mid = 0.5 * (lo + hi);
        if bez(x1, x2, mid) < u { lo = mid } else { hi = mid }
    }
    bez(y1, y2, 0.5 * (lo + hi))
}

fn hermite(p0: f64, p1: f64, m0: f64, m1: f64, u: f64) -> f64 {
    let u2 = u * u;
    let u3 = u2 * u;
    (2.0 * u3 - 3.0 * u2 + 1.0) * p0 + (u3 - 2.0 * u2 + u) * m0 + (-2.0 * u3 + 3.0 * u2) * p1 + (u3 - u2) * m1
}

/// Tangents (value per microsecond) at keys `i` and `i + 1` for component
/// `d`, using Catmull-Rom slopes limited as in Fritsch–Carlson monotone
/// interpolation, so the curve never overshoots its keys.
fn clamped_tangents(keys: &[Keyframe], comps: &Window, d: usize, i: usize) -> (f64, f64) {
    let p = |j: usize| comps.at(j).0[d];
    let t = |j: usize| keys[j].time as f64;
    let slope = |a: usize, b: usize| (p(b) - p(a)) / (t(b) - t(a));
    let n = keys.len();
    let tangent = |j: usize| -> f64 {
        // End keys get a flat tangent: a smooth settle with no overshoot.
        if j == 0 || j + 1 >= n {
            return 0.0;
        }
        let (d0, d1) = (slope(j - 1, j), slope(j, j + 1));
        if d0 * d1 <= 0.0 {
            return 0.0;
        }
        let m = slope(j - 1, j + 1);
        m.signum() * m.abs().min(3.0 * d0.abs().min(d1.abs()))
    };
    let (mut m0, mut m1) = (tangent(i), tangent(i + 1));
    let delta = slope(i, i + 1);
    if delta == 0.0 {
        return (0.0, 0.0);
    }
    let (a, b) = (m0 / delta, m1 / delta);
    let r = a * a + b * b;
    if r > 9.0 {
        let s = 3.0 / r.sqrt();
        m0 *= s;
        m1 *= s;
    }
    (m0, m1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::{Rgba, Vec2};

    fn scalar(t: &Track, at: Micros) -> f64 {
        t.eval(at).unwrap().as_f64().unwrap()
    }

    /// A long track gives at every moment what the keys around that moment
    /// give on their own: each look reads only its neighbourhood.
    #[test]
    fn a_long_track_reads_only_the_keys_around_a_moment() {
        let interps = [Interp::Linear, Interp::Auto, Interp::Ease, Interp::Hold, Interp::Auto, Interp::Curve];
        let keys: Vec<Keyframe> = (0..60).map(|k| Keyframe::new(k * 100, Value::Point(Vec2::new(((k * 37) % 11) as f64, ((k * 13) % 7) as f64 * 3.0)), interps[k as usize % interps.len()])).collect();
        let long = Track::new(keys.clone());
        for i in 1..57 {
            let near = Track::new(keys[i - 1..=i + 2].to_vec());
            for t in [i as i64 * 100 + 10, i as i64 * 100 + 50, i as i64 * 100 + 93] {
                assert_eq!(long.eval(t), near.eval(t), "at {t}");
            }
        }
        // Angles turned the short way still count every turn before.
        let mut spin = Track::new((0..40).map(|k| Keyframe::new(k * 100, Value::Angle(((k * 170) % 360) as f64), Interp::Linear)).collect());
        spin.shortest_angle = true;
        let a = spin.eval(3850).unwrap().as_f64().unwrap();
        let b = spin.eval(3750).unwrap().as_f64().unwrap();
        assert!((a - b).abs() <= 180.0 && b.abs() > 360.0, "{b} -> {a}");
    }

    #[test]
    fn linear_and_hold() {
        let mut tr = Track::new(vec![Keyframe::new(0, 0.0, Interp::Linear), Keyframe::new(100, 10.0, Interp::Hold), Keyframe::new(200, 20.0, Interp::Linear)]);
        assert_eq!(scalar(&tr, -5), 0.0);
        assert_eq!(scalar(&tr, 50), 5.0);
        assert_eq!(scalar(&tr, 150), 10.0);
        assert_eq!(scalar(&tr, 250), 20.0);
        tr.set_key(Keyframe::new(100, 0.0, Interp::Linear));
        assert_eq!(tr.keys.len(), 3);
        assert_eq!(scalar(&tr, 150), 10.0);
        assert!(tr.remove_key(100).is_some());
        assert_eq!(scalar(&tr, 100), 10.0);
        tr.validate().unwrap();
    }

    #[test]
    fn ease_is_symmetric_and_bounded() {
        let tr = Track::new(vec![Keyframe::new(0, 0.0, Interp::Ease), Keyframe::new(1000, 1.0, Interp::Linear)]);
        let mid = scalar(&tr, 500);
        assert!((mid - 0.5).abs() < 1e-6);
        assert!(scalar(&tr, 100) < 0.1, "eases in slowly");
        let mut prev = 0.0;
        for t in (0..=1000).step_by(10) {
            let v = scalar(&tr, t);
            assert!(v >= prev - 1e-12 && v <= 1.0 + 1e-12);
            prev = v;
        }
    }

    #[test]
    fn auto_never_overshoots() {
        let vals = [0.0, 10.0, 10.5, 2.0, 2.0, 8.0];
        let keys = vals.iter().enumerate().map(|(i, v)| Keyframe::new(i as i64 * 1000, *v, Interp::Auto)).collect();
        let tr = Track::new(keys);
        for i in 0..vals.len() - 1 {
            let (lo, hi) = (vals[i].min(vals[i + 1]), vals[i].max(vals[i + 1]));
            for s in 0..=100 {
                let v = scalar(&tr, i as i64 * 1000 + s * 10);
                assert!(v >= lo - 1e-9 && v <= hi + 1e-9, "segment {i}: {v} not in {lo}..{hi}");
            }
        }
    }

    #[test]
    fn colour_interpolates_in_linear_light() {
        let tr = Track::new(vec![Keyframe::new(0, Rgba::BLACK, Interp::Linear), Keyframe::new(100, Rgba::WHITE, Interp::Linear)]);
        let mid = tr.eval(50).unwrap().as_colour().unwrap();
        // Linear-light 0.5 is about 0.735 when sRGB-encoded.
        assert!((mid.r - 0.7354).abs() < 1e-3, "{mid:?}");
    }

    #[test]
    fn angles_shortest_path() {
        let mut tr = Track::new(vec![Keyframe::new(0, Value::Angle(350.0), Interp::Linear), Keyframe::new(100, Value::Angle(10.0), Interp::Linear)]);
        assert_eq!(scalar(&tr, 50), 180.0);
        tr.shortest_angle = true;
        assert_eq!(scalar(&tr, 50), 360.0);
    }

    #[test]
    fn curves_land_exactly_and_have_their_character() {
        use CurveEnd::*;
        let all = [
            Curve::Power { power: 3.0, end: Out },
            Curve::Expo { end: InOut },
            Curve::Circ { end: In },
            Curve::Sine { end: Out },
            Curve::Back { overshoot: 1.70158, end: Out },
            Curve::Elastic { amplitude: 1.0, period: 0.3, end: Out },
            Curve::Bounce { end: Out },
            Curve::Steps { count: 4 },
        ];
        for c in all {
            assert_eq!(c.apply(0.0), 0.0, "{c:?}");
            assert_eq!(c.apply(1.0), 1.0, "{c:?}");
        }
        let samples = |c: Curve| (1..100).map(move |i| c.apply(i as f64 / 100.0));
        // An elastic rings: past the target and back under it, more than once.
        let el = samples(Curve::Elastic { amplitude: 1.0, period: 0.3, end: Out }).collect::<Vec<_>>();
        let crossings = el.windows(2).filter(|w| (w[0] - 1.0).signum() != (w[1] - 1.0).signum()).count();
        assert!(crossings >= 3, "elastic crosses the target {crossings} times");
        assert!(el.iter().cloned().fold(0.0, f64::max) > 1.2);
        // A bounce never passes the floor (1) and touches it more than once.
        let bo = samples(Curve::Bounce { end: Out }).collect::<Vec<_>>();
        assert!(bo.iter().all(|v| *v <= 1.0 + 1e-9));
        assert!(bo.windows(3).filter(|w| w[1] > w[0] && w[1] > w[2]).count() >= 2, "bounces");
        // Back goes past; steps only take whole steps.
        assert!(samples(Curve::Back { overshoot: 1.70158, end: Out }).any(|v| v > 1.05));
        assert!(samples(Curve::Steps { count: 4 }).all(|v| [0.0, 0.25, 0.5, 0.75].contains(&v)));
        // In mirrors out.
        let (i, o) = (Curve::Expo { end: In }, Curve::Expo { end: Out });
        assert!((i.apply(0.3) - (1.0 - o.apply(0.7))).abs() < 1e-12);
        // On a track.
        let tr = Track::new(vec![Keyframe::with_curve(0, 0.0, Curve::Bounce { end: Out }), Keyframe::new(1000, 100.0, Interp::Linear)]);
        assert!((scalar(&tr, 500) - 100.0 * bounce_out(0.5)).abs() < 1e-9);
        // Saved and read back.
        let k = Keyframe::with_curve(0, 1.0, Curve::Elastic { amplitude: 1.2, period: 0.4, end: InOut });
        let json = serde_json::to_string(&k).unwrap();
        assert!(json.contains("\"kind\":\"elastic\""), "{json}");
        assert_eq!(serde_json::from_str::<Keyframe>(&json).unwrap(), k);
        // Old projects (no curve, no path) still read.
        let old: Keyframe = serde_json::from_str(r#"{"time":0,"value":{"scalar":1.0},"interp":"ease"}"#).or_else(|_| serde_json::from_str(r#"{"time":0,"value":1.0,"interp":"ease"}"#)).unwrap();
        assert!(old.curve.is_none() && old.path.is_none());
    }

    #[test]
    fn position_bends_along_its_path_at_an_even_speed() {
        // From (0,0) to (200,0), bowed up through y = -150 at the middle.
        let mut a = Keyframe::new(0, Vec2::ZERO, Interp::Linear);
        a.path = Some(PathHandles { inn: [0.0, 0.0], out: [0.0, -200.0] });
        let mut b = Keyframe::new(1000, Vec2::new(200.0, 0.0), Interp::Linear);
        b.path = Some(PathHandles { inn: [0.0, -200.0], out: [0.0, 0.0] });
        let tr = Track::new(vec![a, b]);
        let at = |t| tr.eval(t).unwrap().as_point().unwrap();
        assert_eq!(at(0), Vec2::ZERO);
        assert_eq!(at(1000), Vec2::new(200.0, 0.0));
        let mid = at(500);
        assert!((mid.x - 100.0).abs() < 0.5 && (mid.y + 150.0).abs() < 0.5, "{mid:?}");
        // Even speed: equal times cover equal distances.
        let steps: Vec<f64> = (0..10).map(|i| {
            let (p, q) = (at(i * 100), at((i + 1) * 100));
            (q.x - p.x).hypot(q.y - p.y)
        }).collect();
        let (lo, hi) = (steps.iter().cloned().fold(f64::MAX, f64::min), steps.iter().cloned().fold(0.0, f64::max));
        assert!(hi / lo < 1.05, "{steps:?}");
        // Without handles it is the straight line it always was.
        let straight = Track::new(vec![Keyframe::new(0, Vec2::ZERO, Interp::Linear), Keyframe::new(1000, Vec2::new(200.0, 0.0), Interp::Linear)]);
        assert_eq!(straight.eval(500).unwrap().as_point().unwrap(), Vec2::new(100.0, 0.0));
        // An overshooting curve overshoots along the path's end direction.
        let mut c = Keyframe::with_curve(0, Vec2::ZERO, Curve::Back { overshoot: 3.0, end: CurveEnd::Out });
        c.path = Some(PathHandles { inn: [0.0, 0.0], out: [0.0, -200.0] });
        let tr = Track::new(vec![c, tr.keys[1].clone()]);
        let late = tr.eval(800).unwrap().as_point().unwrap();
        assert!(late.y > 0.0, "carried on downwards past the end: {late:?}");
    }

    #[test]
    fn points_and_validation() {
        let tr = Track::new(vec![Keyframe::new(0, Vec2::ZERO, Interp::Linear), Keyframe::new(10, Vec2::new(10.0, -10.0), Interp::Linear)]);
        assert_eq!(tr.eval(5).unwrap().as_point().unwrap(), Vec2::new(5.0, -5.0));
        let bad = Track::new(vec![Keyframe::new(0, 1.0, Interp::Linear), Keyframe::new(10, Vec2::ZERO, Interp::Linear)]);
        assert_eq!(bad.validate(), Err(TrackError::MixedKinds));
        assert_eq!(Track::default().validate(), Err(TrackError::Empty));
    }

    #[test]
    fn modifiers_loop_wobble_step_and_limit_the_keys() {
        use crate::modifier::{Modifier, Repeat};
        let f = |tr: &Track, t: Micros| tr.eval(t).unwrap().as_f64().unwrap();
        let mut walk = Track::new(vec![Keyframe::new(0, 0.0, Interp::Linear), Keyframe::new(100, 10.0, Interp::Linear)]);
        walk.modifiers.push(Modifier::Cycles { before: Repeat::Hold, after: Repeat::Offset, count: 0 });
        assert_eq!(f(&walk, 150), 15.0);
        assert_eq!(f(&walk, 300), 30.0);
        assert_eq!(f(&walk, -50), 0.0);
        walk.modifiers[0] = Modifier::Cycles { before: Repeat::Repeat, after: Repeat::Mirror, count: 0 };
        assert_eq!(f(&walk, 130), 7.0);
        assert_eq!(f(&walk, -30), 7.0);
        walk.modifiers.push(Modifier::Limits { min: None, max: Some(5.0) });
        assert_eq!(f(&walk, 130), 5.0);

        // One key plus noise: a procedural wobble around the rest value,
        // the same every time it is read.
        let mut still = Track::new(vec![Keyframe::new(0, Vec2::new(100.0, 50.0), Interp::Linear)]);
        still.modifiers.push(Modifier::Noise { amount: 20.0, per_second: 4.0, octaves: 2, seed: 1, axis: Some(1), from: None, to: None });
        assert!(still.varies() && still.validate().is_ok());
        let mut moved = false;
        for i in 0..100 {
            let p = still.eval(i * 10_000).unwrap().as_point().unwrap();
            assert_eq!(p.x, 100.0, "only y wobbles");
            assert!((p.y - 50.0).abs() <= 20.0);
            moved |= (p.y - 50.0).abs() > 2.0;
            assert_eq!(still.eval(i * 10_000), still.eval(i * 10_000));
        }
        assert!(moved);

        let mut steps = Track::new(vec![Keyframe::new(0, 0.0, Interp::Linear), Keyframe::new(1_000_000, 100.0, Interp::Linear)]);
        steps.modifiers.push(Modifier::Stepped { per_second: 4.0, offset: 0 });
        assert_eq!(f(&steps, 300_000), 25.0);
        assert_eq!(f(&steps, 499_999), 25.0);
        assert_eq!(f(&steps, 500_000), 50.0);

        let mut formula = Track::new(vec![Keyframe::new(0, 10.0, Interp::Linear)]);
        formula.modifiers.push(Modifier::Expression { expr: "value * 2 + t".into() });
        assert_eq!(f(&formula, 1_500_000), 21.5);
        formula.modifiers.push(Modifier::Expression { expr: "nope(".into() });
        assert!(matches!(formula.validate(), Err(TrackError::BadModifier(_))));

        // Old projects (no modifiers) read back unchanged, and modifiers
        // round-trip through JSON.
        let json = serde_json::to_string(&walk).unwrap();
        assert!(json.contains("\"kind\":\"cycles\""));
        assert_eq!(serde_json::from_str::<Track>(&json).unwrap(), walk);
        let plain = serde_json::to_string(&Track::new(vec![Keyframe::new(0, 1.0, Interp::Linear)])).unwrap();
        assert!(!plain.contains("modifiers"));
        assert!(serde_json::from_str::<Track>(&plain).unwrap().modifiers.is_empty());
    }
}

/// Key components for a run of keys, read by key index.
struct Window {
    lo: usize,
    comps: [([f64; 4], usize); 4],
}

impl Window {
    fn at(&self, j: usize) -> ([f64; 4], usize) {
        self.comps[j - self.lo]
    }
}
