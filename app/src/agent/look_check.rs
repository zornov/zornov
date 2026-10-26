// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The look critic: what a lighting artist would say about a 3D scene,
//! each note with the exact op that fixes it.
//!
//! From the lights, models and the scene's air (no rendering):
//! * `3d_flat_key`: several lights but none that leads, so the form has no
//!   direction;
//! * `3d_source_no_effect`: a lamp, bulb, candle or neon light that shows
//!   nothing (no glow, beam or flame);
//! * `3d_flat_source`: a flame or bulb drawn flat (a flame that is a plain
//!   light, a bulb whose glow cannot bloom);
//! * `3d_air_missing`: weather or air the request asks for that the scene
//!   does not show.
//!
//! From the rendered frames of the 3D shots (a few sampled, kept in the
//! session's memo):
//! * `3d_blown_highlights` and `3d_over_bloom`: white where there should be
//!   detail;
//! * `3d_crushed_blacks` and `3d_muddy_blacks`: shadows with no detail, or
//!   blacks that are grey;
//! * `3d_no_separation`: the subject does not stand off the background.
//!
//! The measuring functions are plain functions of pixels, so they are tested
//! without a renderer.

use crate::view;
use studio_model as m;

/// A look note: like the 3D critic's, with a fix written for this scene.
pub struct LookProblem {
    pub kind: &'static str,
    pub at: f64,
    pub clip: u32,
    pub problem: String,
    pub fix: String,
}

fn secs(t: i64) -> f64 {
    t as f64 / view::SEC as f64
}

// --------------------------------------------------------------- the scene

pub struct Lamp {
    pub id: u32,
    pub name: String,
    pub kind: m::LightKind,
    pub intensity: f64,
    pub shows: bool,
    pub start: f64,
}

pub struct Carrier {
    pub id: u32,
    pub fog: f64,
    /// Kind and amount of each weather layer.
    pub weather: Vec<(String, f64)>,
    pub bloom: f64,
    pub threshold: f64,
    pub exposure: f64,
    pub tonemap: String,
}

pub struct Glower {
    pub id: u32,
    pub name: String,
    /// Brightest glow channel over the model's materials, with the clip's
    /// `emissive_strength` (and `emissive` colour) applied.
    pub glow: f32,
    pub has_emissive: bool,
    pub start: f64,
    pub end: f64,
}

#[derive(Default)]
pub struct LookScene {
    pub lamps: Vec<Lamp>,
    pub carrier: Option<Carrier>,
    pub models: Vec<Glower>,
    /// Model clips that stay a while: (id, start, end).
    pub heroes: Vec<(u32, f64, f64)>,
}

fn num(g: &m::Generator, k: &str) -> Option<f64> {
    g.params.get(k).and_then(m::Value::as_f64)
}

/// Reads the lights, the carrier and the glowing models of the active sequence.
pub fn scene(model: &m::Project) -> LookScene {
    let mut s = LookScene::default();
    let Some(seq) = model.sequence(model.active_sequence) else { return s };
    for c in seq.clips.iter().filter(|c| c.enabled) {
        let m::ClipSource::Generator(g) = &c.source else { continue };
        let (start, end) = (secs(c.start), secs(c.start + c.duration));
        match g.kind {
            m::GeneratorKind::Light => {
                let set = m::LightSettings::read(&g.params);
                if matches!(g.params.get("atmosphere"), Some(m::Value::Bool(true))) {
                    if s.carrier.is_none() {
                        let weather = (1..=3)
                            .filter_map(|i| {
                                let k = g.params.get(&format!("weather_{i}")).and_then(m::Value::as_str)?;
                                Some((k.to_owned(), num(g, &format!("weather_{i}_amount")).unwrap_or(0.6)))
                            })
                            .collect();
                        s.carrier = Some(Carrier {
                            id: c.id.0 as u32,
                            fog: num(g, "fog_density").unwrap_or(0.0),
                            weather,
                            bloom: num(g, "bloom_strength").unwrap_or(0.0),
                            threshold: num(g, "bloom_threshold").unwrap_or(0.9),
                            exposure: num(g, "exposure").unwrap_or(0.0),
                            tonemap: g.params.get("tonemap").and_then(m::Value::as_str).unwrap_or("none").to_owned(),
                        });
                    }
                    if set.intensity <= 0.0 {
                        continue;
                    }
                }
                let shows = ["glow", "flare", "beam"].iter().any(|k| num(g, k).is_some_and(|v| v > 0.0)) || matches!(g.params.get("flame"), Some(m::Value::Bool(true)));
                s.lamps.push(Lamp { id: c.id.0 as u32, name: c.name.clone(), kind: set.kind, intensity: set.intensity, shows, start });
            }
            m::GeneratorKind::Model => {
                if end - start >= 1.5 {
                    s.heroes.push((c.id.0 as u32, start, end));
                }
                let (mesh_glow, has) = match crate::mesh_store::file_of(c).and_then(|f| super::three_d_check::info_of_file(&f)) {
                    Some(i) => (i.glow, i.craft.emissive_materials > 0),
                    None => (0.0, false),
                };
                let own = g.params.get("emissive").and_then(m::Value::as_colour).map(|c| c.r.max(c.g).max(c.b) as f32);
                let k = num(g, "emissive_strength").unwrap_or(1.0) as f32;
                let base = own.unwrap_or(mesh_glow);
                s.models.push(Glower { id: c.id.0 as u32, name: c.name.clone(), glow: base * k, has_emissive: has || own.is_some(), start, end });
            }
            _ => {}
        }
    }
    s
}

// ------------------------------------------------------------------- words

fn words(text: &str) -> Vec<String> {
    text.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_owned).collect()
}

/// Weather and air a request asks for: (the kind as the `weather` field
/// writes it, or "fog").
pub fn asked_air(text: &str) -> Vec<&'static str> {
    let w = words(text);
    let has = |list: &[&str]| w.iter().any(|x| list.contains(&x.as_str()));
    let pair = |a: &[&str], b: &[&str]| w.windows(2).any(|p| a.contains(&p[0].as_str()) && b.contains(&p[1].as_str()));
    let mut out = Vec::new();
    if has(&["rain", "rainy", "raining", "rainfall", "drizzle", "downpour", "rainstorm", "rainstorms", "showers"]) {
        out.push("rain");
    }
    if has(&["snow", "snowy", "snowing", "snowfall", "blizzard", "flurries"]) {
        out.push("snow");
    }
    if has(&["fog", "foggy", "mist", "misty", "haze", "hazy", "smog"]) {
        out.push("fog");
    }
    if has(&["dust", "dusty", "motes", "pollen"]) {
        out.push("dust");
    }
    if has(&["smoke", "smoky", "steam", "steamy"]) {
        out.push("smoke");
    }
    if has(&["ember", "embers", "cinders", "fireflies"]) {
        out.push("embers");
    }
    if has(&["sparks", "welding"]) {
        out.push("sparks");
    }
    if pair(&["falling", "autumn", "blowing", "drifting", "swirling"], &["leaves", "petals"]) {
        out.push("leaves");
    }
    if pair(&["falling", "volcanic", "drifting"], &["ash", "ashes"]) {
        out.push("ash");
    }
    out
}

fn name_has(name: &str, list: &[&str]) -> bool {
    words(name).iter().any(|w| list.contains(&w.as_str()))
}

const FLAME_WORDS: &[&str] = &["flame", "flames", "candle", "candles", "fire", "campfire", "bonfire", "torch", "fireplace", "match", "lighter"];
const LAMP_WORDS: &[&str] = &["lamp", "lamps", "bulb", "bulbs", "lantern", "lanterns", "neon", "streetlight", "streetlamp", "sconce", "chandelier", "tube", "headlight", "headlights", "fairy", "led", "lightbulb", "sign", "window", "windows"];

fn flame_height_for(name: &str) -> f64 {
    let w = words(name);
    let has = |k: &str| w.iter().any(|x| x == k);
    if has("bonfire") {
        2.5
    } else if has("campfire") || has("fireplace") || has("fire") {
        0.6
    } else if has("torch") {
        0.3
    } else if has("candle") || has("candles") || has("match") || has("lighter") {
        0.05
    } else {
        0.15
    }
}

// ----------------------------------------------------- judging the scene

/// Notes found without rendering.
pub fn judge_scene(s: &LookScene, text: &str) -> Vec<LookProblem> {
    let mut out = Vec::new();
    let lamps: Vec<&Lamp> = s.lamps.iter().filter(|l| l.kind != m::LightKind::Ambient).collect();

    // A lamp that shows nothing.
    for l in &lamps {
        if l.shows || l.kind == m::LightKind::Parallel {
            continue;
        }
        if name_has(&l.name, FLAME_WORDS) {
            let h = flame_height_for(&l.name);
            out.push(LookProblem {
                kind: "3d_flat_source",
                at: l.start,
                clip: l.id,
                problem: format!("light \"{}\" (clip {}) stands for a flame but is a plain light: it lights the room and draws nothing, so there is no flame to see and it never flickers", l.name, l.id),
                fix: format!("{{\"op\": \"light\", \"clip\": {}, \"flame\": true, \"flame_height\": {h}, \"flicker\": 0.35, \"glow\": 0.6}}", l.id),
            });
        } else if name_has(&l.name, LAMP_WORDS) {
            let air = s.carrier.as_ref().is_some_and(|c| c.fog > 0.0 || !c.weather.is_empty());
            out.push(LookProblem {
                kind: "3d_source_no_effect",
                at: l.start,
                clip: l.id,
                problem: format!("light \"{}\" (clip {}) has no visible effect: no glow, no streak, no beam and no flame, so the viewer sees a lit room with no source{}", l.name, l.id, if air { " and the air in the scene has no shaft to show where it points" } else { "" }),
                fix: format!("{{\"op\": \"light\", \"clip\": {}, \"glow\": 1, \"beam\": 0.6}}{}", l.id, if air { "" } else { " and give the scene air so the beam shows: {\"op\": \"atmosphere\", \"fog_density\": 0.03, \"ground\": \"auto\"}" }),
            });
        }
    }
    // A bulb or window model whose glow cannot bloom.
    for g in &s.models {
        // The thing that gives the light: a bulb, a tube, a window, a flame; not the post, stand or wax it sits on.
        let holder = name_has(&g.name, &["post", "pole", "stand", "base", "holder", "stick", "wax", "body", "shade", "housing", "frame", "bracket", "arm", "cable", "wire"]);
        if g.end - g.start < 1.0 || holder || !(name_has(&g.name, LAMP_WORDS) || name_has(&g.name, &["flame", "flames"])) {
            continue;
        }
        if g.glow <= 1.05 {
            let fix = if g.has_emissive {
                format!("{{\"op\": \"mesh\", \"clip\": {}, \"do\": \"material\", \"emissive_strength\": 6}}", g.id)
            } else {
                format!("{{\"op\": \"mesh\", \"clip\": {}, \"do\": \"material\", \"emissive\": \"#ffd9a0\", \"emissive_strength\": 6}}", g.id)
            };
            out.push(LookProblem {
                kind: "3d_flat_source",
                at: g.start,
                clip: g.id,
                problem: format!("\"{}\" (clip {}) is drawn flat: its glow is {:.1} (white at most), so it reads as a pale surface, not a light, and it cannot bloom or light the air round it", g.name, g.id, g.glow),
                fix,
            });
        }
    }
    // Several lights, none that leads.
    let heroes_on = !s.heroes.is_empty();
    if heroes_on && lamps.len() >= 2 {
        let mut by: Vec<&&Lamp> = lamps.iter().collect();
        by.sort_by(|a, b| b.intensity.total_cmp(&a.intensity));
        let ambient: f64 = s.lamps.iter().filter(|l| l.kind == m::LightKind::Ambient).map(|l| l.intensity).sum();
        let (k, second) = (by[0], by[1]);
        if k.intensity > 0.0 && (k.intensity < 1.25 * second.intensity || ambient >= 0.7 * k.intensity) {
            let why = if k.intensity < 1.25 * second.intensity { format!("the strongest light ({}%) is hardly stronger than the next ({}%)", (k.intensity * 100.0).round(), (second.intensity * 100.0).round()) } else { format!("the ambient light ({}%) fills in most of what the key ({}%) lights", (ambient * 100.0).round(), (k.intensity * 100.0).round()) };
            out.push(LookProblem {
                kind: "3d_flat_key",
                at: k.start,
                clip: k.id,
                problem: format!("the lighting has no key direction: {why}, so shadows have no one side and the form looks flat"),
                fix: format!("make \"{}\" (clip {}) the key: {{\"op\": \"light\", \"clip\": {}, \"intensity\": {}, \"shadows\": true}}, and pull the others down: {{\"op\": \"light\", \"clip\": {}, \"intensity\": {}}}{}", k.name, k.id, k.id, (k.intensity * 100.0 * 1.4).round().max(80.0), second.id, (second.intensity * 100.0 * 0.5).round(), if ambient >= 0.7 * k.intensity { format!(", and the ambient to 15%: {{\"op\": \"set\", \"clip\": {}, \"intensity\": 15}}", s.lamps.iter().find(|l| l.kind == m::LightKind::Ambient).map_or(0, |l| l.id)) } else { String::new() }),
            });
        }
    }
    // Weather or air the request asks for.
    if heroes_on || !s.models.is_empty() {
        for want in asked_air(text) {
            let c = s.carrier.as_ref();
            let (shown, faint) = match want {
                "fog" => {
                    let d = c.map_or(0.0, |c| c.fog);
                    (d >= 0.008, d > 0.0 && d < 0.008)
                }
                k => {
                    let a = c.and_then(|c| c.weather.iter().find(|(w, _)| w == k).map(|w| w.1));
                    (a.is_some_and(|a| a >= 0.15), a.is_some_and(|a| a < 0.15))
                }
            };
            if shown {
                continue;
            }
            let (ctl, fix) = if want == "fog" {
                (None, "\"fog_density\": 0.03, \"fog_falloff\": 0.1")
            } else {
                (Some(want), "")
            };
            let what = if want == "fog" { "fog, mist or haze".to_owned() } else { want.to_owned() };
            let fix = match (c, ctl) {
                (Some(c), None) => format!("{{\"op\": \"atmosphere\", \"clip\": {}, {fix}, \"ground\": \"auto\"}}", c.id),
                (None, None) => format!("{{\"op\": \"atmosphere\", {fix}, \"ground\": \"auto\"}}"),
                (Some(c), Some(k)) => format!("{{\"op\": \"atmosphere\", \"clip\": {}, \"weather\": [{{\"kind\": \"{k}\", \"amount\": 0.7}}]}} (this replaces the weather layers; list every layer you want)", c.id),
                (None, Some(k)) => format!("{{\"op\": \"atmosphere\", \"weather\": [{{\"kind\": \"{k}\", \"amount\": 0.7}}], \"fog_density\": 0.02, \"ground\": \"auto\"}}"),
            };
            out.push(LookProblem {
                kind: "3d_air_missing",
                at: 0.0,
                clip: c.map_or(0, |c| c.id),
                problem: if faint { format!("the request asks for {what}, and the scene has it so faintly that it cannot be seen (the carrier's setting is below what shows)") } else { format!("the request asks for {what}, but nothing in the 3D scene makes any: no air and look carries it") },
                fix,
            });
        }
    }
    out
}

// --------------------------------------------------------- measuring frames

/// What a picture's tones are like (sRGB coded, 0..1).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Tones {
    /// Share of pixels with every channel at least 0.96.
    pub white: f32,
    /// Share of pixels with any channel at 0.99 or more.
    pub clipped: f32,
    /// Share of pixels with every channel at most 0.02.
    pub black: f32,
    /// Share of pixels brighter than 0.85 in luma.
    pub bright: f32,
    pub p2: f32,
    pub p50: f32,
    pub p98: f32,
}

fn luma(p: &[u8]) -> f32 {
    (0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32) / 255.0
}

/// Measures a picture (RGBA8).
pub fn tones(rgba: &[u8]) -> Tones {
    let n = rgba.len() / 4;
    if n == 0 {
        return Tones::default();
    }
    let mut hist = [0u32; 256];
    let (mut white, mut clipped, mut black, mut bright) = (0usize, 0usize, 0usize, 0usize);
    for p in rgba.chunks_exact(4) {
        let (mn, mx) = (p[0].min(p[1]).min(p[2]), p[0].max(p[1]).max(p[2]));
        white += (mn as f32 >= 0.96 * 255.0) as usize;
        clipped += (mx as f32 >= 0.99 * 255.0) as usize;
        black += (mx as f32 <= 0.02 * 255.0) as usize;
        let l = luma(p);
        bright += (l > 0.85) as usize;
        hist[(l * 255.0).round().clamp(0.0, 255.0) as usize] += 1;
    }
    let at = |q: f32| {
        let want = (q * n as f32) as u32;
        let mut acc = 0;
        for (i, c) in hist.iter().enumerate() {
            acc += c;
            if acc > want {
                return i as f32 / 255.0;
            }
        }
        1.0
    };
    let k = 1.0 / n as f32;
    Tones { white: white as f32 * k, clipped: clipped as f32 * k, black: black as f32 * k, bright: bright as f32 * k, p2: at(0.02), p50: at(0.5), p98: at(0.98) }
}

/// How far a subject stands off what is behind it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Separation {
    /// Share of the frame the subject covers.
    pub area: f32,
    pub lum_in: f32,
    pub lum_ring: f32,
    /// Distance between the mean colours inside and in a ring round it.
    pub colour: f32,
    /// Mean contrast across the outline.
    pub edge: f32,
}

impl Separation {
    pub fn separated(&self) -> bool {
        (self.lum_in - self.lum_ring).abs() >= 0.08 || self.colour >= 0.14 || self.edge >= 0.07
    }
}

/// The subject is what changes when it is taken out of the picture: `with`
/// and `without` are the same frame (RGBA8, w × h) with and without it.
pub fn separation(with: &[u8], without: &[u8], w: usize, h: usize) -> Option<Separation> {
    if with.len() != without.len() || with.len() < w * h * 4 || w < 16 || h < 16 {
        return None;
    }
    let n = w * h;
    let mask: Vec<bool> = (0..n)
        .map(|i| {
            let d: i32 = (0..3).map(|k| (with[i * 4 + k] as i32 - without[i * 4 + k] as i32).abs()).sum();
            d as f32 > 0.10 * 255.0
        })
        .collect();
    let area = mask.iter().filter(|m| **m).count() as f32 / n as f32;
    if !(0.004..=0.7).contains(&area) {
        return None;
    }
    // A ring of pixels round the subject: dilate by r.
    let r = (w / 40).max(3) as i64;
    let mut wide = vec![false; n];
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            if !mask[y as usize * w + x as usize] {
                continue;
            }
            for dy in -r..=r {
                for dx in -r..=r {
                    let (xx, yy) = (x + dx, y + dy);
                    if xx >= 0 && yy >= 0 && xx < w as i64 && yy < h as i64 && dx * dx + dy * dy <= r * r {
                        wide[yy as usize * w + xx as usize] = true;
                    }
                }
            }
        }
    }
    let (mut cin, mut cring) = ([0.0f32; 3], [0.0f32; 3]);
    let (mut nin, mut nring) = (0usize, 0usize);
    for i in 0..n {
        let c = [with[i * 4] as f32 / 255.0, with[i * 4 + 1] as f32 / 255.0, with[i * 4 + 2] as f32 / 255.0];
        // What is behind: the frame without the subject.
        let b = [without[i * 4] as f32 / 255.0, without[i * 4 + 1] as f32 / 255.0, without[i * 4 + 2] as f32 / 255.0];
        if mask[i] {
            nin += 1;
            for k in 0..3 {
                cin[k] += c[k];
            }
        } else if wide[i] {
            nring += 1;
            for k in 0..3 {
                cring[k] += b[k];
            }
        }
    }
    if nin == 0 || nring < 8 {
        return None;
    }
    let cin = cin.map(|v| v / nin as f32);
    let cring = cring.map(|v| v / nring as f32);
    let lum = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    let colour = (0..3).map(|k| (cin[k] - cring[k]).powi(2)).sum::<f32>().sqrt();
    // Contrast across the outline: a subject pixel against its background
    // neighbour in the frame without it.
    let (mut edge, mut ne) = (0.0f32, 0usize);
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let i = y * w + x;
            if !mask[i] {
                continue;
            }
            for j in [i - 1, i + 1, i - w, i + w] {
                if !mask[j] {
                    edge += (luma(&with[i * 4..i * 4 + 4]) - luma(&without[j * 4..j * 4 + 4])).abs();
                    ne += 1;
                    break;
                }
            }
        }
    }
    Some(Separation { area, lum_in: lum(cin), lum_ring: lum(cring), colour, edge: if ne > 0 { edge / ne as f32 } else { 0.0 } })
}

/// Notes on one frame's tones. `bloom` and `threshold` are the scene's.
pub fn judge_tones(t: &Tones, carrier: Option<&Carrier>, at: f64) -> Vec<LookProblem> {
    let mut out = Vec::new();
    let carrier_id = carrier.map_or(0, |c| c.id);
    let exposure = carrier.map_or(0.0, |c| c.exposure);
    let lift = |stops: f64| {
        let e = ((exposure - stops) * 10.0).round() / 10.0;
        match carrier {
            Some(c) if c.tonemap != "filmic" => format!("{{\"op\": \"atmosphere\", \"clip\": {}, \"tonemap\": \"filmic\", \"exposure\": {e}}}", c.id),
            Some(c) => format!("{{\"op\": \"atmosphere\", \"clip\": {}, \"auto_exposure\": true, \"exposure\": {e}}}", c.id),
            None => format!("{{\"op\": \"atmosphere\", \"preset\": \"clean_film\", \"exposure\": {}}}", -stops),
        }
    };
    let over_bloom = carrier.is_some_and(|c| c.bloom >= 0.6 || (c.bloom >= 0.25 && t.bright >= 0.12));
    if t.white >= 0.05 || t.clipped >= 0.12 {
        let stops = if t.white >= 0.15 || t.clipped >= 0.3 { 1.5 } else if t.white >= 0.08 || t.clipped >= 0.2 { 1.0 } else { 0.6 };
        out.push(LookProblem {
            kind: "3d_blown_highlights",
            at,
            clip: carrier_id,
            problem: format!("blown highlights: {:.0}% of the frame is pure white and {:.0}% has a channel at maximum, so there is no detail or colour in the brightest parts", t.white * 100.0, t.clipped * 100.0),
            fix: format!("{} (that is {stops} stops down), and if a lamp or sky is the cause, lower its `intensity` or `emissive_strength`", lift(stops)),
        });
    }
    if over_bloom {
        let c = carrier;
        out.push(LookProblem {
            kind: "3d_over_bloom",
            at,
            clip: carrier_id,
            problem: format!("over-bloom: bloom strength {} with {:.0}% of the frame brighter than 85%, so glare spreads over the picture and washes out the shadows round every light", c.map_or(0.0, |c| c.bloom), t.bright * 100.0),
            fix: format!("{{\"op\": \"atmosphere\", \"clip\": {carrier_id}, \"bloom_strength\": 0.18, \"bloom_threshold\": {}, \"bloom_radius\": 0.4}}", c.map_or(1.0, |c| c.threshold.max(1.0))),
        });
    }
    if t.black >= 0.35 {
        out.push(LookProblem {
            kind: "3d_crushed_blacks",
            at,
            clip: carrier_id,
            problem: format!("crushed blacks: {:.0}% of the frame is pure black with no detail, so shapes in the shadows are lost", t.black * 100.0),
            fix: match carrier {
                Some(c) => format!("{{\"op\": \"atmosphere\", \"clip\": {}, \"exposure\": {}, \"richness\": 0.4, \"shadow_tint\": \"#1a2a4a\", \"shadow_tint_amount\": 0.5}} and a soft fill or ambient light at 15%: {{\"op\": \"light\", \"type\": \"ambient\", \"colour\": \"#8090b0\", \"intensity\": 15}}", c.id, ((exposure + 0.7) * 10.0).round() / 10.0),
                None => "{\"op\": \"light\", \"type\": \"ambient\", \"colour\": \"#8090b0\", \"intensity\": 15} for a fill, then {\"op\": \"atmosphere\", \"preset\": \"clean_film\", \"exposure\": 0.5}".to_owned(),
            },
        });
    } else if t.p2 >= 0.12 && t.p98 - t.p2 <= 0.6 && (0.2..=0.75).contains(&t.p50) {
        out.push(LookProblem {
            kind: "3d_muddy_blacks",
            at,
            clip: carrier_id,
            problem: format!("muddy blacks: the darkest parts of the frame are grey ({:.0}% bright) and the whole picture spans only {:.0}% of the tones, so it looks hazy and low in contrast", t.p2 * 100.0, (t.p98 - t.p2) * 100.0),
            fix: match carrier {
                Some(c) => format!("{{\"op\": \"atmosphere\", \"clip\": {}, \"richness\": 0.5, \"contrast\": 1.2, \"exposure\": {}}}; if thick fog causes it, lower fog_density by half", c.id, ((exposure - 0.3) * 10.0).round() / 10.0),
                None => "{\"op\": \"atmosphere\", \"preset\": \"clean_film\", \"richness\": 0.5, \"contrast\": 1.2}".to_owned(),
            },
        });
    }
    out
}

/// A note for a subject that does not stand off its background.
pub fn judge_separation(s: &Separation, clip: u32, name: &str, at: f64) -> Option<LookProblem> {
    if s.separated() {
        return None;
    }
    let side = if s.lum_in >= s.lum_ring { "as bright as" } else { "as dark as" };
    Some(LookProblem {
        kind: "3d_no_separation",
        at,
        clip,
        problem: format!("\"{name}\" (clip {clip}) does not stand off the background: it is {side} what is behind it (brightness {:.0}% against {:.0}%, colour distance {:.2}, edge contrast {:.2}), so its outline is lost", s.lum_in * 100.0, s.lum_ring * 100.0, s.colour, s.edge),
        fix: format!("light its edge: {{\"op\": \"lighting\", \"preset\": \"rim\", \"on\": {clip}, \"replace\": false}}; or make the background clearly lighter or darker than the subject (30 percent or more), or fog behind it: {{\"op\": \"atmosphere\", \"fog_density\": 0.03, \"ground\": \"auto\"}}"),
    })
}

impl super::Session {
    /// Every look note for this film: from the scene, then from a few
    /// rendered frames of the 3D shots.
    pub(super) fn look_problems(&mut self, frames: bool) -> Vec<LookProblem> {
        let sc = scene(&self.doc.model);
        let text = format!("{}\n{}", self.brief.clone().unwrap_or_default(), self.notes_text().unwrap_or_default());
        let mut out = judge_scene(&sc, &text);
        if frames {
            out.extend(self.look_frames(&sc));
        }
        let aside = super::three_d_check::accepted(&text);
        out.retain(|p| !aside.iter().any(|k| k == p.kind));
        out.sort_by(|a, b| a.at.total_cmp(&b.at));
        out
    }

    #[cfg(not(feature = "render"))]
    fn look_frames(&mut self, _: &LookScene) -> Vec<LookProblem> {
        Vec::new()
    }

    /// Up to three frames across the 3D shots: tones, and how the first
    /// model on screen stands off its background.
    #[cfg(feature = "render")]
    fn look_frames(&mut self, sc: &LookScene) -> Vec<LookProblem> {
        use serde_json::json;
        let mut out: Vec<LookProblem> = Vec::new();
        if sc.heroes.is_empty() || std::env::var_os("STUDIO_NO_LOOK_FRAMES").is_some() {
            return out;
        }
        let first = sc.heroes.iter().map(|h| h.1).fold(f64::INFINITY, f64::min);
        let last = sc.heroes.iter().map(|h| h.2).fold(0.0, f64::max);
        let mut seen: Vec<&'static str> = Vec::new();
        for k in 1..=3 {
            let t = first + (last - first) * (k as f64 / 4.0);
            let Some(hero) = sc.heroes.iter().find(|h| h.1 <= t && t < h.2) else { continue };
            let tm = (t * view::SEC as f64) as view::Time;
            let key = self.scene_key_pub(tm, tm, "look320");
            let cached = self.memo_pub().get(&key).and_then(|v| v.as_array().map(|a| a.iter().filter_map(|x| x.as_f64()).collect::<Vec<f64>>()));
            let tn = match cached.filter(|v| v.len() == 7) {
                Some(v) => Tones { white: v[0] as f32, clipped: v[1] as f32, black: v[2] as f32, bright: v[3] as f32, p2: v[4] as f32, p50: v[5] as f32, p98: v[6] as f32 },
                None => {
                    let Ok(img) = self.frames.rgba(&self.doc, tm, 320) else { continue };
                    let t = tones(&img.pixels);
                    self.memo_pub().insert(key, json!([t.white, t.clipped, t.black, t.bright, t.p2, t.p50, t.p98]));
                    t
                }
            };
            for p in judge_tones(&tn, sc.carrier.as_ref(), t) {
                if !seen.contains(&p.kind) {
                    seen.push(p.kind);
                    out.push(p);
                }
            }
            if seen.contains(&"3d_no_separation") {
                continue;
            }
            let skey = self.scene_key_pub(tm, tm, &format!("look320-sep{}", hero.0));
            let cached = self.memo_pub().get(&skey).and_then(|v| v.as_array().map(|a| a.iter().filter_map(|x| x.as_f64()).collect::<Vec<f64>>()));
            let sep = match cached {
                Some(v) if v.len() == 5 => Some(Separation { area: v[0] as f32, lum_in: v[1] as f32, lum_ring: v[2] as f32, colour: v[3] as f32, edge: v[4] as f32 }),
                Some(_) => None,
                None => {
                    let (Ok(full), Ok(without)) = (self.frames.rgba(&self.doc, tm, 320), self.frames.rgba_without(&self.doc, &[hero.0], tm, 320)) else { continue };
                    let s = separation(&full.pixels, &without.pixels, full.width as usize, full.height as usize);
                    self.memo_pub().insert(skey, s.map_or(json!([]), |s| json!([s.area, s.lum_in, s.lum_ring, s.colour, s.edge])));
                    s
                }
            };
            if let Some(s) = sep {
                let name = self.doc.model.sequence(self.doc.seq()).and_then(|q| q.clips.iter().find(|c| c.id.0 as u32 == hero.0)).map(|c| c.name.clone()).unwrap_or_default();
                if let Some(p) = judge_separation(&s, hero.0, &name, t) {
                    seen.push(p.kind);
                    out.push(p);
                }
            }
        }
        self.memo_save_pub();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(w: usize, h: usize, rgb: [u8; 3]) -> Vec<u8> {
        (0..w * h).flat_map(|_| [rgb[0], rgb[1], rgb[2], 255]).collect()
    }

    fn lamp(id: u32, name: &str, kind: m::LightKind, intensity: f64, shows: bool) -> Lamp {
        Lamp { id, name: name.into(), kind, intensity, shows, start: 0.0 }
    }

    #[test]
    fn air_is_found_in_the_request() {
        assert_eq!(asked_air("a rainy night, mist over the road, falling leaves"), vec!["rain", "fog", "leaves"]);
        assert!(asked_air("a train brain storm of leaves").is_empty());
        assert_eq!(asked_air("campfire with drifting ash and embers"), vec!["embers", "ash"]);
    }

    #[test]
    fn white_and_black_are_measured() {
        let white = tones(&flat(32, 32, [255, 255, 255]));
        assert!(white.white > 0.99 && white.clipped > 0.99);
        let black = tones(&flat(32, 32, [0, 0, 0]));
        assert!(black.black > 0.99);
        let grey = tones(&flat(32, 32, [120, 120, 120]));
        assert!(grey.white == 0.0 && grey.black == 0.0 && (grey.p50 - 0.47).abs() < 0.02);
    }

    #[test]
    fn tones_become_notes_with_fixes() {
        let carrier = Carrier { id: 9, fog: 0.02, weather: vec![], bloom: 0.5, threshold: 0.9, exposure: 0.0, tonemap: "none".into() };
        let blown = Tones { white: 0.2, clipped: 0.3, bright: 0.3, ..Default::default() };
        let notes = judge_tones(&blown, Some(&carrier), 1.0);
        let kinds: Vec<&str> = notes.iter().map(|n| n.kind).collect();
        assert!(kinds.contains(&"3d_blown_highlights") && kinds.contains(&"3d_over_bloom"), "{kinds:?}");
        let b = notes.iter().find(|n| n.kind == "3d_blown_highlights").unwrap();
        assert!(b.fix.contains("\"clip\": 9") && b.fix.contains("filmic") && b.fix.contains("-1.5"), "{}", b.fix);
        let crushed = judge_tones(&Tones { black: 0.6, ..Default::default() }, Some(&carrier), 0.0);
        assert!(crushed.iter().any(|n| n.kind == "3d_crushed_blacks" && n.fix.contains("ambient")));
        let muddy = judge_tones(&Tones { p2: 0.2, p50: 0.45, p98: 0.7, ..Default::default() }, None, 0.0);
        assert!(muddy.iter().any(|n| n.kind == "3d_muddy_blacks" && n.fix.contains("richness")));
        let fine = Tones { white: 0.01, clipped: 0.02, black: 0.1, bright: 0.05, p2: 0.02, p50: 0.35, p98: 0.85 };
        assert!(judge_tones(&fine, Some(&carrier), 0.0).is_empty());
    }

    #[test]
    fn a_subject_that_matches_its_background_is_not_separated() {
        let (w, h) = (64, 48);
        let mut without = flat(w, h, [90, 90, 90]);
        let mut with = without.clone();
        let put = |img: &mut Vec<u8>, c: [u8; 3]| {
            for y in 14..34 {
                for x in 22..42 {
                    let i = (y * w + x) * 4;
                    img[i..i + 3].copy_from_slice(&c);
                }
            }
        };
        put(&mut with, [100, 98, 96]);
        // The mask needs a visible change: 10% of 255 over three channels.
        let s = separation(&with, &without, w, h);
        assert!(s.is_none() || !s.unwrap().separated());
        let mut with2 = without.clone();
        put(&mut with2, [140, 130, 120]);
        let s = separation(&with2, &without, w, h).unwrap();
        assert!(s.separated(), "{s:?}");
        // The subject hides something dark, and is the colour of the wall round it.
        let mut wall = flat(w, h, [118, 117, 116]);
        let mut with3 = wall.clone();
        put(&mut wall, [60, 60, 60]);
        put(&mut with3, [118, 117, 116]);
        let without = wall;
        let s = separation(&with3, &without, w, h).unwrap();
        assert!(!s.separated(), "{s:?}");
        let note = judge_separation(&s, 4, "Vase", 2.0).unwrap();
        assert!(note.fix.contains("\"preset\": \"rim\"") && note.fix.contains("\"on\": 4"));
    }

    #[test]
    fn lamps_and_flames_that_show_nothing_are_named() {
        let mut s = LookScene::default();
        s.lamps.push(lamp(3, "Street lamp", m::LightKind::Point, 1.0, false));
        s.lamps.push(lamp(4, "Candle", m::LightKind::Point, 0.8, false));
        s.lamps.push(lamp(5, "Fill light", m::LightKind::Point, 0.4, false));
        s.heroes.push((1, 0.0, 10.0));
        let notes = judge_scene(&s, "");
        assert!(notes.iter().any(|n| n.kind == "3d_source_no_effect" && n.clip == 3 && n.fix.contains("\"glow\"")));
        assert!(notes.iter().any(|n| n.kind == "3d_flat_source" && n.clip == 4 && n.fix.contains("\"flame\": true") && n.fix.contains("0.05")));
        assert!(!notes.iter().any(|n| n.clip == 5 && n.kind != "3d_flat_key"));
    }

    #[test]
    fn a_bulb_that_cannot_bloom_is_flat() {
        let mut s = LookScene::default();
        s.models.push(Glower { id: 9, name: "Lamp post".into(), glow: 0.0, has_emissive: false, start: 0.0, end: 5.0 });
        s.models.push(Glower { id: 8, name: "Bulb".into(), glow: 1.0, has_emissive: true, start: 0.0, end: 5.0 });
        let n = judge_scene(&s, "");
        assert!(n.iter().any(|n| n.kind == "3d_flat_source" && n.fix.contains("\"emissive_strength\": 6") && !n.fix.contains("\"emissive\":")));
        assert!(n.iter().all(|n| n.clip != 9), "a lamp post is not the light");
        s.models[1].glow = 6.0;
        assert!(judge_scene(&s, "").is_empty());
    }

    #[test]
    fn lights_with_no_key_are_named() {
        let mut s = LookScene::default();
        s.heroes.push((1, 0.0, 10.0));
        s.lamps.push(lamp(2, "Key light", m::LightKind::Spot, 1.0, true));
        s.lamps.push(lamp(3, "Fill light", m::LightKind::Point, 0.95, true));
        let n = judge_scene(&s, "");
        let k = n.iter().find(|n| n.kind == "3d_flat_key").expect("flat key");
        assert!(k.fix.contains("\"clip\": 2") && k.fix.contains("\"clip\": 3"), "{}", k.fix);
        s.lamps[0].intensity = 1.6;
        assert!(judge_scene(&s, "").iter().all(|n| n.kind != "3d_flat_key"));
    }

    #[test]
    fn requested_air_that_is_not_there_is_named() {
        let mut s = LookScene::default();
        s.heroes.push((1, 0.0, 10.0));
        let n = judge_scene(&s, "A rainy street at night in fog");
        assert!(n.iter().filter(|n| n.kind == "3d_air_missing").count() == 2);
        s.carrier = Some(Carrier { id: 7, fog: 0.03, weather: vec![("rain".into(), 0.05)], bloom: 0.0, threshold: 0.9, exposure: 0.0, tonemap: "filmic".into() });
        let n = judge_scene(&s, "A rainy street at night in fog");
        assert_eq!(n.len(), 1);
        assert!(n[0].problem.contains("so faintly") && n[0].fix.contains("\"clip\": 7") && n[0].fix.contains("rain"));
    }
}
