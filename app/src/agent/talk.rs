// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Faces for `puppet`: mouths that follow a voice, blinks, and eyes that
//! look at someone.
//!
//! A character drawn as one picture gets face pins (named `face …`) on
//! its eyes and mouth: the mouth opens by pulling a jaw pin down past a
//! held lip, a blink squeezes each eye shut between a lid and a low pin,
//! and a look slides the pupil pin between pins holding the eye's sides.
//! Face pins ride on the head: every key is the head pin's own motion
//! plus the face's, so acts and face motion can be laid in any order.
//!
//! A character made of parts (layers parented to it, named `mouth open`,
//! `mouth closed`, `eye`, `lid`, `pupil`…) gets its mouth layers swapped,
//! its lids shown (or eyes squashed) for blinks and its pupils moved.

use super::super::{Session, f, micros, secs};
use super::{Owner, track};
use crate::view::{self, ClipKind, SEC};
use serde_json::{Value, json};
use studio_model as m;
use studio_speech as speech;

#[path = "face.rs"]
mod face;
#[path = "lipsync.rs"]
mod lipsync;

/// What a face pin does, from its name.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Role {
    Pupil(u32),
    Lid(u32),
    Low(u32),
    Side(u32, bool),
    Jaw,
    Lip,
    Corner(bool),
}

fn role(name: &str) -> Option<Role> {
    let n = name.strip_prefix("face ")?;
    if let Some(rest) = n.strip_prefix("eye") {
        let mut it = rest.split(' ');
        let k: u32 = it.next()?.parse().ok()?;
        return Some(match it.next() {
            None => Role::Pupil(k),
            Some("lid") => Role::Lid(k),
            Some("low") => Role::Low(k),
            Some("left") => Role::Side(k, false),
            Some("right") => Role::Side(k, true),
            _ => return None,
        });
    }
    match n {
        "mouth" => Some(Role::Jaw),
        "lip" => Some(Role::Lip),
        "mouth left" => Some(Role::Corner(false)),
        "mouth right" => Some(Role::Corner(true)),
        _ => None,
    }
}

/// The face pins of a puppet, as `(pin id, role, rest)`.
fn face_pins(pins: &[m::Pin]) -> Vec<(u32, Role, m::Vec2)> {
    pins.iter().filter_map(|p| role(&p.name).map(|r| (p.id, r, p.rest))).collect()
}

/// The pin the face rides on: the head (or beak), not a face pin.
fn head_pin(pins: &[m::Pin]) -> Option<&m::Pin> {
    let named = |w: &str| pins.iter().find(|p| !p.name.starts_with("face ") && p.name.to_lowercase().contains(w));
    named("head").or_else(|| named("beak"))
}

/// Where pin `pid` is at clip time `t` (layer coordinates).
fn pin_at(mc: &m::Clip, pid: u32, t: m::Micros) -> Option<m::Vec2> {
    match mc.property_at(&format!("puppet.{pid}.position"), t) {
        Some(m::Value::Point(p)) => Some(p),
        _ => mc.puppet.as_ref()?.pin(pid).map(|p| p.position),
    }
}

/// How far the head has moved from rest at `t`.
fn head_moved(mc: &m::Clip, t: m::Micros) -> (f64, f64) {
    let Some(pp) = &mc.puppet else { return (0.0, 0.0) };
    let Some(h) = head_pin(&pp.pins) else { return (0.0, 0.0) };
    pin_at(mc, h.id, t).map_or((0.0, 0.0), |p| (p.x - h.rest.x, p.y - h.rest.y))
}

fn key_times(mc: &m::Clip, path: &str) -> Vec<m::Micros> {
    mc.keyframes.get(path).map(|tr| tr.keys.iter().map(|k| k.time).collect()).unwrap_or_default()
}

/// Keys pin `pid` so it rides on the head everywhere and, inside
/// `[from, to]`, adds `own(t)` (layer units) at `times`; outside, the
/// pin keeps whatever face motion `before` gave it.
fn ride(before: &m::Clip, mc: &mut m::Clip, pid: u32, rest: m::Vec2, window: Option<(m::Micros, m::Micros)>, times: &[m::Micros], own: &dyn Fn(m::Micros) -> (f64, f64)) {
    let path = format!("puppet.{pid}.position");
    let head = before.puppet.as_ref().and_then(|p| head_pin(&p.pins)).map(|h| h.id);
    let inside = |t: m::Micros| window.is_some_and(|(a, b)| t >= a - 1000 && t <= b + 1000);
    let mut ts: Vec<m::Micros> = key_times(before, &path).into_iter().filter(|&t| !inside(t)).collect();
    if let Some(h) = head {
        ts.extend(key_times(before, &format!("puppet.{h}.position")));
        ts.extend(key_times(mc, &format!("puppet.{h}.position")));
    }
    if let Some((a, b)) = window {
        ts.extend([a, b]);
        ts.extend(times.iter().copied().filter(|&t| t >= a && t <= b));
    }
    ts.sort_unstable();
    ts.dedup_by(|b, a| *b - *a < 15_000);
    let mut keys = Vec::with_capacity(ts.len());
    let mut moves = false;
    for &t in &ts {
        let (dx, dy) = if inside(t) {
            own(t)
        } else {
            // Its face motion before: where it was, less the head's ride.
            let was = pin_at(before, pid, t).unwrap_or(rest);
            let h = head_moved(before, t);
            (was.x - rest.x - h.0, was.y - rest.y - h.1)
        };
        let h = head_moved(mc, t);
        let p = m::Vec2::new(rest.x + h.0 + dx, rest.y + h.1 + dy);
        moves |= (p.x - rest.x).abs() + (p.y - rest.y).abs() > 1e-7;
        keys.push((t, m::Value::Point(p), "smooth".to_owned()));
    }
    if moves {
        mc.keyframes.insert(path, track(&keys));
    } else {
        mc.keyframes.remove(&path);
    }
}

/// After acts re-keyed the face pins with the head's motion alone, puts
/// their own face motion (lips, blinks, looks) back on top.
pub(super) fn refollow(before: &m::Clip, mc: &mut m::Clip) {
    let pins = mc.puppet.as_ref().map(|p| face_pins(&p.pins)).unwrap_or_default();
    for (pid, _, rest) in pins {
        ride(before, mc, pid, rest, None, &[], &|_| (0.0, 0.0));
    }
}

/// An eye's pins, measured from where they rest.
struct EyePins {
    pupil: (u32, m::Vec2),
    lid: Option<(u32, m::Vec2)>,
    low: Option<(u32, m::Vec2)>,
    sides: Vec<(u32, m::Vec2)>,
    /// Middle and half sizes of the eye's white (layer units).
    c: (f64, f64),
    rx: f64,
    ry: f64,
}

fn eyes_of(fp: &[(u32, Role, m::Vec2)]) -> Vec<EyePins> {
    let mut out = Vec::new();
    for &(pid, r, rest) in fp {
        let Role::Pupil(k) = r else { continue };
        let find = |want: &dyn Fn(Role) -> bool| fp.iter().find(|q| want(q.1)).map(|q| (q.0, q.2));
        let lid = find(&|r| r == Role::Lid(k));
        let low = find(&|r| r == Role::Low(k));
        let sides: Vec<(u32, m::Vec2)> = fp.iter().filter(|q| matches!(q.1, Role::Side(j, _) if j == k)).map(|q| (q.0, q.2)).collect();
        // The pins sit just outside the white (see `place`).
        let ry = match (lid, low) {
            (Some(a), Some(b)) => (b.1.y - a.1.y) / 2.0 / OUT,
            _ => 0.02,
        };
        let cy = match (lid, low) {
            (Some(a), Some(b)) => (a.1.y + b.1.y) / 2.0,
            _ => rest.y,
        };
        let (rx, cx) = match sides.as_slice() {
            [a, b] => ((a.1.x - b.1.x).abs() / 2.0 / OUT, (a.1.x + b.1.x) / 2.0),
            _ => (ry, rest.x),
        };
        out.push(EyePins { pupil: (pid, rest), lid, low, sides, c: (cx, cy), rx, ry });
    }
    out
}

/// How far outside the eye's white its holding pins sit (in its radii).
const OUT: f64 = 1.18;

/// Where a mouth pose moves each mouth pin (layer units), for a mouth
/// `w` wide; `aspect` is the layer's width over its height.
fn mouth_move(r: Role, p: &face::Pose, w: f64, aspect: f64) -> (f64, f64) {
    // `w` is in layer x units; drops are in y units.
    let wy = w * aspect;
    match r {
        Role::Jaw => (0.0, p.open * 0.34 * wy),
        Role::Lip => (0.0, -p.open * 0.05 * wy),
        Role::Corner(right) => {
            let out = if right { 1.0 } else { -1.0 };
            (out * w * (0.10 * p.wide.max(0.0) + 0.16 * p.wide.min(0.0)) * (0.4 + 0.6 * p.open.max(0.3)), p.open * 0.1 * wy)
        }
        _ => (0.0, 0.0),
    }
}

/// The pose at `t` from a list of poses (straight between them).
fn pose_at(poses: &[face::Pose], t: f64) -> face::Pose {
    match poses.iter().position(|p| p.t > t) {
        Some(0) => face::Pose { t, ..poses[0] },
        Some(i) => {
            let (a, b) = (poses[i - 1], poses[i]);
            let k = (t - a.t) / (b.t - a.t).max(1e-6);
            face::Pose { t, open: a.open + k * (b.open - a.open), wide: a.wide + k * (b.wide - a.wide) }
        }
        None => poses.last().map_or(face::Pose { t, open: 0.0, wide: 0.0 }, |p| face::Pose { t, ..*p }),
    }
}

/// A gaze over a window: eased in over a quarter second and out again.
fn gaze_at(t: f64, from: f64, to: f64) -> f64 {
    let e = ((t - from) / 0.25).min((to - t) / 0.25).clamp(0.0, 1.0);
    e * e * (3.0 - 2.0 * e)
}

/// A voice clip's line: its sound (mono), its speed and its words
/// (seconds from the first sample).
struct Line {
    /// The sound file, and where in it (seconds) this line begins.
    path: std::path::PathBuf,
    f0: f64,
    mono: Vec<f32>,
    rate: u32,
    pace: f64,
    words: Vec<(String, f64, f64)>,
}

impl Session {
    /// The part of voice clip `vid` that plays between timeline seconds
    /// `t0` and `t1`.
    fn line_of(&self, vid: u32, t0: f64, t1: f64) -> Result<Line, String> {
        let p = &self.doc.project;
        let c = p.clip(vid).ok_or_else(|| format!("no voice clip {vid}"))?;
        if c.kind != ClipKind::Audio && c.media.and_then(|m| p.media(m)).is_none_or(|mi| mi.path.is_none()) {
            return Err(format!("voice {vid} is not a sound clip: give the clip of the spoken line"));
        }
        let mi = c.media.and_then(|m| p.media(m)).ok_or_else(|| format!("clip {vid} has no sound file"))?;
        let path = mi.path.clone().ok_or_else(|| format!("clip {vid} has no sound file"))?;
        let d = crate::sound_bridge::decoded(&path.to_string_lossy()).ok_or_else(|| format!("clip {vid}'s file has no sound Studio can decode"))?;
        let pace = (c.speed as f64 / 100.0).max(0.05);
        let file_at = |t: f64| secs(c.src_in) + (t - secs(c.start)) * pace;
        let (f0, f1) = (file_at(t0).max(0.0), file_at(t1));
        let rate = d.rate as f64;
        let (i0, i1) = ((f0 * rate) as usize, ((f1 * rate) as usize).min(d.left.len()));
        if i1 <= i0 + d.rate as usize / 10 {
            return Err(format!("voice clip {vid} has no sound between {t0:.2}s and {t1:.2}s"));
        }
        let mono: Vec<f32> = d.left[i0..i1].iter().zip(d.right[i0..i1].iter()).map(|(l, r)| 0.5 * (l + r)).collect();
        let words = std::fs::read(path.with_extension("words.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Vec<super::super::voice::Said>>(&b).ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|w| w.end > f0 && w.start < f1)
            .map(|w| (w.text, w.start - f0, w.end - f0))
            .collect();
        Ok(Line { path, f0, mono, rate: d.rate, pace, words })
    }

    /// The window a voice clip gives an act on clip `id`: `(from,
    /// duration)` in clip seconds, where both play.
    pub(super) fn voice_window(&self, id: u32, vid: u32) -> Result<(f64, f64), String> {
        let c = self.clip(id)?;
        let v = self.clip(vid)?;
        let (a, b) = (v.start.max(c.start), (v.start + v.dur).min(c.start + c.dur));
        if b - a < SEC / 4 {
            return Err(format!("voice clip {vid} ({:.2}–{:.2}s) hardly overlaps clip {id} ({:.2}–{:.2}s)", secs(v.start), secs(v.start + v.dur), secs(c.start), secs(c.start + c.dur)));
        }
        Ok((secs(a - c.start), secs(b - a)))
    }

    /// The face's places on clip `id`: from `spec` (`{"eyes": [points],
    /// "mouth": point, "mouth_width": px}`, points as for pins) or found
    /// in its picture. Returns pins to place and what was found.
    fn find_face(&self, id: u32, mc: &m::Clip, at: view::Time, spec: Option<&Value>) -> Result<(Vec<Value>, String), String> {
        let map = self.layer_map(id, at)?;
        let px = map.pixels();
        let (pw, ph) = (px.x.max(1.0) as f64, px.y.max(1.0) as f64);
        let mut found = None;
        let mut how = String::new();
        if let m::ClipSource::Media(r) = &mc.source {
            if let Some(path) = self.doc.project.media(r.media.0 as u32).and_then(|mi| mi.path.clone()).filter(|p| p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("png"))) {
                let c = &mc.transform.crop;
                if let Some(pic) = face::Picture::of_png(&path, (c.left, c.top, c.right, c.bottom)) {
                    let right = mc.puppet.as_ref().and_then(|p| Self::picture_faces_right(&p.pins));
                    if let Some(fd) = face::find(&pic, right) {
                        found = Some((fd, pic.w as f64, pic.h as f64));
                    }
                }
            }
        }
        // Everything in layer units: (u, v) and sizes across (x) and down (y).
        let mut eyes: Vec<((f64, f64), f64, f64, (f64, f64))> = Vec::new();
        let mut mouth: Option<((f64, f64), f64)> = None;
        if let Some((fd, w, h)) = &found {
            let uv = |p: (f64, f64)| ((p.0 + 0.5) / w, (p.1 + 0.5) / h);
            eyes = fd.eyes.iter().map(|e| (uv(e.c), e.rx / w, e.ry / h, uv(e.pupil))).collect();
            mouth = fd.mouth.map(|mo| (uv(mo.c), mo.w / w));
            how = format!("found {} eye(s){} in the picture", fd.eyes.len(), if fd.mouth_guessed { " and placed the mouth from them" } else { " and the mouth line" });
        }
        if let Some(Value::Object(o)) = spec {
            let point = |v: &Value| Self::layer_point(&map, v, false).map(|p| (p.x, p.y));
            if let Some(Value::Array(list)) = o.get("eyes") {
                let size = f(&Value::Object(o.clone()), "eye_size").map(|s| s / 2.0);
                let mut given = Vec::new();
                for v in list {
                    let c = point(v)?;
                    // The nearest found eye lends its size, else a guess.
                    let near = eyes.iter().min_by(|a, b| ((a.0.0 - c.0).hypot(a.0.1 - c.1)).total_cmp(&(b.0.0 - c.0).hypot(b.0.1 - c.1))).filter(|e| (e.0.0 - c.0).hypot(e.0.1 - c.1) < 2.0 * e.1);
                    let (rx, ry) = match (size, near) {
                        (Some(s), _) => (s / pw, s / ph),
                        (None, Some(e)) => (e.1, e.2),
                        (None, None) => (0.035 * ph.min(pw) / pw, 0.035 * ph.min(pw) / ph),
                    };
                    given.push((c, rx, ry, c));
                }
                eyes = given;
            }
            if let Some(v) = o.get("mouth") {
                let c = point(v)?;
                let w = f(&Value::Object(o.clone()), "mouth_width").map(|x| x / pw).or(mouth.map(|m| m.1)).unwrap_or(0.12);
                mouth = Some((c, w));
            }
            how = if how.is_empty() { "placed as given".into() } else { format!("{how}, corrected as given") };
        }
        if eyes.is_empty() && mouth.is_none() {
            return Err(format!(
                "no face found in clip {id}'s picture (Studio looks for pupils in eye whites and a mouth line below them): give \"face\": {{\"eyes\": [[x, y], …], \"mouth\": [x, y], \"mouth_width\": px}} in canvas pixels (look with frame first)"
            ));
        }
        let mut pins = Vec::new();
        let at = |n: String, (u, v): (f64, f64)| json!({"name": n, "at": {"u": (u * 10000.0).round() / 10000.0, "v": (v * 10000.0).round() / 10000.0}});
        for (k, (c, rx, ry, pupil)) in eyes.iter().enumerate() {
            let k = k + 1;
            pins.push(at(format!("face eye{k}"), *pupil));
            pins.push(at(format!("face eye{k} lid"), (c.0, c.1 - OUT * ry)));
            pins.push(at(format!("face eye{k} low"), (c.0, c.1 + OUT * ry)));
            pins.push(at(format!("face eye{k} left"), (c.0 - OUT * rx, c.1)));
            pins.push(at(format!("face eye{k} right"), (c.0 + OUT * rx, c.1)));
        }
        if let Some((c, w)) = mouth {
            let wy = w * pw / ph;
            pins.push(at("face lip".into(), (c.0, c.1 - 0.07 * wy)));
            pins.push(at("face mouth".into(), (c.0, c.1 + 0.1 * wy)));
            pins.push(at("face mouth left".into(), (c.0 - w / 2.0, c.1)));
            pins.push(at("face mouth right".into(), (c.0 + w / 2.0, c.1)));
        }
        let canvas = |p: (f64, f64)| {
            let q = map.canvas(m::Vec2::new(p.0, p.1));
            json!([q.x.round(), q.y.round()])
        };
        let mut said = vec![how];
        if !eyes.is_empty() {
            said.push(format!("eyes at {}", eyes.iter().map(|e| canvas(e.0).to_string()).collect::<Vec<_>>().join(", ")));
        }
        if let Some((c, w)) = mouth {
            said.push(format!("mouth at {} ({:.0} px wide)", canvas(c), w * pw));
        }
        Ok((pins, said.join("; ")))
    }

    /// Face work for `puppet`: `face` (place or correct the face pins),
    /// `voice` (lip sync), `blink` and `look_at`, on the pins in `mc`, or
    /// on the character's part layers. Window is `(from, dur)` in clip
    /// seconds. Returns what was done; part layers are edited directly.
    pub(super) fn face_work(&mut self, op: &Value, id: u32, mc: &mut m::Clip, vc: &view::Clip, at: view::Time, window: (f64, f64), talking: bool) -> Result<Vec<String>, String> {
        let mut said = Vec::new();
        let voice = op.get("voice").map(|v| v.as_u64().map(|v| v as u32).ok_or("voice is the clip id of the spoken line (a sound clip)")).transpose()?;
        let blink_every = match op.get("blink") {
            None => (talking && voice.is_some()).then_some(4.0),
            Some(Value::Bool(b)) => b.then_some(4.0),
            Some(Value::Number(n)) => Some(n.as_f64().unwrap_or(4.0).clamp(1.0, 20.0)),
            Some(_) => return Err("blink is true, false, or the average seconds between blinks (4 is natural; 2 nervous; 8 calm)".into()),
        };
        let look = op.get("look_at");
        if op.get("face").is_none() && voice.is_none() && blink_every.is_none() && look.is_none() {
            return Ok(said);
        }
        // A character made of parts: its layers do the work.
        let parts = self.parts_of(id, op.get("parts"))?;
        if !parts.is_empty() && op.get("face").is_none_or(|v| v.get("parts").is_some()) {
            return self.parts_work(op, id, vc, &parts, voice, blink_every, look, window);
        }
        let pins = mc.puppet.as_ref().map(|p| p.pins.clone()).unwrap_or_default();
        let has_face = !face_pins(&pins).is_empty();
        if op.get("face").is_some_and(|v| v != &Value::Bool(false)) || !has_face {
            let spec = op.get("face").filter(|v| v.is_object());
            let (new, how) = self.find_face(id, mc, at, spec)?;
            // Old face pins go; something must hold the rest of the body.
            let mut old: Vec<u32> = face_pins(&pins).iter().map(|p| p.0).collect();
            // Other pins on an eye or the mouth would hold it still.
            let placed: Vec<(u32, Role, m::Vec2)> = new
                .iter()
                .filter_map(|p| Some((0, role(p["name"].as_str()?)?, m::Vec2::new(p["at"]["u"].as_f64()?, p["at"]["v"].as_f64()?))))
                .collect();
            let px = self.layer_map(id, at)?.pixels();
            let aspect = (px.x / px.y.max(1.0)) as f64;
            let mouth: Vec<m::Vec2> = placed.iter().filter(|p| matches!(p.1, Role::Corner(_) | Role::Jaw | Role::Lip)).map(|p| p.2).collect();
            let on_face = |q: m::Vec2| {
                eyes_of(&placed).iter().any(|e| (q.x - e.c.0).abs() < 1.25 * e.rx && (q.y - e.c.1).abs() < 1.25 * e.ry)
                    || (mouth.len() == 4 && {
                        let (x0, x1) = (mouth.iter().map(|p| p.x).fold(f64::INFINITY, f64::min), mouth.iter().map(|p| p.x).fold(f64::NEG_INFINITY, f64::max));
                        let (y0, y1) = (mouth.iter().map(|p| p.y).fold(f64::INFINITY, f64::min), mouth.iter().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max));
                        let pad = 0.12 * (x1 - x0) * aspect;
                        q.x > x0 - 0.1 * (x1 - x0) && q.x < x1 + 0.1 * (x1 - x0) && q.y > y0 - pad && q.y < y1 + pad
                    })
            };
            let cleared: Vec<String> = pins.iter().filter(|p| role(&p.name).is_none() && on_face(p.rest)).map(|p| if p.name.is_empty() { p.id.to_string() } else { p.name.clone() }).collect();
            old.extend(pins.iter().filter(|p| role(&p.name).is_none() && on_face(p.rest)).map(|p| p.id));
            if !cleared.is_empty() {
                said.push(format!("took away pins on the eyes and mouth ({}) so they can move", cleared.join(", ")));
            }
            if !old.is_empty() {
                self.edit_pins(&json!({"remove": old}), id, mc, vc, at, Owner::Puppet)?;
            }
            let others = mc.puppet.as_ref().map_or(0, |p| p.pins.len());
            if others == 0 {
                let shape = self.shape_of(mc)?;
                let right = shape.facing().unwrap_or(true);
                let hold = super::rig::rig("object", &shape, right)?;
                self.edit_pins(&json!({"pins": hold}), id, mc, vc, at, Owner::Puppet)?;
                said.push("held its base and top with pins (rig it for acting)".into());
            }
            self.edit_pins(&json!({"pins": new}), id, mc, vc, at, Owner::Puppet)?;
            // A mesh fine enough to fold an eyelid.
            let px = self.layer_map(id, at)?.pixels();
            let fp = face_pins(&mc.puppet.as_ref().map(|p| p.pins.clone()).unwrap_or_default());
            let smallest = eyes_of(&fp).iter().map(|e| e.ry * px.y as f64).fold(f64::INFINITY, f64::min);
            if let Some(pp) = &mut mc.puppet {
                let long = px.x.max(px.y) as f64;
                let want = if smallest.is_finite() { (long / (smallest * 0.9)).clamp(24.0, 96.0) } else { 48.0 };
                pp.density = pp.density.max(want.round());
            }
            // The new pins ride on the head from the start.
            let before = mc.clone();
            refollow(&before, mc);
            said.push(format!("face pins: {how}. Check with frame; correct with \"face\": {{\"eyes\": [[x, y], …], \"mouth\": [x, y]}}"));
        }
        let pins = mc.puppet.as_ref().map(|p| p.pins.clone()).unwrap_or_default();
        let fp = face_pins(&pins);
        let map = self.layer_map(id, at)?;
        let px = map.pixels();
        let aspect = (px.x / px.y.max(1.0)) as f64;
        let (from, dur) = window;
        let w0 = micros(from);
        let w1 = micros(from + dur);
        let us = |t: f64| micros(t);
        if let Some(vid) = voice {
            let (vf, vd) = self.voice_window(id, vid)?;
            let (lf, ld) = (vf.max(from), (vf + vd).min(from + dur) - vf.max(from));
            if ld < 0.25 {
                return Err(format!("voice clip {vid} plays outside the window ({from:.2}s for {dur:.2}s of the clip)"));
            }
            let line = self.line_of(vid, secs(vc.start) + lf, secs(vc.start) + lf + ld)?;
            let fps = (self.doc.project.fps as f64).max(1.0);
            let style = lipsync::style_of(op, fps, id as u64)?;
            let synced = lipsync::sync_line(&line, &style);
            let poses: Vec<face::Pose> = match &synced {
                Some(s) => speech::drive::pose_keys(&s.curves, fps).into_iter().map(|(t, open, wide)| face::Pose { t: lf + t, open, wide }).collect(),
                None => face::lip_sync(&line.mono, line.rate, &line.words).into_iter().map(|p| face::Pose { t: lf + p.t / line.pace, ..p }).collect(),
            };
            let mouth: Vec<(u32, Role, m::Vec2)> = fp.iter().copied().filter(|p| matches!(p.1, Role::Jaw | Role::Lip | Role::Corner(_))).collect();
            if mouth.is_empty() {
                return Err("this face has no mouth pins: give \"face\": {\"mouth\": [x, y]} (canvas pixels) to place them".into());
            }
            let mw = {
                let cs: Vec<f64> = mouth.iter().filter(|p| matches!(p.1, Role::Corner(_))).map(|p| p.2.x).collect();
                if cs.len() == 2 { (cs[0] - cs[1]).abs() } else { 0.1 }
            };
            let times: Vec<m::Micros> = poses.iter().map(|p| us(p.t)).collect();
            let before = mc.clone();
            let (a, b) = (us(lf), us(lf + ld));
            for &(pid, r, rest) in &mouth {
                ride(&before, mc, pid, rest, Some((a, b)), &times, &|t| mouth_move(r, &pose_at(&poses, secs(t)), mw, aspect));
            }
            let inside = self.mouth_inside(id, mc, vc, (lf, ld), &poses)?;
            let open = poses.iter().filter(|p| p.open > 0.18).count();
            let shapes = |s: face::Shape| poses.iter().filter(|p| face::Shape::of(p) == s).count();
            said.push(format!(
                "lip sync to clip {vid} from {lf:.2}s for {ld:.2}s of the clip: {open} mouth opening(s) ({} wide, {} round){}; the open mouth is clip {inside} (it follows the face; act or move the character again, then talk again)",
                shapes(face::Shape::Wide),
                shapes(face::Shape::Round),
                match &synced {
                    Some(s) => format!(", {}", s.how),
                    None if line.words.is_empty() => ", shaped by the sound (no words beside the file: transcribe it or narrate it for phoneme timing)".to_owned(),
                    None => ", vowels from its words".to_owned(),
                }
            ));
        }
        let eyes = eyes_of(&fp);
        // Where the eyes look, as a slide of the pupil (layer units).
        let mut look_moves: Vec<(f64, f64)> = Vec::new();
        let mut look_said = None;
        if let Some(target) = look {
            if eyes.is_empty() {
                return Err("this face has no eye pins: give \"face\": {\"eyes\": [[x, y], …]} (canvas pixels) to place them".into());
            }
            let t_mid = vc.start + micros(from + dur / 2.0);
            let mid_map = self.layer_map(id, t_mid)?;
            let (dir, what) = self.look_dir(target, t_mid, &mid_map, &eyes)?;
            for e in &eyes {
                // Slide the pupil toward `dir`, keeping it inside the white.
                let (gx, gy) = dir;
                let tx = (e.pupil.1.x + gx * 0.45 * e.rx).clamp(e.c.0 - 0.42 * e.rx, e.c.0 + 0.42 * e.rx);
                let ty = (e.pupil.1.y + gy * 0.3 * e.ry).clamp(e.c.1 - 0.3 * e.ry, e.c.1 + 0.3 * e.ry);
                look_moves.push((tx - e.pupil.1.x, ty - e.pupil.1.y));
            }
            look_said = Some(format!("eyes on {what} from {from:.2}s for {dur:.2}s"));
        }
        if blink_every.is_some() || !look_moves.is_empty() {
            if eyes.is_empty() {
                return Err("this face has no eye pins: give \"face\": {\"eyes\": [[x, y], …]} (canvas pixels) to place them".into());
            }
            let starts: Vec<f64> = match blink_every {
                Some(every) => face::blinks(id as u64 ^ micros(from) as u64, dur, every, if look_moves.is_empty() { &[] } else { &[0.02] }).into_iter().map(|t| from + t).collect(),
                None => Vec::new(),
            };
            let lid = |t: f64| starts.iter().map(|&s| face::lid(t - s)).fold(0.0, f64::max);
            let mut times: Vec<m::Micros> = starts.iter().flat_map(|&s| face::LID_KEYS.iter().map(move |k| us(s + k))).collect();
            if !look_moves.is_empty() {
                times.extend([w0, us(from + 0.25), us(from + dur - 0.25), w1]);
            }
            let before = mc.clone();
            for (k, e) in eyes.iter().enumerate() {
                let line = e.c.1 + 0.12 * e.ry;
                let g = look_moves.get(k).copied();
                let mut group: Vec<(u32, m::Vec2, bool)> = vec![(e.pupil.0, e.pupil.1, true)];
                group.extend(e.lid.map(|p| (p.0, p.1, false)));
                group.extend(e.low.map(|p| (p.0, p.1, false)));
                group.extend(e.sides.iter().map(|p| (p.0, p.1, false)));
                for (pid, rest, pupil) in group {
                    // Without a new look, keep any look laid before.
                    let old_look = |t: m::Micros| {
                        let was = pin_at(&before, pid, t).unwrap_or(rest);
                        let h = head_moved(&before, t);
                        (was.x - rest.x - h.0, was.y - rest.y - h.1)
                    };
                    let own = |t: m::Micros| {
                        let s = secs(t);
                        let (lx, ly) = match g {
                            Some(g) if pupil => {
                                let k = gaze_at(s, from, from + dur);
                                (g.0 * k, g.1 * k)
                            }
                            Some(_) => (0.0, 0.0),
                            None if pupil => {
                                let o = old_look(t);
                                (o.0, o.1)
                            }
                            None => (0.0, 0.0),
                        };
                        // Shut: everything round the eye folds onto one line.
                        let shut = lid(s);
                        (lx, ly + shut * (line - rest.y - ly))
                    };
                    ride(&before, mc, pid, rest, Some((w0, w1)), &times, &own);
                }
            }
            if !starts.is_empty() {
                said.push(format!("{} blink(s) from {from:.2}s for {dur:.2}s", starts.len()));
            }
            if let Some(l) = look_said {
                said.push(l);
            }
        }
        Ok(said)
    }

    /// The inside of the mouth: a dark shape over the mouth of clip `id`
    /// that opens between the lip and the jaw as they part, for the line
    /// at `(from, dur)` clip seconds (pins in `mc` already keyed). A
    /// picture's closed mouth only stretches when pins pull it; this is
    /// what reads as open. Replaces any it made before in that window.
    fn mouth_inside(&mut self, id: u32, mc: &m::Clip, vc: &view::Clip, (from, dur): (f64, f64), poses: &[face::Pose]) -> Result<u32, String> {
        let pins = mc.puppet.as_ref().map(|p| p.pins.clone()).unwrap_or_default();
        let fp = face_pins(&pins);
        let get = |r: Role| fp.iter().find(|p| p.1 == r).map(|p| (p.0, p.2));
        let (Some(lip), Some(jaw), Some(cl), Some(cr)) = (get(Role::Lip), get(Role::Jaw), get(Role::Corner(false)), get(Role::Corner(true))) else {
            return Err("the mouth needs its four face pins (face lip, face mouth, face mouth left and right): place them with \"face\": {\"mouth\": [x, y]}".into());
        };
        let name = format!("{} · mouth inside", if mc.name.is_empty() { format!("clip {id}") } else { mc.name.clone() });
        let (t0, t1) = (vc.start + micros(from), vc.start + micros(from + dur));
        let old: Vec<u32> = self.doc.project.clips.iter().filter(|c| c.name == name && c.start < t1 && c.start + c.dur > t0).map(|c| c.id).collect();
        if !old.is_empty() {
            self.op(&json!({"op": "delete", "clips": old, "ripple": false}))?;
        }
        // Where the mouth is, how wide, how open and which way it turns,
        // in canvas pixels at clip time `t`.
        let line = m::Vec2::new((cl.1.x + cr.1.x) / 2.0, (cl.1.y + cr.1.y) / 2.0);
        let shape = |t: m::Micros| -> Option<(eframe::egui::Vec2, f32, f32, f32)> {
            let map = self.layer_map(id, vc.start + t).ok()?;
            let at = |pid: u32, rest: m::Vec2| map.canvas(pin_at(mc, pid, t).unwrap_or(rest));
            let off = |pid: u32, rest: m::Vec2, to: m::Vec2| {
                let p = pin_at(mc, pid, t).unwrap_or(rest);
                map.canvas(m::Vec2::new(p.x + to.x - rest.x, p.y + to.y - rest.y))
            };
            // The lip carries the top of the opening; the jaw its bottom.
            let top = off(lip.0, lip.1, line);
            let bottom = off(jaw.0, jaw.1, line);
            let (l, r) = (at(cl.0, cl.1), at(cr.0, cr.1));
            let across = r - l;
            let w = across.length() * 0.78;
            let h = (bottom - top).length();
            let mut turn = across.y.atan2(across.x).to_degrees();
            if turn > 90.0 {
                turn -= 180.0;
            } else if turn < -90.0 {
                turn += 180.0;
            }
            Some(((top + bottom) / 2.0, w, h, turn))
        };
        // Keys a little more often than the poses, so it rides the head.
        let mut ts: Vec<m::Micros> = poses.iter().map(|p| micros(p.t)).filter(|&t| t >= micros(from) && t <= micros(from + dur)).collect();
        let step = micros(1.0 / 15.0).max(1);
        ts.extend((micros(from)..=micros(from + dur)).step_by(step as usize));
        ts.sort_unstable();
        ts.dedup_by(|b, a| *b - *a < 15_000);
        let samples: Vec<(m::Micros, (eframe::egui::Vec2, f32, f32, f32))> = ts.iter().filter_map(|&t| shape(t).map(|s| (t, s))).collect();
        let rest = samples.first().map(|s| s.1).ok_or("the character has no size there")?;
        let full = (rest.1.max(4.0) as f64).round();
        self.op(&json!({"op": "card", "colour": "#3b1712", "at": secs(t0), "duration": dur, "width": full, "height": full, "shape": "ellipse", "x": rest.0.x, "y": rest.0.y}))?;
        let oid = self.doc.project.clips.iter().map(|c| c.id).max().ok_or("the mouth was not added")?;
        let mut oc = self.model_clip(oid).ok_or("the mouth was not added")?;
        oc.name = name;
        let (mut pos, mut sx, mut sy, mut rot) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for &(t, (c, w, h, turn)) in &samples {
            let k = t - micros(from);
            // Shut, it is nothing: a sliver would show as a dark line.
            let h = if h < 1.5 { 0.0 } else { h };
            pos.push((k, m::Value::Point(m::Vec2::new(c.x as f64, c.y as f64)), "linear".to_owned()));
            sx.push((k, m::Value::Scalar(if h == 0.0 { 0.0 } else { w as f64 / full }), "linear".to_owned()));
            sy.push((k, m::Value::Scalar(h as f64 / full), "linear".to_owned()));
            rot.push((k, m::Value::Angle(turn as f64), "linear".to_owned()));
        }
        oc.keyframes.insert("transform.position".into(), track(&pos));
        oc.keyframes.insert("transform.scale_x".into(), track(&sx));
        oc.keyframes.insert("transform.scale_y".into(), track(&sy));
        oc.keyframes.insert("transform.rotation".into(), track(&rot));
        let seq = self.doc.seq();
        let ok = self.doc.apply("Mouth", m::Edit::SetClip { seq, clip: Box::new(oc) });
        self.check(ok, "")?;
        Ok(oid)
    }

    /// Which way to look (a unit step in layer units, x across and y
    /// down) at `target`: a clip (its head, else its upper middle), or
    /// "camera", "left", "right", "up", "down".
    fn look_dir(&self, target: &Value, t: view::Time, map: &crate::editor::pins::LayerMap, eyes: &[EyePins]) -> Result<((f64, f64), String), String> {
        let n = eyes.len().max(1) as f64;
        let eye = m::Vec2::new(eyes.iter().map(|e| e.c.0).sum::<f64>() / n, eyes.iter().map(|e| e.c.1).sum::<f64>() / n);
        let from = map.canvas(eye);
        let toward = |dx: f32, dy: f32| -> Option<(f64, f64)> {
            let len = (dx * dx + dy * dy).sqrt().max(1e-3);
            let a = map.layer(from)?;
            let b = map.layer(from + eframe::egui::Vec2::new(dx / len * 40.0, dy / len * 40.0))?;
            let px = map.pixels();
            let (x, y) = ((b.x - a.x) * px.x as f64, (b.y - a.y) * px.y as f64);
            let l = x.hypot(y).max(1e-6);
            Some((x / l, y / l))
        };
        let (dir, what) = match target {
            Value::Number(n) => {
                let tid = n.as_u64().ok_or("look_at is a clip id or camera, left, right, up, down")? as u32;
                let tc = self.clip(tid)?;
                let t = t.clamp(tc.start, tc.start + tc.dur);
                let tmap = self.layer_map(tid, t)?;
                let tm = self.model_clip(tid).ok_or("that clip is gone")?;
                let head = tm.puppet.as_ref().and_then(|p| {
                    let h = head_pin(&p.pins)?;
                    pin_at(&tm, h.id, t - tc.start)
                });
                let p = tmap.canvas(head.unwrap_or_else(|| tmap.content(0.0, -0.4)));
                // Eyes go most of the way across and a little up or down.
                let (dx, dy) = (p.x - from.x, (p.y - from.y) * 0.5);
                (toward(dx, dy), format!("clip {tid}"))
            }
            Value::String(s) => match s.as_str() {
                "camera" | "viewer" | "us" => {
                    let c = eyes.iter().map(|e| (e.c.0 - e.pupil.1.x, e.c.1 - e.pupil.1.y)).fold((0.0, 0.0), |a, b| (a.0 + b.0 / n, a.1 + b.1 / n));
                    let l = c.0.hypot(c.1);
                    (Some(if l < 1e-6 { (0.0, 0.0) } else { (c.0 / l, c.1 / l) }), "the camera".to_owned())
                }
                "left" => (toward(-1.0, 0.0), "the left".to_owned()),
                "right" => (toward(1.0, 0.0), "the right".to_owned()),
                "up" => (toward(0.0, -1.0), "up".to_owned()),
                "down" => (toward(0.0, 1.0), "down".to_owned()),
                other => return Err(format!("look_at \"{other}\": a clip id, camera, left, right, up or down")),
            },
            _ => return Err("look_at is a clip id (whoever is talking) or camera, left, right, up, down".into()),
        };
        Ok((dir.ok_or("the layer has no size there (scale 0?)")?, what))
    }

    /// The part layers of a character: `spec` (`{"mouths": {"open": id,
    /// …}, "eyes": [ids], "lids": [ids], "pupils": [ids]}`), else the
    /// layers parented to it (and to those) named mouth…, eye, lid, pupil.
    fn parts_of(&self, id: u32, spec: Option<&Value>) -> Result<Vec<(u32, Part)>, String> {
        let mut out = Vec::new();
        if let Some(Value::Object(o)) = spec {
            if let Some(Value::Object(ms)) = o.get("mouths") {
                for (k, v) in ms {
                    let s = speech::Viseme::named(k).ok_or_else(|| format!("mouth shape \"{k}\": {}", lipsync::CHART))?;
                    out.push((v.as_u64().ok_or("mouths maps a shape to a clip id")? as u32, Part::Mouth(s)));
                }
            }
            for (key, part) in [("eyes", Part::Eye), ("lids", Part::Lid), ("pupils", Part::Pupil)] {
                if let Some(Value::Array(l)) = o.get(key) {
                    out.extend(l.iter().filter_map(Value::as_u64).map(|c| (c as u32, part)));
                }
            }
            for (c, _) in &out {
                self.clip(*c)?;
            }
            return Ok(out);
        }
        let seq = self.doc.model.sequences.iter().find(|s| s.clips.iter().any(|c| c.id.0 == id as u64));
        let Some(seq) = seq else { return Ok(out) };
        let mut family = vec![id as u64];
        let mut i = 0;
        while i < family.len() {
            let p = family[i];
            for c in &seq.clips {
                if c.parent.as_ref().is_some_and(|l| l.clip.0 == p) && !family.contains(&c.id.0) {
                    family.push(c.id.0);
                }
            }
            i += 1;
        }
        for c in seq.clips.iter().filter(|c| family[1..].contains(&c.id.0)) {
            let n = c.name.to_lowercase();
            let words: Vec<&str> = n.split(|ch: char| !ch.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
            let part = if words.contains(&"mouth") {
                words.iter().find_map(|w| speech::Viseme::named(w)).map(Part::Mouth)
            } else if words.iter().any(|w| matches!(*w, "lid" | "lids" | "eyelid" | "eyelids")) {
                Some(Part::Lid)
            } else if words.iter().any(|w| matches!(*w, "pupil" | "pupils" | "iris")) {
                Some(Part::Pupil)
            } else if words.iter().any(|w| matches!(*w, "eye" | "eyes")) {
                Some(Part::Eye)
            } else {
                None
            };
            if let Some(p) = part {
                out.push((c.id.0 as u32, p));
            }
        }
        Ok(out)
    }

    /// Lip sync, blinks and looks on a character's part layers.
    #[allow(clippy::too_many_arguments)]
    fn parts_work(&mut self, op: &Value, id: u32, vc: &view::Clip, parts: &[(u32, Part)], voice: Option<u32>, blink_every: Option<f64>, look: Option<&Value>, (from, dur): (f64, f64)) -> Result<Vec<String>, String> {
        let mut said = Vec::new();
        let t0 = secs(vc.start) + from;
        let t1 = t0 + dur;
        // Hold keys on `path` of clip `cid` at timeline times, keeping
        // its keys outside [t0, t1].
        let put = |s: &mut Session, cid: u32, path: &str, keys: Vec<(f64, m::Value, m::Interp)>| -> Result<(), String> {
            let c = s.clip(cid)?;
            let mut mc = s.model_clip(cid).ok_or("clip gone")?;
            let (a, b) = (micros(t0) - c.start, micros(t1) - c.start);
            let mut ks: Vec<m::Keyframe> = mc.keyframes.get(path).map(|tr| tr.keys.iter().filter(|k| k.time < a - 1000 || k.time > b + 1000).cloned().collect()).unwrap_or_default();
            ks.extend(keys.into_iter().map(|(t, v, i)| m::Keyframe::new((micros(t) - c.start).clamp(0, c.dur), v, i)));
            ks.sort_by_key(|k| k.time);
            mc.keyframes.insert(path.to_owned(), m::Track::new(ks));
            let seq = s.doc.seq();
            let ok = s.doc.apply("Face", m::Edit::SetClip { seq, clip: Box::new(mc) });
            s.check(ok, "").map(|_| ())
        };
        if let Some(vid) = voice {
            let mouths: Vec<(u32, speech::Viseme)> = parts.iter().filter_map(|(c, p)| if let Part::Mouth(v) = p { Some((*c, *v)) } else { None }).collect();
            if mouths.is_empty() {
                return Err(format!("clip {id} has no mouth layers: name its parts after the mouth chart (\"mouth rest\", \"mouth MBP\", \"mouth AA\", \"mouth EE\", \"mouth OH\"… or the plain \"mouth closed\", \"mouth open\", \"mouth wide\", \"mouth round\"), or give \"parts\": {{\"mouths\": {{\"closed\": id, \"open\": id}}}}"));
            }
            let (vf, vd) = self.voice_window(id, vid)?;
            let (lf, ld) = (vf.max(from), (vf + vd).min(from + dur) - vf.max(from));
            let line = self.line_of(vid, secs(vc.start) + lf, secs(vc.start) + lf + ld)?;
            let fps = (self.doc.project.fps as f64).max(1.0);
            let style = lipsync::style_of(op, fps, id as u64)?;
            let start = secs(vc.start) + lf;
            let synced = lipsync::sync_line(&line, &style);
            // Which mouth layer shows when: `(time, index into mouths)`.
            let rest_at = |chart: &[speech::Viseme]| speech::Viseme::Rest.fallbacks().iter().find_map(|v| chart.iter().position(|c| c == v)).unwrap_or(0);
            let chart: Vec<speech::Viseme> = mouths.iter().map(|m| m.1).collect();
            let mut seq: Vec<(f64, usize)> = vec![(t0, rest_at(&chart))];
            let how = match &synced {
                Some(sy) => {
                    for (t, i) in speech::drive::chart_sequence(&sy.curves, &chart, fps) {
                        if seq.last().is_some_and(|l| l.1 != i) {
                            seq.push((start + t, i));
                        }
                    }
                    sy.how.clone()
                }
                None => {
                    // No words: the older reading of the sound alone, four shapes.
                    let poses = face::lip_sync(&line.mono, line.rate, &line.words);
                    let shapes: Vec<face::Shape> = chart.iter().map(|v| lipsync::shape_of(*v)).collect();
                    let pick = |s: face::Shape| -> usize {
                        let order: &[face::Shape] = match s {
                            face::Shape::Closed => &[face::Shape::Closed, face::Shape::Round, face::Shape::Open],
                            face::Shape::Open => &[face::Shape::Open, face::Shape::Wide, face::Shape::Round],
                            face::Shape::Wide => &[face::Shape::Wide, face::Shape::Open, face::Shape::Round],
                            face::Shape::Round => &[face::Shape::Round, face::Shape::Open, face::Shape::Wide],
                        };
                        order.iter().find_map(|o| shapes.iter().position(|s| s == o)).unwrap_or(0)
                    };
                    seq[0].1 = pick(face::Shape::Closed);
                    for p in &poses {
                        let i = pick(face::Shape::of(p));
                        if seq.last().is_some_and(|l| l.1 != i) {
                            seq.push((start + p.t / line.pace, i));
                        }
                    }
                    "shaped by the sound alone (no words beside the file)".to_owned()
                }
            };
            let end_rest = seq[0].1;
            seq.push((t1, end_rest));
            for (mi, &(cid, _)) in mouths.iter().enumerate() {
                let keys: Vec<(f64, m::Value, m::Interp)> = seq.iter().map(|&(t, i)| (t, m::Value::Scalar(if i == mi { 1.0 } else { 0.0 }), m::Interp::Hold)).collect();
                put(self, cid, "transform.opacity", keys)?;
            }
            said.push(format!("lip sync to clip {vid}: {} mouth change(s) across {} mouth layer(s), {how}", seq.len().saturating_sub(2), mouths.len()));
        }
        if let Some(every) = blink_every {
            let starts: Vec<f64> = face::blinks(id as u64 ^ micros(from) as u64, dur, every, &[]).into_iter().map(|t| t0 + t).collect();
            let lids: Vec<u32> = parts.iter().filter(|p| p.1 == Part::Lid).map(|p| p.0).collect();
            let eyes: Vec<u32> = parts.iter().filter(|p| matches!(p.1, Part::Eye | Part::Pupil)).map(|p| p.0).collect();
            if lids.is_empty() && eyes.is_empty() {
                return Err(format!("clip {id} has no eye or lid layers to blink: name them \"eye\", \"lid\", \"pupil\", or give \"parts\": {{\"eyes\": [ids], \"lids\": [ids]}}"));
            }
            for &cid in &lids {
                let mut keys = vec![(t0, m::Value::Scalar(0.0), m::Interp::Hold)];
                for &s in &starts {
                    keys.push((s + 0.04, m::Value::Scalar(1.0), m::Interp::Hold));
                    keys.push((s + 0.16, m::Value::Scalar(0.0), m::Interp::Hold));
                }
                put(self, cid, "transform.opacity", keys)?;
            }
            if lids.is_empty() {
                for &cid in &eyes {
                    let base = self.model_clip(cid).and_then(|c| c.get_property("transform.scale_y")).and_then(|v| if let m::Value::Scalar(s) = v { Some(s) } else { None }).unwrap_or(1.0);
                    let mut keys = vec![(t0, m::Value::Scalar(base), m::Interp::Linear)];
                    for &s in &starts {
                        keys.push((s, m::Value::Scalar(base), m::Interp::Linear));
                        keys.push((s + 0.07, m::Value::Scalar(base * 0.08), m::Interp::Linear));
                        keys.push((s + 0.1, m::Value::Scalar(base * 0.08), m::Interp::Linear));
                        keys.push((s + face::BLINK, m::Value::Scalar(base), m::Interp::Linear));
                    }
                    put(self, cid, "transform.scale_y", keys)?;
                }
            }
            said.push(format!("{} blink(s) on {} layer(s)", starts.len(), if lids.is_empty() { eyes.len() } else { lids.len() }));
        }
        if let Some(target) = look {
            let pupils: Vec<u32> = parts.iter().filter(|p| p.1 == Part::Pupil).map(|p| p.0).collect();
            if pupils.is_empty() {
                return Err(format!("clip {id} has no pupil layers to look with: name them \"pupil\", or give \"parts\": {{\"pupils\": [ids]}}"));
            }
            let tm = micros((t0 + t1) / 2.0);
            for &cid in &pupils {
                let map = self.layer_map(cid, tm)?;
                let size = map.pixels();
                let c = map.canvas(m::Vec2::new(0.5, 0.5));
                let dir = match target {
                    Value::Number(n) => {
                        let tid = n.as_u64().unwrap_or(0) as u32;
                        let tc = self.clip(tid)?;
                        let tmap = self.layer_map(tid, tm.clamp(tc.start, tc.start + tc.dur))?;
                        let p = tmap.canvas(tmap.content(0.0, -0.4));
                        (p.x - c.x, (p.y - c.y) * 0.5)
                    }
                    Value::String(s) => match s.as_str() {
                        "left" => (-1.0, 0.0),
                        "right" => (1.0, 0.0),
                        "up" => (0.0, -1.0),
                        "down" => (0.0, 1.0),
                        _ => (0.0, 0.0),
                    },
                    _ => return Err("look_at is a clip id or camera, left, right, up, down".into()),
                };
                let l = (dir.0 * dir.0 + dir.1 * dir.1).sqrt().max(1e-3);
                let reach = 0.45 * size.x.max(size.y) as f64;
                let mc = self.model_clip(cid).ok_or("clip gone")?;
                let base = match mc.get_property("transform.position") {
                    Some(m::Value::Point(p)) => p,
                    _ => m::Vec2::new(0.0, 0.0),
                };
                // In the parent's own units the slide is close enough for
                // an eye; pupils are rarely scaled apart from their eyes.
                let to = m::Vec2::new(base.x + (dir.0 / l) as f64 * reach, base.y + (dir.1 / l) as f64 * reach * 0.6);
                let mut keys = vec![(t0, m::Value::Point(base), m::Interp::Linear)];
                if dur > 0.6 {
                    keys.push((t0 + 0.25, m::Value::Point(to), m::Interp::Linear));
                    keys.push((t1 - 0.25, m::Value::Point(to), m::Interp::Linear));
                }
                keys.push((t1, m::Value::Point(base), m::Interp::Linear));
                put(self, cid, "transform.position", keys)?;
            }
            said.push(format!("{} pupil layer(s) look toward {target}", pupils.len()));
        }
        Ok(said)
    }
}

/// A character's part layer.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Part {
    Mouth(speech::Viseme),
    Eye,
    Lid,
    Pupil,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_character_of_parts_swaps_mouths_with_its_voice_and_blinks() {
        let mut s = Session::new(crate::services::project::new_custom("Parts", 640, 360, 30.0));
        let card = |s: &mut Session, colour: &str| {
            s.edit(&[json!({"op": "card", "colour": colour, "at": 0, "duration": 3, "width": 100, "height": 100})], "card", false).unwrap();
            s.doc.project.clips.iter().map(|c| c.id).max().unwrap()
        };
        let body = card(&mut s, "#ffcc00");
        let mut parts = Vec::new();
        for name in ["mouth closed", "mouth open", "lid"] {
            let c = card(&mut s, "#330000");
            let mut mc = s.model_clip(c).unwrap();
            mc.name = name.into();
            let seq = s.doc.seq();
            s.doc.apply("name", m::Edit::SetClip { seq, clip: Box::new(mc) });
            s.edit(&[json!({"op": "parent", "clip": c, "to": body})], "parent", false).unwrap();
            parts.push(c);
        }
        // A line of four syllables.
        let rate = 16_000u32;
        let mut mono = vec![0.0f32; rate as usize * 2];
        for (a, b) in [(0.1, 0.3), (0.45, 0.65), (0.8, 1.0), (1.4, 1.7)] {
            for i in (a * rate as f64) as usize..(b * rate as f64) as usize {
                let t = i as f64 / rate as f64;
                mono[i] = (0.4 * (std::f64::consts::PI * (t - a) / (b - a)).sin() * (std::f64::consts::TAU * 150.0 * t).sin()) as f32;
            }
        }
        let dir = std::env::temp_dir().join(format!("studio-talk-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("line.wav");
        let wav = studio_media::wav::Wav { sample_rate: rate, channels: 1, samples: mono };
        std::fs::write(&path, studio_media::wav::encode_pcm16(&wav)).unwrap();
        let report = crate::services::project::import_media(&mut s.doc, &[path]);
        let media = report.ids[0];
        s.edit(&[json!({"op": "place", "media": media, "at": 0.5, "lane": "voice"})], "line", false).unwrap();
        let voice = s.doc.project.clips.iter().map(|c| c.id).max().unwrap();
        let r = s.edit(&[json!({"op": "puppet", "clip": body, "voice": voice, "blink": 1.0})], "talk", false).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let r = r.to_string();
        assert!(r.contains("lip sync") && r.contains("blink"), "{r}");
        let opacity = |c: u32| s.model_clip(c).unwrap().keyframes.get("transform.opacity").map(|t| t.keys.iter().map(|k| (k.time, k.value.clone())).collect::<Vec<_>>()).unwrap_or_default();
        let shows = |c: u32| opacity(c).iter().filter(|k| k.1 == m::Value::Scalar(1.0)).count();
        assert!(shows(parts[1]) >= 3, "the open mouth shows for syllables: {:?}", opacity(parts[1]));
        assert!(shows(parts[0]) >= 3, "the shut mouth comes back between: {:?}", opacity(parts[0]));
        // Never both mouths at once.
        let open = s.model_clip(parts[1]).unwrap();
        let shut = s.model_clip(parts[0]).unwrap();
        for t in (0..60).map(|i| i as m::Micros * 50_000) {
            let o = |c: &m::Clip| match c.property_at("transform.opacity", t) {
                Some(m::Value::Scalar(v)) => v,
                _ => 1.0,
            };
            assert!((o(&open) + o(&shut) - 1.0).abs() < 1e-9, "one mouth at {t}");
        }
        assert!(shows(parts[2]) >= 1, "the lid closes for a blink: {:?}", opacity(parts[2]));
    }

    #[test]
    fn a_picture_gets_face_pins_a_talking_mouth_and_blinks() {
        // A round tan face with two eyes and a mouth line, on transparency.
        let (w, h) = (200usize, 200usize);
        let mut px = vec![0u8; w * h * 4];
        let mut put = |x: usize, y: usize, c: [u8; 4]| px[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&c);
        for y in 0..h {
            for x in 0..w {
                let (dx, dy) = (x as f64 - 100.0, y as f64 - 105.0);
                if (dx / 80.0).powi(2) + (dy / 85.0).powi(2) < 1.0 {
                    put(x, y, [200, 150, 90, 255]);
                }
                for ex in [70.0, 130.0] {
                    let (ex, ey) = (x as f64 - ex, y as f64 - 80.0);
                    if (ex / 14.0).powi(2) + (ey / 16.0).powi(2) < 1.0 {
                        put(x, y, if ex * ex + ey * ey < 49.0 { [20, 15, 10, 255] } else { [245, 245, 240, 255] });
                    }
                }
            }
        }
        for x in 82..120 {
            let y = 140 + ((x as f64 - 100.0).powi(2) / 120.0) as usize;
            put(x, y, [90, 40, 30, 255]);
            put(x, y + 1, [90, 40, 30, 255]);
        }
        let dir = std::env::temp_dir().join(format!("studio-face-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pic = dir.join("face.png");
        {
            let mut e = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(&pic).unwrap()), w as u32, h as u32);
            e.set_color(png::ColorType::Rgba);
            e.set_depth(png::BitDepth::Eight);
            e.write_header().unwrap().write_image_data(&px).unwrap();
        }
        let rate = 16_000u32;
        let mut mono = vec![0.0f32; rate as usize * 2];
        for (a, b) in [(0.1, 0.3), (0.45, 0.65), (0.8, 1.0)] {
            for i in (a * rate as f64) as usize..(b * rate as f64) as usize {
                let t = i as f64 / rate as f64;
                mono[i] = (0.4 * (std::f64::consts::PI * (t - a) / (b - a)).sin() * (std::f64::consts::TAU * 150.0 * t).sin()) as f32;
            }
        }
        let line = dir.join("line.wav");
        std::fs::write(&line, studio_media::wav::encode_pcm16(&studio_media::wav::Wav { sample_rate: rate, channels: 1, samples: mono })).unwrap();
        let mut s = Session::new(crate::services::project::new_custom("Face", 640, 360, 30.0));
        let ids = crate::services::project::import_media(&mut s.doc, &[pic, line]).ids;
        s.edit(&[json!({"op": "place", "media": ids[0], "at": 0, "lane": "overlay"})], "face", false).unwrap();
        let id = s.doc.project.clips.iter().map(|c| c.id).max().unwrap();
        s.edit(&[json!({"op": "trim", "clip": id, "duration": 3})], "trim", false).ok();
        s.edit(&[json!({"op": "place", "media": ids[1], "at": 0.5, "lane": "voice"})], "line", false).unwrap();
        let voice = s.doc.project.clips.iter().map(|c| c.id).max().unwrap();
        let r = s.edit(&[json!({"op": "puppet", "clip": id, "rig": "object", "act": ["breathe", "talk"], "voice": voice, "look_at": "left"})], "talk", false).unwrap().to_string();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(r.contains("found 2 eye(s)") && r.contains("lip sync") && r.contains("blink"), "{r}");
        let mc = s.model_clip(id).unwrap();
        let pins = mc.puppet.as_ref().unwrap().pins.clone();
        let fp = face_pins(&pins);
        assert_eq!(fp.iter().filter(|p| matches!(p.1, Role::Pupil(_))).count(), 2);
        let jaw = fp.iter().find(|p| p.1 == Role::Jaw).unwrap();
        // The jaw drops on syllables and is back by the end.
        let drop = |t: f64| pin_at(&mc, jaw.0, micros(t)).unwrap().y - jaw.2.y - head_moved(&mc, micros(t)).1;
        assert!((0..60).any(|i| drop(0.5 + i as f64 * 0.025) > 0.005), "the jaw never drops");
        assert!(drop(2.9).abs() < 1e-6);
        // The pupils look left while the lids stay put outside blinks.
        let pupil = fp.iter().find(|p| matches!(p.1, Role::Pupil(_))).unwrap();
        let slide = pin_at(&mc, pupil.0, micros(1.5)).unwrap().x - pupil.2.x - head_moved(&mc, micros(1.5)).0;
        assert!(slide < -0.005, "pupil slid {slide}");
        // The dark inside of the mouth opens with it.
        let inside = s.doc.project.clips.iter().find(|c| c.name.ends_with("mouth inside")).expect("a mouth inside");
        let ic = s.model_clip(inside.id).unwrap();
        let sy = &ic.keyframes["transform.scale_y"];
        assert!(sy.keys.iter().any(|k| matches!(k.value, m::Value::Scalar(v) if v > 0.05)) && sy.keys.iter().any(|k| k.value == m::Value::Scalar(0.0)));
        // Acting again keeps the face on top.
        s.edit(&[json!({"op": "puppet", "clip": id, "act": "nod", "from": 0.5, "duration": 1.5})], "nod", false).unwrap();
        let mc = s.model_clip(id).unwrap();
        let drop = |t: f64| pin_at(&mc, jaw.0, micros(t)).unwrap().y - jaw.2.y - head_moved(&mc, micros(t)).1;
        assert!((0..60).any(|i| drop(0.5 + i as f64 * 0.025) > 0.005), "the nod wiped the lips");
    }

    #[test]
    fn face_pins_are_known_by_name() {
        assert_eq!(role("face eye2 lid"), Some(Role::Lid(2)));
        assert_eq!(role("face eye1"), Some(Role::Pupil(1)));
        assert_eq!(role("face mouth right"), Some(Role::Corner(true)));
        assert_eq!(role("face mouth"), Some(Role::Jaw));
        assert_eq!(role("head"), None);
        assert_eq!(role("face eyebrow"), None);
    }

    #[test]
    fn an_open_mouth_drops_the_jaw_and_round_pulls_the_corners_in() {
        let open = face::Pose { t: 0.0, open: 1.0, wide: 0.0 };
        assert!(mouth_move(Role::Jaw, &open, 0.1, 1.0).1 > 0.02);
        assert!(mouth_move(Role::Lip, &open, 0.1, 1.0).1 < 0.0);
        let round = face::Pose { t: 0.0, open: 0.5, wide: -1.0 };
        let wide = face::Pose { t: 0.0, open: 0.5, wide: 1.0 };
        assert!(mouth_move(Role::Corner(true), &round, 0.1, 1.0).0 < 0.0 && mouth_move(Role::Corner(false), &round, 0.1, 1.0).0 > 0.0);
        assert!(mouth_move(Role::Corner(true), &wide, 0.1, 1.0).0 > 0.0);
        let shut = face::Pose { t: 0.0, open: 0.0, wide: 0.0 };
        assert_eq!(mouth_move(Role::Jaw, &shut, 0.1, 1.0), (0.0, 0.0));
        let ps = [face::Pose { t: 0.0, open: 0.0, wide: 0.0 }, face::Pose { t: 1.0, open: 1.0, wide: 0.5 }];
        assert!((pose_at(&ps, 0.5).open - 0.5).abs() < 1e-9 && pose_at(&ps, 2.0).open == 1.0);
        assert_eq!(gaze_at(0.0, 0.0, 2.0), 0.0);
        assert_eq!(gaze_at(1.0, 0.0, 2.0), 1.0);
    }
}
