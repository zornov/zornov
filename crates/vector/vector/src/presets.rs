// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Ready-made animated shapes, each a shape document built from a few
//! options. All designs, timings and names are Studio's own.

use serde_json::{Value, json};

/// Options every preset understands.
#[derive(Clone, Debug)]
pub struct Options {
    /// Main colour.
    pub colour: String,
    /// Second colour (gradients, backgrounds).
    pub accent: String,
    /// Overall size in canvas pixels.
    pub size: f64,
    /// Line width in canvas pixels.
    pub width: f64,
    /// Seconds the entrance (or one loop) takes.
    pub speed: f64,
}

impl Default for Options {
    fn default() -> Self {
        Options { colour: "#ffffff".into(), accent: "#7c5cff".into(), size: 300.0, width: 14.0, speed: 0.8 }
    }
}

/// Preset names and what each is.
pub const PRESETS: &[(&str, &str)] = &[
    ("ring", "a progress ring that fills to the `progress` input (0..1; keyframe generator.progress)"),
    ("check", "a tick that draws itself on, in a filled circle"),
    ("cross", "a cross that draws itself on, in a filled circle"),
    ("burst", "a spiky starburst that pops in and turns slowly"),
    ("badge", "a scalloped seal badge with a gradient, popping in"),
    ("spinner", "a loading arc chasing round"),
    ("dots", "three loading dots pulsing in turn"),
    ("underline", "a hand-drawn swoosh drawing itself on (under a title)"),
    ("circle_draw", "a hand-drawn circle drawing round something"),
    ("arrow", "an arrow that draws itself on and points right"),
    ("pulse", "radar rings spreading out and fading, looping"),
    ("blob", "a soft wobbling blob with a gradient"),
    ("frame", "a rounded frame whose outline draws on (a callout box)"),
    ("bars", "equaliser bars bouncing, looping"),
    ("wipe", "a diagonal colour wipe across the frame (a transition)"),
    ("confetti", "a ring of little shapes bursting out"),
];

fn grow(o: &Options) -> Value {
    json!({"keys": [[0, 0], [o.speed, 1, "overshoot"]]})
}

/// A preset as a shape document, or `None` for an unknown name.
pub fn make(name: &str, o: &Options) -> Option<Value> {
    let s = o.size;
    let w = o.width;
    let sp = o.speed.max(0.05);
    let (c, a) = (o.colour.as_str(), o.accent.as_str());
    let draw_on = json!({"keys": [[0, 0], [sp, 1, "out"]]});
    Some(match name {
        "ring" => json!({"items": [
            {"type": "group", "items": [{"type": "circle", "radius": s / 2.0}, {"type": "stroke", "colour": a, "width": w, "opacity": 0.3}]},
            {"type": "group", "items": [{"type": "arc", "radius": s / 2.0, "start": 0, "end": "=360 * clamp(progress, 0, 1)"}, {"type": "stroke", "colour": c, "width": w, "cap": "round"}]},
        ]}),
        "check" | "cross" => {
            let r = s / 2.0;
            let mark = if name == "check" {
                json!({"type": "path", "points": [[-r * 0.42, r * 0.02], [-r * 0.12, r * 0.32], [r * 0.45, -r * 0.3]]})
            } else {
                json!({"type": "group", "items": [{"type": "line", "from": [-r * 0.3, -r * 0.3], "to": [r * 0.3, r * 0.3]}, {"type": "line", "from": [r * 0.3, -r * 0.3], "to": [-r * 0.3, r * 0.3]}]})
            };
            json!({"items": [
                {"type": "group", "scale": {"keys": [[0, 0], [sp * 0.6, 1, "overshoot"]]}, "items": [{"type": "circle", "radius": r}, {"type": "fill", "colour": a}]},
                mark,
                {"type": "trim", "end": {"keys": [[sp * 0.4, 0], [sp * 1.2, 1, "out"]]}, "mode": "sequence"},
                {"type": "stroke", "colour": c, "width": w * 1.2, "cap": "round", "join": "round"},
            ]})
        }
        "burst" => json!({"items": [
            {"type": "group", "scale": grow(o), "rotation": "=t * 20", "items": [
                {"type": "star", "points": 14, "radius": s / 2.0, "inner": s * 0.36},
                {"type": "fill", "gradient": {"type": "radial", "centre": [0, 0], "radius": s / 2.0, "stops": [c, a]}},
            ]},
        ]}),
        "badge" => json!({"items": [
            {"type": "group", "scale": grow(o), "rotation": {"keys": [[0, -30], [sp, 0, "out"]]}, "items": [
                {"type": "circle", "radius": s / 2.0},
                {"type": "zigzag", "size": s * 0.03, "ridges": 18, "points": "smooth"},
                {"type": "fill", "gradient": {"type": "linear", "from": [0, -s / 2.0], "to": [0, s / 2.0], "stops": [a, c]}},
                {"type": "group", "items": [{"type": "circle", "radius": s * 0.4}, {"type": "stroke", "colour": c, "width": w * 0.4, "dash": [w, w * 0.8]}]},
            ]},
        ]}),
        "spinner" => json!({"items": [
            {"type": "group", "items": [{"type": "circle", "radius": s / 2.0}, {"type": "stroke", "colour": a, "width": w, "opacity": 0.25}]},
            {"type": "group", "items": [
                {"type": "circle", "radius": s / 2.0},
                {"type": "trim", "start": 0, "end": "=0.15 + 0.2 * (sin(t * tau / (2 * speed)) + 1)", "offset": "=t / speed"},
                {"type": "stroke", "colour": c, "width": w, "cap": "round"},
            ]},
        ]}),
        "dots" => {
            let dot = |k: f64| json!({"type": "group", "position": [(k - 1.0) * s * 0.33, "=-abs(sin(t * pi / speed + 0.9 * ".to_string() + &k.to_string() + ")) * " + &(s * 0.08).to_string()], "opacity": "=0.35 + 0.65 * abs(sin(t * pi / speed + 0.9 * ".to_string() + &k.to_string() + "))", "items": [{"type": "circle", "radius": s * 0.09}, {"type": "fill", "colour": c}]});
            json!({"items": [dot(0.0), dot(1.0), dot(2.0)]})
        }
        "underline" => json!({"items": [
            {"type": "path", "points": [[-s, s * 0.04], [-s * 0.4, -s * 0.06], [s * 0.3, -s * 0.02], [s, -s * 0.1]], "smooth": true},
            {"type": "trim", "end": draw_on},
            {"type": "stroke", "colour": c, "width": w, "cap": "round"},
        ]}),
        "circle_draw" => json!({"items": [
            {"type": "path", "points": [[0, -s * 0.5], [s * 0.62, -s * 0.12], [s * 0.42, s * 0.42], [-s * 0.2, s * 0.5], [-s * 0.6, s * 0.06], [-s * 0.3, -s * 0.42], [s * 0.15, -s * 0.56]], "smooth": true},
            {"type": "trim", "end": draw_on},
            {"type": "stroke", "colour": c, "width": w, "cap": "round"},
        ]}),
        "arrow" => json!({"items": [
            {"type": "path", "points": [[-s, s * 0.2], [-s * 0.3, -s * 0.15], [s * 0.5, 0]], "smooth": true},
            {"type": "path", "points": [[s * 0.28, -s * 0.2], [s * 0.52, 0], [s * 0.26, s * 0.2]]},
            {"type": "trim", "end": draw_on, "mode": "sequence"},
            {"type": "stroke", "colour": c, "width": w, "cap": "round", "join": "round"},
        ]}),
        "pulse" => json!({"items": [
            {"type": "group", "items": [
                {"type": "circle", "radius": "=size_px * 0.5 * fract(t / (2 * speed) + 0 / 3)"},
                {"type": "stroke", "colour": c, "width": w, "opacity": "=1 - fract(t / (2 * speed) + 0 / 3)"},
            ]},
            {"type": "group", "items": [
                {"type": "circle", "radius": "=size_px * 0.5 * fract(t / (2 * speed) + 1 / 3)"},
                {"type": "stroke", "colour": c, "width": w, "opacity": "=1 - fract(t / (2 * speed) + 1 / 3)"},
            ]},
            {"type": "group", "items": [
                {"type": "circle", "radius": "=size_px * 0.5 * fract(t / (2 * speed) + 2 / 3)"},
                {"type": "stroke", "colour": c, "width": w, "opacity": "=1 - fract(t / (2 * speed) + 2 / 3)"},
            ]},
            {"type": "group", "items": [{"type": "circle", "radius": s * 0.06}, {"type": "fill", "colour": c}]},
        ]}),
        "blob" => json!({"items": [
            {"type": "group", "scale": grow(o), "items": [
                {"type": "circle", "radius": s / 2.0},
                {"type": "wiggle", "size": s * 0.06, "detail": 1.2, "per_second": 0.6, "correlation": 0.7},
                {"type": "fill", "gradient": {"type": "linear", "from": [-s / 2.0, -s / 2.0], "to": [s / 2.0, s / 2.0], "stops": [c, a]}},
            ]},
        ]}),
        "frame" => json!({"items": [
            {"type": "rect", "size": [s * 2.0, s * 0.9], "roundness": s * 0.08},
            {"type": "fill", "colour": a, "opacity": {"keys": [[sp * 0.6, 0], [sp * 1.2, 0.85]]}},
            {"type": "trim", "end": draw_on},
            {"type": "stroke", "colour": c, "width": w * 0.6, "cap": "round"},
        ]}),
        "bars" => {
            let bar = |k: f64| json!({"type": "rect", "position": [(k - 2.0) * s * 0.22, 0], "size": [s * 0.14, format!("={} * (0.35 + 0.65 * abs(sin(t * tau / (2 * speed) + {})))", s, k * 1.3)], "roundness": s * 0.05});
            json!({"items": [bar(0.0), bar(1.0), bar(2.0), bar(3.0), bar(4.0), {"type": "fill", "gradient": {"type": "linear", "from": [0, s / 2.0], "to": [0, -s / 2.0], "stops": [a, c]}}]})
        }
        "wipe" => json!({"items": [
            {"type": "group", "rotation": -20, "items": [
                {"type": "rect", "size": [4000, 3000], "position": {"keys": [[0, [-4000, 0]], [sp, [0, 0], "in_out"], [sp * 2.0, [4000, 0], "in_out"]]}},
                {"type": "fill", "colour": c},
            ]},
        ]}),
        "confetti" => json!({"items": [
            {"type": "group", "items": [
                {"type": "rect", "size": [s * 0.05, s * 0.1], "position": [0, {"keys": [[0, 0], [sp * 1.5, -s, "out"]]}], "roundness": s * 0.01},
                {"type": "fill", "colour": c},
            ], "opacity": {"keys": [[sp, 1], [sp * 1.5, 0]]}},
            {"type": "group", "rotation": 15, "items": [
                {"type": "circle", "radius": s * 0.03, "position": [0, {"keys": [[0, 0], [sp * 1.5, -s * 0.8, "out"]]}]},
                {"type": "fill", "colour": a},
            ], "opacity": {"keys": [[sp, 1], [sp * 1.5, 0]]}},
            {"type": "repeater", "copies": 12, "position": [0, 0], "rotation": 30},
        ]}),
        _ => return None,
    })
}

/// The inputs a preset reads, with their defaults (the agent and the app
/// store them as generator settings so they can be keyed).
pub fn inputs(name: &str, o: &Options) -> Vec<(&'static str, f64)> {
    match name {
        "ring" => vec![("progress", 0.75)],
        "spinner" | "bars" | "dots" => vec![("speed", o.speed.max(0.05))],
        "pulse" => vec![("speed", o.speed.max(0.05)), ("size_px", o.size)],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::{Ctx, Input};

    #[test]
    fn every_preset_draws_something_without_complaint() {
        let o = Options::default();
        for (name, _) in PRESETS {
            let doc = make(name, &o).unwrap_or_else(|| panic!("{name}"));
            let mut cx = Ctx::at(1.0, 3.0);
            for (k, v) in inputs(name, &o) {
                cx.inputs.insert(k.into(), Input::Num(v));
            }
            let p = crate::render(&doc, &cx, 192, 108, [1920.0, 1080.0]);
            assert!(p.warnings.is_empty(), "{name}: {:?}", p.warnings);
            let ink: u32 = p.rgba.chunks_exact(4).map(|q| q[3] as u32).sum();
            assert!(ink > 0, "{name} draws nothing at 1 s");
        }
        assert!(make("nope", &o).is_none());
    }
}
