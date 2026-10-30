// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A shorter version of the project (15 / 30 / 60 seconds, or any length), optionally vertical:
//! the shots are scored, the weakest dropped or shortened, and the result saved as a project of
//! its own beside the original. The person's project is never changed. The agent's `cutdown`
//! tool and the Export screen both call [`cutdown`].

use crate::agent::Session;
use crate::deliver_run as run;
use crate::doc::Doc;
use crate::services::project as proj;
use crate::view::{self, ClipKind, LaneKind, SEC};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use studio_deliver::cut::{self, Action, Shot};
use studio_deliver::queue::JobSpec;
use studio_deliver::specs;
use studio_model as m;

fn secs(t: view::Time) -> f64 {
    t as f64 / SEC as f64
}

fn r1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// Also render the shorter version to a file.
#[derive(Clone, Debug, Default)]
pub struct RenderReq {
    pub qc: bool,
    pub spec: Option<String>,
    pub size: Option<(u32, u32)>,
    /// Folder for the file; default `<new project>/exports`.
    pub out: Option<PathBuf>,
}

#[derive(Clone, Debug, Default)]
pub struct CutdownReq {
    /// The length to cut to.
    pub seconds: f64,
    /// The project's folder (saved already); the cutdown goes into its cutdowns folder.
    pub root: PathBuf,
    pub vertical: bool,
    pub name: Option<String>,
    /// Shots (clip ids) that must stay, and that must go.
    pub keep: Vec<u32>,
    pub drop: Vec<u32>,
    pub render: Option<RenderReq>,
}

#[derive(Clone, Debug)]
pub struct CutdownOut {
    /// The new project's folder.
    pub project: PathBuf,
    pub name: String,
    pub seconds: f64,
    pub asked: f64,
    pub planned_seconds: f64,
    pub kept: Vec<u32>,
    pub dropped: Vec<u32>,
    pub shortened: Vec<(u32, f64)>,
    pub vertical: bool,
    pub reframed_clips: usize,
    /// The rendered file, when asked for.
    pub file: Option<PathBuf>,
    pub qc: Option<String>,
}

impl CutdownOut {
    /// The agent tool's reply.
    pub fn to_json(&self) -> serde_json::Value {
        let mut reply = json!({
            "project": self.project.display().to_string(), "name": self.name, "seconds": r1(self.seconds), "asked": self.asked, "planned_seconds": r1(self.planned_seconds),
            "kept": self.kept, "dropped": self.dropped, "shortened": self.shortened.iter().map(|(id, d)| json!({"clip": id, "to": r1(*d)})).collect::<Vec<_>>(),
            "vertical": self.vertical, "reframed_clips": self.reframed_clips,
            "next": "open it, look (contact_sheet), adjust with keep / drop and run again if a shot you need is gone",
        });
        if let Some(f) = &self.file {
            reply["file"] = json!(f.display().to_string());
        }
        if let Some(q) = &self.qc {
            reply["qc"] = json!(q);
        }
        reply
    }
}

pub(crate) fn sanitize(s: &str) -> String {
    let t: String = s.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' { c } else { '_' }).collect();
    let t = t.trim().to_string();
    if t.is_empty() { "Untitled".into() } else { t }
}

pub(crate) fn free_dir(parent: &Path, stem: &str) -> PathBuf {
    (1..).map(|n| if n == 1 { parent.join(stem) } else { parent.join(format!("{stem} {n}")) }).find(|p| !p.exists()).expect("a free name")
}

/// Cuts `doc` down to `req.seconds`, saves the result as a project of its own and, when asked, renders it.
pub fn cutdown(doc: &Doc, req: CutdownReq) -> Result<CutdownOut, String> {
    let target = req.seconds;
    if target < 2.0 {
        return Err("a cutdown needs at least 2 seconds".into());
    }
    let root = &req.root;
    let vertical = req.vertical;
    let base = req.name.clone().unwrap_or_else(|| format!("{} {}s{}", doc.project.name, target as u32, if vertical { " vertical" } else { "" }));
    let p = doc.project.clone();
    let spine: Vec<&view::Lane> = p.lanes.iter().filter(|l| l.kind == LaneKind::Spine).collect();
    let main = spine.first().ok_or("the project has no main lane")?.id;
    let mut shots_v: Vec<&view::Clip> = p.clips.iter().filter(|c| c.lane == main).collect();
    shots_v.sort_by_key(|c| c.start);
    if shots_v.is_empty() {
        return Err("the main lane is empty: nothing to cut".into());
    }
    let total = secs(p.duration());
    if target >= total - 0.05 {
        return Err(format!("the programme is {total:.1} s: a {target:.0} s version would be longer; cutdowns shorten"));
    }
    let dur = p.duration();
    let spans = run::speech_spans(doc, 0, dur);
    let (keep_ids, drop_ids) = (&req.keep, &req.drop);
    let n = shots_v.len();
    let shots: Vec<Shot> = shots_v
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let (s, e) = (secs(c.start), secs(c.start + c.dur));
            let speech: f64 = spans.iter().map(|sp| (sp.1.min(e) - sp.0.max(s)).max(0.0)).sum::<f64>() / (e - s).max(0.01);
            let titled = p.clips.iter().filter(|o| o.kind == ClipKind::Title && o.name != "Caption" && secs(o.start) < e && secs(o.start + o.dur) > s).count();
            let marked = p.markers.iter().filter(|mk| secs(mk.time) >= s && secs(mk.time) < e).count();
            let position = if i == 0 || i + 1 == n { 1.0 } else { 0.0 };
            Shot { id: c.id, dur: e - s, score: 1.0 + 3.0 * speech + 1.2 * titled.min(2) as f64 + 1.5 * marked.min(2) as f64 + position, keep: keep_ids.contains(&c.id), drop: drop_ids.contains(&c.id) }
        })
        .collect();
    let plan = cut::plan(&shots, target);
    let kept_secs = cut::kept(&shots, &plan);

    // Work on a copy; the person's project is never changed.
    let mut cp = Session::new(Doc::new(doc.model.clone()));
    let mut dropped: Vec<u32> = Vec::new();
    let mut trimmed: Vec<(u32, f64)> = Vec::new();
    // Ranges to remove, latest first so earlier times stay true.
    let mut ranges: Vec<(f64, f64, Option<u32>, Vec<u32>)> = Vec::new(); // (from, to, trim clip, deleted clips)
    for (c, (id, act)) in shots_v.iter().zip(&plan) {
        let (s, e) = (secs(c.start), secs(c.start + c.dur));
        match act {
            Action::Keep => {}
            Action::Drop => {
                dropped.push(*id);
                ranges.push((s, e, None, vec![*id]));
            }
            Action::Trim(d) => {
                trimmed.push((*id, *d));
                ranges.push((s + d, e, Some(*id), vec![]));
            }
        }
    }
    ranges.sort_by(|x, y| y.0.total_cmp(&x.0));
    let others: Vec<u32> = p.clips.iter().filter(|c| c.lane != main).map(|c| c.id).collect();
    for (from, to, trim, del) in &ranges {
        let l = to - from;
        let mut ops: Vec<Value> = Vec::new();
        // Clips on the other lanes, against this range, from the copy's current state.
        let snapshot: Vec<(u32, f64, f64)> = others.iter().filter_map(|id| cp.doc.project.clip(*id).map(|c| (*id, secs(c.start), secs(c.start + c.dur)))).collect();
        let (mut gone, mut shift, mut shorten_end, mut cut_start_then_shift, mut end_at_from) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for (id, cs, ce) in snapshot {
            if cs >= *from - 1e-6 && ce <= *to + 1e-6 {
                gone.push(id);
            } else if cs >= *to - 1e-6 {
                shift.push(id);
            } else if cs < *from && ce > *to {
                shorten_end.push((id, ce - l));
            } else if cs < *from && ce > *from {
                end_at_from.push((id, *from));
            } else if cs >= *from && cs < *to && ce > *to {
                cut_start_then_shift.push((id, *to));
            }
        }
        if !gone.is_empty() {
            ops.push(json!({"op": "delete", "clips": gone, "ripple": false}));
        }
        for (id, e) in &end_at_from {
            ops.push(json!({"op": "trim", "clip": id, "end": e}));
        }
        for (id, e) in &shorten_end {
            ops.push(json!({"op": "trim", "clip": id, "end": e}));
        }
        for (id, s) in &cut_start_then_shift {
            ops.push(json!({"op": "trim", "clip": id, "start": s}));
        }
        if !del.is_empty() {
            ops.push(json!({"op": "delete", "clips": del, "ripple": true}));
        }
        if let Some(id) = trim {
            ops.push(json!({"op": "trim", "clip": id, "end": from, "ripple": true}));
        }
        let moving: Vec<u32> = shift.iter().copied().chain(cut_start_then_shift.iter().map(|x| x.0)).collect();
        if !moving.is_empty() {
            ops.push(json!({"op": "move", "clips": moving, "by": -l}));
        }
        if !ops.is_empty() {
            cp.edit(&ops, "Cutdown", false)?;
        }
    }
    // Sound and overlays end with the picture; sound fades out.
    let end = secs(cp.doc.project.clips.iter().filter(|c| c.lane == main).map(|c| c.start + c.dur).max().unwrap_or(0));
    let mut tail: Vec<Value> = Vec::new();
    let late: Vec<(u32, bool, f64)> = cp.doc.project.clips.iter().filter(|c| c.lane != main && secs(c.start + c.dur) > end + 1e-6).map(|c| (c.id, secs(c.start) >= end - 1e-6, secs(c.start))).collect();
    let (gone, trim_to): (Vec<_>, Vec<_>) = late.into_iter().partition(|c| c.1);
    if !gone.is_empty() {
        tail.push(json!({"op": "delete", "clips": gone.iter().map(|c| c.0).collect::<Vec<_>>(), "ripple": false}));
    }
    for c in trim_to {
        tail.push(json!({"op": "trim", "clip": c.0, "end": end}));
    }
    if !tail.is_empty() {
        cp.edit(&tail, "Cutdown ends", false)?;
    }
    let audio_lanes: Vec<u32> = cp.doc.project.lanes.iter().filter(|l| l.kind == LaneKind::Audio).map(|l| l.id).collect();
    let fades: Vec<Value> = cp.doc.project.clips.iter().filter(|c| audio_lanes.contains(&c.lane) && (secs(c.start + c.dur) - end).abs() < 0.05).map(|c| json!({"op": "set", "clip": c.id, "fade_out": (c.dur as f64 / 1e6).min(1.5)})).collect();
    if !fades.is_empty() {
        let _ = cp.edit(&fades, "Cutdown fade", false);
    }
    let mut converted = 0;
    if vertical {
        let mut st2 = cp.doc.model.settings.clone();
        let (w, h) = (1080u32, 1920u32);
        st2.width = w;
        st2.height = h;
        let ok = cp.doc.apply("Vertical", m::Edit::SetSettings { settings: Box::new(st2) });
        if !ok {
            return Err("could not set the vertical canvas".into());
        }
        let vids: Vec<u32> = cp.doc.project.clips.iter().filter(|c| c.lane == main && c.kind == ClipKind::Video).map(|c| c.id).collect();
        if !vids.is_empty() {
            cp.edit(&vids.iter().map(|id| json!({"op": "set", "clip": id, "fit": "fill_picture"})).collect::<Vec<_>>(), "Cutdown fill", false)?;
            // Follow the subject where the footage can be read.
            match cp.edit(&[json!({"op": "reframe", "clips": vids})], "Cutdown reframe", false) {
                Ok(_) => converted = vids.len(),
                Err(_) => {}
            }
        }
    }
    // Save as a project of its own.
    let out_root = m::file::ProjectFolder::new(&root).export_dir(m::file::ExportKind::Cutdowns);
    std::fs::create_dir_all(&out_root).map_err(|e| e.to_string())?;
    let dir = free_dir(&out_root, &sanitize(&base));
    let seq = cp.doc.seq();
    cp.doc.apply("Name", m::Edit::RenameSequence { id: seq, name: base.clone() });
    proj::save(&mut cp.doc, Some(&dir))?;
    let final_secs = secs(cp.doc.project.duration());
    let mut out = CutdownOut {
        project: dir.clone(),
        name: base.clone(),
        seconds: final_secs,
        asked: target,
        planned_seconds: kept_secs,
        kept: shots.iter().zip(&plan).filter(|(_, (_, a))| *a != Action::Drop).map(|(s, _)| s.id).collect(),
        dropped,
        shortened: trimmed,
        vertical,
        reframed_clips: converted,
        file: None,
        qc: None,
    };
    if let Some(r) = &req.render {
        let mut j = JobSpec::new(&base, &dir);
        j.qc = r.qc;
        if let Some(d) = &r.spec {
            j.delivery = specs::find(d).map(|s| s.id.to_owned()).unwrap_or_default();
        }
        if j.delivery.is_empty() {
            j.preset = if vertical { "Vertical".into() } else { "Social 1080p".into() };
        }
        j.size = r.size.map(|(w, h)| if vertical { (w.min(h), w.max(h)) } else { (w, h) });
        let file_dir = r.out.clone().unwrap_or_else(|| dir.join("exports"));
        std::fs::create_dir_all(&file_dir).map_err(|e| e.to_string())?;
        j.out = file_dir.join(format!("{}.mp4", sanitize(&base)));
        let o = run::run_spec(&j, &mut |_| {})?;
        out.file = Some(j.out.clone());
        out.qc = o.qc.map(|(v, _)| v);
    }
    Ok(out)
}
