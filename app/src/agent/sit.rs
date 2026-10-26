// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Characters that sit in their backdrop instead of floating on it. A cut-out
//! laid over a generated picture looks pasted on for three reasons, and
//! each op here takes one away, reading the backdrop (characters left out)
//! for what to do:
//!
//! * `shadow`: a soft contact shadow under the feet and a cast shadow
//!   across the ground, its direction, length and softness from where the
//!   backdrop's light comes from and how hard it is (`meta["shadow"]`,
//!   drawn by the renderer from the layer's own bent picture, so it acts
//!   with the puppet).
//! * `light_match`: the character's brightness and colour moved towards
//!   the light where it stands (a gentle shot match in its grade) and a
//!   rim of the scene's key light on the edges that face it (`rim_light`).
//! * `foreground`: a piece of the backdrop (a rock, a trunk, grass) cut out
//!   with SAM and the depth map and laid over the characters, so they can
//!   stand behind it.

use super::sight::ground::DepthGrid;
use super::{Session, f, micros, secs};
use crate::services::backdrop_fit as fit;
use crate::services::cutout::Pick;
use crate::view::Time;
use eframe::egui::Vec2;
use serde_json::{Value, json};
use studio_media::rgba::RgbaImage;
use studio_model as m;

impl Session {
    /// The clips an op works on: `clip` / `clips`, or every character
    /// showing at `at`.
    fn sit_targets(&self, op: &Value, at: Option<Time>) -> Vec<u32> {
        let mut v: Vec<u32> = op.get("clips").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).map(|x| x as u32).collect()).unwrap_or_default();
        if let Some(c) = op.get("clip").and_then(Value::as_u64) {
            v.push(c as u32);
        }
        match at {
            Some(t) if v.is_empty() => fit::characters_at(&self.doc, t),
            _ => v,
        }
    }

    fn is_piece(&self, id: u32) -> bool {
        self.model_clip(id).is_some_and(|mc| fit::is_piece(&mc))
    }

    /// The moment an op reads the backdrop at: `at`, else the middle of the
    /// clip's time on screen.
    fn sit_moment(&self, op: &Value, id: u32) -> Result<Time, String> {
        let vc = self.clip(id)?;
        Ok(f(op, "at").map(micros).unwrap_or(vc.start + vc.dur / 2).clamp(vc.start, vc.start + vc.dur - 1))
    }

    /// The moment for each of `targets`: `at` where it shows then, else its
    /// own middle.
    fn sit_times(&self, op: &Value, at: Option<Time>, targets: &[u32]) -> Result<Vec<Time>, String> {
        targets
            .iter()
            .map(|id| match at {
                Some(t) if self.clip(*id).is_ok_and(|c| c.start <= t && t < c.end()) => Ok(t),
                _ => self.sit_moment(op, *id),
            })
            .collect()
    }

    /// The backdrop at `at` with the pictures above the main lane left out
    /// (characters, their shadows, titles), small.
    fn backdrop_picture(&mut self, at: Time) -> Result<RgbaImage, String> {
        let leave = fit::above_backdrop(&self.doc, at);
        self.frames.rgba_without(&self.doc, &leave, at, 512)
    }

    /// The backdrop's depth on a grid (`fetch`: download the model if it
    /// is missing).
    pub(super) fn depth_grid(&self, img: &RgbaImage, fetch: bool) -> Result<DepthGrid, String> {
        let store = studio_ai::store::ModelStore::default();
        let id = studio_ai::registry::ModelId::DepthAnythingV2Small;
        if !store.is_installed(id) {
            if !fetch {
                return Err("the depth model is not installed".into());
            }
            store.download(id, &studio_ai::jobs::JobCtx::detached()).map_err(|e| format!("could not download the depth model ({e}); get it in Settings → AI models"))?;
        }
        let est = studio_ai::features::depth::DepthEstimator::load(&store).map_err(|e| format!("the depth model is not ready ({e}): get it in Settings → AI models"))?;
        let frame = studio_ai::frame::Frame::new(img.width as usize, img.height as usize, img.pixels.clone()).map_err(|e| e.to_string())?;
        let depth = est.estimate(&frame).map_err(|e| format!("could not read the depth: {e}"))?;
        Ok(DepthGrid::shrink(depth.width, depth.height, &depth.data, 96))
    }

    /// `shadow`: a ground shadow under characters, set from the backdrop's
    /// light unless given.
    pub(super) fn shadow(&mut self, op: &Value) -> Result<String, String> {
        let first = op.get("clip").and_then(Value::as_u64).map(|c| c as u32).or_else(|| op.get("clips").and_then(Value::as_array).and_then(|a| a.first()).and_then(Value::as_u64).map(|c| c as u32));
        let at = match (f(op, "at"), first) {
            (Some(t), _) => Some(micros(t)),
            (None, Some(id)) => Some(self.sit_moment(op, id)?),
            _ => None,
        };
        let targets = self.sit_targets(op, at);
        if targets.is_empty() {
            return Err("shadow needs \"clip\" / \"clips\" (the characters), or \"at\" for every character showing then".into());
        }
        let seq = self.doc.seq();
        if op.get("remove").and_then(Value::as_bool) == Some(true) {
            let mut edits = Vec::new();
            for id in &targets {
                let mut mc = self.model_clip(*id).ok_or_else(|| format!("no clip {id}"))?;
                if mc.meta.remove("shadow").is_some() {
                    edits.push(m::Edit::SetClip { seq, clip: Box::new(mc) });
                }
            }
            if edits.is_empty() {
                return Err("none of those clips has a shadow".into());
            }
            let n = edits.len();
            let ok = self.doc.apply("Remove shadow", m::Edit::Batch { edits });
            self.check(ok, "")?;
            return Ok(format!("took the shadow off {n} clip(s)"));
        }
        let given = ["angle", "length", "softness", "opacity", "contact", "colour"].iter().any(|k| op.get(*k).is_some());
        let auto = op.get("auto").and_then(Value::as_bool).unwrap_or(!given);
        let times = self.sit_times(op, at, &targets)?;
        let mut read: Option<(Time, fit::Light, f32)> = None;
        let mut edits = Vec::new();
        let mut said = Vec::new();
        for (id, t) in targets.iter().zip(times) {
            let mut mc = self.model_clip(*id).ok_or_else(|| format!("no clip {id}"))?;
            let mut o = mc.meta.get("shadow").and_then(Value::as_object).cloned().unwrap_or_default();
            o.remove("enabled");
            if let Some(v) = op.get("feet") {
                o.insert("feet".into(), v.clone());
            }
            if let Some(g) = op.get("ground") {
                if g.is_null() || g.as_bool() == Some(false) {
                    o.remove("ground");
                } else {
                    o.insert("ground".into(), g.clone());
                }
            }
            mc.meta.insert("shadow".into(), Value::Object(o));
            if auto {
                if read.as_ref().is_none_or(|r| r.0 != t) {
                    let img = self.backdrop_picture(t)?;
                    read = Some((t, fit::read_light(&img), fit::horizon(&img)));
                }
                let (_, light, horizon) = read.as_ref().expect("read");
                said.push(format!("clip {id}: {}", fit::auto_shadow(&self.doc, &mut mc, t, light, *horizon)?));
            } else {
                let o = mc.meta.get_mut("shadow").and_then(Value::as_object_mut).expect("just set");
                if !o.contains_key("feet") {
                    o.insert("feet".into(), json!(fit::feet(&self.doc, *id, t)?));
                }
                said.push(format!("clip {id}: shadow set"));
            }
            let o = mc.meta.get_mut("shadow").and_then(Value::as_object_mut).expect("just set");
            for k in ["angle", "length", "softness", "opacity", "contact"] {
                if let Some(v) = f(op, k) {
                    let v = if matches!(k, "opacity" | "contact") && v > 1.0 { v / 100.0 } else { v };
                    o.insert(k.into(), json!(v));
                }
            }
            if let Some(c) = op.get("colour") {
                let rgb = match c {
                    Value::String(s) => super::craft::colour(s).map(|k| [k.r() as f64 / 255.0, k.g() as f64 / 255.0, k.b() as f64 / 255.0]).ok_or_else(|| format!("shadow colour \"{s}\" is not #rrggbb or a basic name"))?,
                    Value::Array(a) if a.len() >= 3 => [0, 1, 2].map(|i| a[i].as_f64().unwrap_or(0.0)),
                    _ => return Err("shadow colour is \"#rrggbb\", a name, or [r, g, b] 0..1".into()),
                };
                o.insert("colour".into(), json!(rgb));
            }
            if op.get("enabled").and_then(Value::as_bool) == Some(false) {
                o.insert("enabled".into(), json!(false));
            }
            edits.push(m::Edit::SetClip { seq, clip: Box::new(mc) });
        }
        let ok = self.doc.apply("Shadow", m::Edit::Batch { edits });
        self.check(ok, "")?;
        Ok(format!(
            "{}. The shadow is drawn from the character's own bent picture, so it walks and acts with it; frame a moment to look. Change angle / length / softness / opacity / contact by hand if the light reads otherwise; ground: <y> keeps it on a ground line while the character jumps",
            said.join("; ")
        ))
    }

    /// `light_match`: the character lit like the place it stands in.
    pub(super) fn light_match(&mut self, op: &Value) -> Result<String, String> {
        let first = op.get("clip").and_then(Value::as_u64).map(|c| c as u32);
        let at = match (f(op, "at"), first) {
            (Some(t), _) => Some(micros(t)),
            (None, Some(id)) => Some(self.sit_moment(op, id)?),
            _ => None,
        };
        let targets = self.sit_targets(op, at);
        if targets.is_empty() {
            return Err("light_match needs \"clip\" / \"clips\" (the characters), or \"at\" for every character showing then".into());
        }
        let seq = self.doc.seq();
        if op.get("remove").and_then(Value::as_bool) == Some(true) {
            let mut edits = Vec::new();
            for id in &targets {
                let mut mc = self.model_clip(*id).ok_or_else(|| format!("no clip {id}"))?;
                if fit::clear_light_match(&mut mc) {
                    edits.push(m::Edit::SetClip { seq, clip: Box::new(mc) });
                }
            }
            if edits.is_empty() {
                return Err("none of those clips is light-matched".into());
            }
            let n = edits.len();
            let ok = self.doc.apply("Remove light match", m::Edit::Batch { edits });
            self.check(ok, "")?;
            return Ok(format!("took the light match off {n} clip(s)"));
        }
        let amount = f(op, "amount").map(|v| if v > 2.0 { v / 100.0 } else { v }).unwrap_or(1.0).clamp(0.0, 2.0) as f32;
        let rim_amount = match op.get("rim") {
            Some(Value::Bool(false)) => 0.0,
            Some(v) => v.as_f64().map(|v| if v > 2.0 { v / 100.0 } else { v }).unwrap_or(1.0).clamp(0.0, 2.0) as f32,
            None => 1.0,
        };
        let times = self.sit_times(op, at, &targets)?;
        let mut edits = Vec::new();
        let mut said = Vec::new();
        for (id, t) in targets.iter().zip(times) {
            let mut mc = self.model_clip(*id).ok_or_else(|| format!("no clip {id}"))?;
            let img = self.backdrop_picture(t)?;
            let rim_id = self.doc.alloc_id();
            said.push(format!("clip {id}: {}", fit::light_match(&self.doc, &mut mc, t, &img, amount, rim_amount, rim_id)?));
            edits.push(m::Edit::SetClip { seq, clip: Box::new(mc) });
        }
        let ok = self.doc.apply("Light match", m::Edit::Batch { edits });
        self.check(ok, "")?;
        Ok(format!("{}. The match sits first in each clip's grade (it replaces a match_colour there); amount 0-200 % scales it, rim: false leaves the edge light off", said.join("; ")))
    }

    /// `foreground`: a piece of the backdrop over the characters.
    pub(super) fn foreground(&mut self, op: &Value) -> Result<String, String> {
        if op.get("remove").and_then(Value::as_bool) == Some(true) {
            let id = op.get("clip").and_then(Value::as_u64).ok_or("foreground remove needs \"clip\": the piece")? as u32;
            if !self.is_piece(id) {
                return Err(format!("clip {id} is not a foreground piece"));
            }
            let seq = self.doc.seq();
            let ok = self.doc.apply("Remove foreground piece", m::Edit::Lift { seq, clips: vec![m::ClipId(id as u64)] });
            self.check(ok, "")?;
            return Ok(format!("removed foreground piece {id}"));
        }
        let p = self.doc.project.clone();
        let spine = p.spine().ok_or("there is no main lane")?;
        let at = match f(op, "at") {
            Some(t) => micros(t),
            None => {
                let c = op.get("hides").and_then(Value::as_array).and_then(|a| a.first()).and_then(Value::as_u64).ok_or("foreground needs \"at\": the moment to cut the backdrop at (seconds)")?;
                self.sit_moment(op, c as u32)?
            }
        };
        let backdrop = match op.get("backdrop").and_then(Value::as_u64) {
            Some(b) => b as u32,
            None => p.clips.iter().find(|c| c.lane == spine && c.start <= at && at < c.end()).map(|c| c.id).ok_or("nothing is on the main lane then: say which clip is the backdrop")?,
        };
        let (w, h) = (p.width as f32, p.height as f32);
        let to_canvas = |q: [f32; 2]| Vec2::new((q[0] - 0.5) * w, (q[1] - 0.5) * h);
        // What to cut: a piece `see place` listed, or a point / box.
        let mut inside: Vec<Vec2> = points(op.get("points").or(op.get("point")))?;
        let mut corners = None;
        let mut behind: Option<[f32; 2]> = None;
        let mut kind = "piece";
        let img = self.backdrop_picture(at)?;
        if let Some(n) = op.get("piece").and_then(Value::as_u64) {
            let g = self.depth_grid(&img, true)?;
            let pieces = g.pieces();
            let pc = pieces.get((n as usize).wrapping_sub(1)).ok_or_else(|| format!("piece {n}: see place listed {} piece(s) at {:.2}s (numbered from 1)", pieces.len(), secs(at)))?;
            inside.push(to_canvas(pc.inside));
            corners = Some([to_canvas([pc.bbox[0], pc.bbox[1]]), to_canvas([pc.bbox[2], pc.bbox[3]])]);
            behind = Some(pc.land);
            kind = pc.short();
        }
        if let Some(b) = op.get("box").and_then(Value::as_array).filter(|a| a.len() == 2) {
            let pt = |v: &Value| -> Option<Vec2> { Some(Vec2::new(v.get(0)?.as_f64()? as f32, v.get(1)?.as_f64()? as f32)) };
            corners = Some([pt(&b[0]).ok_or("box is [[x0, y0], [x1, y1]]")?, pt(&b[1]).ok_or("box is [[x0, y0], [x1, y1]]")?]);
        }
        if inside.is_empty() && corners.is_none() {
            return Err("foreground needs what to cut: piece <n> (from see with place), or point [x, y] on it, or box [[x0, y0], [x1, y1]] (canvas px at \"at\")".into());
        }
        if inside.is_empty()
            && let Some([a, b]) = corners
        {
            inside.push((a + b) * 0.5);
        }
        let hides = op.get("hides").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).map(|v| v as u32).collect());
        let cut = fit::PieceCut { backdrop, at, pick: Pick { inside, outside: points(op.get("exclude"))?, corners }, behind, kind: kind.to_string(), hides };
        let (_, note) = fit::lay_piece(&mut self.doc, &cut)?;
        Ok(note)
    }
}

fn xy(v: &Value) -> Option<Vec2> {
    match v {
        Value::Array(a) if a.len() == 2 => Some(Vec2::new(a[0].as_f64()? as f32, a[1].as_f64()? as f32)),
        Value::Object(o) => Some(Vec2::new(o.get("x")?.as_f64()? as f32, o.get("y")?.as_f64()? as f32)),
        _ => None,
    }
}

/// `[x, y]` or a list of them (canvas px from the centre).
fn points(v: Option<&Value>) -> Result<Vec<Vec2>, String> {
    match v {
        None => Ok(Vec::new()),
        Some(Value::Array(a)) if a.len() == 2 && a[0].is_number() => Ok(vec![xy(&Value::Array(a.clone())).ok_or("a point is [x, y]")?]),
        Some(Value::Array(a)) => a.iter().map(|q| xy(q).ok_or_else(|| "points are [x, y] canvas pixels, or a list of them".to_string())).collect(),
        Some(_) => Err("points are [x, y] canvas pixels, or a list of them".into()),
    }
}

#[cfg(test)]
mod tests {
    use crate::agent::Session;
    use crate::services;
    use crate::services::backdrop_fit as fit;
    use serde_json::json;
    use studio_model as m;

    fn png(path: &std::path::Path, w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) {
        let f = &f;
        let pixels = (0..h).flat_map(|y| (0..w).flat_map(move |x| f(x, y))).collect::<Vec<u8>>();
        std::fs::write(path, studio_media::png::encode(&studio_media::rgba::RgbaImage { width: w, height: h, pixels })).unwrap();
    }

    /// A sunny place (sky with the sun at the upper right over sandy
    /// ground, a grey rock in the middle distance) and a character (a pale
    /// upright body on two feet) standing on its ground. `dark` makes the
    /// place a dim, blue dusk instead.
    fn scene(name: &str, dark: bool) -> (Session, u32, u32, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("studio-sit-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (bg, fig) = (dir.join("place.png"), dir.join("figure.png"));
        png(&bg, 640, 360, |x, y| {
            let (u, v) = (x as f32 / 640.0, y as f32 / 360.0);
            let sun = ((u - 0.82).powi(2) + ((v - 0.12) * 0.56).powi(2)).sqrt() < 0.05;
            let rock = ((u - 0.5) / 0.09).powi(2) + ((v - 0.62) / 0.08).powi(2) < 1.0;
            let c: [f32; 3] = if sun {
                [255.0, 248.0, 225.0]
            } else if v < 0.42 {
                [120.0, 165.0, 215.0]
            } else if rock {
                [110.0, 108.0, 104.0]
            } else {
                [185.0 + 30.0 * u, 155.0 + 20.0 * u, 110.0]
            };
            let c = if dark { [c[0] * 0.3, c[1] * 0.35, c[2] * 0.55] } else { c };
            [c[0] as u8, c[1] as u8, c[2] as u8, 255]
        });
        png(&fig, 80, 160, |x, y| {
            let body = ((x as f32 - 40.0) / 30.0).powi(2) + ((y as f32 - 80.0) / 70.0).powi(2) < 1.0;
            let feet = (148..158).contains(&y) && ((18..32).contains(&x) || (48..62).contains(&x));
            if body || feet { [235, 225, 210, 255] } else { [0, 0, 0, 0] }
        });
        let mut s = Session::new(services::project::new_custom("Sit", 640, 360, 30.0));
        s.run("import", &json!({"paths": [bg.to_string_lossy(), fig.to_string_lossy()]})).unwrap();
        s.edit(&[json!({"op": "place", "media": "place", "at": 0.0, "duration": 2.0}), json!({"op": "place", "media": "figure", "at": 0.0, "duration": 2.0, "lane": "overlay"})], "place", false).unwrap();
        let id_of = |n: &str| s.doc.project.clips.iter().find(|c| c.name.contains(n)).unwrap().id;
        let (b, c) = (id_of("place"), id_of("figure"));
        // 144 px tall (the picture is fitted to the frame's height first),
        // feet at 298 of 360, on the left.
        s.edit(&[json!({"op": "set", "clip": c, "x": -120, "y": 46, "scale": 40})], "stand", false).unwrap();
        (s, b, c, dir)
    }

    fn luma(img: &studio_media::rgba::RgbaImage, x0: u32, y0: u32, x1: u32, y1: u32) -> f32 {
        let mut s = 0.0;
        for y in y0..y1 {
            for x in x0..x1 {
                let i = ((y * img.width + x) * 4) as usize;
                s += 0.2126 * img.pixels[i] as f32 + 0.7152 * img.pixels[i + 1] as f32 + 0.0722 * img.pixels[i + 2] as f32;
            }
        }
        s / ((x1 - x0) * (y1 - y0)) as f32
    }

    #[test]
    fn a_character_gets_a_shadow_from_the_backdrops_light() {
        let (mut s, _, c, dir) = scene("shadow", false);
        let r = s.edit(&[json!({"op": "shadow", "clip": c})], "shadow", false).unwrap();
        let mc = s.model_clip(c).unwrap();
        let o = mc.meta["shadow"].as_object().unwrap().clone();
        // The sun shows at the upper right: the scene is lit from behind,
        // so the shadow comes towards the camera and to the left.
        let angle = o["angle"].as_f64().unwrap();
        assert!((90.0..180.0).contains(&angle), "{angle}: {r:?}");
        let feet = o["feet"].as_array().unwrap();
        assert!((feet[1].as_f64().unwrap() - 0.99).abs() < 0.02, "{feet:?}");
        // Drawn: the ground right under and in front of the feet darkens.
        let t = 500_000;
        let with = s.frames.rgba(&s.doc, t, 640).unwrap();
        s.edit(&[json!({"op": "shadow", "clip": c, "enabled": false})], "off", false).unwrap();
        let without = s.frames.rgba(&s.doc, t, 640).unwrap();
        let (a, b) = (luma(&with, 175, 293, 225, 304), luma(&without, 175, 293, 225, 304));
        assert!(a < b - 12.0, "under the feet: {a} with the shadow, {b} without");
        // Settings by hand are kept and the feet stay where they were read.
        s.edit(&[json!({"op": "shadow", "clip": c, "angle": 300, "opacity": 30})], "hand", false).unwrap();
        let o = s.model_clip(c).unwrap().meta["shadow"].clone();
        assert_eq!((o["angle"].as_f64(), o["opacity"].as_f64()), (Some(300.0), Some(0.3)));
        s.edit(&[json!({"op": "shadow", "clip": c, "remove": true})], "rm", false).unwrap();
        assert!(!s.model_clip(c).unwrap().meta.contains_key("shadow"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_character_in_a_dim_place_is_lit_like_it() {
        let (mut s, _, c, dir) = scene("light", true);
        let t = 500_000;
        let before = s.frames.rgba(&s.doc, t, 640).unwrap();
        let r = s.edit(&[json!({"op": "light_match", "clip": c})], "light", false).unwrap();
        let after = s.frames.rgba(&s.doc, t, 640).unwrap();
        // The pale body darkens in the dusk.
        let (a, b) = (luma(&after, 190, 200, 210, 260), luma(&before, 190, 200, 210, 260));
        assert!(a < b - 15.0, "{a} after, {b} before: {r:?}");
        let mc = s.model_clip(c).unwrap();
        assert!(mc.meta.contains_key("light_match"));
        assert!(services::colour_match::strength(&mc).is_some());
        s.edit(&[json!({"op": "light_match", "clip": c, "remove": true})], "rm", false).unwrap();
        let mc = s.model_clip(c).unwrap();
        assert!(!mc.meta.contains_key("light_match") && services::colour_match::strength(&mc).is_none());
        assert!(mc.effects.iter().all(|e| e.kind != "rim_light"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_sun_behind_puts_a_rim_on_the_edges_facing_it() {
        let (mut s, _, c, dir) = scene("rim", false);
        s.edit(&[json!({"op": "light_match", "clip": c})], "light", false).unwrap();
        let mc = s.model_clip(c).unwrap();
        let rim = mc.effects.iter().find(|e| e.kind == "rim_light").expect("a rim light");
        // The sun is up and to the right of the character.
        let a = rim.params.get("angle").and_then(m::Value::as_f64).unwrap();
        assert!((270.0..360.0).contains(&a), "{a}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_rock_cut_from_the_backdrop_hides_a_character_behind_it() {
        let store = studio_ai::ModelStore::default();
        if [studio_ai::ModelId::Sam21Tiny, studio_ai::ModelId::DepthAnythingV2Small].iter().any(|id| store.require(*id).is_err()) {
            eprintln!("SAM or the depth model is not installed: skipped");
            return;
        }
        let (mut s, b, c, dir) = scene("piece", false);
        // The character walks behind the rock: feet up the frame from its
        // base, a little smaller.
        s.edit(&[json!({"op": "set", "clip": c, "x": 0, "y": 0, "scale": 30})], "behind", false).unwrap();
        let t = 500_000;
        let r = s.edit(&[json!({"op": "foreground", "at": 0.5, "point": [0, 45], "box": [[-60, 20], [60, 75]], "hides": [c]})], "piece", false).unwrap();
        let piece = s.doc.project.clips.iter().find(|k| s.model_clip(k.id).is_some_and(|mc| fit::is_piece(&mc))).map(|k| k.id).unwrap_or_else(|| panic!("no piece: {r:?}"));
        let lanes = &s.doc.project.lanes;
        let at = |id: u32| lanes.iter().position(|l| Some(l.id) == s.doc.project.clip(id).map(|k| k.lane)).unwrap();
        assert!(at(piece) < at(c), "the piece's lane is over the character's");
        assert!(at(c) < at(b));
        // Where the rock is in front of the character, the rock shows.
        let img = s.frames.rgba(&s.doc, t, 640).unwrap();
        let rock = luma(&img, 312, 215, 328, 230);
        assert!((rock - 108.0).abs() < 20.0, "rock grey over the body: {rock}");
        // Above the rock, the character shows.
        assert!(luma(&img, 312, 150, 328, 175) > 190.0);
        s.edit(&[json!({"op": "foreground", "clip": piece, "remove": true})], "rm", false).unwrap();
        assert!(s.doc.project.clip(piece).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}
