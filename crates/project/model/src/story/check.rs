// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What a director would send back about a shot plan: coverage, variety,
//! the 180° line, eyelines, pacing, and a cast and places that hold
//! together.

use super::{LineKind, ScreenSide, ShotKind, Story};
use crate::time::SECOND;
use serde::Serialize;

/// How much a problem matters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Worth a look; sometimes meant.
    Note,
    /// Breaks the craft; fix unless it is a deliberate choice.
    Warning,
    /// The plan does not hold together (unknown names, lines no shot shows).
    Error,
}

/// One thing to fix.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Issue {
    /// How much it matters.
    pub severity: Severity,
    /// A short machine name (`uncovered_line`, `crossed_line`…).
    pub kind: &'static str,
    /// The scene it is in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scene: Option<String>,
    /// The shot it is about.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shot: Option<String>,
    /// What is wrong, in words.
    pub problem: String,
    /// What to do.
    pub fix: String,
}

fn secs(m: i64) -> f64 {
    m as f64 / SECOND as f64
}

impl Story {
    /// Everything the plan gets wrong, worst first.
    pub fn check(&self) -> Vec<Issue> {
        let mut out = Vec::new();
        let mut push = |severity, kind, scene: Option<&str>, shot: Option<&str>, problem: String, fix: &str| {
            out.push(Issue { severity, kind, scene: scene.map(str::to_owned), shot: shot.map(str::to_owned), problem, fix: fix.to_owned() })
        };
        let known = |who: &str| who.eq_ignore_ascii_case("narrator") || self.character(who).is_some();

        // The cast and the places.
        for c in &self.cast {
            if c.look.trim().is_empty() && c.references.is_empty() && c.cast_item.is_none() {
                push(Severity::Warning, "character_has_no_look", None, None, format!("{} has no look: no description, reference picture or cast item", c.id), "give them a look (words precise enough to draw again) and reference pictures, so every shot shows the same character");
            }
        }
        let speakers: Vec<&str> = self.scenes.iter().flat_map(|s| s.lines.iter()).filter(|l| l.kind == LineKind::Dialogue).filter_map(|l| l.who.as_deref()).collect();
        for c in &self.cast {
            if c.voice.is_none() && speakers.iter().any(|w| w.eq_ignore_ascii_case(&c.id)) {
                push(Severity::Warning, "speaker_has_no_voice", None, None, format!("{} speaks but has no voice", c.id), "set their voice (a voice id from voices) and use it for every line they say");
            }
        }
        for p in &self.places {
            if p.backdrop.is_none() {
                push(Severity::Note, "place_has_no_backdrop", None, None, format!("place {} has no backdrop", p.id), "name the backdrop picture (or the words to make it from) so every scene there looks like the same place");
            }
            if p.ambience.is_none() {
                push(Severity::Note, "place_has_no_ambience", None, None, format!("place {} has no ambience", p.id), "give it a bed (stream, forest, birds, wind, night, rain, waves, fire…) to lay under its scenes");
            }
        }

        // The times the writer gave.
        for s in &self.scenes {
            let mut prev: Option<(usize, i64, i64)> = None; // line, start, end
            for (i, l) in s.lines.iter().enumerate() {
                let Some(a) = l.start else { continue };
                let end = a + (l.secs() * SECOND as f64) as i64;
                if let Some((pi, pa, pe)) = prev {
                    if a < pa {
                        push(Severity::Warning, "times_out_of_order", Some(&s.id), None, format!("scene {} line {} starts at {:.1}s, before line {} ({:.1}s)", s.id, i + 1, secs(a), pi + 1, secs(pa)), "put the lines in the order they play, or fix the times");
                    } else if a < pe && l.is_spoken() && s.lines[pi].is_spoken() {
                        push(Severity::Note, "spoken_lines_overlap", Some(&s.id), None, format!("scene {} lines {} and {} are both spoken at {:.1}s", s.id, pi + 1, i + 1, secs(a)), "give the first a shorter length, or move the second later; or keep it if they talk over each other");
                    }
                }
                if l.is_spoken() && l.duration.is_some() {
                    let want = l.text.split_whitespace().count() as f64 / super::times::SPOKEN_WPS;
                    if secs(l.duration.unwrap_or(0)) < want * 0.6 && want > 1.0 {
                        push(Severity::Note, "line_given_too_little_time", Some(&s.id), None, format!("scene {} line {} has {:.1}s for {} words (about {:.1}s at an easy pace)", s.id, i + 1, secs(l.duration.unwrap_or(0)), l.text.split_whitespace().count(), want), "give it more time, or cut words");
                    }
                }
                prev = Some((i, a, end));
            }
        }

        let all: Vec<_> = self.scenes.iter().flat_map(|s| s.shots.iter().map(move |sh| (s, sh))).collect();
        for (si, s) in self.scenes.iter().enumerate() {
            let sid = Some(s.id.as_str());
            match s.place.as_deref() {
                None => push(Severity::Warning, "scene_has_no_place", sid, None, format!("scene {} has no place", s.id), "set its place, with its backdrop, light, time of day and ambience"),
                Some(p) if self.place(p).is_none() => push(Severity::Error, "unknown_place", sid, None, format!("scene {} is set in \"{p}\", which is not a place", s.id), "add the place or correct the name"),
                _ => {}
            }
            for who in &s.cast {
                if !known(who) {
                    push(Severity::Error, "unknown_character", sid, None, format!("scene {} has \"{who}\" in its cast, who is not in the bible", s.id), "add them to the cast with their look, voice, size and moves");
                }
            }
            for (i, l) in s.lines.iter().enumerate() {
                if let Some(w) = &l.who {
                    if l.kind == LineKind::Dialogue && !known(w) {
                        push(Severity::Error, "unknown_speaker", sid, None, format!("line {} of scene {} is said by \"{w}\", who is not in the cast", i + 1, s.id), "add them to the cast or correct the name");
                    }
                }
            }
            if s.shots.is_empty() {
                if !s.lines.is_empty() || !s.summary.is_empty() {
                    push(Severity::Error, "scene_has_no_shots", sid, None, format!("scene {} has no shots", s.id), "plan its coverage: an establishing or wide shot, then closer shots on who matters, reactions, inserts");
                }
                continue;
            }
            // Coverage: every line is seen or heard over some shot.
            for (i, l) in s.lines.iter().enumerate() {
                if !s.shots.iter().any(|sh| sh.lines.contains(&i)) {
                    let what = match l.kind {
                        LineKind::Narration => "narration",
                        LineKind::Dialogue => "dialogue",
                        LineKind::Action => "action",
                    };
                    let excerpt: String = l.text.chars().take(60).collect();
                    push(Severity::Error, "uncovered_line", sid, None, format!("scene {} line {} ({what}: \"{excerpt}\") is in no shot", s.id, i + 1), "give it to the shot that shows what it says: narration needs a picture of what it describes, dialogue a shot of who speaks or who hears it");
                }
            }
            for sh in &s.shots {
                for l in &sh.lines {
                    if *l >= s.lines.len() {
                        push(Severity::Error, "no_such_line", sid, Some(&sh.id), format!("shot {} covers line {} but scene {} has {} lines", sh.id, l + 1, s.id, s.lines.len()), "number lines from 1 in the scene");
                    }
                }
                for who in &sh.subjects {
                    if self.character(who).is_some() && !s.cast.is_empty() && !s.cast.iter().any(|c| self.character(c).map(|c| &c.id) == self.character(who).map(|c| &c.id)) {
                        push(Severity::Warning, "subject_not_in_scene", sid, Some(&sh.id), format!("shot {} shows {who}, who is not in scene {}'s cast", sh.id, s.id), "add them to the scene's cast or frame someone who is there");
                    }
                }
                let need = Story::spoken_secs(s, sh);
                if need > 0.0 && secs(sh.duration) + 0.05 < need {
                    push(Severity::Warning, "shot_too_short_for_its_lines", sid, Some(&sh.id), format!("shot {} lasts {:.1}s but its lines take about {need:.1}s to say", sh.id, secs(sh.duration)), "lengthen it, or give some of its lines to the next shot (a reaction or an insert)");
                }
                if sh.duration < SECOND * 6 / 10 {
                    push(Severity::Warning, "flash_shot", sid, Some(&sh.id), format!("shot {} is only {:.2}s", sh.id, secs(sh.duration)), "under about 0.6 s nothing reads: hold it longer or cut it");
                }
                let long = match sh.kind {
                    ShotKind::Establishing | ShotKind::Wide => 14.0,
                    _ => 10.0,
                };
                if secs(sh.duration) > long && sh.camera == super::CameraMove::Static {
                    push(Severity::Note, "long_static_shot", sid, Some(&sh.id), format!("shot {} holds {:.1}s without moving", sh.id, secs(sh.duration)), "cut to a closer shot or a reaction, or give it a slow push in");
                }
            }
            // Open on where we are, unless the scene goes on from the last
            // in the same place.
            let first = s.shots[0].kind;
            let same_place = si > 0 && self.scenes[si - 1].place.is_some() && self.scenes[si - 1].place == s.place;
            if !matches!(first, ShotKind::Establishing | ShotKind::Wide) && !same_place && s.shots.len() > 1 {
                push(Severity::Warning, "no_establishing_shot", sid, Some(&s.shots[0].id), format!("scene {} opens on a {} without showing where we are", s.id, first.name().replace('_', " ")), "open on an establishing or wide shot of the place (or move in to the close one later)");
            }
            for w in s.shots.windows(2) {
                let (a, b) = (&w[0], &w[1]);
                // Same size on the same subject back to back: a jump cut.
                if a.kind == b.kind && a.subjects == b.subjects && !a.subjects.is_empty() && a.kind != ShotKind::Insert {
                    push(Severity::Warning, "jump_cut", sid, Some(&b.id), format!("shots {} and {} are both {} on {}", a.id, b.id, a.kind.name().replace('_', " "), a.subjects.join(" and ")), "change the size by at least a step (wide to medium, medium to close) or the angle by 30° or more, or cut away between them");
                }
                // The 180° line.
                if let (Some(x), Some(y)) = (&a.side, &b.side) {
                    if !x.eq_ignore_ascii_case(y) && !a.crosses && !matches!(b.kind, ShotKind::Insert | ShotKind::Pov) {
                        push(Severity::Warning, "crossed_line", sid, Some(&b.id), format!("shot {} jumps to side {y} of the line from {}'s side {x}: left and right swap for the audience", b.id, a.id), "stay on one side, or cross on screen (a moving shot marked crosses), or cut through a neutral shot (straight down the line, an insert)");
                    }
                }
                // Eyelines: two singles of different characters talking to
                // each other look opposite ways.
                let single = |k: ShotKind| matches!(k, ShotKind::CloseUp | ShotKind::Medium | ShotKind::Reaction | ShotKind::OverTheShoulder | ShotKind::ExtremeCloseUp);
                if single(a.kind) && single(b.kind) && a.subjects.first() != b.subjects.first() && a.subjects.first().is_some() && b.subjects.first().is_some() {
                    if let (Some(x), Some(y)) = (a.looks, b.looks) {
                        if x == y && x != ScreenSide::Camera {
                            push(Severity::Warning, "eyelines_miss", sid, Some(&b.id), format!("{} in {} and {} in {} both look {}: they seem to look past each other", a.subjects[0], a.id, b.subjects[0], b.id, x.name()), "in a conversation one looks frame left, the other frame right (mirror the singles)");
                        }
                    }
                }
            }
            // Variety: three or more of one size in a row.
            let mut run = 1;
            for i in 1..s.shots.len() {
                run = if s.shots[i].kind.tightness() == s.shots[i - 1].kind.tightness() { run + 1 } else { 1 };
                if run == 3 {
                    push(Severity::Note, "same_size_run", sid, Some(&s.shots[i].id), format!("shots up to {} are all {} in a row", s.shots[i].id, s.shots[i].kind.name().replace('_', " ")), "vary the sizes: wide for where, medium for what, close for how it feels");
                }
            }
            let talk = s.lines.iter().filter(|l| l.kind == LineKind::Dialogue).count();
            if talk >= 2 && !s.shots.iter().any(|sh| matches!(sh.kind, ShotKind::Reaction | ShotKind::OverTheShoulder | ShotKind::CloseUp)) {
                push(Severity::Warning, "dialogue_without_coverage", sid, None, format!("scene {} has {talk} lines of dialogue and no close-ups, reactions or over-the-shoulders", s.id), "cover the exchange: a two shot, then singles (or OTS) of each, and a reaction where a line lands");
            }
        }

        // Pacing across the film.
        if all.len() >= 6 {
            let d: Vec<f64> = all.iter().map(|(_, sh)| secs(sh.duration)).collect();
            let mean = d.iter().sum::<f64>() / d.len() as f64;
            let sd = (d.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / d.len() as f64).sqrt();
            if mean > 0.0 && sd / mean < 0.12 {
                push(Severity::Warning, "monotonous_pacing", None, None, format!("{} shots are all about {mean:.1}s long: the film is cut into even chunks, not paced", d.len()), "cut on the story: hold the wide shots and the moments that land, quicken through action, let a reaction run");
            }
            let kinds: std::collections::HashSet<ShotKind> = all.iter().map(|(_, sh)| sh.kind).collect();
            if kinds.len() <= 2 {
                push(Severity::Warning, "little_shot_variety", None, None, format!("{} shots use only {} shot sizes", all.len(), kinds.len()), "plan coverage per scene: establishing, wide, medium, close-up, reaction, insert, POV where it helps");
            }
        }
        out.sort_by(|a, b| b.severity.cmp(&a.severity));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;

    fn line(kind: LineKind, who: Option<&str>, text: &str) -> Line {
        Line { kind, who: who.map(str::to_owned), text: text.into(), how: None, ..Default::default() }
    }

    fn shot(id: &str, kind: ShotKind, subjects: &[&str], secs: f64, lines: &[usize]) -> Shot {
        Shot { id: id.into(), kind, subjects: subjects.iter().map(|s| s.to_string()).collect(), duration: (secs * SECOND as f64) as i64, lines: lines.to_vec(), ..Default::default() }
    }

    fn base() -> Story {
        let mut st = Story::default();
        st.places.push(Place { id: "meadow".into(), backdrop: Some("meadow.png".into()), ambience: Some("birds".into()), ..Default::default() });
        st.cast.push(Character { id: "fox".into(), look: "red fox, white tail tip".into(), voice: Some("v1".into()), size: 1.0, ..Default::default() });
        st.cast.push(Character { id: "owl".into(), look: "small grey owl".into(), voice: Some("v2".into()), size: 0.6, ..Default::default() });
        st
    }

    fn kinds(st: &Story) -> Vec<&'static str> {
        st.check().iter().map(|i| i.kind).collect()
    }

    #[test]
    fn a_well_covered_scene_passes() {
        let mut st = base();
        st.scenes.push(Scene {
            id: "1".into(),
            place: Some("meadow".into()),
            cast: vec!["fox".into(), "owl".into()],
            lines: vec![line(LineKind::Narration, Some("narrator"), "Morning came to the meadow."), line(LineKind::Dialogue, Some("fox"), "Who is there?"), line(LineKind::Dialogue, Some("owl"), "Only me.")],
            shots: vec![
                Shot { looks: None, ..shot("1A", ShotKind::Establishing, &[], 4.0, &[0]) },
                Shot { side: Some("a".into()), looks: Some(ScreenSide::Right), ..shot("1B", ShotKind::CloseUp, &["fox"], 2.0, &[1]) },
                Shot { side: Some("a".into()), looks: Some(ScreenSide::Left), ..shot("1C", ShotKind::Reaction, &["owl"], 2.0, &[2]) },
            ],
            ..Default::default()
        });
        assert_eq!(kinds(&st), Vec::<&str>::new());
    }

    #[test]
    fn the_usual_mistakes_are_found() {
        let mut st = base();
        st.cast[1].voice = None;
        st.scenes.push(Scene {
            id: "1".into(),
            place: Some("forest".into()),
            cast: vec!["fox".into(), "owl".into(), "bear".into()],
            lines: vec![
                line(LineKind::Narration, Some("narrator"), "The fox ran and ran through the long grass until the sun was high."),
                line(LineKind::Dialogue, Some("fox"), "Who?"),
                line(LineKind::Dialogue, Some("owl"), "Me."),
                line(LineKind::Narration, Some("narrator"), "Nobody saw this."),
            ],
            shots: vec![
                Shot { side: Some("a".into()), looks: Some(ScreenSide::Right), ..shot("1A", ShotKind::Medium, &["fox"], 2.0, &[0]) },
                Shot { side: Some("b".into()), looks: Some(ScreenSide::Right), ..shot("1B", ShotKind::Medium, &["owl"], 2.0, &[1, 2]) },
                shot("1C", ShotKind::Medium, &["owl"], 0.3, &[9]),
            ],
            ..Default::default()
        });
        let k = kinds(&st);
        for want in ["unknown_place", "unknown_character", "uncovered_line", "no_such_line", "shot_too_short_for_its_lines", "flash_shot", "no_establishing_shot", "jump_cut", "crossed_line", "eyelines_miss", "same_size_run", "dialogue_without_coverage", "speaker_has_no_voice"] {
            assert!(k.contains(&want), "{want} missing from {k:?}");
        }
        // Worst first.
        assert_eq!(st.check()[0].severity, Severity::Error);
    }

    #[test]
    fn even_chunks_are_called_out() {
        let mut st = base();
        for n in 0..8 {
            st.scenes.push(Scene { id: n.to_string(), place: Some("meadow".into()), summary: "x".into(), shots: vec![shot(&format!("{n}A"), ShotKind::Wide, &[], 20.0, &[])], ..Default::default() });
        }
        let k = kinds(&st);
        assert!(k.contains(&"monotonous_pacing"), "{k:?}");
        assert!(k.contains(&"little_shot_variety"), "{k:?}");
        assert!(k.contains(&"long_static_shot"), "{k:?}");
    }

    #[test]
    fn times_that_run_backwards_or_leave_no_room_are_flagged() {
        let mut st = Story::default();
        let l = |text: &str, start: i64, dur: Option<i64>| Line { kind: LineKind::Narration, who: Some("narrator".into()), text: text.into(), start: Some(start * SECOND), duration: dur.map(|d| d * SECOND), ..Default::default() };
        st.scenes.push(Scene { id: "1".into(), lines: vec![l("one two three four five six seven eight nine ten", 10, Some(2)), l("later", 5, None), l("very early words here", 20, None), l("overlap words", 21, None)], ..Default::default() });
        let kinds: Vec<&str> = st.check().iter().map(|i| i.kind).collect();
        assert!(kinds.contains(&"times_out_of_order") && kinds.contains(&"line_given_too_little_time") && kinds.contains(&"spoken_lines_overlap"), "{kinds:?}");
    }
}
