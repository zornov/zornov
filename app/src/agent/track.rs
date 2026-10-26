// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Motion tracking and stabilizing for agents: the same tracker, tracks and
//! followers the canvas Track tool uses (`services::tracking`).
//!
//! * `track`: `point` ([x, y] canvas pixels at `at`, or `"face"`) or
//!   `plane` (four corners) starts a track on a video clip; `attach` (a
//!   clip) with `follow` (position, transform, corners, mask) makes another
//!   layer follow it; `track` + `fix` puts it right by hand at `at` and
//!   re-tracks from there (`retrack: true` re-tracks without moving it);
//!   `detach`, `remove`, `name`.
//! * `stabilize`: `mode` smooth (with `smoothness` seconds) or lock;
//!   `rotation`, `scale`, `max_zoom` (1 = never zoom); `track` locks onto a
//!   tracked subject instead of the whole picture; `remove` undoes it.

use super::{Session, f, micros, secs};
use crate::services::tracking::{self as tracking, FollowMode, Kind};
use crate::view;
use eframe::egui::Vec2;
use serde_json::{Value, json};
use studio_ai::JobCtx;
use studio_ai::features::track::{Motion, StabilizeSettings};
use studio_model as m;

fn xy(v: &Value) -> Option<Vec2> {
    match v {
        Value::Array(a) if a.len() == 2 => Some(Vec2::new(a[0].as_f64()? as f32, a[1].as_f64()? as f32)),
        Value::Object(o) => Some(Vec2::new(o.get("x")?.as_f64()? as f32, o.get("y")?.as_f64()? as f32)),
        _ => None,
    }
}

fn r1(v: f32) -> f64 {
    (v as f64 * 10.0).round() / 10.0
}

impl Session {
    /// The largest face at `t` on clip `id`, in canvas pixels, and its size.
    pub(super) fn face_point(&self, id: u32, t: view::Time) -> Result<(Vec2, f32), String> {
        let mc = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
        let ft = tracking::footage(&self.doc.project, &mc)?;
        let mut src = studio_render::source::open_path(&ft.path)?;
        let fr = tracking::frame_at(&mc, ft.fps, t).round().max(0.0) as u64;
        let mut got = None;
        src.decode_to(fr, &mut |i, pic| {
            if i == fr {
                got = Some(super::analyse::small_rgba(&pic, 640));
            }
        });
        let frame = got.ok_or("that frame could not be decoded")?;
        let store = studio_ai::store::ModelStore::default();
        let det = studio_ai::features::faces::FaceDetector::load(&store).map_err(|e| e.to_string())?;
        let faces = det.detect(&frame).map_err(|e| e.to_string())?;
        let best = faces.iter().max_by(|a, b| a.bbox.area().total_cmp(&b.bbox.area())).ok_or_else(|| format!("no face in clip {id} at {:.2}s; give the point as [x, y]", secs(t)))?;
        let geo = crate::editor::xform::geo(&self.doc.project, &self.doc.model, id, t).ok_or("the clip draws nothing there")?;
        let k = geo.source().x / frame.width as f32;
        let (cx, cy) = best.bbox.centre();
        let c = geo.to_canvas(Vec2::new(cx * k, cy * k));
        let edge = geo.to_canvas(Vec2::new((cx + best.bbox.w / 2.0) * k, cy * k));
        Ok((c, (edge - c).length().max(8.0)))
    }

    fn track_line(&self, id: u32, tr: &tracking::Track) -> Value {
        let mc = self.model_clip(id);
        let weak: Vec<Value> = tracking::weak_spots(tr, 4)
            .iter()
            .filter_map(|(fr, c)| {
                let t = tracking::time_of_frame(&self.doc.project, mc.as_ref()?, tr.fps, *fr)?;
                Some(json!({"at": secs(t), "confidence": (c * 100.0).round()}))
            })
            .collect();
        let mut o = json!({"track": tr.id, "name": tr.name, "kind": if tr.kind == Kind::Point { "point" } else { "plane" }, "said": tracking::describe(tr)});
        if !weak.is_empty() {
            o["weak_at"] = json!(weak);
        }
        // Where it is at a few moments, so the result can be checked.
        if let Ok(vc) = self.clip(id) {
            let mut path = Vec::new();
            for k in 0..5 {
                let t = vc.start + vc.dur * k / 4 - if k == 4 { 1 } else { 0 };
                if let Some((p, c, lost)) = tracking::canvas_points(&self.doc.project, &self.doc.model, id, tr, t) {
                    let pts: Vec<[f64; 2]> = if tr.kind == Kind::Point { vec![[r1(p[0].x), r1(p[0].y)]] } else { p.iter().map(|q| [r1(q.x), r1(q.y)]).collect() };
                    path.push(json!({"at": secs(t), "points": pts, "confidence": (c * 100.0).round(), "lost": lost}));
                }
            }
            o["path"] = json!(path);
        }
        o
    }

    pub(super) fn track(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or("track needs \"clip\": the video clip to track (or, with detach, the follower)")? as u32;
        let vc = self.clip(id)?;
        let at = f(op, "at").map(micros).unwrap_or(vc.start + vc.dur / 2).clamp(vc.start, vc.start + vc.dur - 1);
        let seq = self.doc.seq();
        let mut said = Vec::new();

        if op.get("detach").and_then(Value::as_bool) == Some(true) {
            let mut mc = self.model_clip(id).ok_or("clip gone")?;
            let clear = op.get("clear").and_then(Value::as_bool).unwrap_or(false);
            if !tracking::detach(&mut mc, clear) {
                return Err(format!("clip {id} follows no track"));
            }
            let ok = self.doc.apply("Stop following", m::Edit::SetClip { seq, clip: Box::new(mc) });
            self.check(ok, "")?;
            return Ok(format!("clip {id} no longer follows its track{}", if clear { "; its follow keys are gone" } else { " (its keys stay)" }));
        }

        let mut track_id = op.get("track").and_then(Value::as_u64).map(|t| t as u32);
        let motion = match op.get("motion").and_then(Value::as_str) {
            None => None,
            Some("translation" | "move" | "position") => Some(Motion::Translation),
            Some("similarity" | "transform" | "rotation" | "scale") => Some(Motion::Similarity),
            Some("perspective") => Some(Motion::Perspective),
            Some(o) => return Err(format!("motion \"{o}\": translation (moves only), similarity (moves, turns, scales) or perspective (planes)")),
        };
        let name = op.get("name").and_then(Value::as_str).unwrap_or("").to_owned();

        if op.get("remove").and_then(Value::as_bool) == Some(true) {
            let tid = track_id.ok_or("remove needs \"track\": which one")?;
            let changed = tracking::remove_track(&self.doc.model, id, tid);
            if changed.is_empty() {
                return Err(format!("clip {id} has no track {tid}"));
            }
            let ok = self.doc.apply("Remove track", m::Edit::Batch { edits: changed.into_iter().map(|c| m::Edit::SetClip { seq, clip: Box::new(c) }).collect() });
            self.check(ok, "")?;
            return Ok(format!("removed track {tid} of clip {id} (layers that followed it keep their keys)"));
        }

        // A new track.
        let point = op.get("point");
        let plane = op.get("plane").or(op.get("corners"));
        if track_id.is_none() && (point.is_some() || plane.is_some()) {
            let (kind, pts, radius) = if let Some(p) = point {
                if p.as_str() == Some("face") {
                    let (c, r) = self.face_point(id, at)?;
                    said.push(format!("found a face at [{}, {}]", r1(c.x), r1(c.y)));
                    (Kind::Point, vec![c], f(op, "radius").map(|r| r as f32).unwrap_or(r))
                } else {
                    let c = xy(p).ok_or("point is [x, y] canvas pixels from the centre at \"at\", or \"face\"")?;
                    (Kind::Point, vec![c], f(op, "radius").unwrap_or(40.0) as f32)
                }
            } else {
                let Some(Value::Array(a)) = plane else { return Err("plane is four corners [[x, y], …] in order around the surface (top-left, top-right, bottom-right, bottom-left)".into()) };
                let pts: Vec<Vec2> = a.iter().filter_map(xy).collect();
                if pts.len() != 4 {
                    return Err("plane needs exactly four corners [[x, y], …]".into());
                }
                (Kind::Plane, pts, 0.0)
            };
            let job = tracking::plan_new(&self.doc.project, &self.doc.model, id, kind, at, &pts, radius, motion, &name)?;
            let samples = job.run(&JobCtx::detached())?;
            let mut mc = self.model_clip(id).ok_or("clip gone")?;
            let tid = tracking::store(&mut mc, &job, &samples, false);
            let ok = self.doc.apply("Track", m::Edit::SetClip { seq, clip: Box::new(mc) });
            self.check(ok, "")?;
            track_id = Some(tid);
            said.push(format!("tracked {} frames", samples.len()));
        } else if let Some(fix) = op.get("fix") {
            let tid = track_id.ok_or("fix needs \"track\": which one")?;
            let pts: Vec<Vec2> = match fix {
                Value::Array(a) if a.len() == 4 && a.iter().all(|v| v.is_array()) => a.iter().filter_map(xy).collect(),
                v => vec![xy(v).ok_or("fix is [x, y] (a point track) or four corners (a plane), canvas pixels at \"at\"")?],
            };
            let job = tracking::plan_fix(&self.doc.project, &self.doc.model, id, tid, at, Some(&pts), op.get("both_ways").and_then(Value::as_bool).unwrap_or(false))?;
            let samples = job.run(&JobCtx::detached())?;
            let mut mc = self.model_clip(id).ok_or("clip gone")?;
            tracking::store(&mut mc, &job, &samples, true);
            let mut edits = vec![m::Edit::SetClip { seq, clip: Box::new(mc.clone()) }];
            // Followers move with the corrected track.
            let mut model = self.doc.model.clone();
            if let Some(s) = model.sequences.iter_mut().find(|s| s.id == seq)
                && let Some(c) = s.clips.iter_mut().find(|c| c.id == mc.id)
            {
                *c = mc;
            }
            let followers = tracking::refreshed_followers(&self.doc.project, &model, id);
            let n = followers.len();
            edits.extend(followers.into_iter().map(|c| m::Edit::SetClip { seq, clip: Box::new(c) }));
            let ok = self.doc.apply("Correct track", m::Edit::Batch { edits });
            self.check(ok, "")?;
            said.push(format!("corrected at {:.2}s and re-tracked {} frames{}", secs(at), samples.len(), if n > 0 { format!("; {n} follower(s) updated") } else { String::new() }));
        } else if op.get("retrack").and_then(Value::as_bool) == Some(true) {
            let tid = track_id.ok_or("retrack needs \"track\"")?;
            let job = tracking::plan_fix(&self.doc.project, &self.doc.model, id, tid, at, None, false)?;
            let samples = job.run(&JobCtx::detached())?;
            let mut mc = self.model_clip(id).ok_or("clip gone")?;
            tracking::store(&mut mc, &job, &samples, false);
            let ok = self.doc.apply("Re-track", m::Edit::SetClip { seq, clip: Box::new(mc) });
            self.check(ok, "")?;
            said.push(format!("re-tracked {} frames from {:.2}s", samples.len(), secs(at)));
        }

        if let (Some(tid), Some(n)) = (track_id, op.get("name").and_then(Value::as_str)) {
            if said.is_empty() {
                let mut mc = self.model_clip(id).ok_or("clip gone")?;
                let mut list = tracking::tracks(&mc);
                let t = list.iter_mut().find(|t| t.id == tid).ok_or_else(|| format!("clip {id} has no track {tid}"))?;
                t.name = n.to_owned();
                tracking::set_tracks(&mut mc, &list);
                let ok = self.doc.apply("Rename track", m::Edit::SetClip { seq, clip: Box::new(mc) });
                self.check(ok, "")?;
                said.push(format!("named it \"{n}\""));
            }
        }

        // A follower.
        if let Some(a) = op.get("attach") {
            let follower = a.as_u64().ok_or("attach is the clip that follows (an id, \"$1\", a name…)")? as u32;
            let tid = match track_id {
                Some(t) => t,
                None => {
                    let list = tracking::tracks(&self.model_clip(id).ok_or("clip gone")?);
                    match list.as_slice() {
                        [one] => one.id,
                        [] => return Err(format!("clip {id} has no track yet: start one with \"point\" or \"plane\"")),
                        _ => return Err(format!("clip {id} has {} tracks; say which with \"track\"", list.len())),
                    }
                }
            };
            let tr = tracking::track_of(&self.model_clip(id).ok_or("clip gone")?, tid).ok_or_else(|| format!("clip {id} has no track {tid}"))?;
            let default = if tr.kind == Kind::Plane { "corners" } else { "position" };
            let mask = match op.get("mask") {
                Some(v) => {
                    let fc = self.model_clip(follower).ok_or_else(|| format!("no clip {follower}"))?;
                    let n = v.as_u64().ok_or("mask is 1, 2… (which of the follower's masks)")? as usize;
                    Some(fc.masks.get(n.max(1) - 1).ok_or_else(|| format!("clip {follower} has {} mask(s)", fc.masks.len()))?.id.0)
                }
                None => None,
            };
            let mode_name = op.get("follow").and_then(Value::as_str).unwrap_or(if mask.is_some() { "mask" } else { default });
            let mode = FollowMode::parse(mode_name, mask.or_else(|| self.model_clip(follower).and_then(|c| c.masks.first().map(|k| k.id.0))))?;
            let fc = tracking::attach(&self.doc.project, &self.doc.model, follower, id, tid, mode.clone(), at)?;
            let ok = self.doc.apply("Follow track", m::Edit::SetClip { seq, clip: Box::new(fc) });
            self.check(ok, "")?;
            said.push(format!("clip {follower} follows track {tid} ({}) from {:.2}s, a key every frame they share", mode.name(), secs(at)));
        }

        if said.is_empty() {
            return Err("track does nothing without point / plane (start one), track + fix / retrack (correct), attach (a layer follows), detach, remove or name".into());
        }
        let mc = self.model_clip(id).ok_or("clip gone")?;
        let lines: Vec<Value> = match track_id {
            Some(t) => tracking::track_of(&mc, t).iter().map(|tr| self.track_line(id, tr)).collect(),
            None => tracking::tracks(&mc).iter().map(|tr| self.track_line(id, tr)).collect(),
        };
        Ok(format!("{}; {}", said.join("; "), json!(lines)))
    }

    pub(super) fn stabilize(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or("stabilize needs \"clip\"")? as u32;
        let seq = self.doc.seq();
        let mc = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
        if op.get("remove").and_then(Value::as_bool) == Some(true) {
            let c = tracking::unstabilize(&mc).ok_or_else(|| format!("clip {id} is not stabilized"))?;
            let ok = self.doc.apply("Remove stabilize", m::Edit::SetClip { seq, clip: Box::new(c) });
            self.check(ok, "")?;
            return Ok(format!("clip {id} is back to how it was shot"));
        }
        let lock = match op.get("mode").and_then(Value::as_str) {
            None | Some("smooth") => false,
            Some("lock" | "tripod" | "locked") => true,
            Some(o) => return Err(format!("mode \"{o}\": smooth (keeps the camera's move, takes the shake out) or lock (no camera motion at all, like a tripod)")),
        };
        let mut s = StabilizeSettings { lock, ..StabilizeSettings::default() };
        if let Some(v) = f(op, "smoothness") {
            s.smoothness = v.clamp(0.05, 10.0);
        }
        if let Some(b) = op.get("rotation").and_then(Value::as_bool) {
            s.rotation = b;
        }
        if let Some(b) = op.get("scale").and_then(Value::as_bool) {
            s.scale = b;
        }
        match op.get("max_zoom") {
            Some(Value::Bool(false)) => s.max_zoom = 1.0,
            Some(v) => {
                let z = v.as_f64().ok_or("max_zoom is a factor (1.15 = up to 15% bigger) or false (never zoom; the edges may show)")?;
                s.max_zoom = if z > 3.0 { 1.0 + z / 100.0 } else { z.max(1.0) };
            }
            None => {}
        }
        let track = op.get("track").and_then(Value::as_u64).map(|t| t as u32);
        let job = tracking::plan_stabilize(&self.doc.project, &self.doc.model, id, s, track)?;
        let (path, agree) = job.run(&JobCtx::detached())?;
        if track.is_none() && agree < 0.15 {
            return Err(format!("the picture's motion could not be measured (only {:.0}% of it agrees: too dark, blurred or plain); track a detailed point and stabilize on it with \"track\"", agree * 100.0));
        }
        let (c, info) = tracking::apply_stabilize(&self.doc.project, &self.doc.model, &job, &path)?;
        let ok = self.doc.apply("Stabilize", m::Edit::SetClip { seq, clip: Box::new(c) });
        self.check(ok, "")?;
        let (w, h) = tracking::analysis_size(job.footage.width, job.footage.height);
        let plan = studio_ai::features::track::plan_stabilize(&path, w as f64, h as f64, job.footage.fps, &s);
        let (before, after) = shake(&path, &plan, w as f64, h as f64, job.footage.fps, job.footage.width as f64 / w as f64);
        let mut note = format!(
            "clip {id} {} ({:.0}% of the picture agreed on the motion): shake {:.1} px before, {:.1} px after (frame-to-frame jitter in source pixels); zoom {:.0}%",
            if lock { "locked off like a tripod" } else { "smoothed" },
            agree * 100.0,
            before,
            after,
            info.zoom * 100.0
        );
        if info.strength < 0.999 {
            note += &format!("; the correction was eased to {:.0}% to stay within max_zoom {:.2} (raise max_zoom for more, or smooth instead of lock)", info.strength * 100.0, s.max_zoom);
        }
        if s.max_zoom <= 1.0 {
            note += "; no zoom: moving black edges may show (check with frame)";
        }
        Ok(note)
    }
}

/// Frame-to-frame jitter (high-frequency motion) of the centre, before and
/// after the correction, RMS in source pixels.
fn shake(path: &[studio_ai::features::track::Mat3], plan: &studio_ai::features::track::StabilizePlan, w: f64, h: f64, _fps: f64, k: f64) -> (f64, f64) {
    use studio_ai::features::track as tk;
    let c = (w / 2.0, h / 2.0);
    let mut acc = tk::IDENTITY;
    let mut pos = vec![(0.0, 0.0)];
    for m in path {
        acc = tk::mul(m, &acc);
        let q = tk::apply(&acc, c);
        pos.push((q.0 - c.0, q.1 - c.1));
    }
    let after: Vec<(f64, f64)> = pos.iter().zip(&plan.corrections).map(|(p, q)| (p.0 + q.x, p.1 + q.y)).collect();
    let jitter = |p: &[(f64, f64)]| {
        if p.len() < 3 {
            return 0.0;
        }
        // Second difference: what is left once steady motion is taken out.
        let mut s = 0.0;
        for i in 1..p.len() - 1 {
            let (ax, ay) = (p[i + 1].0 - 2.0 * p[i].0 + p[i - 1].0, p[i + 1].1 - 2.0 * p[i].1 + p[i - 1].1);
            s += ax * ax + ay * ay;
        }
        (s / (p.len() - 2) as f64).sqrt() * k
    };
    ((jitter(&pos) * 10.0).round() / 10.0, (jitter(&after) * 10.0).round() / 10.0)
}

impl Session {
    /// What `inspect` says about a clip's tracks, what it follows and its
    /// stabilizing.
    pub(super) fn tracking_said(&self, id: u32) -> Option<Value> {
        let mc = self.model_clip(id)?;
        let mut o = serde_json::Map::new();
        let list = tracking::tracks(&mc);
        if !list.is_empty() {
            o.insert("tracks".into(), json!(list.iter().map(|t| json!({"track": t.id, "name": t.name, "said": tracking::describe(t)})).collect::<Vec<_>>()));
        }
        if let Some(fl) = tracking::follow_of(&mc) {
            o.insert("follows".into(), json!({"clip": fl.clip, "track": fl.track, "follow": fl.mode.name()}));
        }
        if let Some(c) = crate::services::cutout::matte_of(&mc) {
            o.insert("cut_out".into(), json!(crate::services::cutout::describe(&c)));
        }
        if mc.masks.iter().any(|k| k.extra.get("roto").is_some()) {
            o.insert("roto_masks".into(), json!(mc.masks.iter().enumerate().filter(|(_, k)| k.extra.get("roto").is_some()).map(|(i, _)| i + 1).collect::<Vec<_>>()));
        }
        if let m::Speed::Curve(tr) = &mc.speed {
            // A speed ramp is not in `keyframes` (it is the clip's speed):
            // its keys, and the footage it uses.
            o.insert("speed_ramp".into(), json!({
                "keys": tr.keys.iter().map(|k| json!([(k.time as f64 / 1e4).round() / 100.0, k.value.as_f64().map(|v| (v * 1000.0).round() / 1000.0)])).collect::<Vec<_>>(),
                "source_from": (mc.source_in as f64 / 1e4).round() / 100.0,
                "source_to": (mc.source_out() as f64 / 1e4).round() / 100.0,
            }));
        }
        if let Some(d) = crate::services::slowmo::describe(&mc) {
            o.insert("smooth_motion".into(), json!(d));
        }
        if let Some(s) = tracking::stabilized(&mc) {
            o.insert("stabilized".into(), json!({"mode": if s.lock { "lock" } else { "smooth" }, "smoothness": s.smoothness, "zoom": s.zoom, "strength": s.strength}));
        }
        (!o.is_empty()).then_some(Value::Object(o))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Session;
    use crate::services;
    use crate::view::SEC;

    use crate::services::tracking::test_movie::{movie, object, shake, FPS, H, N, W};

    /// A 640×360 project with the shaky shot on the main lane (filling the
    /// canvas: one canvas pixel per source pixel, centre at 320, 180).
    fn session() -> (Session, u32) {
        let mut s = Session::new(services::project::new_custom("Track", W, H, FPS as f32));
        s.run("import", &json!({"paths": [movie().to_string_lossy()], "to_timeline": true})).unwrap();
        let id = s.doc.project.clips.iter().find(|c| c.media.is_some()).unwrap().id;
        (s, id)
    }

    fn canvas_of(p: (f64, f64)) -> (f64, f64) {
        (p.0 - W as f64 / 2.0, p.1 - H as f64 / 2.0)
    }

    /// The object's centre on the canvas in frame `i`.
    fn object_canvas(i: u32) -> (f64, f64) {
        let (ox, oy) = object(i);
        let (sx, sy) = shake(i);
        canvas_of((ox + sx, oy + sy))
    }

    #[test]
    fn a_point_track_follows_and_a_title_follows_it() {
        let (mut s, id) = session();
        let t0 = 15u32;
        let c0 = object_canvas(t0);
        let r = s
            .edit(&[json!({"op": "text", "text": "LOOK", "at": 0, "duration": 2, "x": c0.0, "y": c0.1 - 60.0}), json!({"op": "track", "clip": id, "at": t0 as f64 / FPS as f64, "point": [c0.0, c0.1], "radius": 26, "name": "square", "attach": "$1", "follow": "position"})], "track", false)
            .unwrap();
        let _ = r;
        let mc = s.model_clip(id).unwrap();
        let tr = &tracking::tracks(&mc)[0];
        assert_eq!(tr.samples.len(), N as usize);
        assert!(tr.quality().0 > 0.6, "{}", tracking::describe(tr));
        for i in [0u32, 10, 30, 45, 59] {
            let t = (i as f64 / FPS as f64 * SEC as f64) as view::Time;
            let (p, _, lost) = tracking::canvas_points(&s.doc.project, &s.doc.model, id, tr, t).unwrap();
            let want = object_canvas(i);
            assert!(!lost, "frame {i} lost");
            assert!((p[0].x as f64 - want.0).abs() < 1.5 && (p[0].y as f64 - want.1).abs() < 1.5, "frame {i}: {:?} vs {want:?}", p[0]);
        }
        // The title moves with the square: 60 px above it at every frame.
        let title = s.doc.project.clips.iter().find(|c| c.media.is_none()).unwrap().id;
        let tc = s.model_clip(title).unwrap();
        assert!(tc.keyframes["transform.position"].keys.len() >= 55);
        for i in [0u32, 30, 59] {
            let local = (i as f64 / FPS as f64 * SEC as f64) as m::Micros;
            let Some(m::Value::Point(p)) = tc.property_at("transform.position", local) else { panic!() };
            let want = object_canvas(i);
            assert!((p.x - want.0).abs() < 1.5 && (p.y - (want.1 - 60.0)).abs() < 1.5, "frame {i}: {p:?}");
        }
        // inspect says it, compactly.
        let v = s.run("inspect", &json!({"clip": title})).unwrap();
        let text: String = v.iter().map(|c| match c {
            crate::agent::Content::Text(t) => t.clone(),
            _ => String::new(),
        }).collect();
        assert!(text.contains("dense") && text.contains("follows"), "{text}");
    }

    #[test]
    fn a_fix_retracks_and_moves_the_followers() {
        let (mut s, id) = session();
        let c0 = object_canvas(10);
        s.edit(&[json!({"op": "card", "colour": "#ff0000", "at": 0, "duration": 2, "width": 20, "height": 20, "x": c0.0, "y": c0.1}), json!({"op": "track", "clip": id, "at": 10.0 / 30.0, "point": [c0.0, c0.1], "radius": 26, "attach": "$1"})], "t", false).unwrap();
        let card = s.doc.project.clips.iter().find(|c| c.media.is_none()).unwrap().id;
        // Say the square's corner was meant: 20 px right from frame 30 on.
        let c30 = object_canvas(30);
        let r = s.edit(&[json!({"op": "track", "clip": id, "track": 1, "at": 1.0, "fix": [c30.0 + 20.0, c30.1]})], "fix", false).unwrap();
        let _ = r;
        let mc = s.model_clip(id).unwrap();
        let tr = tracking::track_of(&mc, 1).unwrap();
        assert_eq!(tr.fixes, vec![30]);
        let t = (50.0 / 30.0 * SEC as f64) as view::Time;
        let (p, _, _) = tracking::canvas_points(&s.doc.project, &s.doc.model, id, &tr, t).unwrap();
        let want = object_canvas(50);
        assert!((p[0].x as f64 - (want.0 + 20.0)).abs() < 2.0, "{:?} vs {want:?}", p[0]);
        // Before the fix it is where it was.
        let (p, _, _) = tracking::canvas_points(&s.doc.project, &s.doc.model, id, &tr, (5.0 / 30.0 * SEC as f64) as view::Time).unwrap();
        assert!((p[0].x as f64 - object_canvas(5).0).abs() < 1.5);
        // The card moved with the correction.
        let cc = s.model_clip(card).unwrap();
        let Some(m::Value::Point(q)) = cc.property_at("transform.position", t) else { panic!() };
        assert!((q.x - (want.0 + 20.0)).abs() < 2.5, "{q:?}");
        // Undo takes back the correction as one step.
        s.run("undo", &json!({})).unwrap();
        assert!(tracking::track_of(&s.model_clip(id).unwrap(), 1).unwrap().fixes.is_empty());
    }

    #[test]
    fn stabilize_lock_cancels_the_shake() {
        let (mut s, id) = session();
        let r = s.edit(&[json!({"op": "stabilize", "clip": id, "mode": "lock", "max_zoom": 1.2})], "stab", false).unwrap();
        let said = format!("{r:?}");
        assert!(said.contains("shake"), "{said}");
        let mc = s.model_clip(id).unwrap();
        let info = tracking::stabilized(&mc).unwrap();
        assert!(info.zoom > 1.0 && info.zoom < 1.1, "{info:?}");
        // Each frame is moved back by its shake (relative to frame 0), scaled
        // by the zoom.
        for i in [5u32, 17, 33, 48] {
            let local = (i as f64 / FPS as f64 * SEC as f64) as m::Micros;
            let Some(m::Value::Point(p)) = mc.property_at("transform.position", local) else { panic!() };
            let (dx, dy) = (shake(i).0 - shake(0).0, shake(i).1 - shake(0).1);
            assert!((p.x + dx * info.zoom).abs() < 0.8 && (p.y + dy * info.zoom).abs() < 0.8, "frame {i}: {p:?} vs {dx},{dy}");
        }
        // Removing puts it back.
        s.edit(&[json!({"op": "stabilize", "clip": id, "remove": true})], "un", false).unwrap();
        let mc = s.model_clip(id).unwrap();
        assert!(mc.keyframes.get("transform.position").is_none() && tracking::stabilized(&mc).is_none());
    }

    #[test]
    fn a_plane_track_pins_an_insert_on_it() {
        let (mut s, id) = session();
        let i0 = 20u32;
        let (cx, cy) = object_canvas(i0);
        let quad = [[cx - 26.0, cy - 26.0], [cx + 26.0, cy - 26.0], [cx + 26.0, cy + 26.0], [cx - 26.0, cy + 26.0]];
        s.edit(&[json!({"op": "card", "colour": "#00ff00", "at": 0, "duration": 2, "width": 100, "height": 60}), json!({"op": "track", "clip": id, "at": i0 as f64 / 30.0, "plane": quad, "attach": "$1", "follow": "corners"})], "plane", false).unwrap();
        let card = s.doc.project.clips.iter().find(|c| c.media.is_none()).unwrap().id;
        let cc = s.model_clip(card).unwrap();
        assert!(cc.corner_pin.is_some() && cc.keyframes.contains_key("corner_pin.top_left"));
        // The card's top-left corner lands on the square's corner.
        let i = 50u32;
        let t = (i as f64 / 30.0 * SEC as f64) as view::Time;
        let map = crate::editor::pins::LayerMap::at(&s.doc.project, &s.doc.model, card, t).unwrap();
        let cp = s.model_clip(card).unwrap().corner_pin_at(t).unwrap();
        let tl = map.flat_canvas(cp.top_left);
        let (ox, oy) = object_canvas(i);
        assert!((tl.x as f64 - (ox - 26.0)).abs() < 2.0 && (tl.y as f64 - (oy - 26.0)).abs() < 2.0, "{tl:?} vs {:?}", (ox - 26.0, oy - 26.0));
    }

    #[test]
    fn track_refuses_what_it_cannot_do() {
        let (mut s, id) = session();
        let e = s.edit(&[json!({"op": "track", "clip": id})], "x", false).unwrap_err();
        assert!(e.contains("point"), "{e}");
        let e = s.edit(&[json!({"op": "track", "clip": id, "plane": [[0, 0], [1, 1]]})], "x", false).unwrap_err();
        assert!(e.contains("four"), "{e}");
        let e = s.edit(&[json!({"op": "track", "clip": id, "point": [0, 0], "attach": id})], "x", false).unwrap_err();
        assert!(e.contains("own track"), "{e}");
        let e = s.edit(&[json!({"op": "track", "clip": id, "point": [0, 0], "colour": "red"})], "x", false).unwrap_err();
        assert!(e.contains("no field"), "{e}");
    }
}
