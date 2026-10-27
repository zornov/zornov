// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Lighting set-ups for 2.5D and 3D scenes, for agents: the `lighting` op
//! puts up a whole rig round a subject, placed from where the camera
//! looks: three-point, Rembrandt, butterfly, split, loop, high key, low
//! key (noir), rim, silhouette, golden hour, blue hour, moonlight,
//! firelight, neon, overcast, under-light; or `motivated`, which reads
//! the backdrop for its brightest part (a window, the sun, a lamp) and
//! brings the key from there in its colour, with the fill from the
//! picture's own average colour. A mood (`warm`, `tense`, `romantic`…)
//! picks a set-up.
//!
//! Flat pictures in space are lit evenly across (they have no shape to
//! model), so on them a set-up mostly sets brightness, colour and which
//! side is brighter; a back light lights what is behind them. 3D models
//! are modelled by it fully.

use super::space::{add, len, mul, r1, sub};
use super::{Session, f, micros, secs};
use crate::view::SEC;
use serde_json::{Value, json};
use studio_model as m;
use studio_model::space as sp;

/// One light of a set-up, placed round the subject from the camera:
/// `side` degrees round toward screen left (0 = beside the lens, 90 = at
/// the subject's left, 180 = behind it), `up` degrees above.
struct Lamp {
    role: &'static str,
    kind: &'static str,
    side: f64,
    up: f64,
    intensity: f64,
    colour: &'static str,
    shadows: bool,
    cone: f64,
}

const fn lamp(role: &'static str, kind: &'static str, side: f64, up: f64, intensity: f64, colour: &'static str) -> Lamp {
    Lamp { role, kind, side, up, intensity, colour, shadows: false, cone: 60.0 }
}

const fn shadowed(l: Lamp) -> Lamp {
    Lamp { shadows: true, ..l }
}

/// Set-ups: name, words, lamps and an ambient (intensity %, colour).
struct Setup {
    name: &'static str,
    words: &'static str,
    lamps: &'static [Lamp],
    ambient: (f64, &'static str),
}

const SETUPS: &[Setup] = &[
    Setup { name: "three_point", words: "three-point: warm key 45° left and up, soft fill right, back light", lamps: &[shadowed(lamp("Key", "spot", 45.0, 35.0, 100.0, "#fff1e0")), lamp("Fill", "point", -50.0, 10.0, 40.0, "#e6efff"), lamp("Back", "spot", 160.0, 50.0, 70.0, "#ffffff")], ambient: (15.0, "#9aa6b8") },
    Setup { name: "rembrandt", words: "Rembrandt: high key light from the side, a triangle of light on the far cheek, little fill", lamps: &[shadowed(lamp("Key", "spot", 55.0, 45.0, 110.0, "#ffe8d0")), lamp("Fill", "point", -45.0, 0.0, 20.0, "#dfe6f2")], ambient: (10.0, "#5a6170") },
    Setup { name: "butterfly", words: "butterfly (Paramount): key straight in front and above, fill from below", lamps: &[shadowed(lamp("Key", "spot", 0.0, 55.0, 105.0, "#fff4e8")), lamp("Fill", "point", 0.0, -25.0, 35.0, "#f2f2f2")], ambient: (15.0, "#a0a0a8") },
    Setup { name: "loop", words: "loop: key a little to the side and above, gentle fill", lamps: &[shadowed(lamp("Key", "spot", 35.0, 30.0, 100.0, "#fff0dc")), lamp("Fill", "point", -40.0, 5.0, 35.0, "#e8eef8")], ambient: (15.0, "#9aa0ac") },
    Setup { name: "split", words: "split: one hard light from the side, the other half in shadow", lamps: &[shadowed(lamp("Key", "spot", 90.0, 10.0, 110.0, "#fff2e4"))], ambient: (6.0, "#404652") },
    Setup { name: "high_key", words: "high key: bright, soft, little shadow (comedy, commercials, joy)", lamps: &[lamp("Key", "spot", 30.0, 25.0, 90.0, "#ffffff"), lamp("Fill", "point", -35.0, 10.0, 70.0, "#ffffff"), lamp("Back", "spot", 170.0, 40.0, 50.0, "#ffffff")], ambient: (40.0, "#e8ecf2") },
    Setup { name: "low_key", words: "low key (noir): one hard key, deep shadow, almost no fill", lamps: &[Lamp { cone: 32.0, ..shadowed(lamp("Key", "spot", 70.0, 40.0, 120.0, "#f4ecd8")) }], ambient: (4.0, "#202838") },
    Setup { name: "rim", words: "rim: a strong light from behind outlines the subject against the dark", lamps: &[lamp("Rim", "spot", 165.0, 30.0, 140.0, "#e8f0ff"), lamp("Fill", "point", -30.0, 0.0, 15.0, "#9fb0d0")], ambient: (6.0, "#283040") },
    Setup { name: "silhouette", words: "silhouette: the background lit, the subject left dark in front of it", lamps: &[lamp("Background", "spot", 178.0, 20.0, 150.0, "#ffd9b0")], ambient: (2.0, "#101418") },
    Setup { name: "golden_hour", words: "golden hour: a low warm sun from the side, a cool sky fill", lamps: &[shadowed(lamp("Sun", "parallel", 65.0, 12.0, 110.0, "#ffb46b"))], ambient: (25.0, "#6f7fa8") },
    Setup { name: "blue_hour", words: "blue hour: soft cold light after sunset", lamps: &[lamp("Sky", "parallel", -40.0, 8.0, 55.0, "#8fa8ff")], ambient: (30.0, "#3a4a7a") },
    Setup { name: "moonlight", words: "moonlight: a cool hard light from high up, deep blue shadows", lamps: &[shadowed(lamp("Moon", "parallel", 60.0, 40.0, 50.0, "#9fb6ff"))], ambient: (10.0, "#1b2440") },
    Setup { name: "firelight", words: "firelight: a low warm flickering glow close by, dark around", lamps: &[lamp("Fire", "point", 40.0, -5.0, 95.0, "#ff9a3c")], ambient: (6.0, "#301a10") },
    Setup { name: "neon", words: "neon: magenta from one side, cyan from the other", lamps: &[lamp("Magenta", "spot", 60.0, 20.0, 90.0, "#ff3fb4"), lamp("Cyan", "spot", -60.0, 20.0, 80.0, "#3fe0ff")], ambient: (8.0, "#1a1028") },
    Setup { name: "overcast", words: "overcast: soft even light from above, no hard shadow", lamps: &[lamp("Sky", "parallel", 0.0, 70.0, 60.0, "#eef2f7")], ambient: (45.0, "#d8dee8") },
    Setup { name: "under", words: "under-light: a light from below (horror, the campfire story)", lamps: &[shadowed(lamp("Under", "spot", 10.0, -50.0, 95.0, "#c8ffd0"))], ambient: (5.0, "#101810") },
];

fn setup(name: &str) -> Option<&'static Setup> {
    let k = name.trim().to_lowercase().replace([' ', '-'], "_");
    let k = match k.as_str() {
        "3_point" | "three" | "classic" | "standard" | "interview" => "three_point",
        "paramount" | "glamour" | "beauty" => "butterfly",
        "noir" | "film_noir" | "lowkey" | "chiaroscuro" => "low_key",
        "highkey" | "bright" | "sitcom" => "high_key",
        "backlight" | "back_light" | "edge" | "kicker" => "rim",
        "golden" | "sunset" | "magic_hour" | "late_sun" => "golden_hour",
        "twilight" | "dusk" => "blue_hour",
        "night" | "moon" => "moonlight",
        "fire" | "candle" | "candlelight" | "campfire" => "firelight",
        "soft" | "cloudy" | "diffuse" => "overcast",
        "underlight" | "under_light" | "horror_light" => "under",
        o => o,
    };
    SETUPS.iter().find(|s| s.name == k)
}

/// A mood's set-up.
fn mood(word: &str) -> Option<&'static str> {
    Some(match word.trim().to_lowercase().as_str() {
        "warm" | "cosy" | "cozy" | "nostalgic" | "hopeful" => "golden_hour",
        "cool" | "cold" | "lonely" | "sad" | "melancholy" | "melancholic" => "blue_hour",
        "tense" | "menace" | "menacing" | "dramatic" | "mystery" | "mysterious" | "thriller" => "low_key",
        "romantic" | "dreamy" | "tender" => "butterfly",
        "happy" | "cheerful" | "joyful" | "comedy" | "fun" => "high_key",
        "scary" | "horror" | "eerie" | "creepy" => "under",
        "epic" | "heroic" => "rim",
        "night" | "quiet" => "moonlight",
        "party" | "club" | "cyberpunk" | "electric" => "neon",
        "natural" | "neutral" | "documentary" => "three_point",
        _ => return None,
    })
}

const SETUP_NAMES: &str = "three_point, rembrandt, butterfly, loop, split, high_key, low_key, rim, silhouette, golden_hour, blue_hour, moonlight, firelight, neon, overcast, under, motivated";

impl Session {
    /// `lighting`: a whole set-up of lights round a subject.
    pub(super) fn lighting(&mut self, op: &Value) -> Result<String, String> {
        let (w, h) = (self.width(), self.doc.project.height as f64);
        let at = micros(f(op, "at").unwrap_or(0.0));
        let end = f(op, "duration").map(|d| at + micros(d)).unwrap_or(super::new_clip_end(self.doc.project.duration(), at));
        let dur = (end - at).max(SEC / 10);
        let name = match (op.get("preset").and_then(Value::as_str), op.get("mood").and_then(Value::as_str)) {
            (Some(p), _) => p.to_owned(),
            (None, Some(md)) => mood(md).ok_or_else(|| format!("mood \"{md}\": warm, cool, tense, romantic, happy, scary, epic, night, party, natural (or a preset: {SETUP_NAMES})"))?.to_owned(),
            (None, None) => "three_point".to_owned(),
        };
        let motivated = matches!(name.as_str(), "motivated" | "from_backdrop" | "practical" | "source");
        let su = if motivated { None } else { Some(setup(&name).ok_or_else(|| format!("preset \"{name}\": {SETUP_NAMES}"))?) };
        // Where the subject is, and how big.
        let subject = op.get("subject").or(op.get("on")).cloned();
        let (centre, tall, sid) = match &subject {
            Some(v) => {
                let id = match v.as_u64().or_else(|| v.get("clip").and_then(Value::as_u64)) {
                    Some(id) => id as u32,
                    None => {
                        let n = v.as_str().ok_or("subject is a clip")?.to_lowercase();
                        self.doc.project.clips.iter().find(|c| c.name.to_lowercase() == n).map(|c| c.id).ok_or_else(|| format!("no clip named \"{n}\""))?
                    }
                };
                let b = self.body(id, at)?;
                (b.eyes, b.height, Some(id))
            }
            None => match self.doc.project.clips.iter().filter(|c| self.is_figure(c, at)).find_map(|c| self.body(c.id, at).ok()) {
                Some(b) => (b.eyes, b.height, Some(b.id)),
                None => ([0.0, 0.0, 0.0], 0.5 * h, None),
            },
        };
        let seq = self.doc.model.sequence(self.doc.seq()).ok_or("no sequence")?;
        let view = sp::view_at(seq, at, w);
        let (right, down, fwd) = (view.axes[0], view.axes[1], view.axes[2]);
        let reach = (1.4 * tall).max(0.4 * h);
        // A point round the subject, from the camera's side of it.
        let place = |side: f64, up: f64| {
            let (s, u) = (side.to_radians(), up.to_radians());
            let dir = add(add(mul(right, -s.sin() * u.cos()), mul(down, -u.sin())), mul(fwd, -s.cos() * u.cos()));
            add(centre, mul(dir, reach))
        };
        let scale = f(op, "intensity").map_or(1.0, |v| v / 100.0);
        // Earlier lights in the span are switched off (kept, to turn back on).
        let mut said = Vec::new();
        if op.get("replace").and_then(Value::as_bool).unwrap_or(true) {
            let old: Vec<m::Clip> = seq.clips.iter().filter(|c| c.enabled && super::space::rig_kind(c) == Some("light") && !matches!(super::space::gen_params(c).get("atmosphere"), Some(m::Value::Bool(true))) && c.start < end && c.end() > at).cloned().collect();
            if !old.is_empty() {
                said.push(format!("lights {} switched off (enabled: false brings them back)", old.iter().map(|c| c.id.0.to_string()).collect::<Vec<_>>().join(", ")));
            }
            for mut c in old {
                c.enabled = false;
                self.apply_clip("Lighting", c)?;
            }
        }
        let mut lamps: Vec<(String, &'static str, [f64; 3], f64, String, bool, f64)> = Vec::new();
        let ambient: (f64, String);
        let words: String;
        if let Some(su) = su {
            for l in su.lamps {
                lamps.push((l.role.to_owned(), l.kind, place(l.side, l.up), l.intensity, op.get("colour").and_then(Value::as_str).filter(|_| l.role == "Key").unwrap_or(l.colour).to_owned(), l.shadows, l.cone));
            }
            ambient = (su.ambient.0, su.ambient.1.to_owned());
            words = su.words.to_owned();
        } else {
            let (key, fill, amb, from) = self.motivation(at)?;
            // Sky at the top of the picture: a sun (parallel); else a
            // practical source (a window, a lamp) as a spot.
            let (sx, sy) = from;
            let kind = if sy < 0.3 { "parallel" } else { "spot" };
            let side = ((0.5 - sx) * 140.0).clamp(-80.0, 80.0);
            let up = ((0.5 - sy) * 90.0).clamp(-30.0, 60.0);
            let hex = |c: [f64; 3]| format!("#{:02x}{:02x}{:02x}", (c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8);
            lamps.push(("Key".into(), kind, place(side, up), 80.0 + 50.0 * key.3, hex([key.0, key.1, key.2]), true, 60.0));
            lamps.push(("Fill".into(), "point", place(-side * 0.8, 5.0), 25.0 + 20.0 * fill.3, hex([fill.0, fill.1, fill.2]), false, 60.0));
            ambient = (10.0 + 30.0 * amb.3, hex([amb.0, amb.1, amb.2]));
            let wh = |x: f64, y: f64| format!("{}{}", if y < 0.35 { "upper " } else if y > 0.65 { "lower " } else { "" }, if x < 0.38 { "left" } else if x > 0.62 { "right" } else { "centre" });
            words = format!("motivated by the backdrop: its brightest part is at the {} ({}), so the key comes from there in that colour; the fill takes the picture's own colour ({})", wh(sx, sy), hex([key.0, key.1, key.2]), hex([fill.0, fill.1, fill.2]));
        }
        let mut made = Vec::new();
        for (role, kind, pos, inten, colour, shadows, cone) in lamps {
            let lk = sp::LightKind::parse(kind).unwrap_or(sp::LightKind::Point);
            let id = self.add_rig(m::GeneratorKind::Light, &json!({"at": secs(at), "duration": secs(dur)}), &format!("{role} light"), sp::LightSettings::defaults(lk), pos)?;
            let mut lop = json!({"op": "light", "clip": id, "look_at": [r1(centre[0]), r1(centre[1]), r1(centre[2])], "colour": colour, "intensity": r1(inten * scale), "shadows": shadows});
            if kind == "spot" {
                lop["cone_angle"] = json!(cone);
                lop["cone_feather"] = json!(60);
            }
            // The lamps of a set-up can show themselves: given fields, else
            // what the set-up is (a fire flickers, neon glows).
            if matches!(kind, "spot" | "point") {
                let defaults: &[(&str, f64)] = match su.map(|x| x.name) {
                    Some("firelight") => &[("glow", 0.6), ("flicker", 0.35)],
                    Some("neon") => &[("glow", 0.9)],
                    _ => &[],
                };
                for k in ["glow", "flare", "beam", "flame", "flicker"] {
                    match op.get(k) {
                        Some(v) => lop[k] = v.clone(),
                        None => {
                            if let Some((_, d)) = defaults.iter().find(|(n, _)| *n == k) {
                                lop[k] = json!(d);
                            }
                        }
                    }
                }
                if lop.get("flame").is_some_and(|v| v.as_bool().unwrap_or_else(|| v.as_f64().is_some_and(|x| x > 0.0))) {
                    lop["flame_height"] = json!(0.12);
                }
            }
            self.light(&lop)?;
            made.push(format!("{role} #{id} ({kind}, {:.0}%)", inten * scale));
        }
        let aid = self.add_rig(m::GeneratorKind::Light, &json!({"at": secs(at), "duration": secs(dur)}), "Ambient light", sp::LightSettings::defaults(sp::LightKind::Ambient), [0.0; 3])?;
        self.light(&json!({"op": "light", "clip": aid, "colour": ambient.1, "intensity": r1(ambient.0 * scale)}))?;
        made.push(format!("Ambient #{aid} ({:.0}%)", ambient.0 * scale));
        // The air that goes with it.
        let air = match op.get("atmosphere") {
            Some(Value::Bool(true)) => Some(super::air::preset_for_lighting(if motivated { "motivated" } else { su.map_or("", |s| s.name) }).to_owned()),
            Some(Value::String(p)) => Some(p.clone()),
            Some(Value::Bool(false)) | None => None,
            Some(other) => return Err(format!("atmosphere is true (the air that goes with this set-up) or a preset name ({}); you gave {other}", super::air::preset_names())),
        };
        if let Some(p) = air {
            let r = self.atmosphere(&json!({"op": "atmosphere", "preset": p, "at": secs(at), "duration": secs(dur)}))?;
            made.push(r);
        }
        // Lights fall only on 3D layers.
        let flat: Vec<u32> = self.doc.project.clips.iter().filter(|c| c.start < end && c.start + c.dur > at && self.is_figure(c, (c.start + c.dur / 2).clamp(at, end - 1))).filter(|c| self.model_clip(c.id).is_some_and(|mc| !mc.transform.three_d && super::space::rig_kind(&mc) != Some("model"))).map(|c| c.id).collect();
        let mut out = format!("lighting {} ({:.2}–{:.2}s){}: {}; {}", if motivated { "motivated" } else { su.map_or("", |s| s.name) }, secs(at), secs(end), sid.map(|i| format!(" on #{i}")).unwrap_or_default(), words, made.join(", "));
        if !said.is_empty() {
            out.push_str(&format!("; {}", said.join("; ")));
        }
        if !flat.is_empty() {
            out.push_str(&format!("; note: clips {} are flat (2D): lights fall only on 3D layers (three_d)", flat.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")));
        }
        let _ = len(sub(centre, view.position));
        Ok(out)
    }

    /// The backdrop at `t` read for light: its brightest part's colour
    /// and brightness, the picture's mean colour (twice: fill and
    /// ambient), and where the bright part is (fractions of the frame).
    #[cfg(feature = "render")]
    fn motivation(&mut self, t: m::Micros) -> Result<((f64, f64, f64, f64), (f64, f64, f64, f64), (f64, f64, f64, f64), (f64, f64)), String> {
        let leave: Vec<u32> = self.doc.project.clips.iter().filter(|c| self.is_figure(c, t)).map(|c| c.id).collect();
        let img = self.frames.rgba_without(&self.doc, &leave, t, 192)?;
        let (w, h) = (img.width as usize, img.height as usize);
        let px = |x: usize, y: usize| {
            let i = (y * w + x) * 4;
            [img.pixels[i] as f64 / 255.0, img.pixels[i + 1] as f64 / 255.0, img.pixels[i + 2] as f64 / 255.0]
        };
        let lum = |c: [f64; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        // The brightest cell of a 12 × 8 grid.
        let (gx, gy) = (12, 8);
        let mut best = (0.0f64, 0, 0);
        let mut mean = [0.0f64; 3];
        for cy in 0..gy {
            for cx in 0..gx {
                let mut s = 0.0f64;
                let mut n = 0.0f64;
                for y in cy * h / gy..(cy + 1) * h / gy {
                    for x in cx * w / gx..(cx + 1) * w / gx {
                        let c = px(x, y);
                        s += lum(c);
                        n += 1.0;
                        for k in 0..3 {
                            mean[k] += c[k];
                        }
                    }
                }
                if s / n.max(1.0) > best.0 {
                    best = (s / n.max(1.0), cx, cy);
                }
            }
        }
        let total = (w * h) as f64;
        let mean = mean.map(|v| v / total.max(1.0));
        // The bright part's colour: the top 3% of that cell's surroundings.
        let mut cells: Vec<[f64; 3]> = Vec::new();
        for y in (best.2 * h / gy).saturating_sub(h / gy)..((best.2 + 2) * h / gy).min(h) {
            for x in (best.1 * w / gx).saturating_sub(w / gx)..((best.1 + 2) * w / gx).min(w) {
                cells.push(px(x, y));
            }
        }
        cells.sort_by(|a, b| lum(*b).total_cmp(&lum(*a)));
        let top = &cells[..(cells.len() / 30).max(1)];
        let key = top.iter().fold([0.0; 3], |a, c| [a[0] + c[0], a[1] + c[1], a[2] + c[2]]).map(|v| v / top.len() as f64);
        // A light's colour: the hue kept, brought up to full brightness.
        let bright = |c: [f64; 3]| {
            let m = c[0].max(c[1]).max(c[2]).max(1e-3);
            let k = 0.6 + 0.4 * (1.0 / m).min(2.0) / 2.0;
            c.map(|v| (v / m * k + (1.0 - k)).clamp(0.0, 1.0))
        };
        let (kc, fc) = (bright(key), bright(mean));
        let from = ((best.1 as f64 + 0.5) / gx as f64, (best.2 as f64 + 0.5) / gy as f64);
        Ok(((kc[0], kc[1], kc[2], lum(key)), (fc[0], fc[1], fc[2], lum(mean)), (mean[0], mean[1], mean[2], lum(mean)), from))
    }

    #[cfg(not(feature = "render"))]
    fn motivation(&mut self, _: m::Micros) -> Result<((f64, f64, f64, f64), (f64, f64, f64, f64), (f64, f64, f64, f64), (f64, f64)), String> {
        Err("motivated lighting reads the rendered backdrop; this build has no renderer".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setups_and_moods_resolve() {
        for s in SETUP_NAMES.split(", ").filter(|s| *s != "motivated") {
            assert!(setup(s).is_some(), "{s}");
        }
        assert_eq!(setup("noir").map(|s| s.name), Some("low_key"));
        assert_eq!(mood("tense").and_then(setup).map(|s| s.name), Some("low_key"));
        for s in SETUPS {
            assert!(s.lamps.iter().all(|l| sp::LightKind::parse(l.kind).is_some()), "{}", s.name);
        }
    }
}
