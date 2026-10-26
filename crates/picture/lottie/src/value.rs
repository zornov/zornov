// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Animated values: a Lottie property (`{"a": 0, "k": value}` or
//! `{"a": 1, "k": [keyframes]}`) evaluated at a frame.
//!
//! Each keyframe holds its value `s` (older files also give the segment's
//! end as `e`), the segment's easing as a cubic Bezier from its out tangent
//! `o` to the next one's in tangent `i` (both normalised, x = time and
//! y = progress), `h: 1` for a hold, and for positions the spatial tangents
//! `to` / `ti`. Per the lottie-spec 1.0 description of animated properties.

use serde_json::Value;

/// One keyframe.
#[derive(Clone, Debug, PartialEq)]
pub struct Key {
    t: f64,
    s: Vec<f64>,
    e: Option<Vec<f64>>,
    /// Easing out of this key (x, y per dimension, or one pair for all).
    o: Vec<(f64, f64)>,
    /// Easing into the next key.
    i: Vec<(f64, f64)>,
    hold: bool,
    to: Option<Vec<f64>>,
    ti: Option<Vec<f64>>,
}

/// A number, a vector or a colour, animated or not.
#[derive(Clone, Debug, PartialEq)]
pub enum Prop {
    Static(Vec<f64>),
    Keys(Vec<Key>),
}

impl Default for Prop {
    fn default() -> Self {
        Prop::Static(Vec::new())
    }
}

fn numbers(v: &Value) -> Vec<f64> {
    match v {
        Value::Number(n) => vec![n.as_f64().unwrap_or(0.0)],
        Value::Array(a) => a.iter().filter_map(Value::as_f64).collect(),
        _ => Vec::new(),
    }
}

/// An easing handle: `{"x": 0.3, "y": 1}` or `{"x": [..], "y": [..]}`.
fn handle(v: Option<&Value>, default: (f64, f64)) -> Vec<(f64, f64)> {
    let Some(v) = v else { return vec![default] };
    let (x, y) = (numbers(&v["x"]), numbers(&v["y"]));
    if x.is_empty() || y.is_empty() {
        return vec![default];
    }
    (0..x.len().max(y.len())).map(|k| (x[k.min(x.len() - 1)], y[k.min(y.len() - 1)])).collect()
}

impl Prop {
    /// Reads a property object; `None` (or junk) is a static empty value.
    pub fn parse(v: Option<&Value>) -> Prop {
        let Some(v) = v else { return Prop::default() };
        // Some writers put the value straight in, without {a, k}.
        let Some(k) = v.get("k") else { return Prop::Static(numbers(v)) };
        let animated = v.get("a").and_then(Value::as_i64) == Some(1) || k.as_array().is_some_and(|a| a.first().is_some_and(|f| f.get("t").is_some()));
        if !animated {
            return Prop::Static(numbers(k));
        }
        let keys: Vec<Key> = k
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|f| {
                        Some(Key {
                            t: f.get("t")?.as_f64()?,
                            s: f.get("s").map(numbers).unwrap_or_default(),
                            e: f.get("e").map(numbers),
                            o: handle(f.get("o"), (0.0, 0.0)),
                            i: handle(f.get("i"), (1.0, 1.0)),
                            hold: f.get("h").and_then(Value::as_i64) == Some(1),
                            to: f.get("to").map(numbers).filter(|v| !v.is_empty()),
                            ti: f.get("ti").map(numbers).filter(|v| !v.is_empty()),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        if keys.is_empty() { Prop::Static(Vec::new()) } else { Prop::Keys(keys) }
    }

    /// The value at `frame`.
    pub fn at(&self, frame: f64) -> Vec<f64> {
        let keys = match self {
            Prop::Static(v) => return v.clone(),
            Prop::Keys(k) => k,
        };
        let first = &keys[0];
        if frame <= first.t || keys.len() == 1 {
            return first.s.clone();
        }
        let n = keys.len();
        // The segment holding `frame`.
        let k = keys.windows(2).position(|w| frame < w[1].t).unwrap_or(n - 1);
        if k == n - 1 {
            // After the last key: its value (old files keep it in the
            // previous key's `e`).
            let last = &keys[n - 1];
            return if last.s.is_empty() { keys[n - 2].e.clone().unwrap_or_else(|| keys[n - 2].s.clone()) } else { last.s.clone() };
        }
        let (a, b) = (&keys[k], &keys[k + 1]);
        let start = &a.s;
        let end = a.e.as_ref().filter(|e| !e.is_empty()).unwrap_or(&b.s);
        if a.hold || end.is_empty() {
            return start.clone();
        }
        let span = (b.t - a.t).max(1e-9);
        let x = ((frame - a.t) / span).clamp(0.0, 1.0);
        let dims = start.len().min(end.len());
        let eased = |d: usize| {
            let (ox, oy) = a.o[d.min(a.o.len() - 1)];
            let (ix, iy) = a.i[d.min(a.i.len() - 1)];
            ease(ox, oy, ix, iy, x)
        };
        // A spatial curve: the point moves along a Bezier through the
        // tangents, the eased fraction of the way along its length (as
        // lottie-web moves it), not of its parameter.
        if let (Some(to), Some(ti)) = (&a.to, &a.ti) {
            if dims >= 2 && to.len() >= 2 && ti.len() >= 2 && (to.iter().chain(ti.iter()).any(|v| v.abs() > 1e-9)) {
                let point = |u: f64| -> Vec<f64> {
                    (0..dims)
                        .map(|d| {
                            let (p0, p3) = (start[d], end[d]);
                            let (p1, p2) = (p0 + to.get(d).copied().unwrap_or(0.0), p3 + ti.get(d).copied().unwrap_or(0.0));
                            let m = 1.0 - u;
                            m * m * m * p0 + 3.0 * m * m * u * p1 + 3.0 * m * u * u * p2 + u * u * u * p3
                        })
                        .collect()
                };
                const STEPS: usize = 100;
                let pts: Vec<Vec<f64>> = (0..=STEPS).map(|k| point(k as f64 / STEPS as f64)).collect();
                let mut cum = vec![0.0];
                for w in pts.windows(2) {
                    let d: f64 = w[0].iter().zip(&w[1]).map(|(x, y)| (y - x) * (y - x)).sum::<f64>().sqrt();
                    cum.push(cum[cum.len() - 1] + d);
                }
                let want = eased(0) * cum[STEPS];
                let k = cum.partition_point(|c| *c < want).clamp(1, STEPS);
                let u = if cum[k] > cum[k - 1] { (want - cum[k - 1]) / (cum[k] - cum[k - 1]) } else { 0.0 };
                return pts[k - 1].iter().zip(&pts[k]).map(|(x, y)| x + (y - x) * u).collect();
            }
        }
        (0..dims).map(|d| start[d] + (end[d] - start[d]) * eased(d)).collect()
    }

    /// The first number at `frame` (or `default`).
    pub fn num(&self, frame: f64, default: f64) -> f64 {
        self.at(frame).first().copied().unwrap_or(default)
    }

    /// Two numbers at `frame` (a missing second repeats the first).
    pub fn pair(&self, frame: f64, default: [f64; 2]) -> [f64; 2] {
        let v = self.at(frame);
        match v.len() {
            0 => default,
            1 => [v[0], v[0]],
            _ => [v[0], v[1]],
        }
    }
}

/// Progress at time fraction `x` on the cubic Bezier easing
/// (0,0) (ox,oy) (ix,iy) (1,1).
pub fn ease(ox: f64, oy: f64, ix: f64, iy: f64, x: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    let (ox, ix) = (ox.clamp(0.0, 1.0), ix.clamp(0.0, 1.0));
    let bez = |a: f64, b: f64, t: f64| {
        let m = 1.0 - t;
        3.0 * m * m * t * a + 3.0 * m * t * t * b + t * t * t
    };
    // x(t) is monotonic for handles in 0..1: bisect, then a few Newton steps.
    let (mut lo, mut hi) = (0.0, 1.0);
    let mut t = x;
    for _ in 0..40 {
        let v = bez(ox, ix, t);
        if (v - x).abs() < 1e-7 {
            break;
        }
        if v < x { lo = t } else { hi = t }
        t = 0.5 * (lo + hi);
    }
    bez(oy, iy, t)
}

/// A Bezier path value: `{c, v, i, o}` (tangents relative to vertices).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bez {
    pub closed: bool,
    pub v: Vec<[f64; 2]>,
    pub i: Vec<[f64; 2]>,
    pub o: Vec<[f64; 2]>,
}

impl Bez {
    fn parse(v: &Value) -> Option<Bez> {
        let v = if v.is_array() { v.get(0)? } else { v };
        let pts = |k: &str| -> Vec<[f64; 2]> { v.get(k).and_then(Value::as_array).map(|a| a.iter().map(|p| { let n = numbers(p); [n.first().copied().unwrap_or(0.0), n.get(1).copied().unwrap_or(0.0)] }).collect()).unwrap_or_default() };
        let (vv, i, o) = (pts("v"), pts("i"), pts("o"));
        Some(Bez { closed: v.get("c").and_then(Value::as_bool).unwrap_or(false), i: if i.len() == vv.len() { i } else { vec![[0.0; 2]; vv.len()] }, o: if o.len() == vv.len() { o } else { vec![[0.0; 2]; vv.len()] }, v: vv })
    }

    fn lerp(a: &Bez, b: &Bez, u: f64) -> Bez {
        if a.v.len() != b.v.len() {
            return if u < 1.0 { a.clone() } else { b.clone() };
        }
        let mix = |p: &[[f64; 2]], q: &[[f64; 2]]| p.iter().zip(q).map(|(p, q)| [p[0] + (q[0] - p[0]) * u, p[1] + (q[1] - p[1]) * u]).collect();
        Bez { closed: a.closed, v: mix(&a.v, &b.v), i: mix(&a.i, &b.i), o: mix(&a.o, &b.o) }
    }
}

/// An animated path.
#[derive(Clone, Debug, PartialEq)]
pub enum PathProp {
    Static(Bez),
    Keys(Vec<(f64, Bez, Option<Bez>, (f64, f64), (f64, f64), bool)>),
}

impl PathProp {
    pub fn parse(v: Option<&Value>) -> PathProp {
        let Some(v) = v else { return PathProp::Static(Bez::default()) };
        let k = v.get("k").unwrap_or(v);
        let animated = v.get("a").and_then(Value::as_i64) == Some(1) || k.as_array().is_some_and(|a| a.first().is_some_and(|f| f.get("t").is_some()));
        if !animated {
            return PathProp::Static(Bez::parse(k).unwrap_or_default());
        }
        let keys: Vec<_> = k
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|f| {
                        let h = |n: &str, d: (f64, f64)| handle(f.get(n), d)[0];
                        Some((f.get("t")?.as_f64()?, f.get("s").and_then(Bez::parse).unwrap_or_default(), f.get("e").and_then(Bez::parse), h("o", (0.0, 0.0)), h("i", (1.0, 1.0)), f.get("h").and_then(Value::as_i64) == Some(1)))
                    })
                    .collect()
            })
            .unwrap_or_default();
        if keys.is_empty() { PathProp::Static(Bez::default()) } else { PathProp::Keys(keys) }
    }

    pub fn at(&self, frame: f64) -> Bez {
        let keys = match self {
            PathProp::Static(b) => return b.clone(),
            PathProp::Keys(k) => k,
        };
        if frame <= keys[0].0 || keys.len() == 1 {
            return keys[0].1.clone();
        }
        let n = keys.len();
        let k = keys.windows(2).position(|w| frame < w[1].0).unwrap_or(n - 1);
        if k == n - 1 {
            let last = &keys[n - 1];
            return if last.1.v.is_empty() { keys[n - 2].2.clone().unwrap_or_else(|| keys[n - 2].1.clone()) } else { last.1.clone() };
        }
        let (a, b) = (&keys[k], &keys[k + 1]);
        let end = a.2.as_ref().filter(|e| !e.v.is_empty()).unwrap_or(&b.1);
        if a.5 {
            return a.1.clone();
        }
        let x = ((frame - a.0) / (b.0 - a.0).max(1e-9)).clamp(0.0, 1.0);
        Bez::lerp(&a.1, end, ease(a.3.0, a.3.1, a.4.0, a.4.1, x))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn static_and_linear_and_held_values() {
        assert_eq!(Prop::parse(Some(&json!({"a": 0, "k": [10, 20]}))).at(5.0), vec![10.0, 20.0]);
        let lin = Prop::parse(Some(&json!({"a": 1, "k": [{"t": 0, "s": [0], "o": {"x": 0, "y": 0}, "i": {"x": 1, "y": 1}}, {"t": 10, "s": [100]}]})));
        assert!((lin.num(5.0, 0.0) - 50.0).abs() < 1e-6);
        assert_eq!(lin.num(-3.0, 0.0), 0.0);
        assert_eq!(lin.num(30.0, 0.0), 100.0);
        let hold = Prop::parse(Some(&json!({"a": 1, "k": [{"t": 0, "s": [1], "h": 1}, {"t": 10, "s": [2]}]})));
        assert_eq!(hold.num(9.9, 0.0), 1.0);
        assert_eq!(hold.num(10.0, 0.0), 2.0);
        // Old files: the segment's end in `e`, the last key bare.
        let old = Prop::parse(Some(&json!({"a": 1, "k": [{"t": 0, "s": [0], "e": [8], "o": {"x": [0], "y": [0]}, "i": {"x": [1], "y": [1]}}, {"t": 4}]})));
        assert!((old.num(2.0, 0.0) - 4.0).abs() < 1e-6);
        assert_eq!(old.num(9.0, 0.0), 8.0);
    }

    #[test]
    fn bezier_easing_matches_css_ease_in_out() {
        // CSS ease-in-out (0.42, 0, 0.58, 1) is symmetric about the middle.
        assert!((ease(0.42, 0.0, 0.58, 1.0, 0.5) - 0.5).abs() < 1e-6);
        let a = ease(0.42, 0.0, 0.58, 1.0, 0.25);
        assert!(a < 0.25 && (a + ease(0.42, 0.0, 0.58, 1.0, 0.75) - 1.0).abs() < 1e-6, "{a}");
        // Overshoot handles (y beyond 1) pass through.
        assert!(ease(0.3, 0.0, 0.2, 1.6, 0.8) > 1.0);
    }

    #[test]
    fn paths_morph_vertex_by_vertex() {
        let p = PathProp::parse(Some(&json!({"a": 1, "k": [
            {"t": 0, "s": [{"c": true, "v": [[0, 0], [10, 0]], "i": [[0, 0], [0, 0]], "o": [[0, 0], [0, 0]]}], "o": {"x": 0, "y": 0}, "i": {"x": 1, "y": 1}},
            {"t": 10, "s": [{"c": true, "v": [[0, 10], [20, 0]], "i": [[0, 0], [0, 0]], "o": [[0, 0], [0, 0]]}]}
        ]})));
        let b = p.at(5.0);
        assert!(b.closed);
        assert_eq!(b.v, vec![[0.0, 5.0], [15.0, 0.0]]);
    }
}
