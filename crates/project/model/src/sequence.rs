// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Sequences, lanes, transitions and markers.

use crate::clip::{Clip, Transform};
use crate::id::{ClipId, LaneId, MarkerId, SequenceId, TransitionId};
use crate::time::Micros;
use crate::value::{Extra, ParamMap, Rgba, Vec2};
use serde::{Deserialize, Serialize};

/// A timeline: a stack of lanes holding clips, plus transitions and markers.
///
/// `lanes` is in display order, top to bottom: visual lanes, then exactly
/// one spine lane, then audio lanes. Lane *positions* count from the spine:
/// the spine is 0, visual lanes are +1, +2, … going up and audio lanes are
/// −1, −2, … going down (see [`Sequence::lane_position`]).
///
/// Clips are stored in one list sorted by `(lane, start, id)`; the model
/// re-sorts after every edit so equal timelines compare and save equal.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sequence {
    /// Unique id.
    pub id: SequenceId,
    /// Display name.
    pub name: String,
    /// Lanes, top to bottom.
    pub lanes: Vec<Lane>,
    /// All clips of all lanes.
    #[serde(default)]
    pub clips: Vec<Clip>,
    /// Transitions between adjacent spine clips, sorted by id.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transitions: Vec<Transition>,
    /// Sequence markers, sorted by time.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<Marker>,
    /// Fields this version does not know about.
    #[serde(flatten)]
    pub extra: Extra,
}

impl Sequence {
    /// A clip's transform at sequence time `t` as it is drawn: its own
    /// (keyframes included), carried by its parents. Each parent moves,
    /// scales and turns it by how far the parent has moved, scaled and
    /// turned since they were linked, about the parent's position.
    pub fn placed_transform(&self, clip: &Clip, t: Micros) -> Transform {
        // Drivers worked out first, for the clip and each parent.
        let mut tr = self.driven(clip, t).transform_at(t - clip.start);
        let mut link = clip.parent.clone();
        let mut seen = vec![clip.id];
        // A chain of parents, each once (a loop stops where it repeats).
        while let Some(p) = link.take() {
            if seen.contains(&p.clip) || seen.len() > 16 {
                break;
            }
            let Some(parent) = self.clip(p.clip) else { break };
            seen.push(parent.id);
            let now = self.driven(parent, t).transform_at((t - parent.start).clamp(0, parent.duration));
            let ratio = |now: f64, rest: f64| if rest.abs() < 1e-9 { 1.0 } else { now / rest };
            let (kx, ky) = (ratio(now.scale_x, p.rest_scale.x), ratio(now.scale_y, p.rest_scale.y));
            let turn = |v: Vec2, deg: f64| {
                let (s, c) = deg.to_radians().sin_cos();
                Vec2 { x: v.x * c - v.y * s, y: v.x * s + v.y * c }
            };
            if now.three_d {
                // In space: the offset from the parent turns with the
                // parent's whole rotation, and so does the child.
                use crate::space::{apply, euler, mul, rotation, transpose};
                let rest = rotation([0.0; 3], p.rest_rotation_x, p.rest_rotation_y, p.rest_rotation);
                let turned = rotation([0.0; 3], now.rotation_x, now.rotation_y, now.rotation);
                let kz = ratio(now.scale_z, 1.0);
                let v = apply(&transpose(&rest), [tr.position.x - p.rest_position.x, tr.position.y - p.rest_position.y, tr.position_z - p.rest_z]);
                let v = apply(&turned, [v[0] * kx, v[1] * ky, v[2] * kz]);
                tr.position = Vec2 { x: now.position.x + v[0], y: now.position.y + v[1] };
                tr.position_z = now.position_z + v[2];
                let own = rotation([0.0; 3], tr.rotation_x, tr.rotation_y, tr.rotation);
                let [rx, ry, rz] = euler(&mul(&mul(&turned, &transpose(&rest)), &own));
                // Keep whole turns the child already had about z.
                let spin = ((tr.rotation + now.rotation - p.rest_rotation - rz) / 360.0).round() * 360.0;
                (tr.rotation_x, tr.rotation_y, tr.rotation) = (rx, ry, rz + spin);
                tr.scale_x *= kx;
                tr.scale_y *= ky;
                link = parent.parent.clone();
                continue;
            }
            // Into the parent's resting frame, scaled, then out at its
            // present angle and place.
            let v = turn(Vec2 { x: tr.position.x - p.rest_position.x, y: tr.position.y - p.rest_position.y }, -p.rest_rotation);
            // A part layer on a bone: carried by the bone's motion first,
            // in the parent's resting frame.
            let v = match p.bone.as_ref().and_then(|link| {
                let sk = parent.skeleton_at((t - parent.start).clamp(0, parent.duration)).filter(|s| s.enabled)?;
                let posed = sk.forward();
                Some(link.carry(&posed.bone(link.bone)?.map))
            }) {
                Some(c) => {
                    tr.rotation += c.angle();
                    c.apply(v)
                }
                None => v,
            };
            let v = turn(Vec2 { x: v.x * kx, y: v.y * ky }, now.rotation);
            tr.position = Vec2 { x: now.position.x + v.x, y: now.position.y + v.y };
            tr.scale_x *= kx;
            tr.scale_y *= ky;
            tr.rotation += now.rotation - p.rest_rotation;
            link = parent.parent.clone();
        }
        tr
    }

    /// A sequence holding only an empty spine lane.
    pub fn new(id: SequenceId, name: impl Into<String>, spine: LaneId) -> Self {
        Self {
            id,
            name: name.into(),
            lanes: vec![Lane::new(spine, LaneKind::Spine, "Main")],
            clips: Vec::new(),
            transitions: Vec::new(),
            markers: Vec::new(),
            extra: Extra::new(),
        }
    }

    /// The spine lane.
    ///
    /// # Panics
    /// Panics if the sequence has no spine, which the invariants forbid.
    pub fn spine(&self) -> &Lane {
        self.lanes.iter().find(|l| l.kind == LaneKind::Spine).expect("sequence has a spine lane")
    }

    /// The spine lane's id.
    pub fn spine_id(&self) -> LaneId {
        self.spine().id
    }

    /// Looks up a lane.
    pub fn lane(&self, id: LaneId) -> Option<&Lane> {
        self.lanes.iter().find(|l| l.id == id)
    }

    /// Index of a lane in `lanes` (display order).
    pub fn lane_index(&self, id: LaneId) -> Option<usize> {
        self.lanes.iter().position(|l| l.id == id)
    }

    /// Position counted from the spine: 0 spine, positive above, negative below.
    pub fn lane_position(&self, id: LaneId) -> Option<i32> {
        let spine = self.lanes.iter().position(|l| l.kind == LaneKind::Spine)? as i32;
        Some(spine - self.lane_index(id)? as i32)
    }

    /// The lane at a position counted from the spine.
    pub fn lane_at_position(&self, pos: i32) -> Option<&Lane> {
        let spine = self.lanes.iter().position(|l| l.kind == LaneKind::Spine)? as i32;
        let idx = spine - pos;
        if idx < 0 { None } else { self.lanes.get(idx as usize) }
    }

    /// Looks up a clip.
    pub fn clip(&self, id: ClipId) -> Option<&Clip> {
        self.clips.iter().find(|c| c.id == id)
    }

    /// Clips on one lane, sorted by start.
    pub fn clips_on(&self, lane: LaneId) -> Vec<&Clip> {
        // `clips` is sorted by lane first, so this is a contiguous run.
        self.clips.iter().filter(|c| c.lane == lane).collect()
    }

    /// Spine clips in order.
    pub fn spine_clips(&self) -> Vec<&Clip> {
        self.clips_on(self.spine_id())
    }

    /// The clip on `lane` covering time `t`.
    pub fn clip_at(&self, lane: LaneId, t: Micros) -> Option<&Clip> {
        self.clips.iter().find(|c| c.lane == lane && c.range().contains(t))
    }

    /// End of the last clip on any lane.
    pub fn duration(&self) -> Micros {
        self.clips.iter().map(|c| c.end()).max().unwrap_or(0)
    }

    /// End of the spine.
    pub fn spine_end(&self) -> Micros {
        let spine = self.spine_id();
        self.clips.iter().filter(|c| c.lane == spine).map(|c| c.end()).max().unwrap_or(0)
    }

    /// Looks up a transition.
    pub fn transition(&self, id: TransitionId) -> Option<&Transition> {
        self.transitions.iter().find(|t| t.id == id)
    }

    /// The transition attached to a clip on either side.
    pub fn transitions_of(&self, clip: ClipId) -> impl Iterator<Item = &Transition> {
        self.transitions.iter().filter(move |t| t.left == clip || t.right == clip)
    }

    /// The spine clip directly after `clip`, if `clip` is on the spine.
    pub fn next_on_spine(&self, clip: ClipId) -> Option<&Clip> {
        let c = self.clip(clip)?;
        self.clips.iter().find(|n| n.lane == c.lane && n.start == c.end() && n.id != c.id)
    }

    /// Sorts clips, transitions and markers into canonical order.
    pub(crate) fn normalize(&mut self) {
        self.clips.sort_by_key(|c| (c.lane, c.start, c.id));
        self.transitions.sort_by_key(|t| t.id);
        self.markers.sort_by_key(|m| (m.time, m.id));
        for c in &mut self.clips {
            c.markers.sort_by_key(|m| (m.time, m.id));
        }
    }
}

/// A row of the timeline.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Lane {
    /// Unique id.
    pub id: LaneId,
    /// Spine, visual or audio.
    pub kind: LaneKind,
    /// Display name.
    #[serde(default)]
    pub name: String,
    /// Label colour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colour: Option<Rgba>,
    /// Silence every clip on the lane.
    #[serde(default, skip_serializing_if = "is_false")]
    pub muted: bool,
    /// Play only soloed lanes when any lane is soloed.
    #[serde(default, skip_serializing_if = "is_false")]
    pub solo: bool,
    /// Do not draw the lane's pictures.
    #[serde(default, skip_serializing_if = "is_false")]
    pub hidden: bool,
    /// Refuse edits to the lane's clips.
    #[serde(default, skip_serializing_if = "is_false")]
    pub locked: bool,
    /// Lane gain in decibels.
    #[serde(default)]
    pub volume_db: f64,
    /// Lane stereo position, −1 to 1.
    #[serde(default)]
    pub pan: f64,
    /// Fields this version does not know about.
    #[serde(flatten)]
    pub extra: Extra,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Lane {
    /// An unlocked, audible, visible lane.
    pub fn new(id: LaneId, kind: LaneKind, name: impl Into<String>) -> Self {
        Self { id, kind, name: name.into(), colour: None, muted: false, solo: false, hidden: false, locked: false, volume_db: 0.0, pan: 0.0, extra: Extra::new() }
    }
}

/// Role of a lane.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaneKind {
    /// The main storyline: clips packed end to end from time zero.
    Spine,
    /// Free placement of pictures above the spine (overlays, titles).
    Visual,
    /// Free placement of sound below the spine.
    Audio,
}

/// A transition between two adjacent spine clips.
///
/// The transition straddles the cut; `align` says how. During the
/// transition the left clip keeps playing past its out point and the right
/// clip starts before its in point, so both need enough unused source
/// ("handles"). The timeline length does not change.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transition {
    /// Unique id.
    pub id: TransitionId,
    /// Outgoing clip.
    pub left: ClipId,
    /// Incoming clip; must start where `left` ends.
    pub right: ClipId,
    /// Transition name as registered by the `fx` crate, e.g. `"dissolve"`.
    pub kind: String,
    /// Total length; positive.
    pub duration: Micros,
    /// Placement relative to the cut.
    #[serde(default)]
    pub align: TransitionAlign,
    /// Parameters (direction, softness, colour, …).
    #[serde(default, skip_serializing_if = "ParamMap::is_empty")]
    pub params: ParamMap,
    /// Fields this version does not know about.
    #[serde(flatten)]
    pub extra: Extra,
}

impl Transition {
    /// Time before and after the cut covered by the transition.
    pub fn split(&self) -> (Micros, Micros) {
        match self.align {
            TransitionAlign::Centre => {
                let pre = self.duration / 2;
                (pre, self.duration - pre)
            }
            TransitionAlign::EndAtCut => (self.duration, 0),
            TransitionAlign::StartAtCut => (0, self.duration),
        }
    }
}

/// Where a transition sits relative to the cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionAlign {
    /// Centred on the cut.
    #[default]
    Centre,
    /// Finishes at the cut.
    EndAtCut,
    /// Starts at the cut.
    StartAtCut,
}

/// A note at a point (or span) in time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    /// Unique id.
    pub id: MarkerId,
    /// Time on the sequence, or relative to the clip start for clip markers.
    pub time: Micros,
    /// Length for range markers; zero for a point.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub duration: Micros,
    /// Short title.
    #[serde(default)]
    pub name: String,
    /// Longer note.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// Colour.
    #[serde(default = "marker_colour")]
    pub colour: Rgba,
    /// Fields this version does not know about.
    #[serde(flatten)]
    pub extra: Extra,
}

fn is_zero(v: &Micros) -> bool {
    *v == 0
}

fn marker_colour() -> Rgba {
    Marker::DEFAULT_COLOUR
}

impl Marker {
    /// Default marker colour (a soft blue).
    pub const DEFAULT_COLOUR: Rgba = Rgba::new(0.30, 0.55, 0.95, 1.0);

    /// A point marker with the default colour.
    pub fn new(id: MarkerId, time: Micros, name: impl Into<String>) -> Self {
        Self { id, time, duration: 0, name: name.into(), note: String::new(), colour: Self::DEFAULT_COLOUR, extra: Extra::new() }
    }
}

#[cfg(test)]
mod parent_tests {
    use super::*;
    use crate::clip::{ClipSource, Generator, GeneratorKind, Parent};

    #[test]
    fn children_follow_their_parent() {
        let mut seq = Sequence::new(SequenceId(1), "s", LaneId(1));
        let card = |id: u64, x: f64| {
            let mut c = Clip::new(ClipId(id), LaneId(1), 0, 4_000_000, ClipSource::Generator(Generator::new(GeneratorKind::Solid)));
            c.transform.position = Vec2 { x, y: 0.0 };
            c
        };
        let mut parent = card(1, 0.0);
        let mut child = card(2, 100.0);
        child.parent = Some(Parent { clip: ClipId(1), rest_position: Vec2::ZERO, rest_scale: Vec2 { x: 1.0, y: 1.0 }, rest_rotation: 0.0, rest_z: 0.0, rest_rotation_x: 0.0, rest_rotation_y: 0.0, bone: None });
        seq.clips.push(parent.clone());
        seq.clips.push(child.clone());
        // Linked: nothing moves.
        assert_eq!(seq.placed_transform(&child, 0).position, Vec2 { x: 100.0, y: 0.0 });
        // The parent moves, turns a quarter and doubles: the child goes
        // with it, around it.
        parent.transform.position = Vec2 { x: 50.0, y: 20.0 };
        parent.transform.rotation = 90.0;
        parent.transform.scale_x = 2.0;
        parent.transform.scale_y = 2.0;
        seq.clips[0] = parent;
        let t = seq.placed_transform(&child, 0);
        assert!((t.position.x - 50.0).abs() < 1e-9 && (t.position.y - 220.0).abs() < 1e-9, "{:?}", t.position);
        assert_eq!((t.rotation, t.scale_x, t.scale_y), (90.0, 2.0, 2.0));
        // A 3D parent turning a quarter about y swings the child's offset
        // into depth and turns it with it.
        let mut p3 = seq.clips[0].clone();
        p3.transform = Transform { three_d: true, rotation_y: 90.0, ..Transform::default() };
        seq.clips[0] = p3;
        let mut c3 = child.clone();
        c3.parent = Some(Parent { clip: ClipId(1), rest_position: Vec2::ZERO, rest_scale: Vec2 { x: 1.0, y: 1.0 }, rest_rotation: 0.0, rest_z: 0.0, rest_rotation_x: 0.0, rest_rotation_y: 0.0, bone: None });
        let t = seq.placed_transform(&c3, 0);
        assert!(t.position.x.abs() < 1e-9 && (t.position_z - 100.0).abs() < 1e-9, "{:?} {}", t.position, t.position_z);
        assert!((t.rotation_y - 90.0).abs() < 1e-6, "{}", t.rotation_y);
        // A loop of parents does not hang.
        let mut a = seq.clips[0].clone();
        a.parent = Some(Parent { clip: ClipId(2), rest_position: Vec2::ZERO, rest_scale: Vec2 { x: 1.0, y: 1.0 }, rest_rotation: 0.0, rest_z: 0.0, rest_rotation_x: 0.0, rest_rotation_y: 0.0, bone: None });
        seq.clips[0] = a;
        let _ = seq.placed_transform(&child, 0);
    }
}
