// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `graph`: the graph editor for the AI. A clip's animated values read as
//! curves: a sparkline and samples per curve in the property's units, its
//! keys with their curves, and what an animator would look for: the
//! fastest moment, overshoot past the keys, kinks (the speed jumps at a
//! key, a hitch) and holds. Edits go through `keys` (move, curve and
//! bezier handles) and `animate`.

use super::Session;
use crate::editor::graph::{self as g, Curve};
use serde_json::{Value, json};
use studio_model as m;

fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn spark(v: &[f64]) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let (lo, hi) = v.iter().fold((f64::MAX, f64::MIN), |(a, b), x| (a.min(*x), b.max(*x)));
    if !(hi > lo) {
        return BARS[3].to_string().repeat(v.len());
    }
    v.iter().map(|x| BARS[(((x - lo) / (hi - lo)) * 7.0).round().clamp(0.0, 7.0) as usize]).collect()
}

/// One curve as the AI reads it.
pub fn read(mc: &m::Clip, c: &Curve, n: usize) -> Value {
    let dur = mc.duration.max(1);
    let n = n.clamp(4, 200);
    let at = |t: m::Micros| mc.property_at(&c.path, t.clamp(0, dur)).and_then(|v| g::comp(&v, c.comp)).unwrap_or(0.0) * c.scale;
    let samples: Vec<f64> = (0..n).map(|i| at(dur * i as i64 / (n as i64 - 1))).collect();
    // Speed from a fine pass: units per second.
    let fine = 240usize;
    let dt = dur as f64 / fine as f64 / 1e6;
    let vals: Vec<f64> = (0..=fine).map(|i| at(dur * i as i64 / fine as i64)).collect();
    let speeds: Vec<f64> = vals.windows(2).map(|w| (w[1] - w[0]) / dt).collect();
    let (fast_i, fast) = speeds.iter().enumerate().fold((0, 0.0f64), |(bi, b), (i, s)| if s.abs() > b.abs() { (i, *s) } else { (bi, b) });
    let tr = mc.keyframes.get(&c.path);
    let keys: Vec<Value> = tr
        .map(|tr| {
            tr.keys
                .iter()
                .enumerate()
                .map(|(i, k)| {
                    let mut o = json!({"t": r2(k.time as f64 / 1e6), "value": r2(g::comp(&k.value, c.comp).unwrap_or(0.0) * c.scale), "curve": super::props::curve_name(k, tr.keys.get(i + 1))});
                    if k.interp == m::Interp::Ease {
                        o["out"] = json!([r2(k.ease_out.x), r2(k.ease_out.y)]);
                    }
                    if i > 0 && tr.keys[i - 1].interp == m::Interp::Ease {
                        o["in"] = json!([r2(k.ease_in.x), r2(k.ease_in.y)]);
                    }
                    o
                })
                .collect()
        })
        .unwrap_or_default();
    let key_vals: Vec<f64> = tr.map(|tr| tr.keys.iter().filter_map(|k| g::comp(&k.value, c.comp)).map(|v| v * c.scale).collect()).unwrap_or_default();
    let (klo, khi) = key_vals.iter().fold((f64::MAX, f64::MIN), |(a, b), x| (a.min(*x), b.max(*x)));
    let (lo, hi) = vals.iter().fold((f64::MAX, f64::MIN), |(a, b), x| (a.min(*x), b.max(*x)));
    let mut notes = Vec::new();
    if !key_vals.is_empty() && (hi > khi + 1e-6 || lo < klo - 1e-6) {
        notes.push(format!("overshoots its keys: reaches {} to {} (keys span {} to {})", r2(lo), r2(hi), r2(klo), r2(khi)));
    }
    // Kinks: the speed either side of an inner key differs a lot (a hitch
    // the eye catches), unless one side is a deliberate stop.
    if let Some(tr) = tr {
        for k in tr.keys.iter().skip(1).take(tr.keys.len().saturating_sub(2)) {
            let f = (k.time as f64 / dur as f64 * fine as f64).round() as usize;
            if f < 3 || f + 3 > fine {
                continue;
            }
            let before = speeds[f - 2];
            let after = speeds[f + 1];
            let (a, b) = (before.abs(), after.abs());
            let big = a.max(b);
            // Half the speed or less on one side is a hitch the eye sees.
            if big > 1e-6 && (before.signum() == after.signum() || a.min(b) > big * 0.2) && a.min(b) <= big * 0.55 {
                notes.push(format!("kink at {}s: speed {} before, {} after", r2(k.time as f64 / 1e6), r2(before), r2(after)));
            }
        }
    }
    // Holds: long stretches that don't move.
    let mut still = 0;
    for (i, s) in speeds.iter().enumerate() {
        if s.abs() < 1e-6 {
            still += 1;
        } else {
            if still as f64 * dt >= 0.5 {
                notes.push(format!("holds still {}s to {}s", r2((i - still) as f64 * dt), r2(i as f64 * dt)));
            }
            still = 0;
        }
    }
    if still as f64 * dt >= 0.5 {
        notes.push(format!("holds still {}s to the end", r2((speeds.len() - still) as f64 * dt)));
    }
    let mods: Vec<String> = tr.map(|tr| tr.modifiers.iter().map(super::modifiers::describe_one).collect()).unwrap_or_default();
    let mut out = json!({
        "curve": c.label,
        "unit": c.unit,
        "line": spark(&samples),
        "samples": samples.iter().map(|v| r2(*v)).collect::<Vec<_>>(),
        "every_seconds": r2(dur as f64 / 1e6 / (n as f64 - 1.0)),
        "range": [r2(lo), r2(hi)],
        "fastest": {"t": r2(fast_i as f64 * dt), "per_second": r2(fast)},
        "keys": keys,
    });
    if !notes.is_empty() {
        out["notes"] = json!(notes);
    }
    if !mods.is_empty() {
        out["modifiers"] = json!(mods);
    }
    out
}

impl Session {
    pub(super) fn graph_tool(&mut self, a: &Value) -> Result<Value, String> {
        let id = a.get("clip").and_then(Value::as_u64).ok_or("graph needs \"clip\"")? as u32;
        let mc = self.model_clip(id).ok_or_else(|| format!("No clip {id} (see summary for ids)"))?;
        let n = a.get("samples").and_then(Value::as_u64).unwrap_or(24) as usize;
        let mut all = g::curves(&mc);
        if let Some(p) = a.get("property").and_then(Value::as_str) {
            let want = p.trim().to_lowercase();
            let paths: Vec<String> = match want.as_str() {
                "position" => vec!["transform.position".into()],
                _ => super::props::resolve(&mc, &want)?.paths,
            };
            let axis = match want.as_str() {
                "x" => Some(0),
                "y" => Some(1),
                _ => None,
            };
            all.retain(|c| paths.contains(&c.path) && (axis.is_none() || c.comp == axis));
            if all.is_empty() {
                return Err(format!("{p} has no keyframes on clip {id}; animate it first"));
            }
        }
        if all.is_empty() {
            return Ok(json!({"clip": id, "curves": [], "note": "no keyframes on this clip"}));
        }
        Ok(json!({"clip": id, "duration": r2(mc.duration as f64 / 1e6), "curves": all.iter().map(|c| read(&mc, c, n)).collect::<Vec<_>>()}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services;

    #[test]
    fn curves_read_as_lines_keys_and_faults() {
        let mut s = Session::new(services::project::new_custom("Graph", 640, 360, 30.0));
        s.edit(&[json!({"op": "card", "colour": "#ffcc00", "at": 0, "duration": 3, "width": 200, "height": 100})], "card", false).unwrap();
        let id = s.doc.project.clips.iter().map(|c| c.id).max().unwrap();
        s.edit(&[json!({"op": "animate", "clip": id, "property": "scale", "keys": [[0, 0], [1, 100]], "ease": "back"})], "pop", false).unwrap();
        s.edit(&[json!({"op": "animate", "clip": id, "property": "rotation", "keys": [[0, 0], [1, 10], [2, 90]], "ease": "linear"})], "turn", false).unwrap();
        let v = s.graph_tool(&json!({"clip": id})).unwrap();
        let curves = v["curves"].as_array().unwrap();
        let scale = curves.iter().find(|c| c["curve"] == "Scale X").unwrap();
        assert_eq!(scale["unit"], "%");
        assert!(scale["notes"].to_string().contains("overshoots"), "{scale}");
        assert!(scale["notes"].to_string().contains("holds still"), "after the pop it rests: {scale}");
        assert_eq!(scale["line"].as_str().unwrap().chars().count(), 24);
        // A linear move that halves its speed at a key is a kink too.
        s.edit(&[json!({"op": "animate", "clip": id, "property": "x", "keys": [[0, -400], [1, 0], [3, 400]], "ease": "linear"})], "slide", false).unwrap();
        let x = s.graph_tool(&json!({"clip": id, "property": "x"})).unwrap();
        assert!(x["curves"][0]["notes"].to_string().contains("kink at 1s"), "{x}");
        let rot = curves.iter().find(|c| c["curve"] == "Rotation").unwrap();
        assert!(rot["notes"].to_string().contains("kink at 1s"), "10°/s then 80°/s: {rot}");
        assert!((rot["fastest"]["per_second"].as_f64().unwrap() - 80.0).abs() < 1.0, "{rot}");
        let only = s.graph_tool(&json!({"clip": id, "property": "rotation", "samples": 8})).unwrap();
        assert_eq!(only["curves"].as_array().unwrap().len(), 1);
        assert!(s.graph_tool(&json!({"clip": id, "property": "opacity"})).is_err());
    }
}
