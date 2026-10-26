// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `modifier`: procedural layers on a property's keyframes, the curve
//! modifiers animators know from graph editors: a wobble that
//! never repeats, a loop that keeps going, poses held "on twos", limits
//! and a formula.
//!
//! `{"op": "modifier", "clip": 3, "property": "rotation", "add": "noise",
//! "amount": 6, "per_second": 2}` wobbles the rotation ±6° around
//! whatever it is keyed to (or its value now, when it has no keys).
//!
//! * `add`: noise (`amount` in the property's units, `per_second`,
//!   `octaves` 1..8, `seed`, `from`/`to` seconds), cycles (`before`,
//!   `after`: hold, repeat, offset, mirror; `count`, 0 = for ever),
//!   stepped (`per_second` new poses, 12 = on twos at 24 fps; `offset`
//!   seconds), limits (`min`, `max` in the property's units), expression
//!   (`expr`, in stored units: factors for opacity and scale, degrees,
//!   pixels; names `value`, `t`, `i`).
//!   One of each kind per property (noise: per axis, so x and y can
//!   wobble differently): adding a kind again replaces it.
//! * `remove`: a kind, or "all".
//!
//! Modifiers live on the track, so later keys keep them and the renderer,
//! the preview and an export all see the same frames.

use super::props::{self, Conv};
use super::{Session, f, micros};
use serde_json::{Value, json};
use studio_model as m;

impl Session {
    pub(super) fn modifier(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or("modifier needs \"clip\"")? as u32;
        let name = op.get("property").and_then(Value::as_str).ok_or("modifier needs \"property\" (rotation, scale, opacity, x, y, position, an effect setting…)")?.trim().to_lowercase();
        let mc = self.model_clip(id).ok_or_else(|| format!("No clip {id} (see summary for ids)"))?;
        let position = name == "position";
        let prop = props::resolve(&mc, if position { "x" } else { &name })?;
        if prop.paths.iter().any(|p| p.starts_with("audio.")) {
            return Err("modifiers work on picture properties; shape sound with the audio op's keys".into());
        }
        let seq = self.doc.seq();
        if let Some(kind) = op.get("remove").and_then(Value::as_str) {
            let mut n = 0;
            for path in &prop.paths {
                let Some(mut tr) = mc.keyframes.get(path).cloned() else { continue };
                let before = tr.modifiers.len();
                tr.modifiers.retain(|md| kind != "all" && md.name() != kind);
                n += before - tr.modifiers.len();
                let ok = self.doc.apply("Remove modifier", m::Edit::SetTrack { seq, clip: m::ClipId(id as u64), property: path.clone(), track: Some(tr) });
                self.check(ok, "")?;
            }
            return Ok(if n == 0 { format!("{name} had no {kind} modifier") } else { format!("removed {n} modifier(s) from {name}") });
        }
        let kind = op.get("add").and_then(Value::as_str).ok_or("modifier takes \"add\": noise, cycles, stepped, limits or expression (or \"remove\": a kind or \"all\")")?;
        let axis = match prop.conv {
            Conv::PointX | Conv::BoxX if !position => Some(0),
            Conv::PointY | Conv::BoxY => Some(1),
            _ => None,
        };
        let path0 = prop.paths[0].clone();
        let now = props::value_or_default(&mc, &path0).ok_or_else(|| format!("{name} has no value on clip {id}"))?;
        if !now.is_animatable() {
            return Err(format!("{name} can't be animated, so it can't take a modifier"));
        }
        // A friendly number as the stored one (absolute), and a friendly
        // change as a stored change.
        let stored = |x: f64| -> Result<f64, String> {
            let v = if position { m::Value::Point(m::Vec2 { x, y: x }) } else { props::to_model(&prop, &path0, &json!(x), Some(&now))? };
            Ok(match (v, axis) {
                (m::Value::Point(p), Some(1)) => p.y,
                (m::Value::Point(p), _) => p.x,
                (v, _) => v.as_f64().unwrap_or(x),
            })
        };
        let delta = |x: f64| -> Result<f64, String> { Ok(stored(x)? - stored(0.0)?) };
        let num = |k: &str| f(op, k);
        let modifier = match kind {
            "noise" | "wiggle" => m::Modifier::Noise {
                amount: delta(num("amount").ok_or("noise needs \"amount\": the largest swing, in the property's units")?)?.abs(),
                per_second: num("per_second").unwrap_or(2.0),
                octaves: num("octaves").unwrap_or(1.0).round().clamp(1.0, 8.0) as u32,
                seed: num("seed").unwrap_or(id as f64).max(0.0) as u32,
                axis,
                from: num("from").map(micros),
                to: num("to").map(micros),
            },
            "cycles" | "loop" => {
                let side = |k: &str, default: m::Repeat| -> Result<m::Repeat, String> {
                    match op.get(k).and_then(Value::as_str) {
                        None => Ok(default),
                        Some("hold" | "none") => Ok(m::Repeat::Hold),
                        Some("repeat" | "cycle") => Ok(m::Repeat::Repeat),
                        Some("offset" | "continue") => Ok(m::Repeat::Offset),
                        Some("mirror" | "pingpong" | "ping_pong") => Ok(m::Repeat::Mirror),
                        Some(o) => Err(format!("{k} \"{o}\": hold, repeat, offset or mirror")),
                    }
                };
                let tr = mc.keyframes.get(&path0);
                if tr.is_none_or(|t| t.keys.len() < 2) {
                    return Err(format!("cycles repeats keys: {name} needs at least two keys first (animate it, then loop it)"));
                }
                m::Modifier::Cycles { before: side("before", m::Repeat::Hold)?, after: side("after", m::Repeat::Repeat)?, count: num("count").unwrap_or(0.0).max(0.0) as u32 }
            }
            "stepped" | "steps" | "on_twos" => m::Modifier::Stepped { per_second: num("per_second").unwrap_or(12.0), offset: micros(num("offset").unwrap_or(0.0)) },
            "limits" | "clamp" => m::Modifier::Limits { min: num("min").map(stored).transpose()?, max: num("max").map(stored).transpose()? },
            "expression" | "formula" => m::Modifier::Expression { expr: op.get("expr").and_then(Value::as_str).ok_or("expression needs \"expr\", e.g. \"value + sin(t * tau) * 20\"")?.to_owned() },
            other => return Err(format!("modifier \"{other}\": noise, cycles, stepped, limits or expression")),
        };
        if let Some(p) = modifier.problem() {
            return Err(p);
        }
        let label = describe_one(&modifier);
        for path in &prop.paths {
            let mut tr = match mc.keyframes.get(path) {
                Some(tr) => tr.clone(),
                // A property without keys gets one holding its value now.
                None => {
                    let v = props::value_or_default(&mc, path).ok_or_else(|| format!("{name} has no value on clip {id}"))?;
                    m::Track::new(vec![m::Keyframe::new(0, v, m::Interp::Linear)])
                }
            };
            crate::editor::modifiers::place(&mut tr, &modifier);
            let ok = self.doc.apply("Modifier", m::Edit::SetTrack { seq, clip: m::ClipId(id as u64), property: path.clone(), track: Some(tr) });
            self.check(ok, "")?;
        }
        Ok(format!("{name} on clip {id}: {label}"))
    }
}

/// A modifier in a few words (stored units), for replies and `inspect`.
pub fn describe_one(md: &m::Modifier) -> String {
    let s = |t: m::Micros| t as f64 / 1e6;
    match md {
        m::Modifier::Noise { amount, per_second, octaves, from, to, .. } => {
            let mut o = format!("noise ±{amount:.3} at {per_second}/s");
            if *octaves > 1 {
                o += &format!(", {octaves} octaves");
            }
            if from.is_some() || to.is_some() {
                o += &format!(" from {:.2}s to {}", from.map_or(0.0, s), to.map_or("the end".to_owned(), |t| format!("{:.2}s", s(t))));
            }
            o
        }
        m::Modifier::Cycles { before, after, count } => format!("cycles before {before:?}, after {after:?}{}", if *count > 0 { format!(", {count} times") } else { String::new() }).to_lowercase(),
        m::Modifier::Stepped { per_second, .. } => format!("stepped, {per_second} poses a second"),
        m::Modifier::Limits { min, max } => format!("limits {} to {}", min.map_or("-".into(), |v| format!("{v:.3}")), max.map_or("-".into(), |v| format!("{v:.3}"))),
        m::Modifier::Expression { expr } => format!("expression {expr}"),
    }
}

/// Every modifier on a clip's tracks, by property path, for `inspect`.
pub fn listed(clip: &m::Clip) -> Value {
    let mut out = serde_json::Map::new();
    for (path, tr) in &clip.keyframes {
        if !tr.modifiers.is_empty() {
            out.insert(path.clone(), json!(tr.modifiers.iter().map(describe_one).collect::<Vec<_>>()));
        }
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services;

    fn session_with_card() -> (Session, u32) {
        let mut s = Session::new(services::project::new_custom("Mods", 640, 360, 30.0));
        s.edit(&[json!({"op": "card", "colour": "#ffcc00", "at": 0, "duration": 4, "width": 320, "height": 180})], "card", false).unwrap();
        let id = s.doc.project.clips.iter().map(|c| c.id).max().unwrap();
        (s, id)
    }

    fn at(s: &Session, id: u32, path: &str, t: f64) -> m::Value {
        s.model_clip(id).unwrap().property_at(path, micros(t)).unwrap()
    }

    #[test]
    fn noise_wobbles_a_still_property_in_its_own_units() {
        let (mut s, id) = session_with_card();
        s.edit(&[json!({"op": "modifier", "clip": id, "property": "opacity", "add": "noise", "amount": 10, "per_second": 3})], "n", false).unwrap();
        let mut most: f64 = 0.0;
        for i in 0..120 {
            let v = at(&s, id, "transform.opacity", i as f64 / 30.0).as_f64().unwrap();
            most = most.max((v - 1.0).abs());
        }
        // 10 % of opacity is 0.1 as stored.
        assert!(most > 0.02 && most <= 0.1 + 1e-9, "{most}");
        // y only: x stays put.
        s.edit(&[json!({"op": "modifier", "clip": id, "property": "y", "add": "noise", "amount": 30})], "n", false).unwrap();
        let p = at(&s, id, "transform.position", 1.3).as_point().unwrap();
        assert_eq!(p.x, 0.0);
        // Then x, its own way: both stay.
        s.edit(&[json!({"op": "modifier", "clip": id, "property": "x", "add": "noise", "amount": 5, "per_second": 0.5})], "n", false).unwrap();
        assert_eq!(s.model_clip(id).unwrap().keyframes["transform.position"].modifiers.len(), 2);
        let p = at(&s, id, "transform.position", 1.3).as_point().unwrap();
        assert!(p.x != 0.0 && p.x.abs() <= 5.0, "{p:?}");
        let listed = listed(&s.model_clip(id).unwrap());
        assert!(listed["transform.position"][0].as_str().unwrap().starts_with("noise"), "{listed}");
        s.edit(&[json!({"op": "modifier", "clip": id, "property": "opacity", "remove": "all"})], "r", false).unwrap();
        assert_eq!(at(&s, id, "transform.opacity", 1.3).as_f64(), Some(1.0));
    }

    #[test]
    fn cycles_keep_a_keyed_spin_going_and_steps_hold_poses() {
        let (mut s, id) = session_with_card();
        assert!(s.edit(&[json!({"op": "modifier", "clip": id, "property": "rotation", "add": "cycles"})], "c", false).is_err(), "needs keys");
        s.edit(&[json!({"op": "animate", "clip": id, "property": "rotation", "keys": [[0, 0], [1, 90]], "ease": "linear"})], "a", false).unwrap();
        s.edit(&[json!({"op": "modifier", "clip": id, "property": "rotation", "add": "cycles", "after": "offset"})], "c", false).unwrap();
        let r = |s: &Session, t| at(s, id, "transform.rotation", t).as_f64().unwrap();
        assert!((r(&s, 2.5) - 225.0).abs() < 1e-6, "{}", r(&s, 2.5));
        s.edit(&[json!({"op": "modifier", "clip": id, "property": "rotation", "add": "stepped", "per_second": 4})], "s", false).unwrap();
        assert!((r(&s, 0.3) - 22.5).abs() < 1e-6, "{}", r(&s, 0.3));
        assert!((r(&s, 2.6) - 225.0).abs() < 1e-6, "{}", r(&s, 2.6));
        // Adding the same kind again replaces it.
        s.edit(&[json!({"op": "modifier", "clip": id, "property": "rotation", "add": "stepped", "per_second": 2})], "s", false).unwrap();
        assert_eq!(s.model_clip(id).unwrap().keyframes["transform.rotation"].modifiers.len(), 2);
        // A later key keeps the modifiers.
        s.edit(&[json!({"op": "animate", "clip": id, "property": "rotation", "keys": [[0.5, 10]]})], "k", false).unwrap();
        assert_eq!(s.model_clip(id).unwrap().keyframes["transform.rotation"].modifiers.len(), 2);
    }

    #[test]
    fn limits_and_expressions_check_their_input() {
        let (mut s, id) = session_with_card();
        s.edit(&[json!({"op": "modifier", "clip": id, "property": "scale", "add": "expression", "expr": "value + sin(t * tau) * 0.2"})], "e", false).unwrap();
        let sx = |s: &Session, path: &str| at(s, id, path, 0.25).as_f64().unwrap();
        assert!((sx(&s, "transform.scale_x") - 1.2).abs() < 1e-9 && (sx(&s, "transform.scale_y") - 1.2).abs() < 1e-9);
        s.edit(&[json!({"op": "modifier", "clip": id, "property": "scale", "add": "limits", "max": 110})], "l", false).unwrap();
        assert!((sx(&s, "transform.scale_x") - 1.1).abs() < 1e-9, "{}", sx(&s, "transform.scale_x"));
        assert!(s.edit(&[json!({"op": "modifier", "clip": id, "property": "scale", "add": "expression", "expr": "value +"})], "e", false).is_err());
        assert!(s.edit(&[json!({"op": "modifier", "clip": id, "property": "scale", "add": "glow"})], "e", false).is_err());
    }
}
