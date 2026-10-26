// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Macros: record a sequence of edits once, replay it on other clips (or
//! in other projects).
//!
//! Recording watches every edit the document applies (by hand in the
//! window, or by the AI): [`note`] is called by `Doc` before each one.
//! Changes to the clips that were selected when recording started (the
//! subjects) are kept as the fields that changed, so replaying puts the
//! same change on other clips instead of copying the subject over them;
//! other edits are kept whole, with subject clip ids and ids made during
//! the recording renumbered on replay. A macro recorded on one clip runs
//! once per target clip; one recorded on several maps them in timeline
//! order.
//!
//! Macros are saved in `<Studio>/Macros/<name>.json`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use studio_model as m;

use crate::doc::Doc;

/// One recorded step.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum Step {
    /// The fields of subject `subject` (index into the subjects) that
    /// changed, with their new values.
    ClipChange { label: String, subject: usize, fields: serde_json::Map<String, Value> },
    /// A model edit as it was made (ids renumbered on replay).
    Edit { label: String, edit: Value },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Macro {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// How many clips it was recorded on.
    pub subjects: usize,
    /// The first id made while recording; ids from here up are renumbered.
    pub first_new_id: u64,
    /// Subject clip ids when recorded (renumbered to the targets).
    pub subject_ids: Vec<u64>,
    pub steps: Vec<Step>,
}

struct Recording {
    subjects: Vec<u64>,
    first_new_id: u64,
    steps: Vec<Step>,
}

thread_local! {
    // The window's document lives on one thread, a headless session on
    // another; each records only its own edits.
    static RECORDING: RefCell<Option<Recording>> = const { RefCell::new(None) };
    static REPLAYING: RefCell<bool> = const { RefCell::new(false) };
}

/// Whether a macro is being recorded on this thread.
pub fn recording() -> bool {
    RECORDING.with(|r| r.borrow().is_some())
}

/// Starts recording, with the given clips as subjects (empty: edits are
/// kept whole).
pub fn start(doc: &Doc, subjects: &[u32]) {
    let mut subjects: Vec<u64> = subjects.iter().map(|&c| c as u64).collect();
    subjects.sort_by_key(|id| doc.model.active().clip(m::ClipId(*id)).map_or(0, |c| c.start));
    RECORDING.with(|r| *r.borrow_mut() = Some(Recording { subjects, first_new_id: doc.model.next_id, steps: Vec::new() }));
}

/// Stops recording and returns the macro (not saved yet), or `None` when
/// nothing was recording.
pub fn stop(name: &str, description: &str) -> Option<Macro> {
    let rec = RECORDING.with(|r| r.borrow_mut().take())?;
    Some(Macro { name: name.to_owned(), description: description.to_owned(), subjects: rec.subjects.len(), first_new_id: rec.first_new_id, subject_ids: rec.subjects, steps: rec.steps })
}

/// Called by `Doc` with each edit before it is applied.
pub fn note(model: &m::Project, label: &str, edit: &m::Edit) {
    if REPLAYING.with(|r| *r.borrow()) || !recording() {
        return;
    }
    let mut steps = Vec::new();
    collect(model, label, edit, &mut steps);
    RECORDING.with(|r| {
        if let Some(rec) = r.borrow_mut().as_mut() {
            for s in steps {
                // Subject changes are resolved against the subjects now.
                rec.steps.push(s);
            }
        }
    });
}

fn collect(model: &m::Project, label: &str, edit: &m::Edit, out: &mut Vec<Step>) {
    let subjects: Vec<u64> = RECORDING.with(|r| r.borrow().as_ref().map(|x| x.subjects.clone()).unwrap_or_default());
    match edit {
        m::Edit::Batch { edits } => {
            for e in edits {
                collect(model, label, e, out);
            }
        }
        m::Edit::SetClip { seq, clip } if subjects.contains(&clip.id.0) => {
            let Some(before) = model.sequence(*seq).and_then(|s| s.clip(clip.id)) else { return };
            let (Ok(Value::Object(a)), Ok(Value::Object(b))) = (serde_json::to_value(before), serde_json::to_value(&**clip)) else { return };
            let mut fields = serde_json::Map::new();
            for (k, v) in b {
                // Where the clip is and what it shows stay the target's own.
                if matches!(k.as_str(), "id" | "lane" | "start" | "duration" | "source_in" | "source" | "name" | "link" | "group" | "parent") {
                    continue;
                }
                if a.get(&k) != Some(&v) {
                    fields.insert(k, v);
                }
            }
            if !fields.is_empty() {
                let subject = subjects.iter().position(|s| *s == clip.id.0).unwrap_or(0);
                out.push(Step::ClipChange { label: label.to_owned(), subject, fields });
            }
        }
        e => {
            if let Ok(v) = serde_json::to_value(e) {
                out.push(Step::Edit { label: label.to_owned(), edit: v });
            }
        }
    }
}

fn dir() -> PathBuf {
    studio_paths::studio().join("Macros")
}

fn file_name(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_alphanumeric() || " -_".contains(c) { c } else { '_' }).collect();
    format!("{}.json", s.trim())
}

pub fn save(mac: &Macro) -> Result<PathBuf, String> {
    if mac.name.trim().is_empty() {
        return Err("a macro needs a name".into());
    }
    std::fs::create_dir_all(dir()).map_err(|e| e.to_string())?;
    let path = dir().join(file_name(&mac.name));
    std::fs::write(&path, serde_json::to_vec_pretty(mac).unwrap_or_default()).map_err(|e| format!("could not save the macro: {e}"))?;
    Ok(path)
}

pub fn load(name: &str) -> Result<Macro, String> {
    let path = dir().join(file_name(name));
    let b = std::fs::read(&path).map_err(|_| format!("no macro \"{name}\"; macros are: {}", list().iter().map(|m| m.name.as_str()).collect::<Vec<_>>().join(", ")))?;
    serde_json::from_slice(&b).map_err(|e| format!("the macro \"{name}\" is damaged: {e}"))
}

pub fn delete(name: &str) -> Result<(), String> {
    std::fs::remove_file(dir().join(file_name(name))).map_err(|_| format!("no macro \"{name}\""))
}

/// Saved macros, by name.
pub fn list() -> Vec<Macro> {
    let mut v: Vec<Macro> = std::fs::read_dir(dir()).into_iter().flatten().flatten().filter_map(|e| std::fs::read(e.path()).ok()).filter_map(|b| serde_json::from_slice(&b).ok()).collect();
    v.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    v
}

/// Keys whose numbers are clip ids (or ids made while recording).
const ID_KEYS: &[&str] = &["clip", "clips", "left", "right", "targets", "new_clip", "before", "id", "effect", "mask", "group", "link", "new_sequence"];

struct Ids<'a> {
    subjects: &'a [u64],
    targets: &'a [u64],
    first_new: u64,
    fresh: HashMap<u64, u64>,
}

impl Ids<'_> {
    fn map(&mut self, doc: &mut Doc, n: u64) -> u64 {
        if let Some(i) = self.subjects.iter().position(|s| *s == n) {
            return self.targets.get(i).copied().unwrap_or(n);
        }
        if n >= self.first_new {
            return *self.fresh.entry(n).or_insert_with(|| doc.model.alloc_id());
        }
        n
    }

    fn walk(&mut self, doc: &mut Doc, v: &mut Value, id_key: bool) {
        match v {
            Value::Number(x) if id_key => {
                if let Some(n) = x.as_u64() {
                    *v = json!(self.map(doc, n));
                }
            }
            Value::Array(a) => {
                for x in a {
                    self.walk(doc, x, id_key);
                }
            }
            Value::Object(o) => {
                for (k, x) in o.iter_mut() {
                    if k == "seq" {
                        *x = json!(doc.seq().0);
                        continue;
                    }
                    self.walk(doc, x, ID_KEYS.contains(&k.as_str()));
                }
            }
            _ => {}
        }
    }
}

/// Runs a macro on `targets` (clip ids) as one undo step. Returns how many
/// steps were applied and which were refused (a step that does not fit a
/// target, such as trimming past its footage, is skipped).
pub fn run(doc: &mut Doc, mac: &Macro, targets: &[u32]) -> Result<(usize, Vec<String>), String> {
    let mut targets: Vec<u64> = targets.iter().map(|&c| c as u64).collect();
    targets.sort_by_key(|id| doc.model.active().clip(m::ClipId(*id)).map_or(0, |c| c.start));
    let groups: Vec<Vec<u64>> = if mac.subjects == 0 {
        vec![Vec::new()]
    } else if targets.is_empty() {
        return Err(format!("select the clip{} to run \"{}\" on", if mac.subjects == 1 { "" } else { "s" }, mac.name));
    } else if mac.subjects == 1 {
        targets.iter().map(|t| vec![*t]).collect()
    } else {
        targets.chunks(mac.subjects).map(<[u64]>::to_vec).collect()
    };
    let label = format!("Macro: {}", mac.name);
    REPLAYING.with(|r| *r.borrow_mut() = true);
    doc.begin_batch(&label);
    let mut applied = 0;
    let mut refused = Vec::new();
    for group in &groups {
        let mut ids = Ids { subjects: &mac.subject_ids, targets: group, first_new: mac.first_new_id, fresh: HashMap::new() };
        for step in &mac.steps {
            let r = match step {
                Step::ClipChange { label, subject, fields } => {
                    let Some(&target) = group.get(*subject) else { continue };
                    let seq = doc.seq();
                    let Some(clip) = doc.model.sequence(seq).and_then(|s| s.clip(m::ClipId(target))).cloned() else {
                        refused.push(format!("{label}: clip {target} is gone"));
                        continue;
                    };
                    let mut v = serde_json::to_value(&clip).unwrap_or(Value::Null);
                    for (k, x) in fields {
                        let mut x = x.clone();
                        // Effects and masks made in the recording get ids of
                        // their own on each target (their `id` fields).
                        ids.walk(doc, &mut x, false);
                        v[k.as_str()] = x;
                    }
                    serde_json::from_value::<m::Clip>(v).map_err(|e| e.to_string()).and_then(|c| doc.apply_replayed(label, m::Edit::SetClip { seq, clip: Box::new(c) }).map(|_| label.clone()))
                }
                Step::Edit { label, edit } => {
                    let mut e = edit.clone();
                    ids.walk(doc, &mut e, false);
                    serde_json::from_value::<m::Edit>(e).map_err(|e| e.to_string()).and_then(|e| doc.apply_replayed(label, e).map(|_| label.clone()))
                }
            };
            match r {
                Ok(_) => applied += 1,
                Err(e) => refused.push(e),
            }
        }
    }
    doc.commit_batch();
    REPLAYING.with(|r| *r.borrow_mut() = false);
    Ok((applied, refused))
}

/// A macro as the agent and the library show it.
pub fn json(mac: &Macro) -> Value {
    json!({"name": mac.name, "description": mac.description, "steps": mac.steps.len(), "recorded_on": mac.subjects, "what": mac.steps.iter().map(|s| match s { Step::ClipChange { label, fields, .. } => format!("{label} ({})", fields.keys().cloned().collect::<Vec<_>>().join(", ")), Step::Edit { label, .. } => label.clone() }).collect::<Vec<_>>()})
}
