// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Animated values in a shape document. Any number, point, colour or
//! list in the document may be:
//!
//! * as it is: `40`, `[10, 20]`, `"#ff8800"`;
//! * `"$name"`: the clip's generator setting `name` (keyframed, driven
//!   or expression-led like any other property);
//! * `"=formula"`: an expression (`t` seconds into the clip, `progress`
//!   0..1 through it, `copy` the repeater copy, `i` the component, the
//!   clip's numeric settings by name; see `studio_model::modifier::expr`);
//! * `{"keys": [[t, value], [t, value, "ease"], …], "loop": "cycle"}`:
//!   keyframes (seconds into the clip), eased into each key by the key's
//!   ease (linear, smooth, in, out, in_out, hold, overshoot, anticipate,
//!   bounce, elastic), looping `cycle` or `pingpong` if asked;
//! * `{"from": a, "to": b, "start": s, "duration": d, "ease": "out"}`:
//!   the same as two keys.

use serde_json::Value;
use std::collections::HashMap;

/// A generator setting the document can read with `$name`.
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    Num(f64),
    Point([f64; 2]),
    /// Straight RGBA, 0..1.
    Colour([f64; 4]),
    Text(String),
}

/// Where in time and what settings a document is drawn with.
#[derive(Clone, Debug, Default)]
pub struct Ctx {
    /// Seconds into the clip.
    pub t: f64,
    /// The clip's length in seconds.
    pub duration: f64,
    pub inputs: HashMap<String, Input>,
    /// The repeater copy being drawn (0 outside repeaters).
    pub copy: f64,
}

impl Ctx {
    pub fn at(t: f64, duration: f64) -> Ctx {
        Ctx { t, duration, inputs: HashMap::new(), copy: 0.0 }
    }
}

/// Eases by name, for keys (0..1 in, 0..1 out, overshooting for some).
pub fn ease(name: &str, u: f64) -> f64 {
    let u = u.clamp(0.0, 1.0);
    match name {
        "linear" => u,
        "in" | "ease_in" => u * u * u,
        "out" | "ease_out" => 1.0 - (1.0 - u).powi(3),
        "hold" => {
            if u >= 1.0 {
                1.0
            } else {
                0.0
            }
        }
        "overshoot" | "back" => {
            let c = 1.70158 * 1.2;
            let x = u - 1.0;
            1.0 + (c + 1.0) * x * x * x + c * x * x
        }
        "anticipate" => {
            let c = 1.70158;
            (c + 1.0) * u * u * u - c * u * u
        }
        "bounce" => {
            let (n, d) = (7.5625, 2.75);
            if u < 1.0 / d {
                n * u * u
            } else if u < 2.0 / d {
                let x = u - 1.5 / d;
                n * x * x + 0.75
            } else if u < 2.5 / d {
                let x = u - 2.25 / d;
                n * x * x + 0.9375
            } else {
                let x = u - 2.625 / d;
                n * x * x + 0.984375
            }
        }
        "elastic" => {
            if u <= 0.0 || u >= 1.0 {
                u
            } else {
                2f64.powf(-10.0 * u) * ((u * 10.0 - 0.75) * (std::f64::consts::TAU / 3.0)).sin() + 1.0
            }
        }
        // smooth, ease, in_out and anything else: a gentle S.
        _ => u * u * (3.0 - 2.0 * u),
    }
}

/// A colour by name, `#rgb`, `#rrggbb`, `#rrggbbaa` or `[r, g, b(, a)]`.
pub fn parse_colour(v: &Value) -> Option<[f64; 4]> {
    match v {
        Value::String(s) => {
            let s = s.trim();
            if let Some(h) = s.strip_prefix('#') {
                let hex = |a: usize, n: usize| u8::from_str_radix(h.get(a..a + n)?, 16).ok();
                return match h.len() {
                    3 => Some([hex(0, 1)? as f64 / 15.0, hex(1, 1)? as f64 / 15.0, hex(2, 1)? as f64 / 15.0, 1.0]),
                    6 => Some([hex(0, 2)? as f64 / 255.0, hex(2, 2)? as f64 / 255.0, hex(4, 2)? as f64 / 255.0, 1.0]),
                    8 => Some([hex(0, 2)? as f64 / 255.0, hex(2, 2)? as f64 / 255.0, hex(4, 2)? as f64 / 255.0, hex(6, 2)? as f64 / 255.0]),
                    _ => None,
                };
            }
            let c = match s.to_ascii_lowercase().as_str() {
                "white" => [1.0, 1.0, 1.0, 1.0],
                "black" => [0.0, 0.0, 0.0, 1.0],
                "red" => [0.9, 0.16, 0.2, 1.0],
                "green" => [0.2, 0.75, 0.35, 1.0],
                "blue" => [0.2, 0.45, 0.95, 1.0],
                "yellow" => [1.0, 0.84, 0.15, 1.0],
                "orange" => [1.0, 0.55, 0.1, 1.0],
                "purple" => [0.55, 0.3, 0.9, 1.0],
                "pink" => [1.0, 0.45, 0.7, 1.0],
                "cyan" => [0.1, 0.8, 0.9, 1.0],
                "magenta" => [0.9, 0.2, 0.8, 1.0],
                "grey" | "gray" => [0.5, 0.5, 0.5, 1.0],
                "transparent" | "none" => [0.0, 0.0, 0.0, 0.0],
                _ => return None,
            };
            Some(c)
        }
        Value::Array(a) if (3..=4).contains(&a.len()) && a.iter().all(Value::is_number) => {
            let x: Vec<f64> = a.iter().filter_map(Value::as_f64).collect();
            let k = if x.iter().take(3).any(|v| *v > 1.0) { 1.0 / 255.0 } else { 1.0 };
            Some([x[0] * k, x[1] * k, x[2] * k, x.get(3).map_or(1.0, |a| if *a > 1.0 { a / 255.0 } else { *a })])
        }
        _ => None,
    }
}

fn input_value(i: &Input) -> Value {
    match i {
        Input::Num(v) => Value::from(*v),
        Input::Point(p) => Value::from(vec![p[0], p[1]]),
        Input::Colour(c) => Value::from(c.to_vec()),
        Input::Text(s) => Value::from(s.clone()),
    }
}

/// Blends two resolved values of the same shape (numbers, lists,
/// colours); otherwise holds the first.
fn blend(a: &Value, b: &Value, u: f64) -> Value {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (x.as_f64().unwrap_or(0.0), y.as_f64().unwrap_or(0.0));
            Value::from(x + (y - x) * u)
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => Value::Array(x.iter().zip(y).map(|(p, q)| blend(p, q, u)).collect()),
        _ => match (parse_colour(a), parse_colour(b)) {
            (Some(x), Some(y)) => Value::from((0..4).map(|k| x[k] + (y[k] - x[k]) * u).collect::<Vec<f64>>()),
            _ => {
                if u >= 1.0 {
                    b.clone()
                } else {
                    a.clone()
                }
            }
        },
    }
}

impl Ctx {
    /// Evaluates an expression for component `i`.
    pub fn formula(&self, src: &str, i: usize) -> f64 {
        let mut names: Vec<&str> = vec!["copy", "progress", "duration"];
        names.extend(self.inputs.keys().filter(|k| k.chars().next().is_some_and(|c| c.is_ascii_lowercase()) && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')).map(String::as_str));
        names.sort_unstable();
        let parsed = studio_model::modifier::expr::parse_cached(src, &names);
        let Ok(e) = parsed.as_ref() else { return 0.0 };
        let v = e.eval(&|n| match n {
            "t" | "time" => Some(self.t),
            "i" => Some(i as f64),
            "value" | "v" => Some(0.0),
            "copy" => Some(self.copy),
            // The clip's own inputs come before the clip-time names, so an
            // input called `progress` or `duration` is the input.
            other => match self.inputs.get(other) {
                Some(Input::Num(x)) => Some(*x),
                Some(Input::Point(p)) => Some(p[i.min(1)]),
                _ => match other {
                    "duration" => Some(self.duration),
                    "progress" => Some(if self.duration > 0.0 { (self.t / self.duration).clamp(0.0, 1.0) } else { 0.0 }),
                    _ => None,
                },
            },
        });
        if v.is_finite() { v } else { 0.0 }
    }

    /// `v` with its animation worked out for now (lists element by
    /// element).
    pub fn resolve(&self, v: &Value) -> Value {
        match v {
            Value::String(s) if s.starts_with('$') => self.inputs.get(&s[1..]).map(input_value).unwrap_or(Value::Null),
            Value::String(s) if s.starts_with('=') => Value::from(self.formula(&s[1..], 0)),
            Value::Array(a) => Value::Array(a.iter().map(|x| self.resolve(x)).collect()),
            Value::Object(o) if o.contains_key("keys") => self.keys(o.get("keys"), o.get("loop").and_then(Value::as_str)),
            Value::Object(o) if o.contains_key("from") && o.contains_key("to") => {
                let start = o.get("start").map_or(0.0, |s| self.num(Some(s), 0.0));
                let dur = o.get("duration").map_or(1.0, |s| self.num(Some(s), 1.0)).max(1e-6);
                let e = o.get("ease").and_then(Value::as_str).unwrap_or("smooth");
                let u = ease(e, (self.t - start) / dur);
                blend(&self.resolve(&o["from"]), &self.resolve(&o["to"]), u)
            }
            other => other.clone(),
        }
    }

    fn keys(&self, keys: Option<&Value>, looping: Option<&str>) -> Value {
        let Some(Value::Array(ks)) = keys else { return Value::Null };
        let parsed: Vec<(f64, &Value, &str)> = ks
            .iter()
            .filter_map(|k| {
                let a = k.as_array()?;
                Some((a.first()?.as_f64()?, a.get(1)?, a.get(2).and_then(Value::as_str).unwrap_or("smooth")))
            })
            .collect();
        let Some(first) = parsed.first() else { return Value::Null };
        let last = parsed[parsed.len() - 1];
        let mut t = self.t;
        let span = last.0 - first.0;
        if span > 0.0 && t > last.0 {
            match looping {
                Some("cycle") | Some("repeat") => t = first.0 + (t - first.0).rem_euclid(span),
                Some("pingpong") | Some("mirror") => {
                    let c = ((t - first.0) / span).floor() as i64;
                    let local = (t - first.0).rem_euclid(span);
                    t = if c % 2 == 0 { first.0 + local } else { last.0 - local };
                }
                _ => {}
            }
        }
        if t <= first.0 || parsed.len() == 1 {
            return self.resolve(first.1);
        }
        if t >= last.0 {
            return self.resolve(last.1);
        }
        for w in parsed.windows(2) {
            if t <= w[1].0 {
                let u = (t - w[0].0) / (w[1].0 - w[0].0).max(1e-9);
                return blend(&self.resolve(w[0].1), &self.resolve(w[1].1), ease(w[1].2, u));
            }
        }
        self.resolve(last.1)
    }

    /// A number (a list gives its first element).
    pub fn num(&self, v: Option<&Value>, default: f64) -> f64 {
        let Some(v) = v else { return default };
        match self.resolve(v) {
            Value::Number(n) => n.as_f64().unwrap_or(default),
            Value::Array(a) => a.first().and_then(Value::as_f64).unwrap_or(default),
            Value::Bool(b) => b as u8 as f64,
            _ => default,
        }
    }

    /// A point: `[x, y]`, or one number for both, or a formula worked out
    /// for each axis.
    pub fn pt(&self, v: Option<&Value>, default: [f64; 2]) -> [f64; 2] {
        let Some(v) = v else { return default };
        if let Value::String(s) = v
            && let Some(f) = s.strip_prefix('=')
        {
            return [self.formula(f, 0), self.formula(f, 1)];
        }
        match self.resolve(v) {
            Value::Number(n) => {
                let x = n.as_f64().unwrap_or(0.0);
                [x, x]
            }
            Value::Array(a) => [a.first().and_then(Value::as_f64).unwrap_or(default[0]), a.get(1).and_then(Value::as_f64).unwrap_or(default[1])],
            _ => default,
        }
    }

    /// A colour (straight RGBA 0..1).
    pub fn colour(&self, v: Option<&Value>, default: [f64; 4]) -> [f64; 4] {
        let Some(v) = v else { return default };
        parse_colour(&self.resolve(v)).unwrap_or(default)
    }

    pub fn flag(&self, v: Option<&Value>, default: bool) -> bool {
        match v.map(|v| self.resolve(v)) {
            Some(Value::Bool(b)) => b,
            Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0) != 0.0,
            _ => default,
        }
    }

    pub fn text(&self, v: Option<&Value>, default: &str) -> String {
        match v.map(|v| self.resolve(v)) {
            Some(Value::String(s)) => s,
            _ => default.to_owned(),
        }
    }

    /// A list of numbers.
    pub fn nums(&self, v: Option<&Value>) -> Vec<f64> {
        match v.map(|v| self.resolve(v)) {
            Some(Value::Array(a)) => a.iter().filter_map(Value::as_f64).collect(),
            Some(Value::Number(n)) => vec![n.as_f64().unwrap_or(0.0)],
            _ => Vec::new(),
        }
    }

    /// A list of points.
    pub fn pts(&self, v: Option<&Value>) -> Vec<[f64; 2]> {
        match v.map(|v| self.resolve(v)) {
            Some(Value::Array(a)) => a.iter().filter_map(|p| Some([p.get(0)?.as_f64()?, p.get(1)?.as_f64()?])).collect(),
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn values_animate() {
        let mut cx = Ctx::at(0.5, 2.0);
        cx.inputs.insert("grow".into(), Input::Num(3.0));
        cx.inputs.insert("tint".into(), Input::Colour([1.0, 0.0, 0.0, 1.0]));
        assert_eq!(cx.num(Some(&json!(4)), 0.0), 4.0);
        assert_eq!(cx.num(Some(&json!("$grow")), 0.0), 3.0);
        assert_eq!(cx.num(Some(&json!("=grow * 2 + t")), 0.0), 6.5);
        assert_eq!(cx.num(Some(&json!({"keys": [[0, 0], [1, 100, "linear"]]})), 0.0), 50.0);
        assert_eq!(cx.num(Some(&json!({"from": 0, "to": 10, "duration": 1, "ease": "linear"})), 0.0), 5.0);
        assert_eq!(cx.pt(Some(&json!({"keys": [[0, [0, 0]], [1, [10, 20], "linear"]]})), [0.0; 2]), [5.0, 10.0]);
        assert_eq!(cx.pt(Some(&json!("=i * 10 + 1")), [0.0; 2]), [1.0, 11.0]);
        let c = cx.colour(Some(&json!({"keys": [[0, "#000000"], [1, "#ffffff", "linear"]]})), [0.0; 4]);
        assert!((c[0] - 0.5).abs() < 1e-9 && c[3] == 1.0);
        assert_eq!(cx.colour(Some(&json!("$tint")), [0.0; 4]), [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(cx.colour(Some(&json!("#f80")), [0.0; 4])[1], 8.0 / 15.0);
        // Looping keys.
        let mut late = Ctx::at(2.25, 4.0);
        late.inputs.clear();
        assert_eq!(late.num(Some(&json!({"keys": [[0, 0], [1, 100, "linear"]], "loop": "cycle"})), 0.0), 25.0);
        assert_eq!(late.num(Some(&json!({"keys": [[0, 0], [1, 100, "linear"]], "loop": "pingpong"})), 0.0), 25.0);
        assert!((ease("overshoot", 0.7) - 1.0) > 0.0);
        assert_eq!(ease("bounce", 1.0), 1.0);
    }
}
