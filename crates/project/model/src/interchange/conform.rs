// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Conform and relink: finding the real files for media items that are
//! missing or offline (from an EDL, an XML from another system, or a moved
//! drive), by file name, reel name and timecode.
//!
//! The caller lists candidate files with what probing them found
//! ([`Candidate`]); [`plan`] matches each item to at most one candidate:
//!
//! 1. the same file name (case-insensitive, extension optional);
//! 2. the same reel name (normalised as EDLs write them), preferring the
//!    candidate whose timecode range covers every source frame the
//!    project's clips use;
//! 3. with `by_timecode`, any candidate whose timecode range covers the
//!    clips' frames when nothing else matched (reels absent).
//!
//! [`apply`] builds one edit: each item gets the new path, timecode and
//! reel; clips imported from an EDL (which remember their source
//! timecode) have their source positions recomputed against the real
//! file's timecode, so the same frames are used.

use super::Report;
use super::edl::reel_name;
use crate::clip::ClipSource;
use crate::edit::Edit;
use crate::id::MediaId;
use crate::media::MediaItem;
use crate::project::Project;
use crate::time::{FrameRate, Micros};
use crate::timecode::Timecode;

/// A file that could stand in for a media item.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Candidate {
    /// The file.
    pub path: String,
    /// Its reel / camera roll name, when the file carries one.
    pub reel: Option<String>,
    /// First-frame timecode label.
    pub timecode: Option<String>,
    /// Length of the file.
    pub duration: Option<Micros>,
    /// Frame rate of its timecode (default: the project's).
    pub frame_rate: Option<FrameRate>,
}

impl Candidate {
    fn file_name(&self) -> &str {
        self.path.rsplit(['/', '\\']).next().unwrap_or(&self.path)
    }

    /// Timecode range in frames at `rate`, when known.
    fn tc_range(&self, project_rate: FrameRate) -> Option<(i64, i64)> {
        let rate = self.frame_rate.unwrap_or(project_rate);
        let tc = self.timecode.as_deref()?;
        let start = Timecode::new(rate, tc.contains(';')).parse_lenient(tc)?;
        let len = rate.micros_to_frame_round(self.duration?);
        Some((start, start + len))
    }
}

/// One relink decision.
#[derive(Clone, Debug, PartialEq)]
pub struct Relink {
    /// The media item.
    pub media: MediaId,
    /// Its new file.
    pub path: String,
    /// How it matched: `"name"`, `"reel"`, `"reel+timecode"`, `"timecode"`.
    pub how: &'static str,
    /// The file's first-frame timecode, when known.
    pub timecode: Option<String>,
    /// The reel name to keep.
    pub reel: Option<String>,
    /// The file's length, when known.
    pub duration: Option<Micros>,
}

fn stem(s: &str) -> String {
    let n = s.rsplit(['/', '\\']).next().unwrap_or(s);
    n.rsplit_once('.').map_or(n, |x| x.0).to_ascii_lowercase()
}

/// Whether a media item still needs a file: offline (from an EDL) or its
/// path does not exist (`exists` is how the caller checks).
pub fn needs_file(m: &MediaItem, exists: &dyn Fn(&str) -> bool) -> bool {
    m.path.starts_with("offline:") || !exists(&m.path)
}

/// The source timecode frames (at the media's rate) the project's clips
/// use of media `m`: (first, last).
fn used_range(p: &Project, m: &MediaItem) -> Option<(i64, i64)> {
    let rate = m.timecode_rate(p.settings.frame_rate);
    let origin = rate.micros_to_frame_round(m.timecode_start(p.settings.frame_rate));
    let mut lo = i64::MAX;
    let mut hi = i64::MIN;
    for s in &p.sequences {
        for c in &s.clips {
            if let ClipSource::Media(r) = &c.source
                && r.media == m.id
            {
                lo = lo.min(origin + rate.micros_to_frame_round(c.source_in));
                hi = hi.max(origin + rate.micros_to_frame_round(c.source_out()));
            }
        }
    }
    (lo <= hi).then_some((lo, hi))
}

/// Matches the given media items to candidates.
pub fn plan(p: &Project, items: &[MediaId], candidates: &[Candidate], by_timecode: bool) -> (Vec<Relink>, Report) {
    let mut out = Vec::new();
    let mut report = Report::default();
    for id in items {
        let Some(m) = p.media_item(*id) else { continue };
        let used = used_range(p, m);
        let covers = |c: &Candidate| match (used, c.tc_range(p.settings.frame_rate)) {
            (Some((a, b)), Some((s, e))) => a >= s && b <= e,
            _ => false,
        };
        let name_hit = candidates.iter().find(|c| {
            let f = c.file_name().to_ascii_lowercase();
            f == m.name.to_ascii_lowercase() || stem(&f) == stem(&m.name) || (!m.path.starts_with("offline:") && stem(&f) == stem(&m.path))
        });
        let reel = m.reel.as_deref().map(reel_name);
        let reel_hits: Vec<&Candidate> = match &reel {
            Some(r) => candidates.iter().filter(|c| c.reel.as_deref().map(reel_name).as_deref() == Some(r.as_str()) || reel_name(&stem(c.file_name())) == *r).collect(),
            None => Vec::new(),
        };
        let pick = if let Some(c) = name_hit {
            Some((c, "name"))
        } else if let Some(c) = reel_hits.iter().find(|c| covers(c)) {
            Some((*c, "reel+timecode"))
        } else if reel_hits.len() == 1 {
            Some((reel_hits[0], "reel"))
        } else if !reel_hits.is_empty() {
            report.add(&m.name, format!("{} files have reel {}; none covers the timecode the edit uses", reel_hits.len(), reel.clone().unwrap_or_default()));
            None
        } else if by_timecode {
            let hits: Vec<&Candidate> = candidates.iter().filter(|c| covers(c)).collect();
            match hits.len() {
                1 => Some((hits[0], "timecode")),
                0 => None,
                n => {
                    report.add(&m.name, format!("{n} files cover the timecode the edit uses; give reels to choose"));
                    None
                }
            }
        } else {
            None
        };
        match pick {
            Some((c, how)) => out.push(Relink { media: m.id, path: c.path.clone(), how, timecode: c.timecode.clone(), reel: c.reel.clone().or_else(|| m.reel.clone()), duration: c.duration }),
            None => report.add(&m.name, "no matching file found"),
        }
    }
    (out, report)
}

/// The edit that relinks: new paths, timecodes and reels, and source
/// positions re-derived for clips that remember their EDL source timecode.
pub fn apply(p: &Project, relinks: &[Relink]) -> Edit {
    let mut edits = Vec::new();
    for r in relinks {
        let Some(old) = p.media_item(r.media) else { continue };
        let mut item = old.clone();
        item.path = r.path.clone();
        item.relative_path = None;
        if r.timecode.is_some() {
            item.timecode = r.timecode.clone();
        }
        if r.reel.is_some() {
            item.reel = r.reel.clone();
        }
        if let Some(d) = r.duration {
            item.info.duration = Some(d);
        }
        if old.path.starts_with("offline:") && item.name == old.reel.clone().unwrap_or_default() {
            item.name = r.path.rsplit(['/', '\\']).next().unwrap_or(&r.path).to_string();
        }
        let rate = item.timecode_rate(p.settings.frame_rate);
        let new_origin = item.timecode_start(p.settings.frame_rate);
        edits.push(Edit::SetMedia { item: Box::new(item) });
        // Clips from an EDL keep their source timecode: re-derive.
        if old.timecode == r.timecode || r.timecode.is_none() {
            continue;
        }
        for s in &p.sequences {
            for c in &s.clips {
                let ClipSource::Media(m) = &c.source else { continue };
                if m.media != r.media {
                    continue;
                }
                let Some(tc) = c.meta.get("edl_event").and_then(|v| v.get("src_in")).and_then(|v| v.as_str()) else { continue };
                let Some(f) = Timecode::new(rate, tc.contains(';')).parse_lenient(tc) else { continue };
                let mut nc = c.clone();
                nc.source_in = (rate.frame_to_micros(f) - new_origin).max(0);
                // Dissolves moved the cut by half their length (see edl).
                if let Some(t) = s.transitions.iter().find(|t| t.right == c.id) {
                    nc.source_in += (t.split().0 as f64 * c.speed.rate_at(0)).round() as Micros;
                }
                if nc.source_in != c.source_in {
                    edits.push(Edit::SetClip { seq: s.id, clip: Box::new(nc) });
                }
            }
        }
    }
    Edit::Batch { edits }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_ranges() {
        let c = Candidate { path: "/x/A001C003.mov".into(), reel: Some("A001C003".into()), timecode: Some("01:00:10:00".into()), duration: Some(10_000_000), frame_rate: Some(FrameRate::FPS_25) };
        assert_eq!(c.tc_range(FrameRate::FPS_25), Some((90_250, 90_500)));
        assert_eq!(c.file_name(), "A001C003.mov");
        assert_eq!(stem("/a/B.MOV"), "b");
    }
}
