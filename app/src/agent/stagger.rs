// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `stagger`: one move on many clips, each starting a little after the
//! last, the way GSAP staggers a list of elements.
//!
//! `{"op": "stagger", "clips": [ids], "each": 0.08, "from": "centre",
//! "animate": {"properties": {"scale": [[0, 0], [0.5, 1]]}, "ease":
//! "back"}}` writes real keyframes on every clip, shifted by its delay.
//! Instead of `animate`, `use_move` puts a saved move on each one.
//!
//! * `each`: seconds between neighbours, or `amount`: the whole spread.
//! * `from`: start, end, centre, edges, random, or a clip's place in the
//!   list (0, 1, …).
//! * `by`: the list's order (the default), or where the clips sit on the
//!   picture: `x`, `y`, or `distance` from `origin` ([x, y], the centre by
//!   default), so a grid of clips ripples out from a point.
//! * `spread`: a curve name that bunches the delays (`expo_in`: the last
//!   few come in a rush).

use super::Session;
use serde_json::{Value, json};
use studio_model as m;

impl Session {
    pub(super) fn stagger(&mut self, op: &Value) -> Result<String, String> {
        let ids: Vec<u32> = op.get("clips").and_then(Value::as_array).ok_or("stagger needs \"clips\": [ids] in the order they go")?.iter().map(|c| c.as_u64().map(|c| c as u32).ok_or("clips are ids")).collect::<Result<_, _>>()?;
        if ids.is_empty() {
            return Err("stagger needs at least one clip".into());
        }
        let n = ids.len();
        let delays = delays(op, &ids, |id| self.model_clip(id).and_then(|c| c.property_at("transform.position", 0)))?;
        let mut said = Vec::new();
        for (id, d) in ids.iter().zip(&delays) {
            let mut one = match (op.get("animate"), op.get("use_move")) {
                (Some(Value::Object(a)), None) => {
                    let mut a = a.clone();
                    a.insert("op".into(), json!("animate"));
                    if let Some(k) = a.get_mut("keys") {
                        shift_keys(k, *d)?;
                    }
                    if let Some(Value::Object(props)) = a.get_mut("properties") {
                        for k in props.values_mut() {
                            shift_keys(k, *d)?;
                        }
                    }
                    Value::Object(a)
                }
                (None, Some(mv)) => {
                    let mut o = match mv {
                        Value::String(name) => json!({"move": name}),
                        Value::Object(o) => Value::Object(o.clone()),
                        _ => return Err("use_move is a saved move's name, or {\"move\", \"fit\", \"at\"}".into()),
                    };
                    let at = o.get("at").and_then(Value::as_f64).unwrap_or(0.0);
                    o["op"] = json!("use_move");
                    o["at"] = json!(at + d);
                    o
                }
                _ => return Err("stagger takes \"animate\": {property / properties, keys, ease, …} (keys timed for the first clip), or \"use_move\": a saved move".into()),
            };
            one["clip"] = json!(id);
            let what = if one["op"] == "animate" { self.animate(&one) } else { self.use_move(&one) };
            what.map_err(|e| format!("clip {id} (delay {d:.2}s): {e}"))?;
            said.push(format!("{id}+{d:.2}s"));
        }
        Ok(format!("staggered {n} clip(s): {}", said.join(" ")))
    }
}

/// Each clip's delay in seconds, in the order of `ids`.
fn delays(op: &Value, ids: &[u32], position: impl Fn(u32) -> Option<m::Value>) -> Result<Vec<f64>, String> {
    let n = ids.len();
    let by = op.get("by").and_then(Value::as_str).unwrap_or("order");
    // Where each clip stands: its place in the list, or on the picture.
    let place: Vec<f64> = match by {
        "order" | "list" | "index" => (0..n).map(|i| i as f64).collect(),
        "x" | "y" | "distance" | "radial" => {
            let origin = op.get("origin").and_then(Value::as_array).map(|a| [a.first().and_then(Value::as_f64).unwrap_or(0.0), a.get(1).and_then(Value::as_f64).unwrap_or(0.0)]).unwrap_or([0.0, 0.0]);
            ids.iter()
                .map(|id| {
                    let p = match position(*id) {
                        Some(m::Value::Point(p)) => p,
                        _ => m::Vec2::ZERO,
                    };
                    match by {
                        "x" => p.x,
                        "y" => p.y,
                        _ => (p.x - origin[0]).hypot(p.y - origin[1]),
                    }
                })
                .collect()
        }
        other => return Err(format!("by is order, x, y or distance, not \"{other}\"")),
    };
    let (lo, hi) = place.iter().fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
    let mid = 0.5 * (lo + hi);
    let rank: Vec<f64> = match op.get("from").unwrap_or(&json!("start")) {
        Value::String(s) => match s.as_str() {
            "start" | "first" | "left" | "top" => place.iter().map(|v| v - lo).collect(),
            "end" | "last" | "right" | "bottom" => place.iter().map(|v| hi - v).collect(),
            "centre" | "center" | "middle" => place.iter().map(|v| (v - mid).abs()).collect(),
            "edges" | "outside" => place.iter().map(|v| (hi - lo) / 2.0 - (v - mid).abs()).collect(),
            "random" => {
                let mut seed = ids.iter().fold(0x9e37_79b9_7f4a_7c15u64, |s, id| s.rotate_left(7) ^ (*id as u64).wrapping_mul(0xff51_afd7_ed55_8ccd));
                let mut order: Vec<usize> = (0..n).collect();
                for i in (1..n).rev() {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    order.swap(i, (seed % (i as u64 + 1)) as usize);
                }
                let mut r = vec![0.0; n];
                for (k, i) in order.into_iter().enumerate() {
                    r[i] = k as f64;
                }
                r
            }
            other => return Err(format!("from is start, end, centre, edges, random or a place in the list, not \"{other}\"")),
        },
        Value::Number(i) => {
            let i = i.as_f64().unwrap_or(0.0);
            if by != "order" && by != "list" && by != "index" {
                return Err("from a place in the list goes with by: order".into());
            }
            place.iter().map(|v| (v - i).abs()).collect()
        }
        other => return Err(format!("from is a word or a number, not {other}")),
    };
    let top = rank.iter().cloned().fold(0.0, f64::max);
    // The whole spread: `amount`, or `each` between neighbours.
    let total = match (op.get("amount").and_then(Value::as_f64), op.get("each").and_then(Value::as_f64)) {
        (Some(a), _) => a,
        (None, Some(e)) if by == "order" || by == "list" || by == "index" || op.get("from").is_some_and(Value::is_string) && op["from"] == "random" => e * top,
        (None, Some(e)) => e * (n as f64 - 1.0),
        (None, None) => return Err("stagger needs \"each\" (seconds between clips) or \"amount\" (the whole spread)".into()),
    };
    if !(0.0..=600.0).contains(&total) {
        return Err(format!("a spread of {total:.2}s is not something to stagger by"));
    }
    let spread = match op.get("spread").and_then(Value::as_str) {
        Some(s) => Some(super::craft::named_curve(s).ok_or_else(|| format!("spread \"{s}\" is not a curve (expo_in, power2_out, sine_in_out…)"))?),
        None => None,
    };
    Ok(rank
        .iter()
        .map(|r| {
            let f = if top > 0.0 { r / top } else { 0.0 };
            total * spread.map_or(f, |c| c.apply(f))
        })
        .collect())
}

/// Moves every key in an `animate` key list `d` seconds later.
fn shift_keys(keys: &mut Value, d: f64) -> Result<(), String> {
    let Value::Array(list) = keys else { return Err("keys are a list".into()) };
    for k in list {
        let t = match k {
            Value::Array(a) => a.first_mut(),
            Value::Object(o) => o.get_mut("t"),
            _ => None,
        };
        let t = t.ok_or("each key is [t, value] or {\"t\", \"v\"}")?;
        *t = json!(t.as_f64().ok_or("a key's time is a number")? + d);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(op: Value, n: u32) -> Vec<f64> {
        let ids: Vec<u32> = (1..=n).collect();
        delays(&op, &ids, |id| Some(m::Value::Point(m::Vec2::new(id as f64 * 100.0 - 300.0, 0.0)))).unwrap().iter().map(|v| (v * 1000.0).round() / 1000.0).collect()
    }

    #[test]
    fn delays_follow_gsap() {
        assert_eq!(d(json!({"each": 0.1}), 4), vec![0.0, 0.1, 0.2, 0.3]);
        assert_eq!(d(json!({"each": 0.1, "from": "end"}), 4), vec![0.3, 0.2, 0.1, 0.0]);
        assert_eq!(d(json!({"each": 0.1, "from": "centre"}), 5), vec![0.2, 0.1, 0.0, 0.1, 0.2]);
        assert_eq!(d(json!({"each": 0.1, "from": "edges"}), 5), vec![0.0, 0.1, 0.2, 0.1, 0.0]);
        assert_eq!(d(json!({"amount": 1.0, "from": 1}), 4), vec![0.5, 0.0, 0.5, 1.0]);
        let mut r = d(json!({"each": 0.1, "from": "random"}), 6);
        r.sort_by(f64::total_cmp);
        assert_eq!(r, vec![0.0, 0.1, 0.2, 0.3, 0.4, 0.5], "every slot once");
        // By where the clips are: x from -200 to 100, from the left.
        assert_eq!(d(json!({"amount": 0.6, "by": "x"}), 4), vec![0.0, 0.2, 0.4, 0.6]);
        // Out from the centre of the picture.
        assert_eq!(d(json!({"amount": 1.0, "by": "distance"}), 4), vec![1.0, 0.5, 0.0, 0.5]);
        // A curve bunches them.
        let e = d(json!({"amount": 1.0, "spread": "expo_in"}), 5);
        assert!(e[1] < 0.05 && e[4] == 1.0, "{e:?}");
        assert!(delays(&json!({}), &[1, 2], |_| None).is_err());
    }

    #[test]
    fn keys_move_later() {
        let mut k = json!([[0, 0], {"t": 0.5, "v": 1}]);
        shift_keys(&mut k, 0.25).unwrap();
        assert_eq!(k, json!([[0.25, 0], {"t": 0.75, "v": 1}]));
    }
}
