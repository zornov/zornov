// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `driver`: one property worked out from others, live. Nothing is baked: move the source
//! and the driven property follows.
//!
//! * Follow one property: `{"op": "driver", "clip": 5, "property":
//!   "rotation", "from": {"clip": 3, "property": "rotation"}, "multiply":
//!   0.5, "add": 10, "delay": 0.2}` (`add` in the driven property's
//!   units, `multiply` a plain factor, `delay` seconds behind).
//! * A formula over named variables: `{"op": "driver", "clip": 5,
//!   "property": "y", "vars": {"a": {"clip": 3, "property": "x"}},
//!   "expr": "sin(a / 100) * 50"}`. Formulas work in stored units:
//!   opacity and scale are factors (1 = 100 %), angles degrees, positions
//!   pixels from the centre. Names: the variables, `value` (the property
//!   as keyed), `t` (seconds on the timeline), `i` (axis).
//! * A formula alone, reading other clips by name or id with `prop`:
//!   `{"op": "driver", "clip": 5, "property": "rotation", "expr":
//!   "prop(\"Wheel\", \"rotation\") * -2"}`.
//! * `remove: true` takes the driver off; `enabled: false` pauses it.
//!
//! Any property a clip can key can be driven or read: transform, 3D,
//! effect and generator settings, materials, model layers' settings.

use super::props::{self, Conv};
use super::{Session, f, micros};
use serde_json::{Value, json};
use studio_model as m;

/// A friendly property on `mc` as (model path, the axis it is, if one).
fn target(mc: &m::Clip, name: &str) -> Result<Vec<(String, Option<usize>)>, String> {
    let n = name.trim().to_lowercase();
    if n == "position" {
        return Ok(vec![("transform.position".into(), None)]);
    }
    // A model path as it is (an effect or generator setting, a model
    // layer's own settings), in its own case first: bone names are
    // `upper_arm.L`, and lower-casing them lost the side.
    let exact = name.trim();
    if exact.contains('.') && mc.get_property(exact).is_some() {
        return Ok(vec![(exact.to_owned(), None)]);
    }
    if n.contains('.') && mc.get_property(&n).is_some() {
        return Ok(vec![(n, None)]);
    }
    let prop = props::resolve(mc, &n)?;
    let axis = match prop.conv {
        Conv::PointX | Conv::BoxX => Some(0),
        Conv::PointY | Conv::BoxY => Some(1),
        _ => None,
    };
    Ok(prop.paths.into_iter().map(|p| (p, axis)).collect())
}

/// The driver key for a path and axis.
fn key(path: &str, axis: Option<usize>) -> String {
    match axis {
        Some(c) => format!("{path}#{c}"),
        None => path.to_owned(),
    }
}

impl Session {
    pub(super) fn driver(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or("driver needs \"clip\": the clip whose property is driven")? as u32;
        let name = op.get("property").and_then(Value::as_str).ok_or("driver needs \"property\": the property to drive")?;
        let mut mc = self.model_clip(id).ok_or_else(|| format!("No clip {id} (see summary for ids)"))?;
        let targets = target(&mc, name)?;
        if targets.iter().any(|(p, _)| p.starts_with("audio.")) {
            return Err("drivers work on picture properties; shape sound with the audio op".into());
        }
        let seq = self.doc.seq();
        if op.get("remove").and_then(Value::as_bool) == Some(true) {
            let before = mc.drivers.len();
            for (p, a) in &targets {
                mc.drivers.remove(&key(p, *a));
            }
            let n = before - mc.drivers.len();
            let ok = self.doc.apply("Remove driver", m::Edit::SetClip { seq, clip: Box::new(mc) });
            self.check(ok, "")?;
            return Ok(if n == 0 { format!("{name} on clip {id} had no driver") } else { format!("{name} on clip {id} plays its own keys again") });
        }
        if let Some(on) = op.get("enabled").and_then(Value::as_bool)
            && op.get("from").is_none()
            && op.get("vars").is_none()
        {
            let mut n = 0;
            for (p, a) in &targets {
                if let Some(d) = mc.drivers.get_mut(&key(p, *a)) {
                    d.enabled = on;
                    n += 1;
                }
            }
            if n == 0 {
                return Err(format!("{name} on clip {id} has no driver"));
            }
            let ok = self.doc.apply("Driver", m::Edit::SetClip { seq, clip: Box::new(mc) });
            self.check(ok, "")?;
            return Ok(format!("{name} driver {}", if on { "on" } else { "paused" }));
        }
        // The variables: `from` is one called `a`.
        let mut specs: Vec<(String, Value)> = Vec::new();
        if let Some(from) = op.get("from") {
            specs.push(("a".into(), from.clone()));
        }
        if let Some(Value::Object(vs)) = op.get("vars") {
            for (k, v) in vs {
                specs.push((k.to_lowercase(), v.clone()));
            }
        }
        if specs.is_empty() && op.get("expr").is_none() {
            return Err("driver needs \"from\": {clip, property} (follow it), or \"vars\": {name: {clip, property}} with \"expr\"; or \"remove\": true".into());
        }
        let mut vars = Vec::new();
        for (vname, spec) in &specs {
            let src_id = spec.get("clip").and_then(Value::as_u64).ok_or_else(|| format!("variable {vname} needs \"clip\""))? as u32;
            let src = self.model_clip(src_id).ok_or_else(|| format!("variable {vname}: no clip {src_id}"))?;
            let sp = spec.get("property").and_then(Value::as_str).ok_or_else(|| format!("variable {vname} needs \"property\""))?;
            let (path, axis) = target(&src, sp).map_err(|e| format!("variable {vname}: {e}"))?.into_iter().next().ok_or("no property")?;
            let component = match spec.get("axis").and_then(Value::as_str) {
                Some("x") => Some(0),
                Some("y") => Some(1),
                _ => axis.map(|a| a as u8),
            };
            let delay = spec.get("delay").and_then(Value::as_f64).or_else(|| f(op, "delay")).unwrap_or(0.0);
            vars.push(m::DriverVar { name: vname.clone(), clip: m::ClipId(src_id as u64), property: path, component, delay: micros(delay) });
        }
        // The formula: given, or follow `a` scaled and offset.
        let expr = match op.get("expr").and_then(Value::as_str) {
            Some(e) => e.to_owned(),
            None => {
                let k = f(op, "multiply").unwrap_or(1.0);
                let add = f(op, "add").unwrap_or(0.0);
                // `add` in the driven property's units, as stored.
                let (p0, _) = &targets[0];
                let prop = props::resolve(&mc, name).ok();
                let stored_add = match prop {
                    Some(pr) if !matches!(pr.conv, Conv::PointX | Conv::PointY | Conv::Raw) => {
                        let now = mc.get_property(p0);
                        let a = props::to_model(&pr, p0, &json!(add), now.as_ref()).ok().and_then(|v| v.as_f64()).unwrap_or(add);
                        let z = props::to_model(&pr, p0, &json!(0.0), now.as_ref()).ok().and_then(|v| v.as_f64()).unwrap_or(0.0);
                        a - z
                    }
                    _ => add,
                };
                let mut e = if (k - 1.0).abs() < 1e-12 { "a".to_owned() } else { format!("a * {k}") };
                if stored_add.abs() > 1e-12 {
                    e = format!("{e} + {stored_add}");
                }
                e
            }
        };
        let d = m::Driver { expr, vars, enabled: op.get("enabled").and_then(Value::as_bool).unwrap_or(true) };
        if let Some(p) = d.problem() {
            return Err(p);
        }
        let mut said = Vec::new();
        for (p, a) in &targets {
            if mc.get_property(p).is_none() {
                return Err(format!("{name} has no value on clip {id}"));
            }
            mc.drivers.insert(key(p, *a), d.clone());
            said.push(key(p, *a));
        }
        let ok = self.doc.apply("Driver", m::Edit::SetClip { seq, clip: Box::new(mc) });
        self.check(ok, "")?;
        Ok(format!("{name} on clip {id} is driven: {} ({})", d.expr, said.join(", ")))
    }
}

/// A clip's drivers for `inspect`.
pub fn listed(clip: &m::Clip) -> Value {
    let mut out = serde_json::Map::new();
    for (k, d) in &clip.drivers {
        let vars: Vec<Value> = d
            .vars
            .iter()
            .map(|v| {
                let mut o = json!({"name": v.name, "clip": v.clip.0, "property": v.property});
                if let Some(c) = v.component {
                    o["axis"] = json!(if c == 0 { "x" } else { "y" });
                }
                if v.delay != 0 {
                    o["delay"] = json!(v.delay as f64 / 1e6);
                }
                o
            })
            .collect();
        let mut o = json!({"expr": d.expr, "vars": vars});
        if !d.enabled {
            o["enabled"] = json!(false);
        }
        out.insert(k.clone(), o);
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services;

    fn two_cards() -> (Session, u32, u32) {
        let mut s = Session::new(services::project::new_custom("Drivers", 640, 360, 30.0));
        s.edit(&[json!({"op": "card", "colour": "#ffcc00", "at": 0, "duration": 4, "width": 100, "height": 100, "x": -200}), json!({"op": "card", "colour": "#3366ff", "at": 0, "duration": 4, "width": 100, "height": 100, "x": 200})], "cards", false).unwrap();
        let mut ids: Vec<u32> = s.doc.project.clips.iter().map(|c| c.id).collect();
        ids.sort();
        (s, ids[ids.len() - 2], ids[ids.len() - 1])
    }

    fn at(s: &Session, id: u32, path: &str, t: f64) -> m::Value {
        let seq = s.doc.model.active();
        let c = seq.clip(m::ClipId(id as u64)).unwrap();
        seq.driven(c, micros(t)).property_at(path, micros(t) - c.start).unwrap()
    }

    #[test]
    fn a_layer_follows_another_scaled_offset_and_late() {
        let (mut s, lead, follow) = two_cards();
        s.edit(&[json!({"op": "animate", "clip": lead, "property": "rotation", "keys": [[0, 0], [1, 90]], "ease": "linear"})], "spin", false).unwrap();
        // Clip references ("$1", names) work inside `from` too.
        s.edit(&[json!({"op": "driver", "clip": follow, "property": "rotation", "from": {"clip": format!("{lead}"), "property": "rotation"}, "multiply": 0.5, "add": 10})], "drive", false).unwrap();
        assert_eq!(at(&s, follow, "transform.rotation", 1.0).as_f64(), Some(55.0));
        // Opacity: `add` is in percent.
        s.edit(&[json!({"op": "driver", "clip": follow, "property": "opacity", "from": {"clip": lead, "property": "opacity"}, "multiply": 0.5, "add": 20})], "fade", false).unwrap();
        assert!((at(&s, follow, "transform.opacity", 0.5).as_f64().unwrap() - 0.7).abs() < 1e-9);
        // Late: a quarter second behind.
        s.edit(&[json!({"op": "driver", "clip": follow, "property": "rotation", "from": {"clip": lead, "property": "rotation"}, "delay": 0.25})], "late", false).unwrap();
        assert!((at(&s, follow, "transform.rotation", 0.75).as_f64().unwrap() - 45.0).abs() < 1e-6);
        let mc = s.model_clip(follow).unwrap();
        let l = listed(&mc);
        assert_eq!(l["transform.rotation"]["vars"][0]["delay"], json!(0.25));
        s.edit(&[json!({"op": "driver", "clip": follow, "property": "rotation", "remove": true})], "free", false).unwrap();
        assert_eq!(at(&s, follow, "transform.rotation", 0.75).as_f64(), Some(0.0));
    }

    #[test]
    fn formulas_axes_and_mistakes() {
        let (mut s, lead, follow) = two_cards();
        // y bobs with the leader's x; x stays as keyed.
        s.edit(&[json!({"op": "driver", "clip": follow, "property": "y", "vars": {"lx": {"clip": lead, "property": "x"}}, "expr": "lx / 2"})], "bob", false).unwrap();
        let p = at(&s, follow, "transform.position", 0.0).as_point().unwrap();
        assert_eq!((p.x, p.y), (200.0, -100.0));
        // Scale (both axes) from the leader's rotation, as a factor.
        s.edit(&[json!({"op": "driver", "clip": follow, "property": "scale", "vars": {"r": {"clip": lead, "property": "rotation"}}, "expr": "1 + r / 90"})], "grow", false).unwrap();
        s.edit(&[json!({"op": "set", "clip": lead, "rotation": 45})], "turn", false).unwrap();
        assert_eq!(at(&s, follow, "transform.scale_y", 0.0).as_f64(), Some(1.5));
        assert!(s.edit(&[json!({"op": "driver", "clip": follow, "property": "rotation", "vars": {"sin": {"clip": lead, "property": "x"}}, "expr": "sin"})], "x", false).is_err());
        assert!(s.edit(&[json!({"op": "driver", "clip": follow, "property": "rotation", "vars": {"a": {"clip": 999, "property": "x"}}, "expr": "a"})], "x", false).is_err());
        assert!(s.edit(&[json!({"op": "driver", "clip": follow, "property": "rotation", "from": {"clip": lead, "property": "x"}, "expr": "b"})], "x", false).is_err());
        assert!(s.edit(&[json!({"op": "driver", "clip": follow, "property": "rotation"})], "x", false).is_err());
        let insp = props::describe(&s.model_clip(follow).unwrap());
        assert!(insp["drivers"]["transform.position#1"]["expr"] == "lx / 2", "{insp}");
    }
}
