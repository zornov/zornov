// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What the transport shows: position, length and timecodes, read from
//! the model (the source of truth), plus the frame stepping the buttons do.
//!
//! The length is the active sequence's end in `studio-model`
//! (`Sequence::duration`), so it is right for projects opened from disk,
//! imported timelines and new clips alike; an empty sequence is 0.

use crate::doc::Doc;
use crate::view::{self, SEC, Time};

#[derive(Clone, Debug, PartialEq)]
pub struct Transport {
    pub position: Time,
    pub duration: Time,
    pub fps: f32,
    /// "00:00:04:09"
    pub position_tc: String,
    /// "00:00:53:06"
    pub duration_tc: String,
    /// One frame in microseconds.
    pub frame: Time,
    /// The sequence has nothing in it yet.
    pub empty: bool,
}

/// Length of the active sequence.
pub fn duration(doc: &Doc) -> Time {
    doc.model.active().duration()
}

pub fn state(doc: &Doc, position: Time) -> Transport {
    let fps = doc.project.fps.max(1.0);
    let d = duration(doc);
    Transport {
        position,
        duration: d,
        fps,
        position_tc: view::timecode(position, fps),
        duration_tc: view::timecode(d, fps),
        frame: frame(fps),
        empty: d == 0,
    }
}

pub fn frame(fps: f32) -> Time {
    (SEC as f64 / fps.max(1.0) as f64).round() as Time
}

/// One frame forwards or back, kept inside the sequence.
pub fn step(doc: &Doc, position: Time, frames: i64) -> Time {
    let f = frame(doc.project.fps);
    (position + frames * f).clamp(0, duration(doc).max(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_comes_from_the_model_and_matches_the_view() {
        for v in [view::demo(), view::one_clip(), view::big()] {
            let doc = Doc::from_view(&v);
            let t = state(&doc, 0);
            assert!(t.duration > 0, "{}", v.name);
            assert_eq!(t.duration, doc.project.duration(), "{}", v.name);
            assert_ne!(t.duration_tc, "00:00:00:00", "{}", v.name);
        }
        let empty = Doc::from_view(&view::empty());
        let t = state(&empty, 0);
        assert!(t.empty && t.duration_tc == "00:00:00:00");
    }

    #[test]
    fn stepping_stays_inside() {
        let doc = Doc::from_view(&view::one_clip());
        assert_eq!(step(&doc, 0, -1), 0);
        assert_eq!(step(&doc, 0, 3), 3 * frame(doc.project.fps));
        let end = duration(&doc);
        assert_eq!(step(&doc, end, 10), end);
    }
}
