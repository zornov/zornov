// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Light, air and look of a 3D scene, for agents.
//!
//! * How a lamp shows itself is set on its `light` clip (`glow`, `flare`,
//!   `beam`, `source_size`, `flicker`, `flicker_speed`, and a `flame` with
//!   its `flame_height`, `flame_width` and `flame_tongues`); see
//!   [`set_light_look`].
//! * The scene's air and look sit on one light clip with `atmosphere` on:
//!   the `atmosphere` op makes or finds it (an ambient light with no light
//!   of its own, named "Air and look") and sets fog, weather, bloom,
//!   exposure, tone map, grade, vignette, grain and halation on it, from a
//!   named `preset`, from fields, or both (fields win); see
//!   [`Session::atmosphere`].
//! * A model's `wetness` and `emissive_strength` are in `models.rs`.
//!
//! Every value is checked against the range the renderer works in, and an
//! error says what the value means and what to write instead. The keys are
//! the ones `studio_render::atmosphere::params` reads.

use super::{Session, f, micros};
use serde_json::{Map, Value, json};
use studio_model as m;

/// Number fields of the air and look: key, lowest, highest, what it is and
/// what to write.
const NUMBERS: &[(&str, f64, f64, &str)] = &[
    ("metre", 1.0, 5000.0, "pixels in a metre of the scene (default: the canvas height over 3.5, a person is half the canvas)"),
    ("fog_density", 0.0, 0.5, "how thick the fog is per metre: 0.01 haze, 0.03 mist, 0.08 fog, 0.2 dense"),
    ("fog_falloff", 0.0, 3.0, "how fast the fog thins with height per metre: 0 even all the way up, 0.15 a bank a few metres deep"),
    ("fog_base", -1.0e6, 1.0e6, "world y of the bottom of the fog (px, y down): the ground"),
    ("fog_anisotropy", -0.95, 0.95, "how much fog glows toward a light: 0 even, 0.5 halos round lamps"),
    ("fog_range", 1.0, 2000.0, "how far, in metres, the fog is followed"),
    ("fog_patchiness", 0.0, 1.0, "0 even fog, 1 banks with clear gaps"),
    ("bloom_strength", 0.0, 1.5, "how much bright light spills: 0.15 subtle, 0.3 dreamy, 0.6 heavy; over 1 washes the picture out"),
    ("bloom_threshold", 0.0, 10.0, "brightness from which light spills (1 = white): 0.8 to 1.2"),
    ("bloom_knee", 0.0, 2.0, "softness of the bloom threshold: 0.5 soft"),
    ("bloom_radius", 0.0, 1.0, "how far the bloom reaches: 0.3 tight, 0.7 wide"),
    ("exposure", -6.0, 6.0, "stops brighter (+) or darker (-) than the automatic exposure: -1 is half as bright, +1 twice"),
    ("contrast", 0.2, 3.0, "1 is none: 1.1 to 1.3 punchy, 0.8 flat"),
    ("saturation", 0.0, 3.0, "1 is none: 0 grey, 1.2 vivid"),
    ("temperature", -1.0, 1.0, "white balance: -1 cool blue, 1 warm orange"),
    ("tint", -1.0, 1.0, "-1 green, 1 magenta"),
    ("richness", 0.0, 1.0, "deepens the darks and keeps their colour, no muddy grey: 0.3 to 0.6"),
    ("shadow_tint_amount", 0.0, 2.0, "how much of shadow_tint goes into the shadows: 0.5"),
    ("highlight_tint_amount", 0.0, 2.0, "how much of highlight_tint goes into the highlights: 0.5"),
    ("vignette", 0.0, 1.0, "darkening toward the corners: 0.15 to 0.35"),
    ("grain", 0.0, 1.0, "film grain: 0.05 to 0.2"),
    ("halation", 0.0, 1.0, "warm glow round highlights, like film: 0.1 to 0.3"),
    ("fringe", 0.0, 1.0, "colour fringing at the edges of the frame: 0.1 to 0.3"),
];

/// Colour fields of the air and look.
const COLOURS: &[(&str, &str)] = &[
    ("fog_colour", "colour of the fog, #rrggbb or a name (warm #ffe0c0 dawn, cold #9fb4d8 dusk)"),
    ("shadow_tint", "colour pushed into the shadows (teal #1a4060 for the classic film look)"),
    ("highlight_tint", "colour pushed into the highlights (warm #ffc080)"),
    ("reflection", "what mirrors and wet ground see where nothing is on screen (the sky)"),
    ("air_light", "ambient light the air and the particles take (default: the scene's ambient lights)"),
];

const TONEMAPS: &[(&str, &[&str])] = &[("none", &["none", "off", "linear"]), ("filmic", &["filmic", "aces", "film", "cinematic"]), ("soft", &["soft", "reinhard", "gentle", "shoulder"])];

/// The kinds of weather, with the other words that mean them.
const WEATHER: &[(&str, &[&str])] = &[
    ("rain", &["rain", "drizzle", "downpour", "storm", "shower"]),
    ("snow", &["snow", "snowfall", "flurries", "blizzard"]),
    ("dust", &["dust", "motes", "dust_motes", "pollen", "particles", "specks"]),
    ("embers", &["embers", "ember", "cinders", "firefly", "fireflies"]),
    ("sparks", &["sparks", "spark", "welding"]),
    ("leaves", &["leaves", "leaf", "falling_leaves", "petals", "autumn"]),
    ("ash", &["ash", "ashfall", "ashes", "fallout"]),
    ("smoke", &["smoke", "wisps", "steam", "haze_smoke"]),
];

const WEATHER_NUMBERS: &[(&str, f64, f64, &str)] = &[
    ("amount", 0.0, 1.5, "how much: 0.2 a sprinkle, 0.6 steady, 1 a downpour or blizzard"),
    ("wind_x", -40.0, 40.0, "wind to the right in metres per second"),
    ("wind_y", -40.0, 40.0, "wind downward in metres per second"),
    ("wind_z", -40.0, 40.0, "wind away from the camera in metres per second"),
    ("size", 0.1, 8.0, "multiplies the size of the particles: 1 normal"),
    ("speed", 0.0, 6.0, "multiplies how fast they move: 1 normal"),
    ("splash", 0.0, 1.0, "rain on the ground: ripples and splashes"),
];

/// Every parameter key the air and look sit in (what a `preset` replaces).
fn air_keys() -> Vec<String> {
    let mut v: Vec<String> = ["ground", "tonemap", "auto_exposure"].iter().map(|s| (*s).to_owned()).collect();
    v.extend(NUMBERS.iter().map(|n| n.0.to_owned()));
    v.extend(COLOURS.iter().map(|c| c.0.to_owned()));
    for i in 1..=3 {
        let k = format!("weather_{i}");
        v.extend(WEATHER_NUMBERS.iter().map(|n| format!("{k}_{}", n.0)));
        v.extend(["", "_colour", "_zone"].iter().map(|s| format!("{k}{s}")));
    }
    v
}

/// What the renderer takes as the tone map and weather names.
fn canonical(table: &[(&'static str, &[&str])], word: &str) -> Option<&'static str> {
    let w = word.trim().to_lowercase().replace([' ', '-'], "_");
    table.iter().find(|(_, names)| names.contains(&w.as_str())).map(|(c, _)| *c)
}

fn names(table: &[(&str, &[&str])]) -> String {
    table.iter().map(|(c, _)| *c).collect::<Vec<_>>().join(", ")
}

fn number(key: &str, v: &Value, lo: f64, hi: f64, what: &str) -> Result<f64, String> {
    let x = match v {
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        v => v.as_f64(),
    }
    .filter(|x| x.is_finite())
    .ok_or_else(|| format!("{key} is a number ({what}); you gave {v}"))?;
    if x < lo || x > hi {
        return Err(format!("{key} {x} is out of range ({what}): write a value from {lo} to {hi}"));
    }
    Ok(x)
}

fn colour(key: &str, v: &Value, what: &str) -> Result<m::Value, String> {
    let s = v.as_str().ok_or_else(|| format!("{key} is a colour, #rrggbb or a name ({what})"))?;
    let k = super::craft::colour(s).ok_or_else(|| format!("{key} \"{s}\" is not #rrggbb or a basic name ({what}): write it like \"#ffd9a0\""))?;
    Ok(m::Value::Colour(m::Rgba::new(k.r() as f64 / 255.0, k.g() as f64 / 255.0, k.b() as f64 / 255.0, 1.0)))
}

fn put(p: &mut m::ParamMap, k: &str, v: m::Value) {
    p.insert(k.to_owned(), v);
}

/// Sets how a light shows itself from an op's fields, checking each; the
/// notes say what to do next. `kind` is the light's type.
pub(super) fn set_light_look(op: &Value, p: &mut m::ParamMap, kind: m::LightKind) -> Result<Vec<String>, String> {
    let mut notes = Vec::new();
    let has = |k: &str| op.get(k).is_some_and(|v| !v.is_null());
    let fixed_point = matches!(kind, m::LightKind::Point | m::LightKind::Spot);
    for k in ["glow", "flare", "flame", "flame_height", "flame_width", "flame_tongues", "source_size"] {
        if has(k) && !fixed_point && op.get(k) != Some(&json!(0)) && op.get(k) != Some(&json!(false)) {
            let what = if kind == m::LightKind::Ambient { "an ambient light has no place in the scene" } else { "a sun is not a place in the scene" };
            return Err(format!("{k} on a {} light: {what}, so it has nothing to draw; put {k} on a point or spot light (a lamp, bulb, candle or neon tube) at the source", kind.name()));
        }
    }
    let range = |key: &str, lo: f64, hi: f64, what: &str| -> Result<Option<f64>, String> {
        match op.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => number(key, v, lo, hi, what).map(Some),
        }
    };
    let scalar = |p: &mut m::ParamMap, key: &str, x: Option<f64>| {
        if let Some(x) = x {
            if x == 0.0 {
                p.remove(key);
            } else {
                put(p, key, m::Value::Scalar(x));
            }
        }
    };
    scalar(p, "glow", range("glow", 0.0, 5.0, "strength of the halo round the source: 0.3 soft, 1 clear, 2 strong; 0 removes it")?);
    scalar(p, "flare", range("flare", 0.0, 3.0, "strength of the horizontal lens streak: 0.3 subtle, 1 anamorphic; 0 removes it")?);
    scalar(p, "beam", range("beam", 0.0, 3.0, "strength of the cone or shaft its light makes in fog and dust: 0.5 soft, 1 clear; 0 removes it")?);
    scalar(p, "source_size", range("source_size", 0.0, 5.0, "radius of the source in metres: a bulb 0.05, a lantern 0.15, a window 1")?);
    scalar(p, "flicker", range("flicker", 0.0, 1.0, "how unsteady the light is: 0.15 a bulb, 0.35 a candle or fire, 0.6 a failing tube; 0 removes it")?);
    scalar(p, "flicker_speed", range("flicker_speed", 0.1, 30.0, "flicker rate in Hz: 6 to 9 for fire")?);
    if let Some(b) = op.get("flame") {
        let on = b.as_bool().or_else(|| b.as_f64().map(|x| x > 0.0)).ok_or("flame is true or false: true draws this light as a flame (set flame_height in metres: candle 0.05, torch 0.3, campfire 0.8, bonfire 2.5)")?;
        if on {
            put(p, "flame", m::Value::Bool(true));
        } else {
            p.remove("flame");
        }
    }
    let flame_on = matches!(p.get("flame"), Some(m::Value::Bool(true)));
    for (key, lo, hi, what) in [("flame_height", 0.005, 20.0, "height of the flame in metres: candle 0.05, torch 0.3, campfire 0.8, bonfire 2.5"), ("flame_width", 0.003, 10.0, "width of the flame at its widest in metres: candle 0.02, campfire 0.5"), ("flame_tongues", 1.0, 12.0, "number of tongues: 1 candle, 3 to 6 fire")] {
        if let Some(x) = range(key, lo, hi, what)? {
            if !flame_on {
                return Err(format!("{key} needs the light to be a flame: add \"flame\": true to the same op ({what})"));
            }
            put(p, key, m::Value::Scalar(x));
        }
    }
    if has("flicker_speed") && !p.contains_key("flicker") {
        return Err("flicker_speed needs flicker: add \"flicker\": 0.35 (how unsteady, 0 to 1) to the same op".into());
    }
    if let Some(b) = op.get("atmosphere") {
        let on = b.as_bool().ok_or("atmosphere is true or false: true makes this light clip the one that carries the scene's air and look; set them with the atmosphere op {clip: this id, fog_density, weather, ...}")?;
        if on {
            put(p, "atmosphere", m::Value::Bool(true));
            notes.push("this light now carries the scene's air and look: set fog, weather and look with {\"op\": \"atmosphere\", \"clip\": it}; give the carrier intensity 0 if it should light nothing".to_owned());
        } else {
            p.remove("atmosphere");
        }
    }
    if p.get("beam").is_some() {
        notes.push("a beam shows only in air: fog or dust must be on (atmosphere {fog_density: 0.03} or weather: \"dust\")".to_owned());
    }
    Ok(notes)
}

/// A preset: a name, what it is for, and its fields.
struct Preset {
    name: &'static str,
    words: &'static str,
    fields: &'static [(&'static str, &'static str)],
}

const PRESETS: &[Preset] = &[
    Preset {
        name: "neon_rain",
        words: "night street in rain: wet shine, cold mist, teal shadows, bloom on the signs",
        fields: &[("fog_density", "0.012"), ("fog_falloff", "0.1"), ("fog_colour", "#8c99cc"), ("fog_anisotropy", "0.5"), ("fog_patchiness", "0.2"), ("weather", "rain"), ("bloom_strength", "0.3"), ("bloom_threshold", "1.0"), ("tonemap", "filmic"), ("richness", "0.6"), ("contrast", "1.1"), ("vignette", "0.25"), ("grain", "0.12"), ("halation", "0.2"), ("shadow_tint", "#1a4080"), ("shadow_tint_amount", "0.6")],
    },
    Preset {
        name: "candlelit",
        words: "a warm interior by flame: soft dust in the glow, deep warm shadows",
        fields: &[("fog_density", "0.012"), ("fog_colour", "#ffcc99"), ("fog_anisotropy", "0.5"), ("fog_range", "25"), ("weather", "dust"), ("bloom_strength", "0.25"), ("bloom_threshold", "0.9"), ("tonemap", "filmic"), ("richness", "0.5"), ("temperature", "0.2"), ("vignette", "0.35"), ("grain", "0.15")],
    },
    Preset {
        name: "foggy_dawn",
        words: "low sun through mist with light shafts and floating pollen",
        fields: &[("fog_density", "0.03"), ("fog_falloff", "0.12"), ("fog_colour", "#ffe6cc"), ("fog_anisotropy", "0.6"), ("fog_range", "70"), ("fog_patchiness", "0.45"), ("weather", "dust"), ("bloom_strength", "0.2"), ("bloom_threshold", "0.9"), ("tonemap", "filmic"), ("richness", "0.45"), ("contrast", "1.15"), ("saturation", "1.15"), ("temperature", "0.25"), ("vignette", "0.2"), ("grain", "0.08")],
    },
    Preset {
        name: "snowy_dusk",
        words: "falling snow in blue evening air, windows glowing warm",
        fields: &[("fog_density", "0.025"), ("fog_falloff", "0.05"), ("fog_colour", "#99aed9"), ("fog_anisotropy", "0.4"), ("fog_patchiness", "0.3"), ("weather", "snow"), ("bloom_strength", "0.25"), ("bloom_threshold", "0.9"), ("tonemap", "filmic"), ("richness", "0.3"), ("temperature", "-0.2"), ("vignette", "0.2"), ("grain", "0.1")],
    },
    Preset {
        name: "desert_noon",
        words: "hard sun, clear dry air, a little blowing dust, deep sky",
        fields: &[("fog_density", "0.004"), ("fog_falloff", "0.02"), ("fog_colour", "#ffe0b0"), ("fog_range", "120"), ("weather", "dust"), ("bloom_strength", "0.1"), ("bloom_threshold", "1.1"), ("tonemap", "filmic"), ("richness", "0.35"), ("contrast", "1.2"), ("saturation", "1.15"), ("temperature", "0.1"), ("vignette", "0.15"), ("grain", "0.05")],
    },
    Preset {
        name: "overcast_mist",
        words: "grey soft day, wet air, quiet colours",
        fields: &[("fog_density", "0.02"), ("fog_falloff", "0.1"), ("fog_colour", "#c4ccd6"), ("fog_range", "60"), ("weather", "drizzle"), ("bloom_strength", "0.1"), ("tonemap", "soft"), ("richness", "0.3"), ("saturation", "0.9"), ("temperature", "-0.15"), ("vignette", "0.15"), ("grain", "0.08")],
    },
    Preset {
        name: "storm",
        words: "heavy rain, wind, dark sky, flat cold light",
        fields: &[("fog_density", "0.03"), ("fog_falloff", "0.08"), ("fog_colour", "#7a8499"), ("weather", "rain"), ("weather_1_amount", "1"), ("weather_1_wind_x", "6"), ("bloom_strength", "0.15"), ("tonemap", "filmic"), ("richness", "0.5"), ("contrast", "1.15"), ("saturation", "0.85"), ("temperature", "-0.25"), ("vignette", "0.3"), ("grain", "0.12")],
    },
    Preset {
        name: "ember_smoke",
        words: "a fire's smoke and rising embers, warm glow, orange halation",
        fields: &[("fog_density", "0.02"), ("fog_colour", "#a08060"), ("fog_anisotropy", "0.4"), ("weather", "embers"), ("bloom_strength", "0.3"), ("bloom_threshold", "0.9"), ("tonemap", "filmic"), ("richness", "0.5"), ("temperature", "0.2"), ("halation", "0.3"), ("vignette", "0.3"), ("grain", "0.12")],
    },
    Preset {
        name: "noir_haze",
        words: "hard single key, drifting haze, crushed-but-rich blacks, cool shadows",
        fields: &[("fog_density", "0.02"), ("fog_colour", "#a0a8b8"), ("fog_anisotropy", "0.6"), ("weather", "dust"), ("bloom_strength", "0.15"), ("tonemap", "filmic"), ("richness", "0.6"), ("contrast", "1.2"), ("saturation", "0.8"), ("vignette", "0.35"), ("grain", "0.15"), ("shadow_tint", "#203050"), ("shadow_tint_amount", "0.5")],
    },
    Preset {
        name: "clean_film",
        words: "no air at all: just the film look (exposure, filmic tone, rich darks, a little vignette and grain)",
        fields: &[("bloom_strength", "0.1"), ("tonemap", "filmic"), ("richness", "0.35"), ("contrast", "1.08"), ("vignette", "0.15"), ("grain", "0.06")],
    },
];

pub(super) fn preset_names() -> String {
    PRESETS.iter().map(|p| p.name).collect::<Vec<_>>().join(", ")
}

fn preset(name: &str) -> Option<&'static Preset> {
    let k = name.trim().to_lowercase().replace([' ', '-'], "_");
    PRESETS.iter().find(|p| p.name == k).or(match k.as_str() {
        "neon" | "rainy_night" | "night_rain" | "cyberpunk" => PRESETS.first(),
        "candle" | "candlelight" | "firelight" | "fire" => PRESETS.get(1),
        "fog" | "foggy" | "forest" | "dawn" | "morning" => PRESETS.get(2),
        "snow" | "snowy" | "dusk" | "winter" => PRESETS.get(3),
        "desert" | "noon" | "sunny" | "day" => PRESETS.get(4),
        "overcast" | "grey" | "gray" | "drizzle" | "mist" => PRESETS.get(5),
        "stormy" | "rain" | "thunder" => PRESETS.get(6),
        "embers" | "smoky" | "campfire" => PRESETS.get(7),
        "noir" | "haze" | "smoky_room" => PRESETS.get(8),
        "film" | "clean" | "cinematic" | "look" => PRESETS.get(9),
        _ => None,
    })
}

/// The lighting preset's matching air.
pub(super) fn preset_for_lighting(setup: &str) -> &'static str {
    match setup {
        "firelight" => "candlelit",
        "neon" => "neon_rain",
        "golden_hour" => "foggy_dawn",
        "blue_hour" | "moonlight" => "snowy_dusk",
        "overcast" => "overcast_mist",
        "low_key" | "split" | "under" => "noir_haze",
        "rembrandt" | "rim" | "silhouette" => "noir_haze",
        _ => "clean_film",
    }
}

/// Weather layers from a `weather` field: a word ("rain"), an object, or a
/// list of them (at most three).
fn weather_layers(v: &Value) -> Result<Vec<Map<String, Value>>, String> {
    let list: Vec<Value> = match v {
        Value::Array(a) => a.clone(),
        Value::Null | Value::Bool(false) => Vec::new(),
        other => vec![other.clone()],
    };
    if list.len() > 3 {
        return Err(format!("weather takes at most 3 layers (rain, snow, dust, embers, sparks, leaves, ash, smoke), you gave {}: keep the three that matter most to the shot", list.len()));
    }
    let mut out = Vec::new();
    for item in list {
        let mut o = match item {
            Value::String(s) => {
                let mut o = Map::new();
                o.insert("kind".into(), Value::String(s));
                o
            }
            Value::Object(o) => o,
            other => return Err(format!("a weather layer is a word (\"rain\") or an object {{kind, amount, wind_x, ...}}; you gave {other}")),
        };
        if let Some(w) = o.remove("wind") {
            let a = w.as_array().filter(|a| a.len() == 3).ok_or("wind is [x, y, z] in metres per second (x right, y down, z away), for example [4, 0, 0] for a breeze from the left")?;
            for (k, v) in ["wind_x", "wind_y", "wind_z"].iter().zip(a) {
                o.insert((*k).to_owned(), v.clone());
            }
        }
        out.push(o);
    }
    Ok(out)
}

/// Sets the scene's air and look on `p` from fields (checked). Returns what
/// it understood, as words for the reply.
pub(super) fn set_air(p: &mut m::ParamMap, fields: &Map<String, Value>) -> Result<Vec<String>, String> {
    let mut said = Vec::new();
    put(p, "atmosphere", m::Value::Bool(true));
    for (key, v) in fields {
        if v.is_null() {
            continue;
        }
        if let Some((_, lo, hi, what)) = NUMBERS.iter().find(|n| n.0 == key) {
            let x = number(key, v, *lo, *hi, what)?;
            put(p, key, m::Value::Scalar(x));
            said.push(format!("{key} {x}"));
        } else if let Some((_, what)) = COLOURS.iter().find(|c| c.0 == key) {
            put(p, key, colour(key, v, what)?);
            said.push(format!("{key} {v}"));
        }
    }
    if let Some(v) = fields.get("ground") {
        let g = number("ground", v, -1.0e6, 1.0e6, "world y of the floor in px, y down: the y of your floor plane or the lowest model's bottom; \"auto\" finds it")?;
        put(p, "ground", m::Value::Scalar(g));
        if !fields.contains_key("fog_base") && p.contains_key("fog_base") {
            put(p, "fog_base", m::Value::Scalar(g));
        }
        said.push(format!("ground y {g}"));
    }
    if let Some(v) = fields.get("tonemap") {
        let s = v.as_str().ok_or_else(|| format!("tonemap is a word: {}", names(TONEMAPS)))?;
        let c = canonical(TONEMAPS, s).ok_or_else(|| format!("tonemap \"{s}\": write one of {} (filmic for almost everything, soft for flat looks, none leaves bright light clipped)", names(TONEMAPS)))?;
        put(p, "tonemap", m::Value::Choice(c.into()));
        said.push(format!("tonemap {c}"));
    }
    if let Some(v) = fields.get("auto_exposure") {
        let b = v.as_bool().ok_or("auto_exposure is true or false: true (the default) lowers the exposure of a scene that would blow out; exposure then adds to it. false leaves exposure exactly as written")?;
        put(p, "auto_exposure", m::Value::Bool(b));
        said.push(format!("auto_exposure {b}"));
    }
    if let Some(v) = fields.get("fog") {
        if matches!(v, Value::Bool(false)) {
            for k in p.keys().filter(|k| k.starts_with("fog_")).cloned().collect::<Vec<_>>() {
                p.remove(&k);
            }
            said.push("fog off".into());
        } else if matches!(v, Value::Bool(true)) && !fields.contains_key("fog_density") && !p.contains_key("fog_density") {
            put(p, "fog_density", m::Value::Scalar(0.03));
            said.push("fog_density 0.03".into());
        }
    }
    if let Some(v) = fields.get("weather") {
        let layers = weather_layers(v)?;
        for k in p.keys().filter(|k| k.starts_with("weather_")).cloned().collect::<Vec<_>>() {
            p.remove(&k);
        }
        if layers.is_empty() {
            said.push("weather off".into());
        }
        for (i, layer) in layers.iter().enumerate() {
            let key = format!("weather_{}", i + 1);
            let kind = layer.get("kind").and_then(Value::as_str).ok_or_else(|| format!("a weather layer needs \"kind\": one of {}", names(WEATHER)))?;
            let c = canonical(WEATHER, kind).ok_or_else(|| format!("weather \"{kind}\" is not a kind: write one of {}", names(WEATHER)))?;
            put(p, &key, m::Value::Choice(c.into()));
            for (name, lo, hi, what) in WEATHER_NUMBERS {
                if let Some(v) = layer.get(*name) {
                    put(p, &format!("{key}_{name}"), m::Value::Scalar(number(&format!("weather {name}"), v, *lo, *hi, what)?));
                }
            }
            if let Some(v) = layer.get("colour").or(layer.get("color")) {
                put(p, &format!("{key}_colour"), colour("weather colour", v, "colour of the particles")?);
            }
            if let Some(z) = layer.get("zone") {
                let nums: Vec<f64> = match z {
                    Value::Array(a) => a.iter().filter_map(Value::as_f64).collect(),
                    Value::String(s) => s.split(|c: char| c.is_whitespace() || c == ',').filter_map(|x| x.parse().ok()).collect(),
                    _ => Vec::new(),
                };
                if nums.len() != 6 {
                    return Err("weather zone is the box the particles stay in: [x0, y0, z0, x1, y1, z1] in world px (smoke over a chimney, embers over a fire); leave it out to fill the air round the camera".into());
                }
                put(p, &format!("{key}_zone"), m::Value::Text(nums.iter().map(|x| format!("{x}")).collect::<Vec<_>>().join(" ")));
            }
            said.push(format!("weather {c}{}", layer.get("amount").and_then(Value::as_f64).map(|a| format!(" {a}")).unwrap_or_default()));
        }
    }
    // Per-layer fields set directly (weather_1_amount), for presets.
    for (key, v) in fields {
        if let Some(rest) = key.strip_prefix("weather_") {
            let Some((n, name)) = rest.split_once('_') else { continue };
            if !matches!(n, "1" | "2" | "3") {
                continue;
            }
            let Some((_, lo, hi, what)) = WEATHER_NUMBERS.iter().find(|w| w.0 == name) else { continue };
            if !p.contains_key(&format!("weather_{n}")) {
                return Err(format!("{key} needs weather layer {n}: add \"weather\": [\"rain\"] to the same op"));
            }
            put(p, key, m::Value::Scalar(number(key, v, *lo, *hi, what)?));
        }
    }
    Ok(said)
}

/// The fields `atmosphere` takes, as the argument check lists them.
#[cfg(test)]
fn atmosphere_fields() -> Vec<&'static str> {
    let mut v = vec!["clip", "at", "duration", "lane", "preset", "list", "remove", "ground", "fog", "weather", "tonemap", "auto_exposure"];
    v.extend(NUMBERS.iter().map(|n| n.0));
    v.extend(COLOURS.iter().map(|c| c.0));
    v
}

impl Session {
    /// The light clip that carries the scene's air and look at `t`.
    pub(super) fn air_carrier(&self, t: m::Micros) -> Option<u32> {
        let seq = self.doc.model.sequence(self.doc.seq())?;
        seq.clips
            .iter()
            .filter(|c| c.enabled && matches!(&c.source, m::ClipSource::Generator(g) if g.kind == m::GeneratorKind::Light && matches!(g.params.get("atmosphere"), Some(m::Value::Bool(true)))))
            .find(|c| c.start <= t && t < c.start + c.duration)
            .or_else(|| seq.clips.iter().find(|c| c.enabled && matches!(&c.source, m::ClipSource::Generator(g) if g.kind == m::GeneratorKind::Light && matches!(g.params.get("atmosphere"), Some(m::Value::Bool(true))))))
            .map(|c| c.id.0 as u32)
    }

    /// The lowest bottom edge of the models, as the world y of the floor.
    fn lowest_model_bottom(&self) -> Option<f64> {
        let seq = self.doc.model.sequence(self.doc.seq())?;
        let mut best: Option<f64> = None;
        for c in seq.clips.iter().filter(|c| c.enabled) {
            if !matches!(&c.source, m::ClipSource::Generator(g) if g.kind == m::GeneratorKind::Model) {
                continue;
            }
            let Ok(mesh) = crate::mesh_store::load(c) else { continue };
            let (_, hi) = mesh.bounds();
            let bottom = c.transform.position.y + hi[1] as f64 * c.transform.scale_y.max(0.0);
            best = Some(best.map_or(bottom, |b: f64| b.max(bottom)));
        }
        best
    }

    /// `atmosphere`: the scene's air (fog, weather) and look (exposure, bloom,
    /// tone map, grade, vignette, grain, halation), kept on one light clip.
    pub(super) fn atmosphere(&mut self, op: &Value) -> Result<String, String> {
        if op.get("list").and_then(Value::as_bool) == Some(true) {
            let presets: Vec<String> = PRESETS.iter().map(|p| format!("{}: {}", p.name, p.words)).collect();
            return Ok(format!("presets: {}", presets.join("; ")));
        }
        let at = micros(f(op, "at").unwrap_or(0.0));
        let found = match op.get("clip").and_then(Value::as_u64) {
            Some(id) => Some(id as u32),
            None => self.air_carrier(at),
        };
        if op.get("remove").and_then(Value::as_bool) == Some(true) {
            let id = found.ok_or("no air and look in this scene: nothing to remove")?;
            let mut mc = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
            mc.enabled = false;
            self.apply_clip("Atmosphere", mc)?;
            return Ok(format!("air and look off (clip {id} is switched off; {{\"op\": \"set\", \"clip\": {id}, \"enabled\": true}} brings it back)"));
        }
        let mut fields: Map<String, Value> = Map::new();
        let mut preset_said = String::new();
        let mut replaced = false;
        if let Some(name) = op.get("preset").and_then(Value::as_str) {
            let pr = preset(name).ok_or_else(|| format!("preset \"{name}\": write one of {}", preset_names()))?;
            for (k, v) in pr.fields {
                fields.insert((*k).to_owned(), v.parse::<f64>().map(|x| json!(x)).unwrap_or_else(|_| json!(v)));
            }
            preset_said = format!("preset {} ({})", pr.name, pr.words);
            replaced = true;
        } else if let Some(v) = op.get("preset") {
            return Err(format!("preset is a name: {}; you gave {v}", preset_names()));
        }
        for (k, v) in op.as_object().into_iter().flatten() {
            if !matches!(k.as_str(), "op" | "clip" | "at" | "duration" | "lane" | "preset" | "list" | "remove") && !super::args::is_note(k) {
                fields.insert(k.clone(), v.clone());
            }
        }
        if let Some(Value::String(s)) = fields.get("ground")
            && s.trim().eq_ignore_ascii_case("auto")
        {
            let g = self.lowest_model_bottom().ok_or("ground \"auto\" finds the floor from the models, and there are none: add the floor model first, or give ground as a number (world y in px, y down)")?;
            fields.insert("ground".into(), json!(g));
        }
        if fields.is_empty() {
            return Err(format!("atmosphere needs something to set: a preset ({}), or fields such as fog_density, weather, bloom_strength, tonemap, exposure; {{\"list\": true}} describes the presets", preset_names()));
        }
        // The carrier: given, found, or a new ambient light that lights nothing.
        let (id, made) = match found {
            Some(id) => (id, false),
            None => {
                let mut params = m::LightSettings::defaults(m::LightKind::Ambient);
                params.insert("intensity".into(), m::Value::Scalar(0.0));
                (self.add_rig(m::GeneratorKind::Light, op, "Air and look", params, [0.0; 3])?, true)
            }
        };
        let mut mc = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
        let m::ClipSource::Generator(g) = &mut mc.source else { return Err(format!("clip {id} is not a light")) };
        if g.kind != m::GeneratorKind::Light {
            return Err(format!("clip {id} is not a light: atmosphere goes on a light clip (leave out \"clip\" to make one)"));
        }
        if replaced {
            for k in air_keys() {
                g.params.remove(&k);
            }
        }
        let said = set_air(&mut g.params, &fields)?;
        let lit = !matches!(g.params.get("intensity"), Some(m::Value::Scalar(x)) if *x <= 0.0) && !made;
        self.apply_clip("Atmosphere", mc)?;
        let mut out = format!("air and look on light clip {id}{}: {}{}", if made { " (new, lights nothing)" } else { "" }, if preset_said.is_empty() { String::new() } else { format!("{preset_said}; ") }, said.join(", "));
        if lit {
            out.push_str("; note: this clip is also a light that lights the scene; give it intensity 0 ({\"op\": \"light\", \"clip\": id, \"intensity\": 0}) if it should only carry the air");
        }
        if !self.doc.model.sequence(self.doc.seq()).is_some_and(|q| q.clips.iter().any(|c| c.enabled && matches!(&c.source, m::ClipSource::Generator(g) if g.kind == m::GeneratorKind::Model))) {
            out.push_str("; note: air and look draw on 3D models (model, asset, build_world): there are none yet");
        }
        if fields.contains_key("fog_density") || fields.contains_key("weather") || replaced {
            out.push_str("; the floor is at world y 0 unless you set ground (a number, or \"auto\" to take the lowest model's bottom)");
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(v: Value) -> Map<String, Value> {
        v.as_object().cloned().unwrap_or_default()
    }

    #[test]
    fn every_preset_passes_its_own_checks() {
        for pr in PRESETS {
            let mut p = m::ParamMap::new();
            let fields: Map<String, Value> = pr.fields.iter().map(|(k, v)| ((*k).to_owned(), v.parse::<f64>().map(|x| json!(x)).unwrap_or_else(|_| json!(v)))).collect();
            set_air(&mut p, &fields).unwrap_or_else(|e| panic!("{}: {e}", pr.name));
            assert!(matches!(p.get("atmosphere"), Some(m::Value::Bool(true))));
        }
    }

    #[test]
    fn errors_say_the_fix() {
        let mut p = m::ParamMap::new();
        let e = set_air(&mut p, &obj(json!({"bloom_strength": 4}))).unwrap_err();
        assert!(e.contains("bloom_strength") && e.contains("0 to 1.5"), "{e}");
        let e = set_air(&mut p, &obj(json!({"weather": "fog"}))).unwrap_err();
        assert!(e.contains("rain, snow, dust"), "{e}");
        let e = set_air(&mut p, &obj(json!({"weather": ["rain", "snow", "dust", "ash"]}))).unwrap_err();
        assert!(e.contains("at most 3"), "{e}");
        let e = set_air(&mut p, &obj(json!({"tonemap": "hdr"}))).unwrap_err();
        assert!(e.contains("filmic"), "{e}");
        let e = set_air(&mut p, &obj(json!({"fog_colour": "mist"}))).unwrap_err();
        assert!(e.contains("#rrggbb"), "{e}");
        let e = set_air(&mut p, &obj(json!({"weather_2_amount": 0.4}))).unwrap_err();
        assert!(e.contains("weather layer 2"), "{e}");
    }

    #[test]
    fn weather_and_wind_become_keys() {
        let mut p = m::ParamMap::new();
        set_air(&mut p, &obj(json!({"weather": [{"kind": "downpour", "amount": 0.9, "wind": [3, 0, 0], "zone": [0, 0, 0, 10, 5, 10]}, "leaves"]}))).unwrap();
        assert_eq!(p.get("weather_1").and_then(m::Value::as_str), Some("rain"));
        assert_eq!(p.get("weather_1_wind_x").and_then(m::Value::as_f64), Some(3.0));
        assert_eq!(p.get("weather_2").and_then(m::Value::as_str), Some("leaves"));
        assert!(p.get("weather_1_zone").and_then(m::Value::as_str).is_some_and(|z| z.split(' ').count() == 6));
    }

    #[test]
    fn a_lamp_look_is_checked() {
        let mut p = m::ParamMap::new();
        let e = set_light_look(&json!({"glow": 40}), &mut p, m::LightKind::Point).unwrap_err();
        assert!(e.contains("0 to 5"), "{e}");
        let e = set_light_look(&json!({"flame_height": 0.1}), &mut p, m::LightKind::Point).unwrap_err();
        assert!(e.contains("\"flame\": true"), "{e}");
        let e = set_light_look(&json!({"glow": 1}), &mut p, m::LightKind::Ambient).unwrap_err();
        assert!(e.contains("point or spot"), "{e}");
        let e = set_light_look(&json!({"flicker_speed": 5}), &mut p, m::LightKind::Point).unwrap_err();
        assert!(e.contains("flicker"), "{e}");
        let notes = set_light_look(&json!({"flame": true, "flame_height": 0.1, "flicker": 0.3, "beam": 0.5}), &mut p, m::LightKind::Point).unwrap();
        assert!(p.contains_key("flame") && p.contains_key("flicker") && notes.iter().any(|n| n.contains("fog")));
        set_light_look(&json!({"glow": 0, "flame": false}), &mut p, m::LightKind::Point).unwrap();
        assert!(!p.contains_key("glow") && !p.contains_key("flame"));
    }

    #[test]
    fn the_argument_check_lists_every_field() {
        let listed: std::collections::BTreeSet<&str> = super::super::args::op_fields("atmosphere").expect("atmosphere has a field list").iter().copied().collect();
        let known: std::collections::BTreeSet<&str> = atmosphere_fields().into_iter().collect();
        assert_eq!(listed, known);
    }

    fn session() -> Session {
        Session::new(crate::services::project::new_custom("Air", 640, 360, 30.0))
    }

    fn params_of(s: &Session, id: u32) -> m::ParamMap {
        super::super::space::gen_params(&s.model_clip(id).expect("clip"))
    }

    fn newest(s: &Session) -> u32 {
        s.doc.project.clips.iter().map(|c| c.id).max().unwrap()
    }

    #[test]
    fn a_model_takes_wetness_and_a_glow_that_blooms() {
        let mut s = session();
        s.edit(&[json!({"op": "model", "shape": "sphere", "emissive": "#ffaa44", "emissive_strength": 6, "wetness": 0.7})], "ball", false).unwrap();
        let id = newest(&s);
        assert_eq!(params_of(&s, id).get("wetness").and_then(m::Value::as_f64), Some(0.7));
        let (_, mesh) = s.model_of(id).unwrap();
        assert!(mesh.materials[0].emissive[0] > 3.0, "{:?}", mesh.materials[0].emissive);
        // On its own, a stronger glow scales what the model has; wetness alone is a material step.
        s.edit(&[json!({"op": "mesh", "clip": id, "emissive_strength": 2})], "stronger", false).unwrap();
        let (_, mesh) = s.model_of(id).unwrap();
        assert!(mesh.materials[0].emissive[0] > 6.0);
        s.edit(&[json!({"op": "mesh", "clip": id, "wetness": 0.2})], "drier", false).unwrap();
        assert_eq!(params_of(&s, id).get("wetness").and_then(m::Value::as_f64), Some(0.2));
        // The reasons come with the fix.
        let e = s.edit(&[json!({"op": "model", "shape": "cube", "wetness": 3})], "x", false).unwrap_err();
        assert!(e.contains("0 (dry) to 1 (soaked)"), "{e}");
        let e = s.edit(&[json!({"op": "model", "shape": "cube", "emissive_strength": 6})], "x", false).unwrap_err();
        assert!(e.contains("\"emissive\""), "{e}");
        let e = s.edit(&[json!({"op": "model", "shape": "cube", "colour": "#ffffff", "emissive_strength": 500, "emissive": "#fff"})], "x", false).unwrap_err();
        assert!(e.contains("0 (off) to 100"), "{e}");
        let plain = s.edit(&[json!({"op": "model", "shape": "cube", "colour": "#808080"})], "cube", false).unwrap();
        let _ = plain;
        let cube = newest(&s);
        let e = s.edit(&[json!({"op": "mesh", "clip": cube, "emissive_strength": 4})], "x", false).unwrap_err();
        assert!(e.contains("no material here glows"), "{e}");
    }

    #[test]
    fn a_lamp_shows_itself_and_a_scene_gets_air() {
        let mut s = session();
        s.edit(&[json!({"op": "light", "type": "point", "glow": 1.2, "beam": 0.5, "flame": true, "flame_height": 0.1, "flicker": 0.35})], "lamp", false).unwrap();
        let lamp = newest(&s);
        let p = params_of(&s, lamp);
        assert_eq!(p.get("glow").and_then(m::Value::as_f64), Some(1.2));
        assert!(matches!(p.get("flame"), Some(m::Value::Bool(true))) && p.contains_key("flicker"));
        let e = s.edit(&[json!({"op": "light", "clip": lamp, "glow": 40})], "x", false).unwrap_err();
        assert!(e.contains("0 to 5"), "{e}");
        let e = s.edit(&[json!({"op": "light", "type": "ambient", "glow": 1})], "x", false).unwrap_err();
        assert!(e.contains("point or spot"), "{e}");
        // The air: one carrier, merged into by later calls.
        s.edit(&[json!({"op": "atmosphere", "preset": "neon_rain"})], "air", false).unwrap();
        let carrier = s.air_carrier(0).expect("a carrier");
        let p = params_of(&s, carrier);
        assert_eq!(p.get("weather_1").and_then(m::Value::as_str), Some("rain"));
        assert_eq!(m::LightSettings::read(&p).intensity, 0.0);
        s.edit(&[json!({"op": "atmosphere", "fog_density": 0.06, "exposure": -0.5})], "more", false).unwrap();
        assert_eq!(s.air_carrier(0), Some(carrier), "one carrier, not two");
        let p = params_of(&s, carrier);
        assert_eq!(p.get("fog_density").and_then(m::Value::as_f64), Some(0.06));
        assert_eq!(p.get("weather_1").and_then(m::Value::as_str), Some("rain"));
        s.edit(&[json!({"op": "atmosphere", "weather": "snow"})], "snow", false).unwrap();
        let p = params_of(&s, carrier);
        assert_eq!(p.get("weather_1").and_then(m::Value::as_str), Some("snow"));
        let e = s.edit(&[json!({"op": "atmosphere", "preset": "sunny day in space"})], "x", false).unwrap_err();
        assert!(e.contains("neon_rain"), "{e}");
        let e = s.edit(&[json!({"op": "atmosphere"})], "x", false).unwrap_err();
        assert!(e.contains("something to set"), "{e}");
        s.edit(&[json!({"op": "atmosphere", "remove": true})], "off", false).unwrap();
        assert!(s.air_carrier(0).is_none());
    }

    #[test]
    fn a_lighting_set_up_can_flicker_and_bring_its_air() {
        let mut s = session();
        s.edit(&[json!({"op": "model", "shape": "sphere"})], "ball", false).unwrap();
        s.edit(&[json!({"op": "lighting", "preset": "firelight", "atmosphere": true})], "fire", false).unwrap();
        let fire = s.doc.project.clips.iter().find(|c| c.name == "Fire light").map(|c| c.id).expect("the fire light");
        let p = params_of(&s, fire);
        assert!(p.get("flicker").and_then(m::Value::as_f64).is_some_and(|v| v > 0.0) && p.contains_key("glow"), "a fire flickers and glows");
        assert!(s.air_carrier(0).is_some(), "the air that goes with it");
        // A second set-up does not switch the air off.
        s.edit(&[json!({"op": "lighting", "preset": "three_point"})], "three", false).unwrap();
        assert!(s.air_carrier(0).is_some());
    }

    #[test]
    fn the_critic_names_air_and_lamps_with_fixes() {
        let mut s = session();
        s.edit(&[json!({"op": "model", "shape": "sphere", "name": "Ball"})], "ball", false).unwrap();
        s.edit(&[json!({"op": "light", "type": "point"})], "lamp", false).unwrap();
        let lamp = newest(&s);
        s.edit(&[json!({"op": "set", "clip": lamp, "name": "Street lamp"})], "name", false).ok();
        s.brief = Some("A rainy night street".into());
        let notes = s.look_problems(false);
        let air = notes.iter().find(|n| n.kind == "3d_air_missing").expect("rain asked for and missing");
        assert!(air.fix.contains("\"weather\"") && air.fix.contains("rain"), "{}", air.fix);
        s.edit(&[json!({"op": "atmosphere", "preset": "neon_rain"})], "air", false).unwrap();
        assert!(s.look_problems(false).iter().all(|n| n.kind != "3d_air_missing"));
    }

    #[test]
    fn names_resolve() {
        assert_eq!(canonical(WEATHER, "Snowfall"), Some("snow"));
        assert_eq!(canonical(TONEMAPS, "aces"), Some("filmic"));
        assert!(preset("neon").is_some() && preset("no such").is_none());
    }
}
