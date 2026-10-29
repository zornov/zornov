// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The open document: a `studio-model` project, its undo `History`, and the
//! view the screens draw from.
//!
//! Every change is a model `Edit` applied through the history. Two ways in:
//!
//! * [`Doc::apply`] sends one of the model's operations (split, trim,
//!   ripple delete, move, …) directly.
//! * [`Doc::edit`] runs a closure over a copy of the view; the difference is
//!   turned into primitive edits (see [`bridge::diff`]). Used for plain
//!   attribute changes from the Inspector and the canvas.
//!
//! Continuous gestures (dragging a slider, a clip edge, a canvas handle)
//! are one history transaction: the first change opens it and
//! [`Doc::end_gesture`] commits it when the pointer is released.

use crate::bridge;
use crate::view::{Project, Time};
use studio_model as m;

pub struct Doc {
    pub model: m::Project,
    pub history: m::History,
    /// The view of the active sequence at `view_time`. Shared: screens take
    /// a copy each frame to read while they change other state, and a copy
    /// of a long project's view was most of a frame.
    pub project: std::sync::Arc<Project>,
    view_time: Time,
    gesture: Option<String>,
    /// The clips the last undo or redo changed (for restoring selection).
    history_touched: Vec<u32>,
    /// Bumped whenever the view is rebuilt.
    pub revision: u64,
    /// Bumped when the project itself changes (not when the view only
    /// follows the playhead over keyframes).
    pub edits: u64,
    /// Message of the last edit the model refused, for a toast.
    pub error: Option<String>,
    /// Where the project is saved, once it has been.
    pub folder: Option<m::file::ProjectFolder>,
    /// Where each clip sits in `project.clips`, by id.
    slot: std::collections::HashMap<u32, usize>,
    /// Clips with keyframes: (start, end, id). Moving the playhead only
    /// changes the view of those it passes over.
    keyed: Vec<(Time, Time, u32)>,
    /// The team's rules for this person (reviewer, others' locks): every
    /// edit is checked against them. `None`: nothing limits edits.
    pub guard: Option<crate::collab::team::Guard>,
    /// The project file as this document last read or wrote it: its stamp
    /// and the project it held, so a save can tell that someone else saved
    /// since and take their work in (see `services::project::save`).
    pub synced: Option<Synced>,
    /// This process's hold on the project folder (`ProjectFolder::hold_open`),
    /// so other sessions on the same folder know it is open here.
    pub open_hold: Option<std::fs::File>,
}

/// What `project.json` held when this document last loaded or saved it.
#[derive(Clone)]
pub struct Synced {
    pub stamp: u64,
    pub base: std::sync::Arc<m::Project>,
}

impl Doc {
    pub fn new(model: m::Project) -> Self {
        let project = std::sync::Arc::new(bridge::view_of(&model, 0));
        let mut d = Self { model, history: m::History::new(), project, view_time: 0, gesture: None, history_touched: Vec::new(), revision: 0, edits: 0, error: None, folder: None, slot: Default::default(), keyed: Vec::new(), guard: None, synced: None, open_hold: None };
        d.index();
        d
    }

    pub fn from_view(v: &Project) -> Self {
        Self::new(bridge::to_model(v))
    }

    pub fn seq(&self) -> m::SequenceId {
        self.model.active_sequence
    }

    /// Works on another sequence (the inside of a merged clip): edits and
    /// the view follow it until it is switched back. Returns the one that
    /// was active.
    pub fn focus_sequence(&mut self, seq: m::SequenceId) -> m::SequenceId {
        let was = self.model.active_sequence;
        if was != seq && self.model.sequence(seq).is_some() {
            self.model.active_sequence = seq;
            self.refresh_view();
        }
        was
    }

    fn refresh(&mut self) {
        self.refresh_view();
        self.edits += 1;
    }

    /// The view again, the project unchanged (keyframed values at a new
    /// time).
    fn refresh_view(&mut self) {
        self.project = std::sync::Arc::new(bridge::view_of(&self.model, self.view_time));
        self.index();
        self.revision += 1;
    }

    fn index(&mut self) {
        self.slot = self.project.clips.iter().enumerate().map(|(i, c)| (c.id, i)).collect();
        self.keyed = self.project.clips.iter().filter(|c| !c.keys.is_empty()).map(|c| (c.start, c.end(), c.id)).collect();
    }

    /// The view of just `ids` again (an edit that changed only them): a
    /// whole view of a long project is milliseconds, and every drag step
    /// paid it. Falls back to the whole view when a clip came or went.
    fn refresh_clips(&mut self, ids: &[u32]) {
        let seq = self.model.active();
        let by_id: Option<std::collections::HashMap<u64, &m::Clip>> = (ids.len() > 16).then(|| seq.clips.iter().map(|c| (c.id.0, c)).collect());
        let mut fresh = Vec::with_capacity(ids.len());
        for &id in ids {
            let mc = match &by_id {
                Some(map) => map.get(&(id as u64)).copied(),
                None => seq.clip(m::ClipId(id as u64)),
            };
            let (Some(mc), Some(&i)) = (mc, self.slot.get(&id)) else { return self.refresh_view() };
            let Some(v) = bridge::clip_view(&self.model, mc, self.view_time) else { return self.refresh_view() };
            fresh.push((i, v));
        }
        let mut moved = false;
        let project = std::sync::Arc::make_mut(&mut self.project);
        for (i, v) in fresh {
            let id = v.id;
            self.keyed.retain(|k| k.2 != id);
            if !v.keys.is_empty() {
                self.keyed.push((v.start, v.end(), id));
            }
            let old = &project.clips[i];
            moved |= old.lane != v.lane || old.start != v.start;
            project.clips[i] = v;
        }
        // The model keeps clips in (lane, start, id) order; a clip that
        // moved takes its place in the view the same way.
        if moved {
            project.clips.sort_by_key(|c| (c.lane, c.start, c.id));
            self.slot = project.clips.iter().enumerate().map(|(i, c)| (c.id, i)).collect();
        }
        self.revision += 1;
    }

    /// After `edit` (or its inverse) was applied: the clips it changed, or
    /// the whole view when it changed more than clips.
    fn refresh_for(&mut self, edit: Option<&m::Edit>) {
        match edit.and_then(|e| self.clips_of(e)) {
            Some(ids) => self.refresh_clips(&ids),
            None => self.refresh_view(),
        }
        self.edits += 1;
    }

    /// Keep keyframed values in the view in step with the playhead.
    pub fn set_time(&mut self, t: Time) {
        if t != self.view_time {
            let (a, b) = (t.min(self.view_time), t.max(self.view_time));
            self.view_time = t;
            // A clip's keyed values change only while the playhead is
            // inside it; before and after it they hold.
            let ids: Vec<u32> = self.keyed.iter().filter(|k| k.1 >= a && k.0 <= b).map(|k| k.2).collect();
            if !ids.is_empty() {
                self.refresh_clips(&ids);
            }
        }
    }

    pub fn dirty(&self) -> bool {
        self.history.is_dirty()
    }

    /// A fresh id from the model's shared counter.
    pub fn alloc_id(&mut self) -> u64 {
        self.model.alloc_id()
    }

    fn report(&mut self, e: m::EditError) {
        self.error = Some(match e {
            m::EditError::Locked(_) => "That lane is locked".into(),
            m::EditError::NoHandles(s) => format!("Not enough media: {s}"),
            m::EditError::Invalid(s) => s,
            m::EditError::NotFound(s) => format!("Not found: {s}"),
            m::EditError::Invariant(v) => format!("Not possible: {}", plain(&self.model, &v)),
        });
    }

    fn finish(&mut self, r: Result<(), m::EditError>, ids: Option<Vec<u32>>) -> bool {
        let ok = match r {
            Ok(()) => true,
            Err(e) => {
                self.report(e);
                false
            }
        };
        // What it changed, in primitives (a trim or a move is a few clips
        // set): the view of only those, whatever the edit was.
        let ids = ids.or_else(|| self.history.last_inverse().and_then(|e| self.clips_of(e)));
        match ids {
            Some(ids) if ok => {
                self.refresh_clips(&ids);
                self.edits += 1;
            }
            _ => self.refresh(),
        }
        ok
    }

    /// The clips `edit` changes, when it changes nothing else.
    fn clips_of(&self, edit: &m::Edit) -> Option<Vec<u32>> {
        let mut ids = Vec::new();
        (touched(edit, self.model.active_sequence, &mut ids) && ids.len() <= 4096).then(|| {
            ids.sort_unstable();
            ids.dedup();
            ids
        })
    }

    /// Whether the team's rules let this person make `edit` (the reason
    /// goes to `error` when not).
    fn permitted(&mut self, edit: &m::Edit) -> bool {
        let Some(g) = &self.guard else { return true };
        match g.check(&self.model, edit) {
            Ok(()) => true,
            Err(e) => {
                self.error = Some(e);
                false
            }
        }
    }

    /// Turns the project into `target` as one undo step: restoring a
    /// version, taking in a merge. Team rules do not apply (a reviewer's
    /// copy still follows the edit). Returns how many edits it took.
    pub fn take_in(&mut self, label: &str, target: &m::Project) -> Result<usize, String> {
        self.end_gesture();
        let edits = crate::collab::delta::edits_between(&self.model, target);
        let n = edits.len();
        if n > 0 {
            let r = self.history.apply(&mut self.model, label, m::Edit::Batch { edits });
            if let Err(e) = r {
                self.refresh();
                return Err(e.to_string());
            }
        }
        self.model.next_id = self.model.next_id.max(target.next_id);
        self.refresh();
        Ok(n)
    }

    /// Apply a model operation as its own undo step.
    pub fn apply(&mut self, label: &str, edit: m::Edit) -> bool {
        self.end_gesture();
        if !self.permitted(&edit) {
            return false;
        }
        let ids = self.clips_of(&edit);
        crate::macros::note(&self.model, label, &edit);
        let r = self.history.apply(&mut self.model, label, edit);
        self.finish(r, ids)
    }

    /// Apply a model operation as part of the gesture `key`.
    pub fn apply_gesture(&mut self, key: &str, label: &str, edit: m::Edit) -> bool {
        if !self.permitted(&edit) {
            return false;
        }
        self.begin_gesture(key, label);
        let ids = self.clips_of(&edit);
        crate::macros::note(&self.model, label, &edit);
        let r = self.history.apply(&mut self.model, label, edit);
        self.finish(r, ids)
    }

    fn begin_gesture(&mut self, key: &str, label: &str) {
        if self.gesture.as_deref() != Some(key) {
            self.end_gesture();
            self.history.begin(label);
            self.gesture = Some(key.to_owned());
        }
    }

    /// Change the project through a closure over the view, as one undo step.
    pub fn edit(&mut self, label: &str, f: impl FnOnce(&mut Project)) -> bool {
        self.end_gesture();
        let edits = self.diff_of(f);
        if edits.is_empty() {
            return false;
        }
        let edit = m::Edit::Batch { edits };
        if !self.permitted(&edit) {
            return false;
        }
        let ids = self.clips_of(&edit);
        crate::macros::note(&self.model, label, &edit);
        let r = self.history.apply(&mut self.model, label, edit);
        self.finish(r, ids)
    }

    /// Like [`Self::edit`], merged with earlier changes of the same gesture.
    pub fn edit_gesture(&mut self, key: &str, label: &str, f: impl FnOnce(&mut Project)) -> bool {
        let edits = self.diff_of(f);
        if edits.is_empty() {
            return false;
        }
        let edit = m::Edit::Batch { edits };
        if !self.permitted(&edit) {
            return false;
        }
        self.begin_gesture(key, label);
        let ids = self.clips_of(&edit);
        crate::macros::note(&self.model, label, &edit);
        let r = self.history.apply(&mut self.model, label, edit);
        self.finish(r, ids)
    }

    fn diff_of(&mut self, f: impl FnOnce(&mut Project)) -> Vec<m::Edit> {
        // One copy: the view itself is the "before".
        let mut after = (*self.project).clone();
        after.next_id = self.model.next_id as u32;
        f(&mut after);
        // Ids handed out by the closure come from the model's counter.
        if after.next_id as u64 > self.model.next_id {
            self.model.next_id = after.next_id as u64 + 1;
        }
        let mut edits = bridge::diff(&self.model, &self.project, &after, self.view_time);
        // New effects get ids of their own.
        for e in &mut edits {
            if let m::Edit::SetClip { clip, .. } | m::Edit::InsertClip { clip, .. } = e {
                for fx in &mut clip.effects {
                    if fx.id.0 == 0 {
                        fx.id = m::EffectId(self.model.alloc_id());
                    }
                }
            }
        }
        edits
    }

    /// The clips the last undo or redo changed; empty when unknown.
    pub fn take_history_touched(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.history_touched)
    }

    /// Close the open gesture (pointer released).
    pub fn end_gesture(&mut self) {
        if self.gesture.take().is_some() && self.history.in_transaction() {
            self.history.commit();
        }
    }

    /// Abandon the open gesture (Esc during a drag).
    pub fn cancel_gesture(&mut self) {
        if self.gesture.take().is_some() && self.history.in_transaction() {
            let _ = self.history.cancel(&mut self.model);
            self.refresh();
        }
    }

    /// Starts a batch: everything applied until [`Self::commit_batch`] is
    /// one undo step (agent edits).
    pub fn begin_batch(&mut self, label: &str) {
        self.end_gesture();
        self.history.begin(label);
    }

    /// Closes the batch opened by [`Self::begin_batch`].
    pub fn commit_batch(&mut self) {
        self.history.commit();
        self.refresh();
    }

    /// Whether a batch (or gesture) is open.
    pub fn in_batch(&self) -> bool {
        self.history.in_transaction()
    }

    /// Applies an edit made on another copy of this project (the agent's),
    /// joining an open batch. Refused edits change nothing.
    pub fn apply_replayed(&mut self, label: &str, edit: m::Edit) -> Result<(), String> {
        if !self.permitted(&edit) {
            return Err(self.error.take().unwrap_or_default());
        }
        crate::macros::note(&self.model, label, &edit);
        let r = self.history.apply(&mut self.model, label, edit);
        let out = r.map_err(|e| {
            self.report(e);
            self.error.take().unwrap_or_default()
        });
        self.refresh();
        out
    }

    /// Replaces the model and forgets the history (a copy following
    /// another document). The revision keeps rising.
    pub fn reset_model(&mut self, model: m::Project, folder: Option<m::file::ProjectFolder>) {
        self.model = model;
        self.history.clear();
        self.gesture = None;
        self.folder = folder;
        self.refresh();
    }

    /// Reverts everything since [`Self::begin_batch`].
    pub fn rollback_batch(&mut self) {
        let _ = self.history.cancel(&mut self.model);
        self.refresh();
    }

    /// Whether the person has a step to undo (the AI's work has its own
    /// Undo, in its panel).
    pub fn can_undo(&self) -> bool {
        self.history.undo_steps().iter().any(|s| s.author.is_person()) || self.history.open_author().is_some_and(|a| a.is_person()) && self.history.can_undo()
    }
    /// How many steps there are to undo (tests).
    #[cfg(test)]
    pub fn history_len(&self) -> usize {
        self.history.undo_steps().len()
    }

    pub fn can_redo(&self) -> bool {
        self.history.redo_steps().iter().any(|s| s.author.is_person())
    }
    pub fn undo_label(&self) -> Option<&str> {
        self.history.undo_steps().iter().rev().find(|s| s.author.is_person()).map(|s| s.label.as_str())
    }
    pub fn redo_label(&self) -> Option<&str> {
        self.history.redo_steps().iter().rev().find(|s| s.author.is_person()).map(|s| s.label.as_str())
    }

    /// Changes from here on are `author`'s (an agent's change landing in
    /// the window); everything else is the person's.
    pub fn as_author<R>(&mut self, author: m::Author, f: impl FnOnce(&mut Self) -> R) -> R {
        let was = self.history.set_author(author);
        let r = f(self);
        self.history.set_author(was);
        r
    }

    /// Undoes the person's last step, whatever an agent did since. Returns
    /// its label; why it could not goes to `error`.
    pub fn undo(&mut self) -> Option<String> {
        self.undo_where(&|a: &m::Author| a.is_person()).unwrap_or_else(|e| {
            self.error = Some(e);
            None
        })
    }

    /// Redoes the person's last undone step.
    pub fn redo(&mut self) -> Option<String> {
        self.redo_where(&|a: &m::Author| a.is_person()).unwrap_or_else(|e| {
            self.error = Some(e);
            None
        })
    }

    /// Undoes the newest step whose author `mine` picks. When others made
    /// steps after it, it is taken out from under them: what it changed
    /// goes back, and what they changed since stays (where both changed the
    /// same thing, theirs is kept and said in `error`).
    pub fn undo_where(&mut self, mine: &dyn Fn(&m::Author) -> bool) -> Result<Option<String>, String> {
        self.end_gesture();
        let steps = self.history.undo_steps();
        let Some(i) = steps.iter().rposition(|s| mine(&s.author)) else { return Ok(None) };
        if i + 1 == steps.len() && !self.history.in_transaction() {
            let r = self.history.undo(&mut self.model).map_err(|e| e.to_string());
            let done = self.history.last_redo().cloned();
            self.history_touched = done.as_ref().and_then(|e| self.clips_of(e)).unwrap_or_default();
            self.refresh_for(done.as_ref().filter(|_| r.as_ref().is_ok_and(|l| l.is_some())));
            return r;
        }
        let step = steps[i].clone();
        let cannot = |e: m::EditError| format!("Could not undo {}: {e}", step.label.to_lowercase());
        // The project just after the step: the later steps taken back.
        let mut after = self.model.clone();
        for later in steps[i + 1..].iter().rev() {
            after.apply(later.inverse.clone()).map_err(cannot)?;
        }
        let mut before = after.clone();
        before.apply(step.inverse.clone()).map_err(cannot)?;
        let out = crate::collab::merge::three_way(&after, &self.model, &before)?;
        let edits = crate::collab::delta::edits_between(&self.model, &out.merged);
        let mut old = self.model.clone();
        let redo = self.model.apply(m::Edit::Batch { edits }).map_err(cannot)?;
        self.history.take_undo(i);
        self.history.record_undone(m::Step { label: step.label.clone(), inverse: redo, author: step.author });
        self.rebase_after(i, &step.inverse, &mut old);
        self.refresh();
        if !out.conflicts.is_empty() {
            let kept: Vec<String> = out.conflicts.iter().take(4).map(|c| format!("{} {}", c.what, if c.name.is_empty() { c.id.to_string() } else { c.name.clone() })).collect();
            self.error = Some(format!("Undid {}; kept what was changed since on {}", step.label.to_lowercase(), kept.join(", ")));
        }
        Ok(Some(step.label))
    }

    /// The steps from `i` on were recorded with a step now undone still in
    /// place (`old`: the project as it was then). Where they touched the
    /// same things, their undo is worked out again on the project as it is,
    /// so undoing them later does not bring the undone change back.
    fn rebase_after(&mut self, i: usize, undone: &m::Edit, old: &mut m::Project) {
        let mut hit = std::collections::HashSet::new();
        let all = !keys(undone, &mut hit);
        let mut now = self.model.clone();
        for j in (i..self.history.undo_steps().len()).rev() {
            let inverse = self.history.undo_steps()[j].inverse.clone();
            let mut mine = std::collections::HashSet::new();
            let overlaps = all || !keys(&inverse, &mut mine) || !mine.is_disjoint(&hit);
            let old_after = overlaps.then(|| old.clone());
            if old.apply(inverse.clone()).is_err() {
                return;
            }
            match old_after {
                Some(old_after) => {
                    let Ok(out) = crate::collab::merge::three_way(&old_after, &now, old) else { return };
                    self.history.set_undo_inverse(j, m::Edit::Batch { edits: crate::collab::delta::edits_between(&now, &out.merged) });
                    now = out.merged;
                }
                None => {
                    if now.apply(inverse).is_err() {
                        return;
                    }
                }
            }
        }
    }

    /// Redoes the newest undone step whose author `mine` picks.
    pub fn redo_where(&mut self, mine: &dyn Fn(&m::Author) -> bool) -> Result<Option<String>, String> {
        self.end_gesture();
        let steps = self.history.redo_steps();
        let Some(i) = steps.iter().rposition(|s| mine(&s.author)) else { return Ok(None) };
        if i + 1 == steps.len() && !self.history.in_transaction() {
            let r = self.history.redo(&mut self.model).map_err(|e| e.to_string());
            let done = self.history.last_undo().cloned();
            self.history_touched = done.as_ref().and_then(|e| self.clips_of(e)).unwrap_or_default();
            self.refresh_for(done.as_ref().filter(|_| r.as_ref().is_ok_and(|l| l.is_some())));
            return r;
        }
        let step = steps[i].clone();
        let undo = self.model.apply(step.inverse.clone()).map_err(|e| format!("Could not redo {}: {e}", step.label.to_lowercase()))?;
        self.history.take_redo(i);
        self.history.record_redone(m::Step { label: step.label.clone(), inverse: undo, author: step.author });
        self.refresh();
        Ok(Some(step.label))
    }

    /// Takes back everything an agent did for `request` (the AI panel's
    /// Undo). Returns how many of its steps were undone.
    pub fn undo_request(&mut self, request: u64) -> Result<usize, String> {
        let mut n = 0;
        while self.undo_where(&|a: &m::Author| matches!(a, m::Author::Agent { request: r, .. } if *r == request))?.is_some() {
            n += 1;
        }
        Ok(n)
    }

    /// Whether `request` left anything to undo.
    pub fn request_undoable(&self, request: u64) -> bool {
        self.history.undo_steps().iter().any(|s| matches!(s.author, m::Author::Agent { request: r, .. } if r == request))
    }

    /// An agent's own Undo: its last call, all of it. Returns the label.
    pub fn undo_agent_call(&mut self) -> Result<Option<String>, String> {
        let Some(who) = self.history.undo_steps().iter().rev().map(|s| s.author).find(|a| !a.is_person()) else { return Ok(None) };
        let mut label = None;
        while let Some(l) = self.undo_where(&|a: &m::Author| *a == who)? {
            label = Some(l);
        }
        Ok(label)
    }

    /// An agent's own Redo: the call it undid last.
    pub fn redo_agent_call(&mut self) -> Result<Option<String>, String> {
        let Some(who) = self.history.redo_steps().iter().rev().map(|s| s.author).find(|a| !a.is_person()) else { return Ok(None) };
        let mut label = None;
        while let Some(l) = self.redo_where(&|a: &m::Author| *a == who)? {
            label = Some(l);
        }
        Ok(label)
    }
}

/// The things a primitive edit changes, into `out`; false when it is not
/// a primitive edit (it may change anything).
fn keys(e: &m::Edit, out: &mut std::collections::HashSet<(u8, u64)>) -> bool {
    use m::Edit::*;
    let k = match e {
        InsertClip { clip, .. } | SetClip { clip, .. } => (0, clip.id.0),
        RemoveClip { clip, .. } => (0, clip.0),
        InsertTransition { transition, .. } | SetTransition { transition, .. } => (1, transition.id.0),
        RemoveTransition { id, .. } => (1, id.0),
        InsertMarker { marker, .. } | SetMarker { marker, .. } => (2, marker.id.0),
        RemoveMarker { id, .. } => (2, id.0),
        InsertLane { lane, .. } | SetLane { lane, .. } => (3, lane.id.0),
        RemoveLane { lane, .. } | MoveLane { lane, .. } => (3, lane.0),
        InsertMedia { item, .. } | SetMedia { item } => (4, item.id.0),
        RemoveMedia { id } => (4, id.0),
        InsertSequence { sequence, .. } => (5, sequence.id.0),
        RemoveSequence { id } | RenameSequence { id, .. } => (5, id.0),
        SetActiveSequence { .. } => (6, 0),
        SetSettings { .. } => (7, 0),
        InsertPreset { preset } | SetPreset { preset } => (8, preset.id.0),
        RemovePreset { id } => (8, id.0),
        SetStory { .. } => (9, 0),
        Batch { edits } => return edits.iter().all(|e| keys(e, out)),
        _ => return false,
    };
    out.insert(k);
    true
}

/// Adds to `out` the clips `e` changes; false when it changes anything but
/// clips of `seq` in place (clips coming or going, ripples, lanes,
/// markers, settings…): those need the whole view.
fn touched(e: &m::Edit, seq: m::SequenceId, out: &mut Vec<u32>) -> bool {
    use m::Edit::*;
    let mut one = |s: &m::SequenceId, c: &m::ClipId| {
        out.push(c.0 as u32);
        *s == seq
    };
    match e {
        SetClip { seq: s, clip } => one(s, &clip.id),
        AddEffect { seq: s, clip, .. }
        | RemoveEffect { seq: s, clip, .. }
        | SetEffect { seq: s, clip, .. }
        | MoveEffect { seq: s, clip, .. }
        | SetGrade { seq: s, clip, .. }
        | AddMask { seq: s, clip, .. }
        | RemoveMask { seq: s, clip, .. }
        | SetMask { seq: s, clip, .. }
        | SetKeyframe { seq: s, clip, .. }
        | RemoveKeyframe { seq: s, clip, .. }
        | SetTrack { seq: s, clip, .. }
        | Slip { seq: s, clip, .. }
        | ReplaceMedia { seq: s, clip, .. } => one(s, clip),
        Reverse { seq: s, clips } => {
            out.extend(clips.iter().map(|c| c.0 as u32));
            *s == seq
        }
        Batch { edits } => edits.iter().all(|e| touched(e, seq, out)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{self, SEC};

    #[test]
    fn moving_the_playhead_over_keyframes_is_not_an_edit() {
        let mut s = crate::agent::Session::default();
        s.run("edit", &serde_json::json!({"ops": [{"op": "card", "colour": "#ff0000", "at": 0, "duration": 4}, {"op": "animate", "clip": "$1", "property": "opacity", "keys": [[0, 0], [4, 100]]}]})).unwrap();
        let doc = &mut s.doc;
        let (edits, rev) = (doc.edits, doc.revision);
        for t in 1..30 {
            doc.set_time(t * SEC / 10);
        }
        assert!(doc.revision > rev, "the view follows the keyframes");
        assert_eq!(doc.edits, edits, "but the project did not change");
    }

    /// The view patched after an edit, an undo, a redo and playhead moves
    /// is the view built afresh.
    #[test]
    fn patched_view_matches_a_fresh_one() {
        let mut d = Doc::new(crate::perf::heavy(0.05, 3, 1));
        let fresh = |d: &Doc| bridge::view_of(&d.model, d.view_time);
        let ids: Vec<u32> = d.project.clips.iter().map(|c| c.id).take(40).collect();
        for (k, &id) in ids.iter().enumerate() {
            d.edit_gesture("drag", "Move", |p| {
                let c = p.clip_mut(id).unwrap();
                c.transform.x += 5.0;
                c.start += (k as i64 % 3) * SEC / 10;
            });
            d.set_time(k as i64 * SEC / 3);
            assert_eq!(*d.project, fresh(&d), "after edit {k}");
        }
        d.end_gesture();
        // Model operations: what they changed comes from their inverse.
        let seq = d.seq();
        let over = d.project.clips.iter().find(|c| d.project.lane(c.lane).is_some_and(|l| l.kind != view::LaneKind::Spine)).unwrap().id;
        let main = d.project.clips.iter().find(|c| d.project.lane(c.lane).is_some_and(|l| l.kind == view::LaneKind::Spine)).unwrap().id;
        for (name, e) in [
            ("trim", m::Edit::Trim { seq, clip: m::ClipId(over as u64), edge: m::Edge::End, delta: -SEC / 2, ripple: false }),
            ("ripple trim", m::Edit::Trim { seq, clip: m::ClipId(main as u64), edge: m::Edge::End, delta: -SEC / 2, ripple: true }),
            ("move", m::Edit::Move { seq, clips: vec![m::ClipId(over as u64)], delta: SEC / 4, to_lane: None }),
        ] {
            d.apply(name, e);
            assert_eq!(*d.project, fresh(&d), "after {name}");
        }
        d.undo();
        assert_eq!(*d.project, fresh(&d), "after undo");
        d.redo();
        assert_eq!(*d.project, fresh(&d), "after redo");
        for t in [0, 40 * SEC, 3 * SEC, 170 * SEC] {
            d.set_time(t);
            assert_eq!(*d.project, fresh(&d), "at {t}");
        }
    }

    #[test]
    fn split_undo_redo_round_trip() {
        let mut d = Doc::from_view(&view::demo());
        let before = d.model.clone();
        let n = d.project.clips.len();
        let seq = d.seq();
        assert!(d.apply("Split", m::Edit::Split { seq, at: 8 * SEC, clips: None }));
        assert!(d.project.clips.len() > n);
        assert_eq!(d.undo().as_deref(), Some("Split"));
        assert!(d.model.content_eq(&before));
        d.redo();
        assert!(d.project.clips.len() > n);
    }

    #[test]
    fn gestures_are_one_undo_step() {
        let mut d = Doc::from_view(&view::demo());
        let id = d.project.clips[0].id;
        for i in 0..10 {
            d.edit_gesture("opacity", "Opacity", |p| p.clip_mut(id).unwrap().transform.opacity = 90.0 - i as f32);
        }
        d.end_gesture();
        assert!((d.project.clip(id).unwrap().transform.opacity - 81.0).abs() < 0.01);
        d.undo();
        assert!((d.project.clip(id).unwrap().transform.opacity - 100.0).abs() < 0.01);
        assert!(!d.can_undo());
    }

    #[test]
    fn save_and_load_round_trip() {
        let d = Doc::from_view(&view::demo());
        let dir = std::env::temp_dir().join(format!("studio-app-test-{}", std::process::id()));
        let folder = m::file::ProjectFolder::create(&dir).unwrap();
        folder.save(&d.model).unwrap();
        let loaded = folder.load().unwrap();
        assert!(loaded.project.content_eq(&d.model));
        let back = Doc::new(loaded.project);
        assert_eq!(back.project.clips.len(), d.project.clips.len());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn first_two_free_clips(d: &Doc) -> (u32, u32) {
        let spine = d.project.spine();
        let v: Vec<u32> = d.project.clips.iter().filter(|c| Some(c.lane) != spine).map(|c| c.id).collect();
        (v[0], v[1])
    }

    /// Two trims, each its own gesture key, undo one at a time; a
    /// cancelled one leaves the earlier trim alone.
    #[test]
    fn each_trim_is_its_own_undo_step() {
        let mut d = Doc::from_view(&view::demo());
        let (a, b) = first_two_free_clips(&d);
        let seq = d.seq();
        let dur = |d: &Doc, id| d.project.clip(id).unwrap().dur;
        let (da, db) = (dur(&d, a), dur(&d, b));
        let trim = |id: u32, delta| m::Edit::Trim { seq, clip: m::ClipId(id as u64), edge: m::Edge::End, delta, ripple: false };
        assert!(d.apply_gesture(&format!("tl-trim-{a}"), "Trim", trim(a, -SEC / 4)));
        d.end_gesture();
        assert!(d.apply_gesture(&format!("tl-trim-{b}"), "Trim", trim(b, -SEC / 4)));
        d.end_gesture();
        assert_eq!(d.history_len(), 2);
        // A third, cancelled with Esc: only it goes back.
        assert!(d.apply_gesture(&format!("tl-trim-{a}-2"), "Trim", trim(a, -SEC / 10)));
        d.cancel_gesture();
        assert_eq!(dur(&d, a), da - SEC / 4);
        assert_eq!(d.history_len(), 2);
        d.undo();
        assert_eq!(dur(&d, b), db);
        assert_eq!(dur(&d, a), da - SEC / 4, "the earlier trim of the other clip stays");
        d.undo();
        assert_eq!(dur(&d, a), da);
    }

    #[test]
    fn undo_reports_the_clips_it_changed() {
        let mut d = Doc::from_view(&view::demo());
        let (a, _) = first_two_free_clips(&d);
        let seq = d.seq();
        assert!(d.apply("Trim", m::Edit::Trim { seq, clip: m::ClipId(a as u64), edge: m::Edge::End, delta: -SEC / 4, ripple: false }));
        d.undo();
        assert_eq!(d.take_history_touched(), vec![a]);
        d.redo();
        assert_eq!(d.take_history_touched(), vec![a]);
        assert!(d.take_history_touched().is_empty());
    }

    #[test]
    fn refused_edit_changes_nothing() {
        let mut d = Doc::from_view(&view::demo());
        let before = d.model.clone();
        let seq = d.seq();
        assert!(!d.apply("Trim", m::Edit::Trim { seq, clip: m::ClipId(999_999), edge: m::Edge::End, delta: SEC, ripple: false }));
        assert!(d.error.is_some());
        assert!(d.model.content_eq(&before));
    }
}

/// A broken invariant in words an agent can act on: the clips and lane by
/// name and time, not `Overlap(150, 186)`.
fn plain(p: &m::Project, v: &m::Violation) -> String {
    let secs = |t: m::Micros| format!("{:.2} s", t as f64 / 1e6);
    let clip = |id: u64| {
        p.sequences.iter().find_map(|s| s.clip(m::ClipId(id)).map(|c| (s, c))).map_or(format!("clip {id}"), |(s, c)| {
            let lane = s.lane(c.lane).map_or(format!("lane {}", c.lane.0), |l| if l.name.is_empty() { format!("lane {}", l.id.0) } else { format!("lane {} \"{}\"", l.id.0, l.name) });
            let name = if c.name.is_empty() { String::new() } else { format!(" \"{}\"", c.name) };
            format!("clip {id}{name} ({} to {} on {lane})", secs(c.start), secs(c.end()))
        })
    };
    match v {
        m::Violation::Overlap(a, b) => format!("{} and {} would overlap in time on the same lane; move one to another lane or shorten the first", clip(*a), clip(*b)),
        m::Violation::SpineNotPacked(c) => format!("the main lane would get a hole or an overlap at {}", clip(*c)),
        m::Violation::BadTiming(c) => format!("{} would get a zero or negative length, a negative start or a bad speed", clip(*c)),
        m::Violation::GapOffSpine(c) => format!("{} is a gap and gaps only sit on the main lane", clip(*c)),
        m::Violation::SourceOutOfRange(c) => format!("{} would use more of its media than the file holds", clip(*c)),
        m::Violation::BadTrack(c, prop) => format!("{} would get a broken keyframe track for \"{prop}\"", clip(*c)),
        other => other.to_string(),
    }
}
