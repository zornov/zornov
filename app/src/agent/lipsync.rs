// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Lip sync: `lip_sync {clip, audio}` makes a character's mouth say a line.
//! The line's words become phonemes with times (laid on the audio), the
//! phonemes become mouth channels that blend, lead the sound and hold lip
//! closures, and the channels drive whatever the character has:
//! - a rigged 3D model: face shape keys (jaw, lip closure, pucker,
//!   stretch, tongue, blinks, brows) found by name whatever the case or
//!   separators, one key per viseme if that is how it is built, and the
//!   jaw, tongue and eye bones; all as ordinary keys on the clip
//!   (`generator.shape.*`, `generator.bone.*`) that show in the timeline
//!   and graph;
//! - a 2D puppet (mouth pins) or a character of parts (a drawing per
//!   viseme): through `puppet`'s `voice`, which uses the same timing.
//! `lip_sync` with `check` reads the mouth back against the voice and
//! flags what is wrong, each with its fix.

use super::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use studio_speech::check as sync_check;
use studio_speech::features;
use studio_speech::{Alignment, Curves, Style};

/// The mouth chart's drawings, for messages.
pub(super) const CHART: &str = "rest (closed), MBP, FV, TH, L, R, WOO (w, oo), CHSHJ, SZ, AA (open), AE (wide), AH, EE, IH, OH (round), UH, ER, etc (also closed, open, wide, round)";

/// The older four mouth shapes a viseme reads as.
pub(super) fn shape_of(v: speech::Viseme) -> face::Shape {
    use speech::Viseme::*;
    match v {
        Rest | MBP => face::Shape::Closed,
        AA | AH | Etc | L | TH | FV | ER => face::Shape::Open,
        AE | EE | IH | SZ => face::Shape::Wide,
        WOO | OH | UH | R | CHSHJ => face::Shape::Round,
    }
}

/// How an op wants the mouth to move: `intensity`, `emotion`, `lead`
/// (seconds the mouth leads the sound), `offset` (seconds later), and
/// `extras` false to leave out blinks, brows and eye movement.
pub(super) fn style_of(op: &Value, fps: f64, seed: u64) -> Result<Style, String> {
    let mut st = Style { fps, seed, ..Style::default() };
    if let Some(v) = op.get("intensity") {
        st.intensity = v.as_f64().ok_or("intensity is a number: 1 natural, 0.5 mumbled, 1.5 theatrical")?.clamp(0.0, 3.0) as f32;
    }
    if let Some(v) = op.get("emotion") {
        let n = v.as_str().ok_or("emotion is a word: neutral, happy, sad, angry, surprised, scared")?;
        st.emotion = speech::Emotion::named(n).ok_or_else(|| format!("emotion \"{n}\": neutral, happy, sad, angry, surprised or scared"))?;
    }
    if let Some(v) = op.get("lead") {
        st.lead = v.as_f64().ok_or("lead is seconds the mouth leads the sound (0.04 to 0.08)")?.clamp(0.0, 0.2);
    }
    if let Some(v) = op.get("offset") {
        st.offset = v.as_f64().ok_or("offset is seconds to shift the mouth later (negative: earlier)")?.clamp(-1.0, 1.0);
    }
    if op.get("extras") == Some(&Value::Bool(false)) {
        st.extras = false;
    }
    Ok(st)
}

/// A line's phonemes and mouth channels.
pub(super) struct Synced {
    pub curves: Curves,
    /// Phonemes, in seconds of the clip's time (the line's speed applied).
    pub phones: Vec<speech::Timed>,
    pub how: String,
}

fn cache() -> &'static Mutex<HashMap<u64, Arc<Alignment>>> {
    static C: OnceLock<Mutex<HashMap<u64, Arc<Alignment>>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

fn key_of(line: &Line, words: &[speech::WordIn]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    line.path.hash(&mut h);
    line.f0.to_bits().hash(&mut h);
    line.mono.len().hash(&mut h);
    for w in words {
        w.text.hash(&mut h);
        w.start.map(f64::to_bits).hash(&mut h);
    }
    h.finish()
}

/// The phonemes of a line: kept beside the sound file when the narration
/// made them, else laid on the audio from its words (heard by speech
/// recognition when `listen`, else the words file beside it).
pub(super) fn align_line(line: &Line, listen: bool) -> Option<(Arc<Alignment>, usize, &'static str)> {
    let len = line.mono.len() as f64 / line.rate as f64;
    // Phonemes kept with the file.
    if let Some(a) = std::fs::read_to_string(line.path.with_extension("phonemes.tsv")).ok().and_then(|t| Alignment::from_tsv(&t)) {
        let phones: Vec<speech::Timed> = a
            .phones
            .iter()
            .filter(|p| p.end > line.f0 && p.start < line.f0 + len)
            .map(|p| speech::Timed { start: (p.start - line.f0).max(0.0), end: (p.end - line.f0).min(len), ..*p })
            .collect();
        if phones.len() > 2 {
            let n = phones.iter().filter_map(|p| p.word).collect::<std::collections::BTreeSet<_>>().len();
            let words = (0..a.words.len()).map(|w| phones.iter().filter(|p| p.word == Some(w)).fold(None, |acc: Option<(f64, f64)>, p| Some(acc.map_or((p.start, p.end), |x| (x.0.min(p.start), x.1.max(p.end)))))).collect();
            return Some((Arc::new(Alignment { phones, words, method: a.method }), n, "phonemes kept with the narration"));
        }
    }
    let mut words: Vec<speech::WordIn> = line.words.iter().map(|(t, s, e)| speech::WordIn::timed(t, s.max(0.0), e.min(len))).filter(|w| w.end.unwrap_or(0.0) > w.start.unwrap_or(0.0)).collect();
    let mut heard = "phonemes laid on the audio from the words beside it";
    if words.is_empty() && listen {
        words = super::super::super::voice::words_of(&line.mono, line.rate).into_iter().map(|w| speech::WordIn::timed(&w.text, w.start, w.end)).collect();
        heard = "phonemes laid on the audio from words heard by speech recognition";
    }
    if words.is_empty() {
        return None;
    }
    let key = key_of(line, &words);
    if let Some(a) = cache().lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return Some((a.clone(), words.len(), heard));
    }
    let a = Arc::new(speech::align(&line.mono, line.rate, &words).ok()?);
    let mut c = cache().lock().unwrap_or_else(|e| e.into_inner());
    if c.len() > 64 {
        c.clear();
    }
    c.insert(key, a.clone());
    Some((a, words.len(), heard))
}

/// The mouth channels for a line, or `None` when it has no words to go by.
pub(super) fn sync_line(line: &Line, st: &Style) -> Option<Synced> {
    sync_line_with(line, st, false)
}

pub(super) fn sync_line_with(line: &Line, st: &Style, listen: bool) -> Option<Synced> {
    let (al, nwords, heard) = align_line(line, listen)?;
    let pace = line.pace.max(0.05);
    let len = line.mono.len() as f64 / line.rate as f64;
    let phones: Vec<speech::Timed> = al.phones.iter().map(|p| speech::Timed { start: p.start / pace, end: p.end / pace, ..*p }).collect();
    let loud = if (pace - 1.0).abs() < 0.01 { features::analyse(&line.mono, line.rate).loud } else { Vec::new() };
    let curves = speech::curves(&phones, len / pace, st, &loud);
    let method = match al.method {
        speech::Method::Acoustic => "timed on the sound",
        speech::Method::Estimated => "timed by typical lengths inside the word times",
        speech::Method::Engine => "timed by the voice engine",
    };
    let how = format!(
        "{} phonemes in {nwords} words ({heard}; {method}); mouth leads the sound by {:.0} ms, M/B/P hold shut {} frames",
        phones.iter().filter(|p| p.ph != speech::Ph::Sil).count(),
        st.lead * 1000.0,
        st.min_closure_frames
    );
    Some(Synced { curves, phones, how })
}

/// Keys of a curve in clip time: at most `tol` off, within `[0, dur]`.
fn clip_keys(keys: &[(f64, f32)], dur: f64, lf: f64) -> Vec<(f64, f32)> {
    let value_at = |t: f64| -> f32 {
        match keys.iter().position(|k| k.0 > t) {
            Some(0) => keys[0].1,
            Some(i) => {
                let (a, b) = (keys[i - 1], keys[i]);
                a.1 + (b.1 - a.1) * ((t - a.0) / (b.0 - a.0).max(1e-9)) as f32
            }
            None => keys.last().map_or(0.0, |k| k.1),
        }
    };
    let mut out: Vec<(f64, f32)> = vec![(lf, value_at(0.0))];
    out.extend(keys.iter().filter(|k| k.0 > 1e-6 && k.0 < dur - 1e-6).map(|k| (lf + k.0, k.1)));
    out.push((lf + dur, value_at(dur)));
    out
}

/// Replaces a track's keys inside `[a, b]` (clip micros) with `keys`.
fn replace_window(mc: &mut m::Clip, path: &str, a: m::Micros, b: m::Micros, keys: Vec<(m::Micros, m::Value)>, how: m::Interp) {
    let kept: Vec<m::Keyframe> = mc.keyframes.get(path).map(|tr| tr.keys.iter().filter(|k| k.time < a - 1000 || k.time > b + 1000).cloned().collect()).unwrap_or_default();
    if kept.is_empty() {
        mc.keyframes.remove(path);
    } else {
        mc.keyframes.insert(path.to_owned(), m::Track::new(kept));
    }
    for (t, v) in keys {
        super::super::super::anim3d::key(mc, path, t, v, how);
    }
}

/// Which axis and direction of rotation drops a jaw bone's tip.
fn jaw_turn(rig: &studio_mesh::rig::Rig, bone: &str) -> (char, f32) {
    use studio_mesh::rig::PoseInput;
    let Some(bi) = rig.bone(bone) else { return ('x', 1.0) };
    let mut best = ('x', 1.0f32, f32::MIN);
    for (ax, idx) in [('x', 0), ('y', 1), ('z', 2)] {
        for sign in [1.0f32, -1.0] {
            let mut d = [0.0f32; 3];
            d[idx] = 10.0 * sign;
            let mut input = PoseInput::default();
            input.rotations.insert(bone.to_owned(), d);
            let posed = rig.evaluate(&input);
            let rest = rig.rest_world();
            let len = rig.bones[bi].length();
            let tip_rest = rest[bi].apply([0.0, len, 0.0]);
            let tip = posed.world[bi].apply([0.0, len, 0.0]);
            // Down in the model is -y.
            let drop = tip_rest[1] - tip[1];
            if drop > best.2 {
                best = (ax, sign, drop);
            }
        }
    }
    (best.0, best.1)
}

/// Where voice clip `vid` is for clip `id`: the audio clip (voice lane
/// first) that overlaps it most.
fn voice_for(s: &Session, id: u32, op: &Value) -> Result<u32, String> {
    if let Some(v) = op.get("audio").or_else(|| op.get("voice")).and_then(Value::as_u64) {
        return Ok(v as u32);
    }
    let p = &s.doc.project;
    let c = p.clip(id).ok_or_else(|| format!("No clip {id}"))?;
    let overlap = |a: &view::Clip| (a.start + a.dur).min(c.start + c.dur) - a.start.max(c.start);
    p.clips
        .iter()
        .filter(|a| a.kind == ClipKind::Audio && a.id != id && overlap(a) > SEC / 4)
        .max_by_key(|a| (p.lane(a.lane).is_some_and(|l| l.name.to_lowercase().contains("voice")), overlap(a)))
        .map(|a| a.id)
        .ok_or_else(|| format!("no voice line plays under clip {id}: give \"audio\": <the line's clip id> (narrate makes one)"))
}

impl Session {
    /// `lip_sync`: a character says a line.
    pub(in crate::agent) fn lip_sync(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or("lip_sync needs \"clip\" (the character) and \"audio\" (the line's clip; the voice under the character if left out)")? as u32;
        let vid = voice_for(self, id, op)?;
        if op.get("check") == Some(&Value::Bool(true)) {
            return self.lip_check(op, id, vid);
        }
        if self.is_rigged_model(op) {
            return self.lip_sync3d(op, id, vid);
        }
        // A 2D character: puppet pins or a mouth chart.
        let mut p = json!({"op": "puppet", "clip": id, "voice": vid});
        for k in ["from", "duration", "intensity", "emotion", "lead", "offset", "extras", "parts", "face", "look_at", "blink"] {
            if let Some(v) = op.get(k) {
                p[k] = v.clone();
            }
        }
        if p.get("blink").is_none() {
            p["blink"] = json!(true);
        }
        let said = self.puppet(&p)?;
        let check = self.lip_check(op, id, vid).map(|c| format!("; check: {c}")).unwrap_or_default();
        Ok(format!("{said}{check}"))
    }

    fn lip_sync3d(&mut self, op: &Value, id: u32, vid: u32) -> Result<String, String> {
        let (mut mc, _, rig) = self.rig_of(id)?;
        let vc = self.clip(id)?;
        let fps = (self.doc.project.fps as f64).max(1.0);
        let (vf, vd) = self.voice_window(id, vid)?;
        let from = f(op, "from").unwrap_or(0.0).max(0.0);
        let to = f(op, "duration").map(|d| from + d).unwrap_or(f64::INFINITY);
        let (lf, end) = (vf.max(from), (vf + vd).min(to));
        let ld = end - lf;
        if ld < 0.2 {
            return Err(format!("voice clip {vid} plays outside the window"));
        }
        let line = self.line_of(vid, secs(vc.start) + lf, secs(vc.start) + lf + ld)?;
        let st = style_of(op, fps, id as u64)?;
        let sy = sync_line_with(&line, &st, true).ok_or("the line has no words to go by: narrate it (which keeps its phonemes), install speech recognition, or give a words file beside the sound")?;
        let names: Vec<String> = rig.shapes.iter().map(|k| k.name.clone()).collect();
        let bones: Vec<String> = rig.bones.iter().map(|b| b.name.clone()).collect();
        let d = speech::drive::drive3(&sy.curves, &names, &bones, 0.015);
        let (a, b) = (micros(lf), micros(lf + ld));
        let ease = m::Interp::Linear;
        let mut nkeys = 0;
        let mut driven: Vec<String> = Vec::new();
        for (name, keys) in &d.shapes {
            let ks = clip_keys(keys, ld, lf);
            nkeys += ks.len();
            driven.push(name.clone());
            let ks: Vec<(m::Micros, m::Value)> = ks.into_iter().map(|(t, v)| (micros(t), m::Value::Scalar(v as f64))).collect();
            replace_window(&mut mc, &format!("generator.shape.{name}"), a, b, ks, ease);
        }
        if let Some((bone, keys)) = &d.jaw {
            let (ax, sign) = jaw_turn(&rig, bone);
            let ks: Vec<(m::Micros, m::Value)> = clip_keys(keys, ld, lf).into_iter().map(|(t, v)| (micros(t), m::Value::Angle((v * sign) as f64))).collect();
            nkeys += ks.len();
            driven.push(format!("{bone} bone"));
            replace_window(&mut mc, &format!("generator.bone.{bone}.{ax}"), a, b, ks, ease);
        }
        for (bone, which, keys) in &d.tongue_bones {
            // Up and curl turn the tongue about its own x; forward is left to shape keys.
            if *which == "forward" {
                continue;
            }
            let path = format!("generator.bone.{bone}.x");
            let existing: Vec<(m::Micros, m::Value)> = Vec::new();
            let _ = existing;
            let ks: Vec<(m::Micros, m::Value)> = clip_keys(keys, ld, lf).into_iter().map(|(t, v)| (micros(t), m::Value::Angle(v as f64))).collect();
            if *which == "up" {
                nkeys += ks.len();
                replace_window(&mut mc, &path, a, b, ks, ease);
            } else if bone.to_lowercase().contains("tip") || bone.ends_with("001") || bone.ends_with(".1") {
                nkeys += ks.len();
                replace_window(&mut mc, &path, a, b, ks, ease);
            }
        }
        for (bone, axis, keys) in &d.eye_bones {
            // Gaze: x is across (turn about y), y is up and down (turn about x).
            let ax = if *axis == "x" { 'y' } else { 'x' };
            let sign = if *axis == "y" { -1.0 } else { 1.0 };
            let ks: Vec<(m::Micros, m::Value)> = clip_keys(keys, ld, lf).into_iter().map(|(t, v)| (micros(t), m::Value::Angle((v * sign) as f64))).collect();
            nkeys += ks.len();
            replace_window(&mut mc, &format!("generator.bone.{bone}.{ax}"), a, b, ks, ease);
        }
        super::super::super::anim3d::set_param(&mut mc, "lipsync.intensity", m::Value::Scalar(st.intensity as f64));
        super::super::super::anim3d::set_param(&mut mc, "lipsync.offset", m::Value::Scalar(st.offset));
        let seq = self.doc.seq();
        let ok = self.doc.apply("Lip sync", m::Edit::SetClip { seq, clip: Box::new(mc) });
        self.check(ok, "")?;
        let mode = match d.mode {
            Some(speech::drive::Mode::Channels) => "jaw and lip shape keys apart",
            Some(speech::drive::Mode::Visemes) => "one shape key per viseme",
            Some(speech::drive::Mode::Jaw) => "jaw only (no mouth shape keys)",
            _ => "nothing to drive",
        };
        let mut out = format!("lip sync of clip {id} to clip {vid} from {lf:.2}s for {ld:.2}s: {}; drove {mode}: {} ({nkeys} keys, editable in the timeline and graph under generator.shape.* and generator.bone.*)", sy.how, driven.join(", "));
        if !d.missing.is_empty() {
            out += &format!("; this rig lacks: {}", d.missing.join(", "));
        }
        if d.mode == Some(speech::drive::Mode::None) {
            return Err(format!("clip {id} has no mouth to drive: it needs face shape keys (jawOpen, mouthClose, viseme_aa… any case) or a jaw bone; rig it as a person or add a ready character"));
        }
        if let Ok(c) = self.lip_check(op, id, vid) {
            out += &format!("; check: {c}");
        }
        Ok(out)
    }

    /// The mouth of a character read back: `(open, close)` at 100 samples
    /// a second from clip time `from` for `dur` seconds.
    fn read_mouth(&self, id: u32, from: f64, dur: f64) -> Result<(Vec<f32>, Option<Vec<f32>>), String> {
        let n = (dur * 100.0) as usize;
        let mc = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
        let scalar = |path: &str, t: f64| -> f32 {
            match mc.property_at(path, micros(t)) {
                Some(m::Value::Scalar(v)) => v as f32,
                Some(m::Value::Angle(v)) => v as f32,
                _ => 0.0,
            }
        };
        if self.is_rigged_model(&json!({"clip": id})) {
            let (_, _, rig) = self.rig_of(id)?;
            let names: Vec<String> = rig.shapes.iter().map(|k| k.name.clone()).collect();
            let bones: Vec<String> = rig.bones.iter().map(|b| b.name.clone()).collect();
            let on = speech::drive::open_names(&names, &bones);
            let jt = on.jaw_bone.as_ref().map(|b| (b.clone(), jaw_turn(&rig, b)));
            let at = |t: f64| -> (f32, f32) {
                if !on.jaw.is_empty() {
                    let j = on.jaw.iter().map(|n| scalar(&format!("generator.shape.{n}"), t)).fold(0.0, f32::max);
                    let c = on.close.iter().map(|n| scalar(&format!("generator.shape.{n}"), t)).fold(0.0, f32::max);
                    (j * (1.0 - c), c)
                } else if !on.visemes.is_empty() {
                    let w = |v: speech::Viseme| on.visemes.iter().find(|x| x.0 == v).map_or(0.0, |x| scalar(&format!("generator.shape.{}", x.1), t));
                    let close = w(speech::Viseme::MBP);
                    let open = [speech::Viseme::AA, speech::Viseme::AE, speech::Viseme::AH, speech::Viseme::OH, speech::Viseme::EE, speech::Viseme::IH, speech::Viseme::UH, speech::Viseme::WOO].iter().map(|v| w(*v) * speech::viseme::lip_targets(*v)[0].0.max(0.2)).sum::<f32>();
                    (open.min(1.0) * (1.0 - close), close)
                } else if let Some((b, (ax, sign))) = &jt {
                    ((scalar(&format!("generator.bone.{b}.{ax}"), t) * sign / 16.0).clamp(0.0, 1.0), 0.0)
                } else {
                    (0.0, 0.0)
                }
            };
            let samples: Vec<(f32, f32)> = (0..n).map(|i| at(from + i as f64 / 100.0)).collect();
            let has_close = !on.close.is_empty() || on.visemes.iter().any(|v| v.0 == speech::Viseme::MBP);
            return Ok((samples.iter().map(|s| s.0).collect(), has_close.then(|| samples.iter().map(|s| s.1).collect())));
        }
        self.read_mouth2d(id, &mc, from, dur)
    }

    /// `lip_sync` with `check`: reads the mouth back against the voice.
    fn lip_check(&mut self, op: &Value, id: u32, vid: u32) -> Result<String, String> {
        let vc = self.clip(id)?;
        let fps = (self.doc.project.fps as f64).max(1.0);
        let (vf, vd) = self.voice_window(id, vid)?;
        let from = f(op, "from").unwrap_or(0.0).max(0.0);
        let to = f(op, "duration").map(|d| from + d).unwrap_or(f64::INFINITY);
        let (lf, end) = (vf.max(from), (vf + vd).min(to));
        let ld = end - lf;
        let line = self.line_of(vid, secs(vc.start) + lf, secs(vc.start) + lf + ld)?;
        let st = style_of(op, fps, id as u64)?;
        let sy = sync_line_with(&line, &st, true).ok_or("nothing to check against: the line has no words (narrate it, or install speech recognition)")?;
        let (open, close) = self.read_mouth(id, lf, ld)?;
        let mut frames = features::analyse(&line.mono, line.rate);
        if (line.pace - 1.0).abs() > 0.01 {
            // The sound plays at its own speed on the timeline.
            let n = (frames.len() as f64 / line.pace) as usize;
            let pick = |v: &Vec<f32>| -> Vec<f32> { (0..n).map(|i| v[((i as f64 * line.pace) as usize).min(v.len() - 1)]).collect() };
            frames = features::Frames { loud: pick(&frames.loud), hf: pick(&frames.hf), lf: pick(&frames.lf), voice: pick(&frames.voice), db: pick(&frames.db) };
        }
        let report = sync_check::check(&sync_check::Input { open: &open, close: close.as_deref(), audio: &frames, phones: &sy.phones, fps, lead: (st.lead - st.offset).max(0.0) });
        let t0 = secs(vc.start) + lf;
        if report.issues.is_empty() {
            return Ok(format!("mouth in sync{}", report.lag.map_or(String::new(), |l| format!(" (mouth {} the voice by {:.0} ms)", if l < 0.0 { "leads" } else { "trails" }, l.abs() * 1000.0))));
        }
        let list: Vec<String> = report
            .issues
            .iter()
            .take(8)
            .map(|i| {
                let at = if i.kind == sync_check::Kind::OutOfSync { String::new() } else { format!(" at {:.2}s", t0 + i.t0) };
                format!("{}{at}: {}. Fix: {}", i.kind.name(), i.detail, i.fix)
            })
            .collect();
        Ok(format!("{} problem(s): {}", report.issues.len(), list.join(" | ")))
    }
}

impl Session {
    /// A 2D character's mouth read back: from its mouth pins, else from
    /// the opacity of its mouth drawings.
    fn read_mouth2d(&self, id: u32, mc: &m::Clip, from: f64, dur: f64) -> Result<(Vec<f32>, Option<Vec<f32>>), String> {
        let n = (dur * 100.0) as usize;
        let fp = mc.puppet.as_ref().map(|p| face_pins(&p.pins)).unwrap_or_default();
        let find = |r: Role| fp.iter().find(|p| p.1 == r).map(|p| (p.0, p.2));
        if let (Some(jaw), Some(lip)) = (find(Role::Jaw), find(Role::Lip)) {
            let cs: Vec<f64> = fp.iter().filter(|p| matches!(p.1, Role::Corner(_))).map(|p| p.2.x).collect();
            let mw = if cs.len() == 2 { (cs[0] - cs[1]).abs() } else { 0.1 };
            let vc = self.clip(id)?;
            let px = self.layer_map(id, vc.start + micros(from + dur / 2.0))?.pixels();
            let aspect = (px.x / px.y.max(1.0)) as f64;
            let wy = (mw * aspect).max(1e-6);
            let open = (0..n)
                .map(|i| {
                    let t = micros(from + i as f64 / 100.0);
                    let h = head_moved(mc, t).1;
                    let dj = pin_at(mc, jaw.0, t).map_or(0.0, |p| p.y - jaw.1.y) - h;
                    let dl = pin_at(mc, lip.0, t).map_or(0.0, |p| p.y - lip.1.y) - h;
                    (((dj - dl) / (0.39 * wy)) as f32).clamp(0.0, 1.0)
                })
                .collect();
            return Ok((open, None));
        }
        let parts = self.parts_of(id, None)?;
        let mouths: Vec<(u32, speech::Viseme)> = parts.iter().filter_map(|(c, p)| if let Part::Mouth(v) = p { Some((*c, *v)) } else { None }).collect();
        if mouths.is_empty() {
            return Err(format!("clip {id} has no mouth pins or mouth drawings to check"));
        }
        let vc = self.clip(id)?;
        let t0 = secs(vc.start) + from;
        let starts: Vec<(i64, m::Clip, speech::Viseme)> = mouths.iter().filter_map(|(c, v)| Some((self.clip(*c).ok()?.start, self.model_clip(*c)?, *v))).collect();
        let mut open = Vec::with_capacity(n);
        let mut close = Vec::with_capacity(n);
        for i in 0..n {
            let t = micros(t0 + i as f64 / 100.0);
            let (mut o, mut c) = (0.0f32, 0.0f32);
            for (start, mclip, v) in &starts {
                let op = match mclip.property_at("transform.opacity", t - start) {
                    Some(m::Value::Scalar(x)) => x as f32,
                    _ => 1.0,
                };
                if op > 0.5 {
                    o = o.max(speech::viseme::lip_targets(*v)[0].0);
                    if matches!(v, speech::Viseme::MBP | speech::Viseme::Rest) {
                        c = 1.0;
                    }
                }
            }
            open.push(o);
            close.push(c);
        }
        Ok((open, Some(close)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services;
    use studio_media::RgbaImage;

    const TEXT: &str = "Mama bought a big puppy who could feel the cool moon five thin birds";

    /// A line spoken by the Mac voice, written as a WAV with its words file
    /// (timed by our own alignment) beside it. `None` where there is no `say`.
    fn spoken(dir: &std::path::Path) -> Option<(std::path::PathBuf, studio_speech::Alignment, f64)> {
        std::fs::create_dir_all(dir).ok()?;
        let raw = dir.join("said.wav");
        let ok = std::process::Command::new("say").arg("-o").arg(&raw).args(["--data-format=LEI16@22050", TEXT]).status().ok()?.success();
        if !ok {
            return None;
        }
        let b = std::fs::read(&raw).ok()?;
        let (mut i, mut data) = (12usize, None);
        while i + 8 <= b.len() {
            let len = u32::from_le_bytes(b[i + 4..i + 8].try_into().ok()?) as usize;
            if &b[i..i + 4] == b"data" {
                data = Some(&b[i + 8..(i + 8 + len).min(b.len())]);
                break;
            }
            i += 8 + len + (len & 1);
        }
        let mono: Vec<f32> = data?.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0).collect();
        let words: Vec<studio_speech::WordIn> = TEXT.split(' ').map(studio_speech::WordIn::plain).collect();
        let al = studio_speech::align(&mono, 22_050, &words).ok()?;
        let wav = studio_media::wav::Wav { sample_rate: 22_050, channels: 2, samples: mono.iter().flat_map(|s| [*s, *s]).collect() };
        let path = dir.join("line.wav");
        std::fs::write(&path, studio_media::wav::encode_pcm16(&wav)).ok()?;
        let said: Vec<crate::agent::voice::Said> = TEXT.split(' ').zip(&al.words).map(|(t, w)| { let w = w.unwrap(); crate::agent::voice::Said { text: t.into(), start: w.0, end: w.1 } }).collect();
        std::fs::write(path.with_extension("words.json"), serde_json::to_vec(&said).ok()?).ok()?;
        Some((path, al, mono.len() as f64 / 22_050.0))
    }

    fn sheet_dir() -> std::path::PathBuf {
        std::env::var_os("STUDIO_LIPSYNC_OUT").map(std::path::PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join("studio-lipsync-sheet"))
    }

    /// A square of `size` pixels centred on `(cx, cy)`.
    fn crop(im: &RgbaImage, cx: usize, cy: usize, size: usize) -> RgbaImage {
        let (x0, y0) = (cx - size / 2, cy - size / 2);
        let mut pixels = Vec::with_capacity(size * size * 4);
        for y in y0..y0 + size {
            let at = (y * im.width as usize + x0) * 4;
            pixels.extend_from_slice(&im.pixels[at..at + size * 4]);
        }
        RgbaImage { width: size as u32, height: size as u32, pixels }
    }

    /// Tiles pictures in rows of `cols` into one PNG.
    fn tile(images: &[RgbaImage], cols: usize, path: &std::path::Path) {
        let (w, h) = (images[0].width as usize, images[0].height as usize);
        let rows = images.len().div_ceil(cols);
        let mut out = vec![30u8; w * cols * h * rows * 4];
        for (k, im) in images.iter().enumerate() {
            let (cx, cy) = ((k % cols) * w, (k / cols) * h);
            for y in 0..h {
                let src = &im.pixels[y * w * 4..(y + 1) * w * 4];
                let at = ((cy + y) * w * cols + cx) * 4;
                out[at..at + w * 4].copy_from_slice(src);
            }
        }
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, studio_media::png::encode(&RgbaImage { width: (w * cols) as u32, height: (h * rows) as u32, pixels: out })).unwrap();
    }

    /// The moments worth a frame: the longest of each kind of sound.
    fn moments(al: &studio_speech::Alignment) -> Vec<(String, f64)> {
        use studio_speech::Ph;
        let mut out = Vec::new();
        for (label, want) in [("rest", &[Ph::Sil][..]), ("MBP", &[Ph::P, Ph::B, Ph::M][..]), ("AA", &[Ph::AA, Ph::AO][..]), ("EE", &[Ph::IY][..]), ("OO", &[Ph::UW][..]), ("FV", &[Ph::F, Ph::V][..]), ("TH", &[Ph::TH, Ph::DH][..]), ("IH", &[Ph::IH, Ph::AY][..])] {
            let best = al.phones.iter().filter(|p| want.contains(&p.ph)).max_by(|a, b| a.dur().total_cmp(&b.dur()));
            if let Some(p) = best {
                // The middle of it; a closure is caught by the middle of its first half.
                let t = if label == "MBP" { p.start + 0.4 * p.dur() } else { (p.start + p.end) / 2.0 };
                out.push((label.to_owned(), t));
            }
        }
        out
    }

    fn project_with_line(path: &std::path::Path, w: u32, h: u32) -> Session {
        let mut s = Session::new(services::project::new_custom("Lips", w, h, 30.0));
        let report = services::project::import_media(&mut s.doc, &[path.to_path_buf()]);
        let media = report.ids[0];
        s.edit(&[json!({"op": "place", "media": media, "at": 0.3, "lane": "voice"})], "line", false).unwrap();
        s
    }

    #[test]
    fn a_3d_face_says_a_line_and_the_check_passes() {
        let dir = sheet_dir().join("work3d");
        let Some((path, al, dur)) = spoken(&dir) else { return eprintln!("no say voice here: skipped") };
        let mut s = project_with_line(&path, 1920, 1920);
        let voice = s.doc.project.clips.iter().filter(|c| c.kind == ClipKind::Audio).map(|c| c.id).max().unwrap();
        s.edit(&[json!({"op": "model", "shape": "character", "size": 1260, "detail": 1, "duration": dur + 1.0, "at": 0})], "add", false).unwrap();
        let id = s.doc.project.clips.iter().filter(|c| c.id != voice).map(|c| c.id).max().unwrap();
        let r = s.edit(&[json!({"op": "lip_sync", "clip": id, "audio": voice})], "lip", false).unwrap().to_string();
        assert!(r.contains("phonemes") && r.contains("jawOpen") && r.contains("check:"), "{r}");
        let mc = s.model_clip(id).unwrap();
        let keys = |n: &str| mc.keyframes.get(&format!("generator.shape.{n}")).map(|t| t.keys.iter().map(|k| (k.time as f64 / 1e6, k.value.as_f64().unwrap_or(0.0))).collect::<Vec<_>>()).unwrap_or_default();
        let jaw = keys("jawOpen");
        assert!(jaw.len() > 20 && jaw.iter().any(|k| k.1 > 0.6), "jaw keys: {}", jaw.len());
        // The lips close on the P and B of the line: mouthClose is at 1 inside each plosive (clip time = line time + 0.3).
        let close = keys("mouthClose");
        let at = |k: &[(f64, f64)], t: f64| -> f64 {
            match k.iter().position(|x| x.0 > t) {
                Some(0) => k[0].1,
                Some(i) => k[i - 1].1 + (k[i].1 - k[i - 1].1) * (t - k[i - 1].0) / (k[i].0 - k[i - 1].0).max(1e-9),
                None => k.last().map_or(0.0, |x| x.1),
            }
        };
        let stops: Vec<_> = al.phones.iter().filter(|p| p.ph.is_bilabial_stop()).collect();
        assert!(stops.len() >= 5);
        for p in &stops {
            let peak = (0..20).map(|i| at(&close, 0.3 + p.start - 0.06 + i as f64 * (p.dur() + 0.06) / 20.0)).fold(0.0, f64::max);
            assert!(peak > 0.9, "{} at {:.2}s closes to {peak}", p.ph.name(), p.start);
        }
        // Blinks and brows come with it, on the face's own keys.
        assert!(mc.keyframes.contains_key("generator.shape.eyeBlinkLeft"));
        // The check reads it back: in sync, or says what is wrong with a fix.
        let c = s.edit(&[json!({"op": "lip_sync", "clip": id, "audio": voice, "check": true})], "check", false).unwrap().to_string();
        assert!(!c.contains("out of sync") && !c.contains("missed M/B/P"), "{c}");
        // A mouth shifted 8 frames is caught, with the offset that fixes it.
        s.edit(&[json!({"op": "lip_sync", "clip": id, "audio": voice, "offset": 0.27})], "late", false).unwrap();
        let c = s.edit(&[json!({"op": "lip_sync", "clip": id, "audio": voice, "check": true})], "check", false).unwrap().to_string();
        assert!(c.contains("out of sync") && c.contains("Fix:"), "{c}");
        // Back in step.
        s.edit(&[json!({"op": "lip_sync", "clip": id, "audio": voice})], "again", false).unwrap();
        // Frames for the contact sheet.
        let mut images = Vec::new();
        let mut note = String::new();
        for (label, t) in moments(&al) {
            let tl = 0.3 + t - 0.03;
            images.push(crop(&s.frames.rgba(&s.doc, micros(tl), 1920).unwrap(), 960, 408, 360));
            note += &format!("{label} at {tl:.2}s\n");
        }
        tile(&images, 4, &sheet_dir().join("3d-face.png"));
        std::fs::write(sheet_dir().join("3d-face.txt"), note).unwrap();
    }

    /// A cartoon face on transparency: a head, two eyes with pupils, a mouth line.
    fn cartoon(path: &std::path::Path) {
        let (w, h) = (300usize, 300usize);
        let mut px = vec![0u8; w * h * 4];
        let mut put = |x: usize, y: usize, c: [u8; 4]| px[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&c);
        for y in 20..290 {
            for x in 20..280 {
                if ((x as f64 - 150.0) / 125.0).powi(2) + ((y as f64 - 155.0) / 132.0).powi(2) < 1.0 {
                    put(x, y, [220, 170, 100, 255]);
                }
            }
        }
        for ex in [105.0, 195.0] {
            for y in 80..140 {
                for x in (ex as usize - 30)..(ex as usize + 30) {
                    let (dx, dy) = (x as f64 - ex, y as f64 - 110.0);
                    if (dx / 22.0).powi(2) + (dy / 25.0).powi(2) < 1.0 {
                        put(x, y, [248, 248, 244, 255]);
                    }
                    if dx * dx + dy * dy < 100.0 {
                        put(x, y, [20, 15, 10, 255]);
                    }
                }
            }
        }
        for x in 118..182 {
            let y = 215 + ((x as f64 - 150.0).powi(2) / 200.0) as usize;
            for k in 0..3 {
                put(x, y + k, [90, 40, 30, 255]);
            }
        }
        std::fs::write(path, studio_media::png::encode(&RgbaImage { width: w as u32, height: h as u32, pixels: px })).unwrap();
    }

    #[test]
    fn a_2d_puppet_says_a_line_and_its_lips_close_on_the_plosives() {
        let dir = sheet_dir().join("work2d");
        let Some((path, al, dur)) = spoken(&dir) else { return eprintln!("no say voice here: skipped") };
        let pic = dir.join("face.png");
        cartoon(&pic);
        let mut s = project_with_line(&path, 640, 640);
        let voice = s.doc.project.clips.iter().filter(|c| c.kind == ClipKind::Audio).map(|c| c.id).max().unwrap();
        let report = services::project::import_media(&mut s.doc, &[pic]);
        s.edit(&[json!({"op": "place", "media": report.ids[0], "at": 0, "duration": dur + 1.0, "fit": "contain"})], "face", false).unwrap();
        let id = s.doc.project.clips.iter().filter(|c| c.id != voice).map(|c| c.id).max().unwrap();
        let r = s.edit(&[json!({"op": "lip_sync", "clip": id, "audio": voice})], "lip", false).unwrap().to_string();
        assert!(r.contains("lip sync") && r.contains("phonemes"), "{r}");
        // The mouth reads back shut on each P and B, open on the vowels.
        let mc = s.model_clip(id).unwrap();
        let (open, _) = s.read_mouth(id, 0.3, dur).unwrap();
        let _ = &mc;
        let at = |t: f64| open[((t * 100.0) as usize).min(open.len() - 1)];
        for p in al.phones.iter().filter(|p| matches!(p.ph, studio_speech::Ph::P | studio_speech::Ph::B)) {
            let low = (0..10).map(|i| at(p.start - 0.06 + i as f64 * (p.dur() + 0.05) / 10.0)).fold(1.0f32, f32::min);
            assert!(low < 0.2, "{} at {:.2}s: open {low}", p.ph.name(), p.start);
        }
        let vowels: Vec<f32> = al.phones.iter().filter(|p| p.ph.is_vowel() && p.dur() > 0.1).map(|p| at((p.start + p.end) / 2.0 - 0.03)).collect();
        assert!(vowels.iter().any(|v| *v > 0.4), "vowel openings {vowels:?}");
        let c = s.edit(&[json!({"op": "lip_sync", "clip": id, "audio": voice, "check": true})], "check", false).unwrap().to_string();
        assert!(!c.contains("out of sync"), "{c}");
        let mut images = Vec::new();
        for (_, t) in moments(&al) {
            images.push(s.frames.rgba(&s.doc, micros(0.3 + t - 0.03), 480).unwrap());
        }
        tile(&images, 4, &sheet_dir().join("2d-puppet.png"));
    }
}
