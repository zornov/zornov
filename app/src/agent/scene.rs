// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What an editor does by craft, done by Studio: a `scene` from what it
//! should say and show, `music` for a mood fitted to the video, and the
//! fixes `check` knows how to make. The model says what the video is; the
//! app decides how it is laid out, sized, moved, timed and heard, the same
//! way every time.

use super::{Session, f, inventory, micros, related, secs, shelf, sounds};
use crate::editor::xform;
use crate::services::{self, library::Panel};
use crate::view::{self, ClipKind, SEC};
use serde_json::{Value, json};

/// A background colour per mood (when the library has no place for it).
fn mood_colour(mood: &str) -> &'static str {
    match mood {
        "sad" | "lonely" | "melancholy" | "heartbroken" => "#1b2a44",
        "happy" | "joyful" | "fun" | "funny" | "playful" => "#ff9f43",
        "romantic" | "love" => "#5a1f3d",
        "calm" | "peaceful" | "chill" => "#2a6f73",
        "dramatic" | "dark" | "tense" | "epic" => "#111318",
        "energetic" | "hype" | "exciting" => "#6c2bd9",
        _ => "#1e2235",
    }
}

/// Where up to three pictures go: centre x, y and the box each may fill,
/// as fractions of the canvas. The caption sits above them.
fn slots(n: usize, vertical: bool, captions: bool) -> &'static [(f32, f32, f32, f32)] {
    match (vertical, captions, n) {
        (true, false, 0 | 1) => &[(0.0, 0.08, 0.9, 0.56)],
        (true, false, 2) => &[(-0.08, 0.02, 0.8, 0.42), (0.22, 0.27, 0.5, 0.3)],
        (true, false, _) => &[(0.0, 0.0, 0.76, 0.38), (-0.24, 0.28, 0.46, 0.26), (0.24, 0.28, 0.46, 0.26)],
        // With spoken captions: the pictures stay between the title band
        // and the caption band, clear of the app buttons below it.
        (true, true, 0 | 1) => &[(0.0, -0.05, 0.86, 0.34)],
        (true, true, 2) => &[(-0.1, -0.09, 0.66, 0.27), (0.24, 0.06, 0.42, 0.18)],
        (true, true, _) => &[(0.0, -0.1, 0.62, 0.24), (-0.26, 0.05, 0.42, 0.17), (0.26, 0.05, 0.42, 0.17)],
        (false, false, 0 | 1) => &[(0.0, 0.1, 0.6, 0.72)],
        (false, false, 2) => &[(-0.2, 0.1, 0.46, 0.7), (0.24, 0.16, 0.36, 0.56)],
        (false, false, _) => &[(-0.31, 0.12, 0.33, 0.58), (0.0, 0.08, 0.37, 0.66), (0.31, 0.12, 0.33, 0.58)],
        (false, true, 0 | 1) => &[(0.0, 0.0, 0.5, 0.56)],
        (false, true, 2) => &[(-0.2, 0.0, 0.42, 0.56), (0.23, 0.04, 0.32, 0.44)],
        (false, true, _) => &[(-0.31, 0.02, 0.3, 0.46), (0.0, -0.02, 0.34, 0.52), (0.31, 0.02, 0.3, 0.46)],
    }
}

/// When a word of `term` is first spoken in `words` (timeline seconds).
fn spoken_at(words: &[super::voice::Said], term: &str) -> Option<f64> {
    let norm = |s: &str| s.to_lowercase().trim_matches(|c: char| !c.is_alphanumeric()).to_owned();
    let stem = |s: &str| { let s = norm(s); s.strip_suffix("es").or_else(|| s.strip_suffix('s')).map(str::to_owned).unwrap_or(s) };
    let keys: Vec<String> = term.split_whitespace().map(stem).filter(|k| k.len() > 2).collect();
    words.iter().find(|w| { let s = stem(&w.text); keys.iter().any(|k| s == *k || (s.len() > 3 && (s.starts_with(k.as_str()) || k.starts_with(s.as_str())))) }).map(|w| w.start)
}

/// Whether a clip's footage is shot on a green screen: its first frame is
/// mostly pure green.
#[cfg(feature = "render")]
fn green_screen(mi: &view::MediaItem) -> bool {
    if mi.kind != view::MediaKind::Video {
        return false;
    }
    let Ok(frames) = super::render::media_frames(mi, &[0.2], 96) else { return false };
    let Some(img) = frames.first() else { return false };
    let px: Vec<&[u8]> = img.pixels.chunks_exact(4).collect();
    let green = px.iter().filter(|q| q[1] > 120 && (q[1] as i32) > q[0] as i32 + 50 && (q[1] as i32) > q[2] as i32 + 50).count();
    green * 4 > px.len()
}

#[cfg(not(feature = "render"))]
fn green_screen(_: &view::MediaItem) -> bool {
    false
}

/// A caption broken into lines that fit the frame.
fn lines(text: &str, per_line: usize) -> String {
    let mut out = String::new();
    let mut line = 0;
    for w in text.split_whitespace() {
        if line > 0 && line + 1 + w.chars().count() > per_line {
            out.push('\n');
            line = 0;
        } else if line > 0 {
            out.push(' ');
            line += 1;
        }
        out.push_str(w);
        line += w.chars().count();
    }
    out
}

impl Session {
    /// Runs one op and returns the clips it made.
    fn made_by(&mut self, op: Value) -> Result<Vec<u32>, String> {
        let before: std::collections::BTreeSet<u32> = self.doc.project.clips.iter().map(|c| c.id).collect();
        self.op(&op)?;
        let mut new: Vec<u32> = self.doc.project.clips.iter().map(|c| c.id).filter(|id| !before.contains(id)).collect();
        new.sort();
        Ok(new)
    }

    /// The best library picture for `word`: an exact name first, then the
    /// first match; backgrounds only when `backdrop`.
    fn picture_for(&self, word: &str, backdrop: bool) -> Option<String> {
        let hits = services::library::search(&self.doc, Panel::Assets, 0, word);
        let items: Vec<services::library::Item> = hits.iter().take(40).map(|&i| services::library::item(&self.doc, Panel::Assets, i as usize)).collect();
        let is_back = |it: &services::library::Item| it.path.as_ref().is_some_and(|p| p.to_string_lossy().to_lowercase().contains("background"));
        let fit: Vec<&services::library::Item> = items.iter().filter(|it| is_back(it) == backdrop).collect();
        fit.iter().find(|it| it.name.eq_ignore_ascii_case(word)).or(fit.first()).map(|it| it.name.clone())
    }

    /// `scene`: one beat of the video. `say` is the caption, `show` the
    /// things to see (found in the library, laid out to fill the frame,
    /// brought in one after another, kept moving), `where` a place for
    /// the background, `mood` its colour when the library has no place.
    /// It follows the scenes before it.
    pub(super) fn scene(&mut self, op: &Value) -> Result<String, String> {
        let say = op.get("say").and_then(Value::as_str).unwrap_or("").trim().to_owned();
        let show: Vec<String> = match op.get("show") {
            Some(Value::String(s)) => s.split(',').map(|w| w.trim().to_owned()).filter(|w| !w.is_empty()).collect(),
            Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_owned).collect(),
            _ => Vec::new(),
        };
        if say.is_empty() && show.is_empty() {
            return Err("scene needs \"say\" (what it says: spoken, with captions) and/or \"show\" (things to see)".into());
        }
        let mood = op.get("mood").and_then(Value::as_str).unwrap_or("").to_lowercase();
        // One sentence on screen at a time: a long caption becomes several,
        // each timed to how long it takes to read.
        let mut beats: Vec<String> = Vec::new();
        for part in say.split_inclusive(['.', '!', '?']) {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            match beats.last_mut() {
                Some(last) if last.split_whitespace().count() + part.split_whitespace().count() <= 8 => {
                    last.push(' ');
                    last.push_str(part);
                }
                _ => beats.push(part.to_owned()),
            }
        }
        let read = |t: &str| (1.0 + t.split_whitespace().count() as f64 * 0.36).clamp(1.8, 6.0);
        let p = &self.doc.project;
        let (w, h) = (p.width as f32, p.height as f32);
        let vertical = h > w;
        // After everything on the main lane.
        let at = f(op, "at").unwrap_or_else(|| secs(p.clips.iter().filter(|c| p.spine() == Some(c.lane)).map(|c| c.end()).max().unwrap_or(0)));
        let mut said = Vec::new();
        let mut missing = Vec::new();
        // Spoken unless asked not to: the voice sets the scene's length,
        // when each picture arrives, and the captions.
        let speak = !say.is_empty() && op.get("voice") != Some(&Value::Bool(false));
        let mut words: Vec<super::voice::Said> = Vec::new();
        if speak {
            let mut n = json!({"say": say, "at": at + 0.25});
            if let Some(v) = op.get("speaker") {
                n["speaker"] = v.clone();
            }
            if let Some(v) = op.get("voice").filter(|v| v.is_string()) {
                n["voice"] = v.clone();
            }
            match self.narrate(&n) {
                Ok((note, _, ws)) => {
                    said.push(note);
                    words = ws;
                }
                Err(e) => said.push(format!("not spoken ({e}): shown as a caption instead")),
            }
        }
        let spoken = !words.is_empty();
        let natural: f64 = if spoken {
            words.last().map_or(3.0, |l| l.end - at + 0.7)
        } else if beats.is_empty() {
            3.0
        } else {
            beats.iter().map(|b| read(b)).sum()
        };
        let dur = f(op, "duration").map_or(natural.max(2.5), |d| d.max(if spoken { natural } else { 0.0 }));

        // The background: the place, else a colour for the mood.
        let place = op.get("where").and_then(Value::as_str).map(str::to_owned);
        let backdrop = place.as_deref().and_then(|w| self.picture_for(w, true));
        match backdrop {
            Some(name) => {
                let ids = self.made_by(json!({"op": "add", "item": name, "panel": "assets", "at": at, "lane": "main", "duration": dur}))?;
                if let Some(&id) = ids.first() {
                    self.op(&json!({"op": "set", "clip": id, "fit": "fill"}))?;
                    // A slow push in keeps a still place alive.
                    let _ = self.op(&json!({"op": "animate", "clip": id, "property": "scale", "relative": true, "keys": [[0, 100], [dur, 108]], "ease": "linear"}));
                }
                said.push(format!("background {name}"));
            }
            None => {
                if let Some(w) = &place {
                    missing.push(w.clone());
                }
                self.made_by(json!({"op": "card", "colour": mood_colour(&mood), "at": at, "duration": dur, "lane": "main"}))?;
            }
        }

        // The things to see, each where the layout puts it.
        let found: Vec<(String, String)> = show.iter().filter_map(|s| match self.picture_for(s, false) {
            Some(n) => Some((s.clone(), n)),
            None => {
                missing.push(s.clone());
                None
            }
        }).take(3).collect();
        let layout = slots(found.len(), vertical, spoken);
        for (i, ((term, name), &(sx, sy, tw, th))) in found.iter().zip(layout.iter()).enumerate() {
            // The hero is there almost at once; the others arrive as the
            // voice names them (or one after another without a voice).
            let delay = if i == 0 {
                if at < 0.05 { 0.0 } else { 0.15 }
            } else {
                match spoken_at(&words, term) {
                    Some(t) => (t - at - 0.1).clamp(0.25 * i as f64, (dur - 1.2).max(0.25 * i as f64)),
                    None => 0.25 * i as f64,
                }
            };
            let ids = self.made_by(json!({"op": "add", "item": name, "panel": "assets", "at": at + delay, "duration": dur - delay}))?;
            let Some(&id) = ids.first() else { continue };
            // Sized to its box, whatever the picture's own size.
            if let Some(g) = xform::geo(&self.doc.project, &self.doc.model, id, micros(at + delay) + SEC / 10) {
                let q = g.quad();
                let (bw, bh) = (q.iter().map(|v| v.x).fold(f32::MIN, f32::max) - q.iter().map(|v| v.x).fold(f32::MAX, f32::min), q.iter().map(|v| v.y).fold(f32::MIN, f32::max) - q.iter().map(|v| v.y).fold(f32::MAX, f32::min));
                let k = ((tw * w) / bw.max(1.0)).min((th * h) / bh.max(1.0));
                let scale = (g.xf.sx * k * 100.0) as f64;
                self.op(&json!({"op": "set", "clip": id, "scale": scale, "x": (sx * w) as f64, "y": (sy * h) as f64}))?;
            }
            // Green screen footage: the green keyed out.
            if let Some(mi) = self.doc.project.clip(id).and_then(|c| c.media).and_then(|m| self.doc.project.media(m)).cloned() {
                if green_screen(&mi) {
                    let _ = self.op(&json!({"op": "apply", "item": "Chroma key", "clips": [id]}));
                }
            }
            // The first is already there on the scene's first frame.
            let looping = if i == 0 { "Float" } else { "Breathe" };
            // The video's first frame shows a picture (it is the thumbnail).
            let entry = if i == 0 && at < 0.05 { Value::Null } else if i == 0 { json!("Zoom in") } else { json!("Rise") };
            let _ = self.op(&json!({"op": "motion", "clip": id, "entry": entry, "loop": looping}));
            said.push(name.clone());
        }

        // Spoken: the words on screen as they are said, a few at a time,
        // in the caption band; a short title above if one is given.
        if spoken {
            let groups = super::voice::caption_groups(&words);
            let (per, size, y) = if vertical { (16usize, 46.0, 0.21 * h as f64) } else { (30usize, 54.0, 0.40 * h as f64) };
            for (gi, g) in groups.iter().enumerate() {
                let start = g.first().map_or(at, |w| w.start);
                let next = groups.get(gi + 1).and_then(|n| n.first()).map(|w| w.start);
                let end = g.last().map_or(start + 1.0, |w| w.end + 0.3).min(next.unwrap_or(f64::MAX)).min(at + dur);
                let text: String = g.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");
                if end - start < 0.05 {
                    continue;
                }
                self.made_by(json!({"op": "text", "text": lines(&text, per), "at": start, "duration": end - start, "size": size, "y": y, "style": "Subtitle Bar", "reveal": {"kind": "fade", "unit": "line", "duration": 0.1}}))?;
            }
            if let Some(title) = op.get("title").and_then(Value::as_str).filter(|t| !t.trim().is_empty()) {
                let per = if vertical { 14 } else { 24 };
                let size = (0.86 * w * 1080.0 / (per as f32 * 0.6 * h)).min(110.0).round() as f64;
                let y = if vertical { -0.31 * h } else { -0.36 * h };
                self.made_by(json!({"op": "text", "text": lines(title, per), "at": at + 0.2, "duration": dur - 0.2, "size": size, "y": y, "style": "Ink Shadow", "reveal": {"kind": "rise", "unit": "word", "duration": 0.45}}))?;
            }
        } else if !say.is_empty() {
        // Unspoken: the caption, in the clear band above the pictures.
            // Sizes are in 1/1080 of the canvas height: as many characters a
            // line as fit 86 % of the width.
            let per = if vertical { 16 } else { 28 };
            let size = (0.86 * w * 1080.0 / (per as f32 * 0.58 * h)).min(96.0).round() as f64;
            let y = if vertical { -0.31 * h } else { -0.36 * h };
            let size = if found.is_empty() { size * 1.2 } else { size };
            let y = if found.is_empty() { 0.0 } else { y };
            // Each beat's share of the scene, in reading time.
            let total: f64 = beats.iter().map(|b| read(b)).sum::<f64>().max(0.1);
            let mut t = at;
            for b in &beats {
                let d = dur * read(b) / total;
                self.made_by(json!({"op": "text", "text": lines(b, per), "at": t, "duration": d, "size": size, "y": y, "style": "Ink Shadow", "reveal": {"kind": "rise", "unit": "word", "duration": 0.45}}))?;
                t += d;
            }
        }

        // Sound on the cut and on the first picture's arrival.
        if at > 0.05 {
            let _ = self.op(&json!({"op": "sfx", "kind": "whoosh", "ends_at": at + 0.15, "duration": 0.6, "volume": -12}));
        }
        if !found.is_empty() {
            let _ = self.op(&json!({"op": "sfx", "kind": "hit", "at": at + 0.05, "volume": -14}));
        }
        let mut out = format!("scene {:.2}–{:.2}s: {}", at, at + dur, if said.is_empty() { "caption on colour".to_owned() } else { said.join(", ") });
        if !missing.is_empty() {
            out.push_str(&format!(" (the library has nothing for: {}; catalogue with a list of other words finds what it does have)", missing.join(", ")));
        }
        Ok(out)
    }

    /// `music`: original music composed here in a genre, key and tempo, fitted
    /// to a length and to cue points; or a library track for a mood (or a
    /// named one) under the whole video, fitted to its length with a fade at
    /// the end. A mood the library has no track for is composed instead.
    pub(super) fn music(&mut self, op: &Value) -> Result<String, String> {
        let no_pictures = !self.doc.project.clips.iter().any(|c| c.kind != ClipKind::Audio);
        if self.music_wants_composing(op) || (no_pictures && (op.get("duration").is_some() || op.get("length").is_some())) {
            return self.compose_music(op, None);
        }
        match self.music_from_library(op) {
            Err(e) if op.get("item").is_none() && !e.starts_with("music goes under pictures") => self.compose_music(op, Some(&e)),
            r => r,
        }
    }

    /// `music` from the library: a track for a mood, or a named one.
    fn music_from_library(&mut self, op: &Value) -> Result<String, String> {
        let p = &self.doc.project;
        let end = p.clips.iter().filter(|c| c.kind != ClipKind::Audio).map(|c| c.end()).max().unwrap_or(0);
        if end <= 0 {
            return Err("music goes under pictures: make the scenes first".into());
        }
        let name = match op.get("item").and_then(Value::as_str) {
            Some(n) => n.to_owned(),
            None => {
                let mood = op.get("mood").and_then(Value::as_str).unwrap_or("calm");
                // A track of that mood the project does not play yet, long enough to sit under a video.
                let fresh = shelf::content().and_then(|c| {
                    let ask = inventory::Ask { kind: Some("music".into()), min_secs: Some(30.0), ..inventory::Ask::from_words(mood) };
                    let i = inventory::pick(&c.entries, &self.library_facts(), &ask, &|w| related(w), &|w| sounds::wanted(w), false)?;
                    Some(c.entries[i].name.clone())
                });
                // Tracks, not effects: anything long enough to sit under a video.
                let v = if fresh.is_some() { json!({"items": []}) } else { self.catalogue(Panel::Audio, &format!("{mood} music"), 30) };
                let pick = v["items"].as_array().into_iter().flatten().find(|i| i["dur"].as_f64().is_some_and(|d| d >= 30.0)).or_else(|| {
                    None
                });
                let pick = match pick {
                    _ if fresh.is_some() => fresh.unwrap_or_default(),
                    Some(i) => i["name"].as_str().unwrap_or("").to_owned(),
                    None => {
                        let v = self.catalogue(Panel::Audio, mood, 30);
                        v["items"].as_array().into_iter().flatten().find(|i| i["dur"].as_f64().is_some_and(|d| d >= 30.0)).and_then(|i| i["name"].as_str().map(str::to_owned)).ok_or_else(|| format!("no track for \"{mood}\"; catalogue panel audio lists them"))?
                    }
                };
                pick
            }
        };
        // Any music already under it gives way.
        let old: Vec<u32> = self.doc.project.clips.iter().filter(|c| c.kind == ClipKind::Audio && c.media.and_then(|m| self.doc.project.media(m)).is_some_and(|mi| mi.dur >= 20 * SEC)).map(|c| c.id).collect();
        if !old.is_empty() {
            self.op(&json!({"op": "delete", "clips": old}))?;
        }
        let volume = f(op, "volume").unwrap_or(-10.0);
        let ids = self.made_by(json!({"op": "add", "item": name, "panel": "audio", "at": 0, "lane": "music", "duration": secs(end), "volume": volume}))?;
        let last = ids.iter().copied().max_by_key(|id| self.doc.project.clip(*id).map_or(0, |c| c.end()));
        if let Some(id) = last {
            let _ = self.op(&json!({"op": "set", "clip": id, "fade_out": 1.5}));
        }
        // Under a voice: a bed that steps back while someone speaks.
        let narrated = !self.spoken_words().is_empty();
        let mut volume = volume;
        if narrated {
            if f(op, "volume").is_none() {
                volume = -19.0;
                for id in &ids {
                    let _ = self.op(&json!({"op": "set", "clip": id, "volume": volume}));
                }
            }
            let n = self.duck_music(8.0)?;
            return Ok(format!("music \"{name}\" under 0–{:.2}s at {volume:.1} dB, ducking under the narration on {n} clip(s), fading out", secs(end)));
        }
        // Without a volume asked for: as loud as video usually is (about
        // -16 LUFS for the whole mix), never above 0 dB.
        if f(op, "volume").is_none() {
            if let Some(l) = self.check_loudness() {
                volume = (volume + (-16.0 - l)).clamp(-30.0, 0.0);
                for id in &ids {
                    let _ = self.op(&json!({"op": "set", "clip": id, "volume": volume}));
                }
            }
        }
        Ok(format!("music \"{name}\" under 0–{:.2}s at {volume:.1} dB, fading out", secs(end)))
    }

    /// `check` with `fix`: what Studio can mend by itself (titles over
    /// pictures or off the frame or under the app buttons, titles running
    /// past the picture, a mix too quiet or too loud). Returns what it did.
    pub(super) fn fix(&mut self, problems: &[Value]) -> Vec<String> {
        let mut done = Vec::new();
        let (w, h) = (self.doc.project.width as f64, self.doc.project.height as f64);
        for pr in problems {
            let kind = pr["kind"].as_str().unwrap_or("");
            let clip = pr["clip"].as_u64().map(|c| c as u32);
            let r: Result<String, String> = match (kind, clip) {
                ("title_covers_picture", Some(id)) => {
                    // Up into the band above, or the bottom band when the
                    // picture fills the top.
                    let c = self.doc.project.clip(id).cloned();
                    match c {
                        Some(c) => {
                            let top = if h > w { -0.32 * h } else { -0.37 * h };
                            if (c.transform.y as f64 - top).abs() < 1.0 {
                                self.op(&json!({"op": "set", "clip": id, "y": if h > w { 0.3 * h } else { 0.38 * h }}))
                            } else {
                                self.op(&json!({"op": "set", "clip": id, "y": top}))
                            }
                        }
                        None => Err("gone".into()),
                    }
                }
                ("title_cut_off", Some(id)) => match self.doc.project.clip(id).map(|c| c.extra.text_size) {
                    Some(s) => self.op(&json!({"op": "set", "clip": id, "text_size": (s * 0.8).max(24.0)})),
                    None => Err("gone".into()),
                },
                ("title_under_app_ui", Some(id)) => match self.doc.project.clip(id).map(|c| (c.transform.x, c.transform.y)) {
                    Some((x, y)) => self.op(&json!({"op": "set", "clip": id, "x": (x as f64).clamp(-0.3 * w, 0.18 * w), "y": (y as f64).clamp(-0.4 * h, 0.27 * h)})),
                    None => Err("gone".into()),
                },
                ("picture_ends_early", _) => {
                    let p = &self.doc.project;
                    let pic_end = p.clips.iter().filter(|c| c.kind == ClipKind::Video).map(|c| c.end()).max().unwrap_or(0);
                    let long: Vec<(u32, view::Time)> = p.clips.iter().filter(|c| c.end() > pic_end && pic_end > c.start).map(|c| (c.id, c.end() - pic_end)).collect();
                    let mut r = Ok(String::new());
                    for (id, over) in long {
                        r = self.op(&json!({"op": "trim", "clip": id, "end": -secs(over)}));
                    }
                    r
                }
                ("audio_quiet" | "audio_loud", _) => {
                    let lufs = self.check_loudness();
                    match lufs {
                        Some(l) => {
                            let gain = (-14.0 - l).clamp(-12.0, 12.0);
                            let ids: Vec<(u32, f32)> = self.doc.project.clips.iter().filter(|c| c.kind == ClipKind::Audio).map(|c| (c.id, c.audio.volume)).collect();
                            let mut r = Ok(String::new());
                            for (id, v) in ids {
                                r = self.op(&json!({"op": "set", "clip": id, "volume": (v as f64 + gain).min(0.0)}));
                            }
                            r.map(|_| format!("every sound {gain:+.0} dB"))
                        }
                        None => Err("no sound".into()),
                    }
                }
                _ => continue,
            };
            if let Ok(s) = r {
                done.push(if s.is_empty() { kind.to_owned() } else { format!("{kind}: {s}") });
            }
        }
        done
    }

    pub(super) fn check_loudness(&self) -> Option<f64> {
        let end = self.doc.project.clips.iter().map(|c| c.end()).max()?;
        let l = super::quality::loudness(&self.doc.model, 0, end);
        l.lufs.is_finite().then_some(l.lufs)
    }
}
