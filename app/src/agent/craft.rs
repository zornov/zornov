// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Helpers that let agents say what they mean: media and lanes by name,
//! "fill the frame" instead of a computed percentage, and letterboxed
//! footage detected rather than eyeballed.

use crate::view::{self, LaneKind, MediaKind};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// The reason keys written to one moment come out as one.
pub(super) const SAME_MOMENT: &str = "landed on the same moment as another and merged";

/// What a write did to the keys it was given: how many were written, how
/// many the tracks hold, and what became of the rest. A write op's reply
/// says all three, never only the count it was asked for.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct KeyTally {
    pub written: usize,
    pub kept: usize,
    /// (how many, why), in the order they were lost.
    pub dropped: Vec<(usize, String)>,
}

impl KeyTally {
    pub fn new(written: usize) -> Self {
        Self { written, kept: written, dropped: Vec::new() }
    }

    /// `n` of the keys never reached the tracks, for `why`.
    pub fn lose(&mut self, n: usize, why: &str) {
        if n > 0 {
            self.kept = self.kept.saturating_sub(n);
            self.dropped.push((n, why.to_owned()));
        }
    }

    /// The tracks hold `stored` keys: whatever is missing from what was
    /// still expected is put down to `why`.
    pub fn settle(&mut self, stored: usize, why: &str) {
        self.lose(self.kept.saturating_sub(stored), why);
    }

    pub fn dropped_total(&self) -> usize {
        self.dropped.iter().map(|d| d.0).sum()
    }

    /// "N keys" when nothing was lost, else "K of N keys kept, D dropped (a; b)".
    pub fn say(&self) -> String {
        if self.dropped_total() == 0 {
            return format!("{} keys", self.written);
        }
        let why: Vec<String> = self.dropped.iter().map(|(n, w)| format!("{n} {w}")).collect();
        format!("{} of {} keys kept, {} dropped ({})", self.kept, self.written, self.dropped_total(), why.join("; "))
    }

    pub fn json(&self) -> Value {
        json!({"written": self.written, "kept": self.kept, "dropped": self.dropped_total(), "why": self.dropped.iter().map(|(n, w)| json!({"keys": n, "reason": w})).collect::<Vec<_>>()})
    }
}

/// An id sent as a number or as digits in a string ("8"): agents send both.
pub fn as_id(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_str().and_then(|s| s.trim().trim_start_matches('#').parse::<u64>().ok()))
}

/// A media item by id, by name (loosely), or by file path.
pub fn media_of(p: &view::Project, v: &Value) -> Result<u32, String> {
    if let Some(id) = v.as_u64() {
        return p.media(id as u32).map(|m| m.id).ok_or_else(|| format!("no media {id}; see `media`"));
    }
    let want = v.as_str().ok_or("media is an id or a name")?.trim().to_lowercase();
    // An id written as text ("8"): the tool schemas say "a media id or
    // name", and agents send ids as strings.
    if let Some(id) = as_id(v)
        && let Some(m) = p.media(id as u32)
    {
        return Ok(m.id);
    }
    let stem = |m: &view::MediaItem| m.name.rsplit_once('.').map_or(m.name.as_str(), |(a, _)| a).to_lowercase();
    if let Some(m) = p.media.iter().find(|m| m.name.to_lowercase() == want || stem(m) == want || m.path.as_ref().is_some_and(|x| x.to_string_lossy().to_lowercase() == want)) {
        return Ok(m.id);
    }
    let hits: Vec<&view::MediaItem> = p.media.iter().filter(|m| m.name.to_lowercase().contains(&want)).collect();
    match hits.as_slice() {
        [one] => Ok(one.id),
        [] => Err(format!("no media called \"{want}\"; media: {}", p.media.iter().map(|m| m.name.as_str()).collect::<Vec<_>>().join(", "))),
        many => Err(format!("\"{want}\" matches several media: {}", many.iter().map(|m| m.name.as_str()).collect::<Vec<_>>().join(", "))),
    }
}

/// A lane by id, by name ("Main", "Titles", "Music"…) or by role ("main",
/// "overlay", "sound").
pub fn lane_of(p: &view::Project, v: &Value, kind: Option<MediaKind>) -> Result<u32, String> {
    if let Some(id) = v.as_u64() {
        return p.lane(id as u32).map(|l| l.id).ok_or_else(|| format!("no lane {id}; see summary"));
    }
    let want = v.as_str().unwrap_or("").trim().to_lowercase();
    if let Some(l) = p.lanes.iter().find(|l| l.name.to_lowercase() == want) {
        return Ok(l.id);
    }
    // An id written as text ("2").
    if let Some(l) = as_id(v).and_then(|id| p.lane(id as u32)) {
        return Ok(l.id);
    }
    let role = match want.as_str() {
        "main" | "spine" | "" if kind != Some(MediaKind::Audio) => Some(LaneKind::Spine),
        "overlay" | "top" | "above" => Some(LaneKind::Visual),
        "sound" | "audio" | "music" | "" => Some(LaneKind::Audio),
        _ => None,
    };
    role.and_then(|k| p.lanes.iter().find(|l| l.kind == k).map(|l| l.id)).ok_or_else(|| format!("no lane \"{want}\"; lanes: {}", p.lanes.iter().map(|l| l.name.as_str()).collect::<Vec<_>>().join(", ")))
}

/// Rows of a video that carry picture (letterbox bars excluded), as
/// fractions of its height. Sampled from a few frames; cached per file.
pub fn active_rows(path: &Path, dur_secs: f64) -> (f64, f64) {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, (f64, f64)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(v) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(path) {
        return *v;
    }
    let (w, h) = (160usize, 90usize);
    let mut top = h;
    let mut bottom = 0usize;
    for k in 1..=5 {
        let t = dur_secs * k as f64 / 6.0;
        let Ok(img) = crate::media_io::frame(path, t, w, h, true) else { continue };
        for y in 0..h {
            let lit = (0..w).filter(|&x| {
                let c = img.pixels[y * w + x];
                (c.r() as u32 + c.g() as u32 + c.b() as u32) > 3 * 18
            });
            if lit.count() > w / 20 {
                top = top.min(y);
                bottom = bottom.max(y + 1);
            }
        }
    }
    let r = if bottom > top { (top as f64 / h as f64, bottom as f64 / h as f64) } else { (0.0, 1.0) };
    cache.lock().unwrap_or_else(|e| e.into_inner()).insert(path.to_path_buf(), r);
    r
}

/// The clip scale (percent) for a fit mode: "fit" shows all of the media,
/// "fill" covers the canvas, "fill_picture" covers it with the media's
/// picture (ignoring letterbox bars burned into the footage).
pub fn fit_scale(p: &view::Project, mi: &view::MediaItem, mode: &str) -> Result<f32, String> {
    let (cw, ch) = (p.width as f64, p.height as f64);
    let (mw, mh) = (mi.width.max(1) as f64, mi.height.max(1) as f64);
    let fit = (cw / mw).min(ch / mh);
    let (dw, dh) = (mw * fit, mh * fit);
    Ok(match mode {
        "fit" | "contain" => 100.0,
        "fill" | "cover" => ((cw / dw).max(ch / dh) * 100.0) as f32,
        "fill_picture" | "fill_active" | "fill_no_bars" => {
            let (top, bottom) = match &mi.path {
                Some(path) if mi.kind == MediaKind::Video => active_rows(path, mi.dur as f64 / 1e6),
                _ => (0.0, 1.0),
            };
            let pic_h = dh * (bottom - top).max(0.05);
            ((cw / dw).max(ch / pic_h) * 100.0 * 1.01) as f32
        }
        other => return Err(format!("unknown fit \"{other}\"; use fit, fill or fill_picture")),
    })
}

/// "#rrggbb" or a few names.
pub fn colour(v: &str) -> Option<eframe::egui::Color32> {
    use eframe::egui::Color32;
    let named = match v.trim().to_lowercase().as_str() {
        "white" => Some(Color32::WHITE),
        "black" => Some(Color32::BLACK),
        "red" => Some(Color32::from_rgb(235, 40, 40)),
        "yellow" => Some(Color32::from_rgb(255, 214, 0)),
        "pink" => Some(Color32::from_rgb(255, 64, 160)),
        "cyan" => Some(Color32::from_rgb(0, 220, 255)),
        "blue" => Some(Color32::from_rgb(40, 100, 255)),
        "green" => Some(Color32::from_rgb(40, 200, 90)),
        "orange" => Some(Color32::from_rgb(255, 140, 20)),
        "purple" => Some(Color32::from_rgb(140, 70, 230)),
        "grey" | "gray" => Some(Color32::from_rgb(128, 128, 128)),
        _ => None,
    };
    named.or_else(|| {
        let h = v.trim().trim_start_matches('#');
        // #rgb shorthand (each digit doubled), #rrggbb, or #rrggbbaa (the
        // alpha is opacity's job, so it is dropped).
        let full = match h.len() {
            3 => h.chars().flat_map(|c| [c, c]).collect::<String>(),
            6 => h.to_owned(),
            8 => h[..6].to_owned(),
            _ => return None,
        };
        u32::from_str_radix(&full, 16).ok().map(|n| Color32::from_rgb((n >> 16) as u8, (n >> 8) as u8, n as u8))
    })
}

#[cfg(test)]
mod colour_tests {
    #[test]
    fn short_and_long_hex_and_names() {
        assert_eq!(super::colour("#000"), Some(eframe::egui::Color32::BLACK));
        assert_eq!(super::colour("#f0a"), super::colour("#ff00aa"));
        assert_eq!(super::colour("#ff00aa80"), super::colour("#ff00aa"));
        assert!(super::colour("orange").is_some());
        assert!(super::colour("#12345").is_none());
        assert!(super::colour("#zzz").is_none());
    }
}

// ------------------------------------------------------------------ ops

use super::{Session, f, micros, secs};
use crate::view::{ClipKind, SEC};
use studio_model as m;

impl Session {
    /// `place`: one op for "put this piece of that file here", with the
    /// source range, speed and framing in the same breath.
    pub(super) fn place(&mut self, op: &Value) -> Result<String, String> {
        let p = &self.doc.project;
        let media = match media_of(p, op.get("media").ok_or("place needs \"media\" (an id or a name from `media`)")?) {
            Ok(m) => m,
            // Not in the project's media: a model the person kept by that name.
            Err(e) => match op.get("media").and_then(Value::as_str).filter(|n| self.has_kept_model(n)) {
                Some(n) => {
                    let mut add = op.clone();
                    add["op"] = json!("add");
                    add["item"] = json!(n);
                    add["panel"] = json!("assets");
                    if let Some(o) = add.as_object_mut() {
                        o.remove("media");
                    }
                    return self.op(&add);
                }
                None => return Err(e),
            },
        };
        let mi = p.media(media).cloned().ok_or("that media is gone")?;
        let kind = if mi.kind == MediaKind::Audio { ClipKind::Audio } else { ClipKind::Video };
        let lane = match op.get("lane") {
            Some(v) => lane_of(p, v, Some(mi.kind))?,
            None => lane_of(p, &Value::Null, Some(mi.kind))?,
        };
        let speed = f(op, "speed").unwrap_or(1.0).clamp(0.01, 100.0);
        let media_len = if mi.kind == MediaKind::Image { f64::INFINITY } else { secs(mi.dur) };
        let src_in = f(op, "source_in").or_else(|| f(op, "from")).unwrap_or(0.0);
        if src_in < 0.0 || src_in >= media_len {
            return Err(format!("source_in {src_in}s is outside {} ({:.2}s long)", mi.name, media_len));
        }
        let src_out = match (f(op, "source_out").or_else(|| f(op, "to")), f(op, "duration")) {
            (Some(o), _) => o,
            (None, Some(d)) => src_in + d * speed,
            (None, None) if mi.kind == MediaKind::Image => src_in + 4.0,
            (None, None) => media_len,
        };
        if src_out <= src_in {
            return Err(format!("the source range {src_in}..{src_out}s is empty"));
        }
        let explicit = op.get("source_out").is_some() || op.get("duration").is_some();
        if explicit && src_out > media_len + 1e-3 {
            let room = (media_len - src_in) / speed;
            return Err(format!("{} has only {:.3}s after source_in {src_in} (it is {media_len:.3}s long), so {:.3}s at speed {speed} does not fit; use duration ≤ {room:.3} or source_in ≤ {:.3}", mi.name, media_len - src_in, (src_out - src_in) / speed, (media_len - (src_out - src_in)).max(0.0)));
        }
        let mut note = String::new();
        let src_out = src_out.min(media_len);
        let mut dur = micros((src_out - src_in) / speed).max(SEC / 30);
        if mi.kind != MediaKind::Image {
            // Never a rounding hair past the end of the file.
            let room = ((mi.dur - micros(src_in)) as f64 / speed).floor() as view::Time;
            dur = dur.min(room.max(1));
        }
        let mut at = micros(f(op, "at").unwrap_or_else(|| secs(p.clips.iter().filter(|c| c.lane == lane).map(|c| c.end()).max().unwrap_or(0))));
        let spine = p.spine() == Some(lane);
        // A hole on the main lane (left by a delete without ripple) is
        // filled where it is, without pushing anything.
        let in_hole = spine && {
            let end = at + micros((src_out.min(media_len) - src_in) / speed);
            let lane_end = p.clips.iter().filter(|c| c.lane == lane).map(|c| c.end()).max().unwrap_or(0);
            at < lane_end && !p.clips.iter().any(|c| c.lane == lane && c.start < end && at < c.end())
        };
        if spine && !in_hole {
            // The main lane is magnetic: clips sit end to end, so a clip goes
            // in at the nearest cut.
            let cut = p.clips.iter().filter(|c| c.lane == lane).flat_map(|c| [c.start, c.end()]).chain([0]).min_by_key(|x| (x - at).abs()).unwrap_or(0);
            if cut != at {
                note.push_str(&format!(" (main lane is magnetic: went in at the cut at {:.2}s; use another lane to sit anywhere)", secs(cut)));
            }
            at = cut;
        }
        let stem = mi.name.rsplit_once('.').map_or(mi.name.as_str(), |(a, _)| a).to_owned();
        let mut clip = services_new_clip(at, dur, kind, op.get("name").and_then(Value::as_str).unwrap_or(&stem), Some(media));
        clip.lane = lane;
        clip.src_in = micros(src_in);
        clip.speed = (speed * 100.0) as f32;
        if let Some(b) = op.get("muted").and_then(Value::as_bool) {
            clip.audio.enabled = !b;
        }
        if let Some(v) = f(op, "volume") {
            clip.audio.volume = v as f32;
        }
        if kind == ClipKind::Video {
            if let Some(mode) = op.get("fit").and_then(Value::as_str) {
                clip.transform.scale = fit_scale(p, &mi, mode)?;
            }
            if let Some(v) = f(op, "x") {
                clip.transform.x = v as f32;
            }
            if let Some(v) = f(op, "y") {
                clip.transform.y = v as f32;
            }
        }
        let id = self.doc.alloc_id() as u32;
        clip.id = id;
        let ok = if in_hole {
            let mc = crate::bridge::new_clip(&self.doc.model, &clip);
            let seq = self.doc.seq();
            note.push_str(" (filled the gap on the main lane)");
            self.doc.apply("Place clip", m::Edit::Overwrite { seq, lane: Some(m::LaneId(lane as u64)), at, clip: Box::new(mc) })
        } else if spine {
            let mc = crate::bridge::new_clip(&self.doc.model, &clip);
            let seq = self.doc.seq();
            self.doc.apply("Place clip", m::Edit::Insert { seq, lane: Some(m::LaneId(lane as u64)), at, clip: Box::new(mc) })
        } else {
            self.doc.edit("Place clip", |p| {
                p.clips.push(clip);
                p.resolve_overlap(id);
            })
        };
        self.check(ok, "")?;
        let c = self.clip(id)?;
        let lane_name = self.doc.project.lane(c.lane).map(|l| l.name.clone()).unwrap_or_default();
        Ok(format!("clip {id}: {} {:.2}..{:.2}s of source at {:.2}s on {lane_name}{note}", mi.name, src_in, src_out, secs(c.start)))
    }

    /// `animate`: keys for one property (or several, with `properties`), in
    /// the units `set` uses, times in seconds from the clip's start (or on
    /// the timeline with `timeline: true`). Keys are `[t, value]` or
    /// `{"t", "v", "ease"}`; values are numbers, `#rrggbb` colours or
    /// `[x, y]` points. `ease` names a curve for every key; a key's own
    /// `ease` shapes the move that starts at it. `wiggle` and `pulse` make
    /// keys for you.
    pub(super) fn animate(&mut self, op: &Value) -> Result<String, String> {
        // Several properties in one op: each runs as its own animate.
        if let Some(Value::Object(props)) = op.get("properties") {
            let mut said = Vec::new();
            for (name, keys) in props {
                let mut one = op.clone();
                if let Some(o) = one.as_object_mut() {
                    o.remove("properties");
                    o.insert("property".into(), json!(name));
                    o.insert("keys".into(), keys.clone());
                }
                said.push(self.animate(&one)?);
            }
            return Ok(said.join("; "));
        }
        // Along a curve from mathematics, or by a formula of t.
        if op.get("curve").is_some() || op.get("formula").is_some() {
            return self.animate_math(op);
        }
        let id = op.get("clip").and_then(Value::as_u64).ok_or("animate needs \"clip\"")? as u32;
        let c = self.clip(id)?;
        let name = op.get("property").and_then(Value::as_str).ok_or("animate needs \"property\"")?.trim().to_lowercase();
        if name == "speed" {
            return self.speed_curve(id, op);
        }
        let mut mc = self.model_clip(id).ok_or("that clip is gone")?;
        let position = name == "position";
        let prop = super::props::resolve(&mc, if position { "x" } else { &name })?;
        let path0 = prop.paths[0].clone();
        if path0.starts_with("grade.") && mc.grade.is_none() {
            let seq = self.doc.seq();
            let ok = self.doc.apply("Grade", m::Edit::SetGrade { seq, clip: m::ClipId(id as u64), grade: Some(m::Grade { enabled: true, ..Default::default() }) });
            self.check(ok, "")?;
            mc = self.model_clip(id).ok_or("that clip is gone")?;
        }
        let now = super::props::value_or_default(&mc, &path0).ok_or_else(|| format!("{name} has no value on clip {id}"))?;
        let numeric = matches!(now, m::Value::Scalar(_) | m::Value::Angle(_)) || matches!(prop.conv, super::props::Conv::PointX | super::props::Conv::PointY);
        let relative = op.get("relative").and_then(Value::as_bool).unwrap_or(false);
        let multiply = matches!(prop.conv, super::props::Conv::Percent) && !prop.name.starts_with("crop");
        let base: Vec<f64> = if position {
            match &now { m::Value::Point(p) => vec![p.x, p.y], _ => vec![0.0, 0.0] }
        } else {
            vec![super::props::to_friendly(&prop, &path0, &now).as_f64().unwrap_or(0.0)]
        };
        let offset = if op.get("timeline").and_then(Value::as_bool).unwrap_or(false) { secs(c.start) } else { 0.0 };
        let curve_all = op.get("ease").and_then(Value::as_str).unwrap_or("smooth").to_owned();
        curve(&curve_all).ok_or_else(|| format!("ease \"{curve_all}\": {}", CURVES))?;

        // (time from clip start, value as given, curve leaving this key)
        let mut keys: Vec<(f64, Value, String)> = Vec::new();
        // Path handles a position key was given itself, by key time.
        let mut own_paths: Vec<(f64, m::PathHandles)> = Vec::new();
        if let Some(w) = op.get("wiggle") {
            // A hand-held wobble around the current value.
            let amount = w.get("amount").and_then(Value::as_f64).ok_or("wiggle needs \"amount\" (in the property's units)")?;
            let freq = w.get("per_second").and_then(Value::as_f64).unwrap_or(6.0).clamp(0.5, 30.0);
            let from = w.get("from").and_then(Value::as_f64).unwrap_or(0.0) - offset;
            let to = w.get("to").and_then(Value::as_f64).map(|t| t - offset).unwrap_or(secs(c.dur));
            let mut seed = (id as u64).wrapping_mul(2654435761) ^ (amount.to_bits() >> 7);
            let mut rnd = || {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                (seed >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
            };
            let mut t = from.max(0.0);
            while t <= to.min(secs(c.dur)) + 1e-6 {
                let edge = t <= from.max(0.0) + 1e-6 || t + 1.0 / freq > to;
                let v = if position { json!([base[0] + if edge { 0.0 } else { rnd() * amount }, base[1] + if edge { 0.0 } else { rnd() * amount }]) } else { json!(if edge { base[0] } else { base[0] + rnd() * amount }) };
                keys.push((t, v, "smooth".into()));
                t += 1.0 / freq;
            }
        } else if let Some(pl) = op.get("pulse") {
            // A hit on each time given (beats): up to the peak at once, back
            // to rest over `decay`.
            let times: Vec<f64> = pl.get("at").and_then(Value::as_array).ok_or("pulse needs \"at\": [times on the timeline] (beats from listen)")?.iter().filter_map(Value::as_f64).collect();
            let peak = pl.get("peak").and_then(Value::as_f64).ok_or("pulse needs \"peak\" (with relative: a factor, e.g. 1.12)")?;
            let rest = pl.get("rest").and_then(Value::as_f64).unwrap_or(if relative && multiply { 1.0 } else { base[0] });
            let decay = pl.get("decay").and_then(Value::as_f64).unwrap_or(0.25).max(0.02);
            for t in times {
                let t = t - secs(c.start);
                if t < -1e-6 || t > secs(c.dur) + 1e-6 {
                    continue;
                }
                keys.push((t, json!(peak), "snap".into()));
                keys.push(((t + decay).min(secs(c.dur)), json!(rest), "smooth".into()));
            }
            if keys.is_empty() {
                return Err(format!("none of the pulse times fall inside clip {id} ({:.2}..{:.2}s on the timeline)", secs(c.start), secs(c.end())));
            }
        } else {
            let list = op.get("keys").and_then(Value::as_array).ok_or("animate needs \"keys\" ([t, value] or {\"t\", \"v\", \"ease\"}), or \"wiggle\" / \"pulse\"")?;
            for k in list {
                let (t, v, e) = match k {
                    Value::Array(a) if position && a.len() == 3 => (a[0].as_f64(), json!([a[1], a[2]]), None),
                    Value::Array(a) if a.len() == 2 => (a[0].as_f64(), a[1].clone(), None),
                    Value::Object(o) => (o.get("t").and_then(Value::as_f64), o.get("v").or_else(|| o.get("value")).cloned().unwrap_or(Value::Null), o.get("ease").and_then(Value::as_str).map(str::to_owned)),
                    _ => (None, Value::Null, None),
                };
                let t = t.ok_or("each key is [t, value], [t, x, y] for position, or {\"t\", \"v\", \"ease\"}")? - offset;
                if v.is_null() {
                    return Err("a key has no value".into());
                }
                if let Some(ph) = k.get("path") {
                    if !position {
                        return Err("a key's \"path\" bends position; it is not for {name}".into());
                    }
                    own_paths.push((t, path_handles(ph)?));
                }
                if let Some(e) = &e {
                    curve(e).ok_or_else(|| format!("ease \"{e}\": {CURVES}"))?;
                }
                keys.push((t, v, e.unwrap_or_else(|| curve_all.clone())));
            }
        }
        keys.sort_by(|a, b| a.0.total_cmp(&b.0));
        // Opacity is 0 to 100 here; keys all within 0..1 are almost always
        // meant as fractions (and stagger passed them on as 1% and 0%).
        if name == "opacity" && !relative {
            let vals: Vec<f64> = keys.iter().filter_map(|k| k.1.as_f64()).collect();
            if vals.len() == keys.len() && vals.iter().all(|v| (0.0..=1.0).contains(v)) && vals.iter().any(|v| *v > 0.0) {
                return Err(format!("opacity runs 0 to 100 (percent), and these keys are all between 0 and 1 ({}): multiply by 100, e.g. [[0, 0], [0.5, 100]]", vals.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", ")));
            }
        }
        for (t, _, _) in &keys {
            if *t < -1e-6 || *t > secs(c.dur) + 1e-3 {
                // Key times count from the clip's start; a time that lands
                // inside the clip on the timeline was almost surely meant so.
                let on_timeline = offset == 0.0 && *t >= secs(c.start) - 1e-6 && *t <= secs(c.end()) + 1e-3;
                let hint = if on_timeline {
                    format!(". {:.3}s is inside the clip on the timeline ({:.2}..{:.2}s): add \"timeline\": true to give timeline times, or use {:.3}", t, secs(c.start), secs(c.end()), t - secs(c.start))
                } else if offset == 0.0 {
                    format!("; key times count from the clip's start (it runs {:.2}..{:.2}s on the timeline; \"timeline\": true takes timeline times)", secs(c.start), secs(c.end()))
                } else {
                    String::new()
                };
                return Err(format!("key time {:.3}s is outside the clip (0..{:.2}s from its start{}){hint}", t + offset, secs(c.dur), if offset > 0.0 { format!("; on the timeline {:.2}..{:.2}s", secs(c.start), secs(c.end())) } else { String::new() }));
            }
        }
        // Values as the model holds them.
        let mut values: Vec<m::Value> = Vec::new();
        for (t, v, _) in &keys {
            let mv = if position {
                let a = v.as_array().filter(|a| a.len() == 2).ok_or("position values are [x, y]")?;
                let (mut x, mut y) = (a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0));
                if relative {
                    x += base[0];
                    y += base[1];
                }
                m::Value::Point(m::Vec2 { x, y })
            } else if numeric {
                let mut x = v.as_f64().ok_or_else(|| format!("{name} takes numbers"))?;
                if relative {
                    if multiply && x.abs() > 20.0 {
                        return Err(format!("relative {name} values are factors (1.15 = 15% more), not {x}"));
                    }
                    x = if multiply { base[0] * x } else { base[0] + x };
                }
                let cur = match prop.conv {
                    super::props::Conv::PointX | super::props::Conv::PointY => mc.property_at(&path0, micros(*t)),
                    _ => Some(now.clone()),
                };
                super::props::to_model(&prop, &path0, &json!(x), cur.as_ref())?
            } else {
                super::props::to_model(&prop, &path0, v, Some(&now))?
            };
            values.push(mv);
        }
        // Position: the path between keys, bent smoothly through them, in
        // arcs, or by each key's own handles.
        let mut paths: Vec<Option<m::PathHandles>> = vec![None; keys.len()];
        if position {
            let pts: Vec<[f64; 2]> = values.iter().map(|v| match v { m::Value::Point(p) => [p.x, p.y], _ => [0.0, 0.0] }).collect();
            match op.get("path") {
                None | Some(Value::Null) => {}
                Some(Value::String(s)) if s == "straight" => {}
                Some(Value::String(s)) if s == "smooth" || s == "curved" => paths = smooth_path(&pts),
                Some(other) => return Err(format!("path is \"smooth\" (a curve through every point) or \"straight\", not {other}; bend single moves with \"arc\"")),
            }
            if let Some(a) = op.get("arc") {
                let bends: Vec<f64> = match a {
                    Value::Array(l) => l.iter().map(|x| x.as_f64().unwrap_or(0.0)).collect(),
                    v => vec![v.as_f64().ok_or("arc is how far each move bows sideways, a fraction of its length (0.3; negative bows the other way), or a list, one per move")?; pts.len().saturating_sub(1)],
                };
                arc_path(&pts, &bends, &mut paths);
            }
            for (t, ph) in &own_paths {
                if let Some(i) = keys.iter().position(|k| (k.0 - t).abs() < 1e-9) {
                    paths[i] = Some(*ph);
                }
            }
        } else if op.get("path").is_some() || op.get("arc").is_some() {
            return Err(format!("path and arc bend position moves; {name} is not a position"));
        }
        let seq = self.doc.seq();
        for path in &prop.paths {
            let old: Vec<m::Micros> = mc.keyframes.get(path).map(|t| t.keys.iter().map(|k| k.time).collect()).unwrap_or_default();
            for t in old {
                let ok = self.doc.apply("Animate", m::Edit::RemoveKeyframe { seq, clip: m::ClipId(id as u64), property: path.clone(), time: t });
                self.check(ok, "")?;
            }
            for (i, ((t, _, e), v)) in keys.iter().zip(&values).enumerate() {
                // The curve leaving this key sets its out-handle and the next
                // key's in-handle.
                let (interp, out, _, shape) = curve(e).expect("checked");
                let mut key = m::Keyframe::new(micros(*t), v.clone(), interp);
                key.curve = shape;
                key.path = paths[i];
                if let Some(o) = out {
                    key.ease_out = o;
                }
                if i > 0 {
                    if let Some((_, _, Some(inn), _)) = curve(&keys[i - 1].2) {
                        key.ease_in = inn;
                    }
                }
                let ok = self.doc.apply("Animate", m::Edit::SetKeyframe { seq, clip: m::ClipId(id as u64), property: path.clone(), key });
                self.check(ok, "")?;
            }
            // New keys keep the track's modifiers (a loop or a wobble).
            if let Some(mods) = mc.keyframes.get(path).map(|t| t.modifiers.clone()).filter(|m| !m.is_empty())
                && let Some(mut tr) = self.model_clip(id).and_then(|c| c.keyframes.get(path).cloned())
            {
                tr.modifiers = mods;
                let ok = self.doc.apply("Animate", m::Edit::SetTrack { seq, clip: m::ClipId(id as u64), property: path.clone(), track: Some(tr) });
                self.check(ok, "")?;
            }
        }
        // A box anchored at a side grows from that side: a bar fills from
        // its left edge. The anchor follows the size with the same keys
        // and curves, so the anchored edge stays exactly where it is.
        if matches!(name.as_str(), "width" | "height" | "box_width" | "box_height") && !relative {
            let horizontal = name.ends_with("width");
            let anchor = match mc.property_at("transform.anchor", 0) {
                Some(m::Value::Point(p)) => if horizontal { p.x } else { p.y },
                _ => 0.0,
            };
            let half = base[0] / 2.0;
            let side = if half > 1e-6 { anchor / half } else { 0.0 };
            if side.abs() > 0.25 {
                let side = side.clamp(-1.0, 1.0);
                let follow: Vec<Value> = keys.iter().map(|(t, v, e)| json!({"t": t, "v": side * v.as_f64().unwrap_or(0.0) / 2.0, "ease": e})).collect();
                self.animate(&json!({"op": "animate", "clip": id, "property": if horizontal { "anchor_x" } else { "anchor_y" }, "keys": follow}))?;
                return Ok(format!("{} on {name} of clip {id}, growing from its {} edge", self.keys_kept(id, &prop.paths, keys.len()), match (horizontal, side < 0.0) { (true, true) => "left", (true, false) => "right", (false, true) => "top", (false, false) => "bottom" }));
            }
        }
        Ok(format!("{} on {name} of clip {id}", self.keys_kept(id, &prop.paths, keys.len())))
    }

    /// What a write of `asked` keys left on the tracks of `paths`: keys on
    /// the same moment merge, and the reply must say so rather than echo
    /// the request.
    fn keys_kept(&self, id: u32, paths: &[String], asked: usize) -> String {
        let mut tally = KeyTally::new(asked);
        tally.settle(self.model_clip(id).map_or(0, |c| paths.iter().filter_map(|p| c.keyframes.get(p)).map(|t| t.keys.len()).min().unwrap_or(0)), SAME_MOMENT);
        tally.say()
    }

    /// How many keys the stored track(s) of `property` on clip `id` hold.
    pub(super) fn keys_on(&self, id: u32, property: &str) -> usize {
        let Some(c) = self.model_clip(id) else { return 0 };
        let Ok(prop) = super::props::resolve(&c, property) else { return 0 };
        prop.paths.iter().filter_map(|p| c.keyframes.get(p)).map(|t| t.keys.len()).min().unwrap_or(0)
    }

    /// `animate` `speed`: a speed ramp inside one clip, keys of playback
    /// rate (1 = normal) over the clip's time.
    fn speed_curve(&mut self, id: u32, op: &Value) -> Result<String, String> {
        let list = op.get("keys").and_then(Value::as_array).ok_or("animate speed needs \"keys\": [[t, rate], ...] (1 = normal speed)")?;
        let offset = if op.get("timeline").and_then(Value::as_bool).unwrap_or(false) { secs(self.clip(id)?.start) } else { 0.0 };
        let mut ks = Vec::new();
        for k in list {
            let a = k.as_array().filter(|a| a.len() == 2).ok_or("speed keys are [t, rate]")?;
            let (t, r) = (a[0].as_f64().ok_or("t is a number")? - offset, a[1].as_f64().ok_or("rate is a number")?);
            if !(0.01..=100.0).contains(&r) {
                return Err(format!("rate {r} is outside 0.01..100"));
            }
            ks.push(m::Keyframe::new(micros(t), m::Value::Scalar(r), m::Interp::Auto));
        }
        let seq = self.doc.seq();
        let ok = self.doc.apply("Speed ramp", m::Edit::SetSpeed { seq, clip: m::ClipId(id as u64), speed: m::Speed::Curve(m::Track::new(ks)) });
        self.check(ok, &format!("speed ramp on clip {id}")).map_err(|e| if e.contains("SourceOutOfRange") { format!("that ramp needs more footage than clip {id} has after its source_in; slip it earlier or ramp less ({e})") } else { e })
    }
    /// Text motion on a title: `reveal` (entrance), `exit`, `loop`, and
    /// `unit_motion` (custom per-letter / word / line keys, one or a list),
    /// merged into what the title already has.
    pub(super) fn text_motion(&mut self, id: u32, op: &Value) -> Result<Option<String>, String> {
        if !TEXT_MOTION_FIELDS.iter().any(|k| op.get(*k).is_some()) {
            return Ok(None);
        }
        let mut mc = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
        let m::ClipSource::Generator(g) = &mut mc.source else { return Err(format!("clip {id} is not a title; text motion is for titles")) };
        if g.kind != m::GeneratorKind::Text {
            return Err(format!("clip {id} is not a title; text motion is for titles"));
        }
        let mut said = self.text_extras(g, op)?;
        let mut cur: Value = match g.params.get("motion") {
            Some(m::Value::Text(t)) => serde_json::from_str(t).unwrap_or(json!({})),
            _ => json!({}),
        };
        let reveal = |v: &Value, what: &str| -> Result<Value, String> {
            let mut o = match v {
                Value::String(k) => json!({"kind": k}),
                Value::Object(_) => v.clone(),
                _ => return Err(format!("{what} is a kind ({REVEALS}) or {{\"kind\", \"unit\", \"duration\", \"overlap\", \"order\", \"ease\"}}")),
            };
            unit_word(&mut o);
            Ok(o)
        };
        for (field, dur_key) in [("reveal", "reveal_duration"), ("exit", "exit_duration")] {
            match op.get(field) {
                Some(Value::Null) | Some(Value::Bool(false)) => {
                    cur[field] = Value::Null;
                    said.push(format!("no {field}"));
                }
                Some(v) => {
                    let mut o = reveal(v, field)?;
                    if let Some(d) = o.as_object_mut().and_then(|m| m.remove("duration")) {
                        cur[dur_key] = d;
                    }
                    said.push(format!("{field} {}", o["kind"].as_str().unwrap_or("?")));
                    cur[field] = o;
                }
                None => {}
            }
        }
        match op.get("loop") {
            Some(Value::Null) | Some(Value::Bool(false)) => cur["loop"] = Value::Null,
            Some(v) => {
                let mut o = match v {
                    Value::String(k) => json!({"kind": k}),
                    _ => v.clone(),
                };
                unit_word(&mut o);
                said.push(format!("loop {}", o["kind"].as_str().unwrap_or("?")));
                cur["loop"] = o;
            }
            None => {}
        }
        if let Some(v) = op.get("unit_motion") {
            let mut list: Vec<Value> = match v {
                Value::Array(a) => a.clone(),
                Value::Null | Value::Bool(false) => Vec::new(),
                other => vec![other.clone()],
            };
            for o in list.iter_mut() {
                unit_word(o);
            }
            said.push(format!("{} custom motion(s)", list.len()));
            cur["custom"] = Value::Array(list);
        }
        // Animators: `animators` replaces the list, `animator` adds one. A
        // name is a preset (with `unit`, `start`, `duration`), an object is
        // an animator (or a preset with overrides).
        let animator = |v: &Value| -> Result<Value, String> {
            let (name, over) = match v {
                Value::String(s) => (Some(s.clone()), json!({})),
                Value::Object(o) => (o.get("preset").and_then(Value::as_str).map(str::to_owned), v.clone()),
                _ => return Err("an animator is a preset name or {\"range\", \"position\", \"rotation\", \"scale\", \"opacity\", \"colour\", \"blur\", \"tracking\", …}".into()),
            };
            let mut o = over.clone();
            unit_word(&mut o);
            let mut base = match name {
                Some(n) => {
                    let unit: studio_text::TextUnit = serde_json::from_value(o.get("unit").cloned().unwrap_or(json!("character"))).map_err(|e| format!("animator unit: {e}"))?;
                    let start = o.get("start").and_then(Value::as_f64).unwrap_or(0.0) as f32;
                    let dur = o.get("duration").and_then(Value::as_f64).unwrap_or(1.0) as f32;
                    let a = studio_text::animator::preset(&n, unit, start, dur).ok_or_else(|| format!("no animator preset \"{n}\"; presets: {}", studio_text::animator::PRESETS.join(", ")))?;
                    serde_json::to_value(a).map_err(|e| e.to_string())?
                }
                None => json!({}),
            };
            if let (Some(b), Some(ov)) = (base.as_object_mut(), o.as_object()) {
                for (k, x) in ov {
                    if !matches!(k.as_str(), "preset" | "start" | "duration") {
                        b.insert(k.clone(), x.clone());
                    }
                }
            }
            Ok(base)
        };
        if let Some(v) = op.get("animators") {
            let list: Vec<Value> = match v {
                Value::Array(a) => a.iter().map(animator).collect::<Result<_, _>>()?,
                Value::Null | Value::Bool(false) => Vec::new(),
                other => vec![animator(other)?],
            };
            said.push(format!("{} animator(s)", list.len()));
            cur["animators"] = Value::Array(list);
        }
        if let Some(v) = op.get("animator") {
            let a = animator(v)?;
            match cur.get_mut("animators").and_then(Value::as_array_mut) {
                Some(l) => l.push(a),
                None => cur["animators"] = json!([a]),
            }
            said.push("an animator added".into());
        }
        // Checked by the text engine itself, so nothing it cannot play
        // gets stored.
        let parsed: studio_text::TextMotion = serde_json::from_value(cur.clone()).map_err(|e| format!("text motion not understood ({e}); reveal / exit kinds: {REVEALS}; loop kinds: {LOOPS}; units: letter, word, line; orders: forward, backward, centre, random; eases: linear, smooth, in, out, in_out, overshoot, anticipate, elastic, bounce, hold"))?;
        let text = serde_json::to_string(&parsed).map_err(|e| e.to_string())?;
        g.params.insert("motion".into(), m::Value::Text(text));
        let seq = self.doc.seq();
        let ok = self.doc.apply("Text motion", m::Edit::SetClip { seq, clip: Box::new(mc) });
        self.check(ok, "")?;
        Ok(Some(said.join(", ")))
    }

    /// `keys`: edit single keyframes of one property: remove some (or
    /// all), move one, or change a key's curve and bezier handles.
    pub(super) fn keys(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or("keys needs \"clip\"")? as u32;
        let mc = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
        let name = op.get("property").and_then(Value::as_str).ok_or("keys needs \"property\" (or \"all\": every animated property, as the timeline's key marks do)")?.trim().to_lowercase();
        // "all": every track, so a moment's keys move and ease together.
        let paths: Vec<String> = if name == "all" {
            mc.keyframes.iter().filter(|(_, t)| !t.keys.is_empty()).map(|(p, _)| p.clone()).collect()
        } else {
            super::props::resolve(&mc, if name == "position" { "x" } else { &name })?.paths
        };
        if paths.is_empty() {
            return Err(format!("clip {id} has no keys"));
        }
        let seq = self.doc.seq();
        let cid = m::ClipId(id as u64);
        let mut n = 0;
        for path in &paths {
            // With "all", a track without a key at the asked moment is skipped.
            let all = name == "all";
            let track = mc.keyframes.get(path).cloned().ok_or_else(|| format!("{name} on clip {id} has no keys"))?;
            let find = |t: f64| track.keys.iter().min_by_key(|k| (k.time - micros(t)).abs()).filter(|k| (k.time - micros(t)).abs() <= SEC / 20).cloned().ok_or_else(|| format!("no key on {name} near {t}s; keys at {:?}", track.keys.iter().map(|k| k.time as f64 / 1e6).collect::<Vec<_>>()));
            match op.get("remove") {
                // "all", or just true: every key of the property.
                Some(Value::Bool(true)) => {
                    for k in &track.keys {
                        let ok = self.doc.apply("Remove key", m::Edit::RemoveKeyframe { seq, clip: cid, property: path.clone(), time: k.time });
                        self.check(ok, "")?;
                        n += 1;
                    }
                }
                Some(Value::String(s)) if s.eq_ignore_ascii_case("all") => {
                    for k in &track.keys {
                        let ok = self.doc.apply("Remove key", m::Edit::RemoveKeyframe { seq, clip: cid, property: path.clone(), time: k.time });
                        self.check(ok, "")?;
                        n += 1;
                    }
                }
                Some(Value::Array(ts)) => {
                    for t in ts.iter().filter_map(Value::as_f64) {
                        let k = match find(t) {
                            Ok(k) => k,
                            Err(_) if all => continue,
                            Err(e) => return Err(e),
                        };
                        let ok = self.doc.apply("Remove key", m::Edit::RemoveKeyframe { seq, clip: cid, property: path.clone(), time: k.time });
                        self.check(ok, "")?;
                        n += 1;
                    }
                }
                Some(Value::Bool(false)) | Some(Value::Null) => {}
                Some(_) => return Err("remove is a list of key times, or \"all\" (true also means all)".into()),
                None => {}
            }
            if let Some(Value::Array(moves)) = op.get("move") {
                for mv in moves {
                    let pair: Vec<f64> = mv.as_array().map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
                    let [from, to] = pair[..] else { return Err("move is a list of [from, to] key times".into()) };
                    let mut k = match find(from) {
                        Ok(k) => k,
                        Err(_) if all => continue,
                        Err(e) => return Err(e),
                    };
                    let ok = self.doc.apply("Move key", m::Edit::RemoveKeyframe { seq, clip: cid, property: path.clone(), time: k.time });
                    self.check(ok, "")?;
                    k.time = micros(to);
                    let ok = self.doc.apply("Move key", m::Edit::SetKeyframe { seq, clip: cid, property: path.clone(), key: k });
                    self.check(ok, "")?;
                    n += 1;
                }
            }
            if let Some(Value::Array(curves)) = op.get("curve") {
                // [{"t": 0.5, "curve": "ease", "out": [0.2, 0], "in": [0.6, 1]}]
                for cv in curves {
                    let t = cv.get("t").and_then(Value::as_f64).ok_or("each curve entry needs \"t\"")?;
                    // Keys as they are now (an earlier entry may have moved one).
                    let now = self.model_clip(id).and_then(|c| c.keyframes.get(path).cloned()).unwrap_or_else(|| track.clone());
                    let near = now.keys.iter().min_by_key(|k| (k.time - micros(t)).abs()).filter(|k| (k.time - micros(t)).abs() <= SEC / 20).cloned();
                    let mut k = match (near, find(t)) {
                        (Some(k), _) => k,
                        (None, Ok(k)) => k,
                        (None, Err(_)) if all => continue,
                        (None, Err(e)) => return Err(e),
                    };
                    // A named curve: this key's leaving handle and the next
                    // key's arriving one.
                    let mut next_in = None;
                    if let Some(c) = cv.get("curve").and_then(Value::as_str) {
                        let (interp, out, inn, shape) = curve(c).ok_or_else(|| format!("curve \"{c}\": {CURVES}"))?;
                        k.interp = interp;
                        k.curve = shape;
                        if let Some(h) = out {
                            k.ease_out = h;
                        }
                        next_in = inn;
                    }
                    let handle = |v: Option<&Value>| v.and_then(Value::as_array).map(|a| m::Handle { x: a.first().and_then(Value::as_f64).unwrap_or(0.42).clamp(0.0, 1.0), y: a.get(1).and_then(Value::as_f64).unwrap_or(0.0) });
                    if let Some(h) = handle(cv.get("out")) {
                        k.ease_out = h;
                        k.interp = m::Interp::Ease;
                    }
                    if let Some(h) = handle(cv.get("in")) {
                        k.ease_in = h;
                    }
                    // Bending the path through a position key.
                    if let Some(ph) = cv.get("path") {
                        if !matches!(k.value, m::Value::Point(_)) {
                            return Err("a key's \"path\" bends position keys only".into());
                        }
                        k.path = if ph.is_null() { None } else { Some(path_handles(ph)?) };
                    }
                    let after = now.keys.iter().find(|x| x.time > k.time).cloned();
                    let ok = self.doc.apply("Key curve", m::Edit::SetKeyframe { seq, clip: cid, property: path.clone(), key: k });
                    self.check(ok, "")?;
                    if let (Some(h), Some(mut nk)) = (next_in, after) {
                        nk.ease_in = h;
                        let ok = self.doc.apply("Key curve", m::Edit::SetKeyframe { seq, clip: cid, property: path.clone(), key: nk });
                        self.check(ok, "")?;
                    }
                    n += 1;
                }
            }
        }
        Ok(format!("{n} key change(s) on {name} of clip {id}"))
    }

    /// Properties `set` does not name itself: any friendly property
    /// (`crop_left`, `anchor_x`, `zoom_blur.amount`, `mask1.feather`,
    /// `generator.size`…), plus `blend`, `reversed`, `enabled`.
    pub(super) fn set_properties(&mut self, id: u32, fields: &serde_json::Map<String, Value>) -> Result<Vec<String>, String> {
        if fields.is_empty() {
            return Ok(Vec::new());
        }
        let mut mc = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
        let mut done = Vec::new();
        let mut keyed = Vec::new();
        for (k, v) in fields {
            match k.as_str() {
                "blend" => {
                    let s = v.as_str().ok_or("blend is a name: normal, add, multiply, screen, overlay, soft_light, hard_light, colour_dodge, colour_burn, darken, lighten, difference, exclusion, hue, saturation, colour, luminosity")?;
                    let want = s.to_lowercase().replace([' ', '-', '_'], "");
                    let mode = ["Normal", "Add", "Multiply", "Screen", "Overlay", "SoftLight", "HardLight", "ColourDodge", "ColourBurn", "Darken", "Lighten", "Difference", "Exclusion", "Hue", "Saturation", "Colour", "Luminosity"]
                        .iter()
                        .find(|m| m.to_lowercase() == want.replace("color", "colour"))
                        .ok_or_else(|| format!("no blend mode \"{s}\""))?;
                    mc.blend = serde_json::from_value(json!(mode)).or_else(|_| serde_json::from_value(json!(mode.to_lowercase()))).or_else(|_| {
                        let snake: String = mode.chars().enumerate().flat_map(|(i, c)| if c.is_uppercase() && i > 0 { vec!['_', c.to_ascii_lowercase()] } else { vec![c.to_ascii_lowercase()] }).collect();
                        serde_json::from_value(json!(snake))
                    }).map_err(|e| format!("blend: {e}"))?;
                }
                "reversed" => mc.reversed = v.as_bool().ok_or("reversed is true or false")?,
                "enabled" | "visible" => mc.enabled = v.as_bool().ok_or("enabled is true or false")?,
                _ => {
                    let prop = super::props::resolve(&mc, k)?;
                    if prop.paths[0].starts_with("grade.") && mc.grade.is_none() {
                        mc.grade = Some(Default::default());
                    }
                    for path in &prop.paths {
                        let cur = super::props::value_or_default(&mc, path);
                        let val = super::props::to_model(&prop, path, v, cur.as_ref())?;
                        if !mc.set_property(path, val) {
                            return Err(format!("{k} could not take {v}"));
                        }
                        if mc.keyframes.get(path).is_some_and(|t| !t.keys.is_empty()) {
                            keyed.push(k.clone());
                        }
                    }
                }
            }
            done.push(k.clone());
        }
        let seq = self.doc.seq();
        let ok = self.doc.apply("Change clip", m::Edit::SetClip { seq, clip: Box::new(mc) });
        self.check(ok, "")?;
        if !keyed.is_empty() {
            done.push(format!("(note: {} animated, so the keys still decide; use keys or animate)", keyed.join(", ")));
        }
        Ok(done)
    }
}

/// The text motion fields an op may carry, built into the title's motion.
pub const TEXT_MOTION_FIELDS: &[&str] = &["reveal", "exit", "loop", "unit_motion", "animators", "animator", "runs", "path", "kerning", "font", "weight", "italic"];

const REVEALS: &str = "fade, rise, drop, pop, slide_left, slide_right, type, spin, bounce, zoom, flip, spring";
const LOOPS: &str = "wave, pulse, jitter, sway, shimmer, wobble";

/// Plain unit words to the text engine's.
fn unit_word(v: &mut Value) {
    if let Some(o) = v.as_object_mut() {
        if let Some(u) = o.get("unit").and_then(Value::as_str) {
            let n = match u {
                "letter" | "letters" | "char" | "chars" | "character" | "characters" => "character",
                "word" | "words" => "word",
                "line" | "lines" => "line",
                other => other,
            };
            o.insert("unit".into(), json!(n));
        }
        if let Some(Value::String(c)) = o.get("colour").cloned() {
            if let Some(k) = colour(&c) {
                o.insert("colour".into(), json!({"r": k.r() as f32 / 255.0, "g": k.g() as f32 / 255.0, "b": k.b() as f32 / 255.0, "a": 1.0}));
            }
        }
    }
}

/// Named curves an animation can move along.
pub const CURVES: &str = "smooth, linear, hold, ease_in_out (or ease), ease_in, ease_out, overshoot, anticipate, snap, whip; and the GSAP family, each with _in, _out (the default) or _in_out: elastic (rings and settles; elastic(1.2, 0.4) = amplitude, period), spring (a softer elastic), bounce, back (back(2.5) = how far past), expo, circ, sine, power1..power4 (or quad, cubic, quart, quint), steps(6). GSAP spellings work too: \"elastic.out(1, 0.3)\", \"power3.inOut\", \"steps(12)\"";

/// A curve by name: how keys interpolate, for eased curves the handle
/// leaving the key and the one arriving at the next (fractions of the
/// segment's time and change; a value past 1 overshoots), and for the
/// GSAP family the timing curve itself.
pub fn curve(name: &str) -> Option<(m::Interp, Option<m::Handle>, Option<m::Handle>, Option<m::Curve>)> {
    let h = |x: f64, y: f64| Some(m::Handle { x, y });
    Some(match name {
        "smooth" | "auto" => (m::Interp::Auto, None, None, None),
        "linear" | "none" => (m::Interp::Linear, None, None, None),
        "hold" | "step" => (m::Interp::Hold, None, None, None),
        // The short names (as the shape keys and CSS write them) too.
        "ease" | "ease_in_out" | "in_out" | "inout" | "ease-in-out" | "easeinout" => (m::Interp::Ease, h(0.42, 0.0), h(0.42, 0.0), None),
        "ease_in" | "in" | "ease-in" | "easein" => (m::Interp::Ease, h(0.42, 0.0), h(0.0, 0.0), None),
        "ease_out" | "out" | "ease-out" | "easeout" => (m::Interp::Ease, h(0.0, 0.0), h(0.42, 0.0), None),
        // Past the target, then settles back.
        "overshoot" => (m::Interp::Ease, h(0.34, 1.56), h(0.36, 0.0), None),
        // Pulls back first, then goes.
        "anticipate" => (m::Interp::Ease, h(0.36, 0.0), h(0.34, 1.56), None),
        // Arrives almost at once, then eases in: hits on beats.
        "snap" => (m::Interp::Ease, h(0.16, 1.0), h(0.7, 0.0), None),
        // Leaves slowly, arrives fast: whip moves into a cut.
        "whip" => (m::Interp::Ease, h(0.7, 0.0), h(0.16, 1.0), None),
        other => (m::Interp::Curve, None, None, Some(named_curve(other)?)),
    })
}

/// The GSAP family by name: `elastic`, `bounce_in`, `back(2.5)`,
/// `elastic.out(1, 0.3)`, `power3.inOut`, `steps(6)`.
pub fn named_curve(name: &str) -> Option<m::Curve> {
    use m::{Curve as C, CurveEnd as E};
    let s: String = name.trim().to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    let (head, args) = match s.split_once('(') {
        Some((h, a)) => (h.to_owned(), a.trim_end_matches(')').split(',').map(|x| x.parse::<f64>().ok()).collect::<Option<Vec<f64>>>()?),
        None => (s.clone(), Vec::new()),
    };
    let head = head.replace(['.', '-'], "_").replace("inout", "in_out").replace("easein", "in").replace("easeout", "out");
    let (base, end) = if let Some(b) = head.strip_suffix("_in_out") {
        (b, E::InOut)
    } else if let Some(b) = head.strip_suffix("_out") {
        (b, E::Out)
    } else if let Some(b) = head.strip_suffix("_in") {
        (b, E::In)
    } else {
        (head.as_str(), E::Out)
    };
    let arg = |i: usize, d: f64| args.get(i).copied().filter(|v| v.is_finite()).unwrap_or(d);
    let power = |p: f64| C::Power { power: p, end };
    Some(match base {
        "elastic" => C::Elastic { amplitude: arg(0, 1.0), period: arg(1, 0.3), end },
        "spring" => C::Elastic { amplitude: arg(0, 1.0), period: arg(1, 0.55), end },
        "bounce" => C::Bounce { end },
        "back" => C::Back { overshoot: arg(0, 1.70158), end },
        "expo" => C::Expo { end },
        "circ" => C::Circ { end },
        "sine" => C::Sine { end },
        "power0" => power(1.0),
        "power1" | "quad" => power(2.0),
        "power2" | "cubic" => power(3.0),
        "power3" | "quart" => power(4.0),
        "power4" | "quint" | "strong" => power(5.0),
        "power" => power(arg(0, 3.0)),
        "steps" => C::Steps { count: arg(0, 4.0).clamp(1.0, 240.0) as u32 },
        b => C::Steps { count: b.strip_prefix("steps_")?.parse::<u32>().ok()?.clamp(1, 240) },
    })
}

/// How a GSAP-family curve is spelled back: `elastic.out(1, 0.3)`.
pub fn curve_label(c: m::Curve) -> String {
    use m::{Curve as C, CurveEnd as E};
    let end = |e: E| match e {
        E::In => "in",
        E::Out => "out",
        E::InOut => "inOut",
    };
    let n = |x: f64| format!("{}", (x * 1000.0).round() / 1000.0);
    match c {
        C::Elastic { amplitude, period, end: e } => format!("elastic.{}({}, {})", end(e), n(amplitude), n(period)),
        C::Bounce { end: e } => format!("bounce.{}", end(e)),
        C::Back { overshoot, end: e } => format!("back.{}({})", end(e), n(overshoot)),
        C::Expo { end: e } => format!("expo.{}", end(e)),
        C::Circ { end: e } => format!("circ.{}", end(e)),
        C::Sine { end: e } => format!("sine.{}", end(e)),
        C::Power { power, end: e } if [2.0, 3.0, 4.0, 5.0].contains(&power) => format!("power{}.{}", power as u32 - 1, end(e)),
        C::Power { power, end: e } => format!("power({}).{}", n(power), end(e)),
        C::Steps { count } => format!("steps({count})"),
    }
}

/// `{"in": [dx, dy], "out": [dx, dy]}`: a position key's path handles.
fn path_handles(v: &Value) -> Result<m::PathHandles, String> {
    let pair = |k: &str| -> Result<[f64; 2], String> {
        match v.get(k) {
            None | Some(Value::Null) => Ok([0.0, 0.0]),
            Some(Value::Array(a)) if a.len() == 2 => Ok([a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0)]),
            Some(other) => Err(format!("path {k} is [dx, dy] in pixels from the key's point, not {other}")),
        }
    };
    Ok(m::PathHandles { inn: pair("in")?, out: pair("out")? })
}

/// Handles that curve a path smoothly through every point (Catmull-Rom),
/// leaving its two ends to start and stop along the path.
pub fn smooth_path(pts: &[[f64; 2]]) -> Vec<Option<m::PathHandles>> {
    (0..pts.len())
        .map(|i| {
            let prev = if i > 0 { pts[i - 1] } else { pts[i] };
            let next = pts.get(i + 1).copied().unwrap_or(pts[i]);
            // At the ends: a third of the way to the neighbour.
            let k = if i == 0 || i + 1 == pts.len() { 1.0 / 3.0 } else { 1.0 / 6.0 };
            let t = [(next[0] - prev[0]) * k, (next[1] - prev[1]) * k];
            (pts.len() > 2).then_some(m::PathHandles { inn: [-t[0], -t[1]], out: t })
        })
        .collect()
}

/// Bows each move sideways by `bends[i]` of its length (positive to the
/// left of the direction of travel as seen on screen).
pub fn arc_path(pts: &[[f64; 2]], bends: &[f64], paths: &mut [Option<m::PathHandles>]) {
    for i in 0..pts.len().saturating_sub(1) {
        let b = bends.get(i).copied().unwrap_or(0.0);
        if b == 0.0 {
            continue;
        }
        let d = [pts[i + 1][0] - pts[i][0], pts[i + 1][1] - pts[i][1]];
        let len = d[0].hypot(d[1]);
        if len < 1e-9 {
            continue;
        }
        // Left of travel with y down; a cubic's middle sits 3/4 of the
        // way to its handles, so they reach 4/3 of the bow.
        let n = [d[1] / len, -d[0] / len];
        let k = 4.0 / 3.0 * b * len;
        let out = [d[0] / 3.0 + n[0] * k, d[1] / 3.0 + n[1] * k];
        let inn = [-d[0] / 3.0 + n[0] * k, -d[1] / 3.0 + n[1] * k];
        paths[i].get_or_insert_with(Default::default).out = out;
        paths[i + 1].get_or_insert_with(Default::default).inn = inn;
    }
}

fn services_new_clip(at: view::Time, dur: view::Time, kind: ClipKind, name: &str, media: Option<u32>) -> view::Clip {
    crate::services::library::new_clip(at, dur, kind, name, media)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Doc;
    use serde_json::json;

    fn session() -> Session {
        Session::new(Doc::from_view(&view::demo()))
    }

    fn added(r: &Value) -> u32 {
        r["changed"]["added"][0]["id"].as_u64().expect("a clip was added") as u32
    }

    fn notes(r: &Value) -> String {
        r["notes"].to_string()
    }

    fn most_keys(s: &Session, id: u32) -> usize {
        s.model_clip(id).unwrap().keyframes.values().map(|t| t.keys.len()).max().unwrap_or(0)
    }

    /// Keys that land on the same moment merge; the reply says how many
    /// were kept, not only how many were asked for.
    #[test]
    fn animate_reports_the_keys_it_kept() {
        let mut s = session();
        let id = added(&s.edit(&[json!({"op": "card", "colour": "#ff0000", "at": 0, "duration": 2})], "card", false).unwrap());
        let r = s.edit(&[json!({"op": "animate", "clip": id, "property": "x", "keys": [[0, 0], [0.0000001, 5], [1, 10]]})], "x", false).unwrap();
        let kept = most_keys(&s, id);
        assert_eq!(kept, 2, "two keys round to the same microsecond");
        assert!(notes(&r).contains("2 of 3 keys kept"), "{}", notes(&r));
    }

    /// A formula's undefined samples are skipped, and the reply counts
    /// what was stored, not what was sampled.
    #[test]
    fn a_formula_reports_skipped_samples() {
        let mut s = session();
        let id = added(&s.edit(&[json!({"op": "card", "colour": "#00ff00", "at": 0, "duration": 2})], "card", false).unwrap());
        let r = s.edit(&[json!({"op": "animate", "clip": id, "property": "x", "formula": "sqrt(t - 1) * 100"})], "f", false).unwrap();
        let kept = most_keys(&s, id);
        let said = notes(&r);
        assert!(said.contains(&format!("{kept} of")) && said.contains("dropped"), "{said}");
        assert!(said.contains("undefined, skipped"), "{said}");
    }

    #[test]
    fn a_tally_says_written_kept_and_dropped() {
        let mut t = KeyTally::new(10);
        assert_eq!(t.say(), "10 keys");
        t.lose(2, "undefined");
        t.settle(7, SAME_MOMENT);
        assert_eq!((t.written, t.kept, t.dropped_total()), (10, 7, 3));
        assert!(t.say().starts_with("7 of 10 keys kept, 3 dropped (2 undefined; 1 landed"), "{}", t.say());
        assert_eq!(t.json()["dropped"], 3);
    }

    /// Agents send ids as strings ("8") as often as numbers.
    #[test]
    fn ids_are_numbers_or_numeric_strings_everywhere() {
        let mut s = session();
        let p = s.doc.project.clone();
        let (media, lane) = (p.media[0].id, p.lanes[0].id);
        assert_eq!(media_of(&p, &json!(media.to_string())).unwrap(), media);
        assert_eq!(media_of(&p, &json!(format!("#{media}"))).unwrap(), media);
        assert_eq!(lane_of(&p, &json!(lane.to_string()), None).unwrap(), lane);
        assert_eq!(super::super::ids(&json!({"clips": [p.clips[0].id.to_string(), p.clips[1].id]}), "clips"), vec![p.clips[0].id, p.clips[1].id]);
        assert_eq!(super::super::ids(&json!({"clips": p.clips[0].id.to_string()}), "clips"), vec![p.clips[0].id]);
        let r = s.edit(&[json!({"op": "place", "media": media.to_string(), "at": 30.0, "lane": lane.to_string()})], "place", false).unwrap();
        let c = s.clip(added(&r)).unwrap();
        assert_eq!((c.media, c.lane), (Some(media), lane));
        // The tools that look at footage take it by a string id too.
        let e = s.call("frame", &json!({"media": media.to_string(), "t": 0.5})).err().unwrap_or_default();
        assert!(!e.contains("no media"), "the id was found: {e}");
    }

    #[test]
    fn place_takes_a_named_piece_of_a_file() {
        let mut s = session();
        let r = s.edit(&[json!({"op": "place", "media": "autumn road", "at": 30.0, "lane": "overlay", "source_in": 2.0, "duration": 3.0, "speed": 2.0, "fit": "fill"})], "place", false).unwrap();
        let c = s.clip(added(&r)).unwrap();
        assert_eq!(s.doc.project.media(c.media.unwrap()).unwrap().name, "Autumn road");
        assert_eq!(c.src_in, 2 * SEC);
        assert_eq!(c.speed, 200.0);
        assert_eq!(c.dur, 3 * SEC, "duration is time on the timeline (6 s of source at 2x)");
        assert_eq!(s.doc.project.lane(c.lane).unwrap().kind, LaneKind::Visual);
        assert!(c.transform.scale >= 100.0);
    }

    #[test]
    fn place_refuses_a_range_outside_the_file_and_names_the_media() {
        let mut s = session();
        let e = s.edit(&[json!({"op": "place", "media": "autumn road", "source_in": 50.0})], "place", false).unwrap_err();
        assert!(e.contains("outside"), "{e}");
        let e = s.edit(&[json!({"op": "place", "media": "no such thing"})], "place", false).unwrap_err();
        assert!(e.contains("Autumn road"), "lists the media: {e}");
    }

    #[test]
    fn animate_uses_set_units() {
        let mut s = session();
        let id = s.doc.project.clips.iter().find(|c| c.kind == ClipKind::Video).unwrap().id;
        s.edit(&[json!({"op": "animate", "clip": id, "property": "scale", "keys": [[0.0, 100.0], [1.0, 150.0]]})], "zoom", false).unwrap();
        let mc = s.doc.model.sequences.iter().find_map(|q| q.clip(m::ClipId(id as u64))).unwrap().clone();
        let v = mc.property_at("transform.scale_x", micros(1.0)).unwrap();
        assert_eq!(v, m::Value::Scalar(1.5), "150% in set units is 1.5 in the model");
        // Again replaces, not adds to, the earlier keys.
        s.edit(&[json!({"op": "animate", "clip": id, "property": "scale", "keys": [[0.0, 120.0]]})], "zoom", false).unwrap();
        let c = s.clip(id).unwrap();
        assert_eq!(c.keys.iter().find(|(p, _)| p == "transform.scale_x").map(|(_, t)| t.len()), Some(1));
        let e = s.edit(&[json!({"op": "animate", "clip": id, "property": "wobble", "keys": [[0.0, 1.0]]})], "x", false).unwrap_err();
        assert!(e.contains("no property"), "{e}");
    }

    #[test]
    fn check_finds_a_clip_past_its_footage() {
        let mut s = session();
        let p = &s.doc.project;
        let id = p.clips.iter().find(|c| c.media.and_then(|m| p.media(m)).is_some_and(|m| m.kind == MediaKind::Video)).unwrap().id;
        // The file on disk turned out shorter than the project recorded.
        let media = s.clip(id).unwrap().media.unwrap();
        std::sync::Arc::make_mut(&mut s.doc.project).media.iter_mut().find(|m| m.id == media).unwrap().dur = SEC;
        let r = s.quality(false);
        let kinds: Vec<&str> = r["problems"].as_array().unwrap().iter().filter_map(|p| p["kind"].as_str()).collect();
        assert!(kinds.contains(&"past_media_end"), "{r}");
    }

    #[test]
    fn main_lane_speed_and_length_ripple() {
        let mut s = session();
        let spine = s.doc.project.spine().unwrap();
        let mut cs: Vec<view::Clip> = s.doc.project.clips.iter().filter(|c| c.lane == spine).cloned().collect();
        cs.sort_by_key(|c| c.start);
        let (a, b) = (cs[0].clone(), cs[1].clone());
        // Slow motion: twice as long, the next clip moves out.
        s.edit(&[json!({"op": "set", "clip": a.id, "speed": 0.5})], "slow", false).unwrap();
        assert_eq!(s.clip(a.id).unwrap().dur, a.dur * 2);
        assert_eq!(s.clip(b.id).unwrap().start, a.start + a.dur * 2);
        // Back to normal and shorter: the next clip follows it in.
        s.edit(&[json!({"op": "set", "clip": a.id, "speed": 1.0, "duration": 1.0})], "short", false).unwrap();
        assert_eq!(s.clip(a.id).unwrap().dur, SEC);
        assert_eq!(s.clip(b.id).unwrap().start, a.start + SEC);
        let e = s.edit(&[json!({"op": "set", "clip": a.id, "start": 3.0})], "x", false).unwrap_err();
        assert!(e.contains("magnetic"), "{e}");
    }

    #[test]
    fn wrong_fields_are_refused_and_aliases_mapped() {
        let mut s = session();
        let id = s.doc.project.clips.iter().find(|c| c.kind == ClipKind::Video).unwrap().id;
        let e = s.edit(&[json!({"op": "set", "clip": id, "scael": 120})], "x", false).unwrap_err();
        assert!(e.contains("did you mean \"scale\""), "{e}");
        // The {"place": {...}} form and "color" both work.
        s.edit(&[json!({"set": {"clip": id, "opacity": 50}})], "x", false).unwrap();
        assert_eq!(s.clip(id).unwrap().transform.opacity, 50.0);
        let e = match s.run("frame", &json!({"tt": 1.0})) { Err(e) => e, Ok(_) => panic!("accepted an unknown field") };
        assert!(e.contains("it takes"), "{e}");
    }

    #[test]
    fn relative_punch_in_follows_the_clip_scale() {
        let mut s = session();
        let id = s.doc.project.clips.iter().find(|c| c.kind == ClipKind::Video).unwrap().id;
        s.edit(&[json!({"op": "set", "clip": id, "scale": 200})], "x", false).unwrap();
        s.edit(&[json!({"op": "animate", "clip": id, "property": "scale", "relative": true, "keys": [[0.0, 1.1], [0.2, 1.0]]})], "x", false).unwrap();
        let mc = s.doc.model.sequences.iter().find_map(|q| q.clip(m::ClipId(id as u64))).unwrap().clone();
        assert_eq!(mc.property_at("transform.scale_x", 0).unwrap(), m::Value::Scalar(2.2));
    }

    #[test]
    fn place_refuses_a_duration_the_file_cannot_give() {
        let mut s = session();
        let e = s.edit(&[json!({"op": "place", "media": "autumn road", "source_in": 9.0, "duration": 2.0})], "x", false).unwrap_err();
        assert!(e.contains("only 0.600s"), "{e}");
    }

    #[test]
    fn what_inspect_shows_set_and_keys_change() {
        let mut s = session();
        let id = s.doc.project.clips.iter().find(|c| c.kind == ClipKind::Video).unwrap().id;
        s.edit(&[json!({"op": "apply", "item": "gaussian blur", "clips": [id]})], "fx", false).unwrap();
        s.edit(&[json!({"op": "set", "clip": id, "crop_left": 10, "blend": "screen", "anchor_x": 5})], "props", false).unwrap();
        let fx = s.inspect(id).unwrap()["properties"].as_object().unwrap().keys().find(|k| k.starts_with("gaussian_blur.")).cloned().expect("effect settings listed");
        s.edit(&[json!({"op": "set", "clip": id, fx.as_str(): 7})], "fx", false).unwrap();
        let i = s.inspect(id).unwrap();
        assert_eq!(i["properties"]["crop_left"], json!(10.0));
        assert_eq!(i["properties"][fx.as_str()], json!(7.0));
        assert_eq!(i["blend"], json!("screen"));
        // Animate x alone, then read the keys back in pixels.
        s.edit(&[json!({"op": "animate", "clip": id, "property": "x", "keys": [[0.0, -100.0], [1.0, 100.0], [2.0, 0.0]], "ease": "linear"})], "pan", false).unwrap();
        let k = &s.inspect(id).unwrap()["keyframes"]["position"];
        assert_eq!(k.as_array().unwrap().len(), 3, "{k}");
        assert_eq!(k[1]["value"][0], json!(100.0));
        // Move one key, remove another, give the first a custom curve.
        s.edit(&[json!({"op": "keys", "clip": id, "property": "x", "move": [[1.0, 1.5]], "remove": [2.0], "curve": [{"t": 0.0, "out": [0.1, 0.9]}]})], "keys", false).unwrap();
        let k = s.inspect(id).unwrap()["keyframes"]["position"].clone();
        let ts: Vec<f64> = k.as_array().unwrap().iter().map(|k| k["t"].as_f64().unwrap()).collect();
        assert_eq!(ts, vec![0.0, 1.5]);
        assert_eq!(k[0]["curve"], json!("ease"));
        let e = s.edit(&[json!({"op": "set", "clip": id, "wobble.amount": 1})], "x", false).unwrap_err();
        assert!(e.contains("no property"), "{e}");
    }

    #[test]
    fn clips_by_ref_time_name_and_range() {
        let mut s = session();
        // Make a clip and animate it in the same call with "$1".
        let r = s.edit(&[json!({"op": "place", "media": "night market", "lane": "overlay", "at": 2.0, "duration": 2.0}), json!({"op": "animate", "clip": "$1", "property": "opacity", "keys": [[0.0, 0.0], [0.5, 100.0]]})], "x", false).unwrap();
        let id = added(&r);
        assert!(s.inspect(id).unwrap()["keyframes"]["opacity"].is_array());
        // The main-lane clip at 0.5 s, by time; then by name.
        let at = s.doc.project.clips.iter().find(|c| Some(c.lane) == s.doc.project.spine() && c.start == 0).unwrap().clone();
        s.edit(&[json!({"op": "set", "clip": "@0.5", "opacity": 80})], "x", false).unwrap();
        assert_eq!(s.clip(at.id).unwrap().transform.opacity, 80.0);
        s.edit(&[json!({"op": "set", "clip": at.name.clone(), "opacity": 70})], "x", false).unwrap();
        assert_eq!(s.clip(at.id).unwrap().transform.opacity, 70.0);
        // A range: every main-lane clip starting in the first 10 s.
        let r = s.edit(&[json!({"op": "apply", "item": "gaussian blur", "clips": {"lane": "main", "from": 0, "to": 10}})], "x", false).unwrap();
        assert!(r["changed"]["changed"].as_array().unwrap().len() >= 2, "{r}");
        let e = s.edit(&[json!({"op": "set", "clip": "$3", "opacity": 1})], "x", false).unwrap_err();
        assert!(e.contains("only 0 were made"), "{e}");
    }

    #[test]
    fn transitions_keep_timing_and_the_stack_is_editable() {
        let mut s = session();
        let spine = s.doc.project.spine().unwrap();
        let mut cs: Vec<view::Clip> = s.doc.project.clips.iter().filter(|c| c.lane == spine).cloned().collect();
        cs.sort_by_key(|c| c.start);
        let ends: Vec<view::Time> = cs.iter().map(|c| c.end()).collect();
        s.edit(&[json!({"op": "transition", "clip": cs[0].id, "item": "dissolve", "duration": 0.4})], "x", false).unwrap();
        let after: Vec<view::Time> = cs.iter().map(|c| s.clip(c.id).unwrap().end()).collect();
        assert_eq!(ends, after, "a transition does not move the cuts");
        assert_eq!(s.doc.project.transitions.iter().filter(|t| t.left == cs[0].id).count(), 1);
        s.edit(&[json!({"op": "transition", "clip": cs[0].id, "remove": true})], "x", false).unwrap();
        assert!(s.doc.project.transitions.iter().all(|t| t.left != cs[0].id));
        // Lanes.
        let r = s.edit(&[json!({"op": "lane", "action": "add", "kind": "overlay", "name": "Graphics"}), json!({"op": "lane", "action": "lock", "lane": "Graphics"})], "x", false).unwrap();
        assert!(r["ok"].as_bool().unwrap());
        assert!(s.doc.project.lanes.iter().any(|l| l.name == "Graphics" && l.locked));
        // Effect stack.
        s.edit(&[json!({"op": "apply", "item": "gaussian blur", "clips": [cs[1].id]}), json!({"op": "effect", "clip": cs[1].id, "effect": "gaussian", "enabled": false})], "x", false).unwrap();
        assert!(!s.clip(cs[1].id).unwrap().effects[0].enabled);
        s.edit(&[json!({"op": "effect", "clip": cs[1].id, "effect": 1, "remove": true})], "x", false).unwrap();
        assert!(s.clip(cs[1].id).unwrap().effects.is_empty());
        // A black card on the overlay, then markers moved and removed.
        s.edit(&[json!({"op": "card", "at": 1.0, "duration": 0.5}), json!({"op": "marker", "at": 2.0, "note": "drop"}), json!({"op": "marker", "move": [2.0, 3.0]})], "x", false).unwrap();
        assert!(s.doc.project.markers.iter().any(|m| m.time == 3 * SEC));
        s.edit(&[json!({"op": "marker", "remove": 3.0})], "x", false).unwrap();
        assert!(s.doc.project.markers.iter().all(|m| m.note != "drop"));
    }

    #[test]
    fn editors_moves_each_take_one_op() {
        let mut s = session();
        let spine = s.doc.project.spine().unwrap();
        let mut cs: Vec<view::Clip> = s.doc.project.clips.iter().filter(|c| c.lane == spine && c.kind == ClipKind::Video).cloned().collect();
        cs.sort_by_key(|c| c.start);
        let (a, b, c) = (cs[0].id, cs[1].id, cs[2].id);
        let end_a = s.clip(a).unwrap().end();
        s.edit(&[json!({"op": "roll", "clip": a, "by": -0.5})], "x", false).unwrap();
        assert_eq!(s.clip(a).unwrap().end(), end_a - SEC / 2, "the cut moved");
        s.edit(&[json!({"op": "apply", "item": "gaussian blur", "clips": [a]}), json!({"op": "copy_look", "from": a, "clips": [b, c], "parts": ["effects"]})], "x", false).unwrap();
        assert_eq!(s.clip(c).unwrap().effects.len(), 1);
        s.edit(&[json!({"op": "mask", "clip": b, "shape": "ellipse", "params": {"feather": 0.2}})], "x", false).unwrap();
        assert!(s.inspect(b).unwrap()["properties"]["mask1.feather"].is_number());
        let n = s.doc.project.clips.len();
        s.edit(&[json!({"op": "duplicate", "clips": [c]}), json!({"op": "freeze", "clip": "@1.0", "at": 1.0, "duration": 0.5})], "x", false).unwrap();
        assert!(s.doc.project.clips.len() >= n + 2);
    }

    #[test]
    fn a_failed_edit_leaves_nothing_behind() {
        let mut s = session();
        let before = (s.doc.project.clips.clone(), s.doc.project.lanes.clone(), s.doc.history.revision());
        let ops = [
            json!({"op": "add", "item": "star", "panel": "assets", "at": 1.0, "duration": 2.0, "x": 200}),
            json!({"op": "card", "at": 0.5, "duration": 1.0, "lane": "overlay"}),
            json!({"op": "card", "at": 0.5, "duration": 1.0, "lane": "overlay"}),
            json!({"op": "set", "clip": 999999, "opacity": 10}),
        ];
        assert!(s.edit(&ops, "x", false).is_err());
        assert_eq!(s.doc.project.clips, before.0, "clips restored");
        assert_eq!(s.doc.project.lanes, before.1, "lanes restored");
        assert_eq!(s.doc.history.revision(), before.2, "\"Nothing was changed\" means the revision did not move either");
        let again = s.edit(&ops, "x", false).unwrap_err();
        assert!(again.contains("Nothing was changed"), "{again}");
        assert_eq!(s.doc.history.revision(), before.2);
    }

    /// "card behind <a main-lane clip>" used to fail with LaneOrder: the
    /// bottom of the picture is the main lane, so the backdrop goes there
    /// and what it covers moves up unchanged.
    #[test]
    fn a_background_card_can_always_go_to_the_bottom() {
        let mut s = session();
        let p = s.doc.project.clone();
        let main = p.spine().unwrap();
        let under = p.clips.iter().filter(|c| c.lane == main).min_by_key(|c| c.start).unwrap().clone();
        let r = s.edit(&[json!({"op": "card", "colour": "#F6ECD9", "gradient": ["#FAF3E6", "#CFAD83"], "at": under.start as f64 / SEC as f64, "duration": 2.0, "behind": under.id})], "backdrop", false).unwrap();
        let card = s.clip(added(&r)).unwrap();
        assert_eq!(card.lane, main, "the card is the bottom of the picture");
        let moved = s.clip(under.id).unwrap();
        assert_ne!(moved.lane, main, "what it covered moved up");
        assert_eq!((moved.start, moved.dur), (under.start, under.dur), "at the same time, the same length");
        assert_eq!(s.doc.project.lane(moved.lane).unwrap().kind, LaneKind::Visual);
        // Behind a sound clip, and behind a clip on an overlay, too.
        let sound = p.clips.iter().find(|c| s.doc.project.lane(c.lane).is_some_and(|l| l.kind == LaneKind::Audio)).unwrap().clone();
        s.edit(&[json!({"op": "card", "colour": "black", "at": 40.0, "duration": 2.0, "behind": sound.id})], "backdrop", false).unwrap();
        let over = s.doc.project.clips.iter().find(|c| s.doc.project.lane(c.lane).is_some_and(|l| l.kind == LaneKind::Visual) && s.doc.project.spine() != Some(c.lane)).unwrap().clone();
        let r = s.edit(&[json!({"op": "card", "colour": "black", "at": over.start as f64 / SEC as f64, "duration": 1.0, "behind": over.id})], "backdrop", false).unwrap();
        let card = s.clip(added(&r)).unwrap();
        let order: Vec<u32> = s.doc.project.lanes.iter().map(|l| l.id).collect();
        let pos = |l: u32| order.iter().position(|x| *x == l).unwrap();
        assert!(pos(card.lane) > pos(over.lane), "drawn under the clip it is behind");
    }

    #[test]
    fn cards_backings_and_many_clips() {
        let mut s = session();
        // Two cards over the same time on the overlay: the second finds room.
        s.edit(&[json!({"op": "card", "at": 0.5, "duration": 1.0, "lane": "overlay"}), json!({"op": "card", "colour": "white", "at": 0.5, "duration": 1.0, "lane": "overlay", "width": 400, "height": 120})], "x", false).unwrap();
        // A title with a backing: the box sits on a lane drawn under it.
        let r = s.edit(&[json!({"op": "text", "text": "HELLO THERE", "at": 2.0, "duration": 2.0, "size": 80, "backing": {"colour": "black", "opacity": 60}})], "x", false).unwrap();
        let title = s.doc.project.clips.iter().find(|c| c.text == "HELLO THERE").unwrap().clone();
        let back = s.doc.project.clips.iter().find(|c| c.name == format!("backing for {}", title.id)).expect("backing made").clone();
        let idx = |l: u32| s.doc.project.lanes.iter().position(|x| x.id == l).unwrap();
        assert!(idx(back.lane) > idx(title.lane), "backing drawn under the title: {r}");
        assert_eq!((back.start, back.dur), (title.start, title.dur));
        // set over many clips at once.
        let ids: Vec<u32> = s.doc.project.clips.iter().filter(|c| c.kind == ClipKind::Video).take(3).map(|c| c.id).collect();
        s.edit(&[json!({"op": "set", "clips": ids, "saturation": -40})], "x", false).unwrap();
        for id in &ids {
            assert_eq!(s.clip(*id).unwrap().grade.saturation, -40.0);
        }
    }

    #[test]
    fn place_fills_a_hole_on_the_main_lane() {
        let mut s = session();
        let spine = s.doc.project.spine().unwrap();
        let mut cs: Vec<view::Clip> = s.doc.project.clips.iter().filter(|c| c.lane == spine).cloned().collect();
        cs.sort_by_key(|c| c.start);
        let hole = cs[1].clone();
        s.edit(&[json!({"op": "delete", "clips": [hole.id], "ripple": false})], "x", false).unwrap();
        let after: Vec<(u32, view::Time)> = cs[2..].iter().map(|c| (c.id, s.clip(c.id).unwrap().start)).collect();
        let r = s.edit(&[json!({"op": "place", "media": "autumn road", "at": secs(hole.start), "duration": secs(hole.dur)})], "x", false).unwrap();
        for (id, start) in after {
            assert_eq!(s.clip(id).unwrap().start, start, "nothing moved: {r}");
        }
        assert_eq!(s.clip(added(&r)).unwrap().start, hole.start);
    }

    #[test]
    fn made_sounds_are_what_they_say() {
        for (kind, dur, want) in [("riser", 2.5, "swell"), ("whoosh", 0.8, "swoosh"), ("boom", 2.0, "boom"), ("hit", 0.35, "hit")] {
            let (l, r) = super::super::synth::make(kind, dur, 7).unwrap();
            let mono: Vec<f32> = l.iter().zip(&r).map(|(a, b)| 0.5 * (a + b)).collect();
            assert_eq!(super::super::listen::sound_kind(&mono, 48_000), want, "{kind}");
            assert!(mono.iter().all(|v| v.abs() <= 1.0));
        }
        assert!(super::super::synth::make("kazoo", 1.0, 1).is_err());
    }

    #[test]
    fn sight_reads_light_colour_and_motion() {
        use studio_media::rgba::RgbaImage;
        let img = |f: &dyn Fn(u32, u32) -> [u8; 3]| {
            let (w, h) = (96u32, 54u32);
            RgbaImage { width: w, height: h, pixels: (0..w * h).flat_map(|i| { let c = f(i % w, i / w); [c[0], c[1], c[2], 255] }).collect() }
        };
        // A dark blue frame with a bright square left of centre.
        let a = img(&|x, y| if (20..36).contains(&x) && (18..36).contains(&y) { [250, 250, 250] } else { [10, 20, 90] });
        let s = super::super::sight::look_at(&a, None, &[]);
        assert!(s.brightness < 0.3, "{}", s.brightness);
        assert!(s.colours.iter().any(|c| c.contains("blue")), "{:?}", s.colours);
        assert!(s.subject[0] < 0.5, "subject on the left: {:?}", s.subject);
        // The same frame moved 4 px left: the camera pans right.
        let b = img(&|x, y| if (16..32).contains(&x) && (18..36).contains(&y) { [250, 250, 250] } else { [10, 20, 90] });
        let m = super::super::sight::look_at(&a, Some(&b), &[]).motion;
        assert!(m.contains("pans right") || m.contains("moves right"), "{m}");
        assert_eq!(super::super::sight::look_at(&a, Some(&a), &[]).motion, "still");
    }

    #[test]
    fn motion_curves_pulses_colours_and_ramps() {
        let mut s = session();
        let v = s.doc.project.clips.iter().find(|c| c.kind == ClipKind::Video && c.dur > 3 * SEC).unwrap().clone();
        let id = v.id;
        let mc = |s: &Session| s.doc.model.sequences.iter().find_map(|q| q.clip(m::ClipId(id as u64))).unwrap().clone();
        let at = |s: &Session, path: &str, t: f64| match mc(s).property_at(path, micros(t)).unwrap() { m::Value::Scalar(x) | m::Value::Angle(x) => x, other => panic!("{other:?}") };
        // Overshoot: on the way from 100% to 200% it passes 200%.
        s.edit(&[json!({"op": "animate", "clip": id, "property": "scale", "keys": [[0.0, 100.0], [1.0, 200.0]], "ease": "overshoot"})], "x", false).unwrap();
        let peak = (1..20).map(|i| at(&s, "transform.scale_x", i as f64 / 20.0)).fold(0.0, f64::max);
        assert!(peak > 2.0, "overshoot peaks past the target: {peak}");
        // Per-key curves: hold, then snap.
        s.edit(&[json!({"op": "animate", "clip": id, "property": "opacity", "keys": [{"t": 0.0, "v": 100, "ease": "hold"}, {"t": 1.0, "v": 20, "ease": "snap"}, {"t": 2.0, "v": 100}]})], "x", false).unwrap();
        assert!((at(&s, "transform.opacity", 0.9) - 1.0).abs() < 1e-6, "held");
        assert!(at(&s, "transform.opacity", 1.2) > 0.6, "snap gets most of the way at once");
        // Pulse on beats given in timeline time, relative to the clip's scale.
        let start = secs(v.start);
        s.edit(&[json!({"op": "keys", "clip": id, "property": "scale", "remove": "all"}), json!({"op": "set", "clip": id, "scale": 150}), json!({"op": "animate", "clip": id, "property": "scale", "relative": true, "pulse": {"at": [start + 0.5, start + 1.5], "peak": 1.2, "decay": 0.3}})], "x", false).unwrap();
        assert!((at(&s, "transform.scale_x", 0.5) - 1.8).abs() < 1e-6, "peak on the beat");
        assert!((at(&s, "transform.scale_x", 0.8) - 1.5).abs() < 1e-6, "back to rest");
        // Wiggle stays around the value.
        s.edit(&[json!({"op": "animate", "clip": id, "property": "rotation", "wiggle": {"amount": 3, "per_second": 8}})], "x", false).unwrap();
        let r: Vec<f64> = (0..30).map(|i| at(&s, "transform.rotation", i as f64 * 0.1)).collect();
        let base = v.transform.rotation as f64;
        assert!(r.iter().all(|x| (x - base).abs() <= 4.5) && r.iter().any(|x| (x - base).abs() > 0.3), "around {base}: {r:?}");
        // Several properties in one op, keys on the timeline.
        s.edit(&[json!({"op": "animate", "clip": id, "timeline": true, "properties": {"x": [[start, -100], [start + 1.0, 100]], "exposure": [[start, 0], [start + 1.0, 1]]}})], "x", false).unwrap();
        assert!((at(&s, "grade.exposure", 1.0) - 1.0).abs() < 1e-6);
        // A speed ramp inside the clip.
        let r = s.edit(&[json!({"op": "animate", "clip": id, "property": "speed", "keys": [[0.0, 1.0], [1.0, 0.4], [2.0, 1.0]]})], "x", false);
        assert!(r.is_ok(), "{r:?}");
        let e = s.edit(&[json!({"op": "animate", "clip": id, "property": "scale", "keys": [[0.0, 100.0]], "ease": "bouncy"})], "x", false).unwrap_err();
        assert!(e.contains("overshoot"), "lists the curves: {e}");
    }

    #[test]
    fn gsap_curves_paths_and_staggers_are_real_keys() {
        let mut s = session();
        let before: Vec<u32> = s.doc.project.clips.iter().map(|c| c.id).collect();
        let cards: Vec<Value> = (0..5).map(|i| json!({"op": "card", "colour": "#ff3366", "at": 0.0, "duration": 4.0, "width": 120, "height": 120, "x": -400 + i * 200, "y": 0})).collect();
        s.edit(&cards, "x", false).unwrap();
        let ids: Vec<u32> = s.doc.project.clips.iter().map(|c| c.id).filter(|id| !before.contains(id)).collect();
        assert_eq!(ids.len(), 5);
        let mc = |s: &Session, id: u32| s.doc.model.sequences.iter().find_map(|q| q.clip(m::ClipId(id as u64))).unwrap().clone();
        let scalar = |s: &Session, id: u32, path: &str, t: f64| match mc(s, id).property_at(path, micros(t)).unwrap() { m::Value::Scalar(x) | m::Value::Angle(x) => x, other => panic!("{other:?}") };
        let point = |s: &Session, id: u32, t: f64| match mc(s, id).property_at("transform.position", micros(t)).unwrap() { m::Value::Point(p) => p, other => panic!("{other:?}") };

        // An elastic scale rings around its target more than once.
        let a = ids[0];
        s.edit(&[json!({"op": "animate", "clip": a, "property": "scale", "keys": [[0.0, 0.0], [1.0, 100.0]], "ease": "elastic.out(1, 0.3)"})], "x", false).unwrap();
        let v: Vec<f64> = (1..50).map(|i| scalar(&s, a, "transform.scale_x", i as f64 / 50.0)).collect();
        assert!(v.windows(2).filter(|w| (w[0] - 1.0).signum() != (w[1] - 1.0).signum()).count() >= 3, "rings: {v:?}");
        let k = &mc(&s, a).keyframes["transform.scale_x"].keys[0];
        assert_eq!(k.interp, m::Interp::Curve);
        // Read back by name, and changed with keys.
        let shown = super::super::props::describe(&mc(&s, a));
        assert_eq!(shown["keyframes"]["scale_x"][0]["curve"], "elastic.out(1, 0.3)", "{shown}");
        s.edit(&[json!({"op": "keys", "clip": a, "property": "scale", "curve": [{"t": 0, "curve": "bounce"}]})], "x", false).unwrap();
        let v: Vec<f64> = (1..50).map(|i| scalar(&s, a, "transform.scale_x", i as f64 / 50.0)).collect();
        assert!(v.iter().all(|x| *x <= 1.0 + 1e-9), "a bounce never passes the floor");
        for name in ["bounce_in_out", "back(2.5)", "power3.inOut", "expo_in", "steps(5)", "spring", "circ_out", "sine.in"] {
            assert!(curve(name).is_some_and(|c| c.3.is_some()), "{name}");
        }

        // A position move bowed into an arc.
        let b = ids[1];
        s.edit(&[json!({"op": "animate", "clip": b, "property": "position", "keys": [[0.0, -400, 0], [1.0, 400, 0]], "arc": 0.25, "ease": "linear"})], "x", false).unwrap();
        let mid = point(&s, b, 0.5);
        assert!((mid.x).abs() < 2.0 && (mid.y + 200.0).abs() < 2.0, "bows 200 px up at the middle: {mid:?}");
        // Smooth through several points: the path passes every key.
        s.edit(&[json!({"op": "animate", "clip": b, "property": "position", "keys": [[0.0, -400, 0], [1.0, 0, -300], [2.0, 400, 0]], "path": "smooth"})], "x", false).unwrap();
        let p1 = point(&s, b, 1.0);
        assert!((p1.x).abs() < 1e-6 && (p1.y + 300.0).abs() < 1e-6);
        let q = point(&s, b, 0.5);
        let straight = -300.0 * (q.x + 400.0) / 400.0;
        assert!(q.y < straight - 15.0, "rounds the corner, above the straight line ({straight}): {q:?}");
        let e = s.edit(&[json!({"op": "animate", "clip": b, "property": "opacity", "keys": [[0, 0], [1, 100]], "arc": 0.2})], "x", false).unwrap_err();
        assert!(e.contains("position"), "{e}");

        // Staggered from the centre: the middle card first, the edges last.
        s.edit(&[json!({"op": "stagger", "clips": ids, "each": 0.2, "from": "centre", "animate": {"property": "opacity", "keys": [[0.0, 0], [0.5, 100]], "ease": "power2.out"}})], "x", false).unwrap();
        let first_full = |s: &Session, id: u32| (0..=40).map(|i| i as f64 * 0.05).find(|t| scalar(s, id, "transform.opacity", *t) > 0.999).unwrap();
        let times: Vec<f64> = ids.iter().map(|id| first_full(&s, *id)).collect();
        assert!((times[2] - 0.5).abs() < 0.03 && (times[1] - 0.7).abs() < 0.03 && (times[0] - 0.9).abs() < 0.03, "{times:?}");
        assert_eq!(times[1], times[3]);
        // By where they sit on the picture, left to right.
        s.edit(&[json!({"op": "stagger", "clips": [ids[4], ids[0], ids[2]], "amount": 0.4, "by": "x", "animate": {"property": "rotation", "keys": [[0.0, -90], [0.5, 0]], "ease": "back"}})], "x", false).unwrap();
        assert!(scalar(&s, ids[0], "transform.rotation", 0.5).abs() < 1e-6);
        assert!(scalar(&s, ids[4], "transform.rotation", 0.5) < -1.0, "the rightmost one is still turning");
    }

    #[test]
    fn motions_stack_and_clips_merge() {
        let mut s = session();
        let spine = s.doc.project.spine().unwrap();
        let mut cs: Vec<view::Clip> = s.doc.project.clips.iter().filter(|c| c.lane == spine && c.kind == ClipKind::Video).cloned().collect();
        cs.sort_by_key(|c| c.start);
        let id = cs[0].id;
        let motion = crate::catalogue::motions()[0].name;
        let r = s.edit(&[json!({"op": "motion", "clip": id, "entry": {"item": motion, "duration": 0.4}, "exit": motion, "loop": {"item": motion}})], "x", false).unwrap();
        let mc = s.doc.model.sequences.iter().find_map(|q| q.clip(m::ClipId(id as u64))).unwrap().clone();
        assert!(mc.motion.entry.is_some() && mc.motion.exit.is_some() && mc.motion.looping.is_some(), "{r}");
        assert_eq!(mc.motion.entry.unwrap().duration, micros(0.4));
        // A look whose strength rises over the clip.
        s.edit(&[json!({"op": "apply", "item": crate::catalogue::looks()[0].name, "clips": [id]}), json!({"op": "animate", "clip": id, "property": "look_strength", "keys": [[0, 0], [1, 100]]})], "x", false).unwrap();
        let k = &s.inspect(id).unwrap()["keyframes"]["look_strength"];
        assert_eq!(k[1]["value"], json!(100.0), "{k}");
        // Two overlay clips merged into one, animated as a unit, opened again.
        let r = s.edit(&[json!({"op": "card", "at": 1.0, "duration": 2.0, "lane": "overlay", "width": 300, "height": 300}), json!({"op": "text", "text": "IN THE BOX", "at": 1.0, "duration": 2.0})], "x", false).unwrap();
        let made: Vec<u64> = r["changed"]["added"].as_array().unwrap().iter().filter_map(|c| c["id"].as_u64()).collect();
        let r = s.edit(&[json!({"op": "merge", "clips": made, "name": "Badge"})], "x", false).unwrap();
        let merged = r["changed"]["added"][0]["id"].as_u64().expect("one merged clip") as u32;
        assert!(made.iter().all(|m| s.doc.project.clip(*m as u32).is_none()), "the parts are inside it now");
        s.edit(&[json!({"op": "animate", "clip": merged, "property": "rotation", "keys": [[0, -10], [1, 10]]})], "x", false).unwrap();
        s.edit(&[json!({"op": "unmerge", "clip": merged})], "x", false).unwrap();
        assert!(s.doc.project.clip(merged).is_none());
    }

    #[test]
    fn moves_and_designs_are_made_and_reused() {
        let mut s = session();
        let tag = format!("test-{}", std::process::id());
        let spine = s.doc.project.spine().unwrap();
        let mut cs: Vec<view::Clip> = s.doc.project.clips.iter().filter(|c| c.lane == spine && c.kind == ClipKind::Video).cloned().collect();
        cs.sort_by_key(|c| c.start);
        let (a, b) = (cs[0].clone(), cs[1].clone());
        // A punch-in on a, saved, put on b (another length, another scale).
        s.edit(&[json!({"op": "set", "clip": a.id, "scale": 100}), json!({"op": "animate", "clip": a.id, "property": "scale", "keys": [[0, 100], [secs(a.dur), 120]], "ease": "linear"})], "x", false).unwrap();
        s.run("save_move", &json!({"clip": a.id, "name": format!("{tag}-push"), "description": "slow push"})).map_err(|e| e).unwrap();
        s.edit(&[json!({"op": "set", "clip": b.id, "scale": 200})], "x", false).unwrap();
        s.edit(&[json!({"op": "use_move", "move": format!("{tag}-push"), "clip": b.id})], "x", false).unwrap();
        let mc = s.doc.model.sequences.iter().find_map(|q| q.clip(m::ClipId(b.id as u64))).unwrap().clone();
        let end = mc.property_at("transform.scale_x", b.dur).unwrap();
        assert_eq!(end, m::Value::Scalar(2.4), "120% of its own 200%, at its own end");
        // A design: a box and a title, placed later with new words.
        let r = s.edit(&[json!({"op": "card", "colour": "#ff2d9b", "at": 1.0, "duration": 2.0, "lane": "overlay", "width": 400, "height": 120}), json!({"op": "text", "text": "HELLO", "at": 1.0, "duration": 2.0, "reveal": "pop"})], "x", false).unwrap();
        let ids: Vec<u64> = r["changed"]["added"].as_array().unwrap().iter().filter_map(|c| c["id"].as_u64()).collect();
        s.run("save_design", &json!({"clips": ids, "name": format!("{tag}-badge")})).map_err(|e| e).unwrap();
        s.edit(&[json!({"op": "use_design", "design": format!("{tag}-badge"), "at": 10.0, "y": 300, "text": {"HELLO": "GOODBYE"}})], "x", false).unwrap();
        let t = s.doc.project.clips.iter().find(|c| c.text == "GOODBYE").expect("text swapped").clone();
        assert_eq!(t.start, 10 * SEC);
        assert_eq!(t.transform.y, 300.0);
        let card = s.doc.project.clips.iter().find(|c| c.start == 10 * SEC && c.text.is_empty() && c.kind != ClipKind::Video).expect("box placed").clone();
        let idx = |l: u32| s.doc.project.lanes.iter().position(|x| x.id == l).unwrap();
        assert!(idx(card.lane) > idx(t.lane), "the box is still under the title");
        let made = s.run("made", &json!({})).map_err(|e| e).unwrap();
        let _ = made;
        let _ = std::fs::remove_file(studio_paths::studio().join("Made/moves").join(format!("{tag}-push.json")));
        let _ = std::fs::remove_file(studio_paths::studio().join("Made/designs").join(format!("{tag}-badge.json")));
    }

    #[test]
    fn colours_read_hex_and_names() {
        assert_eq!(colour("#ff0080"), Some(eframe::egui::Color32::from_rgb(255, 0, 128)));
        assert_eq!(colour("white"), Some(eframe::egui::Color32::WHITE));
        assert_eq!(colour("#12"), None);
    }
}
