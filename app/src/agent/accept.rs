// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Findings the agent has looked at and meant: `check {accept: [{kind,
//! clip, why}]}`. They are kept with the project (`accepted.json` in its
//! folder; in the session without one), and `check` and `review` list them
//! as accepted instead of as problems, so an intended choice (a boat sunk
//! in the sea, one long held shot) stops blocking the film.
//!
//! An acceptance names a kind and, optionally, a clip: without a clip it
//! covers that kind everywhere. `accept: []` with `clear: true` forgets
//! them all.

use super::Session;
use serde_json::{Value, json};
use std::path::PathBuf;
use studio_model as m;

impl Session {
    fn accept_file(&self) -> Option<PathBuf> {
        self.doc.folder.as_ref().map(|f| f.root().to_path_buf()).or_else(|| self.home.clone()).map(|h| m::file::ProjectFolder::new(h).state(m::file::State::Accepted))
    }

    /// Every acceptance: {kind, clip?, why?}.
    pub(super) fn accepted_items(&self) -> Vec<Value> {
        if let Some(p) = self.accept_file()
            && let Ok(t) = std::fs::read_to_string(p)
            && let Ok(Value::Array(v)) = serde_json::from_str::<Value>(&t)
        {
            return v;
        }
        self.accepted.clone()
    }

    fn save_accepted(&mut self, items: Vec<Value>) -> Result<(), String> {
        if let Some(p) = self.accept_file() {
            std::fs::write(&p, serde_json::to_string_pretty(&items).unwrap_or_default()).map_err(|e| format!("{}: {e}", p.display()))?;
        }
        self.accepted = items;
        Ok(())
    }

    /// `accept` (and `clear`) of a `check` call: what was added.
    pub(super) fn take_accepts(&mut self, a: &Value) -> Result<Option<Value>, String> {
        let clear = a.get("clear").and_then(Value::as_bool) == Some(true);
        let Some(list) = a.get("accept") else {
            if clear {
                self.save_accepted(Vec::new())?;
                return Ok(Some(json!({"accepted": [], "note": "every acceptance forgotten"})));
            }
            return Ok(None);
        };
        let list: Vec<Value> = match list {
            Value::Array(v) => v.clone(),
            o @ Value::Object(_) => vec![o.clone()],
            _ => return Err("accept is a list of {\"kind\": \"3d_buried\", \"clip\": 12, \"why\": \"the boat sits in the water\"} (clip optional: without it, that kind everywhere)".into()),
        };
        let mut items = if clear { Vec::new() } else { self.accepted_items() };
        for it in &list {
            let kind = it.get("kind").and_then(Value::as_str).ok_or("each accepted item needs \"kind\" (as check or review reported it)")?;
            let clip = it.get("clip").and_then(Value::as_u64);
            let why = it.get("why").and_then(Value::as_str).unwrap_or("");
            items.retain(|x| !(x["kind"] == kind && x.get("clip").and_then(Value::as_u64) == clip));
            let mut e = json!({"kind": kind});
            if let Some(c) = clip {
                e["clip"] = json!(c);
            }
            if !why.is_empty() {
                e["why"] = json!(why);
            }
            items.push(e);
        }
        self.save_accepted(items.clone())?;
        Ok(Some(json!(items)))
    }

    /// Accepts one finding as intended: `kind` on `clip`, or everywhere without one.
    pub fn accept_finding(&mut self, kind: &str, clip: Option<u32>, why: &str) -> Result<(), String> {
        let mut item = json!({"kind": kind});
        if let Some(c) = clip {
            item["clip"] = json!(c);
        }
        if !why.is_empty() {
            item["why"] = json!(why);
        }
        self.take_accepts(&json!({"accept": [item]})).map(|_| ())
    }

    /// Takes an acceptance back: `kind` on `clip` (or the kind-wide one when `clip` is none).
    pub fn unaccept_finding(&mut self, kind: &str, clip: Option<u32>) -> Result<(), String> {
        let mut items = self.accepted_items();
        items.retain(|x| !(x["kind"] == kind && x.get("clip").and_then(Value::as_u64) == clip.map(u64::from)));
        self.save_accepted(items)
    }

    /// Whether a finding of `kind` on `clips` is accepted.
    fn is_accepted(items: &[Value], kind: &str, clips: &[u64]) -> bool {
        items.iter().any(|x| x["kind"] == kind && x.get("clip").and_then(Value::as_u64).is_none_or(|c| clips.is_empty() || clips.contains(&c)))
    }

    /// Moves accepted findings out of a list (`check`'s problems, `review`'s
    /// notes) into `accepted`; true when any moved.
    pub(super) fn apply_accepted(&self, v: &mut Value, list: &str) -> bool {
        let items = self.accepted_items();
        if items.is_empty() {
            return false;
        }
        let Some(all) = v.get_mut(list).and_then(Value::as_array_mut) else { return false };
        let (mut keep, mut moved) = (Vec::new(), Vec::new());
        for p in all.drain(..) {
            let kind = p["kind"].as_str().unwrap_or("").to_owned();
            let mut clips: Vec<u64> = p.get("clips").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
            clips.extend(p.get("clip").and_then(Value::as_u64));
            if Self::is_accepted(&items, &kind, &clips) { moved.push(p) } else { keep.push(p) }
        }
        *all = keep;
        if moved.is_empty() {
            return false;
        }
        v["accepted"] = json!(moved);
        true
    }
}
