// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Changing the script's lines without breaking the shots built on them:
//! a shot names its lines by number, so adding, removing or moving a line
//! renumbers them in step.

use super::{Character, Line, LineKind, Scene, Story};

impl Story {
    /// Puts `line` at `at` (0 = first) in scene `scene`; shots that play
    /// lines from there on keep playing the same words. A speaker the
    /// story does not know yet joins the cast.
    pub fn insert_line(&mut self, scene: usize, at: usize, line: Line) -> bool {
        let Some(sc) = self.scenes.get_mut(scene) else { return false };
        let at = at.min(sc.lines.len());
        sc.lines.insert(at, line);
        for sh in &mut sc.shots {
            for x in &mut sh.lines {
                if *x >= at {
                    *x += 1;
                }
            }
        }
        true
    }

    /// Takes line `at` out of scene `scene`; the shots stop playing it and
    /// the later lines are renumbered.
    pub fn remove_line(&mut self, scene: usize, at: usize) -> bool {
        let Some(sc) = self.scenes.get_mut(scene) else { return false };
        if at >= sc.lines.len() {
            return false;
        }
        sc.lines.remove(at);
        for sh in &mut sc.shots {
            sh.lines.retain(|x| *x != at);
            for x in &mut sh.lines {
                if *x > at {
                    *x -= 1;
                }
            }
        }
        true
    }

    /// Moves line `from` to position `to` in its scene; shots follow the
    /// words.
    pub fn move_line(&mut self, scene: usize, from: usize, to: usize) -> bool {
        let Some(sc) = self.scenes.get_mut(scene) else { return false };
        if from >= sc.lines.len() || to >= sc.lines.len() {
            return false;
        }
        if from == to {
            return true;
        }
        let l = sc.lines.remove(from);
        sc.lines.insert(to, l);
        let map = |x: usize| {
            if x == from {
                to
            } else if from < to && x > from && x <= to {
                x - 1
            } else if from > to && x >= to && x < from {
                x + 1
            } else {
                x
            }
        };
        for sh in &mut sc.shots {
            for x in &mut sh.lines {
                *x = map(*x);
            }
        }
        true
    }

    /// Makes sure a speaker exists in the cast (and, for dialogue, in the
    /// scene's cast). The narrator is never in the cast.
    pub fn know_speaker(&mut self, scene: usize, who: &str, kind: LineKind) {
        if who.is_empty() || who == "narrator" || kind == LineKind::Action {
            return;
        }
        if self.character(who).is_none() {
            self.cast.push(Character { id: who.to_owned(), name: super::fountain::title_case(&who.replace('_', " ")), size: 1.0, ..Default::default() });
        }
        if kind == LineKind::Dialogue {
            if let Some(sc) = self.scenes.get_mut(scene) {
                if !sc.cast.iter().any(|c| c == who) {
                    sc.cast.push(who.to_owned());
                }
            }
        }
    }

    /// A scene id not used yet (`"4"`, or the next number).
    pub fn next_scene_id(&self) -> String {
        (self.scenes.len() + 1..).map(|n| n.to_string()).find(|id| self.scene(id).is_none()).unwrap_or_default()
    }

    /// Adds an empty scene at the end (or after scene `after`) and returns
    /// its index.
    pub fn add_scene(&mut self, after: Option<usize>) -> usize {
        let at = after.map(|a| (a + 1).min(self.scenes.len())).unwrap_or(self.scenes.len());
        let id = self.next_scene_id();
        let beat = after.and_then(|a| self.scenes.get(a)).and_then(|s| s.beat.clone());
        self.scenes.insert(at, Scene { id, beat, ..Default::default() });
        at
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::story::{fountain, Shot};

    fn story() -> Story {
        let mut st = fountain::read("EXT. HILL - DAY #1#\n\nNARRATOR (V.O.)\nOne.\n\nNARRATOR (V.O.)\nTwo.\n\nNARRATOR (V.O.)\nThree.\n");
        st.scenes[0].shots = vec![Shot { id: "1A".into(), lines: vec![0, 1], ..Default::default() }, Shot { id: "1B".into(), lines: vec![2], ..Default::default() }];
        st
    }

    #[test]
    fn shots_keep_playing_the_same_words() {
        let mut st = story();
        let words = |st: &Story, shot: usize| st.scenes[0].shots[shot].lines.iter().map(|i| st.scenes[0].lines[*i].text.clone()).collect::<Vec<_>>();
        assert!(st.insert_line(0, 1, Line { text: "New.".into(), ..Default::default() }));
        assert_eq!(words(&st, 0), vec!["One.", "Two."]);
        assert_eq!(words(&st, 1), vec!["Three."]);
        assert!(st.move_line(0, 3, 0));
        assert_eq!(words(&st, 0), vec!["One.", "Two."]);
        assert_eq!(words(&st, 1), vec!["Three."]);
        assert_eq!(st.scenes[0].lines[0].text, "Three.");
        assert!(st.move_line(0, 0, 3));
        assert_eq!(words(&st, 1), vec!["Three."]);
        assert!(st.remove_line(0, 2));
        assert_eq!(words(&st, 0), vec!["One."]);
        assert_eq!(words(&st, 1), vec!["Three."]);
        assert!(!st.remove_line(0, 9) && !st.move_line(0, 0, 9) && !st.insert_line(5, 0, Line::default()));
    }

    #[test]
    fn scenes_and_speakers_are_added() {
        let mut st = story();
        let i = st.add_scene(Some(0));
        assert_eq!((i, st.scenes[1].id.as_str()), (1, "2"));
        st.know_speaker(1, "mr_owl", LineKind::Dialogue);
        st.know_speaker(1, "narrator", LineKind::Narration);
        assert_eq!(st.character("mr_owl").unwrap().name, "Mr Owl");
        assert_eq!(st.scenes[1].cast, vec!["mr_owl".to_owned()]);
        assert!(st.character("narrator").is_none());
        assert_eq!(st.next_scene_id(), "3");
    }
}
