// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Editing operations, built from primitives.
//!
//! Each operation reads the current state through `self.p`, then changes it
//! only through `Tx::prim` (directly or via the helpers below), so every
//! step is recorded for undo and rollback. Operations finish with
//! `tidy`, which merges neighbouring spine gaps, drops a trailing gap and
//! fixes transitions the edit invalidated.

use super::{AttrSet, Edge, Edit, EditError, PasteMode, Result, Stack, Tx, invalid, nf};
use crate::clip::{Clip, ClipSource, MediaRef, SequenceRef, Speed};
use crate::clipboard::Clipboard;
use crate::id::*;
use crate::sequence::{Lane, LaneKind, Sequence, Transition, TransitionAlign};
use crate::time::{Micros, TimeRange};
use std::collections::{BTreeMap, HashMap, HashSet};

impl Tx<'_> {
    pub(super) fn operation(&mut self, edit: Edit) -> Result<()> {
        use Edit::*;
        let seq = match &edit {
            Insert { seq, .. }
            | Overwrite { seq, .. }
            | Append { seq, .. }
            | Split { seq, .. }
            | Trim { seq, .. }
            | RippleDelete { seq, .. }
            | Lift { seq, .. }
            | Move { seq, .. }
            | Reorder { seq, .. }
            | Slip { seq, .. }
            | Slide { seq, .. }
            | Roll { seq, .. }
            | Duplicate { seq, .. }
            | Paste { seq, .. }
            | PasteAttributes { seq, .. }
            | Group { seq, .. }
            | Ungroup { seq, .. }
            | Link { seq, .. }
            | Unlink { seq, .. }
            | DetachAudio { seq, .. }
            | ReplaceMedia { seq, .. }
            | FreezeFrame { seq, .. }
            | Reverse { seq, .. }
            | SetSpeed { seq, .. }
            | AddTransition { seq, .. }
            | AddEffect { seq, .. }
            | RemoveEffect { seq, .. }
            | SetEffect { seq, .. }
            | MoveEffect { seq, .. }
            | SetGrade { seq, .. }
            | AddMask { seq, .. }
            | RemoveMask { seq, .. }
            | SetMask { seq, .. }
            | SetKeyframe { seq, .. }
            | RemoveKeyframe { seq, .. }
            | SetTrack { seq, .. }
            | AddMarker { seq, .. }
            | DeleteMarker { seq, .. }
            | ChangeMarker { seq, .. }
            | AddLane { seq, .. }
            | DeleteLane { seq, .. }
            | CreateCompound { seq, .. }
            | ExpandCompound { seq, .. } => *seq,
            other => unreachable!("primitive passed to operation: {other:?}"),
        };
        self.seq(seq)?;
        match edit {
            Insert { lane, at, clip, .. } => self.insert(seq, lane, at, *clip),
            Overwrite { lane, at, clip, .. } => self.overwrite(seq, lane, at, *clip),
            Append { lane, clip, .. } => {
                let lane = self.lane_or_spine(seq, lane)?;
                let at = self.seq(seq)?.clips_on(lane).last().map_or(0, |c| c.end());
                self.insert(seq, Some(lane), at, *clip)
            }
            Split { at, clips, .. } => self.split(seq, at, clips),
            Trim { clip, edge, delta, ripple, .. } => self.trim(seq, clip, edge, delta, ripple),
            RippleDelete { clips, .. } => self.ripple_delete(seq, &clips),
            Lift { clips, .. } => self.lift(seq, &clips),
            Move { clips, delta, to_lane, .. } => self.move_clips(seq, &clips, delta, to_lane),
            Reorder { clip, before, .. } => self.reorder(seq, clip, before),
            Slip { clip, delta, .. } => self.slip(seq, clip, delta),
            Slide { clip, delta, .. } => self.slide(seq, clip, delta),
            Roll { left, delta, .. } => self.roll(seq, left, delta),
            Duplicate { clips, .. } => {
                let cb = Clipboard::copy(self.p, seq, &clips).map_err(|e| EditError::Invalid(e.to_string()))?;
                let at = self.seq(seq)?.clips.iter().filter(|c| clips.contains(&c.id)).map(|c| c.end()).max().unwrap_or(0);
                self.paste(seq, at, &cb, PasteMode::Insert)
            }
            Paste { at, clipboard, mode, .. } => self.paste(seq, at, &clipboard, mode),
            PasteAttributes { targets, source, attrs, .. } => self.paste_attributes(seq, &targets, &source, attrs),
            Group { clips, group, .. } => {
                let g = GroupId(self.fresh_id(group.0));
                self.modify_clips(seq, &clips, |c| c.group = Some(g))
            }
            Ungroup { clips, .. } => self.modify_clips(seq, &clips, |c| c.group = None),
            Link { clips, link, .. } => {
                let l = LinkId(self.fresh_id(link.0));
                self.modify_clips(seq, &clips, |c| c.link = Some(l))
            }
            Unlink { clips, .. } => self.modify_clips(seq, &clips, |c| c.link = None),
            DetachAudio { clip, new_clip, .. } => self.detach_audio(seq, clip, new_clip),
            ReplaceMedia { clip, media, .. } => self.replace_media(seq, clip, media),
            FreezeFrame { clip, at, duration, new_clip, .. } => self.freeze_frame(seq, clip, at, duration, new_clip),
            Reverse { clips, .. } => {
                let mut all = clips.clone();
                all.extend(self.partners(seq, &clips)?);
                for id in all {
                    let c = self.editable(seq, id)?;
                    if !c.has_source_time() {
                        continue;
                    }
                    self.modify(seq, id, |c| c.reversed = !c.reversed)?;
                }
                self.tidy(seq)
            }
            SetSpeed { clip, speed, .. } => self.set_speed(seq, clip, speed),
            AddTransition { transition, .. } => self.add_transition(seq, transition),
            AddEffect { clip, stack, mut effect, index, .. } => {
                let c = self.editable(seq, clip)?;
                let ids: HashSet<u64> = c.effects.iter().chain(&c.audio.effects).map(|e| e.id.0).chain(c.masks.iter().map(|m| m.id.0)).collect();
                if effect.id.0 == 0 || ids.contains(&effect.id.0) {
                    effect.id = EffectId(self.p.alloc_id());
                }
                self.modify(seq, clip, |c| {
                    let list = match stack {
                        Stack::Video => &mut c.effects,
                        Stack::Audio => &mut c.audio.effects,
                    };
                    let i = index.unwrap_or(list.len()).min(list.len());
                    list.insert(i, effect);
                })?;
                Ok(())
            }
            RemoveEffect { clip, effect, .. } => {
                let c = self.editable(seq, clip)?;
                let prefix = if c.effects.iter().any(|e| e.id == effect) {
                    format!("effect.{}.", effect.0)
                } else if c.audio.effects.iter().any(|e| e.id == effect) {
                    format!("audio_effect.{}.", effect.0)
                } else {
                    return Err(nf(effect));
                };
                self.modify(seq, clip, |c| {
                    c.effects.retain(|e| e.id != effect);
                    c.audio.effects.retain(|e| e.id != effect);
                    c.drop_tracks_with_prefix(&prefix);
                })
            }
            SetEffect { clip, effect, .. } => {
                let c = self.editable(seq, clip)?;
                if !c.effects.iter().chain(&c.audio.effects).any(|e| e.id == effect.id) {
                    return Err(nf(effect.id));
                }
                self.modify(seq, clip, |c| {
                    for e in c.effects.iter_mut().chain(c.audio.effects.iter_mut()) {
                        if e.id == effect.id {
                            *e = effect.clone();
                        }
                    }
                })
            }
            MoveEffect { clip, effect, to, .. } => {
                let c = self.editable(seq, clip)?;
                if !c.effects.iter().chain(&c.audio.effects).any(|e| e.id == effect) {
                    return Err(nf(effect));
                }
                self.modify(seq, clip, |c| {
                    for list in [&mut c.effects, &mut c.audio.effects] {
                        if let Some(i) = list.iter().position(|e| e.id == effect) {
                            let e = list.remove(i);
                            let to = to.min(list.len());
                            list.insert(to, e);
                        }
                    }
                })
            }
            SetGrade { clip, grade, .. } => {
                self.editable(seq, clip)?;
                self.modify(seq, clip, |c| {
                    if grade.is_none() {
                        c.drop_tracks_with_prefix("grade.");
                    }
                    c.grade = grade;
                })
            }
            AddMask { clip, mut mask, index, .. } => {
                let c = self.editable(seq, clip)?;
                let ids: HashSet<u64> = c.effects.iter().chain(&c.audio.effects).map(|e| e.id.0).chain(c.masks.iter().map(|m| m.id.0)).collect();
                if mask.id.0 == 0 || ids.contains(&mask.id.0) {
                    mask.id = MaskId(self.p.alloc_id());
                }
                self.modify(seq, clip, |c| {
                    let i = index.unwrap_or(c.masks.len()).min(c.masks.len());
                    c.masks.insert(i, mask);
                })
            }
            RemoveMask { clip, mask, .. } => {
                let c = self.editable(seq, clip)?;
                if !c.masks.iter().any(|m| m.id == mask) {
                    return Err(nf(mask));
                }
                self.modify(seq, clip, |c| {
                    c.masks.retain(|m| m.id != mask);
                    c.drop_tracks_with_prefix(&format!("mask.{}.", mask.0));
                })
            }
            SetMask { clip, mask, .. } => {
                let c = self.editable(seq, clip)?;
                if !c.masks.iter().any(|m| m.id == mask.id) {
                    return Err(nf(mask.id));
                }
                self.modify(seq, clip, |c| {
                    if let Some(m) = c.masks.iter_mut().find(|m| m.id == mask.id) {
                        *m = mask;
                    }
                })
            }
            SetKeyframe { clip, property, key, .. } => {
                let mut c = self.editable(seq, clip)?;
                if !key.value.is_animatable() {
                    return invalid("value cannot be keyframed");
                }
                match c.get_property(&property) {
                    Some(v) if !v.same_kind(&key.value) => return invalid("keyframe value has the wrong kind"),
                    Some(_) => {}
                    // A parameter without a stored value: store the key's
                    // value as the static one so the track has a home.
                    None => {
                        if !c.set_property(&property, key.value.clone()) {
                            return Err(nf(format!("property `{property}`")));
                        }
                    }
                }
                c.keyframes.entry(property).or_default().set_key(key);
                self.set_clip(seq, c)
            }
            RemoveKeyframe { clip, property, time, .. } => {
                let mut c = self.editable(seq, clip)?;
                let tr = c.keyframes.get_mut(&property).ok_or_else(|| nf(format!("track `{property}`")))?;
                tr.remove_key(time).ok_or_else(|| nf(format!("key at {time}")))?;
                if tr.keys.is_empty() {
                    c.keyframes.remove(&property);
                }
                self.set_clip(seq, c)
            }
            SetTrack { clip, property, track, .. } => {
                let mut c = self.editable(seq, clip)?;
                match track {
                    Some(tr) => {
                        if c.get_property(&property).is_none() {
                            return Err(nf(format!("property `{property}`")));
                        }
                        c.keyframes.insert(property, tr);
                    }
                    None => {
                        c.keyframes.remove(&property);
                    }
                }
                self.set_clip(seq, c)
            }
            AddMarker { clip, mut marker, .. } => {
                marker.id = MarkerId(self.fresh_id(marker.id.0));
                match clip {
                    None => self.prim(Edit::InsertMarker { seq, marker }),
                    Some(id) => {
                        self.editable(seq, id)?;
                        self.modify(seq, id, |c| c.markers.push(marker))
                    }
                }
            }
            DeleteMarker { clip, id, .. } => match clip {
                None => self.prim(Edit::RemoveMarker { seq, id }),
                Some(cid) => {
                    let c = self.editable(seq, cid)?;
                    if !c.markers.iter().any(|m| m.id == id) {
                        return Err(nf(id));
                    }
                    self.modify(seq, cid, |c| c.markers.retain(|m| m.id != id))
                }
            },
            ChangeMarker { clip, marker, .. } => match clip {
                None => self.prim(Edit::SetMarker { seq, marker }),
                Some(cid) => {
                    let c = self.editable(seq, cid)?;
                    if !c.markers.iter().any(|m| m.id == marker.id) {
                        return Err(nf(marker.id));
                    }
                    self.modify(seq, cid, |c| {
                        if let Some(m) = c.markers.iter_mut().find(|m| m.id == marker.id) {
                            *m = marker;
                        }
                    })
                }
            },
            AddLane { mut lane, index, .. } => {
                if lane.kind == LaneKind::Spine {
                    return invalid("a sequence has exactly one spine");
                }
                lane.id = LaneId(self.fresh_id(lane.id.0));
                let len = self.seq(seq)?.lanes.len();
                let index = index.unwrap_or(if lane.kind == LaneKind::Visual { 0 } else { len });
                self.prim(Edit::InsertLane { seq, index, lane })
            }
            DeleteLane { lane, .. } => {
                let s = self.seq(seq)?;
                let l = s.lane(lane).ok_or_else(|| nf(lane))?;
                if l.kind == LaneKind::Spine {
                    return invalid("the spine cannot be deleted");
                }
                let ids: Vec<ClipId> = s.clips_on(lane).iter().map(|c| c.id).collect();
                for id in ids {
                    self.remove_clip(seq, id)?;
                }
                self.prim(Edit::RemoveLane { seq, lane })
            }
            CreateCompound { clips, name, new_sequence, new_clip, .. } => self.create_compound(seq, &clips, name, new_sequence, new_clip),
            ExpandCompound { clip, .. } => self.expand_compound(seq, clip),
            other => unreachable!("primitive passed to operation: {other:?}"),
        }
    }

    // ------------------------------------------------------------- lookups

    fn seq(&self, id: SequenceId) -> Result<&Sequence> {
        self.p.sequence(id).ok_or_else(|| nf(id))
    }

    fn clip(&self, seq: SequenceId, id: ClipId) -> Result<Clip> {
        self.seq(seq)?.clip(id).cloned().ok_or_else(|| nf(id))
    }

    /// The clip, if its lane is not locked.
    fn editable(&self, seq: SequenceId, id: ClipId) -> Result<Clip> {
        let c = self.clip(seq, id)?;
        self.unlocked(seq, c.lane)?;
        Ok(c)
    }

    fn unlocked(&self, seq: SequenceId, lane: LaneId) -> Result<()> {
        let l = self.seq(seq)?.lane(lane).ok_or_else(|| nf(lane))?;
        if l.locked { Err(EditError::Locked(lane)) } else { Ok(()) }
    }

    fn lane_or_spine(&self, seq: SequenceId, lane: Option<LaneId>) -> Result<LaneId> {
        let s = self.seq(seq)?;
        let id = lane.unwrap_or_else(|| s.spine_id());
        s.lane(id).ok_or_else(|| nf(id))?;
        self.unlocked(seq, id)?;
        Ok(id)
    }

    fn spine(&self, seq: SequenceId) -> Result<LaneId> {
        Ok(self.seq(seq)?.spine_id())
    }

    /// Whether any object in the project uses `id`.
    fn id_used(&self, id: u64) -> bool {
        let p = &*self.p;
        p.media.iter().any(|m| m.id.0 == id)
            || p.presets.iter().any(|x| x.id.0 == id)
            || p.sequences.iter().any(|s| {
                s.id.0 == id
                    || s.lanes.iter().any(|l| l.id.0 == id)
                    || s.transitions.iter().any(|t| t.id.0 == id)
                    || s.markers.iter().any(|m| m.id.0 == id)
                    || s.clips.iter().any(|c| c.id.0 == id || c.link.is_some_and(|l| l.0 == id) || c.group.is_some_and(|g| g.0 == id) || c.markers.iter().any(|m| m.id.0 == id))
            })
    }

    /// `want` if it is non-zero and unused, otherwise a new id.
    pub(crate) fn fresh_id(&mut self, want: u64) -> u64 {
        if want != 0 && !self.id_used(want) {
            // Keep the counter ahead of ids chosen by the caller.
            self.p.next_id = self.p.next_id.max(want + 1);
            want
        } else {
            self.p.alloc_id()
        }
    }

    /// Clips linked to any of `ids` (excluding `ids` themselves).
    fn partners(&self, seq: SequenceId, ids: &[ClipId]) -> Result<Vec<ClipId>> {
        let s = self.seq(seq)?;
        let mut links = HashSet::new();
        for id in ids {
            if let Some(l) = s.clip(*id).ok_or_else(|| nf(*id))?.link {
                links.insert(l);
            }
        }
        Ok(s.clips.iter().filter(|c| c.link.is_some_and(|l| links.contains(&l)) && !ids.contains(&c.id)).map(|c| c.id).collect())
    }

    // ---------------------------------------------------- primitive helpers

    fn set_clip(&mut self, seq: SequenceId, clip: Clip) -> Result<()> {
        self.prim(Edit::SetClip { seq, clip: Box::new(clip) })
    }

    fn modify(&mut self, seq: SequenceId, id: ClipId, f: impl FnOnce(&mut Clip)) -> Result<()> {
        let mut c = self.clip(seq, id)?;
        f(&mut c);
        self.set_clip(seq, c)
    }

    fn modify_clips(&mut self, seq: SequenceId, ids: &[ClipId], f: impl Fn(&mut Clip)) -> Result<()> {
        for id in ids {
            self.editable(seq, *id)?;
            self.modify(seq, *id, &f)?;
        }
        Ok(())
    }

    fn insert_clip(&mut self, seq: SequenceId, mut clip: Clip) -> Result<ClipId> {
        clip.id = ClipId(self.fresh_id(clip.id.0));
        let id = clip.id;
        self.prim(Edit::InsertClip { seq, clip: Box::new(clip) })?;
        Ok(id)
    }

    fn insert_gap(&mut self, seq: SequenceId, start: Micros, duration: Micros) -> Result<()> {
        if duration <= 0 {
            return Ok(());
        }
        let spine = self.spine(seq)?;
        self.insert_clip(seq, Clip::gap(ClipId(0), spine, start, duration)).map(|_| ())
    }

    /// Removes a clip together with the transitions attached to it.
    fn remove_clip(&mut self, seq: SequenceId, id: ClipId) -> Result<Clip> {
        let c = self.clip(seq, id)?;
        let ts: Vec<TransitionId> = self.seq(seq)?.transitions_of(id).map(|t| t.id).collect();
        for t in ts {
            self.prim(Edit::RemoveTransition { seq, id: t })?;
        }
        self.prim(Edit::RemoveClip { seq, clip: id })?;
        Ok(c)
    }

    /// Adds a gap to the spine so it reaches `t`.
    fn fill_spine_to(&mut self, seq: SequenceId, t: Micros) -> Result<()> {
        let end = self.seq(seq)?.spine_end();
        if t > end { self.insert_gap(seq, end, t - end) } else { Ok(()) }
    }

    /// Splits clip `id` at sequence time `at`; returns the new right half.
    /// Does nothing when `at` is not strictly inside the clip.
    fn split_at(&mut self, seq: SequenceId, id: ClipId, at: Micros) -> Result<Option<ClipId>> {
        let c = self.clip(seq, id)?;
        if at <= c.start || at >= c.end() {
            return Ok(None);
        }
        let dl = at - c.start;
        let mut left = c.clone();
        let mut right = c.clone();
        left.duration = dl;
        right.id = ClipId(0);
        right.start = at;
        right.duration = c.duration - dl;
        if c.has_source_time() {
            let off = c.source_offset(dl).round() as Micros;
            if c.reversed {
                // The left half plays the top of the source range backwards.
                left.source_in = c.source_out() - off;
                right.source_in = c.source_in;
            } else {
                right.source_in = c.source_in + off;
            }
        }
        if let Speed::Curve(tr) = &mut right.speed {
            tr.shift(-dl);
        }
        for tr in right.keyframes.values_mut() {
            tr.shift(-dl);
        }
        left.markers.retain(|m| m.time < dl);
        right.markers.retain(|m| m.time >= dl);
        for m in &mut right.markers {
            m.time -= dl;
        }
        left.audio.fade_out = Default::default();
        right.audio.fade_in = Default::default();
        left.motion.exit = None;
        right.motion.entry = None;
        self.set_clip(seq, left)?;
        let rid = self.insert_clip(seq, right)?;
        // A transition leaving the original now leaves the right half.
        let outgoing: Vec<Transition> = self.seq(seq)?.transitions.iter().filter(|t| t.left == id).cloned().collect();
        for mut t in outgoing {
            t.left = rid;
            self.prim(Edit::SetTransition { seq, transition: t })?;
        }
        Ok(Some(rid))
    }

    /// Splits whatever clip on `lane` straddles `at`.
    fn split_lane_at(&mut self, seq: SequenceId, lane: LaneId, at: Micros) -> Result<()> {
        let hit = self.seq(seq)?.clips.iter().find(|c| c.lane == lane && c.start < at && c.end() > at).map(|c| c.id);
        if let Some(id) = hit {
            self.split_at(seq, id, at)?;
        }
        Ok(())
    }

    /// Shifts every clip on `lane` starting at or after `from` by `delta`.
    /// When the lane is the spine, clips linked to shifted spine clips move
    /// too, so detached sound stays in sync with its picture.
    fn ripple(&mut self, seq: SequenceId, lane: LaneId, from: Micros, delta: Micros, exclude: &HashSet<ClipId>) -> Result<()> {
        if delta == 0 {
            return Ok(());
        }
        let s = self.seq(seq)?;
        let moved: Vec<Clip> = s.clips.iter().filter(|c| c.lane == lane && c.start >= from && !exclude.contains(&c.id)).cloned().collect();
        let mut done: HashSet<ClipId> = moved.iter().map(|c| c.id).collect();
        let mut extra = Vec::new();
        if lane == s.spine_id() {
            let links: HashSet<LinkId> = moved.iter().filter_map(|c| c.link).collect();
            for c in &s.clips {
                if c.lane != lane && c.link.is_some_and(|l| links.contains(&l)) && !exclude.contains(&c.id) && done.insert(c.id) {
                    extra.push(c.clone());
                }
            }
        }
        for mut c in moved.into_iter().chain(extra) {
            if c.lane != lane {
                self.unlocked(seq, c.lane)?;
            }
            c.start += delta;
            if c.start < 0 {
                return invalid("clips would move before the sequence start");
            }
            self.set_clip(seq, c)?;
        }
        Ok(())
    }

    /// Empties `range` on `lane`, cutting clips that stick out of it.
    fn clear_range(&mut self, seq: SequenceId, lane: LaneId, range: TimeRange) -> Result<()> {
        self.split_lane_at(seq, lane, range.start)?;
        self.split_lane_at(seq, lane, range.end())?;
        let inside: Vec<ClipId> = self.seq(seq)?.clips.iter().filter(|c| c.lane == lane && c.start >= range.start && c.end() <= range.end()).map(|c| c.id).collect();
        for id in inside {
            self.unlocked(seq, lane)?;
            self.remove_clip(seq, id)?;
        }
        Ok(())
    }

    /// Lane at position `pos` from the spine, creating lanes as needed.
    fn ensure_lane_at(&mut self, seq: SequenceId, pos: i32) -> Result<LaneId> {
        loop {
            if let Some(l) = self.seq(seq)?.lane_at_position(pos) {
                return Ok(l.id);
            }
            let s = self.seq(seq)?;
            let (kind, index, count) = if pos > 0 {
                let n = s.lanes.iter().filter(|l| l.kind == LaneKind::Visual).count();
                (LaneKind::Visual, 0, n)
            } else {
                let n = s.lanes.iter().filter(|l| l.kind == LaneKind::Audio).count();
                (LaneKind::Audio, s.lanes.len(), n)
            };
            let name = if kind == LaneKind::Visual { format!("Video {}", count + 1) } else { format!("Audio {}", count + 1) };
            let id = LaneId(self.p.alloc_id());
            self.prim(Edit::InsertLane { seq, index, lane: Lane::new(id, kind, name) })?;
        }
    }

    /// Clean-up after an operation: merges neighbouring spine gaps, drops a
    /// trailing gap, and removes or shortens transitions the edit broke.
    fn tidy(&mut self, seq: SequenceId) -> Result<()> {
        // Merge gaps.
        loop {
            let s = self.seq(seq)?;
            let spine = s.spine_clips();
            let pair = spine.windows(2).find(|w| w[0].is_gap() && w[1].is_gap() && w[0].end() == w[1].start).map(|w| (w[0].clone(), w[1].id));
            let Some((mut a, b)) = pair else { break };
            let removed = self.remove_clip(seq, b)?;
            a.duration += removed.duration;
            self.set_clip(seq, a)?;
        }
        // Trailing gaps.
        while let Some(last) = self.seq(seq)?.spine_clips().last().filter(|c| c.is_gap()).map(|c| c.id) {
            self.remove_clip(seq, last)?;
        }
        // Transitions.
        let ids: Vec<TransitionId> = self.seq(seq)?.transitions.iter().map(|t| t.id).collect();
        for id in ids {
            let s = self.seq(seq)?;
            let t = s.transition(id).expect("listed above").clone();
            let spine = s.spine_id();
            let adjacent = match (s.clip(t.left), s.clip(t.right)) {
                (Some(l), Some(r)) => l.lane == spine && r.lane == spine && l.end() == r.start,
                _ => false,
            };
            if !adjacent {
                self.prim(Edit::RemoveTransition { seq, id })?;
                continue;
            }
            let max = self.p.transition_limit(s, t.left, t.right, t.align, Some(id));
            if max <= 0 {
                self.prim(Edit::RemoveTransition { seq, id })?;
            } else if t.duration > max {
                let mut t = t;
                t.duration = max;
                self.prim(Edit::SetTransition { seq, transition: t })?;
            }
        }
        Ok(())
    }

    // ----------------------------------------------------------- operations

    fn insert(&mut self, seq: SequenceId, lane: Option<LaneId>, at: Micros, mut clip: Clip) -> Result<()> {
        let lane = self.lane_or_spine(seq, lane)?;
        if at < 0 || clip.duration <= 0 {
            return invalid("insert needs a non-negative time and a positive duration");
        }
        if lane == self.spine(seq)? {
            self.fill_spine_to(seq, at)?;
        }
        self.split_lane_at(seq, lane, at)?;
        self.ripple(seq, lane, at, clip.duration, &HashSet::new())?;
        clip.lane = lane;
        clip.start = at;
        self.insert_clip(seq, clip)?;
        self.tidy(seq)
    }

    fn overwrite(&mut self, seq: SequenceId, lane: Option<LaneId>, at: Micros, mut clip: Clip) -> Result<()> {
        let lane = self.lane_or_spine(seq, lane)?;
        if at < 0 || clip.duration <= 0 {
            return invalid("overwrite needs a non-negative time and a positive duration");
        }
        if lane == self.spine(seq)? {
            self.fill_spine_to(seq, at)?;
        }
        self.clear_range(seq, lane, TimeRange::new(at, clip.duration))?;
        clip.lane = lane;
        clip.start = at;
        self.insert_clip(seq, clip)?;
        self.tidy(seq)
    }

    fn split(&mut self, seq: SequenceId, at: Micros, clips: Option<Vec<ClipId>>) -> Result<()> {
        let s = self.seq(seq)?;
        let targets: Vec<ClipId> = match clips {
            Some(ids) => ids,
            None => s.clips.iter().filter(|c| !c.is_gap() && c.start < at && c.end() > at && !s.lane(c.lane).is_some_and(|l| l.locked)).map(|c| c.id).collect(),
        };
        let mut halves: BTreeMap<LinkId, Vec<ClipId>> = BTreeMap::new();
        let mut any = false;
        for id in targets {
            let c = self.editable(seq, id)?;
            if let Some(r) = self.split_at(seq, id, at)? {
                any = true;
                if let Some(l) = c.link {
                    halves.entry(l).or_default().push(r);
                }
            }
        }
        if !any {
            return invalid("nothing to split at that time");
        }
        // Right halves of a linked set form a new linked set.
        for (_, rs) in halves {
            let link = if rs.len() > 1 { Some(LinkId(self.p.alloc_id())) } else { None };
            for r in rs {
                self.modify(seq, r, |c| c.link = link)?;
            }
        }
        self.tidy(seq)
    }

    fn trim(&mut self, seq: SequenceId, id: ClipId, edge: Edge, delta: Micros, ripple: bool) -> Result<()> {
        let c = self.editable(seq, id)?;
        if delta == 0 {
            return Ok(());
        }
        let keep_start = ripple && edge == Edge::Start;
        let new = trim_local(self.p, &c, edge, delta, keep_start)?;
        let spine = self.spine(seq)?;
        // Partners whose matching edge lines up with this clip's edge.
        let partners: Vec<Clip> = {
            let ids = self.partners(seq, &[id])?;
            let s = self.seq(seq)?;
            ids.iter()
                .filter_map(|p| s.clip(*p))
                .filter(|p| match edge {
                    Edge::Start => p.start == c.start,
                    Edge::End => p.end() == c.end(),
                })
                .cloned()
                .collect()
        };
        let shift = new.end() - c.end();
        let no_room = || invalid("no room next to the clip; trim with ripple or roll the cut");
        if ripple || c.lane != spine {
            self.set_clip(seq, new)?;
            if ripple {
                self.ripple(seq, c.lane, c.end(), shift, &HashSet::from([id]))?;
            }
        } else {
            match edge {
                Edge::End if delta < 0 => {
                    self.set_clip(seq, new)?;
                    self.insert_gap(seq, c.end() + delta, -delta)?;
                }
                Edge::End => {
                    if let Some(next) = self.seq(seq)?.next_on_spine(id).cloned() {
                        if !next.is_gap() || next.duration < delta {
                            return no_room();
                        }
                        self.remove_clip(seq, next.id)?;
                        self.set_clip(seq, new)?;
                        self.insert_gap(seq, next.start + delta, next.duration - delta)?;
                    } else {
                        self.set_clip(seq, new)?;
                    }
                }
                Edge::Start if delta > 0 => {
                    self.set_clip(seq, new)?;
                    self.insert_gap(seq, c.start, delta)?;
                }
                Edge::Start => {
                    let prev = self.seq(seq)?.spine_clips().into_iter().find(|p| p.end() == c.start).cloned();
                    let Some(prev) = prev.filter(|p| p.is_gap() && p.duration >= -delta) else { return no_room() };
                    self.remove_clip(seq, prev.id)?;
                    self.set_clip(seq, new)?;
                    self.insert_gap(seq, prev.start, prev.duration + delta)?;
                }
            }
        }
        for p in partners {
            self.unlocked(seq, p.lane)?;
            let np = trim_local(self.p, &p, edge, delta, keep_start)?;
            self.set_clip(seq, np)?;
        }
        self.tidy(seq)
    }

    fn ripple_delete(&mut self, seq: SequenceId, ids: &[ClipId]) -> Result<()> {
        for p in self.partners(seq, ids)? {
            self.editable(seq, p)?;
            self.remove_clip(seq, p)?;
        }
        let mut sel: Vec<Clip> = ids.iter().map(|id| self.editable(seq, *id)).collect::<Result<_>>()?;
        // Latest first, so each ripple only moves clips not yet processed.
        sel.sort_by_key(|c| std::cmp::Reverse(c.start));
        for c in sel {
            self.remove_clip(seq, c.id)?;
            self.ripple(seq, c.lane, c.end(), -c.duration, &HashSet::new())?;
        }
        self.tidy(seq)
    }

    fn lift(&mut self, seq: SequenceId, ids: &[ClipId]) -> Result<()> {
        let mut all = ids.to_vec();
        all.extend(self.partners(seq, ids)?);
        let spine = self.spine(seq)?;
        for id in all {
            let c = self.editable(seq, id)?;
            if c.is_gap() {
                continue;
            }
            self.remove_clip(seq, id)?;
            if c.lane == spine {
                self.insert_gap(seq, c.start, c.duration)?;
            }
        }
        self.tidy(seq)
    }

    fn move_clips(&mut self, seq: SequenceId, ids: &[ClipId], delta: Micros, to_lane: Option<LaneId>) -> Result<()> {
        if let Some(l) = to_lane {
            self.unlocked(seq, l)?;
        }
        let partners = self.partners(seq, ids)?;
        let spine = self.spine(seq)?;
        let mut originals = Vec::new();
        for (id, selected) in ids.iter().map(|i| (*i, true)).chain(partners.into_iter().map(|i| (i, false))) {
            let c = self.editable(seq, id)?;
            if c.is_gap() {
                return invalid("gap clips cannot be moved");
            }
            self.remove_clip(seq, id)?;
            if c.lane == spine {
                self.insert_gap(seq, c.start, c.duration)?;
            }
            originals.push((c, selected));
        }
        for (mut c, selected) in originals {
            if selected && let Some(l) = to_lane {
                c.lane = l;
            }
            c.start += delta;
            if c.start < 0 {
                return invalid("clips would move before the sequence start");
            }
            if c.lane == spine {
                self.fill_spine_to(seq, c.start)?;
            }
            self.clear_range(seq, c.lane, c.range())?;
            self.prim(Edit::InsertClip { seq, clip: Box::new(c) })?;
        }
        self.tidy(seq)
    }

    fn reorder(&mut self, seq: SequenceId, id: ClipId, before: Option<ClipId>) -> Result<()> {
        let c = self.editable(seq, id)?;
        let spine = self.spine(seq)?;
        if c.lane != spine || before.is_some_and(|b| self.seq(seq).ok().and_then(|s| s.clip(b)).is_none_or(|b| b.lane != spine)) {
            return invalid("reorder works on spine clips");
        }
        if before == Some(id) {
            return Ok(());
        }
        let partners = self.partners(seq, &[id])?;
        self.remove_clip(seq, id)?;
        self.ripple(seq, spine, c.end(), -c.duration, &HashSet::new())?;
        let at = match before {
            Some(b) => self.clip(seq, b)?.start,
            None => self.seq(seq)?.spine_end(),
        };
        self.ripple(seq, spine, at, c.duration, &HashSet::new())?;
        let shift = at - c.start;
        let mut moved = c;
        moved.start = at;
        self.prim(Edit::InsertClip { seq, clip: Box::new(moved) })?;
        for p in partners {
            self.editable(seq, p)?;
            self.modify(seq, p, |p| p.start += shift)?;
        }
        self.tidy(seq)
    }

    fn slip(&mut self, seq: SequenceId, id: ClipId, delta: Micros) -> Result<()> {
        let mut all = vec![id];
        all.extend(self.partners(seq, &[id])?);
        for cid in all {
            let mut c = self.editable(seq, cid)?;
            if !c.has_source_time() {
                if cid == id {
                    return invalid("only media and compound clips can slip");
                }
                continue;
            }
            c.source_in += delta;
            check_source(self.p, &c)?;
            self.set_clip(seq, c)?;
        }
        self.tidy(seq)
    }

    fn slide(&mut self, seq: SequenceId, id: ClipId, delta: Micros) -> Result<()> {
        let c = self.editable(seq, id)?;
        if delta == 0 {
            return Ok(());
        }
        let s = self.seq(seq)?;
        let spine = s.spine_id();
        let prev = s.clips.iter().find(|p| p.lane == c.lane && p.end() == c.start && p.id != id).cloned();
        let next = s.clips.iter().find(|n| n.lane == c.lane && n.start == c.end() && n.id != id).cloned();
        if c.lane == spine && prev.is_none() {
            return invalid("the first spine clip has nothing to slide against");
        }
        let mut moved = c.clone();
        moved.start += delta;
        if moved.start < 0 {
            return invalid("clip would move before the sequence start");
        }
        let mut changes = vec![moved];
        if let Some(p) = prev {
            self.unlocked(seq, p.lane)?;
            changes.push(trim_local(self.p, &p, Edge::End, delta, false)?);
        }
        if let Some(n) = next {
            changes.push(trim_local(self.p, &n, Edge::Start, delta, false)?);
        }
        for ch in changes {
            self.set_clip(seq, ch)?;
        }
        for p in self.partners(seq, &[id])? {
            self.editable(seq, p)?;
            self.modify(seq, p, |p| p.start += delta)?;
        }
        self.tidy(seq)
    }

    fn roll(&mut self, seq: SequenceId, left: ClipId, delta: Micros) -> Result<()> {
        let l = self.editable(seq, left)?;
        if delta == 0 {
            return Ok(());
        }
        let r = self.seq(seq)?.clips.iter().find(|r| r.lane == l.lane && r.start == l.end() && r.id != left).cloned().ok_or_else(|| EditError::Invalid("no clip after the cut".into()))?;
        let nl = trim_local(self.p, &l, Edge::End, delta, false)?;
        let nr = trim_local(self.p, &r, Edge::Start, delta, false)?;
        let lp: Vec<Clip> = self.partners(seq, &[l.id])?.iter().filter_map(|p| self.seq(seq).ok()?.clip(*p).cloned()).filter(|p| p.end() == l.end()).collect();
        let rp: Vec<Clip> = self.partners(seq, &[r.id])?.iter().filter_map(|p| self.seq(seq).ok()?.clip(*p).cloned()).filter(|p| p.start == r.start).collect();
        self.set_clip(seq, nl)?;
        self.set_clip(seq, nr)?;
        for p in lp {
            self.unlocked(seq, p.lane)?;
            let np = trim_local(self.p, &p, Edge::End, delta, false)?;
            self.set_clip(seq, np)?;
        }
        for p in rp {
            self.unlocked(seq, p.lane)?;
            let np = trim_local(self.p, &p, Edge::Start, delta, false)?;
            self.set_clip(seq, np)?;
        }
        self.tidy(seq)
    }

    fn paste(&mut self, seq: SequenceId, at: Micros, cb: &Clipboard, mode: PasteMode) -> Result<()> {
        if at < 0 {
            return invalid("cannot paste before the sequence start");
        }
        if cb.clips.is_empty() {
            return Ok(());
        }
        // Media: reuse items with the same path, add the rest.
        let mut media_map: HashMap<MediaId, MediaId> = HashMap::new();
        for m in &cb.media {
            let existing = self.p.media.iter().find(|x| x.path == m.path).map(|x| x.id);
            let id = match existing {
                Some(id) => id,
                None => {
                    let mut item = m.clone();
                    item.id = MediaId(self.fresh_id(m.id.0));
                    let id = item.id;
                    self.prim(Edit::InsertMedia { item: Box::new(item), index: None })?;
                    id
                }
            };
            media_map.insert(m.id, id);
        }
        // Fresh link and group ids, shared within the pasted set.
        let mut link_map: HashMap<LinkId, LinkId> = HashMap::new();
        let mut group_map: HashMap<GroupId, GroupId> = HashMap::new();
        let mut clip_map: HashMap<ClipId, ClipId> = HashMap::new();
        let mut prepared = Vec::new();
        for item in &cb.clips {
            let mut c = item.clip.clone();
            match &mut c.source {
                ClipSource::Media(r) => r.media = *media_map.get(&r.media).ok_or_else(|| nf(r.media))?,
                ClipSource::Sequence(r) => {
                    if self.p.sequence(r.sequence).is_none() || r.sequence == seq {
                        return invalid("the clipboard's nested sequence cannot be pasted here");
                    }
                }
                _ => {}
            }
            if let Some(l) = c.link {
                let n = *link_map.entry(l).or_insert_with(|| LinkId(self.p.alloc_id()));
                c.link = Some(n);
            }
            if let Some(g) = c.group {
                let n = *group_map.entry(g).or_insert_with(|| GroupId(self.p.alloc_id()));
                c.group = Some(n);
            }
            for m in &mut c.markers {
                m.id = MarkerId(self.p.alloc_id());
            }
            let old = c.id;
            c.id = ClipId(self.p.alloc_id());
            clip_map.insert(old, c.id);
            c.start += at;
            prepared.push((item.lane_position, c));
        }
        let spine = self.spine(seq)?;
        let spine_items: Vec<Clip> = prepared.iter().filter(|(p, _)| *p == 0).map(|(_, c)| c.clone()).collect();
        if !spine_items.is_empty() {
            match mode {
                PasteMode::Insert => {
                    // The pasted spine block runs from `at` to the end of
                    // its last clip; holes inside it become gaps.
                    let end = spine_items.iter().map(|c| c.end()).max().expect("non-empty");
                    self.fill_spine_to(seq, at)?;
                    self.split_lane_at(seq, spine, at)?;
                    self.ripple(seq, spine, at, end - at, &HashSet::new())?;
                    let mut cursor = at;
                    for mut c in spine_items {
                        self.insert_gap(seq, cursor, c.start - cursor)?;
                        cursor = c.end();
                        c.lane = spine;
                        self.prim(Edit::InsertClip { seq, clip: Box::new(c) })?;
                    }
                }
                PasteMode::Overwrite => {
                    for mut c in spine_items {
                        self.fill_spine_to(seq, c.start)?;
                        self.clear_range(seq, spine, c.range())?;
                        c.lane = spine;
                        self.prim(Edit::InsertClip { seq, clip: Box::new(c) })?;
                    }
                }
            }
        }
        for (pos, mut c) in prepared.into_iter().filter(|(p, _)| *p != 0) {
            if c.is_gap() {
                continue;
            }
            let lane = self.ensure_lane_at(seq, pos)?;
            self.unlocked(seq, lane)?;
            c.lane = lane;
            self.clear_range(seq, lane, c.range())?;
            self.prim(Edit::InsertClip { seq, clip: Box::new(c) })?;
        }
        for t in &cb.transitions {
            if let (Some(l), Some(r)) = (clip_map.get(&t.left), clip_map.get(&t.right)) {
                let mut t = t.clone();
                t.id = TransitionId(self.p.alloc_id());
                t.left = *l;
                t.right = *r;
                self.prim(Edit::InsertTransition { seq, transition: t })?;
            }
        }
        self.tidy(seq)
    }

    fn paste_attributes(&mut self, seq: SequenceId, targets: &[ClipId], src: &Clip, a: AttrSet) -> Result<()> {
        let mut prefixes: Vec<&str> = Vec::new();
        if a.transform {
            prefixes.push("transform.");
        }
        if a.audio {
            prefixes.extend(["audio.", "audio_effect."]);
        }
        if a.effects {
            prefixes.push("effect.");
        }
        if a.grade {
            prefixes.push("grade.");
        }
        if a.masks {
            prefixes.push("mask.");
        }
        for id in targets {
            let mut c = self.editable(seq, *id)?;
            if a.transform {
                c.transform = src.transform.clone();
            }
            if a.blend {
                c.blend = src.blend;
            }
            if a.audio {
                c.audio = src.audio.clone();
            }
            if a.effects {
                c.effects = src.effects.clone();
            }
            if a.grade {
                c.grade = src.grade.clone();
            }
            if a.masks {
                c.masks = src.masks.clone();
            }
            if a.motion {
                c.motion = src.motion.clone();
            }
            c.keyframes.retain(|k, _| !prefixes.iter().any(|p| k.starts_with(p)));
            for (k, tr) in &src.keyframes {
                if prefixes.iter().any(|p| k.starts_with(p)) {
                    c.keyframes.insert(k.clone(), tr.clone());
                }
            }
            self.set_clip(seq, c)?;
            if a.speed {
                self.set_speed(seq, *id, src.speed.clone())?;
            }
        }
        self.tidy(seq)
    }

    fn detach_audio(&mut self, seq: SequenceId, id: ClipId, new_clip: ClipId) -> Result<()> {
        let c = self.editable(seq, id)?;
        let ClipSource::Media(r) = &c.source else { return invalid("only media clips have sound to detach") };
        let has_audio = self.p.media_item(r.media).is_some_and(|m| m.has_audio());
        if !r.audio || !has_audio {
            return invalid("the clip has no sound to detach");
        }
        // First audio lane (nearest the spine) that is free and unlocked.
        let s = self.seq(seq)?;
        let free = s.lanes.iter().filter(|l| l.kind == LaneKind::Audio && !l.locked).find(|l| !s.clips.iter().any(|o| o.lane == l.id && o.range().overlaps(&c.range()))).map(|l| l.id);
        let lane = match free {
            Some(l) => l,
            None => {
                let pos = -(s.lanes.iter().filter(|l| l.kind == LaneKind::Audio).count() as i32) - 1;
                self.ensure_lane_at(seq, pos)?
            }
        };
        let link = c.link.unwrap_or_else(|| LinkId(self.p.alloc_id()));
        let mut audio = c.clone();
        audio.id = new_clip;
        audio.lane = lane;
        audio.source = ClipSource::Media(MediaRef { video: false, audio: true, ..r.clone() });
        audio.link = Some(link);
        audio.effects.clear();
        audio.grade = None;
        audio.masks.clear();
        audio.motion = Default::default();
        audio.transform = Default::default();
        audio.keyframes.retain(|k, _| k.starts_with("audio"));
        audio.markers.clear();
        let mut video = c.clone();
        video.source = ClipSource::Media(MediaRef { audio: false, ..r.clone() });
        video.link = Some(link);
        video.audio = Default::default();
        video.keyframes.retain(|k, _| !k.starts_with("audio"));
        self.set_clip(seq, video)?;
        self.insert_clip(seq, audio)?;
        self.tidy(seq)
    }

    fn replace_media(&mut self, seq: SequenceId, id: ClipId, media: MediaId) -> Result<()> {
        let mut c = self.editable(seq, id)?;
        let ClipSource::Media(r) = &c.source else { return invalid("only media clips can change media") };
        let item = self.p.media_item(media).ok_or_else(|| nf(media))?;
        let video = r.video && item.has_video();
        let audio = r.audio && item.has_audio();
        if !video && !audio {
            return invalid("the new media has none of the streams the clip uses");
        }
        let len = c.source_len();
        if let Some(total) = item.duration() {
            if len > total {
                return Err(EditError::NoHandles("new media is shorter than the clip".into()));
            }
            if c.source_in + len > total {
                c.source_in = 0;
            }
        }
        c.source = ClipSource::Media(MediaRef { media, video, audio, extra: r.extra.clone() });
        self.set_clip(seq, c)?;
        self.tidy(seq)
    }

    fn freeze_frame(&mut self, seq: SequenceId, id: ClipId, at: Micros, duration: Micros, new_clip: ClipId) -> Result<()> {
        let c = self.editable(seq, id)?;
        if !c.has_source_time() {
            return invalid("only media and compound clips can freeze");
        }
        if at < c.start || at >= c.end() || duration <= 0 {
            return invalid("freeze time must be inside the clip and the duration positive");
        }
        let mut still = c.clone();
        still.id = new_clip;
        still.source_in = c.source_time_at(at);
        still.speed = Speed::Constant(0.0);
        still.reversed = false;
        still.transform = c.transform_at(at - c.start);
        still.keyframes.retain(|k, _| !k.starts_with("transform."));
        still.duration = duration;
        still.audio.muted = true;
        still.audio.fade_in = Default::default();
        still.audio.fade_out = Default::default();
        still.motion = Default::default();
        still.markers.clear();
        still.link = None;
        self.split_at(seq, id, at)?;
        self.ripple(seq, c.lane, at, duration, &HashSet::new())?;
        still.start = at;
        self.insert_clip(seq, still)?;
        self.tidy(seq)
    }

    fn set_speed(&mut self, seq: SequenceId, id: ClipId, speed: Speed) -> Result<()> {
        let c = self.editable(seq, id)?;
        let mut all = vec![c.clone()];
        for p in self.partners(seq, &[id])? {
            all.push(self.editable(seq, p)?);
        }
        let mut shift = 0;
        for (i, old) in all.into_iter().enumerate() {
            let mut n = old.clone();
            n.speed = speed.clone();
            if old.has_source_time() && !old.speed.is_freeze() && !speed.is_freeze() {
                let d = speed.duration_for(old.source_len()).filter(|d| *d > 0).ok_or_else(|| EditError::Invalid("speed gives no duration".into()))?;
                n.duration = d;
                // Keep animation attached to the same content.
                let k = d as f64 / old.duration as f64;
                for tr in n.keyframes.values_mut() {
                    // Speeding up can round two close keys onto one
                    // microsecond; the later one moves on a tick rather
                    // than being dropped with its value.
                    let mut prev: Option<Micros> = None;
                    for key in &mut tr.keys {
                        let t = (key.time as f64 * k).round() as Micros;
                        key.time = prev.map_or(t, |p| t.max(p + 1));
                        prev = Some(key.time);
                    }
                }
                for m in &mut n.markers {
                    m.time = (m.time as f64 * k).round() as Micros;
                }
            }
            check_source(self.p, &n)?;
            if i == 0 {
                shift = n.end() - old.end();
            }
            self.set_clip(seq, n)?;
        }
        self.ripple(seq, c.lane, c.end(), shift, &HashSet::from([id]))?;
        self.tidy(seq)
    }

    fn add_transition(&mut self, seq: SequenceId, mut t: Transition) -> Result<()> {
        let s = self.seq(seq)?;
        let l = s.clip(t.left).ok_or_else(|| nf(t.left))?;
        if l.lane != s.spine_id() {
            return invalid("transitions join spine clips");
        }
        self.unlocked(seq, l.lane)?;
        let r = s.next_on_spine(t.left).ok_or_else(|| EditError::Invalid("no clip after the cut".into()))?;
        if s.transitions.iter().any(|x| x.left == t.left) {
            return invalid("the cut already has a transition");
        }
        t.right = r.id;
        if t.duration <= 0 {
            return invalid("transition duration must be positive");
        }
        let max = self.p.transition_limit(s, t.left, t.right, t.align, None);
        if t.duration > max {
            return Err(EditError::NoHandles(format!("at most {max} µs fits at this cut")));
        }
        t.id = TransitionId(self.fresh_id(t.id.0));
        self.prim(Edit::InsertTransition { seq, transition: t })
    }

    fn create_compound(&mut self, seq: SequenceId, ids: &[ClipId], name: String, new_sequence: SequenceId, new_clip: ClipId) -> Result<()> {
        if ids.is_empty() {
            return invalid("nothing selected");
        }
        let mut sel: Vec<ClipId> = ids.to_vec();
        sel.extend(self.partners(seq, ids)?);
        let s = self.seq(seq)?;
        let spine = s.spine_id();
        let mut clips: Vec<Clip> = sel.iter().map(|id| s.clip(*id).cloned().ok_or_else(|| nf(*id))).collect::<Result<_>>()?;
        let spine_sel: Vec<&Clip> = clips.iter().filter(|c| c.lane == spine).collect();
        let range = if spine_sel.is_empty() {
            let start = clips.iter().map(|c| c.start).min().expect("non-empty");
            let end = clips.iter().map(|c| c.end()).max().expect("non-empty");
            TimeRange::from_bounds(start, end)
        } else {
            let start = spine_sel.iter().map(|c| c.start).min().expect("non-empty");
            let end = spine_sel.iter().map(|c| c.end()).max().expect("non-empty");
            let r = TimeRange::from_bounds(start, end);
            if clips.iter().any(|c| c.start < r.start || c.end() > r.end()) {
                return invalid("clips off the spine must lie within the selected spine clips");
            }
            // Every spine clip in the range comes along, so the spine stays packed.
            for c in s.spine_clips() {
                if c.start >= r.start && c.end() <= r.end() && !sel.contains(&c.id) {
                    sel.push(c.id);
                    clips.push(c.clone());
                }
            }
            r
        };
        for c in &clips {
            self.unlocked(seq, c.lane)?;
        }
        let s = self.seq(seq)?;
        let target_lane = if clips.iter().any(|c| c.lane == spine) { spine } else { s.clip(ids[0]).expect("checked").lane };
        let positions: Vec<i32> = clips.iter().map(|c| s.lane_position(c.lane).expect("lane exists")).collect();
        let transitions: Vec<Transition> = s.transitions.iter().filter(|t| sel.contains(&t.left) && sel.contains(&t.right)).cloned().collect();
        let top = positions.iter().copied().max().unwrap_or(0).max(0);
        let bottom = positions.iter().copied().min().unwrap_or(0).min(0);

        // Build the nested sequence.
        let nested_id = SequenceId(self.fresh_id(new_sequence.0));
        let spine_lane = LaneId(self.p.alloc_id());
        let mut nested = Sequence::new(nested_id, if name.is_empty() { "Compound".to_string() } else { name.clone() }, spine_lane);
        let mut lane_for: HashMap<i32, LaneId> = HashMap::from([(0, spine_lane)]);
        for pos in (1..=top).rev() {
            let id = LaneId(self.p.alloc_id());
            nested.lanes.insert(nested.lanes.len() - 1, Lane::new(id, LaneKind::Visual, format!("Video {pos}")));
            lane_for.insert(pos, id);
        }
        for pos in (bottom..0).rev() {
            let id = LaneId(self.p.alloc_id());
            nested.lanes.push(Lane::new(id, LaneKind::Audio, format!("Audio {}", -pos)));
            lane_for.insert(pos, id);
        }
        for (c, pos) in clips.iter().zip(&positions) {
            let mut n = c.clone();
            n.start -= range.start;
            n.lane = lane_for[pos];
            nested.clips.push(n);
        }
        nested.transitions = transitions;

        // Remove the originals (and transitions touching them) from the parent.
        for c in &clips {
            self.remove_clip(seq, c.id)?;
        }
        self.prim(Edit::InsertSequence { sequence: Box::new(nested), index: None })?;
        let mut comp = Clip::new(new_clip, target_lane, range.start, range.duration, ClipSource::Sequence(SequenceRef { sequence: nested_id, extra: Default::default() }));
        comp.name = name;
        self.insert_clip(seq, comp)?;
        self.tidy(seq)
    }

    fn expand_compound(&mut self, seq: SequenceId, id: ClipId) -> Result<()> {
        let c = self.editable(seq, id)?;
        let Some(nested_id) = c.sequence_id() else { return invalid("not a compound clip") };
        if c.speed != Speed::Constant(1.0) || c.reversed {
            return invalid("compound clips with speed changes or reverse cannot be expanded");
        }
        let nested = self.p.sequence(nested_id).ok_or_else(|| nf(nested_id))?.clone();
        let window = TimeRange::new(c.source_in, c.duration);
        let offset = c.start - c.source_in;
        let s = self.seq(seq)?;
        let spine = s.spine_id();
        let parent_pos = s.lane_position(c.lane).expect("lane exists");

        // Pieces of the nested clips that fall inside the window.
        let mut pieces = Vec::new();
        for n in &nested.clips {
            if !n.range().overlaps(&window) {
                continue;
            }
            let mut piece = n.clone();
            if piece.start < window.start {
                piece = trim_local(self.p, &piece, Edge::Start, window.start - piece.start, false)?;
            }
            if piece.end() > window.end() {
                piece = trim_local(self.p, &piece, Edge::End, window.end() - piece.end(), false)?;
            }
            let pos = nested.lane_position(n.lane).expect("lane exists");
            let target = if parent_pos == 0 {
                pos
            } else if pos >= 0 {
                parent_pos + pos
            } else {
                pos.min(-1)
            };
            if piece.is_gap() && target != 0 {
                continue;
            }
            piece.start += offset;
            pieces.push((n.id, target, piece));
        }

        self.remove_clip(seq, id)?;
        let mut clip_map = HashMap::new();
        let mut link_map: HashMap<LinkId, LinkId> = HashMap::new();
        let mut spine_cursor = c.start;
        pieces.sort_by_key(|(_, pos, p)| (*pos != 0, p.start));
        for (old, pos, mut piece) in pieces {
            let lane = if pos == 0 && c.lane == spine { spine } else { self.ensure_lane_at(seq, pos)? };
            if lane == spine {
                self.insert_gap(seq, spine_cursor, piece.start - spine_cursor)?;
                spine_cursor = piece.end();
            }
            piece.lane = lane;
            piece.id = ClipId(self.p.alloc_id());
            if let Some(l) = piece.link {
                piece.link = Some(*link_map.entry(l).or_insert_with(|| LinkId(self.p.alloc_id())));
            }
            for m in &mut piece.markers {
                m.id = MarkerId(self.p.alloc_id());
            }
            clip_map.insert(old, piece.id);
            self.prim(Edit::InsertClip { seq, clip: Box::new(piece) })?;
        }
        if c.lane == spine {
            self.insert_gap(seq, spine_cursor, c.end() - spine_cursor)?;
        }
        for t in &nested.transitions {
            if let (Some(l), Some(r)) = (clip_map.get(&t.left), clip_map.get(&t.right)) {
                let mut t = t.clone();
                t.id = TransitionId(self.p.alloc_id());
                t.left = *l;
                t.right = *r;
                self.prim(Edit::InsertTransition { seq, transition: t })?;
            }
        }
        self.tidy(seq)
    }
}

/// Moves one edge of a copy of `c` by `delta` (positive is later), keeping
/// the content under the other edge in place. With `keep_start`, a start
/// trim leaves the clip's position alone and the end moves instead (ripple).
pub(crate) fn trim_local(p: &crate::Project, c: &Clip, edge: Edge, delta: Micros, keep_start: bool) -> Result<Clip> {
    let mut n = c.clone();
    match edge {
        Edge::End => {
            n.duration = c.duration + delta;
            if n.duration <= 0 {
                return invalid("trim would leave no clip");
            }
            if c.reversed && c.has_source_time() {
                // The end of a reversed clip is the start of its source range.
                let new_len = n.source_len();
                n.source_in = c.source_out() - new_len;
            }
        }
        Edge::Start => {
            n.duration = c.duration - delta;
            if n.duration <= 0 {
                return invalid("trim would leave no clip");
            }
            if !keep_start {
                n.start = c.start + delta;
                if n.start < 0 {
                    return invalid("trim would start before the sequence start");
                }
            }
            if c.has_source_time() && !c.reversed {
                let consumed = if delta >= 0 { c.source_offset(delta) } else { c.speed.rate_at(0) * delta as f64 };
                n.source_in = c.source_in + consumed.round() as Micros;
            }
            if let Speed::Curve(tr) = &mut n.speed {
                tr.shift(-delta);
            }
            for tr in n.keyframes.values_mut() {
                tr.shift(-delta);
            }
            for m in &mut n.markers {
                m.time -= delta;
            }
        }
    }
    check_source(p, &n)?;
    Ok(n)
}

/// Checks that a clip's source range lies inside its media.
fn check_source(p: &crate::Project, c: &Clip) -> Result<()> {
    if !c.has_source_time() {
        return Ok(());
    }
    if c.source_in < 0 {
        return Err(EditError::NoHandles(format!("clip {} would start before its source", c.id.0)));
    }
    if let ClipSource::Media(_) = c.source
        && let Some(total) = p.source_duration(c)
        && c.source_out() > total
    {
        return Err(EditError::NoHandles(format!("clip {} would run past the end of its media", c.id.0)));
    }
    Ok(())
}

impl crate::Project {
    /// The longest transition with `align` that fits the cut after spine
    /// clip `left` in sequence `seq`, given both clips' handles and any
    /// transitions on their far edges. `None` if `left` has no spine
    /// neighbour.
    pub fn max_transition_duration(&self, seq: SequenceId, left: ClipId, align: TransitionAlign) -> Option<Micros> {
        let s = self.sequence(seq)?;
        let right = s.next_on_spine(left)?.id;
        let existing = s.transitions.iter().find(|t| t.left == left).map(|t| t.id);
        Some(self.transition_limit(s, left, right, align, existing).max(0))
    }

    pub(crate) fn transition_limit(&self, s: &Sequence, left: ClipId, right: ClipId, align: TransitionAlign, ignore: Option<TransitionId>) -> Micros {
        let (Some(l), Some(r)) = (s.clip(left), s.clip(right)) else { return 0 };
        let others = || s.transitions.iter().filter(|t| Some(t.id) != ignore);
        // Time at l's start used by its incoming transition, and at r's end
        // by its outgoing one.
        let l_head = others().find(|t| t.right == left).map_or(0, |t| t.split().1);
        let r_tail = others().find(|t| t.left == right).map_or(0, |t| t.split().0);
        let (_, l_after) = l.handles(self.source_duration(l));
        let (r_before, _) = r.handles(self.source_duration(r));
        let big = Micros::MAX / 4;
        // `pre` lies inside `l` and needs `r`'s head handle; `post` lies
        // inside `r` and needs `l`'s tail handle.
        let pre_max = (l.duration - l_head).min(r_before.unwrap_or(big));
        let post_max = (r.duration - r_tail).min(l_after.unwrap_or(big));
        match align {
            TransitionAlign::Centre => (2 * post_max).min(2 * pre_max + 1),
            TransitionAlign::EndAtCut => pre_max,
            TransitionAlign::StartAtCut => post_max,
        }
    }
}
