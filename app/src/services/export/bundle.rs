// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The delivery bundle: master, proxy, sound and stems, captions, thumbnails, poster and
//! cutdowns in one folder with a manifest. The agent's `bundle` tool and the Export screen both
//! go through [`bundle`]; the writing itself lives with the agent session, which owns the frame
//! renderer the thumbnails and poster need.

use crate::agent::Session;
use crate::doc::Doc;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// What goes into the bundle. `Default` is everything but cutdowns.
#[derive(Clone, Debug)]
pub struct BundleReq {
    pub name: Option<String>,
    /// The folder to write; default `<project>/deliveries/<name>`.
    pub out: Option<PathBuf>,
    pub spec: Option<String>,
    pub master: bool,
    pub proxy: bool,
    pub captions: bool,
    pub audio: bool,
    pub stems: bool,
    /// How many thumbnails (0 for none).
    pub thumbnails: u32,
    pub poster: bool,
    /// Lengths in seconds of shorter versions to include.
    pub cutdowns: Vec<f64>,
    /// A vertical 9:16 version of each cutdown too.
    pub verticals: bool,
}

impl Default for BundleReq {
    fn default() -> Self {
        Self { name: None, out: None, spec: None, master: true, proxy: true, captions: true, audio: true, stems: true, thumbnails: 8, poster: true, cutdowns: Vec::new(), verticals: false }
    }
}

impl BundleReq {
    /// The arguments the agent tool takes.
    pub fn to_args(&self) -> Value {
        let mut a = json!({"master": self.master, "proxy": self.proxy, "captions": self.captions, "audio": self.audio, "stems": self.stems, "poster": self.poster, "verticals": self.verticals, "cutdowns": self.cutdowns});
        a["thumbnails"] = if self.thumbnails == 0 { json!(false) } else { json!(self.thumbnails) };
        if let Some(n) = &self.name {
            a["name"] = json!(n);
        }
        if let Some(o) = &self.out {
            a["out"] = json!(o.display().to_string());
        }
        if let Some(s) = &self.spec {
            a["spec"] = json!(s);
        }
        a
    }

    /// How many things the bundle will make, for a checklist summary.
    pub fn parts(&self) -> usize {
        [self.master, self.proxy, self.captions, self.audio, self.stems && self.audio, self.thumbnails > 0, self.poster].iter().filter(|b| **b).count() + self.cutdowns.len() * if self.verticals { 2 } else { 1 }
    }
}

#[derive(Clone, Debug)]
pub struct BundleOut {
    pub folder: PathBuf,
    pub files: usize,
    /// What could not be made, each with the reason.
    pub missing: Vec<String>,
    pub notes: Vec<Value>,
    pub manifest: PathBuf,
}

impl BundleOut {
    pub fn to_json(&self) -> Value {
        json!({"folder": self.folder.display().to_string(), "files": self.files, "missing": self.missing, "notes": self.notes, "manifest": self.manifest.display().to_string()})
    }
}

/// Writes the bundle for `doc`, saved at `root`. Works on a copy; the project is not changed.
pub fn bundle(doc: &Doc, root: &Path, req: &BundleReq) -> Result<BundleOut, String> {
    let mut s = Session::new(Doc::new(doc.model.clone()));
    s.home = Some(root.to_path_buf());
    s.bundle_at(&req.to_args(), root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_bundle_is_everything_but_cutdowns() {
        let r = BundleReq::default();
        assert_eq!(r.parts(), 7);
        let a = r.to_args();
        assert_eq!(a["thumbnails"], 8);
        assert_eq!(a["master"], true);
        let none = BundleReq { thumbnails: 0, poster: false, cutdowns: vec![15.0, 30.0], verticals: true, ..Default::default() };
        assert_eq!(none.to_args()["thumbnails"], false);
        assert_eq!(none.parts(), 5 + 4);
    }
}
