// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A first shot plan for a scene from its lines: where we are first, then
//! closer shots on who the words are about, a single for each speaker with
//! mirrored eyelines on one side of the line, reactions where a line
//! lands, inserts for things, every line given to a shot and every shot as
//! long as its words. A starting point to revise, not a finished plan.

use super::{CameraMove, LineKind, ScreenSide, Scene, Shot, ShotKind, Story};
use crate::time::{Micros, SECOND};

fn us(s: f64) -> Micros {
    (s * SECOND as f64).round() as Micros
}

const MOVING: &[&str] = &["ran", "run", "runs", "running", "walked", "walk", "walks", "walking", "raced", "chased", "chase", "flew", "fly", "flying", "hopped", "hop", "swam", "swim", "rushed", "dashed", "climbed", "jumped", "leapt", "fled", "followed", "crossed", "travelled", "traveled", "wandered"];
const FEELING: &[&str] = &["afraid", "scared", "sad", "cried", "tears", "smiled", "laughed", "heart", "alone", "lonely", "happy", "angry", "shocked", "surprised", "wondered", "realised", "realized", "hoped", "loved", "missed", "worried"];

fn has_word(text: &str, words: &[&str]) -> bool {
    text.split(|c: char| !c.is_alphanumeric()).any(|w| words.iter().any(|x| w.eq_ignore_ascii_case(x)))
}

impl Story {
    /// Characters of `scene` a text speaks about, by id or any word of
    /// their name.
    fn named_in(&self, scene: &Scene, text: &str) -> Vec<String> {
        let words: Vec<String> = text.split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() > 1).map(str::to_lowercase).collect();
        let pool: Vec<&super::Character> = if scene.cast.is_empty() { self.cast.iter().collect() } else { scene.cast.iter().filter_map(|c| self.character(c)).collect() };
        pool.into_iter()
            .filter(|c| {
                let mut names: Vec<String> = c.id.split('_').map(str::to_owned).collect();
                names.extend(c.name.split_whitespace().map(|w| w.to_lowercase()).filter(|w| !matches!(w.as_str(), "the" | "a" | "an" | "of" | "and" | "little" | "old" | "young" | "mr" | "mrs" | "ms")));
                names.iter().any(|n| n.len() > 1 && words.contains(n))
            })
            .map(|c| c.id.clone())
            .collect()
    }

    /// A first coverage plan for scene `index`, replacing its shots.
    pub fn plan_scene(&mut self, index: usize) {
        let s = self.scenes[index].clone();
        let mut shots: Vec<Shot> = Vec::new();
        // A scene that goes on where the last one was opens wide, not on an
        // establishing shot again.
        let same_place = index > 0 && self.scenes[index - 1].place == s.place;
        // Who stands frame left (looks right) and frame right (looks left).
        let speakers: Vec<String> = {
            let mut v: Vec<String> = Vec::new();
            for l in s.lines.iter().filter(|l| l.kind == LineKind::Dialogue) {
                if let Some(w) = &l.who {
                    if !v.contains(w) {
                        v.push(w.clone());
                    }
                }
            }
            v
        };
        let looks_of = |who: &str| match speakers.iter().position(|w| w == who) {
            Some(0) => Some(ScreenSide::Right),
            Some(_) => Some(ScreenSide::Left),
            None => None,
        };
        let mut i = 0;
        // Opening: the place, under the first narration and action.
        {
            let mut lines = Vec::new();
            let mut need = 0.0;
            while i < s.lines.len() && s.lines[i].kind != LineKind::Dialogue && (lines.is_empty() || need + s.lines[i].spoken_secs() <= 6.5) {
                need += s.lines[i].spoken_secs();
                lines.push(i);
                i += 1;
            }
            let kind = if same_place { ShotKind::Wide } else { ShotKind::Establishing };
            let text: Vec<&str> = lines.iter().map(|k| s.lines[*k].text.as_str()).collect();
            let moving = has_word(&text.join(" "), MOVING);
            shots.push(Shot {
                kind,
                camera: if moving { CameraMove::Track } else if same_place { CameraMove::Static } else { CameraMove::PushIn },
                duration: us((need + 0.8).max(if same_place { 2.5 } else { 3.5 })),
                subjects: if moving { self.named_in(&s, &text.join(" ")) } else { Vec::new() },
                lines,
                action: if s.summary.is_empty() { String::new() } else { s.summary.clone() },
                ..Default::default()
            });
        }
        // Narration sizes cycle so no two in a row match.
        let cycle = [ShotKind::Medium, ShotKind::CloseUp, ShotKind::Wide, ShotKind::Insert, ShotKind::Medium, ShotKind::Pov, ShotKind::CloseUp, ShotKind::Wide];
        // Started apart per scene so scenes do not all cut alike.
        let mut turn = index * 3;
        let mut said = 0usize;
        while i < s.lines.len() {
            let l = &s.lines[i];
            if l.kind == LineKind::Dialogue {
                let who = l.who.clone().unwrap_or_default();
                let first_exchange = said == 0 && speakers.len() >= 2;
                let kind = if first_exchange { ShotKind::TwoShot } else if said % 3 == 2 && speakers.len() >= 2 { ShotKind::OverTheShoulder } else { ShotKind::CloseUp };
                let subjects = if first_exchange { speakers.iter().take(2).cloned().collect() } else { vec![who.clone()] };
                shots.push(Shot {
                    kind,
                    subjects,
                    duration: us((l.spoken_secs() + 0.5).max(1.5)),
                    lines: vec![i],
                    side: (speakers.len() >= 2).then(|| "a".to_owned()),
                    looks: if first_exchange { None } else { looks_of(&who) },
                    camera: if has_word(&l.text, FEELING) { CameraMove::PushIn } else { CameraMove::Static },
                    ..Default::default()
                });
                said += 1;
                // A line that lands: hold on the one who hears it.
                let lands = l.text.trim_end().ends_with('!') || l.text.trim_end().ends_with('?') || has_word(&l.text, FEELING);
                let listener = speakers.iter().find(|w| **w != who).cloned();
                let next_is_theirs = s.lines.get(i + 1).is_some_and(|n| n.kind == LineKind::Dialogue && n.who == listener);
                if let (true, Some(other), false) = (lands, listener, next_is_theirs) {
                    shots.push(Shot { kind: ShotKind::Reaction, looks: looks_of(&other), subjects: vec![other], duration: us(1.4), side: Some("a".into()), ..Default::default() });
                }
                i += 1;
                continue;
            }
            // Narration and action: up to about six seconds of words a shot.
            let mut lines = vec![i];
            let mut need = l.spoken_secs();
            i += 1;
            while i < s.lines.len() && s.lines[i].kind != LineKind::Dialogue && need + s.lines[i].spoken_secs() <= 6.0 && s.lines[i].kind == s.lines[lines[0]].kind {
                need += s.lines[i].spoken_secs();
                lines.push(i);
                i += 1;
            }
            let text: String = lines.iter().map(|k| s.lines[*k].text.as_str()).collect::<Vec<_>>().join(" ");
            let named = self.named_in(&s, &text);
            let moving = has_word(&text, MOVING);
            let feeling = has_word(&text, FEELING);
            let mut kind = if moving {
                ShotKind::Wide
            } else if feeling && !named.is_empty() {
                ShotKind::CloseUp
            } else {
                let k = cycle[turn % cycle.len()];
                turn += 1;
                k
            };
            if named.is_empty() && matches!(kind, ShotKind::CloseUp | ShotKind::Pov) && s.cast.is_empty() {
                kind = ShotKind::Insert;
            }
            // Never three of a size in a row, nor the same size twice on the
            // same subject.
            let subjects = if kind == ShotKind::Insert && named.is_empty() { Vec::new() } else if named.is_empty() { s.cast.iter().take(1).cloned().collect() } else { named };
            let n = shots.len();
            let tight = kind.tightness();
            if (n >= 2 && shots[n - 1].kind.tightness() == tight && shots[n - 2].kind.tightness() == tight) || shots.last().is_some_and(|p| p.kind == kind && p.subjects == subjects) {
                kind = match kind {
                    ShotKind::Wide | ShotKind::Establishing => ShotKind::Medium,
                    ShotKind::Medium | ShotKind::Pov | ShotKind::OverTheShoulder | ShotKind::TwoShot => ShotKind::CloseUp,
                    _ => ShotKind::Wide,
                };
            }
            let action = s.lines[lines[0]].kind == LineKind::Action;
            shots.push(Shot {
                kind,
                camera: if moving { CameraMove::Track } else if feeling { CameraMove::PushIn } else { CameraMove::Static },
                duration: us(if action && need == 0.0 { 2.5 * lines.len() as f64 } else { (need + 0.6).max(if kind == ShotKind::Insert { 1.5 } else { 2.0 }) }),
                subjects,
                lines,
                ..Default::default()
            });
        }
        if shots.len() == 1 && s.lines.is_empty() && !s.cast.is_empty() {
            shots.push(Shot { kind: ShotKind::Medium, subjects: s.cast.iter().take(2).cloned().collect(), duration: us(3.0), ..Default::default() });
        }
        self.scenes[index].shots.clear();
        for mut sh in shots {
            sh.id = self.next_shot_id(index);
            self.scenes[index].shots.push(sh);
        }
    }

    /// Plans every scene that has no shots yet (or every scene with
    /// `replace`). Returns the scenes planned.
    pub fn plan(&mut self, replace: bool) -> Vec<String> {
        let mut done = Vec::new();
        for i in 0..self.scenes.len() {
            if replace || self.scenes[i].shots.is_empty() {
                self.plan_scene(i);
                done.push(self.scenes[i].id.clone());
            }
        }
        // Where the writer timed lines, the cuts fall on those times.
        self.retime_shots();
        done
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;

    #[test]
    fn a_plan_covers_every_line_and_passes_its_own_check() {
        let text = "Title: Test\n\nEXT. RIVER BANK - DAY\n\nThe river runs fast.\n\nNARRATOR (V.O.)\nFern the fox walked down to the water every morning.\n\nNARRATOR (V.O.)\nShe was afraid of the deep part.\n\nFERN\nIs anyone there?\n\nHERON\nOnly me, little fox.\n\nFERN\nCan you help me cross?\n\nHERON\nOf course!\n\nEXT. RIVER BANK - DAY\n\nNARRATOR (V.O.)\nAnd so they crossed together.\n";
        let mut st = fountain::read(text);
        st.places[0].backdrop = Some("river.png".into());
        st.places[0].ambience = Some("stream".into());
        for c in &mut st.cast {
            c.look = format!("{} as drawn", c.id);
            c.voice = Some("v".into());
        }
        let planned = st.plan(false);
        assert_eq!(planned, vec!["1".to_owned(), "2".to_owned()]);
        let s = &st.scenes[0];
        assert_eq!(s.shots[0].kind, ShotKind::Establishing);
        for i in 0..s.lines.len() {
            assert!(s.shots.iter().any(|sh| sh.lines.contains(&i)), "line {i} uncovered: {:#?}", s.shots);
        }
        assert!(s.shots.iter().any(|sh| sh.kind == ShotKind::TwoShot));
        assert!(s.shots.iter().any(|sh| sh.kind == ShotKind::Reaction), "{:#?}", s.shots);
        // Fern looks right, the heron left.
        let single = |who: &str| s.shots.iter().find(|sh| matches!(sh.kind, ShotKind::CloseUp | ShotKind::OverTheShoulder) && sh.subjects == [who]).expect("a single");
        let (fern, heron) = (single("fern"), single("heron"));
        assert_eq!((fern.looks, heron.looks), (Some(ScreenSide::Right), Some(ScreenSide::Left)));
        // The second scene goes on in the same place: wide, not establishing.
        assert_eq!(st.scenes[1].shots[0].kind, ShotKind::Wide);
        let bad: Vec<_> = st.check().into_iter().filter(|i| i.severity >= Severity::Warning).collect();
        assert!(bad.is_empty(), "{bad:#?}\n{}", st.shot_list());
        assert_eq!(st.scenes[0].shots[0].id, "1A");
    }

    #[test]
    fn a_long_narration_is_paced_not_chunked() {
        let para = "The fox ran across the hill. The wind was cold. She thought of home and felt alone. A crow watched from the old oak. Night was coming fast.";
        let text = (0..10).map(|_| para).collect::<Vec<_>>().join("\n\n");
        let mut st = fountain::read(&text);
        st.cast.push(Character { id: "fox".into(), look: "red fox".into(), size: 1.0, ..Default::default() });
        for s in &mut st.scenes {
            s.cast = vec!["fox".into()];
        }
        st.plan(false);
        let k: Vec<&str> = st.check().iter().map(|i| i.kind).collect();
        assert!(!k.contains(&"monotonous_pacing") && !k.contains(&"little_shot_variety") && !k.contains(&"uncovered_line"), "{k:?}");
        let kinds: std::collections::HashSet<ShotKind> = st.scenes.iter().flat_map(|s| s.shots.iter().map(|sh| sh.kind)).collect();
        assert!(kinds.len() >= 4, "{kinds:?}");
    }
}
