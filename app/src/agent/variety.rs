// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Sound that does not vary: the same effect played again and again, one
//! music track under a film whose brief changes mood, one kind of sound
//! from end to end. `check` and `review` both ask for these findings.

use super::inventory;
use super::review::is_music;
use super::Session;
use crate::view::{ClipKind, SEC};
use std::collections::BTreeMap;

/// One thing worth changing, in the words both tools use.
pub(super) struct Finding {
    pub kind: &'static str,
    pub at: f64,
    pub to: f64,
    pub clips: Vec<u32>,
    pub note: String,
    pub fix: String,
}

/// A sound clip as the checks see it.
struct Played {
    id: u32,
    start: f64,
    end: f64,
    name: String,
    /// The file (or the name, when there is none): what makes two clips "the same sound".
    key: String,
    music: bool,
    voice: bool,
    long: bool,
}

/// The kind of a sound without measuring it: its name up to the first
/// digit, dash or space ("whoosh-0.80s" and "Whoosh 3" are both "whoosh").
fn type_of(name: &str) -> String {
    name.to_lowercase().split(|c: char| !c.is_alphabetic()).find(|w| !w.is_empty()).unwrap_or("").to_owned()
}

fn list(times: &[f64]) -> String {
    let mut v: Vec<String> = times.iter().take(5).map(|t| format!("{t:.1}s")).collect();
    if times.len() > 5 {
        v.push(format!("and {} more", times.len() - 5));
    }
    v.join(", ")
}

impl Session {
    /// The brief, in words: the notes, and the story plan if there is one.
    fn brief_text(&self) -> String {
        let mut t = self.notes_text().unwrap_or_default();
        if let Ok(s) = serde_json::to_string(&self.doc.model.story) {
            t.push(' ');
            t.push_str(&s);
        }
        t
    }

    pub(super) fn variety(&self) -> Vec<Finding> {
        let p = &self.doc.project;
        let end = p.duration() as f64 / SEC as f64;
        let mut played: Vec<Played> = Vec::new();
        for c in p.clips.iter().filter(|c| c.kind == ClipKind::Audio) {
            let Some(lane) = p.lane(c.lane) else { continue };
            if lane.muted || lane.hidden {
                continue;
            }
            let Some(mi) = c.media.and_then(|m| p.media(m)) else { continue };
            let l = lane.name.to_lowercase();
            played.push(Played {
                id: c.id,
                start: c.start as f64 / SEC as f64,
                end: c.end() as f64 / SEC as f64,
                name: mi.name.clone(),
                key: mi.path.as_ref().map_or_else(|| mi.name.clone(), |p| p.display().to_string()),
                // The default sound lane is called Music, so a lane name alone proves nothing: a track is long.
                music: is_music(&lane.name, &mi.name, mi.dur) && mi.dur >= 20 * SEC,
                voice: ["voice", "narr", "dialog", "speech", "vo"].iter().any(|w| l.contains(w)),
                long: mi.dur >= 8 * SEC,
            });
        }
        let mut out = Vec::new();
        let effects: Vec<&Played> = played.iter().filter(|s| !s.music && !s.voice && !s.long).collect();

        // The same effect again and again.
        if effects.len() >= 4 {
            let mut by: BTreeMap<&str, Vec<&Played>> = BTreeMap::new();
            for s in &effects {
                by.entry(s.key.as_str()).or_default().push(s);
            }
            for (_, group) in by.iter().filter(|(_, g)| g.len() >= 3 && (g.len() * 100 >= effects.len() * 35 || g.len() >= 5)) {
                let times: Vec<f64> = group.iter().map(|s| s.start).collect();
                out.push(Finding {
                    kind: "sound_repeated",
                    at: times.iter().cloned().fold(f64::MAX, f64::min),
                    to: group.iter().map(|s| s.end).fold(0.0, f64::max),
                    clips: group.iter().map(|s| s.id).collect(),
                    note: format!("\"{}\" plays {} times ({}), {}% of the film's {} sound effects: heard that often it turns into a tic", group[0].name, group.len(), list(&times), group.len() * 100 / effects.len(), effects.len()),
                    fix: "give each moment its own sound: catalogue {panel: audio, kind: <hit|boom|swoosh|swell>, fresh: true} lists unused ones; add {query, kind} picks a new one each time; sfx makes a fresh variant".into(),
                });
            }
        }

        // One mood in the sound while the brief has several.
        let moods = inventory::brief_moods(&self.brief_text());
        if moods.len() >= 2 && end >= 45.0 {
            let tracks: Vec<&Played> = played.iter().filter(|s| s.music).collect();
            let distinct: std::collections::BTreeSet<&str> = tracks.iter().map(|s| s.key.as_str()).collect();
            let covered: f64 = tracks.iter().map(|s| s.end.min(end) - s.start.max(0.0)).sum();
            if distinct.len() == 1 && covered >= end * 0.85 {
                out.push(Finding {
                    kind: "one_track_whole_film",
                    at: 0.0,
                    to: end,
                    clips: tracks.iter().map(|s| s.id).collect(),
                    note: format!("one music track (\"{}\") runs under the whole film, but the brief has {} moods ({}): the music never turns with the story", tracks[0].name, moods.len(), moods.join(", ")),
                    fix: "end the track where the mood turns (fade_out about a second) and start a track of the next mood there (fade_in): catalogue {panel: audio, kind: music, mood: <mood>, min_seconds: 30, fresh: true}".into(),
                });
            }
            let kinds: std::collections::BTreeSet<String> = effects.iter().map(|s| type_of(&s.name)).collect();
            if effects.len() >= 5 && kinds.len() == 1 {
                out.push(Finding {
                    kind: "one_sound_type",
                    at: effects.iter().map(|s| s.start).fold(f64::MAX, f64::min),
                    to: effects.iter().map(|s| s.end).fold(0.0, f64::max),
                    clips: effects.iter().map(|s| s.id).collect(),
                    note: format!("all {} sound effects are \"{}\"-type sounds, but the brief has {} moods ({}): the film has one kind of sound from start to end", effects.len(), kinds.iter().next().cloned().unwrap_or_default(), moods.len(), moods.join(", ")),
                    fix: "match each scene's sound to its mood: soft ambience and foley where it is calm, hits, booms and risers where it turns; catalogue {panel: audio} lists the kinds".into(),
                });
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::library::new_clip;
    use crate::view::{MediaItem, MediaKind};
    use serde_json::json;

    /// A sound of `secs` on the named lane, as the library's add leaves it.
    fn put(s: &mut Session, lane: &str, name: &str, at: f64, secs: f64) {
        let lane_id = s.doc.project.lanes.iter().find(|l| l.name == lane).map(|l| l.id).unwrap();
        let media = s.doc.alloc_id() as u32;
        let clip = s.doc.alloc_id() as u32;
        let dur = (secs * SEC as f64) as i64;
        let mi = MediaItem { id: media, name: name.into(), kind: MediaKind::Audio, dur, width: 0, height: 0, fps: 30.0, seed: media, path: Some(format!("/fixture/{name}.ogg").into()) };
        let mut c = new_clip((at * SEC as f64) as i64, dur, ClipKind::Audio, name, Some(media));
        c.lane = lane_id;
        c.id = clip;
        assert!(s.doc.edit("put", |p| {
            p.media.push(mi);
            p.clips.push(c);
        }));
    }

    fn film() -> Session {
        let mut s = Session::default();
        s.edit(&[json!({"op": "lane", "action": "add", "name": "music", "kind": "audio"}), json!({"op": "lane", "action": "add", "name": "fx", "kind": "audio"})], "lanes", false).unwrap();
        s.edit(&[json!({"op": "card", "colour": "#224466", "at": 0, "duration": 60, "lane": "main"})], "picture", false).unwrap();
        s
    }

    #[test]
    fn the_same_sound_five_times_is_flagged_and_varied_ones_are_not() {
        let mut s = film();
        for i in 0..5 {
            // On the lane called music, as the library's add leaves them.
            put(&mut s, "music", "Whoosh 1", 2.0 + 8.0 * i as f64, 0.7);
        }
        put(&mut s, "fx", "Punch 3", 20.0, 0.3);
        let f = s.variety();
        let r = f.iter().find(|f| f.kind == "sound_repeated").expect("a repeat");
        assert_eq!(r.clips.len(), 5);
        assert!(r.note.contains("Whoosh 1") && r.note.contains("5 times"), "{}", r.note);

        let mut varied = film();
        for i in 0..6 {
            put(&mut varied, "fx", &format!("Whoosh {i}"), 2.0 + 8.0 * i as f64, 0.7);
        }
        assert!(varied.variety().iter().all(|f| f.kind != "sound_repeated"));
    }

    #[test]
    fn one_track_under_a_film_with_two_moods_is_flagged() {
        let mut s = film();
        put(&mut s, "music", "Sunny Walk", 0.0, 60.0);
        assert!(s.variety().is_empty(), "no brief, no mood to miss");
        s.notes(&json!({"write": "A calm morning in the valley, then a tense chase at dusk."})).unwrap();
        let f = s.variety();
        let one = f.iter().find(|f| f.kind == "one_track_whole_film").expect("one track");
        assert!(one.note.contains("calm") && one.note.contains("dark"), "{}", one.note);
        // A second track from the middle on is the fix.
        let mut two = film();
        two.notes(&json!({"write": "calm, then tense"})).unwrap();
        put(&mut two, "music", "Still Water", 0.0, 30.0);
        put(&mut two, "music", "Night Hunt", 30.0, 30.0);
        assert!(two.variety().iter().all(|f| f.kind != "one_track_whole_film"));
    }

    #[test]
    fn one_kind_of_sound_through_a_multi_mood_film_is_flagged() {
        let mut s = film();
        s.notes(&json!({"write": "starts peaceful, ends in a scary storm"})).unwrap();
        for i in 0..5 {
            put(&mut s, "fx", &format!("Click {i}"), 3.0 + 9.0 * i as f64, 0.2);
        }
        assert!(s.variety().iter().any(|f| f.kind == "one_sound_type"));
    }
}
