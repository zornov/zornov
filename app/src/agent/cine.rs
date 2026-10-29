// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The virtual camera's lenses and rigs, for agents: what the `camera` op
//! adds to the 3D camera of `space`.
//!
//! * **Lens kits and sensors.** A focal length means what it does on a
//!   real camera: the sensor's width and an anamorphic squeeze set the
//!   field of view. A kit (spherical primes, vintage, anamorphic 2× or
//!   1.5×, cine zoom, macro, ultra wide) brings its character: barrel
//!   distortion and colour fringes (a "Lens" adjustment layer over the
//!   shot), focus breathing (the picture grows a little as focus comes
//!   close), and the shape of its out-of-focus highlights (bokeh: round
//!   from the renderer's depth of field, or hexagon, octagon or the
//!   anamorphic oval baked as a lens blur on each layer by its depth).
//! * **Focus pulls.** `rack_focus` moves focus to a subject (or a point or
//!   a distance) over a time; `follow_focus` keeps a moving subject sharp.
//! * **Rigs.** `dolly_path` (a track through points), `crane` / `jib` (an
//!   arm that swings up or down round a pivot), `steadicam` (a gliding
//!   walk), `handheld` with a shake profile (calm, documentary, nervous,
//!   running, shoulder), `drone` (rise, reveal, flyover, orbit, pullback)
//!   and `follow` (keeps pace with a subject).
//! * **Never the empty canvas.** [`Session::gaps`] casts rays through the
//!   frame at moments of a camera's span and reports where nothing opaque
//!   is hit: a 2.5D camera on stills must not show past the pictures.

use super::space::{Rig, add, axes, gen_params, key, len, mul, r1, rig_at, rig_value, sub, turn_about, v3, RIG_PATHS};
use super::{Session, f, micros, secs};
use crate::view::{self, SEC};
use serde_json::{Value, json};
use studio_model as m;
use studio_model::space as sp;

/// Sensor widths (mm) by name, the full-frame 36 mm first.
pub const SENSORS: [(&str, f64, &str); 9] = [
    ("full_frame", 36.0, "full frame"),
    ("large_format", 36.7, "large format"),
    ("super35", 24.89, "Super 35"),
    ("aps_c", 23.6, "APS-C"),
    ("micro_four_thirds", 17.3, "Micro Four Thirds"),
    ("super16", 12.52, "Super 16"),
    ("65mm", 52.48, "65 mm"),
    ("imax", 70.41, "IMAX 15/70"),
    ("phone", 7.6, "phone"),
];

/// A lens kit: its focal lengths and its character.
pub struct Kit {
    pub name: &'static str,
    pub label: &'static str,
    /// Focal lengths it has (mm); two values for a zoom (its range).
    pub focal: &'static [f64],
    pub zoom: bool,
    pub squeeze: f64,
    /// Barrel distortion (the fisheye effect's amount; 0 is none).
    pub distortion: f64,
    /// How much the picture grows as focus comes close (a fraction).
    pub breathing: f64,
    pub bokeh: &'static str,
    /// Colour fringes at the edges (chromatic aberration, % of height).
    pub fringe: f64,
    pub look: &'static str,
}

pub const KITS: [Kit; 8] = [
    Kit { name: "spherical", label: "spherical cine primes", focal: &[18.0, 25.0, 32.0, 40.0, 50.0, 65.0, 75.0, 100.0, 135.0], zoom: false, squeeze: 1.0, distortion: 0.0, breathing: 0.01, bokeh: "round", fringe: 0.0, look: "clean and true: the modern default" },
    Kit { name: "vintage", label: "vintage primes", focal: &[24.0, 28.0, 35.0, 50.0, 85.0, 105.0], zoom: false, squeeze: 1.0, distortion: 0.04, breathing: 0.04, bokeh: "hexagon", fringe: 0.25, look: "softer, warm character, colour fringes and six-sided bokeh: period films, memory" },
    Kit { name: "anamorphic", label: "anamorphic 2× primes", focal: &[32.0, 40.0, 50.0, 75.0, 100.0, 135.0], zoom: false, squeeze: 2.0, distortion: 0.06, breathing: 0.03, bokeh: "oval", fringe: 0.15, look: "widescreen cinema: twice the width of view, tall oval bokeh, barrel bend at the edges" },
    Kit { name: "anamorphic_1.5", label: "anamorphic 1.5× primes", focal: &[35.0, 50.0, 75.0, 100.0], zoom: false, squeeze: 1.5, distortion: 0.04, breathing: 0.02, bokeh: "oval", fringe: 0.1, look: "a lighter anamorphic look for 16:9 delivery" },
    Kit { name: "zoom", label: "cine zoom", focal: &[24.0, 290.0], zoom: true, squeeze: 1.0, distortion: 0.01, breathing: 0.02, bokeh: "octagon", fringe: 0.05, look: "any focal length between; documentary and long-lens work" },
    Kit { name: "macro", label: "macro primes", focal: &[60.0, 100.0], zoom: false, squeeze: 1.0, distortion: 0.0, breathing: 0.08, bokeh: "round", fringe: 0.0, look: "inserts and tiny worlds: focus very close, strong breathing" },
    Kit { name: "ultra_wide", label: "ultra-wide primes", focal: &[12.0, 14.0, 16.0], zoom: false, squeeze: 1.0, distortion: 0.12, breathing: 0.01, bokeh: "round", fringe: 0.1, look: "huge space, bent edges, figures stretched near the frame's sides" },
    Kit { name: "tele", label: "telephoto primes", focal: &[150.0, 200.0, 300.0, 400.0], zoom: false, squeeze: 1.0, distortion: 0.0, breathing: 0.02, bokeh: "round", fringe: 0.05, look: "compressed depth: far things stacked close, heat-haze distance" },
];

fn kit(name: &str) -> Option<&'static Kit> {
    let k = name.trim().to_lowercase().replace([' ', '-'], "_");
    let k = match k.as_str() {
        "anamorphic_2x" | "anamorphic2x" | "anamorphic_2" | "scope" | "anamorphic_lens" => "anamorphic",
        "anamorphic_1.5x" | "anamorphic1.5" | "anamorphic_15" => "anamorphic_1.5",
        "primes" | "modern" | "clean" | "sphericals" => "spherical",
        "old" | "retro" | "classic" | "soviet" => "vintage",
        "cine_zoom" | "zoom_lens" => "zoom",
        "wide" | "ultrawide" | "fisheye" => "ultra_wide",
        "telephoto" | "long" | "long_lens" => "tele",
        other => other,
    };
    KITS.iter().find(|x| x.name == k)
}

fn sensor(name: &str) -> Option<(&'static str, f64)> {
    let k = name.trim().to_lowercase().replace([' ', '-'], "_");
    let k = match k.as_str() {
        "ff" | "fullframe" | "35mm" | "vista" | "vistavision" => "full_frame",
        "lf" | "alexa_lf" => "large_format",
        "s35" | "super_35" | "super35mm" => "super35",
        "apsc" => "aps_c",
        "mft" | "m43" | "micro_4/3" => "micro_four_thirds",
        "s16" | "super_16" | "16mm" => "super16",
        "65" | "alexa_65" => "65mm",
        "imax_70" | "70mm" => "imax",
        "iphone" | "mobile" => "phone",
        other => other,
    };
    SENSORS.iter().find(|s| s.0 == k).map(|s| (s.0, s.1))
}

fn num(p: &m::ParamMap, k: &str, d: f64) -> f64 {
    p.get(k).and_then(m::Value::as_f64).unwrap_or(d)
}
fn text(p: &m::ParamMap, k: &str) -> Option<String> {
    p.get(k).and_then(|v| v.as_str()).map(str::to_owned)
}

/// The camera's lens as set: sensor width (mm), squeeze, kit.
pub struct Lens {
    pub sensor: f64,
    pub sensor_name: String,
    pub squeeze: f64,
    pub kit: Option<&'static Kit>,
    pub breathing: f64,
    pub bokeh: String,
}

pub fn lens_of(p: &m::ParamMap) -> Lens {
    let kit = text(p, "lens_kit").and_then(|k| kit(&k));
    let (sensor_name, sensor) = text(p, "sensor").and_then(|s| sensor(&s)).unwrap_or(("full_frame", 36.0));
    Lens {
        sensor,
        sensor_name: sensor_name.to_owned(),
        squeeze: num(p, "squeeze", kit.map_or(1.0, |k| k.squeeze)).max(1.0),
        kit,
        breathing: num(p, "breathing", kit.map_or(0.0, |k| k.breathing)).max(0.0),
        bokeh: text(p, "bokeh").unwrap_or_else(|| "round".into()),
    }
}

impl Lens {
    /// The camera's zoom (px) for a focal length on this sensor: the
    /// squeeze widens the view across.
    pub fn zoom(&self, mm: f64, width: f64) -> f64 {
        width * mm.max(1.0) / (self.sensor * self.squeeze)
    }
    pub fn focal(&self, zoom: f64, width: f64) -> f64 {
        zoom * self.sensor * self.squeeze / width.max(1.0)
    }
    /// The kit's nearest focal length to `mm` (any within a zoom's range).
    pub fn nearest(&self, mm: f64) -> f64 {
        match self.kit {
            Some(k) if k.zoom => mm.clamp(k.focal[0], k.focal[1]),
            Some(k) => k.focal.iter().copied().min_by(|a, b| (a - mm).abs().total_cmp(&(b - mm).abs())).unwrap_or(mm),
            None => mm,
        }
    }
    /// "40 mm anamorphic 2× primes on Super 35".
    pub fn words(&self, zoom: f64, width: f64) -> String {
        let mm = self.focal(zoom, width);
        let kit = self.kit.map(|k| format!(" {}", k.label)).unwrap_or_default();
        let squeeze = if self.kit.is_none() && self.squeeze > 1.0 { format!(" anamorphic {}×", self.squeeze) } else { String::new() };
        let on = if self.sensor_name != "full_frame" || self.kit.is_some() { format!(" on {}", SENSORS.iter().find(|s| s.0 == self.sensor_name).map_or("full frame", |s| s.2)) } else { String::new() };
        format!("{mm:.0} mm{kit}{squeeze}{on}")
    }
}

/// A focal length given as words: "anamorphic 40", "40mm vintage",
/// "50 mm".
fn focal_words(s: &str) -> (Option<f64>, Option<&'static Kit>) {
    let low = s.to_lowercase();
    let mm = low.split(|c: char| !(c.is_ascii_digit() || c == '.')).filter_map(|w| w.parse::<f64>().ok()).find(|v| *v >= 4.0);
    let rest: String = low.chars().filter(|c| c.is_alphabetic() || *c == ' ' || *c == '_' || *c == '.').collect();
    let k = rest.split_whitespace().filter(|w| *w != "mm" && *w != "lens").collect::<Vec<_>>().join("_");
    (mm, if k.is_empty() { None } else { kit(&k).or_else(|| kit(k.split('_').next().unwrap_or(""))) })
}

/// Smooth value noise in -1..1 (a hash on whole numbers, eased between).
fn noise(x: f64, seed: u64) -> f64 {
    let h = |i: i64| {
        let mut z = (i as u64).wrapping_add(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z ^= z >> 31;
        z = z.wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 29;
        (z >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    };
    let i = x.floor();
    let t = x - i;
    let s = t * t * (3.0 - 2.0 * t);
    h(i as i64) * (1.0 - s) + h(i as i64 + 1) * s
}

/// Two octaves of [`noise`].
fn wobble(x: f64, seed: u64) -> f64 {
    noise(x, seed) * 0.7 + noise(x * 2.3 + 17.0, seed + 1) * 0.3
}

/// A hand-held profile: rotation shake (degrees), its rate (Hz),
/// travel (fraction of the width), a fast jitter (degrees) and a step
/// bounce (fraction of the width, at the walking rate).
struct Shake {
    turn: f64,
    rate: f64,
    travel: f64,
    jitter: f64,
    bounce: f64,
    roll: f64,
}

fn profile(name: &str) -> Option<Shake> {
    Some(match name {
        "calm" | "breathing" | "still" | "gentle" | "subtle" => Shake { turn: 0.25, rate: 0.35, travel: 0.0015, jitter: 0.0, bounce: 0.0, roll: 0.15 },
        "documentary" | "doc" | "natural" | "handheld" | "default" => Shake { turn: 0.6, rate: 0.7, travel: 0.003, jitter: 0.08, bounce: 0.0, roll: 0.3 },
        "shoulder" | "shoulder_rig" | "heavy" => Shake { turn: 0.4, rate: 0.45, travel: 0.002, jitter: 0.03, bounce: 0.001, roll: 0.6 },
        "nervous" | "tense" | "anxious" | "frantic" => Shake { turn: 1.2, rate: 1.4, travel: 0.004, jitter: 0.25, bounce: 0.0, roll: 0.5 },
        "running" | "run" | "chase" | "action" => Shake { turn: 2.0, rate: 1.2, travel: 0.01, jitter: 0.3, bounce: 0.008, roll: 0.9 },
        _ => return None,
    })
}

const PROFILES: &str = "calm, documentary, shoulder, nervous, running";

/// Fields each rig move takes.
fn move_fields(kind: &str) -> Option<&'static [&'static str]> {
    Some(match kind {
        "rack_focus" | "focus_pull" | "pull_focus" | "rack" => &["type", "to", "from", "duration", "ease"],
        "follow_focus" => &["type", "subject", "to", "from", "duration"],
        "dolly_path" | "path" | "track_path" => &["type", "path", "aim", "from", "duration", "ease"],
        "crane" | "jib" | "crane_up" | "crane_down" => &["type", "by", "arm", "swing", "aim", "from", "duration", "ease"],
        "steadicam" | "glide" | "float" => &["type", "by", "subject", "sway", "from", "duration", "ease"],
        "handheld" => &["type", "profile", "by", "from", "duration", "ease"],
        "drone" | "aerial" => &["type", "style", "by", "height", "from", "duration", "ease"],
        "follow" | "track_subject" => &["type", "subject", "from", "duration", "aim"],
        _ => return None,
    })
}

/// What a rig move does at progress u (0..1) and with which ease.
type State = Box<dyn Fn(f64) -> Rig>;

impl Session {
    /// The lens fields of the `camera` op: `focal_length` (mm or words
    /// like "anamorphic 40"), `zoom`, `fov`, `lens_kit`, `sensor`,
    /// `squeeze`, `breathing` (%), `bokeh`, `distortion`, `fringe`. Sets
    /// the camera's parameters; returns what to say, and whether the lens
    /// layer must be brought up to date.
    pub(super) fn lens_setup(&mut self, mc: &mut m::Clip, op: &Value) -> Result<(Vec<String>, bool), String> {
        let w = self.width();
        let mut said = Vec::new();
        let set = |mc: &mut m::Clip, k: &str, v: m::Value| {
            if let m::ClipSource::Generator(g) = &mut mc.source {
                g.params.insert(k.into(), v);
            }
        };
        let mut mm = None;
        let mut kit_named = None;
        match op.get("focal_length") {
            Some(Value::String(s)) => {
                let (n, k) = focal_words(s);
                if n.is_none() && k.is_none() {
                    return Err(format!("focal_length \"{s}\": a number of mm, or a kit and mm (\"anamorphic 40\"); kits are {}", KITS.iter().map(|k| k.name).collect::<Vec<_>>().join(", ")));
                }
                mm = n;
                kit_named = k;
            }
            Some(v) => mm = Some(v.as_f64().ok_or("focal_length is mm (a number) or words like \"anamorphic 40\"")?),
            None => {}
        }
        if let Some(k) = op.get("lens_kit").and_then(Value::as_str) {
            kit_named = Some(kit(k).ok_or_else(|| format!("lens_kit \"{k}\": {}", KITS.iter().map(|k| format!("{} ({})", k.name, k.look)).collect::<Vec<_>>().join("; ")))?);
        }
        let before = lens_of(&gen_params(mc));
        let mut character = false;
        if let Some(k) = kit_named {
            set(mc, "lens_kit", m::Value::Choice(k.name.into()));
            for p in ["squeeze", "breathing", "bokeh", "distortion", "fringe"] {
                if let m::ClipSource::Generator(g) = &mut mc.source {
                    g.params.remove(p);
                }
            }
            if k.bokeh != "round" {
                set(mc, "bokeh", m::Value::Choice(k.bokeh.into()));
            }
            set(mc, "distortion", m::Value::Scalar(k.distortion));
            set(mc, "fringe", m::Value::Scalar(k.fringe));
            said.push(format!("{} ({})", k.label, k.look));
            character = true;
        }
        if let Some(s) = op.get("sensor").and_then(Value::as_str) {
            let (n, _) = sensor(s).ok_or_else(|| format!("sensor \"{s}\": {}", SENSORS.iter().map(|s| s.0).collect::<Vec<_>>().join(", ")))?;
            set(mc, "sensor", m::Value::Choice(n.into()));
        }
        if let Some(q) = f(op, "squeeze") {
            if !(1.0..=2.5).contains(&q) {
                return Err("squeeze is 1 (spherical) to 2.5; anamorphic lenses are 1.33, 1.5, 1.8 or 2".into());
            }
            set(mc, "squeeze", m::Value::Scalar(q));
        }
        if let Some(b) = f(op, "breathing") {
            set(mc, "breathing", m::Value::Scalar((b / 100.0).clamp(0.0, 0.3)));
        }
        if let Some(b) = op.get("bokeh").and_then(Value::as_str) {
            let b = match b.to_lowercase().as_str() {
                "round" | "disc" | "circle" | "circular" => "round",
                "hexagon" | "hex" | "six" => "hexagon",
                "octagon" | "eight" => "octagon",
                "oval" | "anamorphic" | "ellipse" | "cat_eye" => "oval",
                "star" => "star",
                other => return Err(format!("bokeh \"{other}\": round, hexagon, octagon, oval (anamorphic) or star")),
            };
            set(mc, "bokeh", m::Value::Choice(b.into()));
        }
        for k in ["distortion", "fringe"] {
            if let Some(v) = f(op, k) {
                set(mc, k, m::Value::Scalar((v / 100.0).clamp(0.0, 1.0) * if k == "fringe" { 5.0 } else { 1.0 }));
                character = true;
            }
        }
        let after = lens_of(&gen_params(mc));
        // A new sensor, squeeze or kit keeps the focal length, not the
        // picture: the same 40 mm frames wider on a smaller sensor.
        let cur = num(&gen_params(mc), "zoom", sp::default_zoom(w));
        let keep_mm = before.focal(cur, w);
        let lens_changed = (before.sensor - after.sensor).abs() > 1e-6 || (before.squeeze - after.squeeze).abs() > 1e-6;
        let target = match (mm, f(op, "zoom"), f(op, "fov")) {
            (Some(mm), _, _) => Some(after.zoom(after.nearest(mm), w)),
            (None, Some(z), _) => Some(z),
            (None, None, Some(d)) => Some(sp::zoom_from_fov(d, w)),
            _ if lens_changed || kit_named.is_some() => Some(after.zoom(after.nearest(keep_mm), w)),
            _ => None,
        };
        if let Some(z) = target {
            set(mc, "zoom", m::Value::Scalar(z.max(1.0)));
            if let (Some(want), Some(k)) = (mm, after.kit) {
                let got = after.nearest(want);
                if (got - want).abs() > 0.5 {
                    said.push(format!("the {} have no {want:.0} mm: {got:.0} mm", k.label));
                }
            }
        }
        Ok((said, character || op.get("bokeh").is_some()))
    }

    /// The "Lens" adjustment layer over a camera's shot: its kit's barrel
    /// distortion and colour fringes. Made, changed or removed to match.
    pub(super) fn lens_layer(&mut self, cam: u32) -> Result<Option<String>, String> {
        let mc = self.model_clip(cam).ok_or("camera gone")?;
        let p = gen_params(&mc);
        let (dist, fringe) = (num(&p, "distortion", 0.0), num(&p, "fringe", 0.0));
        let tag = format!("Lens (camera {cam})");
        let seq = self.doc.seq();
        let existing = self.doc.model.sequence(seq).and_then(|q| q.clips.iter().find(|c| c.name == tag).cloned());
        if dist <= 0.0 && fringe <= 0.0 {
            if let Some(c) = existing {
                let mut c = c;
                c.enabled = false;
                self.apply_clip("Lens", c)?;
            }
            return Ok(None);
        }
        let mut c = match existing {
            Some(c) => c,
            None => {
                let lane = self.room(None, mc.start, mc.duration, None)?;
                let id = self.doc.alloc_id();
                let mut c = m::Clip::new(m::ClipId(id), m::LaneId(lane as u64), mc.start, mc.duration, m::ClipSource::Generator(m::Generator::new(m::GeneratorKind::Adjustment)));
                c.name = tag.clone();
                let ok = self.doc.apply("Lens", m::Edit::InsertClip { seq, clip: Box::new(c.clone()) });
                self.check(ok, "")?;
                c
            }
        };
        c.enabled = true;
        c.start = mc.start;
        c.duration = mc.duration;
        c.effects.retain(|e| !matches!(e.kind.as_str(), "fisheye" | "chromatic_aberration"));
        if dist > 0.0 {
            let mut e = m::Effect::new(m::EffectId(self.doc.alloc_id()), "fisheye");
            e.params.insert("amount".into(), m::Value::Scalar(dist));
            // Zoomed in just enough that the bent corners stay filled.
            e.params.insert("zoom".into(), m::Value::Scalar(1.0 + dist * 0.6));
            c.effects.push(e);
        }
        if fringe > 0.0 {
            let mut e = m::Effect::new(m::EffectId(self.doc.alloc_id()), "chromatic_aberration");
            e.params.insert("amount".into(), m::Value::Scalar(fringe));
            c.effects.push(e);
        }
        let id = c.id.0;
        self.apply_clip("Lens", c)?;
        Ok(Some(format!("lens layer clip {id} over the shot (barrel {:.0}%, fringes {:.2})", dist * 100.0, fringe)))
    }

    /// A rig move the `space` camera does not know; `None` for its own.
    pub(super) fn rig_move(&mut self, mc: &mut m::Clip, vc: &view::Clip, mv: &Value) -> Result<Option<String>, String> {
        let kind = mv.get("type").or(mv.get("kind")).and_then(Value::as_str).unwrap_or("").to_lowercase();
        // A plain shake, or a handheld move without a profile, stays the
        // camera's own (as before).
        let handheld_profile = kind == "handheld" && mv.get("profile").is_some();
        let Some(fields) = move_fields(&kind).filter(|_| kind != "handheld" || handheld_profile) else { return Ok(None) };
        if let Some(k) = mv.as_object().and_then(|o| o.keys().find(|k| !fields.contains(&k.as_str()))) {
            return Err(format!("a {kind} move has no field \"{k}\"; it takes {}", fields.join(", ")));
        }
        let w = self.width();
        let from = micros(f(mv, "from").unwrap_or(0.0)).clamp(0, vc.dur);
        let dur = f(mv, "duration").map(micros).unwrap_or(vc.dur - from).clamp(SEC / 30, (vc.dur - from).max(SEC / 30));
        let ease = mv.get("ease").and_then(Value::as_str).unwrap_or("ease_in_out").to_owned();
        super::craft::curve(&ease).ok_or_else(|| format!("ease \"{ease}\": {}", super::craft::CURVES))?;
        let set = |mc: &mut m::Clip, k: &str, v: m::Value| {
            if let m::ClipSource::Generator(g) = &mut mc.source {
                g.params.insert(k.into(), v);
            }
        };
        if matches!(kind.as_str(), "rack_focus" | "focus_pull" | "pull_focus" | "rack" | "follow_focus") {
            return self.focus_move(mc, vc, mv, &kind, from, dur, &ease).map(Some);
        }
        // Rigs aim: everything but a free path looks at a point.
        if !sp::CameraSettings::read(&gen_params(mc), w).two_node {
            let r = rig_at(mc, from, w);
            let fwd = axes(&r, false)[2];
            let d = sp::default_zoom(w);
            set(mc, "two_node", m::Value::Bool(true));
            let poi = add(r.pos, mul(fwd, d));
            set(mc, "poi", m::Value::Point(m::Vec2::new(poi[0], poi[1])));
            set(mc, "poi_z", m::Value::Scalar(poi[2]));
        }
        let r0 = rig_at(mc, from, w);
        let ax = axes(&r0, true);
        let dist = len(sub(r0.poi, r0.pos)).max(1.0);
        let by = f(mv, "by");
        let seed = mc.id.0;
        let secs_ = secs(dur);
        // A subject's position over the move, by progress.
        let track = |s: &Session, v: Option<&Value>| -> Result<Option<Vec<[f64; 3]>>, String> {
            let Some(v) = v else { return Ok(None) };
            let id = v.get("clip").and_then(Value::as_u64).or_else(|| v.as_u64()).ok_or("subject is a clip")? as u32;
            let sc = s.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
            let seq = s.doc.model.sequence(s.doc.seq()).ok_or("no sequence")?;
            Ok(Some((0..=40).map(|i| {
                let t = vc.start + from + dur * i / 40;
                let tr = seq.placed_transform(&sc, t.clamp(sc.start, sc.end() - 1));
                [tr.position.x, tr.position.y, tr.position_z]
            }).collect()))
        };
        let at_u = |path: &[[f64; 3]], u: f64| {
            let x = u.clamp(0.0, 1.0) * (path.len() - 1) as f64;
            let i = (x.floor() as usize).min(path.len() - 2);
            let k = x - i as f64;
            add(mul(path[i], 1.0 - k), mul(path[i + 1], k))
        };
        let (state, steps, eased, words): (State, usize, bool, String) = match kind.as_str() {
            "dolly_path" | "path" | "track_path" => {
                let pts: Vec<[f64; 3]> = match mv.get("path") {
                    Some(Value::Array(a)) if !a.is_empty() => a.iter().map(|p| v3(p).ok_or("each path point is [x, y, z]")).collect::<Result<_, _>>()?,
                    _ => return Err("dolly_path needs \"path\": points [x, y, z] the dolly runs through after where the camera stands".into()),
                };
                let mut all = vec![r0.pos];
                all.extend(pts);
                // Catmull-Rom through the points, spaced by distance so the
                // dolly runs at an even speed.
                let n = all.len();
                let p = |i: isize| all[i.clamp(0, n as isize - 1) as usize];
                let mut dense = Vec::new();
                for i in 0..n - 1 {
                    let (a, b, c, d) = (p(i as isize - 1), p(i as isize), p(i as isize + 1), p(i as isize + 2));
                    for s in 0..24 {
                        let t = s as f64 / 24.0;
                        let (t2, t3) = (t * t, t * t * t);
                        dense.push(std::array::from_fn(|k| 0.5 * (2.0 * b[k] + (c[k] - a[k]) * t + (2.0 * a[k] - 5.0 * b[k] + 4.0 * c[k] - d[k]) * t2 + (3.0 * b[k] - a[k] - 3.0 * c[k] + d[k]) * t3)));
                    }
                }
                dense.push(all[n - 1]);
                let mut acc = vec![0.0];
                for wd in dense.windows(2) {
                    acc.push(acc.last().copied().unwrap_or(0.0) + len(sub(wd[1], wd[0])));
                }
                let total = acc.last().copied().unwrap_or(0.0).max(1e-6);
                let even: Vec<[f64; 3]> = (0..=120)
                    .map(|i| {
                        let want = total * i as f64 / 120.0;
                        let j = acc.partition_point(|a| *a < want).clamp(1, dense.len() - 1);
                        let k = ((want - acc[j - 1]) / (acc[j] - acc[j - 1]).max(1e-9)).clamp(0.0, 1.0);
                        add(mul(dense[j - 1], 1.0 - k), mul(dense[j], k))
                    })
                    .collect();
                let aim = mv.get("aim").cloned().unwrap_or(json!("keep"));
                let follow = if aim.is_object() || aim.is_u64() { track(self, Some(&aim))? } else { None };
                let fixed = match &aim {
                    Value::Array(_) => Some(self.point_of(&aim)?),
                    Value::String(s) if s == "keep" => Some(r0.poi),
                    Value::String(s) if s == "ahead" || s == "path" => None,
                    Value::String(s) => return Err(format!("aim \"{s}\": keep (looks where it looked), ahead (along the track), a point [x, y, z] or a clip")),
                    _ => None,
                };
                let ahead = aim.as_str().is_some_and(|s| s == "ahead" || s == "path");
                (
                    Box::new(move |u| {
                        let pos = at_u(&even, u);
                        let poi = if ahead {
                            let next = at_u(&even, (u + 0.04).min(1.0));
                            let d = sub(next, pos);
                            let d = if len(d) < 1e-6 { sub(pos, at_u(&even, (u - 0.04).max(0.0))) } else { d };
                            add(pos, mul(d, dist / len(d).max(1e-6)))
                        } else if let Some(fp) = &follow {
                            at_u(fp, u)
                        } else {
                            fixed.unwrap_or(r0.poi)
                        };
                        Rig { pos, poi, ..r0 }
                    }),
                    ((secs_ * 6.0).round() as usize).clamp(12, 240),
                    true,
                    format!("dolly along a {:.0} px track through {} points", total, n - 1),
                )
            }
            "crane" | "jib" | "crane_up" | "crane_down" => {
                let deg = by.unwrap_or(30.0) * if kind == "crane_down" { -1.0 } else { 1.0 };
                let arm = f(mv, "arm").unwrap_or(0.6 * dist).max(10.0);
                let swing = f(mv, "swing").unwrap_or(0.0);
                // The arm reaches toward the scene from a pivot behind and
                // level with the camera; it swings up (or down) round it.
                let flat = { let d = sub(r0.poi, r0.pos); let h = [d[0], 0.0, d[2]]; if len(h) < 1e-6 { [0.0, 0.0, 1.0] } else { mul(h, 1.0 / len(h)) } };
                let pivot = sub(r0.pos, mul(flat, arm));
                let level = mv.get("aim").and_then(Value::as_str) == Some("level");
                (
                    Box::new(move |u| {
                        let a = (deg * u).to_radians();
                        let dir = turn_about(flat, [0.0; 3], [0.0, 1.0, 0.0], -swing * u);
                        let pos = add(pivot, add(mul(dir, arm * a.cos()), [0.0, -arm * a.sin(), 0.0]));
                        let poi = if level { add(r0.poi, sub(pos, r0.pos)) } else { turn_about(r0.poi, pivot, [0.0, 1.0, 0.0], -swing * u) };
                        Rig { pos, poi, ..r0 }
                    }),
                    12,
                    true,
                    format!("crane {} {:.0}° on a {:.0} px arm{}", if deg >= 0.0 { "up" } else { "down" }, deg.abs(), arm, if swing != 0.0 { format!(", swinging {swing:.0}°") } else { String::new() }),
                )
            }
            "steadicam" | "glide" | "float" => {
                let d = by.unwrap_or(0.2 * dist);
                let sway = f(mv, "sway").map_or(1.0, |s| s / 100.0);
                let follow = track(self, mv.get("subject"))?;
                let keeps = if follow.is_some() { ", keeping the subject in its eye" } else { "" };
                let walk = 1.7; // steps a second
                (
                    Box::new(move |u| {
                        let t = u * secs_;
                        let side = wobble(t * 0.3, seed) * 0.004 * w * sway;
                        let bob = (t * walk * std::f64::consts::TAU).sin() * 0.0015 * w * sway;
                        let off = add(mul(ax[0], side), mul(ax[1], bob));
                        let along = mul(ax[2], d * u);
                        let base_poi = match &follow { Some(fp) => at_u(fp, u), None => add(r0.poi, along) };
                        Rig { pos: add(add(r0.pos, along), off), poi: add(base_poi, mul(off, 0.4)), roll: r0.roll + wobble(t * 0.25, seed + 7) * 0.3 * sway, ..r0 }
                    }),
                    ((secs_ * 8.0).round() as usize).clamp(8, 400),
                    false,
                    format!("steadicam glide {:.0} px{keeps}", d),
                )
            }
            "handheld" => {
                let name = mv.get("profile").and_then(Value::as_str).unwrap_or("documentary").to_lowercase();
                let s = profile(&name).ok_or_else(|| format!("profile \"{name}\": {PROFILES}"))?;
                let k = by.map_or(1.0, |b| b / 100.0);
                (
                    Box::new(move |u| {
                        let t = u * secs_;
                        let deg = |ch: u64| (wobble(t * s.rate, seed + ch) * s.turn + noise(t * 9.0, seed + ch + 50) * s.jitter) * k;
                        // Mostly the camera turns in the hands: aim moves.
                        let yaw = deg(1).to_radians().tan() * dist;
                        let pitch = deg(2).to_radians().tan() * dist;
                        let step = if s.bounce > 0.0 { ((t * 2.6 * std::f64::consts::PI).sin().abs() - 0.64) * s.bounce * w * k } else { 0.0 };
                        let tr = add(mul(ax[0], wobble(t * s.rate * 0.6, seed + 3) * s.travel * w * k), mul(ax[1], wobble(t * s.rate * 0.5, seed + 4) * s.travel * w * k + step));
                        Rig { pos: add(r0.pos, tr), poi: add(add(r0.poi, tr), add(mul(ax[0], yaw), mul(ax[1], pitch))), roll: r0.roll + wobble(t * s.rate * 0.8, seed + 5) * s.roll * k, ..r0 }
                    }),
                    ((secs_ * 12.0).round() as usize).clamp(8, 720),
                    false,
                    format!("handheld ({name})"),
                )
            }
            "drone" | "aerial" => {
                let style = mv.get("style").and_then(Value::as_str).unwrap_or("rise").to_lowercase();
                let h = f(mv, "height").unwrap_or(0.5 * dist);
                let float = move |u: f64| {
                    let t = u * secs_;
                    [wobble(t * 0.2, seed) * 0.0012 * w, wobble(t * 0.17, seed + 9) * 0.0008 * w, 0.0]
                };
                let flat = { let d = sub(r0.poi, r0.pos); let hz = [d[0], 0.0, d[2]]; if len(hz) < 1e-6 { [0.0, 0.0, 1.0] } else { mul(hz, 1.0 / len(hz)) } };
                let st: State = match style.as_str() {
                    // Rises over the scene, tilting down to keep it.
                    "rise" | "up" | "ascend" => {
                        let fwd = by.unwrap_or(0.15 * dist);
                        Box::new(move |u| Rig { pos: add(add(r0.pos, [0.0, -h * u, 0.0]), add(mul(flat, fwd * u), float(u))), ..r0 })
                    }
                    // Rises from behind the foreground; the aim travels out to
                    // the distance.
                    "reveal" => {
                        let out = by.unwrap_or(0.8 * dist);
                        Box::new(move |u| Rig { pos: add(add(r0.pos, [0.0, -h * u, 0.0]), float(u)), poi: add(r0.poi, mul(flat, out * u)), ..r0 })
                    }
                    // Flies forward over it at a steady height.
                    "flyover" | "fly_over" | "forward" => {
                        let d = by.unwrap_or(1.2 * dist);
                        Box::new(move |u| {
                            let step = add(mul(flat, d * u), float(u));
                            Rig { pos: add(add(r0.pos, [0.0, -h * 0.3, 0.0]), step), poi: add(r0.poi, step), ..r0 }
                        })
                    }
                    "orbit" | "circle" => {
                        let deg = by.unwrap_or(90.0);
                        let c = r0.poi;
                        Box::new(move |u| Rig { pos: add(add(turn_about(r0.pos, c, [0.0, 1.0, 0.0], -deg * u), [0.0, -h * 0.4 * u, 0.0]), float(u)), poi: c, ..r0 })
                    }
                    // Backs away and climbs: the ending.
                    "pullback" | "pull_back" | "pull_out" | "away" => {
                        let d = by.unwrap_or(0.8 * dist);
                        Box::new(move |u| Rig { pos: add(add(r0.pos, add(mul(flat, -d * u), [0.0, -h * u, 0.0])), float(u)), ..r0 })
                    }
                    other => return Err(format!("drone style \"{other}\": rise, reveal, flyover, orbit, pullback")),
                };
                (st, ((secs_ * 4.0).round() as usize).clamp(10, 200), true, format!("drone {style}"))
            }
            "follow" | "track_subject" => {
                let fp = track(self, mv.get("subject"))?.ok_or("follow needs \"subject\": the clip the camera keeps pace with")?;
                let start = fp[0];
                let aim_lead = mv.get("aim").and_then(Value::as_str) == Some("ahead");
                (
                    Box::new(move |u| {
                        let d = sub(at_u(&fp, u), start);
                        let lead = if aim_lead { mul(sub(at_u(&fp, (u + 0.05).min(1.0)), at_u(&fp, u)), 3.0) } else { [0.0; 3] };
                        Rig { pos: add(r0.pos, d), poi: add(add(r0.poi, d), lead), ..r0 }
                    }),
                    ((secs_ * 8.0).round() as usize).clamp(8, 400),
                    false,
                    "follows the subject, keeping its distance".into(),
                )
            }
            _ => return Ok(None),
        };
        self.bake(mc, from, dur, &ease, steps, eased, &*state)?;
        let travel = len(sub(state(1.0).pos, r0.pos));
        Ok(Some(format!("{words}{} over {:.2}s from {:.2}s into the camera clip", if travel > 1.0 { format!(" ({:.0} px of travel)", travel) } else { String::new() }, secs(dur), secs(from))))
    }

    /// Keys a rig's state at `steps` + 1 moments of a move, eased along
    /// when `eased` (the noise of a shake is not).
    fn bake(&mut self, mc: &mut m::Clip, from: m::Micros, dur: m::Micros, ease: &str, steps: usize, eased: bool, state: &dyn Fn(f64) -> Rig) -> Result<(), String> {
        let r0 = state(0.0);
        let r1_ = state(1.0);
        let mut paths: Vec<&str> = RIG_PATHS.iter().copied().filter(|p| rig_value(&r0, p) != rig_value(&r1_, p) || steps > 1).collect();
        if r1_.zoom != r0.zoom {
            paths.push("generator.zoom");
        }
        let ease_u = |u: f64| -> f64 {
            if !eased {
                return u;
            }
            // The same curve shapes as `animate`'s, sampled.
            let (interp, out, inn, shape) = super::craft::curve(ease).unwrap_or((m::Interp::Linear, None, None, None));
            let mut a = m::Keyframe::new(0, 0.0, interp);
            a.curve = shape;
            if let Some(o) = out {
                a.ease_out = o;
            }
            let mut b = m::Keyframe::new(1_000_000, 1.0, m::Interp::Linear);
            if let Some(i) = inn {
                b.ease_in = i;
            }
            m::Track::new(vec![a, b]).eval((u * 1e6).round() as m::Micros).and_then(|v| v.as_f64()).unwrap_or(u)
        };
        for i in 0..=steps {
            let u = i as f64 / steps as f64;
            let t = from + (dur as f64 * u).round() as m::Micros;
            let rig = state(ease_u(u));
            for p in &paths {
                key(mc, p, t, rig_value(&rig, p), "spline")?;
            }
        }
        Ok(())
    }

    /// `rack_focus` and `follow_focus`: keys on the focus distance (and on
    /// the zoom when the lens breathes).
    #[allow(clippy::too_many_arguments)]
    fn focus_move(&mut self, mc: &mut m::Clip, vc: &view::Clip, mv: &Value, kind: &str, from: m::Micros, dur: m::Micros, ease: &str) -> Result<String, String> {
        let w = self.width();
        let seq_id = self.doc.seq();
        let target = mv.get(if kind == "follow_focus" { "subject" } else { "to" }).or(mv.get("to")).ok_or_else(|| format!("{kind} needs \"{}\": a clip, a point [x, y, z] or a distance (px)", if kind == "follow_focus" { "subject" } else { "to" }))?.clone();
        // Distance from the lens along its view to the target at `t`.
        let depth_at = |s: &Session, mc: &m::Clip, t: m::Micros| -> Result<f64, String> {
            if let Some(d) = target.as_f64() {
                return Ok(d.max(1.0));
            }
            let seq = s.doc.model.sequence(seq_id).ok_or("no sequence")?;
            let p = match target.get("clip").and_then(Value::as_u64).or_else(|| target.as_u64()) {
                Some(id) => {
                    let sc = s.model_clip(id as u32).ok_or_else(|| format!("no clip {id}"))?;
                    // Its eyes, where focus goes on a figure.
                    match s.body(id as u32, vc.start + t) {
                        Ok(b) => b.eyes,
                        Err(_) => {
                            let tr = seq.placed_transform(&sc, (vc.start + t).clamp(sc.start, sc.end() - 1));
                            [tr.position.x, tr.position.y, tr.position_z]
                        }
                    }
                }
                None => s.point_of(&target)?,
            };
            let v = sp::camera_view(seq, mc, vc.start + t, w);
            Ok(v.view(p)[2].max(1.0))
        };
        let lens = lens_of(&gen_params(mc));
        let base_zoom = num(&gen_params(mc), "zoom", sp::default_zoom(w));
        let breathe = lens.breathing > 0.0 && !mc.keyframes.contains_key("generator.zoom");
        let zoom_for = |focus: f64| base_zoom * (1.0 + lens.breathing * (base_zoom / focus).clamp(0.25, 4.0).log2() / 2.0);
        if let m::ClipSource::Generator(g) = &mut mc.source {
            g.params.insert("dof".into(), m::Value::Bool(true));
        }
        let mut keys = Vec::new();
        if kind == "follow_focus" {
            let n = ((secs(dur) * 6.0).round() as usize).clamp(2, 600);
            for i in 0..=n {
                let t = from + dur * i as i64 / n as i64;
                keys.push((t, depth_at(self, mc, t)?, "spline"));
            }
        } else {
            let start = mc.generator_params_at(from).get("focus_distance").and_then(m::Value::as_f64).unwrap_or(num(&gen_params(mc), "focus_distance", base_zoom));
            let end = depth_at(self, mc, from + dur)?;
            keys.push((from, start, ease));
            keys.push((from + dur, end, "smooth"));
        }
        for (t, d, e) in &keys {
            key(mc, "generator.focus_distance", *t, m::Value::Scalar(*d), e)?;
            if breathe {
                key(mc, "generator.zoom", *t, m::Value::Scalar(zoom_for(*d)), e)?;
            }
        }
        let (a, b) = (keys.first().map_or(0.0, |k| k.1), keys.last().map_or(0.0, |k| k.1));
        Ok(format!(
            "{} from {} px to {} px over {:.2}s{}",
            if kind == "follow_focus" { "focus follows the subject" } else { "rack focus" },
            r1(a),
            r1(b),
            secs(dur),
            if breathe { format!(" (the lens breathes: {:.1}% larger at the closest)", (zoom_for(a.min(b)) / base_zoom - 1.0).abs() * 100.0) } else { String::new() }
        ))
    }

    /// Shaped bokeh: with a bokeh other than round, the renderer's round
    /// depth of field is turned off for this camera and each 3D picture it
    /// sees gets a lens blur of that shape, sized by its distance from the
    /// focus (keyed when that changes over the shot).
    pub(super) fn bake_bokeh(&mut self, cam: u32) -> Result<Option<String>, String> {
        let w = self.width();
        let ch = self.doc.project.height as f64;
        let mut mc = self.model_clip(cam).ok_or("camera gone")?;
        let p = gen_params(&mc);
        let lens = lens_of(&p);
        let shape = match lens.bokeh.as_str() {
            "hexagon" => "Hexagon",
            "octagon" => "Octagon",
            "oval" => "Oval",
            "star" => "Star",
            _ => return Ok(None),
        };
        let focus_on = p.get("dof").and_then(m::Value::as_bool).unwrap_or(false) || p.get("bokeh_dof").and_then(m::Value::as_bool).unwrap_or(false);
        if !focus_on {
            return Ok(None);
        }
        // The camera keeps its focus settings; the lens blurs instead.
        if let m::ClipSource::Generator(g) = &mut mc.source {
            g.params.insert("dof".into(), m::Value::Bool(false));
            g.params.insert("bokeh_dof".into(), m::Value::Bool(true));
        }
        self.apply_clip("Bokeh", mc.clone())?;
        let seq_id = self.doc.seq();
        let layers: Vec<m::Clip> = self
            .doc
            .model
            .sequence(seq_id)
            .map(|q| q.clips.iter().filter(|c| c.transform.three_d && c.enabled && c.start < mc.end() && c.end() > mc.start && matches!(c.source, m::ClipSource::Media(_) | m::ClipSource::Generator(_)) && !c.is_camera_or_light()).cloned().collect())
            .unwrap_or_default();
        let mut said = Vec::new();
        for mut lc in layers {
            let Some(geo) = self.plane_of(&lc, lc.start) else { continue };
            let (s, e) = (lc.start.max(mc.start), lc.end().min(mc.end()));
            let n = ((secs(e - s) * 4.0).round() as i64).clamp(1, 240);
            let mut samples = Vec::new();
            for i in 0..=n {
                let t = s + (e - s) * i / n;
                let Some(seq) = self.doc.model.sequence(seq_id) else { break };
                let Some(plane) = self.plane_of(&lc, t) else { continue };
                let cam_now = self.model_clip(cam).ok_or("camera gone")?;
                let view = sp::camera_view(seq, &cam_now, t, w);
                let set = sp::CameraSettings::read(&cam_now.generator_params_at((t - cam_now.start).clamp(0, cam_now.duration)), w);
                let centre = plane.0.at(plane.1.x / 2.0, plane.1.y / 2.0);
                let depth = view.view(centre)[2].max(1.0);
                // The renderer's own circle of confusion, as a radius.
                let sigma = 0.5 * set.aperture * set.blur_level * (depth - set.focus_distance).abs() / depth;
                let layer_h = len(sub(plane.0.at(0.0, plane.1.y), plane.0.at(0.0, 0.0))) * view.zoom / depth;
                let radius = (2.0 * sigma / layer_h.max(1.0) * 100.0).clamp(0.0, 10.0);
                samples.push((t - lc.start, radius));
            }
            let _ = geo;
            let Some(&(_, r0)) = samples.first() else { continue };
            let varies = samples.iter().any(|(_, r)| (r - r0).abs() > 0.05);
            let eid = match lc.effects.iter().find(|e| e.kind == "lens_blur" && e.extra.get("bokeh_of").is_some()) {
                Some(e) => e.id,
                None => {
                    let mut e = m::Effect::new(m::EffectId(self.doc.alloc_id()), "lens_blur");
                    e.extra.insert("bokeh_of".into(), json!(cam));
                    let id = e.id;
                    lc.effects.push(e);
                    id
                }
            };
            let path = format!("effect.{}.radius", eid.0);
            lc.keyframes.remove(&path);
            if let Some(e) = lc.effects.iter_mut().find(|e| e.id == eid) {
                e.params.insert("shape".into(), m::Value::Choice(shape.into()));
                e.params.insert("rotation".into(), m::Value::Scalar(if shape == "Oval" { 0.0 } else { 15.0 }));
                e.params.insert("radius".into(), m::Value::Scalar(r0));
            }
            if varies {
                let keys: Vec<m::Keyframe> = samples.iter().map(|(t, r)| m::Keyframe::new(*t, *r, m::Interp::Linear)).collect();
                lc.keyframes.insert(path, m::Track::new(keys));
            }
            let id = lc.id.0;
            let max = samples.iter().map(|s| s.1).fold(0.0, f64::max);
            self.apply_clip("Bokeh", lc)?;
            said.push(format!("{id} ({:.1}%{})", max, if varies { ", keyed" } else { "" }));
            let _ = ch;
        }
        Ok((!said.is_empty()).then(|| format!("{} bokeh baked as a lens blur on layers {} (the round depth of field is off for this camera; focus settings kept)", lens.bokeh, said.join(", "))))
    }

    /// The plane a 3D picture lies on at `t`, and its source size.
    pub(super) fn plane_of(&self, c: &m::Clip, t: m::Micros) -> Option<(sp::Plane, m::Vec2)> {
        let (cw, ch) = (self.doc.project.width as f64, self.doc.project.height as f64);
        let vc = self.doc.project.clip(c.id.0 as u32)?;
        let source = match vc.media.and_then(|m| self.doc.project.media(m)) {
            Some(mi) => m::Vec2::new(mi.width.max(1) as f64, mi.height.max(1) as f64),
            None => m::Vec2::new(cw, ch),
        };
        let fit = (cw / source.x).min(ch / source.y);
        let seq = self.doc.model.sequence(self.doc.seq())?;
        let tr = sp::oriented_transform(seq, c, t.clamp(c.start, c.end() - 1), cw);
        tr.three_d.then(|| (sp::Plane::of(&tr, source, fit), source))
    }

    /// Where the empty canvas shows through a 3D shot between `t0` and
    /// `t1`: the first moment and the part of the frame where a ray meets
    /// no opaque picture. `None` when it never does (or nothing is 3D).
    pub(super) fn gaps(&self, t0: view::Time, t1: view::Time) -> Option<(view::Time, String)> {
        let p = &self.doc.project;
        let (cw, ch) = (p.width as f64, p.height as f64);
        let seq = self.doc.model.sequence(self.doc.seq())?;
        let hidden = |c: &m::Clip| seq.lane(c.lane).is_none_or(|l| l.hidden);
        let opaque = |c: &m::Clip| match &c.source {
            m::ClipSource::Media(r) => p.media(r.media.0 as u32).and_then(|mi| mi.path.clone()).is_none_or(|path| !super::shots::has_alpha(&path)),
            m::ClipSource::Generator(g) => matches!(g.kind, m::GeneratorKind::Solid | m::GeneratorKind::Gradient),
            _ => false,
        };
        let n = 10;
        let mut any3d = false;
        for k in 0..=n {
            let t = t0 + (t1 - t0) * k / n;
            let t = if k == n { t - 1 } else { t };
            let on: Vec<&m::Clip> = seq.clips.iter().filter(|c| c.enabled && c.start <= t && t < c.end() && !hidden(c) && opaque(c)).collect();
            // A flat full-frame picture behind everything covers the frame.
            if on.iter().any(|c| !c.transform.three_d && seq.placed_transform(c, t).opacity > 0.95 && self.flat_covers(c, t)) {
                continue;
            }
            let planes: Vec<(sp::Plane, m::Vec2)> = on.iter().filter(|c| c.transform.three_d).filter_map(|c| self.plane_of(c, t)).collect();
            if planes.is_empty() {
                continue;
            }
            any3d = true;
            let view = sp::view_at(seq, t, cw);
            for (gy, yn) in [(-0.5, "top"), (0.0, ""), (0.5, "bottom")] {
                for (gx, xn) in [(-0.5, "left"), (-0.25, "left"), (0.0, ""), (0.25, "right"), (0.5, "right")] {
                    let ray = view.ray(m::Vec2::new(gx * cw * 0.995, gy * ch * 0.995));
                    let hit = planes.iter().any(|(pl, s)| matches!(pl.hit(view.position, ray), Some((u, v, d)) if d > 0.0 && (-0.5..=s.x + 0.5).contains(&u) && (-0.5..=s.y + 0.5).contains(&v)));
                    if !hit {
                        let place = match (yn, xn) {
                            ("", "") => "the middle".to_owned(),
                            (y, "") => format!("the {y} edge"),
                            ("", x) => format!("the {x} edge"),
                            (y, x) if gx.abs() == 0.5 => format!("the {y} {x} corner"),
                            (y, x) => format!("the {y} edge, {x}"),
                        };
                        return Some((t, place));
                    }
                }
            }
        }
        let _ = any3d;
        None
    }

    /// Whether a flat (2D) picture covers the whole frame at `t`.
    fn flat_covers(&self, c: &m::Clip, t: m::Micros) -> bool {
        let p = &self.doc.project;
        let Some(map) = crate::editor::pins::LayerMap::at(p, &self.doc.model, c.id.0 as u32, t) else { return false };
        let q: Vec<(f32, f32)> = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)].iter().map(|&(u, v)| {
            let pt = map.canvas(m::Vec2::new(u, v));
            (pt.x, pt.y)
        }).collect();
        let (hw, hh) = (p.width as f32 / 2.0 - 1.0, p.height as f32 / 2.0 - 1.0);
        [(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)].iter().all(|&(x, y)| {
            let mut sign = 0.0f32;
            (0..4).all(|i| {
                let (a, b) = (q[i], q[(i + 1) % 4]);
                let cr = (b.0 - a.0) * (y - a.1) - (b.1 - a.1) * (x - a.0);
                if cr.abs() < 1e-3 {
                    return true;
                }
                if sign == 0.0 {
                    sign = cr.signum();
                    true
                } else {
                    cr.signum() == sign
                }
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lens_maths_follows_the_sensor_and_squeeze() {
        let ff = Lens { sensor: 36.0, sensor_name: "full_frame".into(), squeeze: 1.0, kit: None, breathing: 0.0, bokeh: "round".into() };
        // Same as the model's full-frame conversion.
        assert!((ff.zoom(50.0, 1920.0) - sp::zoom_from_focal_length(50.0, 1920.0)).abs() < 1e-9);
        let s35 = Lens { sensor: 24.89, ..Lens { sensor_name: "super35".into(), ..ff } };
        // A smaller sensor crops: 50 mm frames tighter (a longer zoom).
        assert!(s35.zoom(50.0, 1920.0) > sp::zoom_from_focal_length(50.0, 1920.0));
        let ana = Lens { sensor: 24.89, sensor_name: "super35".into(), squeeze: 2.0, kit: kit("anamorphic"), breathing: 0.0, bokeh: "oval".into() };
        // 2× squeeze: twice the width of view of the same focal length.
        assert!((ana.zoom(50.0, 1920.0) * 2.0 - s35.zoom(50.0, 1920.0)).abs() < 1e-6);
        assert_eq!(ana.nearest(45.0), 40.0);
        assert!(ana.words(ana.zoom(40.0, 1920.0), 1920.0).starts_with("40 mm anamorphic 2× primes on Super 35"));
    }

    #[test]
    fn focal_words_read() {
        let (mm, k) = focal_words("anamorphic 40");
        assert_eq!(mm, Some(40.0));
        assert_eq!(k.map(|k| k.name), Some("anamorphic"));
        let (mm, k) = focal_words("85mm");
        assert_eq!(mm, Some(85.0));
        assert!(k.is_none());
        assert_eq!(focal_words("vintage 50 mm").1.map(|k| k.name), Some("vintage"));
    }

    #[test]
    fn noise_is_smooth_and_bounded() {
        let mut prev = noise(0.0, 3);
        for i in 1..1000 {
            let v = noise(i as f64 * 0.01, 3);
            assert!((-1.0..=1.0).contains(&v));
            assert!((v - prev).abs() < 0.05, "a jump at {i}");
            prev = v;
        }
    }
}
