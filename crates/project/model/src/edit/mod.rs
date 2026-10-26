// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Reversible editing commands.
//!
//! Every change to a [`Project`] is an [`Edit`] passed to
//! [`Project::apply`], which returns the inverse edit: applying the inverse
//! undoes the change, and applying *its* inverse redoes it. Use
//! [`crate::History`] to keep undo/redo stacks and group edits into
//! transactions.
//!
//! There are two layers of edits:
//!
//! * **Primitives** (`Insert*`, `Remove*`, `Set*`, `MoveLane`, `Batch`)
//!   change one object each. Inverses are always made of primitives, which
//!   makes undo exact: it restores whole objects instead of recomputing.
//! * **Operations** (`Insert`, `Split`, `Trim`, `Slide`, …) are what the
//!   user does. They read the current state, then run primitives.
//!
//! An edit is all-or-nothing: if any step fails, or the result breaks an
//! invariant (see [`Project::check_invariants`]), the project is put back
//! exactly as it was and an error is returned.
//!
//! **Choosing ids.** Edits that create objects take them with an id. Fill
//! it from [`Project::alloc_id`] to know the id in advance, or leave it as
//! `0` to let the edit allocate one. An id that is already in use is
//! replaced by a fresh one.

mod ops;
mod validate;

pub use validate::Violation;

use crate::clip::{Clip, Effect, Grade, Mask};
use crate::clipboard::Clipboard;
use crate::id::*;
use crate::keyframe::{Keyframe, Track};
use crate::media::MediaItem;
use crate::project::{Preset, Project, ProjectSettings};
use crate::sequence::{Lane, Marker, Sequence, Transition};
use crate::time::Micros;
use serde::{Deserialize, Serialize};
use std::fmt;

/// A reversible change to a project. See the [module docs](self).
///
/// Field names follow one convention, so they are documented here once:
/// `seq` is the sequence edited; `clip`/`clips` name existing clips (or
/// carry a clip to add); `lane: None` means the spine; `at` is a sequence
/// time; `delta` a signed change in microseconds (sequence time, except for
/// [`Edit::Slip`] where it is source time); `index`/`to` are list
/// positions; `new_clip`, `new_sequence`, `group`, `link` are ids for
/// objects the edit creates (`0` lets the edit choose).
#[allow(missing_docs)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Edit {
    // ----------------------------------------------------------- primitives
    /// Adds a clip as given (no rippling).
    InsertClip { seq: SequenceId, clip: Box<Clip> },
    /// Removes a clip (no rippling).
    RemoveClip { seq: SequenceId, clip: ClipId },
    /// Replaces the clip with the same id; any field may change.
    SetClip { seq: SequenceId, clip: Box<Clip> },
    /// Adds a transition as given.
    InsertTransition { seq: SequenceId, transition: Transition },
    /// Removes a transition.
    RemoveTransition { seq: SequenceId, id: TransitionId },
    /// Replaces the transition with the same id.
    SetTransition { seq: SequenceId, transition: Transition },
    /// Adds a sequence marker.
    InsertMarker { seq: SequenceId, marker: Marker },
    /// Removes a sequence marker.
    RemoveMarker { seq: SequenceId, id: MarkerId },
    /// Replaces the sequence marker with the same id.
    SetMarker { seq: SequenceId, marker: Marker },
    /// Adds a lane at `index` in display order (top to bottom).
    InsertLane { seq: SequenceId, index: usize, lane: Lane },
    /// Removes an empty lane.
    RemoveLane { seq: SequenceId, lane: LaneId },
    /// Replaces a lane's settings (name, colour, mute, lock, …).
    SetLane { seq: SequenceId, lane: Lane },
    /// Moves a lane to `to` in display order.
    MoveLane { seq: SequenceId, lane: LaneId, to: usize },
    /// Adds a media item to the bin at `index` (end if `None`).
    InsertMedia { item: Box<MediaItem>, index: Option<usize> },
    /// Removes an unused media item.
    RemoveMedia { id: MediaId },
    /// Replaces the media item with the same id.
    SetMedia { item: Box<MediaItem> },
    /// Adds a sequence at `index` (end if `None`).
    InsertSequence { sequence: Box<Sequence>, index: Option<usize> },
    /// Removes a sequence that is neither active nor nested anywhere.
    RemoveSequence { id: SequenceId },
    /// Renames a sequence.
    RenameSequence { id: SequenceId, name: String },
    /// Chooses the sequence shown on open.
    SetActiveSequence { id: SequenceId },
    /// Replaces the project settings.
    SetSettings { settings: Box<ProjectSettings> },
    /// Adds a preset.
    InsertPreset { preset: Preset },
    /// Removes a preset.
    RemovePreset { id: PresetId },
    /// Replaces the preset with the same id.
    SetPreset { preset: Preset },
    /// Replace the story (script, scenes, shots, places, cast).
    SetStory { story: Box<crate::story::Story> },
    /// Several edits as one; applied in order, all or nothing.
    Batch { edits: Vec<Edit> },

    // ----------------------------------------------------------- operations
    /// Inserts a clip at `at`, pushing later clips on the lane right.
    /// `lane: None` means the spine. A clip under `at` is split.
    Insert { seq: SequenceId, lane: Option<LaneId>, at: Micros, clip: Box<Clip> },
    /// Places a clip at `at`, replacing whatever was under it.
    Overwrite { seq: SequenceId, lane: Option<LaneId>, at: Micros, clip: Box<Clip> },
    /// Inserts a clip after the last clip of the lane.
    Append { seq: SequenceId, lane: Option<LaneId>, clip: Box<Clip> },
    /// Cuts clips at `at`: the given clips, or every clip on every unlocked
    /// lane when `clips` is `None`.
    Split { seq: SequenceId, at: Micros, clips: Option<Vec<ClipId>> },
    /// Moves a clip edge by `delta` (positive is later). With `ripple`,
    /// later clips on the lane follow; without, neighbours stay put (on the
    /// spine the opened or consumed space is a gap clip).
    Trim { seq: SequenceId, clip: ClipId, edge: Edge, delta: Micros, ripple: bool },
    /// Removes clips and closes the space they leave.
    RippleDelete { seq: SequenceId, clips: Vec<ClipId> },
    /// Removes clips and leaves the space empty (a gap on the spine).
    Lift { seq: SequenceId, clips: Vec<ClipId> },
    /// Moves clips by `delta`, optionally onto lane `to_lane`, replacing
    /// what is under their new position. Clips leaving the spine leave a gap.
    Move { seq: SequenceId, clips: Vec<ClipId>, delta: Micros, to_lane: Option<LaneId> },
    /// Moves a spine clip to just before `before` (end if `None`), closing
    /// the space it left.
    Reorder { seq: SequenceId, clip: ClipId, before: Option<ClipId> },
    /// Shifts the source range by `delta` without moving the clip.
    Slip { seq: SequenceId, clip: ClipId, delta: Micros },
    /// Moves a clip between its neighbours, which shorten and lengthen.
    Slide { seq: SequenceId, clip: ClipId, delta: Micros },
    /// Moves the cut after `left` by `delta`, trimming both sides.
    Roll { seq: SequenceId, left: ClipId, delta: Micros },
    /// Copies clips and inserts the copies right after them.
    Duplicate { seq: SequenceId, clips: Vec<ClipId> },
    /// Pastes clipboard content at `at`.
    Paste { seq: SequenceId, at: Micros, clipboard: Box<Clipboard>, mode: PasteMode },
    /// Copies chosen attributes of `source` onto `targets`.
    PasteAttributes { seq: SequenceId, targets: Vec<ClipId>, source: Box<Clip>, attrs: AttrSet },
    /// Puts clips in a new selection group.
    Group { seq: SequenceId, clips: Vec<ClipId>, group: GroupId },
    /// Removes clips from their selection groups.
    Ungroup { seq: SequenceId, clips: Vec<ClipId> },
    /// Links clips so they move and trim together.
    Link { seq: SequenceId, clips: Vec<ClipId>, link: LinkId },
    /// Unlinks clips.
    Unlink { seq: SequenceId, clips: Vec<ClipId> },
    /// Moves a media clip's sound to its own linked clip on an audio lane.
    DetachAudio { seq: SequenceId, clip: ClipId, new_clip: ClipId },
    /// Points a clip at another media item, keeping its timing.
    ReplaceMedia { seq: SequenceId, clip: ClipId, media: MediaId },
    /// Inserts a still of the frame at `at`, lasting `duration`.
    FreezeFrame { seq: SequenceId, clip: ClipId, at: Micros, duration: Micros, new_clip: ClipId },
    /// Toggles reverse playback.
    Reverse { seq: SequenceId, clips: Vec<ClipId> },
    /// Changes the speed, keeping the source range; later clips on the
    /// lane ripple so the spine stays packed.
    SetSpeed { seq: SequenceId, clip: ClipId, speed: crate::clip::Speed },
    /// Adds a transition after spine clip `transition.left` (its `right`
    /// is filled in from the timeline).
    AddTransition { seq: SequenceId, transition: Transition },
    /// Adds an effect to a clip's video or audio stack.
    AddEffect { seq: SequenceId, clip: ClipId, stack: Stack, effect: Effect, index: Option<usize> },
    /// Removes an effect and its keyframe tracks.
    RemoveEffect { seq: SequenceId, clip: ClipId, effect: EffectId },
    /// Replaces the effect with the same id.
    SetEffect { seq: SequenceId, clip: ClipId, effect: Effect },
    /// Moves an effect within its stack.
    MoveEffect { seq: SequenceId, clip: ClipId, effect: EffectId, to: usize },
    /// Sets or clears a clip's grade.
    SetGrade { seq: SequenceId, clip: ClipId, grade: Option<Grade> },
    /// Adds a mask.
    AddMask { seq: SequenceId, clip: ClipId, mask: Mask, index: Option<usize> },
    /// Removes a mask and its keyframe tracks.
    RemoveMask { seq: SequenceId, clip: ClipId, mask: MaskId },
    /// Replaces the mask with the same id.
    SetMask { seq: SequenceId, clip: ClipId, mask: Mask },
    /// Adds or replaces one key on a property's track (creating the track).
    SetKeyframe { seq: SequenceId, clip: ClipId, property: String, key: Keyframe },
    /// Removes the key at `time`; removes the track when it empties.
    RemoveKeyframe { seq: SequenceId, clip: ClipId, property: String, time: Micros },
    /// Replaces or removes a property's whole track.
    SetTrack { seq: SequenceId, clip: ClipId, property: String, track: Option<Track> },
    /// Adds a marker to the sequence (`clip: None`) or a clip.
    AddMarker { seq: SequenceId, clip: Option<ClipId>, marker: Marker },
    /// Removes a sequence or clip marker.
    DeleteMarker { seq: SequenceId, clip: Option<ClipId>, id: MarkerId },
    /// Replaces a sequence or clip marker.
    ChangeMarker { seq: SequenceId, clip: Option<ClipId>, marker: Marker },
    /// Adds a visual or audio lane at its default place (visual on top,
    /// audio at the bottom) or at `index`.
    AddLane { seq: SequenceId, lane: Lane, index: Option<usize> },
    /// Removes a non-spine lane with all its clips.
    DeleteLane { seq: SequenceId, lane: LaneId },
    /// Moves clips into a new nested sequence and puts one clip playing it
    /// in their place.
    CreateCompound { seq: SequenceId, clips: Vec<ClipId>, name: String, new_sequence: SequenceId, new_clip: ClipId },
    /// Replaces a compound clip with the clips of its nested sequence.
    ExpandCompound { seq: SequenceId, clip: ClipId },
}

/// A clip edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Edge {
    /// The in point.
    Start,
    /// The out point.
    End,
}

/// Which effect stack of a clip.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stack {
    /// Picture effects.
    Video,
    /// Sound effects.
    Audio,
}

/// How pasted spine clips enter the timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PasteMode {
    /// Push later clips right.
    Insert,
    /// Replace what is under the pasted clips.
    Overwrite,
}

/// Which attributes [`Edit::PasteAttributes`] copies. Keyframe tracks of a
/// chosen attribute come along with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AttrSet {
    /// Position, scale, rotation, anchor, opacity, crop, flips.
    pub transform: bool,
    /// Blend mode.
    pub blend: bool,
    /// Volume, pan, fades and audio effects.
    pub audio: bool,
    /// Video effect stack.
    pub effects: bool,
    /// Colour grade.
    pub grade: bool,
    /// Masks.
    pub masks: bool,
    /// Motion presets.
    pub motion: bool,
    /// Speed (the target keeps its source range and ripples).
    pub speed: bool,
}

impl AttrSet {
    /// Every attribute.
    pub const ALL: Self = Self { transform: true, blend: true, audio: true, effects: true, grade: true, masks: true, motion: true, speed: true };
}

/// Why an edit was refused. The project is unchanged whenever an error is
/// returned.
#[derive(Clone, Debug, PartialEq)]
pub enum EditError {
    /// A referenced object does not exist.
    NotFound(String),
    /// The edit touches a locked lane.
    Locked(LaneId),
    /// The edit makes no sense in the current state (with a reason).
    Invalid(String),
    /// Not enough unused source media for the requested change.
    NoHandles(String),
    /// The edit would leave the project inconsistent.
    Invariant(Violation),
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EditError::NotFound(what) => write!(f, "{what} not found"),
            EditError::Locked(l) => write!(f, "lane {} is locked", l.0),
            EditError::Invalid(why) => write!(f, "invalid edit: {why}"),
            EditError::NoHandles(why) => write!(f, "not enough source media: {why}"),
            EditError::Invariant(v) => write!(f, "edit would break the project: {v}"),
        }
    }
}

impl std::error::Error for EditError {}

pub(crate) type Result<T> = std::result::Result<T, EditError>;

pub(crate) fn invalid<T>(why: impl Into<String>) -> Result<T> {
    Err(EditError::Invalid(why.into()))
}

impl Edit {
    /// Whether this edit is a primitive (the kind inverses are made of).
    pub fn is_primitive(&self) -> bool {
        use Edit::*;
        matches!(
            self,
            InsertClip { .. }
                | RemoveClip { .. }
                | SetClip { .. }
                | InsertTransition { .. }
                | RemoveTransition { .. }
                | SetTransition { .. }
                | InsertMarker { .. }
                | RemoveMarker { .. }
                | SetMarker { .. }
                | InsertLane { .. }
                | RemoveLane { .. }
                | SetLane { .. }
                | MoveLane { .. }
                | InsertMedia { .. }
                | RemoveMedia { .. }
                | SetMedia { .. }
                | InsertSequence { .. }
                | RemoveSequence { .. }
                | RenameSequence { .. }
                | SetActiveSequence { .. }
                | SetSettings { .. }
                | InsertPreset { .. }
                | RemovePreset { .. }
                | SetPreset { .. }
                | SetStory { .. }
                | Batch { .. }
        )
    }

    /// Whether the edit does nothing (an empty batch).
    pub fn is_empty(&self) -> bool {
        matches!(self, Edit::Batch { edits } if edits.iter().all(Edit::is_empty))
    }
}

impl Project {
    /// Applies an edit and returns its inverse. On error the project is
    /// left exactly as it was.
    pub fn apply(&mut self, edit: Edit) -> Result<Edit> {
        let mut tx = Tx { p: self, done: Vec::new() };
        let outcome = tx.run(edit).and_then(|()| tx.p.check_invariants().map_err(EditError::Invariant));
        let mut done = std::mem::take(&mut tx.done);
        match outcome {
            Ok(()) => {
                done.reverse();
                Ok(if done.len() == 1 { done.pop().expect("one edit") } else { Edit::Batch { edits: done } })
            }
            Err(e) => {
                // Roll back in reverse order; the inverses were valid when
                // recorded, so they cannot fail.
                while let Some(inv) = done.pop() {
                    let mut sink = Vec::new();
                    self.apply_primitive(inv, &mut sink).expect("rollback of a recorded inverse");
                }
                Err(e)
            }
        }
    }

    /// Applies one primitive (or a batch), pushing inverses onto `done` in
    /// application order.
    fn apply_primitive(&mut self, edit: Edit, done: &mut Vec<Edit>) -> Result<()> {
        use Edit::*;
        let inverse = match edit {
            Batch { edits } => {
                for e in edits {
                    if e.is_primitive() {
                        self.apply_primitive(e, done)?;
                    } else {
                        let mut tx = Tx { p: self, done: std::mem::take(done) };
                        let r = tx.run(e);
                        *done = tx.done;
                        r?;
                    }
                }
                return Ok(());
            }
            InsertClip { seq, clip } => {
                let s = seq_mut(self, seq)?;
                if s.clips.iter().any(|c| c.id == clip.id) {
                    return invalid(format!("clip id {} already used", clip.id.0));
                }
                let id = clip.id;
                s.clips.push(*clip);
                s.normalize();
                RemoveClip { seq, clip: id }
            }
            RemoveClip { seq, clip } => {
                let s = seq_mut(self, seq)?;
                let i = s.clips.iter().position(|c| c.id == clip).ok_or_else(|| nf(clip))?;
                let old = s.clips.remove(i);
                InsertClip { seq, clip: Box::new(old) }
            }
            SetClip { seq, clip } => {
                let s = seq_mut(self, seq)?;
                let slot = s.clips.iter_mut().find(|c| c.id == clip.id).ok_or_else(|| nf(clip.id))?;
                let old = std::mem::replace(slot, *clip);
                s.normalize();
                SetClip { seq, clip: Box::new(old) }
            }
            InsertTransition { seq, transition } => {
                let s = seq_mut(self, seq)?;
                if s.transitions.iter().any(|t| t.id == transition.id) {
                    return invalid("transition id already used");
                }
                let id = transition.id;
                s.transitions.push(transition);
                s.normalize();
                RemoveTransition { seq, id }
            }
            RemoveTransition { seq, id } => {
                let s = seq_mut(self, seq)?;
                let i = s.transitions.iter().position(|t| t.id == id).ok_or_else(|| nf(id))?;
                InsertTransition { seq, transition: s.transitions.remove(i) }
            }
            SetTransition { seq, transition } => {
                let s = seq_mut(self, seq)?;
                let slot = s.transitions.iter_mut().find(|t| t.id == transition.id).ok_or_else(|| nf(transition.id))?;
                SetTransition { seq, transition: std::mem::replace(slot, transition) }
            }
            InsertMarker { seq, marker } => {
                let s = seq_mut(self, seq)?;
                if s.markers.iter().any(|m| m.id == marker.id) {
                    return invalid("marker id already used");
                }
                let id = marker.id;
                s.markers.push(marker);
                s.normalize();
                RemoveMarker { seq, id }
            }
            RemoveMarker { seq, id } => {
                let s = seq_mut(self, seq)?;
                let i = s.markers.iter().position(|m| m.id == id).ok_or_else(|| nf(id))?;
                InsertMarker { seq, marker: s.markers.remove(i) }
            }
            SetMarker { seq, marker } => {
                let s = seq_mut(self, seq)?;
                let slot = s.markers.iter_mut().find(|m| m.id == marker.id).ok_or_else(|| nf(marker.id))?;
                let old = std::mem::replace(slot, marker);
                s.normalize();
                SetMarker { seq, marker: old }
            }
            InsertLane { seq, index, lane } => {
                let s = seq_mut(self, seq)?;
                if index > s.lanes.len() {
                    return invalid("lane index out of range");
                }
                let id = lane.id;
                s.lanes.insert(index, lane);
                RemoveLane { seq, lane: id }
            }
            RemoveLane { seq, lane } => {
                let s = seq_mut(self, seq)?;
                let i = s.lane_index(lane).ok_or_else(|| nf(lane))?;
                if s.clips.iter().any(|c| c.lane == lane) {
                    return invalid("lane still holds clips");
                }
                InsertLane { seq, index: i, lane: s.lanes.remove(i) }
            }
            SetLane { seq, lane } => {
                let s = seq_mut(self, seq)?;
                let slot = s.lanes.iter_mut().find(|l| l.id == lane.id).ok_or_else(|| nf(lane.id))?;
                if slot.kind != lane.kind {
                    return invalid("a lane's kind cannot change");
                }
                SetLane { seq, lane: std::mem::replace(slot, lane) }
            }
            MoveLane { seq, lane, to } => {
                let s = seq_mut(self, seq)?;
                let from = s.lane_index(lane).ok_or_else(|| nf(lane))?;
                if to >= s.lanes.len() {
                    return invalid("lane index out of range");
                }
                let l = s.lanes.remove(from);
                s.lanes.insert(to, l);
                MoveLane { seq, lane, to: from }
            }
            InsertMedia { item, index } => {
                if self.media.iter().any(|m| m.id == item.id) {
                    return invalid("media id already used");
                }
                let id = item.id;
                let i = index.unwrap_or(self.media.len()).min(self.media.len());
                self.media.insert(i, *item);
                RemoveMedia { id }
            }
            RemoveMedia { id } => {
                let i = self.media.iter().position(|m| m.id == id).ok_or_else(|| nf(id))?;
                InsertMedia { item: Box::new(self.media.remove(i)), index: Some(i) }
            }
            SetMedia { item } => {
                let slot = self.media.iter_mut().find(|m| m.id == item.id).ok_or_else(|| nf(item.id))?;
                SetMedia { item: Box::new(std::mem::replace(slot, *item)) }
            }
            InsertSequence { sequence, index } => {
                if self.sequences.iter().any(|s| s.id == sequence.id) {
                    return invalid("sequence id already used");
                }
                let id = sequence.id;
                let i = index.unwrap_or(self.sequences.len()).min(self.sequences.len());
                let mut sequence = *sequence;
                sequence.normalize();
                self.sequences.insert(i, sequence);
                RemoveSequence { id }
            }
            RemoveSequence { id } => {
                let i = self.sequences.iter().position(|s| s.id == id).ok_or_else(|| nf(id))?;
                InsertSequence { sequence: Box::new(self.sequences.remove(i)), index: Some(i) }
            }
            RenameSequence { id, name } => {
                let s = seq_mut(self, id)?;
                RenameSequence { id, name: std::mem::replace(&mut s.name, name) }
            }
            SetActiveSequence { id } => {
                if self.sequence(id).is_none() {
                    return Err(nf(id));
                }
                SetActiveSequence { id: std::mem::replace(&mut self.active_sequence, id) }
            }
            SetSettings { settings } => SetSettings { settings: Box::new(std::mem::replace(&mut self.settings, *settings)) },
            InsertPreset { preset } => {
                if self.presets.iter().any(|p| p.id == preset.id) {
                    return invalid("preset id already used");
                }
                let id = preset.id;
                self.presets.push(preset);
                RemovePreset { id }
            }
            RemovePreset { id } => {
                let i = self.presets.iter().position(|p| p.id == id).ok_or_else(|| nf(id))?;
                InsertPreset { preset: self.presets.remove(i) }
            }
            SetPreset { preset } => {
                let slot = self.presets.iter_mut().find(|p| p.id == preset.id).ok_or_else(|| nf(preset.id))?;
                SetPreset { preset: std::mem::replace(slot, preset) }
            }
            SetStory { story } => SetStory { story: Box::new(std::mem::replace(&mut self.story, *story)) },
            op => unreachable!("not a primitive: {op:?}"),
        };
        done.push(inverse);
        Ok(())
    }
}

fn seq_mut(p: &mut Project, id: SequenceId) -> Result<&mut Sequence> {
    p.sequence_mut(id).ok_or_else(|| nf(id))
}

pub(crate) fn nf(what: impl fmt::Display) -> EditError {
    EditError::NotFound(what.to_string())
}

/// An edit in progress: applies primitives immediately and records their
/// inverses so a failure can roll everything back.
pub(crate) struct Tx<'a> {
    pub(crate) p: &'a mut Project,
    pub(crate) done: Vec<Edit>,
}

impl Tx<'_> {
    /// Runs any edit: primitives directly, operations through `ops`.
    pub(crate) fn run(&mut self, edit: Edit) -> Result<()> {
        if edit.is_primitive() {
            self.p.apply_primitive(edit, &mut self.done)
        } else {
            self.operation(edit)
        }
    }

    /// Applies one primitive.
    pub(crate) fn prim(&mut self, edit: Edit) -> Result<()> {
        debug_assert!(edit.is_primitive());
        self.p.apply_primitive(edit, &mut self.done)
    }
}
