// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Exchanging timelines with other tools.
//!
//! * [`otio`]: OpenTimelineIO JSON (`.otio`) import and export.
//! * [`fcpxml`]: FCPXML 1.9+ import (spine, clips, titles, basics) and,
//!   in [`fcpxml_write`], FCPXML 1.10 export.
//! * [`edl`]: CMX3600 EDL export of the spine and import.
//! * [`conform`]: relinking media by name, reel and timecode.
//!
//! Every function returns a [`Report`] listing what could not be carried
//! over exactly, so nothing is dropped silently. Importers do not change
//! the project themselves: they return an [`Imported`] whose `edit` adds
//! the new media and sequences when applied (so an import is one undo step).

pub mod conform;
pub mod edl;
pub mod fcpxml;
pub mod fcpxml_write;
pub mod otio;

use crate::edit::Edit;
use crate::id::SequenceId;
use crate::project::Project;
use std::fmt;

/// Things an import or export could not represent exactly.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    /// One entry per problem.
    pub issues: Vec<Issue>,
}

/// One interchange problem.
#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    /// What it concerns (a clip name, an element, …).
    pub item: String,
    /// What happened to it.
    pub message: String,
}

impl Report {
    /// Records a problem.
    pub fn add(&mut self, item: impl Into<String>, message: impl Into<String>) {
        self.issues.push(Issue { item: item.into(), message: message.into() });
    }

    /// Whether everything was carried over.
    pub fn is_clean(&self) -> bool {
        self.issues.is_empty()
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for i in &self.issues {
            writeln!(f, "{}: {}", i.item, i.message)?;
        }
        Ok(())
    }
}

/// Result of an import.
#[derive(Clone, Debug, PartialEq)]
pub struct Imported {
    /// Adds the imported media and sequences; apply it with
    /// [`Project::apply`] or [`crate::History::apply`].
    pub edit: Edit,
    /// The main imported sequence.
    pub sequence: SequenceId,
    /// What was not carried over exactly.
    pub report: Report,
}

/// Why an import failed outright.
#[derive(Debug)]
pub enum ImportError {
    /// The text is not valid JSON or XML.
    Parse(String),
    /// The document is valid but not something this importer understands.
    Unsupported(String),
    /// The result could not be added to the project.
    Edit(crate::EditError),
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImportError::Parse(e) => write!(f, "could not read the file: {e}"),
            ImportError::Unsupported(e) => write!(f, "unsupported document: {e}"),
            ImportError::Edit(e) => write!(f, "could not add the import: {e}"),
        }
    }
}

impl std::error::Error for ImportError {}

/// Checks that `edit` applies cleanly to `project` without changing it.
pub(crate) fn dry_run(project: &Project, edit: &Edit) -> Result<(), ImportError> {
    let mut scratch = project.clone();
    scratch.apply(edit.clone()).map(|_| ()).map_err(ImportError::Edit)
}

/// Adds `transitions` to `seq`, shortening or dropping (with a report
/// entry) those that do not fit their clips' handles. `media` and `nested`
/// are items being imported alongside `seq`, needed to know handle sizes.
pub(crate) fn fit_transitions(project: &Project, media: &[crate::MediaItem], nested: &[crate::Sequence], seq: &mut crate::Sequence, transitions: Vec<crate::Transition>, report: &mut Report) {
    let mut scratch = project.clone();
    scratch.media.extend(media.iter().cloned());
    scratch.sequences.extend(nested.iter().cloned());
    for mut t in transitions {
        seq.normalize();
        scratch.sequences.retain(|s| s.id != seq.id);
        scratch.sequences.push(seq.clone());
        let s = scratch.sequence(seq.id).expect("pushed");
        let max = scratch.transition_limit(s, t.left, t.right, t.align, None);
        if max <= 0 {
            report.add(format!("transition {}", t.kind), "not enough media for the transition; dropped");
            continue;
        }
        if t.duration > max {
            report.add(format!("transition {}", t.kind), "shortened to fit the available media");
            t.duration = max;
        }
        seq.transitions.push(t);
    }
    seq.normalize();
}

/// `file://` URL for an absolute path, percent-encoding unsafe bytes.
pub fn path_to_url(path: &str) -> String {
    let mut out = String::from("file://");
    if !path.starts_with('/') {
        out.push('/');
    }
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Path from a `file://` URL (or the text itself if it is not a URL).
pub fn url_to_path(url: &str) -> String {
    let Some(rest) = url.strip_prefix("file://") else { return url.to_string() };
    // Skip an authority such as "localhost".
    let rest = if rest.starts_with('/') { rest } else { rest.find('/').map_or(rest, |i| &rest[i..]) };
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(v) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("zz"), 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_urls() {
        let p = "/Users/me/My Clips/día 1.mp4";
        let u = path_to_url(p);
        assert_eq!(u, "file:///Users/me/My%20Clips/d%C3%ADa%201.mp4");
        assert_eq!(url_to_path(&u), p);
        assert_eq!(url_to_path("file://localhost/a/b%20c"), "/a/b c");
        assert_eq!(url_to_path("/plain/path"), "/plain/path");
    }
}
