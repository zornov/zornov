// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Cut-outs and roto for agents: the same jobs the Inspector's "Cut out"
//! section runs (`services::cutout`).
//!
//! * `cut_out`: the people in a shot (`subject: "people"`, the default with
//!   nothing picked), or an object picked at `at` with `point` / `points`
//!   (on it), `exclude` (not it) or `box` (two corners), carried through
//!   the clip. `invert` keeps the background instead; `enabled`, `remove`.
//! * `roto`: the same pick, as a path mask keyed every `every` frames
//!   (`detail` points around it), editable point by point afterwards.

use super::{Session, f, micros, secs};
use crate::services::cutout::{self, Pick};
use eframe::egui::Vec2;
use serde_json::Value;
use studio_ai::JobCtx;
use studio_model as m;

fn xy(v: &Value) -> Option<Vec2> {
    match v {
        Value::Array(a) if a.len() == 2 => Some(Vec2::new(a[0].as_f64()? as f32, a[1].as_f64()? as f32)),
        Value::Object(o) => Some(Vec2::new(o.get("x")?.as_f64()? as f32, o.get("y")?.as_f64()? as f32)),
        _ => None,
    }
}

fn points(v: Option<&Value>, what: &str) -> Result<Vec<Vec2>, String> {
    match v {
        None => Ok(Vec::new()),
        Some(Value::Array(a)) if a.len() == 2 && a[0].is_number() => Ok(vec![xy(&Value::Array(a.clone())).ok_or_else(|| format!("{what} is [x, y] or a list of them"))?]),
        Some(Value::Array(a)) => a.iter().map(|p| xy(p).ok_or_else(|| format!("{what} is [x, y] canvas pixels at \"at\", or a list of them"))).collect(),
        Some(_) => Err(format!("{what} is [x, y] canvas pixels at \"at\", or a list of them")),
    }
}

impl Session {
    /// The object an op picks, if it picks one.
    fn pick_of(&self, op: &Value, id: u32, at: crate::view::Time) -> Result<Option<Pick>, String> {
        let mut inside = points(op.get("points").or(op.get("point")), "point")?;
        if op.get("point").and_then(Value::as_str) == Some("face") || op.get("subject").and_then(Value::as_str) == Some("face") {
            let (c, r) = self.face_point(id, at)?;
            inside.push(c);
            let _ = r;
        }
        let outside = points(op.get("exclude"), "exclude")?;
        let corners = match op.get("box") {
            None => None,
            Some(Value::Array(a)) if a.len() == 2 => Some([xy(&a[0]).ok_or("box is two corners [[x0, y0], [x1, y1]]")?, xy(&a[1]).ok_or("box is two corners [[x0, y0], [x1, y1]]")?]),
            Some(Value::Array(a)) if a.len() == 4 && a.iter().all(Value::is_number) => {
                let n: Vec<f32> = a.iter().filter_map(Value::as_f64).map(|v| v as f32).collect();
                Some([Vec2::new(n[0], n[1]), Vec2::new(n[2], n[3])])
            }
            Some(_) => return Err("box is two corners [[x0, y0], [x1, y1]] (canvas pixels at \"at\")".into()),
        };
        let p = Pick { inside, outside, corners };
        Ok((!p.is_empty()).then_some(p))
    }

    pub(super) fn cut_out(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or("cut_out needs \"clip\": the video clip")? as u32;
        let vc = self.clip(id)?;
        let seq = self.doc.seq();
        let mut mc = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
        if op.get("remove").and_then(Value::as_bool) == Some(true) {
            if mc.meta.remove("matte").is_none() {
                return Err(format!("clip {id} has no cut-out"));
            }
            let ok = self.doc.apply("Remove cut-out", m::Edit::SetClip { seq, clip: Box::new(mc) });
            self.check(ok, "")?;
            return Ok(format!("clip {id} shows its whole picture again"));
        }
        let has = cutout::matte_of(&mc).is_some();
        let pick_given = ["point", "points", "box", "exclude", "subject"].iter().any(|k| op.get(*k).is_some());
        if has && !pick_given {
            // Settings of the existing cut-out.
            let mut said = Vec::new();
            for key in ["invert", "enabled"] {
                if let Some(b) = op.get(key).and_then(Value::as_bool) {
                    cutout::set_flag(&mut mc, key, b);
                    said.push(format!("{key} {b}"));
                }
            }
            if said.is_empty() {
                return Err(format!("clip {id} is already cut out ({}); pick again with subject / point / box to redo it, or invert, enabled, remove", cutout::describe(&cutout::matte_of(&mc).unwrap_or_default())));
            }
            let ok = self.doc.apply("Cut-out", m::Edit::SetClip { seq, clip: Box::new(mc.clone()) });
            self.check(ok, "")?;
            return Ok(format!("{}; {}", said.join(", "), cutout::describe(&cutout::matte_of(&mc).unwrap_or_default())));
        }
        let at = f(op, "at").map(micros).unwrap_or(vc.start + vc.dur / 2).clamp(vc.start, vc.start + vc.dur - 1);
        let subject = op.get("subject").and_then(Value::as_str);
        let pick = match subject {
            Some("people" | "person" | "background") => None,
            Some("object" | "face") | None => self.pick_of(op, id, at)?,
            Some(o) => return Err(format!("subject \"{o}\": people (everyone in the shot, the default), object (with point / points / box), face")),
        };
        if subject == Some("object") && pick.is_none() {
            return Err("an object cut-out needs it picked: point [x, y] on it (or points), exclude for what is not it, or box [[x0, y0], [x1, y1]]".into());
        }
        let smoothing = f(op, "smoothing").map(|v| if v > 1.0 { v / 100.0 } else { v }).unwrap_or(0.6) as f32;
        let job = cutout::plan(&self.doc, id, pick.as_ref().map(|p| (p, at)), smoothing, 1)?;
        cutout::models_ready(&job)?;
        let r = job.run_matte(&JobCtx::detached())?;
        let mut mc = self.model_clip(id).ok_or("clip gone")?;
        cutout::store_matte(&mut mc, &job, &r, op.get("invert").and_then(Value::as_bool).unwrap_or(false));
        let said = cutout::describe(&cutout::matte_of(&mc).unwrap_or_default());
        let ok = self.doc.apply("Cut out", m::Edit::SetClip { seq, clip: Box::new(mc) });
        self.check(ok, "")?;
        let mut note = format!("clip {id}: {said}");
        if r.empty * 4 > r.frames {
            note += ". The subject is missing on many frames: check with frame; for an object pick it with more points or a box";
        }
        if r.coverage > 0.9 {
            note += ". Almost everything was kept: the pick may have caught the background (use exclude, or a box)";
        }
        note += ". What is below now shows through: put a background (card, footage, a blur of the same clip) on the lane under it";
        Ok(note)
    }

    pub(super) fn roto(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or("roto needs \"clip\": the video clip")? as u32;
        let vc = self.clip(id)?;
        let at = f(op, "at").map(micros).unwrap_or(vc.start + vc.dur / 2).clamp(vc.start, vc.start + vc.dur - 1);
        let pick = self.pick_of(op, id, at)?.ok_or("roto needs the object picked at \"at\": point [x, y] on it (or points), exclude, or box [[x0, y0], [x1, y1]]")?;
        let fps = (self.doc.project.fps as f64).max(1.0);
        // Keys: every frame for a short shot, fewer for a long one (SAM
        // takes about a second a frame).
        let frames = (vc.dur as f64 / 1e6 * fps).round().max(1.0);
        let every = op.get("every").and_then(Value::as_u64).unwrap_or(if frames <= 90.0 { 1 } else if frames <= 240.0 { 2 } else { 3 }).clamp(1, 30);
        let detail = op.get("detail").and_then(Value::as_u64).unwrap_or(32).clamp(8, 128) as usize;
        let job = cutout::plan(&self.doc, id, Some((&pick, at)), 0.0, every)?;
        cutout::models_ready(&job)?;
        let outlines = job.run_roto(detail, &JobCtx::detached())?;
        // Feather in pixels of the picture → the mask's fraction.
        let short = job.footage.width.min(job.footage.height).max(1) as f64;
        let feather = f(op, "feather").unwrap_or(2.0).max(0.0) * 2.0 / short;
        let (mc, mid) = cutout::store_roto(&self.doc.project, &self.doc.model, id, &job, &outlines, feather, op.get("invert").and_then(Value::as_bool).unwrap_or(false))?;
        let n = mc.masks.len();
        let seq = self.doc.seq();
        let ok = self.doc.apply("Roto", m::Edit::SetClip { seq, clip: Box::new(mc) });
        self.check(ok, "")?;
        Ok(format!(
            "clip {id}: roto mask {n} (id {mid}) with {detail} points, keyed on {} frames (every {every}) from {:.2}s to {:.2}s; look at it with frame, and where it slips move a point with mask_puppet (mask {n}, point, keys)",
            outlines.len(),
            secs(vc.start),
            secs(vc.start + vc.dur)
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Session;
    use crate::services;
    use crate::services::tracking::test_movie::{self, FPS, H, W};
    use serde_json::json;

    fn have(id: studio_ai::ModelId) -> bool {
        let ok = studio_ai::ModelStore::default().require(id).is_ok();
        if !ok {
            eprintln!("{id:?} is not installed: skipped");
        }
        ok
    }

    /// The test shot, trimmed to its first half second (SAM takes a while a
    /// frame).
    fn session() -> (Session, u32) {
        let mut s = Session::new(services::project::new_custom("Cut", W, H, FPS as f32));
        s.run("import", &json!({"paths": [test_movie::movie().to_string_lossy()], "to_timeline": true})).unwrap();
        let id = s.doc.project.clips.iter().find(|c| c.media.is_some()).unwrap().id;
        s.edit(&[json!({"op": "trim", "clip": id, "end": 0.5})], "trim", false).unwrap();
        (s, id)
    }

    fn square_canvas(i: u32) -> [f64; 2] {
        let (ox, oy) = test_movie::object(i);
        let (sx, sy) = test_movie::shake(i);
        [ox + sx - W as f64 / 2.0, oy + sy - H as f64 / 2.0]
    }

    #[test]
    fn an_object_cut_out_keeps_the_square_and_drops_the_wall() {
        if !have(studio_ai::ModelId::Sam21Tiny) {
            return;
        }
        let (mut s, id) = session();
        let c = square_canvas(6);
        let r = s.edit(&[json!({"op": "cut_out", "clip": id, "point": c, "at": 6.0 / 30.0})], "cut", false).unwrap();
        let _ = r;
        let mc = s.model_clip(id).unwrap();
        let o = cutout::matte_of(&mc).expect("a cut-out");
        let cov = o["coverage"].as_f64().unwrap();
        // The square is 56 × 56 of 640 × 360: about 1.4 % of the picture.
        assert!(cov > 0.008 && cov < 0.03, "{o:?}");
        assert!(std::path::Path::new(o["file"].as_str().unwrap()).exists());
        // Drawn: the square shows, the wall around it is gone (black canvas).
        let t = (8.0 / 30.0 * 1e6) as crate::view::Time;
        let img = s.frames.rgba(&s.doc, t, W).unwrap();
        let px = |p: [f64; 2]| {
            let (x, y) = ((p[0] + W as f64 / 2.0) as u32, (p[1] + H as f64 / 2.0) as u32);
            let i = ((y * img.width + x) * 4) as usize;
            img.pixels[i] as u32 + img.pixels[i + 1] as u32 + img.pixels[i + 2] as u32
        };
        let inside = square_canvas(8);
        assert!(px(inside) > 60, "the square: {}", px(inside));
        assert!(px([inside[0] + 120.0, inside[1] + 60.0]) < 6, "the wall: {}", px([inside[0] + 120.0, inside[1] + 60.0]));
        // Inverted: the other way round.
        s.edit(&[json!({"op": "cut_out", "clip": id, "invert": true})], "inv", false).unwrap();
        let img = s.frames.rgba(&s.doc, t, W).unwrap();
        let i = (((inside[1] + H as f64 / 2.0) as u32 * img.width + (inside[0] + W as f64 / 2.0) as u32) * 4) as usize;
        assert!(img.pixels[i] < 4, "the square is gone when inverted");
        // Removed.
        s.edit(&[json!({"op": "cut_out", "clip": id, "remove": true})], "rm", false).unwrap();
        assert!(cutout::matte_of(&s.model_clip(id).unwrap()).is_none());
    }

    #[test]
    fn roto_keys_a_path_mask_round_the_square() {
        if !have(studio_ai::ModelId::Sam21Tiny) {
            return;
        }
        let (mut s, id) = session();
        let c = square_canvas(3);
        s.edit(&[json!({"op": "roto", "clip": id, "point": c, "at": 0.1, "every": 2, "detail": 16})], "roto", false).unwrap();
        let mc = s.model_clip(id).unwrap();
        let mk = mc.masks.last().expect("a roto mask");
        assert_eq!(mk.shape, m::MaskShape::Path);
        assert_eq!(mk.path.len(), 16);
        let track = mc.keyframes.get(&format!("mask.{}.point.0", mk.id.0)).expect("keyed points");
        assert!(track.keys.len() >= 6, "{}", track.keys.len());
        // At frame 10 the outline sits round the square: its points' centre
        // is the square's centre.
        let local = (10.0 / 30.0 * 1e6) as m::Micros;
        let pts: Vec<m::Vec2> = (0..16).map(|k| match mc.property_at(&format!("mask.{}.point.{k}", mk.id.0), local) { Some(m::Value::Point(p)) => p, _ => panic!() }).collect();
        let (cx, cy) = (pts.iter().map(|p| p.x).sum::<f64>() / 16.0 * W as f64, pts.iter().map(|p| p.y).sum::<f64>() / 16.0 * H as f64);
        let want = square_canvas(10);
        assert!((cx - (want[0] + W as f64 / 2.0)).abs() < 4.0 && (cy - (want[1] + H as f64 / 2.0)).abs() < 4.0, "centre {cx},{cy} vs {want:?}");
    }

    #[test]
    fn cut_out_people_runs_and_refusals_say_why() {
        let (mut s, id) = session();
        let e = s.edit(&[json!({"op": "cut_out", "clip": id, "subject": "object"})], "x", false).unwrap_err();
        assert!(e.contains("point"), "{e}");
        let e = s.edit(&[json!({"op": "roto", "clip": id})], "x", false).unwrap_err();
        assert!(e.contains("point"), "{e}");
        let e = s.edit(&[json!({"op": "cut_out", "clip": id, "remove": true})], "x", false).unwrap_err();
        assert!(e.contains("no cut-out"), "{e}");
        if !have(studio_ai::ModelId::Modnet) {
            return;
        }
        // No person in this shot: it runs and says little was kept.
        let r = s.edit(&[json!({"op": "cut_out", "clip": id})], "people", false).unwrap();
        let _ = r;
        let o = cutout::matte_of(&s.model_clip(id).unwrap()).unwrap();
        assert_eq!(o["method"], "people");
        assert_eq!(o["frames"].as_u64(), Some(15));
    }
}
