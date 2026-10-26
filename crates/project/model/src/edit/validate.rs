// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Project invariants.
//!
//! [`Project::apply`] checks these after every edit and rolls the edit back
//! if one fails, so a project reached only through edits always satisfies
//! them. Loading a file also checks them.

use crate::clip::{ClipSource, Speed};
use crate::project::Project;
use crate::sequence::{LaneKind, Sequence};
use std::collections::{HashMap, HashSet};
use std::fmt;

/// A broken invariant, with enough detail to find the culprit.
#[derive(Clone, Debug, PartialEq)]
pub enum Violation {
    /// Two objects share an id.
    DuplicateId(String),
    /// A sequence does not have exactly one spine, or lanes are out of
    /// order (visual lanes must sit above the spine, audio lanes below).
    LaneOrder(u64),
    /// A clip refers to a lane, media item or sequence that does not exist.
    DanglingReference(String),
    /// A clip has zero or negative duration, a negative start, or a bad speed.
    BadTiming(u64),
    /// Spine clips overlap or leave a hole (the spine is packed from zero).
    SpineNotPacked(u64),
    /// Clips on a free lane overlap.
    Overlap(u64, u64),
    /// A gap clip sits on a lane other than the spine.
    GapOffSpine(u64),
    /// A clip uses source media outside the file.
    SourceOutOfRange(u64),
    /// A transition does not join adjacent spine clips or does not fit
    /// their handles.
    BadTransition(u64),
    /// A keyframe track is malformed or names an unknown property.
    BadTrack(u64, String),
    /// A sequence contains itself through compound clips.
    NestingCycle(u64),
    /// The active sequence does not exist.
    NoActiveSequence,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Violation::DuplicateId(what) => write!(f, "duplicate id: {what}"),
            Violation::LaneOrder(s) => write!(f, "sequence {s}: lanes must be visual…, one spine, audio…"),
            Violation::DanglingReference(what) => write!(f, "dangling reference: {what}"),
            Violation::BadTiming(c) => write!(f, "clip {c}: duration must be positive, start non-negative, speed finite and non-negative"),
            Violation::SpineNotPacked(c) => write!(f, "spine is not packed end to end at clip {c}"),
            Violation::Overlap(a, b) => write!(f, "clips {a} and {b} overlap"),
            Violation::GapOffSpine(c) => write!(f, "gap clip {c} is not on the spine"),
            Violation::SourceOutOfRange(c) => write!(f, "clip {c} uses source outside its media"),
            Violation::BadTransition(t) => write!(f, "transition {t} does not fit its clips"),
            Violation::BadTrack(c, p) => write!(f, "clip {c}: bad keyframe track `{p}`"),
            Violation::NestingCycle(s) => write!(f, "sequence {s} contains itself"),
            Violation::NoActiveSequence => write!(f, "active sequence does not exist"),
        }
    }
}

impl std::error::Error for Violation {}

type Check = Result<(), Violation>;

impl Project {
    /// Checks every invariant: unique ids, lane order, positive durations,
    /// a packed spine, no overlaps on free lanes, transitions within their
    /// handles, valid keyframe tracks, no dangling references or nesting
    /// cycles. Returns the first violation found.
    pub fn check_invariants(&self) -> Check {
        if self.sequence(self.active_sequence).is_none() {
            return Err(Violation::NoActiveSequence);
        }
        // Kinds by a tag, not a string per id: this runs after every edit,
        // and a long project has tens of thousands of ids.
        let n = self.sequences.iter().map(|s| s.clips.len() + s.lanes.len() + s.transitions.len() + s.markers.len()).sum::<usize>() + self.media.len();
        let mut ids: HashSet<(u8, u64)> = HashSet::with_capacity(n);
        let mut unique = |kind: &str, id: u64| {
            let tag = match kind {
                "media" => 0,
                "preset" => 1,
                "sequence" => 2,
                "lane" => 3,
                "clip" => 4,
                "transition" => 5,
                _ => 6,
            };
            if ids.insert((tag, id)) { Ok(()) } else { Err(Violation::DuplicateId(format!("{kind} {id}"))) }
        };
        for m in &self.media {
            unique("media", m.id.0)?;
        }
        for p in &self.presets {
            unique("preset", p.id.0)?;
        }
        for s in &self.sequences {
            unique("sequence", s.id.0)?;
            for l in &s.lanes {
                unique("lane", l.id.0)?;
            }
            for c in &s.clips {
                unique("clip", c.id.0)?;
            }
            for t in &s.transitions {
                unique("transition", t.id.0)?;
            }
            for m in s.markers.iter().chain(s.clips.iter().flat_map(|c| c.markers.iter())) {
                unique("marker", m.id.0)?;
            }
        }
        for s in &self.sequences {
            self.check_sequence(s)?;
        }
        self.check_nesting()
    }

    fn check_sequence(&self, s: &Sequence) -> Check {
        // Lane order: Visual*, Spine, Audio*.
        let kinds: Vec<LaneKind> = s.lanes.iter().map(|l| l.kind).collect();
        let spine_count = kinds.iter().filter(|k| **k == LaneKind::Spine).count();
        let spine_at = kinds.iter().position(|k| *k == LaneKind::Spine);
        let ordered = spine_at.is_some_and(|i| kinds[..i].iter().all(|k| *k == LaneKind::Visual) && kinds[i + 1..].iter().all(|k| *k == LaneKind::Audio));
        if spine_count != 1 || !ordered {
            return Err(Violation::LaneOrder(s.id.0));
        }
        let spine = s.spine_id();
        // Looked up once, not scanned for every clip and transition.
        let media: HashMap<crate::MediaId, &crate::MediaItem> = self.media.iter().map(|m| (m.id, m)).collect();
        let lanes: HashSet<crate::LaneId> = s.lanes.iter().map(|l| l.id).collect();

        for c in &s.clips {
            if !lanes.contains(&c.lane) {
                return Err(Violation::DanglingReference(format!("clip {} lane {}", c.id.0, c.lane.0)));
            }
            let speed_ok = match &c.speed {
                Speed::Constant(r) => r.is_finite() && *r >= 0.0,
                Speed::Curve(tr) => tr.validate().is_ok() && tr.keys.iter().all(|k| matches!(k.value, crate::Value::Scalar(_))),
            };
            if c.duration <= 0 || c.start < 0 || !speed_ok || c.source_in < 0 {
                return Err(Violation::BadTiming(c.id.0));
            }
            if c.is_gap() && c.lane != spine {
                return Err(Violation::GapOffSpine(c.id.0));
            }
            match &c.source {
                ClipSource::Media(m) => {
                    let item = media.get(&m.media).copied().ok_or_else(|| Violation::DanglingReference(format!("clip {} media {}", c.id.0, m.media.0)))?;
                    if let Some(total) = item.duration()
                        && c.source_out() > total
                    {
                        return Err(Violation::SourceOutOfRange(c.id.0));
                    }
                }
                ClipSource::Sequence(r) => {
                    if self.sequence(r.sequence).is_none() {
                        return Err(Violation::DanglingReference(format!("clip {} sequence {}", c.id.0, r.sequence.0)));
                    }
                }
                ClipSource::Generator(_) | ClipSource::Gap => {}
            }
            if c.effects.len() + c.audio.effects.len() + c.masks.len() > 1 {
                let mut sub = HashSet::new();
                for e in c.effects.iter().map(|e| e.id.0).chain(c.audio.effects.iter().map(|e| e.id.0)).chain(c.masks.iter().map(|m| m.id.0)) {
                    if !sub.insert(e) {
                        return Err(Violation::DuplicateId(format!("effect/mask {e} in clip {}", c.id.0)));
                    }
                }
            }
            for (path, tr) in &c.keyframes {
                let bad = || Violation::BadTrack(c.id.0, path.clone());
                tr.validate().map_err(|_| bad())?;
                let stat = c.get_property(path).ok_or_else(bad)?;
                if !stat.same_kind(&tr.keys[0].value) {
                    return Err(bad());
                }
            }
            for (path, d) in &c.drivers {
                if c.get_property(crate::driver::split_key(path).0).is_none() || d.problem().is_some() {
                    return Err(Violation::BadTrack(c.id.0, path.clone()));
                }
            }
        }

        // Spine packed from zero; free lanes without overlaps. Clips are
        // sorted by (lane, start), so each lane is a contiguous sorted run.
        let mut expected_start = 0;
        let mut prev: HashMap<_, &crate::Clip> = HashMap::new();
        for c in &s.clips {
            if c.lane == spine {
                if c.start != expected_start {
                    return Err(Violation::SpineNotPacked(c.id.0));
                }
                expected_start = c.end();
            } else if let Some(p) = prev.get(&c.lane)
                && p.end() > c.start
            {
                return Err(Violation::Overlap(p.id.0, c.id.0));
            }
            prev.insert(c.lane, c);
        }

        // Transitions.
        let mut cuts = HashSet::new();
        // Time each clip gives to transitions at its end (`tail`) and start (`head`).
        let mut tail_use: HashMap<u64, i64> = HashMap::new();
        let mut head_use: HashMap<u64, i64> = HashMap::new();
        let clip_of: HashMap<crate::ClipId, &crate::Clip> = if s.transitions.is_empty() { HashMap::new() } else { s.clips.iter().map(|c| (c.id, c)).collect() };
        for t in &s.transitions {
            let bad = || Violation::BadTransition(t.id.0);
            let (l, r) = (clip_of.get(&t.left).copied().ok_or_else(bad)?, clip_of.get(&t.right).copied().ok_or_else(bad)?);
            if l.lane != spine || r.lane != spine || l.end() != r.start || t.duration <= 0 || !cuts.insert(l.id) {
                return Err(bad());
            }
            let (pre, post) = t.split();
            if pre > l.duration || post > r.duration {
                return Err(bad());
            }
            let (_, l_after) = l.handles(self.source_duration(l));
            let (r_before, _) = r.handles(self.source_duration(r));
            if l_after.is_some_and(|h| post > h) || r_before.is_some_and(|h| pre > h) {
                return Err(bad());
            }
            tail_use.insert(l.id.0, pre);
            head_use.insert(r.id.0, post);
        }
        for (id, pre) in &tail_use {
            let post = head_use.get(id).copied().unwrap_or(0);
            let c = clip_of[&crate::ClipId(*id)];
            if pre + post > c.duration {
                let t = s.transitions_of(c.id).next().expect("has transitions");
                return Err(Violation::BadTransition(t.id.0));
            }
        }
        Ok(())
    }

    /// Length of a clip's source, if known and finite.
    pub fn source_duration(&self, c: &crate::Clip) -> Option<crate::Micros> {
        match &c.source {
            ClipSource::Media(m) => self.media_item(m.media).and_then(|i| i.duration()),
            ClipSource::Sequence(r) => self.sequence(r.sequence).map(|s| s.duration()),
            _ => None,
        }
    }

    fn check_nesting(&self) -> Check {
        // Depth-first search over "sequence contains compound of" edges.
        let edges: HashMap<u64, Vec<u64>> = self.sequences.iter().map(|s| (s.id.0, s.clips.iter().filter_map(|c| c.sequence_id()).map(|i| i.0).collect())).collect();
        #[derive(Clone, Copy, PartialEq)]
        enum Mark {
            Visiting,
            Done,
        }
        fn visit(n: u64, edges: &HashMap<u64, Vec<u64>>, marks: &mut HashMap<u64, Mark>) -> Check {
            match marks.get(&n) {
                Some(Mark::Done) => return Ok(()),
                Some(Mark::Visiting) => return Err(Violation::NestingCycle(n)),
                None => {}
            }
            marks.insert(n, Mark::Visiting);
            for &m in edges.get(&n).into_iter().flatten() {
                visit(m, edges, marks)?;
            }
            marks.insert(n, Mark::Done);
            Ok(())
        }
        let mut marks = HashMap::new();
        for s in &self.sequences {
            visit(s.id.0, &edges, &mut marks)?;
        }
        Ok(())
    }
}
