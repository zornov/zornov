// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Staging checks that read the layout: what is really drawn, not the
//! transform boxes. Used by `check` (through `staging_problems_seen`) and by
//! `review` before export.
//!
//! * `layout_overlap`: two things (characters, props, titles) cover each
//!   other by their drawn alpha.
//! * `title_over_face`, `title_over_subject`: a title drawn over a
//!   character's head, over a detected face or person, or over a character.
//! * `off_screen`, `cut_by_frame`: a thing outside the frame, or cut by its
//!   edge (a title always; a character when it is not walking in or out).
//! * `title_unsafe`: lettering outside the title-safe area.
//! * `hidden_behind`: a thing almost entirely covered by another.
//! * `faces_away_listener`: the character whose mouth is animated while a
//!   voice speaks turns its back on the one it talks to.

use super::grid::{self, Rect};
use super::{Layout, Opts, Role};
use crate::agent::{Session, secs};
use crate::view::{self, ClipKind, SEC};
use serde_json::{Value, json};
use studio_model as m;

/// One thing the layout read finds wrong.
#[derive(Clone, Debug)]
pub(in crate::agent) struct LayoutProblem {
    pub kind: &'static str,
    pub at: f64,
    pub clip: u64,
    pub problem: String,
    pub fix: &'static str,
    pub hits: usize,
}

impl LayoutProblem {
    pub(in crate::agent) fn value(&self) -> Value {
        let mut o = json!({"kind": self.kind, "problem": self.problem, "fix": self.fix, "at": (self.at * 100.0).round() / 100.0});
        if self.hits > 1 {
            o["moments"] = json!(self.hits);
        }
        if self.clip != 0 {
            o["clip"] = json!(self.clip);
        }
        o
    }
}

/// The most moments looked at.
const MOMENTS: usize = 28;
/// A circle round a head, as a share of the figure's height.
const HEAD_RADIUS: f32 = 0.16;

/// Whether a circle meets a box.
fn circle_meets(c: (f32, f32), r: f32, b: &Rect) -> bool {
    let (nx, ny) = (c.0.clamp(b[0], b[2]), c.1.clamp(b[1], b[3]));
    (nx - c.0).powi(2) + (ny - c.1).powi(2) <= r * r
}

impl Session {
    /// Moments that show how the things on screen stand: a little into and
    /// before the end of every drawn clip, thinned to [`MOMENTS`].
    fn layout_moments(&self) -> Vec<view::Time> {
        let p = &self.doc.project;
        let mut times: Vec<view::Time> = Vec::new();
        for c in p.clips.iter().filter(|c| c.kind != ClipKind::Audio && p.lane(c.lane).is_some_and(|l| !l.hidden)) {
            if c.dur < SEC / 3 {
                continue;
            }
            times.push(c.start + c.dur * 3 / 10);
            times.push(c.start + c.dur * 7 / 10);
        }
        times.sort_unstable();
        times.dedup_by(|a, b| (*a - *b).abs() < SEC / 3);
        if times.len() > MOMENTS {
            let all = times.clone();
            times = (0..MOMENTS).map(|i| all[i * all.len() / MOMENTS]).collect();
        }
        times
    }

    /// Voice clips playing at `t`.
    fn voices_at(&self, t: view::Time) -> Vec<u32> {
        let p = &self.doc.project;
        p.clips
            .iter()
            .filter(|c| c.kind == ClipKind::Audio && t >= c.start && t < c.start + c.dur && p.lane(c.lane).is_some_and(|l| ["voice", "narrat", "dialog"].iter().any(|w| l.name.to_lowercase().contains(w))))
            .map(|c| c.id)
            .collect()
    }

    /// Whether the figure's mouth is animated within `from..to` (clip time
    /// in micros): a mouth pin with keys, or a mouth part layer swapped.
    fn mouth_moves(&self, id: u64, from: view::Time, to: view::Time) -> bool {
        let seq = self.doc.model.active();
        let Some(mc) = seq.clip(m::ClipId(id)) else { return false };
        let Some(vc) = self.doc.project.clip(id as u32) else { return false };
        let inside = |tr: &m::Track| tr.keys.len() > 1 && tr.keys.iter().any(|k| vc.start + k.time >= from && vc.start + k.time <= to);
        let mouth_pins: Vec<u32> = mc.puppet.as_ref().map(|p| p.pins.iter().filter(|q| q.name.to_lowercase().contains("mouth") || q.name.to_lowercase().contains("jaw")).map(|q| q.id).collect()).unwrap_or_default();
        if mc.keyframes.iter().any(|(path, tr)| mouth_pins.iter().any(|pid| path.starts_with(&format!("puppet.{pid}."))) && inside(tr)) {
            return true;
        }
        seq.clips.iter().filter(|c| c.meta.get("part").and_then(|p| p.get("rig")).and_then(Value::as_u64) == Some(id) && c.name.to_lowercase().contains("mouth")).any(|c| {
            let Some(pv) = self.doc.project.clip(c.id.0 as u32) else { return false };
            c.keyframes.values().any(|tr| tr.keys.len() > 1 && tr.keys.iter().any(|k| pv.start + k.time >= from && pv.start + k.time <= to))
        })
    }

    /// Everything the layout read finds at the moments of the film.
    #[cfg(feature = "render")]
    pub(in crate::agent) fn layout_problems(&mut self) -> Vec<LayoutProblem> {
        let moments = self.layout_moments();
        if moments.is_empty() {
            return Vec::new();
        }
        let key = self.scene_key_pub(0, self.doc.project.duration(), "layout-problems-v1");
        if let Some(v) = self.memo_pub().get(&key).cloned() {
            if let Ok(list) = serde_json::from_value::<Vec<(String, f64, u64, String, usize)>>(v) {
                return list.into_iter().filter_map(|(k, at, clip, problem, hits)| Some(LayoutProblem { kind: kind_of(&k)?, at, clip, problem, fix: fix_of(&k), hits })).collect();
            }
        }
        let mut found: Vec<LayoutProblem> = Vec::new();
        let note = |found: &mut Vec<LayoutProblem>, kind: &'static str, t: view::Time, clip: u64, problem: String, same: &dyn Fn(&LayoutProblem) -> bool| {
            match found.iter_mut().find(|p| p.kind == kind && p.clip == clip && same(p)) {
                Some(p) => p.hits += 1,
                None => found.push(LayoutProblem { kind, at: secs(t), clip, problem, fix: fix_of(kind), hits: 1 }),
            }
        };
        for &t in &moments {
            let Ok(lay) = self.layout_at(t, &Opts { grid: 192, ..Opts::default() }) else { continue };
            self.layout_problems_at(&lay, &mut |kind, clip, problem, tag| note(&mut found, kind, t, clip, problem, &|p: &LayoutProblem| tag.is_empty() || p.problem.contains(tag)));
        }
        let stored: Vec<(String, f64, u64, String, usize)> = found.iter().map(|p| (p.kind.to_owned(), p.at, p.clip, p.problem.clone(), p.hits)).collect();
        self.memo_pub().insert(key, json!(stored));
        self.memo_save_pub();
        found
    }

    /// The problems at one moment.
    fn layout_problems_at(&self, lay: &Layout, say: &mut dyn FnMut(&'static str, u64, String, &str)) {
        let at = secs(lay.t);
        let who = |i: usize| lay.items[i].id;
        let name = |i: usize| format!("clip {} (\"{}\")", lay.items[i].id, lay.items[i].name);
        // Things that meet.
        for p in lay.pairs(0.05) {
            let (a, b) = (&lay.items[p.above], &lay.items[p.below]);
            let texts = [a.role == Role::Text, b.role == Role::Text];
            match (texts[0], texts[1]) {
                (false, false) if p.share >= 0.35 => say("layout_overlap", b.id, format!("{} covers {:.0}% of {} at {at:.1}s by what is drawn: one hides the other", name(p.above), p.share * 100.0, name(p.below)), &name(p.below)),
                (true, true) if p.share >= 0.1 => say("layout_overlap", b.id, format!("the lettering of {} and {} overlap ({:.0}%) at {at:.1}s", name(p.above), name(p.below), p.share * 100.0), &name(p.below)),
                (true, false) | (false, true) if p.share >= 0.08 => {
                    let (ti, oi) = if texts[0] { (p.above, p.below) } else { (p.below, p.above) };
                    let face = self.layout_face_hit(lay, ti, oi);
                    match face {
                        Some(what) => say("title_over_face", lay.items[ti].id, format!("{} is drawn over {} ({what}) at {at:.1}s", name(ti), name(oi)), &name(oi)),
                        None if lay.items[oi].role != Role::Part => say("title_over_subject", lay.items[ti].id, format!("{} is drawn over {} ({:.0}% of the lettering) at {at:.1}s", name(ti), name(oi), p.share * 100.0), &name(oi)),
                        None => {}
                    }
                }
                _ => {}
            }
        }
        // A title can cross a face without the alpha overlap being large.
        for (ti, t) in lay.items.iter().enumerate().filter(|(_, i)| i.role == Role::Text && i.bounds.is_some()) {
            let _ = t;
            for (oi, o) in lay.items.iter().enumerate().filter(|(_, o)| matches!(o.role, Role::Figure | Role::Prop)) {
                if let Some(what) = self.layout_face_hit(lay, ti, oi) {
                    say("title_over_face", who(ti), format!("{} is drawn over {} ({what}) at {at:.1}s", name(ti), name(oi)), &name(oi));
                }
            }
        }
        for i in 0..lay.items.len() {
            let it = &lay.items[i];
            if !matches!(it.role, Role::Text | Role::Figure | Role::Prop) || it.object.is_some() {
                continue;
            }
            for fl in lay.flags(i) {
                match fl.as_str() {
                    "off_screen" => say("off_screen", it.id, format!("{} is drawn outside the frame at {at:.1}s (its box is {} px from the centre)", name(i), it.centre().0.round() as i64), ""),
                    f if f.starts_with("cut_") => {
                        let side = &f[4..];
                        let idx = ["left", "top", "right", "bottom"].iter().position(|s| *s == side).unwrap_or(0);
                        let px = it.spill[idx];
                        // A figure may walk in or out; lettering never should be cut.
                        let worth = it.role == Role::Text || px > 0.06 * if idx % 2 == 0 { lay.w } else { lay.h };
                        if worth {
                            say("cut_by_frame", it.id, format!("{} is cut by the {side} edge of the frame at {at:.1}s ({} px outside)", name(i), px.round() as i64), "");
                        }
                    }
                    "outside_title_safe" => say("title_unsafe", it.id, format!("{} reaches outside the title-safe area at {at:.1}s: some screens crop it", name(i)), ""),
                    "hidden" => {
                        let by = it.covered_by.iter().max_by(|a, b| a.1.total_cmp(&b.1)).map(|&(j, _)| name(j)).unwrap_or_else(|| "something".into());
                        say("hidden_behind", it.id, format!("{} is {:.0}% hidden behind {by} at {at:.1}s", name(i), (1.0 - it.visible) * 100.0), "");
                    }
                    _ => {}
                }
            }
        }
        // Whoever speaks, turned away from who they speak to.
        let voices = self.voices_at(lay.t);
        if !voices.is_empty() {
            let figures: Vec<usize> = (0..lay.items.len()).filter(|&i| lay.items[i].role == Role::Figure && lay.items[i].bounds.is_some()).collect();
            let p = &self.doc.project;
            let speakers: Vec<usize> = figures
                .iter()
                .copied()
                .filter(|&i| {
                    voices.iter().any(|&v| {
                        let Some(vc) = p.clip(v) else { return false };
                        self.mouth_moves(lay.items[i].id, vc.start.max(lay.t - SEC), (vc.start + vc.dur).min(lay.t + SEC))
                    })
                })
                .collect();
            if speakers.len() == 1 && figures.len() >= 2 {
                let s = speakers[0];
                let sx = lay.items[s].centre().0;
                let listener = figures.iter().copied().filter(|&i| i != s).min_by(|&a, &b| (lay.items[a].centre().0 - sx).abs().total_cmp(&(lay.items[b].centre().0 - sx).abs()));
                if let (Some(l), Some(fc)) = (listener, lay.items[s].facing) {
                    let lx = lay.items[l].centre().0;
                    let gap = (lx - sx).abs();
                    if gap > lay.w * 0.06 && ((fc == "right" && lx < sx) || (fc == "left" && lx > sx)) {
                        say("faces_away_listener", lay.items[s].id, format!("{} is speaking at {at:.1}s and faces {fc}, but {} it talks to is to its {}: it turns its back on them", name(s), name(l), if lx < sx { "left" } else { "right" }), "");
                    }
                }
            }
        }
    }

    /// Whether the lettering of item `ti` is over the head of figure `oi`
    /// (its head part, else its drawn top), or over a face or a person found
    /// in its picture. Says what.
    fn layout_face_hit(&self, lay: &Layout, ti: usize, oi: usize) -> Option<String> {
        let text = &lay.items[ti];
        let other = &lay.items[oi];
        let boxes: Vec<Rect> = if text.lines.is_empty() { text.bounds.into_iter().collect() } else { text.lines.clone() };
        if boxes.is_empty() {
            return None;
        }
        if let Some(h) = other.parts.iter().find(|p| p.name == "head") {
            let height = other.bounds.map_or(lay.h * 0.3, |b| b[3] - b[1]);
            if boxes.iter().any(|b| circle_meets(h.at, (HEAD_RADIUS * height).max(lay.h * 0.03), b)) {
                return Some("over its head".to_owned());
            }
        }
        for l in &other.labels {
            let face = ["face", "person", "people", "head", "man", "woman", "child", "animal", "dog", "cat"].iter().any(|w| l.name.to_lowercase().contains(w));
            if face && boxes.iter().any(|b| grid::rect_area(&grid::rect_intersect(b, &l.canvas)) > 0.0) {
                return Some(format!("over the {} found in it", l.name));
            }
        }
        None
    }
}

fn kind_of(k: &str) -> Option<&'static str> {
    ["layout_overlap", "title_over_face", "title_over_subject", "off_screen", "cut_by_frame", "title_unsafe", "hidden_behind", "faces_away_listener"].into_iter().find(|s| *s == k)
}

fn fix_of(kind: &str) -> &'static str {
    match kind {
        "layout_overlap" => "give each its own place: canvas_layout with view \"fit\" and `like` the clip says where it fits; move or scale one, or put one behind with its own space",
        "title_over_face" => "move the title off the face: canvas_layout view \"fit\" with the title's size and `prefer` lower_third or upper_third finds free space; or move the character",
        "title_over_subject" => "move the title into free space (canvas_layout view \"free\" or \"fit\"), or move the subject so the lettering sits on empty picture",
        "off_screen" => "bring it into the frame (set x and y) or remove it; canvas_layout view \"item\" gives its box and motion",
        "cut_by_frame" => "move it inside the frame (canvas_layout view \"safe\" lists the safe areas), or scale it down; only a character walking in or out may be cut",
        "title_unsafe" => "move it inside the title-safe box (canvas_layout view \"safe\" gives it) or make the lettering smaller",
        "hidden_behind" => "bring it forward (restack) or move what covers it; canvas_layout view \"item\" names what covers it and by how much",
        "faces_away_listener" => "flip the speaker (set \"flip\": true) or move it to the other side of who it talks to, so it faces them",
        _ => "",
    }
}
