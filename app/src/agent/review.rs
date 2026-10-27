// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `review`: the film watched the way a viewer watches it, before anyone
//! is told it is done. `check` finds faults clip by clip; this finds what
//! makes a whole film lifeless: one place held for minutes, every shot
//! the same length, characters gliding over the ground with no foot ever
//! planted, everyone doing the same move on the same keys, the voice
//! naming things nobody sees while it says them, places with no sound of
//! their own, and long spans where the sampled frames barely change or
//! look like frames from minutes before. Each note has a time, the clips
//! and what to do.

use super::{Session, secs};
use crate::editor::pins::LayerMap;
use crate::view::{self, ClipKind, SEC};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use studio_model as m;

struct Note {
    kind: &'static str,
    at: f64,
    to: f64,
    clips: Vec<u32>,
    note: String,
    fix: String,
}

fn r1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn ids(v: &[u32]) -> String {
    let mut s: Vec<String> = v.iter().take(8).map(u32::to_string).collect();
    if v.len() > 8 {
        s.push(format!("… {} more", v.len() - 8));
    }
    s.join(", ")
}

/// Things a picture can show: what a voice names that a viewer expects
/// to see. Words in the project's own layer and file names count too.
const THINGS: &[&str] = &[
    // animals
    "animal", "ant", "badger", "bat", "bear", "beaver", "bee", "beetle", "bird", "boar", "bug", "bull", "bunny", "butterfly", "calf", "camel", "cat", "caterpillar", "chick", "chicken", "cow", "crab", "crane", "cub", "deer", "dog", "dolphin", "donkey", "dove", "dragon", "dragonfly", "duck", "duckling", "eagle", "elephant", "fawn", "finch", "fish", "fox", "frog", "goat", "goose", "hawk", "hedgehog", "hen", "heron", "horse", "kitten", "lamb", "lion", "lizard", "mole", "monkey", "moose", "mouse", "otter", "owl", "pig", "pony", "puppy", "rabbit", "raccoon", "rat", "raven", "robin", "rooster", "salmon", "sheep", "snail", "snake", "sparrow", "spider", "squirrel", "stag", "swan", "tiger", "toad", "tortoise", "trout", "turtle", "whale", "wolf", "worm",
    // body parts that get pointed at
    "beak", "claw", "comb", "ear", "eye", "feather", "fin", "hoof", "paw", "tail", "wing", "whisker",
    // nature and places
    "bank", "beach", "berry", "boulder", "branch", "brook", "burrow", "bush", "canyon", "cave", "cliff", "cloud", "creek", "dam", "den", "desert", "field", "flower", "forest", "fern", "garden", "grass", "hill", "hole", "island", "lake", "leaf", "log", "marsh", "meadow", "moon", "moss", "mountain", "mud", "nest", "oak", "ocean", "pebble", "pine", "pond", "puddle", "rain", "rainbow", "ravine", "reed", "ridge", "river", "rock", "root", "sand", "sea", "seed", "shore", "sky", "snow", "spring", "star", "stick", "stone", "storm", "stream", "stump", "sun", "sunrise", "sunset", "swamp", "thorn", "tree", "valley", "water", "waterfall", "wave", "wood", "woods",
    // made things
    "bag", "ball", "barn", "basket", "boat", "book", "bottle", "bowl", "box", "bridge", "bucket", "cake", "candle", "car", "cart", "castle", "chair", "channel", "clock", "coin", "crown", "cup", "door", "fence", "fire", "flag", "gate", "hat", "house", "ladder", "lamp", "lantern", "letter", "map", "mill", "path", "plate", "raft", "road", "roof", "rope", "ship", "sign", "sluice", "table", "tent", "tower", "toy", "track", "trail", "train", "wagon", "wall", "wheel", "window",
    // people
    "baby", "boy", "child", "farmer", "father", "friend", "girl", "king", "man", "mother", "queen", "woman",
];

/// Words in names that say nothing about what is shown.
const PLAIN: &[&str] = &[
    "scene", "shot", "card", "backing", "caption", "line", "audio", "video", "studio", "clip", "layer", "title", "titles", "overlay", "main", "music", "voice", "candidate", "generated", "png", "jpg", "mp3", "wav", "m4a", "ogg", "the", "and", "with", "for", "from", "into", "part", "take", "copy", "merged", "world", "cast", "staging", "action", "three", "story", "shots", "kevin", "macleod",
];

/// A word as a viewer would match it: lower case, no punctuation, no
/// possessive, singular.
pub(super) fn norm(w: &str) -> String {
    let w = w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
    let w = w.strip_suffix("'s").or_else(|| w.strip_suffix("’s")).unwrap_or(&w).to_owned();
    for (end, put) in [("ies", "y"), ("ves", "ve"), ("ches", "ch"), ("shes", "sh"), ("xes", "x"), ("es", "e"), ("s", "")] {
        if w.len() > end.len() + 2 && w.ends_with(end) && !w.ends_with("ss") {
            let stem = &w[..w.len() - end.len()];
            // Leaves, wolves: the thing is a leaf, a wolf (waves stay waves).
            if end == "ves" && THINGS.contains(&format!("{stem}f").as_str()) {
                return format!("{stem}f");
            }
            return format!("{stem}{put}");
        }
    }
    w
}

fn words_of(s: &str) -> impl Iterator<Item = String> + '_ {
    s.split(|c: char| !c.is_alphanumeric() && c != '\'' && c != '’').filter(|w| w.len() > 1).map(norm)
}

fn is_voice(lane: &str, path: &str) -> bool {
    let l = lane.to_lowercase();
    ["voice", "narrat", "dialog", "speech"].iter().any(|w| l.contains(w)) || path.contains("made-voice") || std::path::Path::new(path).with_extension("words.json").exists()
}

pub(super) fn is_music(lane: &str, name: &str, dur: view::Time) -> bool {
    let (l, n) = (lane.to_lowercase(), name.to_lowercase());
    ["music", "song", "score"].iter().any(|w| l.contains(w)) || (dur >= 60 * SEC && n.contains(" - "))
}

fn is_ambience(lane: &str, name: &str, dur: view::Time) -> bool {
    let s = format!("{} {}", lane.to_lowercase(), name.to_lowercase());
    ["ambien", "atmos", "room tone", "wind", "rain", "forest", "nature", "birds", "stream", "creek", "river", "water", "crickets", "waves", "ocean", "storm", "leaves", "night", "city", "crowd", "traffic", "bed"].iter().any(|w| s.contains(w)) || dur >= 8 * SEC
}

/// One character layer, watched over its clip.
struct Acted {
    id: u32,
    who: String,
    start: view::Time,
    end: view::Time,
    label: String,
    /// Spans (seconds) where it travels over the ground with no foot down.
    slides: Vec<(f64, f64)>,
    travel: f64,
    /// The same keyed performance (paths, times, moves) gives the same value.
    copy: Option<u64>,
    /// The same key times, whatever the moves.
    timing: Option<u64>,
}

impl Session {
    /// The main lane's pictures, in order.
    fn review_shots(&self) -> Vec<view::Clip> {
        let p = &self.doc.project;
        let spine = p.spine();
        let mut cs: Vec<view::Clip> = p.clips.iter().filter(|c| Some(c.lane) == spine && c.kind == ClipKind::Video).cloned().collect();
        // A film built on overlay lanes (vector scenes, a 3D set with its
        // camera) has an empty main lane but shots all the same: the
        // picture clips of the lowest picture lane that has any (it said
        // "0 shots").
        if cs.is_empty() {
            let rig = |id: u32| self.doc.model.active().clip(m::ClipId(id as u64)).is_some_and(|mc| mc.is_camera_or_light());
            let picture = |c: &&view::Clip| c.kind == ClipKind::Video && !rig(c.id);
            let lanes: Vec<u32> = p.lanes.iter().rev().filter(|l| l.kind == view::LaneKind::Visual).map(|l| l.id).collect();
            if let Some(lane) = lanes.into_iter().find(|l| p.clips.iter().filter(picture).any(|c| c.lane == *l)) {
                cs = p.clips.iter().filter(picture).filter(|c| c.lane == lane).cloned().collect();
            }
        }
        cs.sort_by_key(|c| c.start);
        cs
    }

    /// What picture a shot shows and how it is framed: two shots with the
    /// same value look the same to a viewer.
    fn framing_of(&self, c: &view::Clip) -> String {
        let p = &self.doc.project;
        let what = c.media.map(|m| format!("media {m}")).unwrap_or_else(|| c.name.clone());
        let crop = self.doc.model.sequences.iter().find_map(|q| q.clip(m::ClipId(c.id as u64))).map(|mc| mc.transform.crop);
        let crop = crop.map(|k| format!("{:.2},{:.2},{:.2},{:.2}", k.left, k.top, k.right, k.bottom)).unwrap_or_default();
        format!("{what}|{crop}|{}|{}|{}", (c.transform.scale / 10.0).round(), (c.transform.x / (p.width as f32 * 0.1)).round(), (c.transform.y / (p.height as f32 * 0.1)).round())
    }

    /// The picture a shot shows (its file and the part of it cropped), however
    /// it is scaled or moved.
    fn picture_of(&self, c: &view::Clip) -> String {
        self.framing_of(c).rsplitn(4, '|').last().unwrap_or_default().to_owned()
    }

    /// Visible picture and title layers at `t` (not the main lane's, not
    /// captions), with their names and notes.
    fn shown_at(&self, t: view::Time) -> Vec<&view::Clip> {
        let p = &self.doc.project;
        p.clips
            .iter()
            .filter(|c| c.kind != ClipKind::Audio && c.start <= t && t < c.end())
            .filter(|c| p.lane(c.lane).is_some_and(|l| !l.hidden && !l.name.to_lowercase().contains("caption") && !l.name.to_lowercase().contains("subtitle")))
            .filter(|c| c.name != "Caption" && self.doc.model.sequences.iter().find_map(|q| q.clip(m::ClipId(c.id as u64))).is_none_or(|mc| mc.enabled && mc.transform.opacity > 0.05))
            .collect()
    }

    fn vocabulary(&self, c: &view::Clip) -> HashSet<String> {
        let p = &self.doc.project;
        let mut s = String::new();
        s.push_str(&c.name);
        s.push(' ');
        if let Some(mi) = c.media.and_then(|m| p.media(m)) {
            s.push_str(&mi.name);
            s.push(' ');
        }
        if c.kind == ClipKind::Title {
            s.push_str(&c.text);
            s.push(' ');
        }
        if let Some(mc) = self.doc.model.sequences.iter().find_map(|q| q.clip(m::ClipId(c.id as u64))) {
            for v in mc.meta.values() {
                if let Some(t) = v.as_str() {
                    s.push_str(t);
                    s.push(' ');
                }
            }
            for mk in &mc.markers {
                s.push_str(&mk.name);
                s.push(' ');
                s.push_str(&mk.note);
                s.push(' ');
            }
        }
        words_of(&s).collect()
    }

    pub(super) fn review(&mut self, a: &Value) -> Value {
        let mut notes: Vec<Note> = Vec::new();
        let p = self.doc.project.clone();
        let end = p.duration();
        let shots = self.review_shots();

        // ------------------------------------------------------ 3D craft
        // Open 3D notes keep the film "not done" however it looks.
        let (three_d, set_aside) = self.three_d_problems();
        // The performances of rigged characters: feet that slide or cross, limbs
        // stretched or broken, gazes that miss what they were told.
        let performed = self.character_problems();
        let looks = self.look_problems(true);
        if a.get("only").and_then(Value::as_str).is_some_and(|o| o.eq_ignore_ascii_case("3d")) {
            let mut open: Vec<Value> = three_d.iter().map(|p| json!({"kind": p.kind, "at": r1(p.at), "clip": p.clip, "note": p.problem, "fix": p.fix})).collect();
            open.extend(performed.iter().map(|p| json!({"kind": p.kind, "at": r1(p.at), "clip": p.clip, "note": p.problem, "fix": p.fix})));
            open.extend(looks.iter().map(|p| json!({"kind": p.kind, "at": r1(p.at), "clip": p.clip, "note": p.problem, "fix": p.fix})));
            return json!({
                "ok": open.is_empty(),
                "verdict": if open.is_empty() { "no 3D notes open".to_owned() } else { format!("NOT DONE: {} 3D notes open; fix each, or list them plainly in your answer", open.len()) },
                "notes": open,
                "set_aside_by_ai": set_aside,
            });
        }
        for p in &three_d {
            notes.push(Note { kind: p.kind, at: p.at, to: p.at, clips: if p.clip != 0 { vec![p.clip] } else { Vec::new() }, note: p.problem.clone(), fix: p.fix.to_owned() });
        }
        for p in &performed {
            notes.push(Note { kind: p.kind, at: p.at, to: p.at, clips: vec![p.clip], note: p.problem.clone(), fix: p.fix.clone() });
        }
        for p in &looks {
            notes.push(Note { kind: p.kind, at: p.at, to: p.at, clips: if p.clip != 0 { vec![p.clip] } else { Vec::new() }, note: p.problem.clone(), fix: p.fix.clone() });
        }
        let three_d_open = three_d.len() + performed.len() + looks.len();

        // ------------------------------------------------ where things are
        // Drawn alpha, not boxes: overlaps, titles over faces, things off
        // screen or cut, lettering outside the safe area, a speaker turned
        // from the one it talks to.
        #[cfg(feature = "render")]
        for p in self.layout_problems().iter().take(10) {
            notes.push(Note { kind: p.kind, at: p.at, to: p.at, clips: if p.clip != 0 { vec![p.clip as u32] } else { Vec::new() }, note: p.problem.clone(), fix: p.fix.to_owned() });
        }

        // ---------------------------------------------------- places
        // One picture held shot after shot (pushed in or out a little, it
        // is still the same place to a viewer).
        let mut runs: Vec<(String, view::Time, view::Time, Vec<u32>, String)> = Vec::new();
        for c in &shots {
            let k = self.picture_of(c);
            match runs.last_mut() {
                Some(r) if r.0 == k && c.start - r.2 <= SEC / 2 => {
                    r.2 = c.end();
                    r.3.push(c.id);
                }
                _ => runs.push((k, c.start, c.end(), vec![c.id], c.name.clone())),
            }
        }
        let held = (45 * SEC).max(end / 8);
        for r in runs.iter().filter(|r| r.2 - r.1 >= held) {
            let ways: HashSet<String> = shots.iter().filter(|c| r.3.contains(&c.id)).map(|c| self.framing_of(c)).collect();
            let framed = if ways.len() > 1 { format!("framed only {} ways", ways.len()) } else { "framed the same way".to_owned() };
            notes.push(Note {
                kind: "place_held",
                at: secs(r.1),
                to: secs(r.2),
                clips: r.3.clone(),
                note: format!("from {:.0}s to {:.0}s the backdrop is one picture, \"{}\", {framed} for {:.0}s ({} shots: clips {}): the place never changes while the story moves on", secs(r.1), secs(r.2), r.4, secs(r.2 - r.1), r.3.len(), ids(&r.3)),
                fix: "give each beat its own view of the place: another part of it, a closer or wider framing, a new angle, different light or weather (generate the view, or crop and reframe a wider picture); something about the place should change every 20 to 30 seconds".into(),
            });
        }
        let mut share: HashMap<String, (view::Time, Vec<u32>, String)> = HashMap::new();
        for c in &shots {
            let e = share.entry(self.picture_of(c)).or_insert((0, Vec::new(), c.name.clone()));
            e.0 += c.dur;
            e.1.push(c.id);
        }
        if end >= 120 * SEC {
            for (_, (t, cs, name)) in share.iter().filter(|(_, v)| v.0 * 10 >= end * 3) {
                let first = shots.iter().find(|c| c.id == cs[0]).map_or(0, |c| c.start);
                notes.push(Note {
                    kind: "place_reused",
                    at: secs(first),
                    to: secs(first),
                    clips: cs.clone(),
                    note: format!("\"{name}\" fills {:.0}% of the film ({:.0}s over {} shots: clips {}): the viewer keeps seeing the same picture", *t as f64 * 100.0 / end.max(1) as f64, secs(*t), cs.len(), ids(cs)),
                    fix: "make each return to the place a different shot of it, and give new events new places".into(),
                });
            }
        }

        // ---------------------------------------------------- sound variety
        for f in self.variety() {
            notes.push(Note { kind: f.kind, at: f.at, to: f.to, clips: f.clips, note: f.note, fix: f.fix });
        }

        // ---------------------------------------------------- pacing
        if shots.len() >= 6 {
            let d: Vec<f64> = shots.iter().map(|c| secs(c.dur)).collect();
            let mean = d.iter().sum::<f64>() / d.len() as f64;
            let sd = (d.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / d.len() as f64).sqrt();
            if sd < mean * 0.08 {
                notes.push(Note {
                    kind: "even_cuts",
                    at: 0.0,
                    to: secs(end),
                    clips: Vec::new(),
                    note: format!("all {} shots on the main lane last about {:.1}s: the film cuts like a metronome, not with the story", d.len(), mean),
                    fix: "cut where the story turns: at the end of a sentence, on an action, on a reaction; short shots for action, longer ones for a moment that lands, so no two beats have the same length".into(),
                });
            }
        }
        let words = self.spoken_words();
        let mut mid = Vec::new();
        for c in shots.iter().skip(1) {
            let t = secs(c.start);
            let i = words.partition_point(|w| w.end <= t);
            let Some(w) = words.get(i) else { continue };
            let Some(prev) = i.checked_sub(1).and_then(|j| words.get(j)) else { continue };
            let inside = w.start < t - 0.05;
            let sentence_goes_on = w.start - t < 0.35 && t - prev.end < 0.35 && !prev.text.ends_with(['.', '!', '?', '…']);
            if inside || sentence_goes_on {
                mid.push((c.id, t, format!("{} {}", prev.text, w.text)));
            }
        }
        for (id, t, said) in mid.iter().take(6) {
            notes.push(Note { kind: "cut_mid_sentence", at: r1(*t), to: r1(*t), clips: vec![*id], note: format!("the cut to clip {id} at {t:.1}s falls in the middle of a sentence (\"{said}\")"), fix: "move the cut to the pause after the sentence ends (spoken words are on the voice lane's clips; summary lists them)".into() });
        }
        // Cuts under music against its beat.
        let mut beats: Vec<f64> = Vec::new();
        for c in p.clips.iter().filter(|c| c.kind == ClipKind::Audio) {
            let lane = p.lane(c.lane).map(|l| l.name.clone()).unwrap_or_default();
            let Some(mi) = c.media.and_then(|m| p.media(m)) else { continue };
            if !is_music(&lane, &mi.name, mi.dur) || p.lane(c.lane).is_some_and(|l| l.muted) {
                continue;
            }
            let Some(path) = &mi.path else { continue };
            let Some(d) = crate::sound_bridge::decoded(&path.to_string_lossy()) else { continue };
            let rate = d.rate as f64;
            let speed = (c.speed as f64 / 100.0).max(0.01);
            let i0 = ((secs(c.src_in) * rate) as usize).min(d.left.len());
            let i1 = (((secs(c.src_in) + secs(c.dur) * speed) * rate) as usize).min(d.left.len());
            if i1 <= i0 {
                continue;
            }
            let bt = studio_ai::features::beats::beat_times(&d.left[i0..i1], d.rate, 1);
            if studio_ai::features::beats::tempo(&bt).is_some() {
                beats.extend(bt.iter().map(|b| secs(c.start) + b / speed));
            }
        }
        beats.sort_by(f64::total_cmp);
        if !beats.is_empty() {
            let under: Vec<f64> = shots.iter().skip(1).map(|c| secs(c.start)).filter(|t| beats.first().is_some_and(|b| b - 1.0 <= *t) && beats.last().is_some_and(|b| *t <= b + 1.0)).collect();
            let on = under.iter().filter(|t| studio_ai::features::beats::nearest_beat(&beats, **t, 0.1).is_some()).count();
            if under.len() >= 6 && on * 10 < under.len() * 3 {
                notes.push(Note { kind: "cuts_off_beat", at: 0.0, to: secs(end), clips: Vec::new(), note: format!("of {} cuts under the music, {on} land on a beat: picture and music do not move together", under.len()), fix: "where a sentence ends near a beat, put the cut on that beat (listen on the music clip gives its beats); let a shot change with the music when a new piece starts".into() });
            }
        }

        // ---------------------------------------------------- acting
        let acted = self.review_acting();
        let mut slid: Vec<&Acted> = acted.iter().filter(|a| !a.slides.is_empty()).collect();
        slid.sort_by(|a, b| b.travel.total_cmp(&a.travel));
        for a in slid.iter().take(8) {
            let spans: Vec<String> = a.slides.iter().take(3).map(|(s, e)| format!("{s:.1}–{e:.1}s")).collect();
            let total: f64 = a.slides.iter().map(|(s, e)| e - s).sum();
            notes.push(Note {
                kind: "slides",
                at: r1(a.slides[0].0),
                to: r1(a.slides.last().map_or(0.0, |s| s.1)),
                clips: vec![a.id],
                note: format!("{} (clip {}) glides over the ground for {total:.1}s ({}) with no foot planted: the layer moves, the character does not walk", a.who, a.id, spans.join(", ")),
                fix: "walk it instead of sliding it: puppet act walk (or keys on the feet pins) so each step plants a foot that stays put on the ground while the body passes over it, and the body travels only as far as the steps carry it; or keep it in place and act on the spot".into(),
            });
        }
        if slid.len() > 8 {
            notes.push(Note { kind: "slides", at: f64::INFINITY, to: 0.0, clips: slid[8..].iter().map(|a| a.id).collect(), note: format!("{} more character layers slide the same way", slid.len() - 8), fix: "the same".into() });
        }
        let mut copies: BTreeMap<u64, Vec<&Acted>> = BTreeMap::new();
        let mut timings: BTreeMap<u64, Vec<&Acted>> = BTreeMap::new();
        for a in &acted {
            if let Some(h) = a.copy {
                copies.entry(h).or_default().push(a);
            }
            if let Some(h) = a.timing {
                timings.entry(h).or_default().push(a);
            }
        }
        // Per character: the performances it repeats, and where.
        let mut copied: HashSet<u32> = HashSet::new();
        let mut repeats: BTreeMap<&str, Vec<&Vec<&Acted>>> = BTreeMap::new();
        for g in copies.values().filter(|g| g.len() >= 2) {
            copied.extend(g.iter().map(|a| a.id));
            repeats.entry(g[0].who.as_str()).or_default().push(g);
        }
        for (who, gs) in &repeats {
            let mut all: Vec<&Acted> = gs.iter().flat_map(|g| g.iter().copied()).collect();
            all.sort_by_key(|a| a.start);
            let cs: Vec<u32> = all.iter().map(|a| a.id).collect();
            let each: Vec<String> = gs.iter().take(4).map(|g| g.iter().take(6).map(|a| format!("{:.0}s", secs(a.start))).collect::<Vec<_>>().join(", ")).collect();
            let what = if gs.len() == 1 { "the very same keyed performance".to_owned() } else { format!("{} keyed performances over and over", gs.len()) };
            notes.push(Note {
                kind: "same_acting",
                at: secs(all[0].start),
                to: secs(all[all.len() - 1].end),
                clips: cs.clone(),
                note: format!("{who} plays {what} in {} shots (clips {}; the same at {}): the same moves at the same moments, whatever the scene is about", all.len(), ids(&cs), each.join("; and at ")),
                fix: "act each shot for what happens in it: what the character wants there, who it reacts to, what it does with its body (puppet act with different actions, timed to the line)".into(),
            });
        }
        for g in timings.values() {
            let who: HashSet<&str> = g.iter().map(|a| a.who.as_str()).collect();
            if who.len() < 3 {
                continue;
            }
            let cs: Vec<u32> = g.iter().map(|a| a.id).filter(|id| !copied.contains(id)).collect();
            if cs.len() < 3 {
                continue;
            }
            let mut names: Vec<&str> = who.into_iter().collect();
            names.sort();
            notes.push(Note {
                kind: "moves_alike",
                at: secs(g[0].start),
                to: secs(g[g.len() - 1].end),
                clips: cs.clone(),
                note: format!("{} all move on exactly the same key times ({} layers: clips {}): one performance stamped on every character", names.join(", "), cs.len(), ids(&cs)),
                fix: "give each character its own tempo and its own kind of move (a rabbit hops and twitches, a turtle plods, a bird bobs and pecks), and let them act at different moments: one speaks, the others react after it".into(),
            });
        }
        // In one shot, everyone doing the same thing.
        let mut alike_shots = 0;
        for s in &shots {
            let t = s.start + s.dur / 2;
            let here: Vec<&Acted> = acted.iter().filter(|a| a.start <= t && t < a.end).collect();
            if here.len() >= 2 && here.iter().all(|a| a.label == here[0].label) && !here[0].label.starts_with("stays, still") {
                alike_shots += 1;
                if alike_shots <= 4 {
                    let cs: Vec<u32> = here.iter().map(|a| a.id).collect();
                    let who: Vec<&str> = here.iter().map(|a| a.who.as_str()).collect();
                    notes.push(Note { kind: "moves_alike", at: secs(s.start), to: secs(s.end()), clips: cs.clone(), note: format!("in the shot from {:.0}s ({}), {} all do the same thing: {}", secs(s.start), s.name, who.join(", "), here[0].label), fix: "one character leads the moment and the others answer it differently: turn to look, step back, freeze, nod; vary tempo and size".into() });
                }
            }
        }
        if acted.len() >= 6 {
            let mut count: HashMap<&str, usize> = HashMap::new();
            for a in &acted {
                *count.entry(a.label.as_str()).or_default() += 1;
            }
            if let Some((label, n)) = count.into_iter().max_by_key(|e| e.1).filter(|(_, n)| n * 10 >= acted.len() * 6) {
                notes.push(Note { kind: "one_move", at: 0.0, to: secs(end), clips: Vec::new(), note: format!("{n} of {} character layers do the same thing: {label}", acted.len()), fix: "match each move to the line and the character: walk in, stop, look, point, hop, sit, startle; some still, some busy".into() });
            }
        }

        // ---------------------------------------------------- motion
        notes.extend(self.review_motion());

        // ---------------------------------------------------- words
        let (said, shown, unshown) = self.review_words(&words);
        let mut told = 0;
        for (t0, t1, subjects, on, elsewhere) in &unshown {
            told += 1;
            if told > 10 {
                break;
            }
            let ws: Vec<String> = subjects.iter().map(|w| format!("\"{w}\"")).collect();
            let seen = if on.is_empty() { "nothing named".to_owned() } else { on.join(", ") };
            let later = if elsewhere.is_empty() { String::new() } else { format!(" ({})", elsewhere.join("; ")) };
            let cs: Vec<u32> = self.shown_at(super::micros(*t0)).iter().map(|c| c.id).collect();
            notes.push(Note {
                kind: "unshown",
                at: r1(*t0),
                to: r1(*t1),
                clips: cs,
                note: format!("from {t0:.1}s to {t1:.1}s the voice says {} but none of it is on screen (on screen: {seen}){later}", ws.join(", ")),
                fix: "show what is said while it is said: put the thing or the character in the picture (scene / add, named for what it is), cut to it, or have someone point at it; if it cannot be shown, say it differently".into(),
            });
        }
        if unshown.len() > 10 {
            notes.push(Note { kind: "unshown", at: f64::INFINITY, to: 0.0, clips: Vec::new(), note: format!("{} more sentences name things that are not on screen", unshown.len() - 10), fix: "the same".into() });
        }

        // ---------------------------------------------------- sound
        let mut places: Vec<(String, view::Time, view::Time, Vec<u32>)> = Vec::new();
        for c in &shots {
            let key = c.media.map(|m| m.to_string()).unwrap_or_else(|| c.name.clone());
            match places.last_mut() {
                Some(r) if r.0 == key && c.start - r.2 <= SEC / 2 => {
                    r.2 = c.end();
                    r.3.push(c.id);
                }
                _ => places.push((key, c.start, c.end(), vec![c.id])),
            }
        }
        let beds: Vec<&view::Clip> = p
            .clips
            .iter()
            .filter(|c| c.kind == ClipKind::Audio && !p.lane(c.lane).is_some_and(|l| l.muted))
            .filter(|c| {
                let lane = p.lane(c.lane).map(|l| l.name.as_str()).unwrap_or("");
                let mi = c.media.and_then(|m| p.media(m));
                let (name, path, dur) = mi.map_or((c.name.clone(), String::new(), 0), |mi| (mi.name.clone(), mi.path.as_ref().map(|q| q.to_string_lossy().into_owned()).unwrap_or_default(), mi.dur));
                !is_voice(lane, &path) && !is_music(lane, &name, dur) && is_ambience(lane, &name, c.dur)
            })
            .collect();
        let mut bare = 0;
        for (_, s, e, cs) in places.iter().filter(|r| r.2 - r.1 >= 12 * SEC) {
            let covered: view::Time = beds.iter().map(|b| (b.end().min(*e) - b.start.max(*s)).max(0)).sum();
            if covered * 2 < e - s {
                bare += 1;
                if bare <= 6 {
                    let name = shots.iter().find(|c| c.id == cs[0]).map(|c| c.name.clone()).unwrap_or_default();
                    notes.push(Note { kind: "no_ambience", at: secs(*s), to: secs(*e), clips: cs.clone(), note: format!("\"{name}\" ({:.0}–{:.0}s) has no sound of its own under it, only voice and music: the place is a painting, not somewhere", secs(*s), secs(*e)), fix: "lay a quiet bed of the place under it (sounds / catalogue: wind, birds, water, rain; listen to pick one) about 18 dB under the voice, changing when the place or the weather changes".into() });
                }
            }
        }
        for b in &beds {
            let under: Vec<&(String, view::Time, view::Time, Vec<u32>)> = places.iter().filter(|r| (b.end().min(r.2) - b.start.max(r.1)) > 5 * SEC).collect();
            let kinds: HashSet<&str> = under.iter().map(|r| r.0.as_str()).collect();
            if kinds.len() >= 3 {
                notes.push(Note { kind: "same_ambience", at: secs(b.start), to: secs(b.end()), clips: vec![b.id], note: format!("one bed, clip {} (\"{}\"), plays on under {} different places ({:.0}–{:.0}s)", b.id, b.name, kinds.len(), secs(b.start), secs(b.end())), fix: "end it where its place ends and give the next place its own sound".into() });
            }
        }

        // ---------------------------------------------------- frames
        #[cfg(feature = "render")]
        if a.get("frames").and_then(Value::as_bool) != Some(false) {
            notes.extend(self.review_frames(end));
        }
        let _ = a;

        let order = |k: &str| [crate::agent::three_d_check::KINDS, studio_mesh::perform::check::KINDS, &["unshown", "slides", "faked_change", "place_held", "still_picture", "looks_same", "same_acting", "moves_alike", "jitter", "stiff_motion", "one_move", "place_reused", "even_cuts", "no_ambience", "same_ambience", "cut_mid_sentence", "cuts_off_beat"][..]].concat().iter().position(|x| *x == k).unwrap_or(99);
        notes.sort_by(|x, y| order(x.kind).cmp(&order(y.kind)).then(x.at.total_cmp(&y.at)));
        let kinds: Vec<&str> = {
            let mut k: Vec<&str> = notes.iter().map(|n| n.kind).collect();
            k.dedup();
            k
        };
        let out: Vec<Value> = notes
            .iter()
            .map(|n| {
                let mut o = json!({"kind": n.kind, "note": n.note, "fix": n.fix});
                if n.at.is_finite() {
                    o["at"] = json!(r1(n.at));
                }
                if n.to > n.at {
                    o["to"] = json!(r1(n.to));
                }
                if !n.clips.is_empty() {
                    o["clips"] = json!(n.clips);
                }
                o
            })
            .collect();
        let distinct: HashSet<String> = shots.iter().map(|c| self.framing_of(c)).collect();
        let verdict = if three_d_open > 0 {
            format!("NOT DONE: {three_d_open} 3D notes are open ({} notes in all). A model that is a primitive, missing its parts or flatly lit is not finished however the frames look: fix each, then review again. If you cannot, the final answer must list them as open; never say review passed", notes.len())
        } else if notes.is_empty() {
            "nothing a viewer would find lifeless: places change, characters act differently, what is said is shown".to_owned()
        } else {
            format!("a viewer would find this lifeless in places: {} notes ({}). Act on each, then review again; do not call the film done while these stand", notes.len(), kinds.join(", "))
        };
        json!({
            "ok": notes.is_empty(),
            "verdict": verdict,
            "film": {
                "duration": r1(secs(end)),
                "shots": shots.len(),
                "different_shots": distinct.len(),
                "average_shot": if shots.is_empty() { 0.0 } else { r1(shots.iter().map(|c| secs(c.dur)).sum::<f64>() / shots.len() as f64) },
                "character_layers": acted.len(),
                "sliding": slid.len(),
                "things_said": said,
                "things_said_and_shown": shown,
            },
            "three_d": {"open": three_d_open, "set_aside_by_ai": set_aside},
            "notes": out,
        })
    }

    /// Every character layer, sampled five times a second: where its body
    /// and feet go relative to the ground under them.
    fn review_acting(&self) -> Vec<Acted> {
        use std::hash::{Hash, Hasher};
        let p = &self.doc.project;
        let model = &self.doc.model;
        let spine = p.spine();
        let step = SEC / 5;
        let dt = secs(step);
        let h = p.height.max(1) as f64;
        let ground_at = |t: view::Time| p.clips.iter().find(|c| Some(c.lane) == spine && c.kind == ClipKind::Video && c.start <= t && t < c.end()).and_then(|c| LayerMap::at(p, model, c.id, t));
        let mut out = Vec::new();
        for c in self.figures() {
            let Some(mc) = model.sequences.iter().find_map(|q| q.clip(m::ClipId(c.id as u64))) else { continue };
            let pins: Vec<&m::puppet::Pin> = mc.puppet.as_ref().filter(|q| q.enabled).map(|q| q.pins.iter().filter(|p| p.kind == m::puppet::PinKind::Position).collect()).unwrap_or_default();
            let mut feet: Vec<&m::puppet::Pin> = pins.iter().copied().filter(|p| ["foot", "feet", "toe", "hoof", "paw", "heel"].iter().any(|w| p.name.to_lowercase().contains(w))).collect();
            if feet.is_empty() && pins.len() >= 4 {
                let mut low = pins.clone();
                low.sort_by(|a, b| b.rest.y.total_cmp(&a.rest.y));
                feet = low.into_iter().take(2).collect();
            }
            let pin_at = |pin: &m::puppet::Pin, local: view::Time| match mc.property_at(&format!("puppet.{}.position", pin.id), local) {
                Some(m::Value::Point(v)) => v,
                _ => pin.position,
            };
            let (s, e) = (c.start, c.end());
            let mut prev: Option<(m::Vec2, Vec<m::Vec2>, Option<LayerMap>)> = None;
            let mut travel_n = 0usize;
            let mut sliding: Vec<(f64, f64)> = Vec::new();
            let mut net = 0.0f64;
            let mut both = (false, false);
            let mut act = (0.0f64, 0usize);
            let mut t = s;
            while t < e {
                let local = t - s;
                let Some(map) = LayerMap::at(p, model, c.id, t) else {
                    t += step;
                    prev = None;
                    continue;
                };
                let cv = map.canvas(m::Vec2::new(0.5, 0.5));
                let centre = m::Vec2::new(cv.x as f64, cv.y as f64);
                let fv: Vec<m::Vec2> = feet
                    .iter()
                    .map(|f| {
                        let q = map.canvas(pin_at(f, local));
                        m::Vec2::new(q.x as f64, q.y as f64)
                    })
                    .collect();
                for pin in &pins {
                    let v = pin_at(pin, local);
                    act.0 += ((v.x - pin.rest.x).powi(2) + (v.y - pin.rest.y).powi(2)).sqrt();
                    act.1 += 1;
                }
                let ground = ground_at(t);
                if let Some((pc, pf, was)) = &prev {
                    // How far the ground under a point moved between samples.
                    let shift = |q: &m::Vec2| -> (f64, f64) {
                        match (was, &ground) {
                            (Some(a), Some(b)) => match a.layer(eframe::egui::Vec2::new(q.x as f32, q.y as f32)) {
                                Some(uv) => {
                                    let n = b.canvas(uv);
                                    (n.x as f64 - q.x, n.y as f64 - q.y)
                                }
                                None => (0.0, 0.0),
                            },
                            _ => (0.0, 0.0),
                        }
                    };
                    let rel = |a: &m::Vec2, b: &m::Vec2| {
                        let g = shift(a);
                        (b.x - a.x - g.0, b.y - a.y - g.1)
                    };
                    let (bx, by) = rel(pc, &centre);
                    let speed = (bx * bx + by * by).sqrt() / dt;
                    if speed / h > 0.025 {
                        travel_n += 1;
                        net += bx;
                        both = (both.0 || bx < 0.0, both.1 || bx > 0.0);
                        let planted = pf.iter().zip(&fv).any(|(a, b)| {
                            let (fx, fy) = rel(a, b);
                            (fx * fx + fy * fy).sqrt() / dt < 0.35 * speed
                        });
                        if !planted {
                            let at = secs(t);
                            match sliding.last_mut() {
                                Some(r) if at - r.1 <= dt * 2.5 => r.1 = at,
                                _ => sliding.push((at - dt, at)),
                            }
                        }
                    }
                }
                prev = Some((centre, fv, ground));
                t += step;
            }
            // A 3D rigged character has no pins to test: its feet are judged from the
            // posed bones by the character critic, not from layer movement here.
            if matches!(&mc.source, m::ClipSource::Generator(g) if g.kind == m::GeneratorKind::Model) {
                sliding.clear();
            }
            sliding.retain(|(a, b)| b - a >= 0.8);
            let travel = travel_n as f64 * dt;
            let way = if travel < 1.0 {
                "stays"
            } else if both.0 && both.1 && net.abs() < travel * 0.02 * h {
                "paces to and fro"
            } else if net < 0.0 {
                "goes left"
            } else {
                "goes right"
            };
            let amount = if act.1 == 0 { 0.0 } else { act.0 / act.1 as f64 };
            let size = if amount < 0.004 { "still" } else if amount < 0.02 { "small moves" } else { "big moves" };
            // The tempo of its keys: the usual gap between pin keys.
            let mut gaps: Vec<i64> = mc.keyframes.iter().filter(|(k, tr)| (k.starts_with("puppet.") || k.starts_with("bone.") || k.starts_with("ik.") || k.starts_with("generator.bone.") || k.starts_with("generator.ik.")) && tr.keys.len() >= 4).flat_map(|(_, tr)| tr.keys.windows(2).map(|w| (w[1].time - w[0].time) / (SEC / 20)).collect::<Vec<_>>()).filter(|g| *g > 0).collect();
            gaps.sort();
            let tempo = gaps.get(gaps.len() / 2).map(|g| format!(", a key every {:.2}s", *g as f64 / 20.0)).unwrap_or_default();
            let label = format!("{way}, {size}{tempo}");
            let tracks: Vec<(&String, &m::Track)> = mc.keyframes.iter().filter(|(k, tr)| super::perform::is_performance_key(k) && tr.keys.len() > 1).collect();
            let (copy, timing) = if tracks.len() >= 2 {
                let mut hc = std::collections::hash_map::DefaultHasher::new();
                let mut seqs: Vec<Vec<i64>> = Vec::new();
                for (path, tr) in &tracks {
                    path.hash(&mut hc);
                    let first = tr.keys[0].value.clone();
                    let ts: Vec<i64> = tr.keys.iter().map(|k| k.time / (SEC / 20)).collect();
                    ts.hash(&mut hc);
                    for k in &tr.keys {
                        let d = match (&k.value, &first) {
                            (m::Value::Point(a), m::Value::Point(b)) => ((a.x - b.x) * 100.0).round() as i64 * 100_000 + ((a.y - b.y) * 100.0).round() as i64,
                            (m::Value::Scalar(a), m::Value::Scalar(b)) | (m::Value::Angle(a), m::Value::Angle(b)) => (a - b).round() as i64,
                            _ => 0,
                        };
                        d.hash(&mut hc);
                    }
                    seqs.push(ts);
                }
                seqs.sort();
                seqs.dedup();
                let mut ht = std::collections::hash_map::DefaultHasher::new();
                seqs.hash(&mut ht);
                (Some(hc.finish()), (seqs.iter().map(Vec::len).sum::<usize>() >= 6).then(|| ht.finish()))
            } else {
                (None, None)
            };
            let who = c.name.split('|').next().unwrap_or(&c.name).trim().to_owned();
            out.push(Acted { id: c.id, who, start: s, end: e, label, slides: sliding, travel, copy, timing });
        }
        out
    }

    /// How things move, from their keys: moves at one constant speed that
    /// stop dead, positions that shake back and forth frame to frame, and
    /// changes faked rather than made (a run of near-identical layers
    /// swapped one after another, two pictures cross-faded as a "morph").
    fn review_motion(&self) -> Vec<Note> {
        let p = &self.doc.project;
        let model = &self.doc.model;
        let h = p.height.max(1) as f64;
        let frame = (SEC as f64 / p.fps.max(1.0) as f64) as view::Time;
        let mc_of = |id: u32| model.sequences.iter().find_map(|q| q.clip(m::ClipId(id as u64)));
        let mut out = Vec::new();
        let visual: Vec<&view::Clip> = p.clips.iter().filter(|c| c.kind != ClipKind::Audio && p.lane(c.lane).is_some_and(|l| !l.hidden)).collect();
        // Stiff: big moves on straight lines in time.
        let mut stiff: Vec<(u32, String, f64, Vec<String>)> = Vec::new();
        for c in &visual {
            let Some(mc) = mc_of(c.id) else { continue };
            let mut found = Vec::new();
            for (path, tr) in mc.keyframes.iter().filter(|(k, _)| matches!(k.as_str(), "transform.position" | "transform.rotation" | "transform.scale_x" | "transform.scale_y" | "transform.scale")) {
                for w in tr.keys.windows(2) {
                    if w[0].interp != m::Interp::Linear || w[1].time - w[0].time < SEC / 3 {
                        continue;
                    }
                    let big = match (&w[0].value, &w[1].value) {
                        (m::Value::Point(a), m::Value::Point(b)) => (b.x - a.x).hypot(b.y - a.y) > 0.05 * h,
                        (m::Value::Angle(a), m::Value::Angle(b)) => (b - a).abs() > 10.0,
                        (m::Value::Scalar(a), m::Value::Scalar(b)) => (b - a).abs() > if path.contains("scale") { 0.1 } else { 10.0 },
                        _ => false,
                    };
                    if big {
                        found.push(format!("{} {:.1}–{:.1}s", path.trim_start_matches("transform."), secs(c.start + w[0].time), secs(c.start + w[1].time)));
                    }
                }
            }
            if !found.is_empty() {
                stiff.push((c.id, c.name.clone(), secs(c.start), found));
            }
        }
        for (id, name, at, found) in stiff.iter().take(6) {
            out.push(Note { kind: "stiff_motion", at: *at, to: *at, clips: vec![*id], note: format!("clip {id} (\"{name}\") moves at one constant speed and stops dead ({}{}): mechanical, not alive", found.iter().take(3).cloned().collect::<Vec<_>>().join(", "), if found.len() > 3 { ", …" } else { "" }), fix: "ease every move in and out (keys curve ease, or animate with ease), let it overshoot and settle a little, and let parts follow through after the body stops".into() });
        }
        if stiff.len() > 6 {
            out.push(Note { kind: "stiff_motion", at: f64::INFINITY, to: 0.0, clips: stiff[6..].iter().map(|s| s.0).collect(), note: format!("{} more layers move the same stiff way", stiff.len() - 6), fix: "the same".into() });
        }
        // Jitter: the position turns back within a few frames, over and over.
        let mut shaky = 0;
        for c in &visual {
            let Some(mc) = mc_of(c.id) else { continue };
            let Some(tr) = mc.keyframes.get("transform.position").filter(|t| t.keys.len() >= 4 && t.modifiers.is_empty()) else { continue };
            let pos = |t: view::Time| match tr.eval(t) {
                Some(m::Value::Point(v)) => Some(v),
                _ => None,
            };
            let n = ((c.dur / frame.max(1)) as usize).min(900);
            let pts: Vec<m::Vec2> = (0..n).filter_map(|i| pos(i as view::Time * frame)).collect();
            let mut turns: Vec<usize> = Vec::new();
            for i in 2..pts.len() {
                let (a, b) = ((pts[i - 1].x - pts[i - 2].x, pts[i - 1].y - pts[i - 2].y), (pts[i].x - pts[i - 1].x, pts[i].y - pts[i - 1].y));
                if a.0.hypot(a.1) > 0.5 && b.0.hypot(b.1) > 0.5 && a.0 * b.0 + a.1 * b.1 < 0.0 {
                    turns.push(i);
                }
            }
            let quick = turns.windows(2).filter(|w| w[1] - w[0] <= 3).count();
            if quick as f64 * secs(frame) >= 0.5 {
                shaky += 1;
                if shaky <= 6 {
                    let first = turns.windows(2).find(|w| w[1] - w[0] <= 3).map_or(0, |w| w[0]);
                    let at = secs(c.start + first as view::Time * frame);
                    out.push(Note { kind: "jitter", at, to: at, clips: vec![c.id], note: format!("clip {} (\"{}\") shakes: its position turns back {quick} times within three frames (from {at:.1}s); it reads as a glitch, not as life", c.id, c.name), fix: "thin the keys to the poses that matter and ease between them; for a deliberate tremble use a noise modifier with a low amount instead of hand-set keys".into() });
                }
            }
        }
        // Faked changes: a run of near-identical layers swapped quickly.
        let stem = |s: &str| s.to_lowercase().chars().filter(|ch| ch.is_alphabetic() || *ch == ' ').collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ");
        let mut quick: Vec<&view::Clip> = visual.iter().copied().filter(|c| c.dur <= SEC * 3 / 5).collect();
        quick.sort_by_key(|c| c.start);
        let mut by: BTreeMap<String, Vec<&view::Clip>> = BTreeMap::new();
        for c in quick {
            by.entry(stem(&c.name)).or_default().push(c);
        }
        for (name, cs) in &by {
            let mut run: Vec<&view::Clip> = Vec::new();
            let flush = |run: &mut Vec<&view::Clip>, out: &mut Vec<Note>| {
                if run.len() >= 5 {
                    let ids: Vec<u32> = run.iter().map(|c| c.id).collect();
                    let (s, e) = (run[0].start, run[run.len() - 1].end());
                    out.push(Note { kind: "faked_change", at: secs(s), to: secs(e), clips: ids.clone(), note: format!("{} layers named like \"{name}\" (\"{}\" … \"{}\") are shown one after another, each for {:.2}s or less, from {:.1}s to {:.1}s: the change is faked by swapping pictures or models, not one thing changing", run.len(), run[0].name, run[run.len() - 1].name, run.iter().map(|c| secs(c.dur)).fold(0.0, f64::max), secs(s), secs(e)), fix: "make it one layer that really changes (model morph or blended parts, mesh or puppet keys, a real transition); if the swap stays, say so plainly in your report (\"faked with N layers\"), never call it a morph".into() });
                }
                run.clear();
            };
            for c in cs {
                if run.last().is_some_and(|l| c.start - l.end() > 2 * frame) {
                    flush(&mut run, &mut out);
                }
                run.push(c);
            }
            flush(&mut run, &mut out);
        }
        // Faked morphs: one picture faded out while another fades in over it.
        let fades = |c: &view::Clip, rising: bool| -> bool {
            mc_of(c.id).and_then(|mc| mc.keyframes.get("transform.opacity")).is_some_and(|tr| {
                let (Some(a), Some(b)) = (tr.keys.first(), tr.keys.last()) else { return false };
                let (m::Value::Scalar(x), m::Value::Scalar(y)) = (&a.value, &b.value) else { return false };
                if rising { *x < 0.05 && *y > 0.9 } else { *x > 0.9 && *y < 0.05 }
            })
        };
        let mut morphs = 0;
        for a in visual.iter().filter(|c| fades(c, false)) {
            for b in visual.iter().filter(|c| c.id != a.id && fades(c, true)) {
                let overlap = a.end().min(b.end()) - a.start.max(b.start);
                if overlap > frame && overlap <= 3 * SEC && b.start >= a.start && stem(&a.name).split(' ').next() == stem(&b.name).split(' ').next() && morphs < 4 {
                    morphs += 1;
                    out.push(Note { kind: "faked_change", at: secs(b.start), to: secs(a.end()), clips: vec![a.id, b.id], note: format!("clip {} (\"{}\") fades out while clip {} (\"{}\") fades in over it ({:.1}–{:.1}s): a cross-fade standing in for a change of shape", a.id, a.name, b.id, b.name, secs(b.start), secs(a.end())), fix: "if it should transform, transform one layer (morph, puppet, mesh keys); if the cross-fade stays, call it a cross-fade in your report".into() });
                }
            }
        }
        out
    }

    /// Sentences whose things are not on screen while they are said:
    /// (from, to, the things, what is on screen, where they are shown).
    /// Also how many things were said and how many were shown.
    #[allow(clippy::type_complexity)]
    fn review_words(&self, words: &[super::voice::Said]) -> (usize, usize, Vec<(f64, f64, Vec<String>, Vec<String>, Vec<String>)>) {
        let p = &self.doc.project;
        if words.is_empty() {
            return (0, 0, Vec::new());
        }
        let plain: HashSet<&str> = PLAIN.iter().copied().collect();
        let mut things: HashSet<String> = THINGS.iter().map(|w| w.to_string()).collect();
        // Names of characters, and what each picture is: the last word of its
        // name or its file's ("Silent creek", "The beaver dam"). A character's
        // shot title ("Pip | A promise") names a beat, not a thing.
        let mut cast: HashSet<String> = HashSet::new();
        for c in p.clips.iter().filter(|c| c.kind != ClipKind::Audio) {
            if let Some(who) = c.name.split('|').next().filter(|_| c.name.contains('|')) {
                cast.extend(words_of(who));
            }
            let file = c.media.and_then(|m| p.media(m)).map(|mi| mi.name.rsplit_once('.').map_or(mi.name.clone(), |(n, _)| n.to_owned()));
            let names = [(!c.name.contains('|')).then(|| c.name.clone()), file];
            for n in names.iter().flatten() {
                if let Some(w) = words_of(n).last().filter(|w| w.len() >= 4 && !plain.contains(w.as_str()) && w.chars().all(char::is_alphabetic)) {
                    things.insert(w);
                }
            }
        }
        things.extend(cast.iter().cloned());
        // "Molly the rabbit", "a turtle named Tumble": names and what they are.
        let text: String = words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ") + " " + &self.notes_text().unwrap_or_default();
        let toks: Vec<&str> = text.split_whitespace().collect();
        let mut alias: HashMap<String, HashSet<String>> = HashMap::new();
        for i in 0..toks.len() {
            let a = norm(toks[i]);
            if !cast.contains(&a) {
                continue;
            }
            for j in i + 1..(i + 5).min(toks.len()) {
                let b = norm(toks[j]);
                if THINGS.contains(&b.as_str()) {
                    alias.entry(a.clone()).or_default().insert(b.clone());
                    alias.entry(b).or_default().insert(a.clone());
                    break;
                }
                if !["the", "a", "an", "little", "old", "young", "wise", "small", "big", "tiny", "kind"].contains(&b.as_str()) {
                    break;
                }
            }
        }
        for i in 0..toks.len().saturating_sub(2) {
            let (a, mid, b) = (norm(toks[i]), norm(toks[i + 1]), norm(toks[i + 2]));
            if (mid == "named" || mid == "called") && THINGS.contains(&a.as_str()) && cast.contains(&b) {
                alias.entry(a.clone()).or_default().insert(b.clone());
                alias.entry(b).or_default().insert(a);
            }
        }
        let wide = SEC * 3 / 2;
        let on_screen = |t: view::Time| -> (HashSet<String>, Vec<String>) {
            let mut v = HashSet::new();
            let mut names = Vec::new();
            for c in self.shown_at(t).into_iter().chain(self.shown_at(t - wide)).chain(self.shown_at(t + wide)) {
                let words = self.vocabulary(c);
                for w in &words {
                    if let Some(al) = alias.get(w) {
                        v.extend(al.iter().cloned());
                    }
                }
                v.extend(words);
                if c.start <= t && t < c.end() && !names.contains(&c.name) && names.len() < 6 {
                    names.push(c.name.clone());
                }
            }
            (v, names)
        };
        // Where each thing is shown at all, a few seconds at a time.
        let mut shown_where: HashMap<String, Vec<f64>> = HashMap::new();
        let mut t = 0;
        while t < p.duration() {
            for w in on_screen(t).0 {
                let e = shown_where.entry(w).or_default();
                if e.len() < 3 && e.last().is_none_or(|l| secs(t) - l > 20.0) {
                    e.push(secs(t));
                }
            }
            t += 2 * SEC;
        }
        // Sentences: split on full stops and long pauses.
        let mut sentences: Vec<&[super::voice::Said]> = Vec::new();
        let mut from = 0;
        for i in 0..words.len() {
            let last = i + 1 == words.len() || words[i].text.ends_with(['.', '!', '?']) || words[i + 1].start - words[i].end > 0.9;
            if last {
                sentences.push(&words[from..=i]);
                from = i + 1;
            }
        }
        let (mut said, mut shown) = (0, 0);
        let mut out = Vec::new();
        for s in sentences {
            let mut missing: Vec<String> = Vec::new();
            let mut on: Vec<String> = Vec::new();
            let mut elsewhere: Vec<String> = Vec::new();
            for w in s {
                let n = norm(&w.text);
                if n.len() < 3 || !things.contains(&n) || plain.contains(n.as_str()) {
                    continue;
                }
                said += 1;
                let (vis, names) = on_screen(super::micros((w.start + w.end) / 2.0));
                if vis.contains(&n) {
                    shown += 1;
                    continue;
                }
                if !missing.contains(&n) {
                    if let Some(ts) = shown_where.get(&n) {
                        elsewhere.push(format!("\"{n}\" is shown at {}", ts.iter().map(|t| format!("{t:.0}s")).collect::<Vec<_>>().join(", ")));
                    }
                    missing.push(n);
                }
                if on.is_empty() {
                    on = names;
                }
            }
            if !missing.is_empty() {
                out.push((s[0].start, s[s.len() - 1].end, missing, on, elsewhere));
            }
        }
        (said, shown, out)
    }

    /// The cut sampled as a contact sheet (a small frame every second or
    /// two): long spans where nothing changes, and frames far apart that
    /// look the same.
    #[cfg(feature = "render")]
    fn review_frames(&mut self, end: view::Time) -> Vec<Note> {
        const W: usize = 16;
        const H: usize = 9;
        let mut out = Vec::new();
        if end < 10 * SEC {
            return out;
        }
        let step = (end / 300).max(SEC);
        let times: Vec<view::Time> = (0..).map(|i| i * step + step / 2).take_while(|t| *t < end).collect();
        if std::env::var_os("STUDIO_NO_PREFETCH").is_none() {
            let mut jobs: Vec<(view::Time, u32, Vec<u32>)> = Vec::new();
            for &t in &times {
                let key = self.scene_key_pub(t, t, "review64");
                if !self.memo_pub().contains_key(&key) {
                    jobs.push((t, 64, Vec::new()));
                }
            }
            if !jobs.is_empty() {
                self.frames.prefetch(&self.doc, jobs);
            }
        }
        // Each sample as a 16×9 grid of light and colour.
        let mut grids: Vec<Option<Vec<f64>>> = Vec::new();
        for &t in &times {
            let key = self.scene_key_pub(t, t, "review64");
            if let Some(v) = self.memo_pub().get(&key).and_then(Value::as_array) {
                grids.push(Some(v.iter().filter_map(Value::as_f64).collect()));
                continue;
            }
            let Ok(img) = self.frames.rgba(&self.doc, t, 64) else {
                grids.push(None);
                continue;
            };
            let (w, h) = (img.width as usize, img.height as usize);
            let mut g = vec![0.0f64; W * H * 3];
            let mut n = vec![0usize; W * H];
            for y in 0..h {
                for x in 0..w {
                    let cell = (y * H / h.max(1)).min(H - 1) * W + (x * W / w.max(1)).min(W - 1);
                    let q = &img.pixels[(y * w + x) * 4..(y * w + x) * 4 + 3];
                    for k in 0..3 {
                        g[cell * 3 + k] += q[k] as f64;
                    }
                    n[cell] += 1;
                }
            }
            for (i, c) in n.iter().enumerate() {
                for k in 0..3 {
                    g[i * 3 + k] = (g[i * 3 + k] / (*c).max(1) as f64).round();
                }
            }
            self.memo_pub().insert(key, json!(g));
            grids.push(Some(g));
        }
        self.frames.clear_prefetch();
        self.memo_save_pub();
        let dist = |a: &[f64], b: &[f64]| a.iter().zip(b).map(|(x, y)| (x - y).abs()).sum::<f64>() / a.len().max(1) as f64;
        let p = self.doc.project.clone();
        let on = |t: view::Time| -> Vec<u32> { p.clips.iter().filter(|c| c.kind == ClipKind::Video && c.start <= t && t < c.end()).map(|c| c.id).collect() };
        // Nothing changes: neighbouring samples all but identical.
        let mut still: Vec<(usize, usize)> = Vec::new();
        for i in 1..times.len() {
            let (Some(a), Some(b)) = (&grids[i - 1], &grids[i]) else { continue };
            if dist(a, b) < 1.5 {
                match still.last_mut() {
                    Some(r) if r.1 == i - 1 => r.1 = i,
                    _ => still.push((i - 1, i)),
                }
            }
        }
        let mut long: Vec<(usize, usize)> = still.into_iter().filter(|(a, b)| times[*b] - times[*a] >= 10 * SEC).collect();
        long.sort_by_key(|(a, b)| std::cmp::Reverse(times[*b] - times[*a]));
        for (a, b) in long.iter().take(6) {
            let (t0, t1) = (times[*a], times[*b]);
            out.push(Note { kind: "still_picture", at: secs(t0), to: secs(t1), clips: on((t0 + t1) / 2), note: format!("from {:.0}s to {:.0}s ({:.0}s) the picture barely changes from one second to the next: a viewer sees a still", secs(t0), secs(t1), secs(t1 - t0)), fix: "something must happen in the picture: a character acts or crosses, the framing pushes in or cuts closer, the light shifts, the place moves (water, leaves, weather)".into() });
        }
        // Frames far apart that look alike: the film repeats itself.
        let n = times.len();
        let mut group: Vec<usize> = (0..n).collect();
        fn root(g: &mut [usize], i: usize) -> usize {
            let mut i = i;
            while g[i] != i {
                g[i] = g[g[i]];
                i = g[i];
            }
            i
        }
        for i in 0..n {
            let Some(a) = &grids[i] else { continue };
            for j in i + 1..n {
                if times[j] - times[i] < 30 * SEC {
                    continue;
                }
                let Some(b) = &grids[j] else { continue };
                if dist(a, b) < 4.0 {
                    let (ri, rj) = (root(&mut group, i), root(&mut group, j));
                    group[ri.max(rj)] = ri.min(rj);
                }
            }
        }
        let mut sets: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for i in 0..n {
            let r = root(&mut group, i);
            sets.entry(r).or_default().push(i);
        }
        let mut alike: Vec<Vec<usize>> = sets.into_values().filter(|s| s.len() >= 2 && times[s[s.len() - 1]] - times[s[0]] >= 30 * SEC).collect();
        alike.sort_by_key(|s| std::cmp::Reverse(s.len()));
        for s in alike.iter().take(5) {
            // Spans of neighbouring samples, read as moments.
            let mut spans: Vec<(view::Time, view::Time)> = Vec::new();
            for &i in s {
                match spans.last_mut() {
                    Some(r) if times[i] - r.1 <= step * 2 => r.1 = times[i],
                    _ => spans.push((times[i], times[i])),
                }
            }
            if spans.len() < 2 {
                continue;
            }
            let total = s.len() as f64 * secs(step);
            let list: Vec<String> = spans.iter().take(8).map(|(a, b)| if b > a { format!("{:.0}–{:.0}s", secs(*a), secs(*b)) } else { format!("{:.0}s", secs(*a)) }).collect();
            let mut cs: Vec<u32> = spans.iter().flat_map(|(a, b)| on((a + b) / 2)).collect();
            cs.sort();
            cs.dedup();
            out.push(Note { kind: "looks_same", at: secs(spans[0].0), to: secs(spans[spans.len() - 1].1), clips: cs, note: format!("about {total:.0}s of the film looks nearly the same at {} moments: {}{}", spans.len(), list.join(", "), if spans.len() > 8 { ", …" } else { "" }), fix: "make each return look different: another view, framing, light or arrangement of characters; a contact sheet of the cut (contact_sheet every 10) shows the repeats side by side".into() });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_are_matched_as_a_viewer_would() {
        assert_eq!(norm("Turtles,"), "turtle");
        assert_eq!(norm("Pip's"), "pip");
        assert_eq!(norm("berries"), "berry");
        assert_eq!(norm("wolves"), "wolf");
        assert_eq!(norm("grass"), "grass");
        assert_eq!(norm("branches"), "branch");
        assert_eq!(words_of("Molly | scene 4").collect::<Vec<_>>(), vec!["molly", "scene"]);
    }

    #[test]
    fn a_change_faked_by_swapping_layers_is_named() {
        let mut s = Session::default();
        for i in 0..6 {
            let r = s.edit(&[json!({"op": "card", "colour": "#806040", "at": 1.0 + 0.2 * i as f64, "duration": 0.2})], "t", false).unwrap();
            let id = r["read_back"]["clips"][0]["clip"].as_u64().unwrap();
            s.edit(&[json!({"op": "set", "clip": id, "name": format!("Rooster {}", i + 1)})], "t", false).unwrap();
        }
        let notes = s.review_motion();
        let n = notes.iter().find(|n| n.kind == "faked_change").expect("the swap is found");
        assert_eq!(n.clips.len(), 6);
        assert!(n.note.contains("rooster"), "{}", n.note);
    }

    #[test]
    fn sounds_are_told_apart() {
        assert!(is_music("Music", "Carefree - Kevin MacLeod", 120 * SEC));
        assert!(!is_music("Audio 3", "Rain on the leaves", 120 * SEC));
        assert!(is_ambience("Audio 3", "Rain on the leaves", 30 * SEC));
        assert!(!is_ambience("Audio 233", "Footstep grass 000", SEC / 2));
        assert!(is_voice("Voice", ""));
    }
}
