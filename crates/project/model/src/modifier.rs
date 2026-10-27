// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Modifiers on a keyframe [`Track`](crate::Track): procedural layers on
//! top of the keys, the way animators loop, wobble and step a curve
//! without baking hundreds of keys.
//!
//! Time modifiers ([`Modifier::Cycles`], [`Modifier::Stepped`]) change the
//! moment the keys are read at; value modifiers ([`Modifier::Noise`],
//! [`Modifier::Limits`], [`Modifier::Expression`]) then change what comes
//! out, in list order. A track with a single key and a modifier is a
//! procedural property: one key holding the rest value, plus noise.
//!
//! Everything here is a pure function of clip time, so a frame renders
//! the same on the CPU, on the GPU and in an export.

use crate::time::{Micros, SECOND};
use serde::{Deserialize, Serialize};

/// One modifier on a track.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Modifier {
    /// Smooth random wobble around the keyed value.
    Noise {
        /// Largest swing either way, in the track's units.
        amount: f64,
        /// Wobbles per second (the noise's frequency).
        #[serde(default = "six")]
        per_second: f64,
        /// Layers of finer detail (1 is smooth, 3 is jittery).
        #[serde(default = "one_u")]
        octaves: u32,
        /// Different seeds wobble differently.
        #[serde(default)]
        seed: u32,
        /// Only this component (0 = x, 1 = y); every component when unset.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        axis: Option<u8>,
        /// Clip time the wobble eases in from (it ramps over a tenth of a
        /// second), and out at `to`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<Micros>,
        /// Clip time the wobble eases out by.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<Micros>,
    },
    /// Repeats the keyed stretch before the first key and after the last.
    Cycles {
        /// What happens before the first key.
        #[serde(default = "hold")]
        before: Repeat,
        /// What happens after the last key.
        #[serde(default = "repeat")]
        after: Repeat,
        /// How many repeats on each side; 0 repeats for ever.
        #[serde(default)]
        count: u32,
    },
    /// Holds each value for a fixed time, like animating "on twos".
    Stepped {
        /// How many new poses a second (12 is "on twos" at 24 fps).
        per_second: f64,
        /// Shifts the moments the steps land on, in clip time.
        #[serde(default)]
        offset: Micros,
    },
    /// Keeps every component between `min` and `max`.
    Limits {
        /// Lowest allowed value.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min: Option<f64>,
        /// Highest allowed value.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<f64>,
    },
    /// A formula for each component. Names: `value` (the keyed value),
    /// `t` (seconds into the clip), `i` (the component: 0 x, 1 y), `pi`,
    /// `tau`; functions sin, cos, tan, abs, sqrt, exp, ln, floor, ceil,
    /// round, fract, sign, min, max, pow, clamp, lerp, step, smoothstep,
    /// noise(x) (smooth, -1..1) and random(x) (0..1, the same for the
    /// same x).
    Expression {
        /// The formula, e.g. `value + sin(t * tau) * 20`.
        expr: String,
    },
}

/// What a [`Modifier::Cycles`] does outside the keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Repeat {
    /// Hold the end value (no cycling on this side).
    #[default]
    Hold,
    /// Play the keys again from the start.
    Repeat,
    /// Play them again, each repeat carrying on from where the last ended
    /// (a walk that keeps walking).
    Offset,
    /// Play them forwards, then backwards (ping-pong).
    Mirror,
}

fn six() -> f64 {
    6.0
}
fn one_u() -> u32 {
    1
}
fn hold() -> Repeat {
    Repeat::Hold
}
fn repeat() -> Repeat {
    Repeat::Repeat
}

impl Modifier {
    /// The modifier's name as ops and the Inspector say it.
    pub fn name(&self) -> &'static str {
        match self {
            Modifier::Noise { .. } => "noise",
            Modifier::Cycles { .. } => "cycles",
            Modifier::Stepped { .. } => "stepped",
            Modifier::Limits { .. } => "limits",
            Modifier::Expression { .. } => "expression",
        }
    }

    /// Why this modifier can't run, if it can't.
    pub fn problem(&self) -> Option<String> {
        match self {
            Modifier::Noise { amount, per_second, octaves, .. } => {
                if !amount.is_finite() || !per_second.is_finite() || *per_second <= 0.0 || *per_second > 240.0 {
                    return Some("noise needs a finite amount and 0 < per_second ≤ 240".into());
                }
                (*octaves == 0 || *octaves > 8).then(|| "noise octaves are 1..=8".into())
            }
            Modifier::Stepped { per_second, .. } => (!per_second.is_finite() || *per_second <= 0.0 || *per_second > 240.0).then(|| "stepped needs 0 < per_second ≤ 240".into()),
            Modifier::Limits { min: Some(a), max: Some(b) } if a > b => Some("limits: min is above max".into()),
            Modifier::Expression { expr } => expr::parse(expr).err().map(|e| format!("expression: {e}")),
            _ => None,
        }
    }

    /// Whether the modifier changes the time the keys are read at.
    pub(crate) fn is_time(&self) -> bool {
        matches!(self, Modifier::Cycles { .. } | Modifier::Stepped { .. })
    }
}

/// The time a time modifier reads the keys at, and how many key spans
/// it moved by (for [`Repeat::Offset`]).
pub(crate) fn remap_time(m: &Modifier, t: Micros, first: Micros, last: Micros) -> (Micros, i64) {
    match *m {
        Modifier::Cycles { before, after, count } => {
            let span = last - first;
            if span <= 0 {
                return (t, 0);
            }
            let (mode, outside) = if t < first {
                (before, t - first)
            } else if t > last {
                (after, t - last)
            } else {
                return (t, 0);
            };
            if mode == Repeat::Hold {
                return (t, 0);
            }
            // Cycle number counted away from the keys: 1 is the first repeat.
            let mut n = (outside.abs() + span - 1) / span;
            if count > 0 && n > count as i64 {
                n = count as i64;
                // Past the last repeat: hold its end.
                let end = if t > last { last + n * span } else { first - n * span };
                return remap_time(m, end, first, last);
            }
            let local = (t - first).rem_euclid(span);
            let forwards = match mode {
                Repeat::Mirror => n % 2 == 0,
                _ => true,
            };
            // Exactly on a cycle boundary after the keys: the end of that cycle.
            let local = if local == 0 && t > last { span } else { local };
            let at = if forwards { first + local } else { last - local };
            let shift = if mode == Repeat::Offset { if t > last { n } else { -n } } else { 0 };
            (at, shift)
        }
        Modifier::Stepped { per_second, offset } => {
            let step = (SECOND as f64 / per_second).round().max(1.0) as i64;
            ((t - offset).div_euclid(step) * step + offset, 0)
        }
        _ => (t, 0),
    }
}

/// Applies a value modifier to components `c` (of which `dims` are used)
/// at clip time `t`.
pub(crate) fn apply_value(m: &Modifier, c: &mut [f64; 4], dims: usize, t: Micros, track: &crate::keyframe::Track) {
    let secs = t as f64 / SECOND as f64;
    match m {
        Modifier::Noise { amount, per_second, octaves, seed, axis, from, to } => {
            let ramp = |edge: Micros, rising: bool| {
                let d = if rising { t - edge } else { edge - t } as f64 / (SECOND as f64 * 0.1);
                d.clamp(0.0, 1.0)
            };
            let fade = from.map_or(1.0, |f| ramp(f, true)) * to.map_or(1.0, |e| ramp(e, false));
            if fade <= 0.0 {
                return;
            }
            for (d, slot) in c.iter_mut().enumerate().take(dims) {
                if axis.is_some_and(|a| a as usize != d) {
                    continue;
                }
                let s = seed.wrapping_mul(7919).wrapping_add(d as u32 * 104_729);
                *slot += amount * fade * fractal(secs * per_second, *octaves, s);
            }
        }
        Modifier::Limits { min, max } => {
            for slot in c.iter_mut().take(dims) {
                if let Some(lo) = min {
                    *slot = slot.max(*lo);
                }
                if let Some(hi) = max {
                    *slot = slot.min(*hi);
                }
            }
        }
        Modifier::Expression { expr } => {
            let parsed = expr::parse_cached(expr, &[]);
            let Ok(e) = parsed.as_ref() else { return };
            let start = *c;
            for (d, slot) in c.iter_mut().enumerate().take(dims) {
                let cx = TrackCx { track, comps: start, d, secs, slot: *slot };
                let v = e.eval_in(&cx);
                if v.is_finite() {
                    *slot = v;
                }
            }
        }
        _ => {}
    }
}

/// What an expression modifier sees: its track's keys and the value so
/// far.
struct TrackCx<'a> {
    track: &'a crate::keyframe::Track,
    comps: [f64; 4],
    d: usize,
    secs: f64,
    slot: f64,
}

impl expr::Context for TrackCx<'_> {
    fn var(&self, name: &str) -> Option<f64> {
        match name {
            "value" | "v" => Some(self.slot),
            "t" | "time" => Some(self.secs),
            "i" => Some(self.d as f64),
            _ => None,
        }
    }
    fn component(&self, name: &str, k: usize) -> Option<f64> {
        (name == "value").then(|| self.comps[k.min(3)])
    }
    fn value_at(&self, t: f64) -> Option<f64> {
        let v = self.track.eval_keys((t * SECOND as f64).round() as Micros)?;
        v.components().map(|(c, _)| c[self.d.min(3)])
    }
    fn num_keys(&self) -> usize {
        self.track.keys.len()
    }
    fn key(&self, n: usize) -> Option<(f64, f64)> {
        let k = self.track.keys.get(n.checked_sub(1)?)?;
        let (c, _) = k.value.components()?;
        Some((k.time as f64 / SECOND as f64, c[self.d.min(3)]))
    }
}

/// Integer hash to a gradient in -1..1.
fn grad(i: i64, seed: u32) -> f64 {
    let mut h = (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (seed as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 31;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 27;
    (h >> 11) as f64 / (1u64 << 52) as f64 - 1.0
}

/// 1D gradient noise: 0 at whole numbers, smooth between, about -1..1.
pub fn noise(x: f64, seed: u32) -> f64 {
    let i = x.floor();
    let f = x - i;
    let (a, b) = (grad(i as i64, seed) * f, grad(i as i64 + 1, seed) * (f - 1.0));
    let s = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    // Two gradients reach at most ±0.5 between them; scale to ±1.
    2.0 * (a + (b - a) * s)
}

/// Octaves of noise, each twice as fine and half as strong, kept to ±1.
fn fractal(x: f64, octaves: u32, seed: u32) -> f64 {
    let (mut sum, mut norm, mut amp, mut freq) = (0.0, 0.0, 1.0, 1.0);
    for o in 0..octaves.max(1) {
        sum += amp * noise(x * freq + o as f64 * 17.13, seed.wrapping_add(o));
        norm += amp;
        amp *= 0.5;
        freq *= 2.0;
    }
    sum / norm
}

/// The expression language of [`Modifier::Expression`] and drivers.
#[path = "expr.rs"]
pub mod expr;

/// The expression language compiled for shapes: fast, allocation-free
/// evaluation of pure formulas over x, y, z, u, v.
#[path = "formula.rs"]
pub mod formula;

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(s: &str, value: f64, t: f64) -> f64 {
        expr::parse(s).unwrap().eval(&|n| match n {
            "value" => Some(value),
            "t" => Some(t),
            _ => None,
        })
    }

    #[test]
    fn formulas_parse_and_evaluate() {
        assert_eq!(ev("1 + 2 * 3", 0.0, 0.0), 7.0);
        assert_eq!(ev("(1 + 2) * 3", 0.0, 0.0), 9.0);
        assert_eq!(ev("-2^2", 0.0, 0.0), -4.0);
        assert_eq!(ev("2^-1", 0.0, 0.0), 0.5);
        assert_eq!(ev("2**3", 0.0, 0.0), 8.0);
        assert_eq!(ev("value + t * 10", 5.0, 2.0), 25.0);
        assert!((ev("sin(pi / 2)", 0.0, 0.0) - 1.0).abs() < 1e-12);
        assert_eq!(ev("clamp(value, 0, 1)", 3.0, 0.0), 1.0);
        assert_eq!(ev("t > 1", 0.0, 2.0), 1.0);
        assert_eq!(ev("-7 % 3", 0.0, 0.0), 2.0);
        assert!(expr::parse("foo + 1").is_err());
        assert!(expr::parse("sin(1, 2)").is_err());
        assert!(expr::parse("(1 + 2").is_err());
        assert!(expr::parse("1 +").is_err());
        assert!(expr::parse(&"(".repeat(200)).is_err());
    }

    #[test]
    fn noise_is_smooth_bounded_and_repeatable() {
        let mut prev = noise(0.0, 3);
        let mut most: f64 = 0.0;
        for i in 1..4000 {
            let x = i as f64 * 0.01;
            let v = noise(x, 3);
            assert!((v - prev).abs() < 0.06, "jump at {x}");
            most = most.max(v.abs());
            prev = v;
            assert_eq!(v, noise(x, 3));
        }
        assert!(most <= 1.0 && most > 0.3, "{most}");
        assert_ne!(noise(0.5, 1), noise(0.5, 2));
    }

    #[test]
    fn cycles_wrap_time_and_count_spans() {
        let rep = Modifier::Cycles { before: Repeat::Mirror, after: Repeat::Offset, count: 0 };
        assert_eq!(remap_time(&rep, 150, 0, 100), (50, 1));
        assert_eq!(remap_time(&rep, 200, 0, 100), (100, 1));
        assert_eq!(remap_time(&rep, 250, 0, 100), (50, 2));
        // Mirror before: the first cycle back plays backwards.
        assert_eq!(remap_time(&rep, -30, 0, 100), (30, 0));
        assert_eq!(remap_time(&rep, -130, 0, 100), (70, 0));
        let twice = Modifier::Cycles { before: Repeat::Hold, after: Repeat::Repeat, count: 2 };
        assert_eq!(remap_time(&twice, 999, 0, 100), (100, 0));
        assert_eq!(remap_time(&twice, -50, 0, 100), (-50, 0));
        let steps = Modifier::Stepped { per_second: 12.0, offset: 0 };
        assert_eq!(remap_time(&steps, 100_000, 0, 0).0, 83_333);
    }
}
