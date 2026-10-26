// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Loading models into `studio-nn` sessions.
//!
//! Compiling a session (parsing, constant folding, weight packing) takes from
//! a few hundred milliseconds to seconds, so compiled sessions are kept in a
//! process-wide table keyed by file path and shared between features and
//! jobs. [`unload_all`] frees them (e.g. when memory runs low).

use crate::error::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use studio_nn::{BackendKind, Model, Options, Session};

pub use studio_nn::Tensor;

type Table = Mutex<HashMap<(PathBuf, Option<String>), Arc<Session>>>;

fn table() -> &'static Table {
    static T: OnceLock<Table> = OnceLock::new();
    T.get_or_init(Default::default)
}

static BACKEND: Mutex<BackendKind> = Mutex::new(BackendKind::Auto);

/// Chooses the backend for sessions created from now on (Settings >
/// Performance). The default is Metal when available, otherwise CPU.
pub fn set_backend(kind: BackendKind) {
    if let Ok(mut b) = BACKEND.lock() {
        *b = kind;
    }
    unload_all();
}

fn options() -> Options {
    Options::default().with_backend(BACKEND.lock().map(|b| *b).unwrap_or_default())
}

/// The compiled session for a model file (`.onnx` or `.tflite`).
pub fn session(path: &Path) -> Result<Arc<Session>> {
    load(path, None, || Model::load(path).map_err(Into::into))
}

/// The compiled session for one `.tflite` member of a MediaPipe `.task`
/// bundle.
pub fn task_session(path: &Path, member: &str) -> Result<Arc<Session>> {
    load(path, Some(member.to_string()), || {
        let models = studio_nn::tflite::load_task(path)?;
        models.into_iter().find(|(n, _)| n.ends_with(member)).map(|(_, m)| m).ok_or_else(|| crate::Error::Inference(format!("{}: no member {member}", path.display())))
    })
}

fn load(path: &Path, member: Option<String>, model: impl FnOnce() -> Result<Model>) -> Result<Arc<Session>> {
    let key = (path.to_path_buf(), member);
    if let Some(s) = table().lock().ok().and_then(|t| t.get(&key).cloned()) {
        return Ok(s);
    }
    // Compile outside the lock: other features keep working meanwhile.
    let m = model()?;
    let s = Arc::new(Session::new(&m, options())?);
    if let Ok(mut t) = table().lock() {
        t.insert(key, s.clone());
    }
    Ok(s)
}

/// Drops every cached session (sessions in use stay alive until released).
pub fn unload_all() {
    if let Ok(mut t) = table().lock() {
        t.clear();
    }
}

/// Runs a session and returns its outputs by name.
pub(crate) fn run(s: &Session, inputs: Vec<(&str, Tensor)>) -> Result<Outputs> {
    let outs = s.run_owned(inputs.into_iter().map(|(n, t)| (n.to_string(), t)).collect())?;
    Ok(Outputs(outs))
}

pub(crate) struct Outputs(Vec<(String, Tensor)>);

impl Outputs {
    /// The first output's name.
    pub fn first_name(&self) -> Option<String> {
        self.0.first().map(|(n, _)| n.clone())
    }

    pub fn take(&mut self, name: &str) -> Result<Tensor> {
        let i = self.0.iter().position(|(n, _)| n == name).ok_or_else(|| crate::Error::Inference(format!("model has no output {name}")))?;
        Ok(self.0.swap_remove(i).1)
    }
}
