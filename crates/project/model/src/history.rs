// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Undo/redo stacks and transactions.

use crate::edit::{Edit, EditError};
use crate::project::Project;

/// Who made a step: the person at the editor, or an AI agent working on
/// the same project (each has its own Undo).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Author {
    #[default]
    Person,
    /// An agent's tool call `call`, part of request `request`.
    Agent { request: u64, call: u64 },
}

impl Author {
    pub fn is_person(&self) -> bool {
        *self == Author::Person
    }
}

/// One undoable step.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Step {
    /// What the user did, shown in "Undo …" menu items.
    pub label: String,
    /// The edit that reverses the step.
    pub inverse: Edit,
    /// Who made it.
    #[serde(default, skip_serializing_if = "Author::is_person")]
    pub author: Author,
}

/// Undo / redo stacks as stored on disk.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SavedHistory {
    pub undo: Vec<Step>,
    pub redo: Vec<Step>,
    pub revision: u64,
}

/// Undo and redo stacks for one project.
///
/// Every change goes through [`History::apply`]. A *transaction* groups
/// several applies into one undo step — use it for drags, where each mouse
/// move applies an edit for live feedback but the whole gesture should
/// undo at once:
///
/// ```
/// # use studio_model::*;
/// let mut project = Project::default();
/// let mut history = History::new();
/// history.begin("Rename");
/// let seq = project.active_sequence;
/// history.apply(&mut project, "", Edit::RenameSequence { id: seq, name: "A".into() }).unwrap();
/// history.apply(&mut project, "", Edit::RenameSequence { id: seq, name: "B".into() }).unwrap();
/// history.commit();
/// history.undo(&mut project).unwrap();
/// assert_eq!(project.active().name, "Sequence 1");
/// ```
#[derive(Debug, Default)]
pub struct History {
    undo: Vec<Step>,
    redo: Vec<Step>,
    open: Option<Open>,
    limit: Option<usize>,
    revision: u64,
    saved_revision: u64,
    /// Who the steps applied from now on belong to.
    author: Author,
}

#[derive(Debug)]
struct Open {
    label: String,
    author: Author,
    depth: usize,
    inverses: Vec<Edit>,
    /// Where each nested level's edits start in `inverses`, innermost
    /// last: a nested level is cancelled on its own.
    starts: Vec<usize>,
    /// The revision when the transaction opened, and at each nested level:
    /// cancelling puts the project back, so the revision goes back with it.
    revision: u64,
    level_revisions: Vec<u64>,
}

impl History {
    /// Empty history with no step limit.
    pub fn new() -> Self {
        Self::default()
    }

    /// Keeps at most `limit` undo steps (oldest are dropped first).
    pub fn with_limit(limit: usize) -> Self {
        Self { limit: Some(limit), ..Self::default() }
    }

    /// Applies `edit` to `project` and records it. Inside a transaction the
    /// step joins the transaction and `label` is ignored.
    pub fn apply(&mut self, project: &mut Project, label: impl Into<String>, edit: Edit) -> Result<(), EditError> {
        let inverse = project.apply(edit)?;
        if inverse.is_empty() {
            // Nothing changed (e.g. a zero-length trim): no undo step.
            return Ok(());
        }
        self.revision += 1;
        match &mut self.open {
            Some(open) => open.inverses.push(inverse),
            None => self.push_undo(Step { label: label.into(), inverse, author: self.author }),
        }
        // A new change ends what this author could redo; the other's stays.
        let author = self.open.as_ref().map_or(self.author, |o| o.author);
        self.redo.retain(|s| s.author != author);
        Ok(())
    }

    /// Starts a transaction (nesting is allowed; only the outermost
    /// `commit` closes it).
    pub fn begin(&mut self, label: impl Into<String>) {
        match &mut self.open {
            Some(open) => {
                open.depth += 1;
                open.starts.push(open.inverses.len());
                open.level_revisions.push(self.revision);
            }
            None => self.open = Some(Open { label: label.into(), author: self.author, depth: 1, inverses: Vec::new(), starts: Vec::new(), revision: self.revision, level_revisions: Vec::new() }),
        }
    }

    /// Whether a transaction is open.
    pub fn in_transaction(&self) -> bool {
        self.open.is_some()
    }

    /// Who the open transaction belongs to.
    pub fn open_author(&self) -> Option<Author> {
        self.open.as_ref().map(|o| o.author)
    }

    /// Makes the steps applied from now on `author`'s; returns who they
    /// belonged to before.
    pub fn set_author(&mut self, author: Author) -> Author {
        std::mem::replace(&mut self.author, author)
    }

    /// Closes the current transaction level; the outermost one becomes a
    /// single undo step (none if nothing was applied).
    pub fn commit(&mut self) {
        let Some(open) = &mut self.open else { return };
        open.depth -= 1;
        if open.depth > 0 {
            open.starts.pop();
            open.level_revisions.pop();
            return;
        }
        let open = self.open.take().expect("open transaction");
        if open.inverses.is_empty() {
            return;
        }
        let mut inverses = open.inverses;
        inverses.reverse();
        let inverse = if inverses.len() == 1 { inverses.pop().expect("one") } else { Edit::Batch { edits: inverses } };
        self.push_undo(Step { label: open.label, inverse, author: open.author });
    }

    /// Abandons the innermost open transaction level, reverting its edits
    /// (e.g. a drag cancelled with Escape). The levels around it stay open
    /// with their own edits: a failed edit inside a longer one (an agent's
    /// request, a batch of ops) takes back only itself.
    pub fn cancel(&mut self, project: &mut Project) -> Result<(), EditError> {
        if let Some(open) = &mut self.open
            && let Some(start) = open.starts.pop()
        {
            open.depth -= 1;
            let back_to = open.level_revisions.pop();
            let mut inverses = open.inverses.split_off(start);
            inverses.reverse();
            if !inverses.is_empty() {
                project.apply(Edit::Batch { edits: inverses })?;
                // The project is as it was, so is its revision: a failed
                // edit must not look like a change.
                self.revision = back_to.unwrap_or(self.revision + 1);
            }
            return Ok(());
        }
        let Some(open) = self.open.take() else { return Ok(()) };
        let mut inverses = open.inverses;
        inverses.reverse();
        if !inverses.is_empty() {
            project.apply(Edit::Batch { edits: inverses })?;
            self.revision = open.revision;
        }
        Ok(())
    }

    /// Undoes the last step, returning its label (`None` if there was
    /// nothing to undo). An open transaction is committed first.
    pub fn undo(&mut self, project: &mut Project) -> Result<Option<String>, EditError> {
        self.close_all();
        let Some(step) = self.undo.pop() else { return Ok(None) };
        match project.apply(step.inverse.clone()) {
            Ok(redo) => {
                self.revision += 1;
                let label = step.label.clone();
                self.redo.push(Step { label: step.label, inverse: redo, author: step.author });
                Ok(Some(label))
            }
            Err(e) => {
                self.undo.push(step);
                Err(e)
            }
        }
    }

    /// Redoes the last undone step, returning its label.
    pub fn redo(&mut self, project: &mut Project) -> Result<Option<String>, EditError> {
        self.close_all();
        let Some(step) = self.redo.pop() else { return Ok(None) };
        match project.apply(step.inverse.clone()) {
            Ok(undo) => {
                self.revision += 1;
                let label = step.label.clone();
                self.undo.push(Step { label: step.label, inverse: undo, author: step.author });
                Ok(Some(label))
            }
            Err(e) => {
                self.redo.push(step);
                Err(e)
            }
        }
    }

    /// The edit that would undo the last step (after a redo or an apply:
    /// it names what that step changed).
    pub fn last_undo(&self) -> Option<&Edit> {
        self.undo.last().map(|s| &s.inverse)
    }

    /// The inverse of the edit applied last, inside an open transaction or
    /// not: in primitives, exactly what that edit changed.
    pub fn last_inverse(&self) -> Option<&Edit> {
        match &self.open {
            Some(o) => o.inverses.last(),
            None => self.undo.last().map(|s| &s.inverse),
        }
    }

    /// The edit that would redo the last undone step (after an undo: it
    /// names what the undo changed).
    pub fn last_redo(&self) -> Option<&Edit> {
        self.redo.last().map(|s| &s.inverse)
    }

    /// Whether there is a step to undo.
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty() || self.open.as_ref().is_some_and(|o| !o.inverses.is_empty())
    }

    /// Whether there is a step to redo.
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Label of the step [`History::undo`] would undo.
    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|s| s.label.as_str())
    }

    /// Label of the step [`History::redo`] would redo.
    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|s| s.label.as_str())
    }

    /// Number of undo steps.
    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    /// Forgets all steps (e.g. after opening another project).
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.open = None;
    }

    /// Counts every change made through this history; compare values to
    /// tell whether anything happened.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The undo and redo stacks and the revision, to keep beside a saved
    /// project so a later session can still undo. An open transaction is
    /// in it as the step it will become, and stays open: saving in the
    /// middle of a drag or an agent's change does not split it.
    pub fn snapshot(&self) -> SavedHistory {
        let mut undo = self.undo.clone();
        if let Some(open) = self.open.as_ref().filter(|o| !o.inverses.is_empty()) {
            let mut inverses = open.inverses.clone();
            inverses.reverse();
            let inverse = if inverses.len() == 1 { inverses.pop().expect("one") } else { Edit::Batch { edits: inverses } };
            undo.push(Step { label: open.label.clone(), inverse, author: open.author });
        }
        SavedHistory { undo, redo: self.redo.clone(), revision: self.revision }
    }

    /// The undo steps, oldest first.
    pub fn undo_steps(&self) -> &[Step] {
        &self.undo
    }

    /// The redo steps, the next to redo last.
    pub fn redo_steps(&self) -> &[Step] {
        &self.redo
    }

    /// Takes undo step `i` out of the stack without applying anything (an
    /// undo worked out by the caller, see [`History::record_undone`]).
    pub fn take_undo(&mut self, i: usize) -> Step {
        self.undo.remove(i)
    }

    /// Takes redo step `i` out of the stack without applying anything.
    pub fn take_redo(&mut self, i: usize) -> Step {
        self.redo.remove(i)
    }

    /// Gives undo step `i` a new inverse (its undo worked out again after
    /// an earlier step was taken out from under it).
    pub fn set_undo_inverse(&mut self, i: usize, inverse: Edit) {
        self.undo[i].inverse = inverse;
    }

    /// Records a step undone by the caller: `redo` brings it back.
    pub fn record_undone(&mut self, step: Step) {
        self.revision += 1;
        self.redo.push(step);
    }

    /// Records a step redone by the caller: `undo` takes it back.
    pub fn record_redone(&mut self, step: Step) {
        self.revision += 1;
        self.push_undo(step);
    }

    /// Puts back stacks from [`History::snapshot`]. Only valid on the
    /// project state they were taken from.
    pub fn restore(&mut self, saved: SavedHistory) {
        self.undo = saved.undo;
        self.redo = saved.redo;
        self.open = None;
        self.revision = saved.revision;
        self.saved_revision = saved.revision;
    }

    /// Records that the project was just saved.
    pub fn mark_saved(&mut self) {
        self.saved_revision = self.revision;
    }

    /// Marks the project as saved as it was at `revision` (a save that ran
    /// in the background while editing went on).
    pub fn mark_saved_at(&mut self, revision: u64) {
        self.saved_revision = revision;
    }

    /// Whether the project changed since [`History::mark_saved`].
    pub fn is_dirty(&self) -> bool {
        self.revision != self.saved_revision
    }

    fn close_all(&mut self) {
        while self.open.is_some() {
            self.commit();
        }
    }

    fn push_undo(&mut self, step: Step) {
        self.undo.push(step);
        if let Some(limit) = self.limit
            && self.undo.len() > limit
        {
            let excess = self.undo.len() - limit;
            self.undo.drain(..excess);
        }
    }
}
