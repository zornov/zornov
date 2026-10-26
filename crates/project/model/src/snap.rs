// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Snapping helpers for interactive moves and trims.
//!
//! Snapping is kept out of the edits themselves: the UI asks where a
//! dragged edge should land, then applies a plain edit with the snapped
//! delta. That keeps edits deterministic and testable.

use crate::id::ClipId;
use crate::sequence::Sequence;
use crate::time::Micros;

/// Times a dragged edge may snap to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SnapTargets {
    times: Vec<Micros>,
}

impl SnapTargets {
    /// Collects clip edges (except those of `exclude`, usually the clips
    /// being dragged), sequence and clip markers, the sequence start, and
    /// the playhead.
    pub fn collect(seq: &Sequence, playhead: Option<Micros>, exclude: &[ClipId]) -> Self {
        let mut times = vec![0];
        times.extend(playhead);
        for c in seq.clips.iter().filter(|c| !exclude.contains(&c.id)) {
            times.push(c.start);
            times.push(c.end());
            times.extend(c.markers.iter().map(|m| c.start + m.time));
        }
        times.extend(seq.markers.iter().map(|m| m.time));
        times.sort_unstable();
        times.dedup();
        Self { times }
    }

    /// Adds extra targets (e.g. beat markers from audio analysis).
    pub fn add(&mut self, extra: impl IntoIterator<Item = Micros>) {
        self.times.extend(extra);
        self.times.sort_unstable();
        self.times.dedup();
    }

    /// All targets, sorted.
    pub fn times(&self) -> &[Micros] {
        &self.times
    }

    /// The target nearest `t` within `threshold`, if any.
    pub fn snap(&self, t: Micros, threshold: Micros) -> Option<Micros> {
        let i = self.times.partition_point(|x| *x < t);
        [i.checked_sub(1), Some(i)]
            .into_iter()
            .flatten()
            .filter_map(|j| self.times.get(j).copied())
            .filter(|x| (x - t).abs() <= threshold)
            .min_by_key(|x| (x - t).abs())
    }

    /// For a span `[start, start + duration)` being dragged, the smallest
    /// correction that snaps either edge to a target within `threshold`.
    pub fn snap_span(&self, start: Micros, duration: Micros, threshold: Micros) -> Option<Micros> {
        let a = self.snap(start, threshold).map(|x| x - start);
        let b = self.snap(start + duration, threshold).map(|x| x - start - duration);
        match (a, b) {
            (Some(a), Some(b)) => Some(if a.abs() <= b.abs() { a } else { b }),
            (a, b) => a.or(b),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip::Clip;
    use crate::id::{LaneId, SequenceId};

    #[test]
    fn snaps_to_nearest_edge() {
        let mut s = Sequence::new(SequenceId(1), "s", LaneId(2));
        s.clips.push(Clip::gap(ClipId(3), LaneId(2), 0, 1000));
        s.clips.push(Clip::gap(ClipId(4), LaneId(2), 1000, 500));
        let t = SnapTargets::collect(&s, Some(1200), &[ClipId(4)]);
        assert_eq!(t.times(), &[0, 1000, 1200]);
        assert_eq!(t.snap(1090, 100), Some(1000));
        assert_eq!(t.snap(1150, 100), Some(1200));
        assert_eq!(t.snap(1600, 100), None);
        assert_eq!(t.snap_span(950, 240, 60), Some(10));
    }
}
