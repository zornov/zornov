// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! "Check project": what would go wrong in the export, found without rendering, grouped by
//! severity, with a fix where Studio can mend it and "accepted as intended" for what was meant.
//! Also the checks against a delivery spec before a render, and the quality control of a
//! finished file. The agent's `check` and `qc` tools and the Export screen share this.

use crate::agent::Session;
use crate::deliver_run as run;
use crate::doc::Doc;
use serde_json::Value;
use std::path::{Path, PathBuf};
use studio_deliver::{qc, specs};
use studio_model as m;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// The export would be wrong or empty there.
    Fail,
    /// Probably not meant.
    Warning,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Fail => "Problems",
            Severity::Warning => "Worth a look",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    pub kind: String,
    pub clip: Option<u32>,
    /// Seconds into the programme.
    pub at: Option<f64>,
    pub message: String,
    pub advice: String,
    pub severity: Severity,
    /// Studio can mend it ("Fix").
    pub fixable: bool,
}

#[derive(Clone, Debug, Default)]
pub struct CheckOut {
    pub problems: Vec<Problem>,
    /// Findings the person marked as intended.
    pub accepted: Vec<Problem>,
    /// What "Fix" did, one line each.
    pub fixed: Vec<String>,
    pub took: f64,
}

impl CheckOut {
    pub fn is_clear(&self) -> bool {
        self.problems.is_empty()
    }

    pub fn count(&self, s: Severity) -> usize {
        self.problems.iter().filter(|p| p.severity == s).count()
    }
}

/// Kinds that make the export wrong, not just doubtful.
fn severity_of(kind: &str) -> Severity {
    match kind {
        "gap" | "past_media_end" | "picture_ends_early" | "title_cut_off" | "title_off_canvas" | "no_sound" | "audio_clipping" | "unknown_effect" => Severity::Fail,
        _ => Severity::Warning,
    }
}

/// Kinds the `check` fix can mend (`agent::scene::fix`).
pub fn is_fixable(kind: &str) -> bool {
    matches!(kind, "title_covers_picture" | "title_cut_off" | "title_under_app_ui" | "picture_ends_early" | "audio_quiet" | "audio_loud")
}

fn problems_of(v: &Value, list: &str) -> Vec<Problem> {
    let mut out: Vec<Problem> = v[list]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|p| {
                    let kind = p["kind"].as_str().unwrap_or("").to_owned();
                    Problem {
                        clip: p["clip"].as_u64().map(|c| c as u32),
                        at: p["at"].as_f64(),
                        message: p["problem"].as_str().unwrap_or("").to_owned(),
                        advice: p["fix"].as_str().unwrap_or("").to_owned(),
                        severity: severity_of(&kind),
                        fixable: is_fixable(&kind),
                        kind,
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by(|a, b| a.severity.cmp(&b.severity).then(a.at.unwrap_or(0.0).total_cmp(&b.at.unwrap_or(0.0))));
    out
}

/// Runs `f` on an agent session that works on `doc` itself (swapped in and out, so history and
/// the undo stack are the project's own).
pub(crate) fn with_session<R>(doc: &mut Doc, f: impl FnOnce(&mut Session) -> R) -> R {
    let mut s = Session::new(Doc::new(m::Project::default()));
    std::mem::swap(&mut s.doc, doc);
    let r = f(&mut s);
    std::mem::swap(&mut s.doc, doc);
    r
}

/// Checks the project. With `fix`, mends what can be mended as one undo step "Fix" first.
pub fn check_project(doc: &mut Doc, frames: bool, fix: bool) -> CheckOut {
    with_session(doc, |s| {
        let v = s.check_project(frames, fix);
        CheckOut {
            problems: problems_of(&v, "problems"),
            accepted: problems_of(&v, "accepted"),
            fixed: v["fixed"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()).unwrap_or_default(),
            took: v["took_seconds"].as_f64().unwrap_or(0.0),
        }
    })
}

/// Marks a finding as intended; it is listed as accepted from now on.
pub fn accept(doc: &mut Doc, kind: &str, clip: Option<u32>) -> Result<(), String> {
    with_session(doc, |s| s.accept_finding(kind, clip, ""))
}

/// Takes an acceptance back.
pub fn unaccept(doc: &mut Doc, kind: &str, clip: Option<u32>) -> Result<(), String> {
    with_session(doc, |s| s.unaccept_finding(kind, clip))
}

/// The profile of a delivery spec for this project (default checks without one).
pub fn profile_for(doc: &Doc, spec: Option<&str>) -> Result<qc::Profile, String> {
    match spec {
        Some(d) => Ok(specs::find(d).ok_or_else(|| format!("No delivery spec \"{d}\""))?.profile(doc.project.fps as f64, (doc.project.width, doc.project.height))),
        None => Ok(qc::Profile::default()),
    }
}

/// Checks that need no render, against a delivery spec: loudness of the mix, captions against
/// speech, the canvas against the spec's shape, titles at the frame edge.
pub fn preflight(doc: &mut Doc, spec: Option<&str>) -> Result<qc::Report, String> {
    let profile = profile_for(doc, spec)?;
    Ok(with_session(doc, |s| s.preflight_report(profile, true)))
}

/// Quality control of a rendered file; writes `<file>.qc.json` and `<file>.qc.txt` beside it.
pub fn check_file(doc: &Doc, path: &Path, spec: Option<&str>) -> Result<(qc::Report, PathBuf), String> {
    let profile = profile_for(doc, spec)?;
    let range = (0, doc.project.duration());
    let extra = run::qc_extra(doc, range);
    run::qc_and_write(path, &profile, &extra)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_and_fixability_follow_the_kind() {
        assert_eq!(severity_of("gap"), Severity::Fail);
        assert_eq!(severity_of("title_covers_picture"), Severity::Warning);
        assert!(is_fixable("audio_quiet") && !is_fixable("gap"));
        let v = serde_json::json!({"problems": [
            {"kind": "title_covers_picture", "clip": 3, "at": 9.0, "problem": "covers", "fix": "move"},
            {"kind": "gap", "at": 2.0, "problem": "hole", "fix": "close"}]});
        let p = problems_of(&v, "problems");
        assert_eq!(p[0].kind, "gap", "failures first");
        assert_eq!(p[1].clip, Some(3));
        assert!(p[1].fixable && !p[0].fixable);
    }
}
