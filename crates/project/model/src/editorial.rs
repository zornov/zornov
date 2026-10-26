// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Film editing the way editors work with source and record, targeted
//! tracks and sync locks, built on the model's [`Edit`]s.
//!
//! * **Source to record**: three- and four-point edits from a source (a
//!   media item or another sequence) into the timeline as insert,
//!   overwrite, replace or fit-to-fill, with picture and sound patched to
//!   chosen lanes ([`source_edit`]).
//! * **Trimming**: ripple trims that keep sync-locked lanes in step
//!   ([`ripple_trim`]), several sides at once for asymmetric trims
//!   ([`trim_sides`]), extend edit ([`extend`]), split edits / J and L cuts
//!   ([`split_edit`]).
//! * **Ranges**: lift and extract between marks across targeted lanes
//!   ([`lift`], [`extract`]).
//! * **Lanes**: sync locks and targeting, kept in a lane's `extra`
//!   ([`sync_locked`], [`targeted`]).
//! * **Looking things up**: match frame and reverse match frame, gaps,
//!   duplicate frames, a timeline index.
//! * **Organising**: sequences (new, duplicate), string-outs and selects
//!   reels, scene and take metadata, speed ramps, and cutting a clip by
//!   the source ranges of words taken out of its transcript
//!   ([`remove_source_ranges`]).
//!
//! Every operation is worked out on a copy of the project ([`Draft`]) and
//! comes back as one forward [`Edit`] made of primitives, so the caller
//! applies it through its history as a single undo step. Ids the plan
//! hands out are reserved in the real project straight away.

use crate::clip::{Clip, ClipSource, MediaRef, SequenceRef, Speed};
use crate::edit::{Edge, Edit, EditError};
use crate::id::*;
use crate::keyframe::{Interp, Keyframe, Track};
use crate::media::MediaItem;
use crate::project::Project;
use crate::sequence::{Lane, LaneKind, Marker, Sequence};
use crate::time::{Micros, SECOND, TimeRange};
use crate::value::{Rgba, Value};
use std::collections::{BTreeMap, HashMap, HashSet};

type Result<T> = std::result::Result<T, EditError>;

fn invalid<T>(why: impl Into<String>) -> Result<T> {
    Err(EditError::Invalid(why.into()))
}

fn nf(what: impl Into<String>) -> EditError {
    EditError::NotFound(what.into())
}

// ------------------------------------------------------------------ drafts

/// A change worked out on a copy of the project. Apply edits to it one
/// after another (each sees the result of the last), then [`Draft::finish`]
/// turns everything into one forward edit for the real project.
pub struct Draft {
    p: Project,
    undo: Vec<Edit>,
}

impl Draft {
    /// A copy of `p` to plan on.
    pub fn new(p: &Project) -> Self {
        Self { p: p.clone(), undo: Vec::new() }
    }

    /// The project as planned so far.
    pub fn project(&self) -> &Project {
        &self.p
    }

    /// Applies one edit to the copy.
    pub fn apply(&mut self, e: Edit) -> Result<()> {
        let inv = self.p.apply(e)?;
        self.undo.push(inv);
        Ok(())
    }

    /// A fresh id (reserved in the real project by [`Draft::finish`]).
    pub fn alloc_id(&mut self) -> u64 {
        self.p.alloc_id()
    }

    /// The whole plan as one edit for `original` (the project the draft was
    /// copied from), whose id counter moves past every id the plan used.
    pub fn finish(mut self, original: &mut Project) -> Result<Edit> {
        original.next_id = original.next_id.max(self.p.next_id);
        if self.undo.is_empty() {
            return Ok(Edit::Batch { edits: Vec::new() });
        }
        self.undo.reverse();
        // Undoing the copy hands back the redo: primitives that rebuild the
        // planned state exactly.
        self.p.apply(Edit::Batch { edits: std::mem::take(&mut self.undo) })
    }

    fn seq(&self, id: SequenceId) -> Result<&Sequence> {
        self.p.sequence(id).ok_or_else(|| nf(format!("sequence {}", id.0)))
    }
}

// ------------------------------------------------------------------- lanes

/// Lane flag: shifts with ripple edits made on other lanes.
pub const SYNC_LOCK: &str = "sync_lock";
/// Lane flag: left out of range edits (lift, extract) when set.
pub const UNTARGETED: &str = "untargeted";

/// Whether the lane follows ripples made elsewhere (a sync lock).
pub fn sync_locked(l: &Lane) -> bool {
    l.extra.get(SYNC_LOCK).and_then(|v| v.as_bool()).unwrap_or(false)
}

/// Whether range edits (lift, extract) and source edits target the lane.
pub fn targeted(l: &Lane) -> bool {
    !l.extra.get(UNTARGETED).and_then(|v| v.as_bool()).unwrap_or(false)
}

/// Turns a lane's sync lock on or off.
pub fn set_sync_lock(p: &Project, seq: SequenceId, lane: LaneId, on: bool) -> Result<Edit> {
    lane_flag(p, seq, lane, SYNC_LOCK, on)
}

/// Targets or untargets a lane for range and source edits.
pub fn set_targeted(p: &Project, seq: SequenceId, lane: LaneId, on: bool) -> Result<Edit> {
    lane_flag(p, seq, lane, UNTARGETED, !on)
}

fn lane_flag(p: &Project, seq: SequenceId, lane: LaneId, key: &str, on: bool) -> Result<Edit> {
    let s = p.sequence(seq).ok_or_else(|| nf(format!("sequence {}", seq.0)))?;
    let mut l = s.lane(lane).ok_or_else(|| nf(format!("lane {}", lane.0)))?.clone();
    if on {
        l.extra.insert(key.into(), true.into());
    } else {
        l.extra.remove(key);
    }
    Ok(Edit::SetLane { seq, lane: l })
}

fn sync_lanes(s: &Sequence, except: &[LaneId]) -> Vec<LaneId> {
    s.lanes.iter().filter(|l| sync_locked(l) && !l.locked && !except.contains(&l.id)).map(|l| l.id).collect()
}

// ---------------------------------------------------------- span helpers

/// Clips on `lanes` that straddle `t` (gaps too), minus `exclude`.
fn straddlers(s: &Sequence, lanes: &[LaneId], t: Micros, exclude: &HashSet<ClipId>) -> Vec<ClipId> {
    s.clips.iter().filter(|c| lanes.contains(&c.lane) && c.start < t && c.end() > t && !exclude.contains(&c.id)).map(|c| c.id).collect()
}

fn split_lanes(d: &mut Draft, seq: SequenceId, lanes: &[LaneId], t: Micros, exclude: &HashSet<ClipId>) -> Result<()> {
    let hit = straddlers(d.seq(seq)?, lanes, t, exclude);
    if hit.is_empty() { Ok(()) } else { d.apply(Edit::Split { seq, at: t, clips: Some(hit) }) }
}

/// Clips linked to `ids` that are not in `ids`.
fn partners_of(s: &Sequence, ids: &HashSet<ClipId>) -> Vec<ClipId> {
    let links: HashSet<LinkId> = s.clips.iter().filter(|c| ids.contains(&c.id)).filter_map(|c| c.link).collect();
    s.clips.iter().filter(|c| c.link.is_some_and(|l| links.contains(&l)) && !ids.contains(&c.id)).map(|c| c.id).collect()
}

/// Opens `len` of empty time at `at` on `lanes`: clips there split and
/// everything from `at` on moves later (with linked partners), the spine
/// gets a gap. `exclude` clips stay put (already moved by another edit).
fn open_span(d: &mut Draft, seq: SequenceId, lanes: &[LaneId], at: Micros, len: Micros, exclude: &HashSet<ClipId>) -> Result<()> {
    if len <= 0 || lanes.is_empty() {
        return Ok(());
    }
    split_lanes(d, seq, lanes, at, exclude)?;
    let s = d.seq(seq)?;
    let spine = s.spine_id();
    let mut moved: HashSet<ClipId> = s.clips.iter().filter(|c| lanes.contains(&c.lane) && c.start >= at && !exclude.contains(&c.id)).map(|c| c.id).collect();
    for p in partners_of(s, &moved) {
        if !exclude.contains(&p) {
            moved.insert(p);
        }
    }
    let spine_moves = s.clips.iter().any(|c| c.lane == spine && moved.contains(&c.id));
    let mut edits: Vec<Edit> = s
        .clips
        .iter()
        .filter(|c| moved.contains(&c.id))
        .map(|c| {
            let mut n = c.clone();
            n.start += len;
            Edit::SetClip { seq, clip: Box::new(n) }
        })
        .collect();
    if spine_moves {
        let id = ClipId(d.alloc_id());
        edits.push(Edit::InsertClip { seq, clip: Box::new(Clip::gap(id, spine, at, len)) });
    }
    d.apply(Edit::Batch { edits })
}

/// Takes `[a, b)` out of `lanes`. With `close`, later clips (and their
/// partners) move left to close it; without, the space stays empty (a gap
/// on the spine).
fn remove_span(d: &mut Draft, seq: SequenceId, lanes: &[LaneId], a: Micros, b: Micros, close: bool, exclude: &HashSet<ClipId>) -> Result<()> {
    if b <= a || lanes.is_empty() {
        return Ok(());
    }
    split_lanes(d, seq, lanes, a, exclude)?;
    split_lanes(d, seq, lanes, b, exclude)?;
    let s = d.seq(seq)?;
    let spine = s.spine_id();
    let inside: HashSet<ClipId> = s.clips.iter().filter(|c| lanes.contains(&c.lane) && c.start >= a && c.end() <= b && !exclude.contains(&c.id)).map(|c| c.id).collect();
    let mut edits = Vec::new();
    for t in s.transitions.iter().filter(|t| inside.contains(&t.left) || inside.contains(&t.right)) {
        edits.push(Edit::RemoveTransition { seq, id: t.id });
    }
    for id in &inside {
        edits.push(Edit::RemoveClip { seq, clip: *id });
    }
    let len = b - a;
    if close {
        let mut later: HashSet<ClipId> = s.clips.iter().filter(|c| lanes.contains(&c.lane) && c.start >= b && !exclude.contains(&c.id)).map(|c| c.id).collect();
        for p in partners_of(s, &later) {
            if !exclude.contains(&p) && !inside.contains(&p) {
                later.insert(p);
            }
        }
        for c in s.clips.iter().filter(|c| later.contains(&c.id)) {
            let mut n = c.clone();
            n.start -= len;
            if n.start < 0 {
                return invalid("clips would move before the sequence start");
            }
            edits.push(Edit::SetClip { seq, clip: Box::new(n) });
        }
    } else if lanes.contains(&spine) && s.clips.iter().any(|c| c.lane == spine && inside.contains(&c.id)) {
        let id = ClipId(d.alloc_id());
        edits.push(Edit::InsertClip { seq, clip: Box::new(Clip::gap(id, spine, a, len)) });
    }
    d.apply(Edit::Batch { edits })?;
    tidy_spine(d, seq)
}

/// Merges neighbouring spine gaps, drops a trailing gap and transitions
/// whose clips no longer meet on the spine (what the model's own
/// operations do after every edit).
fn tidy_spine(d: &mut Draft, seq: SequenceId) -> Result<()> {
    loop {
        let s = d.seq(seq)?;
        let spine = s.spine_clips();
        let mut edits = Vec::new();
        if let Some(w) = spine.windows(2).find(|w| w[0].is_gap() && w[1].is_gap()) {
            let mut a = w[0].clone();
            a.duration += w[1].duration;
            edits.push(Edit::RemoveClip { seq, clip: w[1].id });
            edits.push(Edit::SetClip { seq, clip: Box::new(a) });
        } else if let Some(last) = spine.last().filter(|c| c.is_gap()) {
            for t in s.transitions_of(last.id) {
                edits.push(Edit::RemoveTransition { seq, id: t.id });
            }
            edits.push(Edit::RemoveClip { seq, clip: last.id });
        }
        if edits.is_empty() {
            break;
        }
        d.apply(Edit::Batch { edits })?;
    }
    let s = d.seq(seq)?;
    let spine = s.spine_id();
    let broken: Vec<TransitionId> = s
        .transitions
        .iter()
        .filter(|t| match (s.clip(t.left), s.clip(t.right)) {
            (Some(l), Some(r)) => !(l.lane == spine && r.lane == spine && l.end() == r.start) || d.p.transition_limit(s, t.left, t.right, t.align, Some(t.id)) < t.duration,
            _ => true,
        })
        .map(|t| t.id)
        .collect();
    if !broken.is_empty() {
        d.apply(Edit::Batch { edits: broken.into_iter().map(|id| Edit::RemoveTransition { seq, id }).collect() })?;
    }
    Ok(())
}

// ------------------------------------------------------- source to record

/// What a source edit takes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// A media item from the bin.
    Media(MediaId),
    /// Another sequence, placed as a nested (compound) clip.
    Sequence(SequenceId),
}

/// How a source edit enters the timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceMode {
    /// Splice in: later material on the patched and sync-locked lanes moves
    /// right.
    Insert,
    /// Record over whatever is there; nothing moves.
    Overwrite,
    /// Swap the source into the clip under the record mark, keeping its
    /// length, place and settings; the source mark lines up with the record
    /// mark.
    Replace,
    /// Overwrite the record range exactly, changing the speed so the
    /// source range fits.
    FitToFill,
}

/// A three- or four-point edit. Any three of the four marks decide the
/// fourth: with both source marks and one record mark the source length
/// is used (the record out backtimes), with both record marks and one
/// source mark the record length is used. With all four, fit-to-fill
/// changes the speed; the other modes use the record length.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceEdit {
    /// What to take from.
    pub source: Source,
    /// Source in mark (source time).
    pub src_in: Option<Micros>,
    /// Source out mark (source time).
    pub src_out: Option<Micros>,
    /// Record in mark (sequence time).
    pub rec_in: Option<Micros>,
    /// Record out mark (sequence time).
    pub rec_out: Option<Micros>,
    /// Insert, overwrite, replace or fit-to-fill.
    pub mode: SourceMode,
    /// Lane for the picture (`None`: no picture).
    pub video_lane: Option<LaneId>,
    /// Lane for the sound as its own linked clip (`None`: the sound stays
    /// inside the picture clip, or is left out when there is no picture).
    pub audio_lane: Option<LaneId>,
    /// Take the sound at all.
    pub audio: bool,
}

/// What a source edit did.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceEditDone {
    /// The record range filled.
    pub rec: TimeRange,
    /// The source range used.
    pub src: TimeRange,
    /// Clips made (picture first) or, for replace, the clips changed.
    pub clips: Vec<ClipId>,
    /// Speed used (fit-to-fill), 1 otherwise.
    pub speed: f64,
}

/// Length of a source, `None` when it can be held for any length (stills).
pub fn source_length(p: &Project, src: Source) -> Result<Option<Micros>> {
    match src {
        Source::Media(m) => Ok(p.media_item(m).ok_or_else(|| nf(format!("media {}", m.0)))?.duration()),
        Source::Sequence(s) => Ok(Some(p.sequence(s).ok_or_else(|| nf(format!("sequence {}", s.0)))?.duration())),
    }
}

/// Works out the four marks of a source edit: `(src_in, rec_in, length,
/// speed)`. `playhead` stands in for a missing record mark.
pub fn resolve_marks(e: &SourceEdit, src_len: Option<Micros>, playhead: Micros) -> Result<(Micros, Micros, Micros, f64)> {
    const STILL: Micros = 5 * SECOND;
    let room = |src_in: Micros| src_len.map(|l| l - src_in);
    let (si, so, ri, ro) = (e.src_in, e.src_out, e.rec_in, e.rec_out);
    let r = match (si, so, ri, ro) {
        (Some(si), Some(so), _, _) if so <= si => return invalid("the source out mark must come after the source in mark"),
        (_, _, Some(ri), Some(ro)) if ro <= ri => return invalid("the record out mark must come after the record in mark"),
        (Some(si), Some(so), Some(ri), Some(ro)) => {
            if e.mode == SourceMode::FitToFill {
                (si, ri, ro - ri, (so - si) as f64 / (ro - ri) as f64)
            } else {
                (si, ri, ro - ri, 1.0)
            }
        }
        (Some(si), Some(so), Some(ri), None) => (si, ri, so - si, 1.0),
        (Some(si), Some(so), None, Some(ro)) => (si, ro - (so - si), so - si, 1.0),
        (Some(si), Some(so), None, None) => (si, playhead, so - si, 1.0),
        (si, None, Some(ri), Some(ro)) => (si.unwrap_or(0), ri, ro - ri, 1.0),
        (None, Some(so), Some(ri), Some(ro)) => (so - (ro - ri), ri, ro - ri, 1.0),
        (si, None, ri, ro) => {
            let si = si.unwrap_or(0);
            let len = room(si).unwrap_or(STILL);
            match (ri, ro) {
                (Some(ri), None) => (si, ri, len, 1.0),
                (None, Some(ro)) => (si, ro - len, len, 1.0),
                _ => (si, playhead, len, 1.0),
            }
        }
        (None, Some(so), ri, ro) => {
            // Source out only: the clip runs from the source start to it.
            let len = so;
            match (ri, ro) {
                (Some(ri), None) => (0, ri, len, 1.0),
                (None, Some(ro)) => (0, ro - len, len, 1.0),
                _ => (0, playhead, len, 1.0),
            }
        }
    };
    let (si, ri, len, speed) = r;
    if si < 0 {
        return Err(EditError::NoHandles("the source marks reach before the start of the source".into()));
    }
    if ri < 0 {
        return invalid("the edit would start before the sequence start (backtimed from the record out)");
    }
    if len <= 0 {
        return invalid("the marks give an empty edit");
    }
    if let Some(l) = src_len
        && si as f64 + len as f64 * speed > l as f64 + 1.0
    {
        return Err(EditError::NoHandles(format!("the source has {:.2}s after the in mark; the edit needs {:.2}s", (l - si) as f64 / 1e6, len as f64 * speed / 1e6)));
    }
    Ok((si, ri, len, speed))
}

/// A three- or four-point edit from a source into `seq`.
pub fn source_edit(p: &mut Project, seq: SequenceId, e: &SourceEdit, playhead: Micros) -> Result<(Edit, SourceEditDone)> {
    let src_len = source_length(p, e.source)?;
    let (si, ri, len, speed) = resolve_marks(e, src_len, playhead)?;
    let (has_video, has_audio) = match e.source {
        Source::Media(m) => {
            let item = p.media_item(m).expect("checked by source_length");
            (item.has_video(), item.has_audio())
        }
        Source::Sequence(_) => (true, true),
    };
    if let Source::Sequence(s) = e.source
        && s == seq
    {
        return invalid("a sequence cannot be edited into itself");
    }
    let mut d = Draft::new(p);
    let s = d.seq(seq)?;
    for l in [e.video_lane, e.audio_lane].into_iter().flatten() {
        let lane = s.lane(l).ok_or_else(|| nf(format!("lane {}", l.0)))?;
        if lane.locked {
            return Err(EditError::Locked(l));
        }
    }
    if let Some(l) = e.audio_lane.and_then(|l| s.lane(l))
        && l.kind != LaneKind::Audio
    {
        return invalid(format!("lane \"{}\" is not an audio lane", l.name));
    }
    let want_audio = e.audio && has_audio;
    let video_lane = e.video_lane.filter(|_| has_video || e.audio_lane.is_none());
    let audio_lane = e.audio_lane.filter(|_| want_audio);
    if video_lane.is_none() && audio_lane.is_none() {
        return invalid("nothing is patched: give a lane for the picture or the sound");
    }

    if e.mode == SourceMode::Replace {
        let lane = video_lane.or(audio_lane).expect("one is patched");
        let target = s.clip_at(lane, ri).filter(|c| !c.is_gap()).cloned().ok_or_else(|| EditError::Invalid("replace needs a clip under the record mark".into()))?;
        let mut ids = vec![target.id];
        ids.extend(partners_of(s, &HashSet::from([target.id])));
        let mut edits = Vec::new();
        for id in &ids {
            let mut c = s.clip(*id).expect("listed").clone();
            if !c.has_source_time() {
                continue;
            }
            // The source mark lands on the record mark.
            let off = target.source_offset(ri - target.start).round() as Micros;
            c.source_in = si - off;
            if c.source_in < 0 {
                return Err(EditError::NoHandles("not enough source before the mark to fill the clip".into()));
            }
            c.source = match (e.source, &c.source) {
                (Source::Media(m), ClipSource::Media(r)) => ClipSource::Media(MediaRef { media: m, ..r.clone() }),
                (Source::Media(m), _) => ClipSource::Media(MediaRef::new(m)),
                (Source::Sequence(q), _) => ClipSource::Sequence(SequenceRef { sequence: q, extra: Default::default() }),
            };
            if let Some(l) = src_len
                && c.source_out() > l
            {
                return Err(EditError::NoHandles("not enough source after the mark to fill the clip".into()));
            }
            edits.push(Edit::SetClip { seq, clip: Box::new(c) });
        }
        d.apply(Edit::Batch { edits })?;
        let done = SourceEditDone { rec: target.range(), src: TimeRange::new(si - target.source_offset(ri - target.start).round() as Micros, target.source_len()), clips: ids, speed: 1.0 };
        return Ok((d.finish(p)?, done));
    }

    let make = |d: &mut Draft, lane: LaneId, video: bool, audio: bool| {
        let id = ClipId(d.alloc_id());
        let source = match e.source {
            Source::Media(m) => ClipSource::Media(MediaRef { media: m, video, audio, extra: Default::default() }),
            Source::Sequence(q) => ClipSource::Sequence(SequenceRef { sequence: q, extra: Default::default() }),
        };
        let mut c = Clip::new(id, lane, ri, len, source);
        c.source_in = si;
        if speed != 1.0 {
            c.speed = Speed::Constant(speed);
        }
        c.name = match e.source {
            Source::Media(m) => d.p.media_item(m).map(|i| i.name.clone()).unwrap_or_default(),
            Source::Sequence(q) => d.p.sequence(q).map(|s| s.name.clone()).unwrap_or_default(),
        };
        if !audio {
            c.audio.muted = matches!(e.source, Source::Sequence(_));
        }
        c
    };
    let mut clips = Vec::new();
    if let Some(l) = video_lane {
        clips.push(make(&mut d, l, has_video, want_audio && audio_lane.is_none()));
    }
    if let Some(l) = audio_lane {
        clips.push(make(&mut d, l, false, true));
    }
    if clips.len() == 2 {
        let link = LinkId(d.alloc_id());
        for c in &mut clips {
            c.link = Some(link);
        }
    }
    let ids: Vec<ClipId> = clips.iter().map(|c| c.id).collect();
    if e.mode == SourceMode::Insert {
        let s = d.seq(seq)?;
        let mut lanes: Vec<LaneId> = clips.iter().map(|c| c.lane).collect();
        lanes.extend(sync_lanes(s, &lanes));
        open_span(&mut d, seq, &lanes, ri, len, &HashSet::new())?;
    }
    for c in clips {
        let lane = c.lane;
        d.apply(Edit::Overwrite { seq, lane: Some(lane), at: ri, clip: Box::new(c) })?;
    }
    let done = SourceEditDone { rec: TimeRange::new(ri, len), src: TimeRange::new(si, (len as f64 * speed).round() as Micros), clips: ids, speed };
    Ok((d.finish(p)?, done))
}

// ------------------------------------------------------------ range edits

fn range_lanes(s: &Sequence, lanes: Option<&[LaneId]>) -> Vec<LaneId> {
    match lanes {
        Some(l) => l.iter().copied().filter(|id| s.lane(*id).is_some_and(|l| !l.locked)).collect(),
        None => s.lanes.iter().filter(|l| targeted(l) && !l.locked).map(|l| l.id).collect(),
    }
}

/// Lift: takes `[a, b)` off the lanes (targeted lanes when `None`), leaving
/// the space empty.
pub fn lift(p: &mut Project, seq: SequenceId, a: Micros, b: Micros, lanes: Option<&[LaneId]>) -> Result<Edit> {
    let mut d = Draft::new(p);
    let l = range_lanes(d.seq(seq)?, lanes);
    if l.is_empty() {
        return invalid("no unlocked lane is targeted");
    }
    remove_span(&mut d, seq, &l, a, b, false, &HashSet::new())?;
    d.finish(p)
}

/// Extract: takes `[a, b)` out of the lanes (targeted lanes when `None`)
/// and closes the space; sync-locked lanes lose the same span so they stay
/// in sync.
pub fn extract(p: &mut Project, seq: SequenceId, a: Micros, b: Micros, lanes: Option<&[LaneId]>) -> Result<Edit> {
    let mut d = Draft::new(p);
    let s = d.seq(seq)?;
    let mut l = range_lanes(s, lanes);
    if l.is_empty() {
        return invalid("no unlocked lane is targeted");
    }
    l.extend(sync_lanes(s, &l));
    remove_span(&mut d, seq, &l, a, b, true, &HashSet::new())?;
    d.finish(p)
}

// ---------------------------------------------------------------- trimming

fn starts(s: &Sequence) -> HashMap<ClipId, Micros> {
    s.clips.iter().map(|c| (c.id, c.start)).collect()
}

/// Clips whose start differs between two states (moved by an edit), and
/// clips that did not exist before.
fn moved(before: &HashMap<ClipId, Micros>, s: &Sequence) -> HashSet<ClipId> {
    s.clips.iter().filter(|c| before.get(&c.id) != Some(&c.start)).map(|c| c.id).collect()
}

/// Ripple trim of one edge by `delta` (positive is later). Later clips on
/// the lane follow, and sync-locked lanes gain or lose the same time at
/// the edit point so everything stays in sync.
pub fn ripple_trim(p: &mut Project, seq: SequenceId, clip: ClipId, edge: Edge, delta: Micros) -> Result<Edit> {
    let mut d = Draft::new(p);
    ripple_trim_in(&mut d, seq, clip, edge, delta, true)?;
    d.finish(p)
}

fn ripple_trim_in(d: &mut Draft, seq: SequenceId, clip: ClipId, edge: Edge, delta: Micros, sync: bool) -> Result<()> {
    let s = d.seq(seq)?;
    let c = s.clip(clip).ok_or_else(|| nf(format!("clip {}", clip.0)))?.clone();
    let before = starts(s);
    d.apply(Edit::Trim { seq, clip, edge, delta, ripple: true })?;
    if !sync || delta == 0 {
        return Ok(());
    }
    let s = d.seq(seq)?;
    let lanes = sync_lanes(s, &[c.lane]);
    if lanes.is_empty() {
        return Ok(());
    }
    let mut skip = moved(&before, s);
    skip.insert(clip);
    // Partners trimmed along with the clip stay as the trim left them.
    skip.extend(partners_of(s, &HashSet::from([clip])));
    // Where later material moved, and by how much.
    let (at, shift) = match edge {
        Edge::End => (c.end(), delta),
        Edge::Start => (c.start, -delta),
    };
    if shift > 0 {
        open_span(d, seq, &lanes, at, shift, &skip)
    } else {
        // Shortened: the frames that went are taken out of the sync lanes
        // too (before an out point, after an in point).
        let (a, b) = match edge {
            Edge::End => (at + shift, at),
            Edge::Start => (at, at - shift),
        };
        remove_span(d, seq, &lanes, a, b, true, &skip)
    }
}

/// How one side of a multi-side trim moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrimKind {
    /// The edge moves and later clips on the lane follow.
    Ripple,
    /// The cut between the clip and its neighbour moves; both change.
    Roll,
    /// The edge moves; neighbours stay (space opens or is used up).
    Plain,
}

/// One edge in a multi-side trim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrimSide {
    /// The clip whose edge moves.
    pub clip: ClipId,
    /// Which edge.
    pub edge: Edge,
    /// Ripple, roll or plain.
    pub kind: TrimKind,
}

/// Trims several edges by the same `delta` as one edit: an asymmetric
/// trim (the outgoing side on one lane and the incoming side on another),
/// a ripple on several lanes at once, or rolls mixed with ripples. Sides
/// are trimmed in order; each lane ripples on its own (sync locks are
/// left to the sides given).
pub fn trim_sides(p: &mut Project, seq: SequenceId, sides: &[TrimSide], delta: Micros) -> Result<Edit> {
    if sides.is_empty() {
        return invalid("no sides to trim");
    }
    let mut d = Draft::new(p);
    let mut done: HashSet<(ClipId, bool)> = HashSet::new();
    for side in sides {
        // The partner of an earlier side was trimmed with it.
        if !done.insert((side.clip, side.edge == Edge::End)) {
            continue;
        }
        match side.kind {
            TrimKind::Ripple => ripple_trim_in(&mut d, seq, side.clip, side.edge, delta, sides.len() == 1)?,
            TrimKind::Plain => d.apply(Edit::Trim { seq, clip: side.clip, edge: side.edge, delta, ripple: false })?,
            TrimKind::Roll => {
                let left = match side.edge {
                    Edge::End => side.clip,
                    Edge::Start => before_cut(d.seq(seq)?, side.clip)?,
                };
                d.apply(Edit::Roll { seq, left, delta })?;
            }
        }
        let s = d.seq(seq)?;
        for p in partners_of(s, &HashSet::from([side.clip])) {
            done.insert((p, side.edge == Edge::End));
        }
    }
    d.finish(p)
}

/// The clip ending where `clip` starts, on its lane.
fn before_cut(s: &Sequence, clip: ClipId) -> Result<ClipId> {
    let c = s.clip(clip).ok_or_else(|| nf(format!("clip {}", clip.0)))?;
    s.clips.iter().find(|o| o.lane == c.lane && o.end() == c.start && o.id != c.id).map(|o| o.id).ok_or_else(|| EditError::Invalid("no clip before that cut".into()))
}

/// Extend edit: moves the cut at a clip's edge to `to` (usually the
/// playhead), rolling it so both sides change and nothing else moves. A
/// clip with nothing against that edge just trims there.
pub fn extend(p: &mut Project, seq: SequenceId, clip: ClipId, edge: Edge, to: Micros) -> Result<Edit> {
    let mut d = Draft::new(p);
    let s = d.seq(seq)?;
    let c = s.clip(clip).ok_or_else(|| nf(format!("clip {}", clip.0)))?.clone();
    match edge {
        Edge::End => {
            let delta = to - c.end();
            if s.clips.iter().any(|o| o.lane == c.lane && o.start == c.end() && o.id != c.id) {
                d.apply(Edit::Roll { seq, left: clip, delta })?;
            } else {
                d.apply(Edit::Trim { seq, clip, edge, delta, ripple: false })?;
            }
        }
        Edge::Start => {
            let delta = to - c.start;
            match before_cut(s, clip) {
                Ok(left) => d.apply(Edit::Roll { seq, left, delta })?,
                Err(_) => d.apply(Edit::Trim { seq, clip, edge, delta, ripple: false })?,
            }
        }
    }
    d.finish(p)
}

/// Which half of a cut a split edit moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// The sound cut moves (J and L cuts).
    Audio,
    /// The picture cut moves.
    Video,
}

/// Split edit: moves only the sound (or only the picture) side of the cut
/// after `left` by `delta`. Sound moved earlier is a J cut (the next shot
/// is heard before it is seen), later an L cut (this shot's sound runs
/// under the next picture). Sound still inside the picture clips is
/// detached first (kept linked).
pub fn split_edit(p: &mut Project, seq: SequenceId, left: ClipId, delta: Micros, part: Part) -> Result<Edit> {
    if delta == 0 {
        return invalid("a split edit needs a non-zero offset");
    }
    let mut d = Draft::new(p);
    let s = d.seq(seq)?;
    let l = s.clip(left).ok_or_else(|| nf(format!("clip {}", left.0)))?.clone();
    let r = s.clips.iter().find(|o| o.lane == l.lane && o.start == l.end() && o.id != l.id && !o.is_gap()).cloned().ok_or_else(|| EditError::Invalid("no clip after that cut on its lane".into()))?;
    // Sound of each side as its own clip.
    let sound = |d: &mut Draft, c: &Clip| -> Result<ClipId> {
        let s = d.seq(seq)?;
        let has_own = partners_of(s, &HashSet::from([c.id])).into_iter().find(|p| s.lane(s.clip(*p).expect("listed").lane).is_some_and(|l| l.kind == LaneKind::Audio));
        if let Some(a) = has_own {
            return Ok(a);
        }
        let new_clip = ClipId(d.alloc_id());
        d.apply(Edit::DetachAudio { seq, clip: c.id, new_clip })?;
        Ok(new_clip)
    };
    let la = sound(&mut d, &l)?;
    let ra = sound(&mut d, &r)?;
    let s = d.seq(seq)?;
    let (a, b) = (s.clip(la).expect("made").clone(), s.clip(ra).expect("made").clone());
    if a.lane != b.lane || a.end() != b.start {
        return invalid("the two sides' sound clips do not meet on one lane");
    }
    let left_moving = match part {
        Part::Audio => la,
        Part::Video => l.id,
    };
    // Roll only this half: unlink the four, roll, link them back as they
    // were.
    let ids = [l.id, r.id, la, ra];
    let links: Vec<(ClipId, Option<LinkId>)> = ids.iter().map(|id| (*id, s.clip(*id).and_then(|c| c.link))).collect();
    d.apply(Edit::Unlink { seq, clips: ids.to_vec() })?;
    d.apply(Edit::Roll { seq, left: left_moving, delta })?;
    relink(&mut d, seq, links)?;
    d.finish(p)
}

/// Moves clips by `delta` (optionally onto `to_lane`) without their
/// linked partners, as with linked selection off: the links stay, so the
/// picture and its sound are now out of sync by `delta` (a sync slip).
pub fn move_alone(p: &mut Project, seq: SequenceId, clips: &[ClipId], delta: Micros, to_lane: Option<LaneId>) -> Result<Edit> {
    if clips.is_empty() {
        return invalid("no clips to move");
    }
    without_links(p, seq, clips, Edit::Move { seq, clips: clips.to_vec(), delta, to_lane })
}

/// Runs `edit` as if `clips` had no linked partners (linked selection
/// off): the partners stay where they are, and the links come back
/// afterwards. Any edit works: a move, a reorder on the main lane, a trim.
pub fn without_links(p: &mut Project, seq: SequenceId, clips: &[ClipId], edit: Edit) -> Result<Edit> {
    let mut d = Draft::new(p);
    let s = d.seq(seq)?;
    let set: HashSet<ClipId> = clips.iter().copied().collect();
    let mut all: Vec<ClipId> = clips.to_vec();
    all.extend(partners_of(s, &set));
    let links: Vec<(ClipId, Option<LinkId>)> = all.iter().map(|id| (*id, s.clip(*id).and_then(|c| c.link))).collect();
    d.apply(Edit::Unlink { seq, clips: all })?;
    d.apply(edit)?;
    relink(&mut d, seq, links)?;
    d.finish(p)
}

/// Links clips back into the groups they had.
fn relink(d: &mut Draft, seq: SequenceId, links: Vec<(ClipId, Option<LinkId>)>) -> Result<()> {
    let mut by_link: BTreeMap<u64, Vec<ClipId>> = BTreeMap::new();
    for (id, link) in links {
        if let Some(k) = link
            && d.seq(seq)?.clip(id).is_some()
        {
            by_link.entry(k.0).or_default().push(id);
        }
    }
    for (k, clips) in by_link {
        if clips.len() > 1 {
            d.apply(Edit::Link { seq, clips, link: LinkId(k) })?;
        }
    }
    Ok(())
}

// ------------------------------------------------------------ look-ups

/// Where a timeline frame comes from.
#[derive(Clone, Debug, PartialEq)]
pub struct Match {
    /// The clip on screen.
    pub clip: ClipId,
    /// What it plays.
    pub source: Source,
    /// Source time shown at the timeline time.
    pub source_time: Micros,
    /// First source instant the clip uses.
    pub source_in: Micros,
    /// One past the last source instant it uses.
    pub source_out: Micros,
}

/// Match frame: the source frame on screen at `t` (the topmost enabled,
/// visible clip with a source; `lane` picks one lane instead).
pub fn match_frame(p: &Project, seq: SequenceId, t: Micros, lane: Option<LaneId>) -> Option<Match> {
    let s = p.sequence(seq)?;
    s.lanes
        .iter()
        .filter(|l| lane.is_none_or(|x| x == l.id) && (lane.is_some() || !l.hidden))
        .find_map(|l| s.clip_at(l.id, t).filter(|c| c.enabled && c.has_source_time()))
        .map(|c| Match {
            clip: c.id,
            source: match &c.source {
                ClipSource::Media(m) => Source::Media(m.media),
                ClipSource::Sequence(r) => Source::Sequence(r.sequence),
                _ => unreachable!("has source time"),
            },
            source_time: c.source_time_at(t),
            source_in: c.source_in,
            source_out: c.source_out(),
        })
}

/// A use of a source frame on a timeline.
#[derive(Clone, Debug, PartialEq)]
pub struct Use {
    /// The sequence.
    pub seq: SequenceId,
    /// The clip showing the frame.
    pub clip: ClipId,
    /// Timeline time showing the frame.
    pub at: Micros,
}

/// Reverse match frame: every clip in every sequence showing source time
/// `t` of `src`.
pub fn reverse_match(p: &Project, src: Source, t: Micros) -> Vec<Use> {
    let mut out = Vec::new();
    for s in &p.sequences {
        for c in &s.clips {
            let same = match (&c.source, src) {
                (ClipSource::Media(m), Source::Media(x)) => m.media == x,
                (ClipSource::Sequence(r), Source::Sequence(x)) => r.sequence == x,
                _ => false,
            };
            if !same || t < c.source_in || t >= c.source_out() {
                continue;
            }
            if let Some(at) = timeline_time(c, t) {
                out.push(Use { seq: s.id, clip: c.id, at });
            }
        }
    }
    out
}

/// The timeline time at which a clip shows source time `src_t` (searching
/// through speed changes and reverse), if it does.
pub fn timeline_time(c: &Clip, src_t: Micros) -> Option<Micros> {
    if !c.has_source_time() || src_t < c.source_in || src_t > c.source_out() {
        return None;
    }
    if let Speed::Constant(r) = c.speed
        && r > 0.0
        && !c.reversed
    {
        return Some(c.start + (((src_t - c.source_in) as f64) / r).round() as Micros);
    }
    // Monotone in the clip: bisect.
    let (mut lo, mut hi) = (c.start, c.end());
    let up = !c.reversed;
    for _ in 0..64 {
        if hi - lo <= 1 {
            break;
        }
        let mid = (lo + hi) / 2;
        let v = c.source_time_at(mid);
        if (v < src_t) == up { lo = mid } else { hi = mid }
    }
    Some(lo)
}

/// An empty stretch of a lane.
#[derive(Clone, Debug, PartialEq)]
pub struct Gap {
    /// The lane.
    pub lane: LaneId,
    /// Where the empty stretch starts.
    pub start: Micros,
    /// Where it ends.
    pub end: Micros,
}

/// Gaps: spine gap clips, and holes between clips on other lanes. Holes
/// shorter than `min` are left out.
pub fn find_gaps(s: &Sequence, min: Micros) -> Vec<Gap> {
    let mut out = Vec::new();
    let spine = s.spine_id();
    for l in &s.lanes {
        let clips = s.clips_on(l.id);
        if l.id == spine {
            out.extend(clips.iter().filter(|c| c.is_gap()).map(|c| Gap { lane: l.id, start: c.start, end: c.end() }));
        } else {
            for w in clips.windows(2) {
                if w[1].start > w[0].end() {
                    out.push(Gap { lane: l.id, start: w[0].end(), end: w[1].start });
                }
            }
        }
    }
    out.retain(|g| g.end - g.start >= min.max(1));
    out.sort_by_key(|g| (g.start, g.lane));
    out
}

/// Source frames used more than once on a timeline.
#[derive(Clone, Debug, PartialEq)]
pub struct Duplicate {
    /// The media item used twice.
    pub media: MediaId,
    /// One clip.
    pub a: ClipId,
    /// The other clip.
    pub b: ClipId,
    /// The source range both clips show.
    pub source: TimeRange,
}

/// Duplicate frames: pairs of clips (not linked to each other, and both
/// showing picture or both sound) that use overlapping source of the same
/// media item, overlap at least `min` long.
pub fn find_duplicates(s: &Sequence, min: Micros) -> Vec<Duplicate> {
    let mut by_media: BTreeMap<u64, Vec<&Clip>> = BTreeMap::new();
    for c in &s.clips {
        if let ClipSource::Media(m) = &c.source
            && c.enabled
        {
            by_media.entry(m.media.0).or_default().push(c);
        }
    }
    let picture = |c: &Clip| matches!(&c.source, ClipSource::Media(m) if m.video) && s.lane(c.lane).is_some_and(|l| l.kind != LaneKind::Audio);
    let mut out = Vec::new();
    for (m, cs) in by_media {
        for (i, a) in cs.iter().enumerate() {
            for b in &cs[i + 1..] {
                if a.link.is_some() && a.link == b.link || picture(a) != picture(b) {
                    continue;
                }
                let lo = a.source_in.max(b.source_in);
                let hi = a.source_out().min(b.source_out());
                if hi - lo >= min.max(1) {
                    out.push(Duplicate { media: MediaId(m), a: a.id, b: b.id, source: TimeRange::from_bounds(lo, hi) });
                }
            }
        }
    }
    out
}

/// One line of the timeline index.
#[derive(Clone, Debug, PartialEq)]
pub struct IndexRow {
    /// `clip`, `gap`, `marker`, `clip_marker`, `transition`, `effect`.
    pub kind: &'static str,
    /// Where it starts on the sequence.
    pub start: Micros,
    /// Where it ends.
    pub end: Micros,
    /// Its lane (none for sequence markers).
    pub lane: Option<LaneId>,
    /// The clip it belongs to.
    pub clip: Option<ClipId>,
    /// Clip, marker, effect or transition name.
    pub name: String,
    /// Notes, source range, marker colour…
    pub detail: String,
}

/// Every clip, gap, marker, transition and effect of a sequence in time
/// order (the index shown beside the timeline).
pub fn timeline_index(p: &Project, s: &Sequence) -> Vec<IndexRow> {
    let mut rows = Vec::new();
    let secs = |t: Micros| t as f64 / 1e6;
    for c in &s.clips {
        let lane_name = s.lane(c.lane).map(|l| l.name.clone()).unwrap_or_default();
        if c.is_gap() {
            rows.push(IndexRow { kind: "gap", start: c.start, end: c.end(), lane: Some(c.lane), clip: Some(c.id), name: "gap".into(), detail: lane_name });
            continue;
        }
        let src = match &c.source {
            ClipSource::Media(m) => p.media_item(m.media).map(|i| format!("{} {:.2}-{:.2}", i.name, secs(c.source_in), secs(c.source_out()))).unwrap_or_default(),
            ClipSource::Sequence(r) => p.sequence(r.sequence).map(|q| format!("sequence \"{}\" {:.2}-{:.2}", q.name, secs(c.source_in), secs(c.source_out()))).unwrap_or_default(),
            _ => String::new(),
        };
        let name = if c.name.is_empty() { src.split(' ').next().unwrap_or("").to_owned() } else { c.name.clone() };
        let mut detail = format!("{lane_name}: {src}");
        if !c.speed.is_freeze() && c.speed != Speed::Constant(1.0) {
            detail.push_str(match c.speed {
                Speed::Curve(_) => ", speed ramp",
                _ => ", speed changed",
            });
        }
        if !c.enabled {
            detail.push_str(", disabled");
        }
        rows.push(IndexRow { kind: "clip", start: c.start, end: c.end(), lane: Some(c.lane), clip: Some(c.id), name, detail });
        for e in &c.effects {
            rows.push(IndexRow { kind: "effect", start: c.start, end: c.end(), lane: Some(c.lane), clip: Some(c.id), name: e.kind.clone(), detail: if e.enabled { String::new() } else { "off".into() } });
        }
        for m in &c.markers {
            rows.push(IndexRow { kind: "clip_marker", start: c.start + m.time, end: c.start + m.time + m.duration, lane: Some(c.lane), clip: Some(c.id), name: m.name.clone(), detail: marker_detail(m) });
        }
    }
    for m in &s.markers {
        rows.push(IndexRow { kind: "marker", start: m.time, end: m.time + m.duration, lane: None, clip: None, name: m.name.clone(), detail: marker_detail(m) });
    }
    for t in &s.transitions {
        if let Some(r) = s.clip(t.right) {
            let (pre, post) = t.split();
            rows.push(IndexRow { kind: "transition", start: r.start - pre, end: r.start + post, lane: Some(r.lane), clip: Some(t.left), name: t.kind.clone(), detail: format!("clips {} to {}", t.left.0, t.right.0) });
        }
    }
    rows.sort_by_key(|r| (r.start, r.kind != "marker", r.lane.map(|l| s.lane_index(l).unwrap_or(0)).unwrap_or(0)));
    rows
}

fn marker_detail(m: &Marker) -> String {
    let mut d = colour_name(m.colour).to_owned();
    if !m.note.is_empty() {
        d.push_str(": ");
        d.push_str(&m.note);
    }
    d
}

/// The marker colours editors pick from, by name.
pub const MARKER_COLOURS: [(&str, Rgba); 8] = [
    ("red", Rgba::new(0.90, 0.22, 0.21, 1.0)),
    ("orange", Rgba::new(0.96, 0.55, 0.15, 1.0)),
    ("yellow", Rgba::new(0.96, 0.84, 0.20, 1.0)),
    ("green", Rgba::new(0.30, 0.75, 0.35, 1.0)),
    ("cyan", Rgba::new(0.20, 0.78, 0.85, 1.0)),
    ("blue", Marker::DEFAULT_COLOUR),
    ("purple", Rgba::new(0.62, 0.40, 0.90, 1.0)),
    ("white", Rgba::new(0.95, 0.95, 0.95, 1.0)),
];

/// A marker colour by name (`None` for an unknown name).
pub fn marker_colour(name: &str) -> Option<Rgba> {
    let n = name.trim().to_lowercase();
    MARKER_COLOURS.iter().find(|(k, _)| *k == n || (n == "magenta" || n == "pink") && *k == "purple").map(|(_, c)| *c)
}

/// The nearest named marker colour.
pub fn colour_name(c: Rgba) -> &'static str {
    let d = |a: Rgba| (a.r - c.r).powi(2) + (a.g - c.g).powi(2) + (a.b - c.b).powi(2);
    MARKER_COLOURS.iter().min_by(|a, b| d(a.1).total_cmp(&d(b.1))).map(|(k, _)| *k).unwrap_or("blue")
}

// ------------------------------------------------------------- sequences

/// A new empty sequence (spine plus one audio lane), not made active.
pub fn new_sequence(p: &mut Project, name: &str) -> (Edit, SequenceId) {
    let id = SequenceId(p.alloc_id());
    let spine = LaneId(p.alloc_id());
    let audio = LaneId(p.alloc_id());
    let mut s = Sequence::new(id, name, spine);
    s.lanes.push(Lane::new(audio, LaneKind::Audio, "Audio 1"));
    (Edit::InsertSequence { sequence: Box::new(s), index: None }, id)
}

/// A copy of a sequence with fresh ids (lanes, clips, transitions,
/// markers, links and groups), placed after it.
pub fn duplicate_sequence(p: &mut Project, seq: SequenceId, name: &str) -> Result<(Edit, SequenceId)> {
    let src = p.sequence(seq).ok_or_else(|| nf(format!("sequence {}", seq.0)))?.clone();
    let index = p.sequences.iter().position(|s| s.id == seq).map(|i| i + 1);
    let mut map: HashMap<u64, u64> = HashMap::new();
    let mut fresh = |p: &mut Project, old: u64| *map.entry(old).or_insert_with(|| p.alloc_id());
    let mut s = src.clone();
    s.id = SequenceId(p.alloc_id());
    s.name = name.to_owned();
    for l in &mut s.lanes {
        l.id = LaneId(fresh(p, l.id.0));
    }
    for c in &mut s.clips {
        c.id = ClipId(fresh(p, c.id.0));
        c.lane = LaneId(fresh(p, c.lane.0));
        c.link = c.link.map(|k| LinkId(fresh(p, k.0)));
        c.group = c.group.map(|g| GroupId(fresh(p, g.0)));
        for m in &mut c.markers {
            m.id = MarkerId(fresh(p, m.id.0));
        }
    }
    for c in &mut s.clips {
        if let Some(par) = &mut c.parent {
            par.clip = ClipId(fresh(p, par.clip.0));
        }
    }
    for t in &mut s.transitions {
        t.id = TransitionId(fresh(p, t.id.0));
        t.left = ClipId(fresh(p, t.left.0));
        t.right = ClipId(fresh(p, t.right.0));
    }
    for m in &mut s.markers {
        m.id = MarkerId(fresh(p, m.id.0));
    }
    let id = s.id;
    Ok((Edit::InsertSequence { sequence: Box::new(s), index }, id))
}

/// One piece of a string-out or selects reel.
#[derive(Clone, Debug, PartialEq)]
pub struct Take {
    /// The media item.
    pub media: MediaId,
    /// Source start; `None` is the file's start.
    pub from: Option<Micros>,
    /// Source end; `None` is the file's end (stills: 5 s).
    pub to: Option<Micros>,
    /// Marker name put on the clip (scene/take, the line said…).
    pub label: String,
}

/// A new sequence with the takes end to end on the spine (each with a
/// marker carrying its label), `gap` apart: a string-out of dailies or a
/// selects reel of chosen moments. Not made active.
pub fn string_out(p: &mut Project, name: &str, takes: &[Take], gap: Micros) -> Result<(Edit, SequenceId)> {
    if takes.is_empty() {
        return invalid("no takes to string out");
    }
    let (edit, id) = new_sequence(p, name);
    let Edit::InsertSequence { mut sequence, index } = edit else { unreachable!() };
    let spine = sequence.spine_id();
    let mut at = 0;
    for t in takes {
        let item = p.media_item(t.media).ok_or_else(|| nf(format!("media {}", t.media.0)))?;
        let (len, item_name) = (item.duration(), item.name.clone());
        let from = t.from.unwrap_or(0).max(0);
        let to = t.to.or(len).unwrap_or(from + 5 * SECOND);
        let to = len.map_or(to, |l| to.min(l));
        if to <= from {
            return invalid(format!("take of \"{item_name}\" has an empty range"));
        }
        if gap > 0 && at > 0 {
            sequence.clips.push(Clip::gap(ClipId(p.alloc_id()), spine, at, gap));
            at += gap;
        }
        let mut c = Clip::media(ClipId(p.alloc_id()), spine, at, t.media, from, to - from);
        c.name = item_name;
        if !t.label.is_empty() {
            c.markers.push(Marker::new(MarkerId(p.alloc_id()), 0, t.label.clone()));
        }
        at += to - from;
        sequence.clips.push(c);
    }
    Ok((Edit::InsertSequence { sequence, index }, id))
}

// ------------------------------------------------------- scene and take

/// Scene and take facts kept on a media item (in its `extra`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TakeInfo {
    /// Scene number (`12`, `12A`).
    pub scene: Option<String>,
    /// Take number.
    pub take: Option<String>,
    /// Camera (`A`, `B`).
    pub camera: Option<String>,
    /// The director's pick ("circle take").
    pub circled: bool,
    /// Script supervisor's note.
    pub note: Option<String>,
    /// Star rating 0 to 5 (0: unrated).
    pub rating: u8,
}

const TAKE_KEYS: [&str; 6] = ["scene", "take", "camera", "circled", "take_note", "rating"];

/// The scene and take facts of a media item, falling back to what its
/// file name says.
pub fn take_info(m: &MediaItem) -> TakeInfo {
    let s = |k: &str| m.extra.get(k).and_then(|v| v.as_str()).map(str::to_owned).filter(|v| !v.is_empty());
    let parsed = parse_take(&m.name);
    TakeInfo {
        scene: s("scene").or_else(|| parsed.as_ref().map(|p| p.0.clone())),
        take: s("take").or_else(|| parsed.as_ref().and_then(|p| p.1.clone())),
        camera: s("camera").or_else(|| parsed.as_ref().and_then(|p| p.2.clone())),
        circled: m.extra.get("circled").and_then(|v| v.as_bool()).unwrap_or(false),
        note: s("take_note"),
        rating: m.extra.get("rating").and_then(|v| v.as_u64()).unwrap_or(0).min(5) as u8,
    }
}

/// Sets a media item's scene and take facts (empty strings clear).
pub fn set_take_info(p: &Project, media: MediaId, info: &TakeInfo) -> Result<Edit> {
    let mut m = p.media_item(media).ok_or_else(|| nf(format!("media {}", media.0)))?.clone();
    for k in TAKE_KEYS {
        m.extra.remove(k);
    }
    let mut put = |k: &str, v: &Option<String>| {
        if let Some(v) = v.as_ref().filter(|v| !v.trim().is_empty()) {
            m.extra.insert(k.into(), v.trim().into());
        }
    };
    put("scene", &info.scene);
    put("take", &info.take);
    put("camera", &info.camera);
    put("take_note", &info.note);
    if info.circled {
        m.extra.insert("circled".into(), true.into());
    }
    if info.rating > 0 {
        m.extra.insert("rating".into(), (info.rating.min(5) as u64).into());
    }
    Ok(Edit::SetMedia { item: Box::new(m) })
}

/// Scene, take and camera read from a file name: `S12_T3`, `Sc12-Tk3`,
/// `12A-3`, `Scene 4 Take 2`, `A001_S12_T03_CamB`…
pub fn parse_take(name: &str) -> Option<(String, Option<String>, Option<String>)> {
    let stem = name.rsplit_once('.').map_or(name, |(a, _)| a).to_lowercase();
    let tokens: Vec<&str> = stem.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()).collect();
    let mut scene = None;
    let mut take = None;
    let mut camera = None;
    let num_after = |t: &str, prefixes: &[&str]| prefixes.iter().find_map(|p| t.strip_prefix(p).filter(|r| r.chars().next().is_some_and(|c| c.is_ascii_digit())).map(str::to_owned));
    let mut i = 0;
    while i < tokens.len() {
        let t = tokens[i];
        let next = tokens.get(i + 1).copied();
        if matches!(t, "scene" | "sc" | "s") && next.is_some_and(|n| n.chars().next().is_some_and(|c| c.is_ascii_digit())) {
            scene = next.map(str::to_owned);
            i += 2;
            continue;
        }
        if matches!(t, "take" | "tk" | "t") && next.is_some_and(|n| n.chars().all(|c| c.is_ascii_digit())) {
            take = next.map(str::to_owned);
            i += 2;
            continue;
        }
        if let Some(v) = num_after(t, &["scene", "sc", "s"]).filter(|_| scene.is_none()) {
            scene = Some(v);
        } else if let Some(v) = num_after(t, &["take", "tk", "t"]).filter(|v| v.chars().all(|c| c.is_ascii_digit())) {
            take = Some(v);
        } else if let Some(v) = t.strip_prefix("cam").filter(|v| !v.is_empty()) {
            camera = Some(v.to_uppercase());
        }
        i += 1;
    }
    // Slate style `12A-3`: a scene with a letter, then the take.
    if scene.is_none() && tokens.len() >= 2 {
        for w in tokens.windows(2) {
            let s0 = w[0];
            if s0.chars().next().is_some_and(|c| c.is_ascii_digit()) && s0.chars().last().is_some_and(|c| c.is_ascii_alphabetic()) && s0.len() <= 5 && w[1].chars().all(|c| c.is_ascii_digit()) && w[1].len() <= 3 {
                scene = Some(s0.to_uppercase());
                take = Some(w[1].to_owned());
                break;
            }
        }
    }
    let tidy = |v: String| {
        let trimmed = v.trim_start_matches('0');
        if trimmed.is_empty() { "0".to_owned() } else { trimmed.to_uppercase() }
    };
    scene.map(|s| (tidy(s), take.map(tidy), camera))
}

// ------------------------------------------------------------ speed ramps

/// A speed ramp through `(source position 0..1, speed)` points: the clip
/// keeps its source range and its length follows the speeds (slow parts
/// last longer). Speeds change in straight lines between the points.
pub fn speed_ramp(p: &Project, seq: SequenceId, clip: ClipId, points: &[(f64, f64)]) -> Result<Edit> {
    let s = p.sequence(seq).ok_or_else(|| nf(format!("sequence {}", seq.0)))?;
    let c = s.clip(clip).ok_or_else(|| nf(format!("clip {}", clip.0)))?;
    if !c.has_source_time() {
        return invalid("only media and nested clips can change speed");
    }
    if points.len() < 2 {
        return invalid("a speed ramp needs at least two points");
    }
    let mut pts: Vec<(f64, f64)> = points.iter().map(|&(u, r)| (u.clamp(0.0, 1.0), r)).collect();
    pts.sort_by(|a, b| a.0.total_cmp(&b.0));
    if pts.iter().any(|p| !(p.1 > 0.01 && p.1 <= 100.0)) {
        return invalid("ramp speeds must be between 0.01 and 100");
    }
    if pts[0].0 > 0.0 {
        pts.insert(0, (0.0, pts[0].1));
    }
    if pts.last().expect("two").0 < 1.0 {
        let r = pts.last().expect("two").1;
        pts.push((1.0, r));
    }
    let len = c.source_len() as f64;
    // Clip time at each point: t = ∫ ds / r(s), r linear in source between
    // points (exact for linear r: ds/r integrates to a log).
    let mut keys = Vec::new();
    let mut t = 0.0;
    for (i, &(u, r)) in pts.iter().enumerate() {
        if i > 0 {
            let (u0, r0) = pts[i - 1];
            let ds = (u - u0) * len;
            t += if (r - r0).abs() < 1e-9 { ds / r } else { ds * (r / r0).ln() / (r - r0) };
        }
        keys.push(Keyframe::new(t.round() as Micros, Value::Scalar(r), Interp::Linear));
    }
    keys.dedup_by_key(|k| k.time);
    Ok(Edit::SetSpeed { seq, clip, speed: Speed::Curve(Track::new(keys)) })
}

// ------------------------------------------------------- text-based edits

/// Cuts source ranges out of a clip (words taken out of its transcript):
/// each range is found on the timeline, extracted from the clip's lane and
/// its linked partners' lanes (and sync-locked lanes), and the rest closes
/// up. Ranges outside the clip are ignored.
pub fn remove_source_ranges(p: &mut Project, seq: SequenceId, clip: ClipId, ranges: &[(Micros, Micros)]) -> Result<Edit> {
    let s = p.sequence(seq).ok_or_else(|| nf(format!("sequence {}", seq.0)))?;
    let c = s.clip(clip).ok_or_else(|| nf(format!("clip {}", clip.0)))?.clone();
    if !c.has_source_time() {
        return invalid("only media clips have source to cut");
    }
    // Timeline spans, merged, latest first.
    let mut spans: Vec<(Micros, Micros)> = ranges
        .iter()
        .filter_map(|&(a, b)| {
            let (a, b) = (a.max(c.source_in), b.min(c.source_out()));
            if b <= a {
                return None;
            }
            let (ta, tb) = (timeline_time(&c, a)?, timeline_time(&c, b)?);
            Some((ta.min(tb).max(c.start), ta.max(tb).min(c.end())))
        })
        .filter(|(a, b)| b > a)
        .collect();
    if spans.is_empty() {
        return invalid("none of those words fall inside the clip");
    }
    spans.sort();
    let mut merged: Vec<(Micros, Micros)> = Vec::new();
    for (a, b) in spans {
        match merged.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => merged.push((a, b)),
        }
    }
    let mut lanes = vec![c.lane];
    for id in partners_of(s, &HashSet::from([clip])) {
        if let Some(l) = s.clip(id).map(|x| x.lane)
            && !lanes.contains(&l)
        {
            lanes.push(l);
        }
    }
    lanes.extend(sync_lanes(s, &lanes));
    let mut d = Draft::new(p);
    for (a, b) in merged.into_iter().rev() {
        remove_span(&mut d, seq, &lanes, a, b, true, &HashSet::new())?;
    }
    d.finish(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::{MediaInfo, MediaKind, VideoInfo, AudioInfo};

    const S: Micros = SECOND;

    struct T {
        p: Project,
        seq: SequenceId,
        spine: LaneId,
        over: LaneId,
        music: LaneId,
        a: MediaId,
        b: MediaId,
    }

    fn movie(p: &mut Project, name: &str, secs: i64) -> MediaId {
        let id = MediaId(p.alloc_id());
        let info = MediaInfo { kind: MediaKind::Video, duration: Some(secs * S), video: Some(VideoInfo::default()), audio: Some(AudioInfo::default()), ..MediaInfo::default() };
        let mut m = MediaItem::new(id, format!("/footage/{name}"), info);
        m.name = name.into();
        p.apply(Edit::InsertMedia { item: Box::new(m), index: None }).unwrap();
        id
    }

    fn setup() -> T {
        let mut p = Project::default();
        let seq = p.active_sequence;
        let spine = p.active().spine_id();
        let a = movie(&mut p, "S12_T3.mov", 60);
        let b = movie(&mut p, "Sc4-Tk2_CamB.mp4", 60);
        let over = LaneId(p.alloc_id());
        p.apply(Edit::AddLane { seq, lane: Lane::new(over, LaneKind::Visual, "Titles"), index: None }).unwrap();
        let music = LaneId(p.alloc_id());
        p.apply(Edit::AddLane { seq, lane: Lane::new(music, LaneKind::Audio, "Music"), index: None }).unwrap();
        // Spine: a 0-10 from 0s, b 10-20 from 5s, a 20-30 from 30s.
        for (m, sin, at) in [(a, 0, 0), (b, 5, 10), (a, 30, 20)] {
            let c = Clip::media(ClipId(0), spine, 0, m, sin * S, 10 * S);
            p.apply(Edit::Insert { seq, lane: None, at: at * S, clip: Box::new(c) }).unwrap();
        }
        T { p, seq, spine, over, music, a, b }
    }

    impl T {
        fn apply(&mut self, e: Edit) {
            self.p.apply(e).expect("planned edit applies");
        }

        fn put(&mut self, lane: LaneId, at: i64, dur: i64) -> ClipId {
            let id = ClipId(self.p.alloc_id());
            let c = Clip::media(id, lane, at * S, self.b, 0, dur * S);
            self.p.apply(Edit::InsertClip { seq: self.seq, clip: Box::new(c) }).unwrap();
            id
        }
    }

    fn spine(t: &T) -> Vec<(Micros, Micros, Micros)> {
        t.p.active().spine_clips().iter().map(|c| (c.start / S, c.duration / S, c.source_in / S)).collect()
    }

    fn edit(src_in: Option<i64>, src_out: Option<i64>, rec_in: Option<i64>, rec_out: Option<i64>, mode: SourceMode, t: &T) -> SourceEdit {
        SourceEdit { source: Source::Media(t.b), src_in: src_in.map(|v| v * S), src_out: src_out.map(|v| v * S), rec_in: rec_in.map(|v| v * S), rec_out: rec_out.map(|v| v * S), mode, video_lane: Some(t.spine), audio_lane: None, audio: true }
    }

    #[test]
    fn three_point_insert_overwrite_and_backtiming() {
        let mut t = setup();
        let before = t.p.clone();
        // Insert 4 s of b (source 20-24) at 10 s: everything after moves.
        let e = edit(Some(20), Some(24), Some(10), None, SourceMode::Insert, &t);
        let (ed, done) = source_edit(&mut t.p, t.seq, &e, 0).unwrap();
        assert_eq!(done.rec, TimeRange::new(10 * S, 4 * S));
        t.apply(ed);
        assert_eq!(spine(&t), vec![(0, 10, 0), (10, 4, 20), (14, 10, 5), (24, 10, 30)]);
        // Undo returns exactly.
        let mut u = before.clone();
        let (ed, _) = source_edit(&mut u, t.seq, &e, 0).unwrap();
        let inv = u.apply(ed).unwrap();
        u.apply(inv).unwrap();
        assert!(u.content_eq(&before));
        // Backtimed overwrite: source in/out and record out; ends at 24.
        let e = edit(Some(40), Some(42), None, Some(24), SourceMode::Overwrite, &t);
        let (ed, done) = source_edit(&mut t.p, t.seq, &e, 0).unwrap();
        assert_eq!(done.rec, TimeRange::new(22 * S, 2 * S));
        t.apply(ed);
        assert_eq!(spine(&t), vec![(0, 10, 0), (10, 4, 20), (14, 8, 5), (22, 2, 40), (24, 10, 30)]);
        // Record in/out plus source in: the record length wins.
        let e = edit(Some(1), None, Some(0), Some(3), SourceMode::Overwrite, &t);
        let (_, done) = source_edit(&mut t.p, t.seq, &e, 0).unwrap();
        assert_eq!(done.src, TimeRange::new(S, 3 * S));
        // Not enough source after the in mark.
        let e = edit(Some(58), None, Some(0), Some(5), SourceMode::Overwrite, &t);
        assert!(matches!(source_edit(&mut t.p, t.seq, &e, 0), Err(EditError::NoHandles(_))));
    }

    #[test]
    fn fit_to_fill_and_replace() {
        let mut t = setup();
        // 6 s of source into 3 s of record: double speed.
        let e = edit(Some(0), Some(6), Some(10), Some(13), SourceMode::FitToFill, &t);
        let (ed, done) = source_edit(&mut t.p, t.seq, &e, 0).unwrap();
        assert!((done.speed - 2.0).abs() < 1e-9);
        t.apply(ed);
        let c = t.p.active().clip(done.clips[0]).unwrap().clone();
        assert_eq!((c.start, c.duration, c.source_out()), (10 * S, 3 * S, 6 * S));
        assert_eq!(t.p.active().duration(), 30 * S);
        // Replace the last clip with a, the source mark at 40 s lining up
        // with the record mark 25 s (5 s into the clip).
        let mut e = edit(Some(40), None, Some(25), None, SourceMode::Replace, &t);
        e.source = Source::Media(t.a);
        let (ed, done) = source_edit(&mut t.p, t.seq, &e, 0).unwrap();
        t.apply(ed);
        let c = t.p.active().clip(done.clips[0]).unwrap();
        assert_eq!((c.start, c.duration, c.source_in, c.media_id()), (20 * S, 10 * S, 35 * S, Some(t.a)));
    }

    #[test]
    fn insert_moves_sync_locked_lanes_only() {
        let mut t = setup();
        let title = t.put(t.over, 15, 2);
        let song = t.put(t.music, 12, 6);
        t.apply(set_sync_lock(&t.p, t.seq, t.music, true).unwrap());
        let e = edit(Some(0), Some(2), Some(14), None, SourceMode::Insert, &t);
        let (ed, _) = source_edit(&mut t.p, t.seq, &e, 0).unwrap();
        t.apply(ed);
        let s = t.p.active();
        // The title (not sync-locked) stays; the song splits at 14 and its
        // tail moves 2 s.
        assert_eq!(s.clip(title).unwrap().start, 15 * S);
        let music: Vec<(Micros, Micros)> = s.clips_on(t.music).iter().map(|c| (c.start / S, c.duration / S)).collect();
        assert_eq!(music, vec![(12, 2), (16, 4)]);
        let _ = song;
    }

    #[test]
    fn lift_and_extract_ranges() {
        let mut t = setup();
        t.put(t.over, 8, 4);
        let lanes = [t.spine];
        let ed = lift(&mut t.p, t.seq, 5 * S, 15 * S, Some(&lanes)).unwrap();
        t.apply(ed);
        assert_eq!(spine(&t), vec![(0, 5, 0), (5, 10, 0), (15, 5, 10), (20, 10, 30)]);
        assert!(t.p.active().spine_clips()[1].is_gap());
        // Extract on every targeted lane closes the space everywhere.
        let mut t = setup();
        t.put(t.over, 8, 4);
        t.put(t.over, 20, 2);
        let ed = extract(&mut t.p, t.seq, 5 * S, 15 * S, None).unwrap();
        t.apply(ed);
        assert_eq!(spine(&t), vec![(0, 5, 0), (5, 5, 10), (10, 10, 30)]);
        let over: Vec<(Micros, Micros)> = t.p.active().clips_on(t.over).iter().map(|c| (c.start / S, c.duration / S)).collect();
        // The title 8-12 lost 8-12 ∩ 5-15 entirely; the one at 20 moved to 10.
        assert_eq!(over, vec![(10, 2)]);
        // An untargeted lane is left alone.
        let mut t = setup();
        let keep = t.put(t.over, 20, 2);
        t.apply(set_targeted(&t.p, t.seq, t.over, false).unwrap());
        let ed = extract(&mut t.p, t.seq, 5 * S, 15 * S, None).unwrap();
        t.apply(ed);
        assert_eq!(t.p.active().clip(keep).unwrap().start, 20 * S);
    }

    #[test]
    fn ripple_trim_keeps_sync_locked_music_in_step() {
        let mut t = setup();
        let song = t.put(t.music, 25, 4);
        let title = t.put(t.over, 25, 4);
        t.apply(set_sync_lock(&t.p, t.seq, t.music, true).unwrap());
        let first = t.p.active().spine_clips()[0].id;
        let ed = ripple_trim(&mut t.p, t.seq, first, Edge::End, -2 * S).unwrap();
        t.apply(ed);
        assert_eq!(spine(&t), vec![(0, 8, 0), (8, 10, 5), (18, 10, 30)]);
        assert_eq!(t.p.active().clip(song).unwrap().start, 23 * S);
        assert_eq!(t.p.active().clip(title).unwrap().start, 25 * S);
        // Lengthening opens time on the music lane too.
        let ed = ripple_trim(&mut t.p, t.seq, first, Edge::End, 3 * S).unwrap();
        t.apply(ed);
        assert_eq!(t.p.active().clip(song).unwrap().start, 26 * S);
        // A head trim pulls everything after it left by the same amount.
        let second = t.p.active().spine_clips()[1].id;
        let ed = ripple_trim(&mut t.p, t.seq, second, Edge::Start, S).unwrap();
        t.apply(ed);
        assert_eq!(t.p.active().clip(song).unwrap().start, 25 * S);
    }

    #[test]
    fn extend_and_asymmetric_trims() {
        let mut t = setup();
        let first = t.p.active().spine_clips()[0].id;
        let second = t.p.active().spine_clips()[1].id;
        let ed = extend(&mut t.p, t.seq, first, Edge::End, 12 * S).unwrap();
        t.apply(ed);
        assert_eq!(spine(&t), vec![(0, 12, 0), (12, 8, 7), (20, 10, 30)]);
        let ed = extend(&mut t.p, t.seq, second, Edge::Start, 11 * S).unwrap();
        t.apply(ed);
        assert_eq!(spine(&t), vec![(0, 11, 0), (11, 9, 6), (20, 10, 30)]);
        // Asymmetric: the spine's outgoing side and the music's incoming
        // side ripple the same direction.
        let m1 = t.put(t.music, 0, 11);
        let m2 = t.put(t.music, 11, 9);
        let sides = [TrimSide { clip: first, edge: Edge::End, kind: TrimKind::Ripple }, TrimSide { clip: m2, edge: Edge::Start, kind: TrimKind::Ripple }];
        let ed = trim_sides(&mut t.p, t.seq, &sides, S).unwrap();
        t.apply(ed);
        let s = t.p.active();
        assert_eq!(s.clip(first).unwrap().duration, 12 * S);
        assert_eq!(s.clip(m2).unwrap().duration, 8 * S);
        assert_eq!(s.clip(m1).unwrap().duration, 11 * S);
    }

    #[test]
    fn j_and_l_cuts_move_only_the_sound() {
        let mut t = setup();
        let first = t.p.active().spine_clips()[0].id;
        let second = t.p.active().spine_clips()[1].id;
        // J cut: the second shot is heard 2 s before it is seen.
        let ed = split_edit(&mut t.p, t.seq, first, -2 * S, Part::Audio).unwrap();
        t.apply(ed);
        let s = t.p.active();
        assert_eq!(spine(&t), vec![(0, 10, 0), (10, 10, 5), (20, 10, 30)]);
        let audio: Vec<&Clip> = s.clips.iter().filter(|c| s.lane(c.lane).is_some_and(|l| l.kind == LaneKind::Audio)).collect();
        let a1 = audio.iter().find(|c| c.link == s.clip(first).unwrap().link).unwrap();
        let a2 = audio.iter().find(|c| c.link == s.clip(second).unwrap().link).unwrap();
        assert_eq!((a1.end(), a2.start, a2.source_in), (8 * S, 8 * S, 3 * S));
        assert!(s.clip(first).unwrap().link.is_some());
        // L cut on the next cut: the second shot's sound runs 1 s on.
        let ed = split_edit(&mut t.p, t.seq, second, S, Part::Audio).unwrap();
        t.apply(ed);
        let s = t.p.active();
        let a2 = s.clips.iter().find(|c| c.link == s.clip(second).unwrap().link && c.id != second).unwrap();
        assert_eq!(a2.end(), 21 * S);
        assert_eq!(s.clip(second).unwrap().end(), 20 * S);
    }

    #[test]
    fn moving_without_the_linked_sound_keeps_the_link() {
        let mut t = setup();
        let first = t.p.active().spine_clips()[0].id;
        let audio = ClipId(t.p.alloc_id());
        t.apply(Edit::DetachAudio { seq: t.seq, clip: first, new_clip: audio });
        // A sync slip of the sound by a quarter frame (subframe).
        let ed = move_alone(&mut t.p, t.seq, &[audio], S / 96, None).unwrap();
        t.apply(ed);
        let s = t.p.active();
        assert_eq!(s.clip(audio).unwrap().start, S / 96);
        assert_eq!(s.clip(first).unwrap().start, 0);
        assert!(s.clip(audio).unwrap().link.is_some() && s.clip(audio).unwrap().link == s.clip(first).unwrap().link);
        // A reorder on the main lane leaves the sound where it was.
        let last = t.p.active().spine_clips()[2].id;
        let ed = without_links(&mut t.p, t.seq, &[first], Edit::Reorder { seq: t.seq, clip: first, before: Some(last) }).unwrap();
        t.apply(ed);
        let s = t.p.active();
        assert_eq!(s.clip(first).unwrap().start, 10 * S);
        assert_eq!(s.clip(audio).unwrap().start, S / 96);
        assert_eq!(s.clip(audio).unwrap().link, s.clip(first).unwrap().link);
    }

    #[test]
    fn match_frames_gaps_duplicates_and_index() {
        let mut t = setup();
        let m = match_frame(&t.p, t.seq, 12 * S, None).unwrap();
        assert_eq!((m.source, m.source_time), (Source::Media(t.b), 7 * S));
        let uses = reverse_match(&t.p, Source::Media(t.a), 35 * S);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].at, 25 * S);
        // Same source used twice.
        let c = Clip::media(ClipId(0), t.spine, 0, t.a, 2 * S, 3 * S);
        t.p.apply(Edit::Append { seq: t.seq, lane: None, clip: Box::new(c) }).unwrap();
        let d = find_duplicates(t.p.active(), S / 2);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].source, TimeRange::new(2 * S, 3 * S));
        t.put(t.over, 2, 2);
        t.put(t.over, 6, 2);
        let g = find_gaps(t.p.active(), 0);
        assert_eq!(g, vec![Gap { lane: t.over, start: 4 * S, end: 6 * S }]);
        let ix = timeline_index(&t.p, t.p.active());
        assert!(ix.iter().filter(|r| r.kind == "clip").count() >= 6);
    }

    #[test]
    fn sequences_string_outs_and_takes() {
        let mut t = setup();
        let (ed, id) = duplicate_sequence(&mut t.p, t.seq, "Cut 2").unwrap();
        t.apply(ed);
        let (a, b) = (t.p.sequence(t.seq).unwrap(), t.p.sequence(id).unwrap());
        assert_eq!(a.clips.len(), b.clips.len());
        assert!(a.clips.iter().all(|c| b.clip(c.id).is_none()));
        let (ed, id) = string_out(&mut t.p, "Dailies", &[Take { media: t.a, from: None, to: None, label: "12/3".into() }, Take { media: t.b, from: Some(S), to: Some(4 * S), label: String::new() }], S).unwrap();
        t.apply(ed);
        let s = t.p.sequence(id).unwrap();
        assert_eq!(s.duration(), 64 * S);
        assert_eq!(s.spine_clips()[0].markers[0].name, "12/3");
        assert_eq!(parse_take("S12_T3.mov"), Some(("12".into(), Some("3".into()), None)));
        assert_eq!(parse_take("Sc4-Tk2_CamB.mp4"), Some(("4".into(), Some("2".into()), Some("B".into()))));
        assert_eq!(parse_take("12A-3.mov"), Some(("12A".into(), Some("3".into()), None)));
        assert_eq!(parse_take("Scene 7 Take 11.mxf"), Some(("7".into(), Some("11".into()), None)));
        assert_eq!(parse_take("holiday.mp4"), None);
        let info = TakeInfo { circled: true, rating: 4, note: Some("best read".into()), ..take_info(t.p.media_item(t.a).unwrap()) };
        t.apply(set_take_info(&t.p, t.a, &info).unwrap());
        let back = take_info(t.p.media_item(t.a).unwrap());
        assert_eq!((back.scene.as_deref(), back.take.as_deref(), back.circled, back.rating), (Some("12"), Some("3"), true, 4));
    }

    #[test]
    fn speed_ramps_keep_the_source_range() {
        let mut t = setup();
        let first = t.p.active().spine_clips()[0].id;
        let ed = speed_ramp(&t.p, t.seq, first, &[(0.0, 1.0), (0.5, 0.25), (1.0, 1.0)]).unwrap();
        t.apply(ed);
        let c = t.p.active().clip(first).unwrap();
        assert!(matches!(c.speed, Speed::Curve(_)));
        assert!((c.source_len() - 10 * S).abs() < S / 50, "{}", c.source_len());
        assert!(c.duration > 15 * S, "slow middle makes it longer: {}", c.duration);
        // Later clips moved with it.
        assert_eq!(t.p.active().spine_clips()[1].start, c.end());
    }

    #[test]
    fn text_cuts_take_words_out() {
        let mut t = setup();
        let second = t.p.active().spine_clips()[1].id;
        t.put(t.music, 0, 30);
        t.apply(set_sync_lock(&t.p, t.seq, t.music, true).unwrap());
        // The second clip shows source 5-15; cut 7-8 and 12-13.5.
        let ed = remove_source_ranges(&mut t.p, t.seq, second, &[(7 * S, 8 * S), (12 * S, 27 * S / 2)]).unwrap();
        t.apply(ed);
        let sp: Vec<(Micros, Micros)> = t.p.active().spine_clips().iter().map(|c| (c.source_in * 2 / S, c.duration * 2 / S)).collect();
        assert_eq!(sp, vec![(0, 20), (10, 4), (16, 8), (27, 3), (60, 20)]);
        // The sync-locked music lost the same 2.5 s.
        assert_eq!(t.p.active().clips_on(t.music).iter().map(|c| c.duration).sum::<Micros>(), 30 * S - 5 * S / 2);
    }
}
