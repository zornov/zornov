// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `check`: the things a finished edit should not have, found before the
//! export rather than after it — holes in the picture, clips that run out
//! of footage, titles off the canvas, sound that clips, dark or blank
//! frames.

use super::{Session, secs};
use crate::view::{self, ClipKind, LaneKind, MediaKind, SEC};
use serde_json::{Value, json};
use studio_model as m;

/// Peak and loudness of the mix.
pub struct Loudness {
    pub peak_db: f64,
    /// Integrated loudness (BS.1770 gating, unweighted), in LUFS-like dB.
    pub lufs: f64,
}

/// Measures `start..end` of the mix in one-minute pieces (long timelines
/// never sit in memory whole), mixed side by side: each piece is its own
/// mix from its start, as the export's would be, and a 10-minute film
/// otherwise waits on ten mixes in a row.
pub fn loudness(project: &studio_model::Project, start: view::Time, end: view::Time) -> Loudness {
    let rate = crate::sound_bridge::RATE as usize;
    let block = rate * 4 / 10; // 400 ms
    let hop = rate / 10; // 75% overlap
    let pieces: Vec<(view::Time, view::Time)> = (0..).map(|k| start + k * 60 * SEC).take_while(|t| *t < end).map(|t| (t, (t + 60 * SEC).min(end))).collect();
    // A few at a time: each minute is ~23 MB of samples.
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 8);
    let mut peak = 0f32;
    let mut energies: Vec<f64> = Vec::new();
    for wave in pieces.chunks(threads) {
        let measured: Vec<(f32, Vec<f64>)> = std::thread::scope(|sc| {
            let hs: Vec<_> = wave
                .iter()
                .map(|&(t, to)| {
                    sc.spawn(move || {
                        let st = crate::sound_bridge::mix_for_export(project, t, to);
                        let mut peak = 0f32;
                        let sq: Vec<f32> = st
                            .chunks_exact(2)
                            .map(|fr| {
                                peak = peak.max(fr[0].abs()).max(fr[1].abs());
                                fr[0] * fr[0] + fr[1] * fr[1]
                            })
                            .collect();
                        let energies = (0..).map(|k| k * hop).take_while(|i| i + block <= sq.len()).map(|i| sq[i..i + block].iter().map(|&x| x as f64).sum::<f64>() / block as f64).collect();
                        (peak, energies)
                    })
                })
                .collect();
            hs.into_iter().map(|h| h.join().unwrap_or_default()).collect()
        });
        for (p, e) in measured {
            peak = peak.max(p);
            energies.extend(e);
        }
    }
    let db = |e: f64| -0.691 + 10.0 * e.max(1e-12).log10();
    let gated: Vec<f64> = energies.iter().copied().filter(|&e| db(e) > -70.0).collect();
    let lufs = if gated.is_empty() {
        f64::NEG_INFINITY
    } else {
        let rel = db(gated.iter().sum::<f64>() / gated.len() as f64) - 10.0;
        let g2: Vec<f64> = gated.into_iter().filter(|&e| db(e) > rel).collect();
        if g2.is_empty() { f64::NEG_INFINITY } else { db(g2.iter().sum::<f64>() / g2.len() as f64) }
    };
    Loudness { peak_db: 20.0 * (peak.max(1e-9) as f64).log10(), lufs }
}

fn round(v: f64, places: i32) -> f64 {
    let k = 10f64.powi(places);
    (v * k).round() / k
}

impl Session {
    /// A fingerprint of everything that decides the picture over t0..t1:
    /// the clips on screen then (all their settings and keys), the
    /// transitions between them, the canvas, the app version.
    fn scene_key(&self, t0: view::Time, t1: view::Time, tag: &str) -> String {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        env!("CARGO_PKG_VERSION").hash(&mut h);
        tag.hash(&mut h);
        let p = &self.doc.project;
        (p.width, p.height, (p.fps * 1000.0) as u32, t0, t1).hash(&mut h);
        for q in &self.doc.model.sequences {
            let mut ids = Vec::new();
            for c in q.clips.iter().filter(|c| c.start <= t1 && t0 < c.start + c.duration) {
                serde_json::to_string(c).unwrap_or_default().hash(&mut h);
                ids.push(c.id);
            }
            for t in q.transitions.iter().filter(|t| ids.contains(&t.left) || ids.contains(&t.right)) {
                serde_json::to_string(t).unwrap_or_default().hash(&mut h);
            }
            for l in &q.lanes {
                (l.id.0, l.hidden).hash(&mut h);
            }
        }
        format!("{:016x}", h.finish())
    }

    fn memo_file(&self) -> Option<std::path::PathBuf> {
        self.home.as_ref().map(|h| m::file::ProjectFolder::new(h).cache_dir().join("check.json"))
    }

    fn memo(&mut self) -> &mut std::collections::HashMap<String, Value> {
        if self.look_memo.is_none() {
            let loaded = self.memo_file().and_then(|f| std::fs::read(f).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
            self.look_memo = Some(loaded);
        }
        self.look_memo.as_mut().expect("memo")
    }

    /// The pictures check will need that the cache does not have (same
    /// times and sizes as the checks themselves).
    #[cfg(feature = "render")]
    fn check_jobs(&mut self, picture_end: view::Time) -> Vec<(view::Time, u32, Vec<u32>)> {
        let p = self.doc.project.clone();
        let mut jobs = Vec::new();
        let mut want = |s: &mut Self, t: view::Time, tag: &str, width: u32, out: Vec<u32>, jobs: &mut Vec<(view::Time, u32, Vec<u32>)>| {
            let key = s.scene_key(t, t, tag);
            if !s.memo().contains_key(&key) {
                jobs.push((t, width, out));
            }
        };
        if let Some(spine) = p.spine() {
            let frame = (SEC as f64 / p.fps.max(1.0) as f64) as view::Time;
            let others: Vec<u32> = p.clips.iter().filter(|c| c.lane != spine).map(|c| c.id).collect();
            for c in p.clips.iter().filter(|c| c.lane == spine && c.kind == ClipKind::Video) {
                for t in [c.start, (c.end() - frame).max(c.start)] {
                    want(self, t, "edge64", 64, Vec::new(), &mut jobs);
                }
                for frac in [0.25, 0.5, 0.75] {
                    let t = c.start + (c.dur as f64 * frac) as view::Time;
                    want(self, t, "src192", 192, others.clone(), &mut jobs);
                }
            }
        }
        if picture_end > 0 {
            let n = ((secs(picture_end) / 1.0).ceil() as usize).clamp(4, 120);
            for i in 0..n {
                let t = (picture_end - SEC / 30).max(0) * i as i64 / (n as i64 - 1).max(1);
                want(self, t, "scan96", 96, Vec::new(), &mut jobs);
            }
        }
        for c in p.clips.iter().filter(|c| c.kind == ClipKind::Title && !c.text.trim().is_empty()) {
            let mut unseen = 0;
            for frac in [0.2, 0.5, 0.8] {
                let t = c.start + (c.dur as f64 * frac) as view::Time;
                let key = self.scene_key(t, t, &format!("design360:{}", c.id));
                if !self.memo().contains_key(&key) {
                    jobs.push((t, 360, Vec::new()));
                    jobs.push((t, 360, vec![c.id]));
                }
            }
        }
        jobs
    }

    pub(super) fn scene_key_pub(&self, t0: view::Time, t1: view::Time, tag: &str) -> String {
        self.scene_key(t0, t1, tag)
    }

    pub(super) fn memo_pub(&mut self) -> &mut std::collections::HashMap<String, Value> {
        self.memo()
    }

    pub(super) fn memo_save_pub(&self) {
        self.memo_save()
    }

    fn memo_save(&self) {
        if let (Some(f), Some(m)) = (self.memo_file(), &self.look_memo) {
            // Keep it bounded: a long session should not grow it forever.
            if m.len() < 20_000 {
                let _ = std::fs::create_dir_all(f.parent().unwrap_or(std::path::Path::new(".")));
                let _ = std::fs::write(f, serde_json::to_vec(m).unwrap_or_default());
            }
        }
    }

    /// The project check as the `check` tool runs it: with `fix`, what Studio can mend is mended
    /// in one undo step and the project looked at again; `fixed` lists what was done.
    pub fn check_project(&mut self, frames: bool, fix: bool) -> Value {
        let began = std::time::Instant::now();
        let mut v = self.quality(frames);
        let took = began.elapsed().as_secs_f64();
        v["took_seconds"] = json!((took * 10.0).round() / 10.0);
        if fix {
            let probs = v["problems"].as_array().cloned().unwrap_or_default();
            self.doc.begin_batch("Fix");
            let fixed = self.fix(&probs);
            self.doc.commit_batch();
            if !fixed.is_empty() {
                v = self.quality(frames);
            }
            v["fixed"] = json!(fixed);
        }
        // What was accepted as intended is listed apart, not as a problem.
        if self.apply_accepted(&mut v, "problems") {
            v["ok"] = json!(v["problems"].as_array().is_none_or(|p| p.is_empty()));
        }
        v
    }

    /// Mends one finding (`kind` on `clip`) as one undo step; says what was done, nothing when it cannot.
    pub fn fix_finding(&mut self, kind: &str, clip: Option<u32>) -> Vec<String> {
        let mut p = json!({"kind": kind});
        if let Some(c) = clip {
            p["clip"] = json!(c);
        }
        self.doc.begin_batch("Fix");
        let done = self.fix(&[p]);
        self.doc.commit_batch();
        done
    }

    pub(super) fn quality(&mut self, look: bool) -> Value {
        let p = self.doc.project.clone();
        let mut problems: Vec<Value> = Vec::new();
        let mut warn = |kind: &str, at: Option<f64>, clip: Option<u32>, msg: String, fix: &str| {
            let mut o = json!({"kind": kind, "problem": msg, "fix": fix});
            if let Some(t) = at { o["at"] = json!(round(t, 2)); }
            if let Some(c) = clip { o["clip"] = json!(c); }
            problems.push(o);
        };
        let end = p.duration();

        // Holes in the main lane (black in the export).
        if let Some(spine) = p.spine() {
            let mut cs: Vec<&view::Clip> = p.clips.iter().filter(|c| c.lane == spine).collect();
            cs.sort_by_key(|c| c.start);
            let mut cursor = 0;
            for c in &cs {
                if c.start - cursor > SEC / 30 {
                    // Covered by something on another picture lane?
                    let covered = p.clips.iter().any(|o| o.lane != spine && p.lane(o.lane).is_some_and(|l| l.kind == LaneKind::Visual) && o.kind == ClipKind::Video && o.start <= cursor && o.end() >= c.start);
                    if !covered {
                        warn("gap", Some(secs(cursor)), None, format!("{:.2}s of nothing on the main lane ({:.2}..{:.2}s)", secs(c.start - cursor), secs(cursor), secs(c.start)), "move the next clip back, or delete with ripple");
                    }
                }
                cursor = cursor.max(c.end());
            }
        }

        for c in &p.clips {
            // Running out of footage: the clip freezes or goes black.
            if let Some(mi) = c.media.and_then(|m| p.media(m)) {
                if mi.kind != MediaKind::Image {
                    // A speed ramp uses the footage its curve covers, not the
                    // speed at one moment times the length.
                    let used = self.model_clip(c.id).map_or(c.src_in as f64 + c.dur as f64 * c.speed as f64 / 100.0, |mc| mc.source_out() as f64);
                    if used > mi.dur as f64 + (SEC / 20) as f64 {
                        warn("past_media_end", Some(secs(c.start)), Some(c.id), format!("clip {} needs {:.2}s of {} but the file is {:.2}s", c.id, used / 1e6, mi.name, secs(mi.dur)), "shorten it (set duration) or slip it earlier");
                    }
                }
            }
            // Titles that land off the canvas.
            if c.kind == ClipKind::Title && !c.text.is_empty() && !look {
                let (hw, hh) = (p.width as f32 / 2.0, p.height as f32 / 2.0);
                let size = c.extra.text_size * p.height as f32 / 1080.0;
                let longest = c.text.lines().map(|l| l.chars().count()).max().unwrap_or(0) as f32;
                let width = longest * size * 0.55 * c.transform.scale / 100.0;
                if c.transform.y.abs() > hh - size * 0.5 || c.transform.x.abs() > hw {
                    warn("title_off_canvas", Some(secs(c.start)), Some(c.id), format!("title {} sits at ({:.0}, {:.0}), outside the {}x{} canvas", c.id, c.transform.x, c.transform.y, p.width, p.height), "set y within ±(height/2 − text size)");
                } else if width > p.width as f32 * 0.96 {
                    warn("title_too_wide", Some(secs(c.start)), Some(c.id), format!("title {} (\"{}\") is about {:.0}px wide on a {}px canvas", c.id, c.text.chars().take(30).collect::<String>(), width, p.width), "lower text_size, or break the line with \\n");
                }
            }
            for e in &c.effects {
                if studio_fx::effects::find(&e.kind).is_none() && !e.kind.starts_with("look") {
                    warn("unknown_effect", Some(secs(c.start)), Some(c.id), format!("clip {} has an effect \"{}\" Zornov does not know; it renders as nothing", c.id, e.kind), "remove it and apply one from catalogue effects");
                }
            }
        }

        // Sound.
        let mut sound = json!(null);
        if p.clips.iter().any(|c| c.kind == ClipKind::Audio || (c.kind == ClipKind::Video && c.audio.enabled)) && end > 0 {
            let l = loudness(&self.doc.model, 0, end);
            sound = json!({"peak_db": round(l.peak_db, 1), "loudness_lufs": if l.lufs.is_finite() { json!(round(l.lufs, 1)) } else { json!(null) }});
            if l.peak_db > -0.3 {
                warn("audio_clipping", None, None, format!("the mix peaks at {:.1} dBFS and will distort", l.peak_db), "lower the loudest clip's volume (music beds sit well around -8 to -12 dB)");
            }
            if l.lufs.is_finite() && l.lufs < -24.0 {
                warn("audio_quiet", None, None, format!("the mix is quiet ({:.1} LUFS; social video sits near -14)", l.lufs), &format!("raise every clip's volume by about {:.0} dB to land near -14 LUFS", -14.0 - l.lufs));
            } else if l.lufs.is_finite() && l.lufs > -8.0 {
                warn("audio_loud", None, None, format!("the mix is very loud ({:.1} LUFS)", l.lufs), &format!("lower every clip's volume by about {:.0} dB to land near -14 LUFS", l.lufs + 14.0));
            }
        }

        // The same sound again and again; one mood of sound for a film that changes.
        for f in self.variety() {
            warn(f.kind, Some(f.at), f.clips.first().copied(), f.note, &f.fix);
        }

        // A title drawn over a picture (a sticker, footage that is not the
        // whole frame): one of them cannot be read or seen.
        {
            use crate::editor::xform;
            let cw = p.width as f32 * p.height as f32;
            let bounds = |g: &xform::Geo| {
                let q = g.quad();
                let f = |pick: fn(&eframe::egui::Vec2) -> f32, max: bool| q.iter().map(pick).fold(if max { f32::MIN } else { f32::MAX }, |a, b| if max { a.max(b) } else { a.min(b) });
                [f(|v| v.x, false), f(|v| v.y, false), f(|v| v.x, true), f(|v| v.y, true)]
            };
            let mut said = 0;
            for t in p.clips.iter().filter(|c| c.kind == ClipKind::Title && !c.text.trim().is_empty()) {
                let at = t.start + t.dur / 2;
                let Some(tg) = xform::geo(&p, &self.doc.model, t.id, at) else { continue };
                let tb = bounds(&tg);
                let t_area = ((tb[2] - tb[0]) * (tb[3] - tb[1])).max(1.0);
                for c in p.clips.iter().filter(|c| c.kind == ClipKind::Video && c.start <= at && at < c.end() && c.id != t.id) {
                    let Some(cg) = xform::geo(&p, &self.doc.model, c.id, at) else { continue };
                    let cb = bounds(&cg);
                    // A background (most of the frame) is meant to be under titles.
                    if (cb[2] - cb[0]) * (cb[3] - cb[1]) > 0.6 * cw {
                        continue;
                    }
                    let ix = (tb[2].min(cb[2]) - tb[0].max(cb[0])).max(0.0);
                    let iy = (tb[3].min(cb[3]) - tb[1].max(cb[1])).max(0.0);
                    if ix * iy > 0.15 * t_area && said < 6 {
                        said += 1;
                        let above = cb[1] - (tb[3] - tb[1]) / 2.0 - 30.0;
                        warn("title_covers_picture", Some(secs(at)), Some(t.id), format!("title {} (\"{}\") is drawn over clip {} ({}): the words and the picture fight", t.id, t.text.chars().take(24).collect::<String>(), c.id, c.name), &format!("move the title clear of it (y about {:.0}, above it, or below its bottom at {:.0}), or move or shrink the picture", above, cb[3]));
                    }
                }
            }
        }
        // Motion that jumps or stalls (keyframes a still frame cannot show).
        {
            let moves = super::motion_check::problems(&self.doc.model, p.fps as f64);
            let more = moves.len().saturating_sub(12);
            for m in moves.into_iter().take(12) {
                warn(m.kind, Some(m.at), Some(m.clip), m.problem, m.fix);
            }
            if more > 0 {
                warn("motion_more", None, None, format!("{more} more moves jump or stall the same way"), "fix the ones above, then check again");
            }
        }
        super::jobs::report(0.2, "check: craft and 3D");
        // Craft: hidden layers, blown-up footage, overdoing.
        for m in super::craft_check::problems(&self.doc.model, &p) {
            warn(m.kind, (m.at > 0.0 || m.clip != 0).then_some(m.at), (m.clip != 0).then_some(m.clip), m.problem, m.fix);
        }
        // 3D: hero models that are primitives, missing parts, flat light.
        for m in self.three_d_problems().0 {
            warn(m.kind, (m.at > 0.0 || m.clip != 0).then_some(m.at), (m.clip != 0).then_some(m.clip), m.problem, m.fix);
        }
        // Light, air and look: a lamp that shows nothing, blown or crushed
        // frames, a subject lost in its background.
        super::jobs::report(0.35, "check: light and look");
        for m in self.look_problems(look) {
            warn(m.kind, (m.at > 0.0 || m.clip != 0).then_some(m.at), (m.clip != 0).then_some(m.clip), m.problem, &m.fix);
        }
        super::jobs::report(0.5, "check: characters' performances");
        // Characters' performances: sliding feet, broken limbs, gazes that miss.
        for m in self.character_problems() {
            warn(m.kind, Some(m.at), Some(m.clip), m.problem, &m.fix);
        }
        // Words on flat colour and nothing else: nothing to look at.
        if end > 3 * SEC && !p.clips.iter().any(|c| c.kind == ClipKind::Video) && p.clips.iter().any(|c| c.kind == ClipKind::Title && !c.text.trim().is_empty()) {
            warn("only_text", None, None, "the picture is only text on flat colour: there is nothing to look at".into(), "show what it is about: scene with show (the things in it) puts library pictures in, laid out and moving; or catalogue with query as a list of words, then add");
        }
        if sound.is_null() && end > SEC {
            warn("no_sound", None, None, "nothing on the timeline makes a sound (no music, every clip's own sound off)".into(), "place the music on a sound lane, or unmute a clip");
        }

        // Sound running on after the last picture: one problem, not a
        // black frame every two seconds. (A sound-only project, a mix or an
        // audio export, has no picture to end early.)
        let sound_only = p.clips.iter().all(|c| c.kind == ClipKind::Audio);
        // Drawn pictures (colour cards, shapes, gradients) and 3D models
        // (worlds and places too) are picture: an animation built from
        // nothing has no footage at all.
        let model = |s: &studio_model::ClipSource| matches!(s, studio_model::ClipSource::Generator(g) if g.kind == studio_model::GeneratorKind::Model);
        let drawn: std::collections::HashSet<u32> = self.doc.model.sequences.iter().flat_map(|q| q.clips.iter()).filter(|c| drawn_picture(&c.source) || model(&c.source)).map(|c| c.id.0 as u32).collect();
        let picture_end = p.clips.iter().filter(|c| (c.kind == ClipKind::Video || drawn.contains(&c.id)) && !p.lane(c.lane).is_some_and(|l| l.hidden)).map(|c| c.end()).max().unwrap_or(0);
        if !sound_only && end - picture_end > SEC / 10 {
            let long: Vec<String> = p.clips.iter().filter(|c| c.kind == ClipKind::Audio && c.end() > picture_end + SEC / 10).map(|c| c.id.to_string()).collect();
            // What runs on: sound, or only titles over nothing.
            let (what, fix) = if long.is_empty() {
                ("titles go on", format!("end the titles at {:.2}, or put picture behind them", secs(picture_end)))
            } else {
                ("sound goes on", format!("trim the end of clip(s) {} to {:.2} (with a fade_out), or add picture", long.join(", "), secs(picture_end)))
            };
            warn("picture_ends_early", Some(secs(picture_end)), None, format!("the picture ends at {:.2}s but {what} to {:.2}s (black screen)", secs(picture_end), secs(end)), &fix);
        }

        super::jobs::report(0.6, "check: rendering frames to look at");
        // Everything below that is not cached yet, rendered at once in
        // parallel before the checks read it.
        #[cfg(feature = "render")]
        if look {
            if std::env::var_os("STUDIO_NO_PREFETCH").is_none() {
                let jobs = self.check_jobs(picture_end);
                self.frames.prefetch(&self.doc, jobs);
            }
        }

        // A black frame right at a cut (a file that opens or ends on black)
        // flashes even when the samples below miss it.
        #[cfg(feature = "render")]
        if look {
            if let Some(spine) = p.spine() {
                let frame = (SEC as f64 / p.fps.max(1.0) as f64) as view::Time;
                for c in p.clips.iter().filter(|c| c.lane == spine && c.kind == ClipKind::Video) {
                    for (t, edge) in [(c.start, "first"), ((c.end() - frame).max(c.start), "last")] {
                        let key = self.scene_key(t, t, "edge64");
                        let mean = match self.memo().get(&key).and_then(Value::as_f64) {
                            Some(m) => m,
                            None => {
                                let Ok(img) = self.frames.rgba(&self.doc, t, 64) else { continue };
                                let m = img.pixels.chunks_exact(4).map(|q| 0.2126 * q[0] as f64 + 0.7152 * q[1] as f64 + 0.0722 * q[2] as f64).sum::<f64>() / (img.pixels.len() / 4).max(1) as f64;
                                self.memo().insert(key, json!(m));
                                m
                            }
                        };
                        if mean < 8.0 {
                            let card = self.doc.model.sequences.iter().flat_map(|q| q.clips.iter()).any(|k| matches!(&k.source, studio_model::ClipSource::Generator(g) if g.kind == studio_model::GeneratorKind::Solid) && k.start <= t && t < k.start + k.duration);
                            if !card {
                                warn("black_at_cut", Some(secs(t)), Some(c.id), format!("clip {}'s {edge} frame is black (the file has black there): a black flash at the cut", c.id), if edge == "first" { "slip the clip a little later (slip by 0.1)" } else { "trim its end a frame or two earlier" });
                            }
                        }
                    }
                }
            }
        }

        // Pictures: dark, blank or flat stretches, as ranges.
        #[cfg(feature = "render")]
        if look && picture_end > 0 {
            let n = ((secs(picture_end) / 1.0).ceil() as usize).clamp(4, 120);
            let mut found: Vec<(&str, view::Time, f64)> = Vec::new();
            for i in 0..n {
                let t = (picture_end - SEC / 30).max(0) * i as i64 / (n as i64 - 1).max(1);
                let key = self.scene_key(t, t, "scan96");
                let (mean, var, blown) = match self.memo().get(&key).and_then(Value::as_array).filter(|a| a.len() == 3).map(|a| (a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0), a[2].as_f64().unwrap_or(0.0))) {
                    Some(mv) => mv,
                    None => {
                        let Ok(img) = self.frames.rgba(&self.doc, t, 96) else { continue };
                        let px = img.pixels.chunks_exact(4).map(|q| 0.2126 * q[0] as f64 + 0.7152 * q[1] as f64 + 0.0722 * q[2] as f64).collect::<Vec<f64>>();
                        let mean = px.iter().sum::<f64>() / px.len().max(1) as f64;
                        let var = px.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / px.len().max(1) as f64;
                        let blown = px.iter().filter(|&&v| v > 250.0).count() as f64 / px.len().max(1) as f64;
                        self.memo().insert(key, json!([mean, var, blown]));
                        (mean, var, blown)
                    }
                };
                // A drawn picture on screen (a colour card, shapes, a
                // gradient) is a choice, not a fault.
                let card = self.doc.model.sequences.iter().flat_map(|q| q.clips.iter()).any(|c| drawn_picture(&c.source) && c.start <= t && t < c.start + c.duration);
                let kind = if card { "" } else if mean < 10.0 { "black_frame" } else if mean < 28.0 { "dark_frame" } else if blown > 0.35 { "blown_out" } else if var.sqrt() < 3.0 { "flat_frame" } else { "" };
                found.push((kind, t, mean));
            }
            let mut i = 0;
            while i < found.len() {
                let (kind, t0, mean) = found[i];
                let mut j = i;
                while j + 1 < found.len() && found[j + 1].0 == kind {
                    j += 1;
                }
                if !kind.is_empty() {
                    let t1 = found[j].1;
                    let at = |t: view::Time| p.clips.iter().filter(|c| c.kind == ClipKind::Video && c.start <= t && t < c.end()).map(|c| c.id.to_string()).collect::<Vec<_>>().join(", ");
                    let span = if j > i { format!("from {:.2}s to {:.2}s", secs(t0), secs(t1)) } else { format!("at {:.2}s", secs(t0)) };
                    let clips = at(t0);
                    let on = if clips.is_empty() { String::new() } else { format!(" (clip {clips})") };
                    match kind {
                        "black_frame" => warn(kind, Some(secs(t0)), None, format!("the picture is black {span}{on}"), "check for a gap, a fade, a clip past its footage, or exposure set too low"),
                        "dark_frame" => warn(kind, Some(secs(t0)), None, format!("the picture is very dark {span}{on} (mean {mean:.0}/255)"), "raise exposure (stops) on that clip, or pick a brighter moment"),
                        "blown_out" => warn(kind, Some(secs(t0)), None, format!("a large part of the picture is pure white {span}{on}"), "lower exposure or contrast on that clip, or ease the effect that flares it"),
                        _ => warn(kind, Some(secs(t0)), None, format!("the picture is one flat colour {span}{on}"), "is a clip missing or fully covered?"),
                    }
                }
                i = j + 1;
            }
        }
        #[cfg(feature = "render")]
        if look {
            problems.extend(self.design_problems());
            problems.extend(self.source_text());
            self.frames.clear_prefetch();
            self.memo_save();
        }
        super::jobs::report(0.9, "check: facing, framing and staging");
        problems.extend(self.facing_problems());
        problems.extend(self.framing_problems());
        problems.extend(self.staging_problems_seen(look));
        json!({"ok": problems.is_empty(), "duration": secs(end), "problems": problems, "sound": sound})
    }
}

// ------------------------------------------------------------- design

/// Relative luminance (sRGB) of an 8-bit pixel, 0..1.
#[cfg(feature = "render")]
fn lum(p: &[u8]) -> f64 {
    let lin = |c: u8| {
        let c = c as f64 / 255.0;
        if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(p[0]) + 0.7152 * lin(p[1]) + 0.0722 * lin(p[2])
}

/// What a title looks like where it actually landed: measured from the
/// rendered picture with and without it, not estimated from settings.
#[cfg(feature = "render")]
#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct Placed {
    /// Box of the title's pixels, fractions of the canvas (x0, y0, x1, y1).
    pub(super) bbox: [f64; 4],
    touches_edge: bool,
    /// WCAG contrast of the title's pixels against what is under them.
    contrast: f64,
    /// Detail (edge strength) under the title, and over the whole frame.
    busy_under: f64,
    busy_frame: f64,
    faces_covered: usize,
}

#[cfg(feature = "render")]
pub(super) fn measure(with: &studio_media::rgba::RgbaImage, without: &studio_media::rgba::RgbaImage, faces: &[studio_ai::features::faces::Face]) -> Option<Placed> {
    let (w, h) = (with.width as usize, with.height as usize);
    let px = |img: &studio_media::rgba::RgbaImage, x: usize, y: usize| -> [u8; 4] {
        let i = (y * w + x) * 4;
        [img.pixels[i], img.pixels[i + 1], img.pixels[i + 2], img.pixels[i + 3]]
    };
    let mut mask = vec![false; w * h];
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    let mut n = 0;
    for y in 0..h {
        for x in 0..w {
            let (a, b) = (px(with, x, y), px(without, x, y));
            let d: i32 = (0..3).map(|k| (a[k] as i32 - b[k] as i32).abs()).sum();
            if d > 48 {
                mask[y * w + x] = true;
                n += 1;
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    if n < 6 {
        return None; // not on screen at this moment (animating in, faded)
    }
    // Contrast: the title's light and dark parts (fill vs. stroke or
    // shadow) each against the picture under the box; the better one is
    // what the eye reads.
    let mut text: Vec<f64> = (0..w * h).filter(|&i| mask[i]).map(|i| lum(&px(with, i % w, i / w))).collect();
    text.sort_by(f64::total_cmp);
    let half = text.len() / 2;
    let dark = text[..half.max(1)].iter().sum::<f64>() / half.max(1) as f64;
    let light = text[half..].iter().sum::<f64>() / (text.len() - half).max(1) as f64;
    let (bx0, by0, bx1, by1) = (x0.saturating_sub(3), y0.saturating_sub(3), (x1 + 3).min(w - 1), (y1 + 3).min(h - 1));
    let mut bg = 0.0;
    let mut bn: f64 = 0.0;
    for y in by0..=by1 {
        for x in bx0..=bx1 {
            bg += lum(&px(without, x, y));
            bn += 1.0;
        }
    }
    let bg = bg / bn.max(1.0);
    let ratio = |a: f64, b: f64| (a.max(b) + 0.05) / (a.min(b) + 0.05);
    // Text that carries its own plate, outline or shadow reads on any
    // picture: its brightest and darkest quarters contrast with each
    // other, and the dark part is substantial (not just soft edges).
    let q = (text.len() / 4).max(1);
    let lo = text[..q].iter().sum::<f64>() / q as f64;
    let hi = text[text.len() - q..].iter().sum::<f64>() / q as f64;
    let own = if ratio(hi, lo) >= 4.5 { ratio(hi, lo) } else { 0.0 };
    let contrast = ratio(light, bg).max(ratio(dark, bg)).max(own);
    // Detail: mean luminance gradient (0..255 scale).
    let grad = |xa: usize, ya: usize, xb: usize, yb: usize| {
        let mut s = 0.0;
        let mut c: f64 = 0.0;
        for y in ya.max(1)..yb.min(h - 1) {
            for x in xa.max(1)..xb.min(w - 1) {
                let l = |x: usize, y: usize| lum(&px(without, x, y)).powf(1.0 / 2.2) * 255.0;
                s += (l(x + 1, y) - l(x - 1, y)).abs() + (l(x, y + 1) - l(x, y - 1)).abs();
                c += 1.0;
            }
        }
        s / c.max(1.0)
    };
    let bbox = [x0 as f64 / w as f64, y0 as f64 / h as f64, (x1 + 1) as f64 / w as f64, (y1 + 1) as f64 / h as f64];
    let faces_covered = faces
        .iter()
        .filter(|f| {
            let r = &f.bbox;
            let ix = (r.x + r.w).min(x1 as f32 + 1.0) - r.x.max(x0 as f32);
            let iy = (r.y + r.h).min(y1 as f32 + 1.0) - r.y.max(y0 as f32);
            ix > 0.0 && iy > 0.0 && ix * iy > 0.25 * r.w * r.h
        })
        .count();
    Some(Placed {
        bbox,
        touches_edge: x0 == 0 || y0 == 0 || x1 + 1 >= w || y1 + 1 >= h,
        contrast,
        busy_under: grad(bx0, by0, bx1 + 1, by1 + 1),
        busy_frame: grad(0, 0, w, h),
        faces_covered,
    })
}

#[cfg(feature = "render")]
impl Session {
    /// Text that came with the footage: a title or credits card from the
    /// source (white lettering on black) showing in the cut. Looked for on
    /// the main lane alone, so the edit's own titles do not count.
    pub(super) fn source_text(&mut self) -> Vec<Value> {
        let p = self.doc.project.clone();
        let Some(spine) = p.spine() else { return Vec::new() };
        let others: Vec<u32> = p.clips.iter().filter(|c| c.lane != spine).map(|c| c.id).collect();
        let mut out = Vec::new();
        for c in p.clips.iter().filter(|c| c.lane == spine && c.kind == ClipKind::Video) {
            for frac in [0.25, 0.5, 0.75] {
                let t = c.start + (c.dur as f64 * frac) as view::Time;
                let key = self.scene_key(t, t, "src192");
                if let Some(v) = self.memo().get(&key).and_then(Value::as_bool) {
                    if v {
                        out.push(json!({"kind": "source_text", "problem": format!("clip {} looks like it shows lettering from the source (a title or credits card: light letters on black) at {:.2}s", c.id, secs(t)), "fix": "frame it to confirm; slip the clip past it or pick another moment", "at": round(secs(t), 2), "clip": c.id}));
                        break;
                    }
                    continue;
                }
                let Ok(img) = self.frames.rgba_without(&self.doc, &others, t, 192) else { continue };
                let (w, h) = (img.width as usize, img.height as usize);
                let mut dark = 0usize;
                let mut bright_rows = vec![false; h];
                let mut bright = 0usize;
                let mut grey = 0usize;
                for (i, q) in img.pixels.chunks_exact(4).enumerate() {
                    let l = 0.2126 * q[0] as f64 + 0.7152 * q[1] as f64 + 0.0722 * q[2] as f64;
                    if l < 25.0 {
                        dark += 1;
                    } else if l > 150.0 {
                        bright += 1;
                        bright_rows[i / w] = true;
                        let (mx, mn) = (q[0].max(q[1]).max(q[2]), q[0].min(q[1]).min(q[2]));
                        if mx - mn < 45 {
                            grey += 1;
                        }
                    }
                }
                let n = (w * h) as f64;
                let rows = bright_rows.iter().filter(|b| **b).count() as f64 / h as f64;
                // Letters are thin: most light pixels sit next to dark ones.
                // A blurred bright shape is not.
                let px = |x: usize, y: usize| {
                    let q = &img.pixels[(y * w + x) * 4..(y * w + x) * 4 + 3];
                    0.2126 * q[0] as f64 + 0.7152 * q[1] as f64 + 0.0722 * q[2] as f64
                };
                let (mut lit, mut edge) = (0usize, 0usize);
                for y in 2..h.saturating_sub(2) {
                    for x in 2..w.saturating_sub(2) {
                        if px(x, y) > 150.0 {
                            lit += 1;
                            if [(x - 2, y), (x + 2, y), (x, y - 2), (x, y + 2)].iter().any(|&(a, b)| px(a, b) < 60.0) {
                                edge += 1;
                            }
                        }
                    }
                }
                let thin = lit > 0 && edge as f64 / lit as f64 > 0.45;
                let looks_like_card = thin && dark as f64 / n > 0.7 && (0.002..0.12).contains(&(bright as f64 / n)) && grey as f64 / bright.max(1) as f64 > 0.8 && rows < 0.35;
                self.memo().insert(key, json!(looks_like_card));
                if looks_like_card {
                    out.push(json!({"kind": "source_text", "problem": format!("clip {} looks like it shows lettering from the source (a title or credits card: light letters on black) at {:.2}s", c.id, secs(t)), "fix": "frame it to confirm; slip the clip past it or pick another moment", "at": round(secs(t), 2), "clip": c.id}));
                    break;
                }
            }
        }
        out
    }

    /// Titles judged on the picture: cut off, under the platform's
    /// buttons (vertical video), hard to read, over busy detail or other
    /// text, over faces, on top of each other.
    pub(super) fn design_problems(&mut self) -> Vec<Value> {
        let p = self.doc.project.clone();
        let vertical = p.height as f64 > p.width as f64 * 1.5;
        let mut faces: Option<Option<studio_ai::features::faces::FaceDetector>> = None;
        let titles: Vec<view::Clip> = p
            .clips
            .iter()
            .filter(|c| c.kind == ClipKind::Title && !c.text.trim().is_empty() && !p.lane(c.lane).is_some_and(|l| l.hidden))
            .cloned()
            .collect();
        let mut out = Vec::new();
        let mut boxes: Vec<(u32, view::Time, [f64; 4])> = Vec::new();
        for c in &titles {
            let mut worst: std::collections::BTreeMap<&str, (f64, view::Time, String, &str)> = Default::default();
            let mut note = |kind: &'static str, score: f64, t: view::Time, msg: String, fix: &'static str| {
                if worst.get(kind).is_none_or(|w| score > w.0) {
                    worst.insert(kind, (score, t, msg, fix));
                }
            };
            let mut unseen = 0;
            for frac in [0.2, 0.5, 0.8] {
                let t = c.start + (c.dur as f64 * frac) as view::Time;
                let key = self.scene_key(t, t, &format!("design360:{}", c.id));
                let cached = self.memo().get(&key).cloned();
                let m: Option<Placed> = match cached {
                    Some(v) => serde_json::from_value(v).ok().flatten(),
                    None => {
                        let (Ok(a), Ok(b)) = (self.frames.rgba(&self.doc, t, 360), self.frames.rgba_without(&self.doc, &[c.id], t, 360)) else { continue };
                        let fd = faces.get_or_insert_with(|| studio_ai::features::faces::FaceDetector::load(&studio_ai::store::ModelStore::default()).ok());
                        let found = match fd {
                            Some(fd) => fd.detect(&studio_ai::frame::Frame { width: b.width as usize, height: b.height as usize, rgba: b.pixels.clone() }).unwrap_or_default(),
                            None => Vec::new(),
                        };
                        let found: Vec<_> = found.into_iter().filter(|f| f.score > 0.6).collect();
                        let m = measure(&a, &b, &found);
                        self.memo().insert(key, serde_json::to_value(&m).unwrap_or(Value::Null));
                        m
                    }
                };
                let name = format!("title {} (\"{}\")", c.id, c.text.chars().take(24).collect::<String>());
                // Drawn but nothing changes on screen: the text is the same
                // colour as what is behind it (or off the frame).
                let Some(m) = m else {
                    // At every look, not just while animating in or out.
                    unseen += 1;
                    if unseen == 3 {
                        note("title_invisible", 5.0, t, format!("{name} cannot be seen: it draws nothing that stands out from the picture behind it"), "change its colour (light on dark, dark on light), give it a backing, or move it into the frame");
                    }
                    continue;
                };
                boxes.push((c.id, t, m.bbox));
                if m.touches_edge {
                    note("title_cut_off", 1.0, t, format!("{name} runs off the edge of the frame"), "lower text_size, break the line with \\n, or move it in from the edge");
                }
                if vertical {
                    let [x0, y0, x1, y1] = m.bbox;
                    let under = if y1 > 0.80 { Some("the caption and buttons at the bottom") } else if y0 < 0.07 { Some("the status bar and tabs at the top") } else if x1 > 0.88 && y1 > 0.45 { Some("the like / share buttons on the right") } else { None };
                    if let Some(zone) = under {
                        note("title_under_app_ui", 1.0, t, format!("{name} sits under {zone} on TikTok / Reels / Shorts"), "keep titles between 8% and 78% of the height (y from -800 to 500 on 1920) and clear of the right 12% (lower text_size or move x left)");
                    }
                    let _ = x0;
                }
                if m.contrast < 3.0 {
                    note("title_low_contrast", 3.0 - m.contrast, t, format!("{name} is hard to read: contrast {:.1}:1 against the picture behind it (aim for 4.5:1)", m.contrast), "change its colour, use a text style with a stroke or shadow, or darken behind it (an fx_layer, or a shape from assets)");
                }
                if m.busy_under > 26.0 && m.busy_under > m.busy_frame * 1.4 {
                    note("title_over_detail", m.busy_under, t, format!("{name} sits on the busiest part of the frame (detail {:.0} vs {:.0} on average): it may cover on-screen text, a logo or the action", m.busy_under, m.busy_frame), "move it (y) to a calmer area; frame / contact_sheet shows where");
                }
                if m.faces_covered > 0 {
                    note("title_over_face", m.faces_covered as f64, t, format!("{name} covers {} face(s)", m.faces_covered), "move it (y) above or below the face");
                }
            }
            for (kind, (_, t, msg, fix)) in worst {
                out.push(json!({"kind": kind, "problem": msg, "fix": fix, "at": round(secs(t), 2), "clip": c.id}));
            }
        }
        // Two titles on screen together whose text overlaps.
        for (i, a) in boxes.iter().enumerate() {
            for b in &boxes[i + 1..] {
                if a.0 != b.0 && a.1 == b.1 {
                    let ix = a.2[2].min(b.2[2]) - a.2[0].max(b.2[0]);
                    let iy = a.2[3].min(b.2[3]) - a.2[1].max(b.2[1]);
                    if ix > 0.0 && iy > 0.0 {
                        out.push(json!({"kind": "titles_overlap", "problem": format!("titles {} and {} are drawn on top of each other", a.0, b.0), "fix": "move one (y) or shift it in time", "at": round(secs(a.1), 2), "clip": b.0}));
                    }
                }
            }
        }
        out
    }
}

#[cfg(all(test, feature = "render"))]
mod tests {
    use super::*;
    use studio_media::rgba::RgbaImage;

    fn image(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 3]) -> RgbaImage {
        let mut pixels = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let c = f(x, y);
                pixels.extend_from_slice(&[c[0], c[1], c[2], 255]);
            }
        }
        RgbaImage { width: w, height: h, pixels }
    }

    #[test]
    fn a_title_is_measured_where_it_landed() {
        let bg = |_, _| [40, 40, 40];
        let under = image(100, 100, bg);
        let block = |x: u32, y: u32| (20..80).contains(&x) && (40..50).contains(&y);
        // White on dark grey: readable, inside the frame.
        let white = image(100, 100, |x, y| if block(x, y) { [255, 255, 255] } else { [40, 40, 40] });
        let m = measure(&white, &under, &[]).unwrap();
        assert!(m.contrast > 7.0, "{}", m.contrast);
        assert!(!m.touches_edge);
        assert!((m.bbox[0] - 0.2).abs() < 0.02 && (m.bbox[3] - 0.5).abs() < 0.02, "{:?}", m.bbox);
        // Grey on a near-grey: hard to read.
        let grey = image(100, 100, |x, y| if block(x, y) { [95, 95, 95] } else { [40, 40, 40] });
        assert!(measure(&grey, &under, &[]).unwrap().contrast < 3.0);
        // Nothing drawn: no title on screen.
        assert!(measure(&under, &under, &[]).is_none());
        // Running off the left edge.
        let off = image(100, 100, |x, y| if x < 30 && (40..50).contains(&y) { [255, 255, 255] } else { [40, 40, 40] });
        assert!(measure(&off, &under, &[]).unwrap().touches_edge);
    }
}

/// A picture Studio draws itself: a colour card, a shape or a gradient
/// (not text, not an adjustment layer).
fn drawn_picture(source: &studio_model::ClipSource) -> bool {
    matches!(source, studio_model::ClipSource::Generator(g) if matches!(g.kind, studio_model::GeneratorKind::Solid | studio_model::GeneratorKind::Shape | studio_model::GeneratorKind::Gradient))
}
