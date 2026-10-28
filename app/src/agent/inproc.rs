// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! An agent running inside the app, reading the engine directly.
//!
//! The panel's agents already call tools in-process through the
//! [`Gate`]; what they got back were JSON strings and JPEG bytes meant for
//! the wire. [`Inside`] is the same calls without the wire:
//!
//! * [`Inside::call`] goes through the same gate as every other call (the
//!   same pause, Stop, step limit, approvals, guard and recorder: there is no
//!   other way in), and its [`Reply`] gives the JSON parsed once and the
//!   pictures as the JPEG bytes in memory, never base64.
//! * [`Inside::frame`] is a picture of the timeline at a time, as bytes.
//! * [`Inside::project`] is the project model itself, read-only, as the
//!   window has it (an `Arc` shared with the window, updated when the
//!   revision moves), so reading the scene or the timeline costs no tool
//!   call at all. It is refused after Stop like a call. Changes are never
//!   made through it: they go through `edit`, so they are checked, undoable
//!   and recorded.
//!
//! Nothing here opens a socket or a file; it adds no way in from outside
//! the process.
//!
//! The API brain (`openai.rs`) and the crew still take the JSON replies;
//! moving them onto [`Inside`] is the next step, so it is unused for now.
#![allow(dead_code)]

use super::Content;
use super::brain::Gate;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use studio_model as m;

/// A tool's answer, as it is in memory.
pub struct Reply(pub Vec<Content>);

impl Reply {
    /// The text parts, joined.
    pub fn text(&self) -> String {
        self.0.iter().filter_map(|p| if let Content::Text(t) = p { Some(t.as_str()) } else { None }).collect::<Vec<_>>().join("\n")
    }

    /// The first text part as JSON, when it is JSON.
    pub fn json(&self) -> Option<Value> {
        self.0.iter().find_map(|p| if let Content::Text(t) = p { serde_json::from_str(t).ok() } else { None })
    }

    /// The pictures, as JPEG bytes.
    pub fn pictures(&self) -> impl Iterator<Item = &[u8]> {
        self.0.iter().filter_map(|p| if let Content::Jpeg(b) = p { Some(b.as_slice()) } else { None })
    }
}

/// The project as the window has it, at a revision.
#[derive(Clone)]
pub struct Snapshot {
    pub revision: u64,
    pub model: Arc<m::Project>,
}

static WANTED: AtomicBool = AtomicBool::new(false);
static LATEST: Mutex<Option<Snapshot>> = Mutex::new(None);

/// Called by the window as it runs: keeps [`LATEST`] at the window's
/// revision, only while an inside agent reads it (nothing is copied
/// otherwise), and only when the revision moved.
pub fn publish_if_wanted(model: &m::Project, revision: u64) {
    if !WANTED.load(Ordering::Relaxed) {
        return;
    }
    let mut l = LATEST.lock().unwrap_or_else(|e| e.into_inner());
    if l.as_ref().is_some_and(|s| s.revision == revision) {
        return;
    }
    *l = Some(Snapshot { revision, model: Arc::new(model.clone()) });
}

/// An agent inside the app.
pub struct Inside {
    gate: Arc<Gate>,
}

impl Inside {
    pub fn new(gate: Arc<Gate>) -> Self {
        WANTED.store(true, Ordering::Relaxed);
        Inside { gate }
    }

    /// A tool call, through the gate.
    pub fn call(&self, tool: &str, args: &Value) -> Result<Reply, String> {
        self.gate.call_as(None, tool, args).map(Reply)
    }

    /// The picture at `t` seconds, as JPEG bytes.
    pub fn frame(&self, t: f64) -> Result<Vec<u8>, String> {
        let r = self.call("frame", &serde_json::json!({"t": t}))?;
        r.pictures().next().map(<[u8]>::to_vec).ok_or_else(|| r.text())
    }

    /// The project model, read-only, without a tool call.
    pub fn project(&self) -> Result<Snapshot, String> {
        if self.gate.stopped() {
            return Err("The user stopped this request. Do nothing more.".into());
        }
        LATEST.lock().unwrap_or_else(|e| e.into_inner()).clone().ok_or_else(|| "the window has not shared its project yet; call summary".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_reads_without_the_wire() {
        let r = Reply(vec![Content::Text("{\"ok\":true}".into()), Content::Jpeg(vec![0xFF, 0xD8])]);
        assert_eq!(r.json().unwrap()["ok"], true);
        assert_eq!(r.pictures().next().unwrap(), &[0xFF, 0xD8]);
    }

    #[test]
    fn the_project_is_shared_only_when_wanted_and_when_it_moved() {
        let p = crate::services::project::new_custom("Inside", 640, 360, 30.0).model;
        publish_if_wanted(&p, 1);
        WANTED.store(true, Ordering::Relaxed);
        publish_if_wanted(&p, 7);
        let a = LATEST.lock().unwrap().clone().unwrap();
        publish_if_wanted(&p, 7);
        let b = LATEST.lock().unwrap().clone().unwrap();
        assert_eq!(a.revision, 7);
        assert!(Arc::ptr_eq(&a.model, &b.model), "the same revision is not copied again");
    }
}
