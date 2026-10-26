// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Drivers: a property whose value is worked out from other properties.
//! A [`Driver`] names
//! variables (each a property of some clip, optionally one axis of it and
//! a delay) and a formula over them, in the formula language of
//! [`crate::modifier::expr`]: `a * 0.5 + 10`, `sin(t * tau) * b`.
//!
//! Drivers live on the driven clip, keyed by property path (any path a
//! clip can key: transform, effect settings, generator settings,
//! material…); `path#0` / `path#1` drive one axis of a point only, so x
//! and y can follow different things. They are worked out per moment by [`Sequence::driven`],
//! which every place that evaluates a clip goes through (the renderer,
//! parenting, the canvas), so a frame is the same everywhere. A formula
//! can use `value` (the property's own keyed value), `t` (seconds on the
//! timeline) and `i` (the axis being worked out) as well.

use crate::clip::Clip;
use crate::id::ClipId;
use crate::keyframe::{Interp, Keyframe, Track};
use crate::modifier::expr;
use crate::sequence::Sequence;
use crate::time::{Micros, SECOND};
use crate::value::{Value, Vec2};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

/// One property's driver.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Driver {
    /// The formula, over the variables' names and `value`, `t`, `i`.
    pub expr: String,
    /// The properties it reads.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vars: Vec<DriverVar>,
    /// Off: the property plays its own keys.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
}

/// A variable of a [`Driver`]: another clip's property.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DriverVar {
    /// The name the formula uses (lower case letters, digits, `_`).
    pub name: String,
    /// The clip read (it may be the driven clip itself).
    pub clip: ClipId,
    /// The property path read.
    pub property: String,
    /// One axis of a point (0 x, 1 y); unset follows the axis being
    /// worked out (x drives x, y drives y).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<u8>,
    /// Read the source this long ago (a follower that lags).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub delay: Micros,
}

fn yes() -> bool {
    true
}
fn is_true(b: &bool) -> bool {
    *b
}
fn is_zero(v: &Micros) -> bool {
    *v == 0
}

/// Names the formula language keeps for itself.
const RESERVED: &[&str] = &["value", "v", "t", "time", "i", "pi", "tau", "e", "sin", "cos", "tan", "abs", "sqrt", "exp", "ln", "floor", "ceil", "round", "fract", "sign", "noise", "random", "min", "max", "pow", "step", "clamp", "lerp", "smoothstep"];

/// How deep drivers may read drivers (a loop stops here).
const DEPTH: u32 = 8;

impl Driver {
    /// A driver that copies one property: `name` is `a`.
    pub fn copy(clip: ClipId, property: &str) -> Driver {
        Driver { expr: "a".into(), vars: vec![DriverVar { name: "a".into(), clip, property: property.into(), component: None, delay: 0 }], enabled: true }
    }

    /// Why this driver can't run, if it can't.
    pub fn problem(&self) -> Option<String> {
        let mut seen = Vec::new();
        for v in &self.vars {
            let ok = !v.name.is_empty() && v.name.chars().next().is_some_and(|c| c.is_ascii_lowercase()) && v.name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
            if !ok {
                return Some(format!("variable name \"{}\": lower-case letters, digits and _, starting with a letter", v.name));
            }
            if RESERVED.contains(&v.name.as_str()) {
                return Some(format!("\"{}\" is a word the formula language keeps; pick another name", v.name));
            }
            if seen.contains(&&v.name) {
                return Some(format!("two variables are called \"{}\"", v.name));
            }
            if v.component.is_some_and(|c| c > 1) {
                return Some("a variable's component is 0 (x) or 1 (y)".into());
            }
            seen.push(&v.name);
        }
        let names: Vec<&str> = self.vars.iter().map(|v| v.name.as_str()).collect();
        expr::parse_with(&self.expr, &names).err().map(|e| format!("driver formula: {e}"))
    }
}

impl Sequence {
    /// `clip` as it is at timeline time `t` with its drivers worked out:
    /// the clip itself when it has none; otherwise a copy where each
    /// driven property holds its driven value (as a one-key track, so
    /// every evaluator reads it).
    pub fn driven<'a>(&'a self, clip: &'a Clip, t: Micros) -> Cow<'a, Clip> {
        self.driven_at(clip, t, 0)
    }

    fn driven_at<'a>(&'a self, clip: &'a Clip, t: Micros, depth: u32) -> Cow<'a, Clip> {
        if clip.drivers.is_empty() || depth > DEPTH {
            return Cow::Borrowed(clip);
        }
        let mut out = clip.clone();
        let local = (t - clip.start).clamp(0, clip.duration.max(0));
        for (key, d) in clip.drivers.iter().filter(|(_, d)| d.enabled) {
            let (path, only) = split_key(key);
            let Some(v) = self.drive(clip, path, d, t, depth) else { continue };
            // One axis: on top of what the clip holds so far (its keys, or
            // the other axis's driver).
            let v = match (only, out.property_at(path, local), &v) {
                (Some(0), Some(Value::Point(now)), Value::Point(p)) => Value::Point(Vec2::new(p.x, now.y)),
                (Some(_), Some(Value::Point(now)), Value::Point(p)) => Value::Point(Vec2::new(now.x, p.y)),
                _ => v,
            };
            out.keyframes.insert(path.to_owned(), Track::new(vec![Keyframe::new(0, v, Interp::Hold)]));
        }
        Cow::Owned(out)
    }

    /// The driven value of `path` on `clip` at timeline time `t`.
    fn drive(&self, clip: &Clip, path: &str, d: &Driver, t: Micros, depth: u32) -> Option<Value> {
        let local = (t - clip.start).clamp(0, clip.duration.max(0));
        let own = clip.property_at(path, local)?;
        let (own_c, dims) = own.components()?;
        let names: Vec<&str> = d.vars.iter().map(|v| v.name.as_str()).collect();
        let parsed = expr::parse_cached(&d.expr, &names);
        let e = parsed.as_ref().as_ref().ok()?;
        // Each variable's value, all its components.
        let vars: Vec<Option<([f64; 4], usize)>> = d
            .vars
            .iter()
            .map(|v| {
                let at = t - v.delay;
                let src = if v.clip == clip.id { Cow::Borrowed(clip) } else { self.driven_at(self.clip(v.clip)?, at, depth + 1) };
                let local = (at - src.start).clamp(0, src.duration.max(0));
                src.property_at(&v.property, local).and_then(components)
            })
            .collect();
        // A variable whose clip is gone: the property plays as keyed.
        if vars.iter().any(Option::is_none) {
            return None;
        }
        let secs = t as f64 / SECOND as f64;
        let mut c = own_c;
        for (i, slot) in c.iter_mut().enumerate().take(dims) {
            let cx = DriverCx { seq: self, clip, path, d, vars: &vars, own: own_c, i, secs, depth };
            let v = e.eval_in(&cx);
            if v.is_finite() {
                *slot = v;
            }
        }
        Some(own.with_components(c))
    }
}

/// A value's components, a switch as 0 or 1.
fn components(x: Value) -> Option<([f64; 4], usize)> {
    match x {
        Value::Bool(b) => Some(([b as u8 as f64, 0.0, 0.0, 0.0], 1)),
        x => x.components(),
    }
}

/// A property as a formula names it: a model path, or a short name
/// (`position`, `x`, `rotation`, `opacity`, `scale`…) with the axis it
/// is, if one.
pub fn friendly_path(name: &str) -> (Cow<'_, str>, Option<usize>) {
    let p = |s: &'static str| Cow::Borrowed(s);
    match name.trim().to_ascii_lowercase().as_str() {
        "position" => (p("transform.position"), None),
        "x" => (p("transform.position"), Some(0)),
        "y" => (p("transform.position"), Some(1)),
        "z" => (p("transform.position_z"), None),
        "rotation" | "angle" => (p("transform.rotation"), None),
        "opacity" => (p("transform.opacity"), None),
        "scale" | "scale_x" => (p("transform.scale_x"), None),
        "scale_y" => (p("transform.scale_y"), None),
        "anchor" => (p("transform.anchor"), None),
        _ => (Cow::Borrowed(name.trim()), None),
    }
}

/// What a driver's formula sees: its variables, the driven property's
/// keys, and every other clip through `prop`.
struct DriverCx<'a> {
    seq: &'a Sequence,
    clip: &'a Clip,
    path: &'a str,
    d: &'a Driver,
    vars: &'a [Option<([f64; 4], usize)>],
    own: [f64; 4],
    i: usize,
    secs: f64,
    depth: u32,
}

impl DriverCx<'_> {
    fn var_comp(&self, name: &str, k: Option<usize>) -> Option<f64> {
        let at = self.d.vars.iter().position(|v| v.name == name)?;
        let (vc, vd) = self.vars[at]?;
        let comp = k.or(self.d.vars[at].component.map(|c| c as usize)).unwrap_or(if vd > 1 { self.i.min(vd - 1) } else { 0 });
        Some(vc[comp.min(3)])
    }
}

impl expr::Context for DriverCx<'_> {
    fn var(&self, name: &str) -> Option<f64> {
        match name {
            "value" | "v" => Some(self.own[self.i]),
            "t" | "time" => Some(self.secs),
            "i" => Some(self.i as f64),
            _ => self.var_comp(name, None),
        }
    }
    fn component(&self, name: &str, k: usize) -> Option<f64> {
        if name == "value" {
            return Some(self.own[k.min(3)]);
        }
        self.var_comp(name, Some(k))
    }
    fn value_at(&self, t: f64) -> Option<f64> {
        let at = (t * SECOND as f64).round() as Micros;
        let local = (at - self.clip.start).clamp(0, self.clip.duration.max(0));
        components(self.clip.property_at(self.path, local)?).map(|(c, _)| c[self.i.min(3)])
    }
    fn num_keys(&self) -> usize {
        self.clip.keyframes.get(self.path).map_or(0, |tr| tr.keys.len())
    }
    fn key(&self, n: usize) -> Option<(f64, f64)> {
        let k = self.clip.keyframes.get(self.path)?.keys.get(n.checked_sub(1)?)?;
        let (c, _) = components(k.value.clone())?;
        Some(((self.clip.start + k.time) as f64 / SECOND as f64, c[self.i.min(3)]))
    }
    fn prop(&self, clip: expr::ClipRef, path: &str, component: Option<usize>, t: Option<f64>) -> Option<f64> {
        let src = match clip {
            expr::ClipRef::Id(id) => self.seq.clip(ClipId(id))?,
            expr::ClipRef::Name(n) => self.seq.clips.iter().find(|c| c.name.eq_ignore_ascii_case(n.trim()))?,
        };
        let at = t.map_or((self.secs * SECOND as f64).round() as Micros, |s| (s * SECOND as f64).round() as Micros);
        let src = if src.id == self.clip.id { Cow::Borrowed(self.clip) } else { self.seq.driven_at(src, at, self.depth + 1) };
        let (path, axis) = friendly_path(path);
        let local = (at - src.start).clamp(0, src.duration.max(0));
        let (c, dims) = components(src.property_at(&path, local)?)?;
        let k = component.or(axis).unwrap_or(if dims > 1 { self.i.min(dims - 1) } else { 0 });
        Some(c[k.min(3)])
    }
}

/// A driver key's property path and the one axis it drives, if one.
pub fn split_key(key: &str) -> (&str, Option<usize>) {
    match key.rsplit_once('#') {
        Some((p, "0")) => (p, Some(0)),
        Some((p, "1")) => (p, Some(1)),
        _ => (key, None),
    }
}

/// The clips `clip`'s drivers read (for showing links and for keeping
/// them right when clips are copied).
pub fn sources(clip: &Clip) -> Vec<ClipId> {
    let mut out: Vec<ClipId> = clip.drivers.values().flat_map(|d| d.vars.iter().map(|v| v.clip)).filter(|c| *c != clip.id).collect();
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip::ClipSource;
    use crate::id::LaneId;

    fn seq_with(clips: Vec<Clip>) -> Sequence {
        let mut s = Sequence::new(crate::id::SequenceId(1), "s", LaneId(1));
        s.clips = clips;
        s
    }

    fn clip(id: u64) -> Clip {
        Clip::new(ClipId(id), LaneId(1), 0, 4 * SECOND, ClipSource::Gap)
    }

    #[test]
    fn a_property_follows_another_clips_with_a_formula_and_a_delay() {
        let mut leader = clip(1);
        leader.keyframes.insert("transform.rotation".into(), Track::new(vec![Keyframe::new(0, Value::Angle(0.0), Interp::Linear), Keyframe::new(SECOND, Value::Angle(100.0), Interp::Linear)]));
        let mut follower = clip(2);
        follower.drivers.insert("transform.rotation".into(), Driver { expr: "a * 0.5 + 10".into(), ..Driver::copy(ClipId(1), "transform.rotation") });
        let mut lag = clip(3);
        let mut d = Driver::copy(ClipId(1), "transform.rotation");
        d.vars[0].delay = SECOND / 2;
        lag.drivers.insert("transform.rotation".into(), d);
        let s = seq_with(vec![leader, follower, lag]);
        let rot = |id: u64, t: Micros| s.driven(s.clip(ClipId(id)).unwrap(), t).property_at("transform.rotation", t).unwrap().as_f64().unwrap();
        assert_eq!(rot(2, SECOND / 2), 35.0);
        assert_eq!(rot(2, 2 * SECOND), 60.0);
        assert_eq!(rot(3, SECOND), 50.0, "half a second behind");
        assert_eq!(rot(3, SECOND / 4), 0.0, "before the leader moved");
        // Undriven clips come back as they are.
        assert!(matches!(s.driven(s.clip(ClipId(1)).unwrap(), 0), Cow::Borrowed(_)));
    }

    #[test]
    fn points_follow_axis_by_axis_and_a_driver_can_read_its_own_clip() {
        let mut a = clip(1);
        a.transform.position = Vec2::new(100.0, -50.0);
        let mut b = clip(2);
        b.drivers.insert("transform.position".into(), Driver { expr: "p * -1".into(), vars: vec![DriverVar { name: "p".into(), clip: ClipId(1), property: "transform.position".into(), component: None, delay: 0 }], enabled: true });
        // Its own x turns it: rotation = x / 10.
        b.drivers.insert("transform.rotation".into(), Driver { expr: "x / 10".into(), vars: vec![DriverVar { name: "x".into(), clip: ClipId(2), property: "transform.position".into(), component: Some(0), delay: 0 }], enabled: true });
        b.transform.position = Vec2::new(30.0, 0.0);
        let s = seq_with(vec![a, b]);
        let d = s.driven(s.clip(ClipId(2)).unwrap(), 0);
        assert_eq!(d.property_at("transform.position", 0).unwrap().as_point().unwrap(), Vec2::new(-100.0, 50.0));
        // The own-clip read sees the undriven value.
        assert_eq!(d.property_at("transform.rotation", 0).unwrap().as_f64().unwrap(), 3.0);
        // The transform evaluators see it too.
        assert_eq!(d.transform_at(0).position, Vec2::new(-100.0, 50.0));
    }

    #[test]
    fn loops_stop_and_bad_drivers_are_named() {
        let mut a = clip(1);
        a.drivers.insert("transform.opacity".into(), Driver { expr: "b + 0.1".into(), vars: vec![DriverVar { name: "b".into(), clip: ClipId(2), property: "transform.opacity".into(), component: None, delay: 0 }], enabled: true });
        let mut b = clip(2);
        b.drivers.insert("transform.opacity".into(), Driver { expr: "a + 0.1".into(), vars: vec![DriverVar { name: "a".into(), clip: ClipId(1), property: "transform.opacity".into(), component: None, delay: 0 }], enabled: true });
        let s = seq_with(vec![a, b]);
        let v = s.driven(s.clip(ClipId(1)).unwrap(), 0).property_at("transform.opacity", 0).unwrap().as_f64().unwrap();
        assert!(v.is_finite() && v > 1.0, "a loop settles after a few rounds: {v}");
        assert!(Driver { expr: "sin + 1".into(), ..Driver::copy(ClipId(1), "x") }.problem().is_some());
        let mut bad = Driver::copy(ClipId(1), "x");
        bad.vars[0].name = "sin".into();
        assert!(bad.problem().unwrap().contains("keeps"));
        assert!(Driver { expr: "q * 2".into(), ..Driver::copy(ClipId(1), "x") }.problem().unwrap().contains("unknown name q"));
        assert!(Driver::copy(ClipId(1), "x").problem().is_none());
        // A missing source clip leaves the property as keyed.
        let mut lone = clip(5);
        lone.drivers.insert("transform.opacity".into(), Driver { expr: "a * 0".into(), ..Driver::copy(ClipId(9), "transform.opacity") });
        let s = seq_with(vec![lone]);
        let v = s.driven(s.clip(ClipId(5)).unwrap(), 0).property_at("transform.opacity", 0).unwrap().as_f64().unwrap();
        assert_eq!(v, 1.0, "a missing source leaves the property as keyed");
    }

    #[test]
    fn x_and_y_can_follow_different_things() {
        let mut a = clip(1);
        a.transform.position = Vec2::new(100.0, 0.0);
        a.transform.rotation = 45.0;
        let mut b = clip(2);
        b.transform.position = Vec2::new(7.0, 8.0);
        b.drivers.insert("transform.position#0".into(), Driver { expr: "p + 10".into(), vars: vec![DriverVar { name: "p".into(), clip: ClipId(1), property: "transform.position".into(), component: Some(0), delay: 0 }], enabled: true });
        let s = seq_with(vec![a.clone(), b.clone()]);
        assert_eq!(s.driven(s.clip(ClipId(2)).unwrap(), 0).transform_at(0).position, Vec2::new(110.0, 8.0), "y as keyed");
        b.drivers.insert("transform.position#1".into(), Driver { expr: "r * 2".into(), ..Driver::copy(ClipId(1), "transform.rotation") });
        b.drivers.get_mut("transform.position#1").unwrap().vars[0].name = "r".into();
        let s = seq_with(vec![a, b]);
        assert_eq!(s.driven(s.clip(ClipId(2)).unwrap(), 0).transform_at(0).position, Vec2::new(110.0, 90.0));
        assert_eq!(split_key("transform.position#1"), ("transform.position", Some(1)));
        assert_eq!(split_key("effect.3.amount"), ("effect.3.amount", None));
    }

    #[test]
    fn expressions_link_by_name_loop_keys_and_remap_time() {
        let mut ball = clip(1);
        ball.name = "Ball".into();
        ball.transform.rotation = 30.0;
        ball.transform.position = Vec2::new(5.0, 6.0);
        // No variables: the formula names the clip itself.
        let mut follower = clip(2);
        follower.drivers.insert("transform.rotation".into(), Driver { expr: "prop(\"ball\", \"rotation\") * 2 + prop(1, \"y\")".into(), vars: vec![], enabled: true });
        // Keys 0 → 100 over a second, looped by the formula.
        let mut looper = clip(3);
        looper.keyframes.insert("transform.opacity".into(), Track::new(vec![Keyframe::new(0, 0.0, Interp::Linear), Keyframe::new(SECOND, 1.0, Interp::Linear)]));
        looper.drivers.insert("transform.opacity".into(), Driver { expr: "loop_out(\"pingpong\")".into(), vars: vec![], enabled: true });
        // A movie played at double speed by a time remap formula.
        let mut movie = clip(4);
        movie.keyframes.insert(crate::clip::TIME_REMAP.into(), Track::new(vec![Keyframe::new(0, 0.0, Interp::Linear)]));
        movie.drivers.insert(crate::clip::TIME_REMAP.into(), Driver { expr: "t * 2".into(), vars: vec![], enabled: true });
        let s = seq_with(vec![ball, follower, looper, movie]);
        let at = |id: u64, p: &str, t: Micros| s.driven(s.clip(ClipId(id)).unwrap(), t).property_at(p, t).unwrap().as_f64().unwrap();
        assert_eq!(at(2, "transform.rotation", 0), 66.0);
        assert!((at(3, "transform.opacity", SECOND * 5 / 4) - 0.75).abs() < 1e-9);
        assert!((at(3, "transform.opacity", SECOND / 2) - 0.5).abs() < 1e-9);
        let m = s.driven(s.clip(ClipId(4)).unwrap(), SECOND);
        assert_eq!(m.source_time_at(SECOND), 2 * SECOND);
        assert!(Driver { expr: "prop(\"ball\", \"rotation\")".into(), vars: vec![], enabled: true }.problem().is_none());
    }

    #[test]
    fn modifiers_see_their_keys() {
        use crate::modifier::Modifier;
        let mut tr = Track::new(vec![Keyframe::new(0, 0.0, Interp::Linear), Keyframe::new(SECOND, 10.0, Interp::Linear)]);
        tr.modifiers.push(Modifier::Expression { expr: "loopOut(\"offset\")".into() });
        assert!((tr.eval(SECOND * 3 / 2).unwrap().as_f64().unwrap() - 15.0).abs() < 1e-9);
        tr.modifiers[0] = Modifier::Expression { expr: "amp = 3\nvalue + amp".into() };
        assert_eq!(tr.eval(SECOND / 2).unwrap().as_f64().unwrap(), 8.0);
    }
}
