// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Finishing and delivery as tools: `queue` (background renders, nodes),
//! `qc` (quality control of a file), `deliver` (a platform or mastering
//! spec), `bundle` (master, proxy, sound, captions, pictures, cutdowns in
//! one folder with a manifest) and `cutdown` (15 / 30 / 60 second versions,
//! vertical reframes).
//!
//! The work is in `studio-deliver` and [`crate::deliver_run`]; these read
//! the arguments and answer in small JSON.

use super::{Session, f, secs};
use crate::deliver_run as run;
use crate::doc::Doc;
use crate::services::project as proj;
use crate::view::{self, ClipKind, LaneKind};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use studio_deliver::bundle::{self, Cue, Manifest};
use studio_deliver::cut::{self, Action, Shot};
use studio_deliver::queue::{JobSpec, QueuePreset};
use studio_deliver::{qc, specs, wire};
use studio_model as m;

fn st<'a>(a: &'a Value, k: &str) -> Option<&'a str> {
    a.get(k).and_then(Value::as_str).filter(|s| !s.trim().is_empty())
}

fn flag(a: &Value, k: &str) -> Option<bool> {
    a.get(k).and_then(Value::as_bool)
}

fn size_arg(a: &Value) -> Option<(u32, u32)> {
    match a.get("size") {
        Some(Value::Array(sz)) if sz.len() == 2 => Some((sz[0].as_u64()? as u32, sz[1].as_u64()? as u32)),
        _ => None,
    }
}

fn r1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// A report in a few lines: the verdict, the numbers, the worst problems.
pub fn report_json(r: &qc::Report, report_file: Option<&Path>) -> Value {
    let findings: Vec<Value> = r.findings.iter().take(25).map(|f| json!({"check": f.check, "severity": f.severity, "from": r1(f.start), "to": r1(f.end), "problem": f.message, "fix": f.fix})).collect();
    json!({
        "file": r.file, "profile": r.profile, "verdict": r.verdict, "passed": r.passed, "fails": r.fails, "warnings": r.warnings,
        "measurements": r.measurements, "findings": findings,
        "more_findings": r.findings.len().saturating_sub(25),
        "report": report_file.map(|p| p.display().to_string()),
    })
}

impl Session {
    /// The project's folder; the project is saved first so a render reads what the agent sees.
    fn project_root(&mut self) -> Result<PathBuf, String> {
        let root = self.home.clone().or_else(|| self.doc.folder.as_ref().map(|f| f.root().to_path_buf())).ok_or_else(|| "work in a project folder first (zornov agent --project DIR, or open one): renders and nodes read the saved project".to_owned())?;
        if self.doc.folder.is_some() || self.home.is_some() {
            let _ = proj::save(&mut self.doc, None);
        }
        Ok(root)
    }

    fn job_spec(&mut self, a: &Value, preset: Option<&QueuePreset>) -> Result<JobSpec, String> {
        let root = self.project_root()?;
        let name = st(a, "name").map(str::to_owned).unwrap_or_else(|| self.doc.project.name.clone());
        let mut j = JobSpec::new(&name, &root);
        if let Some(p) = preset {
            j.delivery = p.delivery.clone();
            j.preset = p.preset.clone();
            j.qc = p.qc;
            j.bundle = p.bundle;
            j.distributed = p.distributed;
            j.priority = p.priority;
        }
        if let Some(d) = st(a, "delivery").or_else(|| st(a, "spec")) {
            j.delivery = specs::find(d).map(|s| s.id.to_owned()).ok_or_else(|| format!("No delivery spec \"{d}\"; specs: {}", specs::SPECS.iter().map(|s| s.id).collect::<Vec<_>>().join(", ")))?;
        }
        if let Some(p) = st(a, "preset") {
            j.preset = p.to_owned();
        }
        if let Some(p) = st(a, "path") {
            j.out = PathBuf::from(p);
        }
        if let Some(Value::Array(r)) = a.get("range") {
            if r.len() == 2 {
                j.range = Some((r[0].as_f64().unwrap_or(0.0), r[1].as_f64().unwrap_or(0.0)));
            }
        }
        j.qc = flag(a, "qc").unwrap_or(j.qc);
        j.bundle = flag(a, "bundle").unwrap_or(j.bundle);
        j.distributed = flag(a, "distributed").unwrap_or(j.distributed);
        if let Some(p) = a.get("priority").and_then(Value::as_i64) {
            j.priority = p as i32;
        }
        if let Some(c) = f(a, "chunk_secs") {
            j.chunk_secs = c;
        }
        j.size = size_arg(a).or(j.size);
        // Refuse a job that could never run, now.
        run::plan_job(&self.doc, &j)?;
        Ok(j)
    }

    pub(super) fn deliver_tool(&mut self, tool: &str, a: &Value) -> Result<Value, String> {
        match tool {
            "queue" => self.queue_tool(a),
            "qc" => self.qc_tool(a),
            "deliver" => self.deliver_spec_tool(a),
            "bundle" => self.bundle_tool(a),
            "cutdown" => self.cutdown_tool(a),
            _ => Err(format!("unknown tool {tool}")),
        }
    }

    // ---------------------------------------------------------------- queue

    fn queue_tool(&mut self, a: &Value) -> Result<Value, String> {
        let store = run::store();
        let action = st(a, "action").unwrap_or("list");
        let job_id = |a: &Value| -> Result<u32, String> { a.get("job").and_then(Value::as_u64).map(|v| v as u32).ok_or_else(|| "this needs \"job\": the queue job id".to_owned()) };
        match action {
            "add" => {
                let preset = match st(a, "from_preset") {
                    Some(n) => Some(store.read().presets.iter().find(|p| p.name.eq_ignore_ascii_case(n)).cloned().ok_or_else(|| format!("No queue preset \"{n}\""))?),
                    None => None,
                };
                let spec = self.job_spec(a, preset.as_ref())?;
                let name = spec.name.clone();
                let id = store.update(|q| q.add(spec))?;
                // Stop takes back the queued render the AI added.
                super::jobs::on_cancel(move || {
                    let _ = run::store().update(|q| q.cancel(id));
                });
                let ahead = store.read().jobs.iter().filter(|j| !j.state.finished() && j.id != id).count();
                Ok(json!({"job": id, "name": name, "ahead_of_it": ahead, "next": "queue {\"action\": \"run\"} draws the queue here; the Zornov window draws it while open; zornov queue run draws it from a shell"}))
            }
            "list" => Ok(json!({"jobs": run::queue_json(&store.read())})),
            "pause" => {
                let id = job_id(a)?;
                store.update(|q| q.pause(id))??;
                Ok(json!({"job": id, "state": "paused"}))
            }
            "resume" => {
                let id = job_id(a)?;
                store.update(|q| q.resume(id))??;
                Ok(json!({"job": id, "resumed": true}))
            }
            "cancel" => {
                let id = job_id(a)?;
                store.update(|q| q.cancel(id))??;
                Ok(json!({"job": id, "state": "cancelled"}))
            }
            "retry" => {
                let id = job_id(a)?;
                store.update(|q| q.retry(id))??;
                Ok(json!({"job": id, "state": "queued"}))
            }
            "priority" => {
                let id = job_id(a)?;
                let p = a.get("priority").and_then(Value::as_i64).ok_or("priority needs \"priority\": a number, higher first")? as i32;
                store.update(|q| q.set_priority(id, p))??;
                Ok(json!({"job": id, "priority": p}))
            }
            "clear" => Ok(json!({"removed": store.update(|q| q.clear_finished())?})),
            "run" => {
                let stop = std::sync::atomic::AtomicBool::new(false);
                if flag(a, "wait") == Some(false) {
                    std::thread::spawn(|| {
                        let stop = std::sync::atomic::AtomicBool::new(false);
                        let _ = run::run_queue(&stop, true);
                    });
                    return Ok(json!({"started": true, "note": "drawing in the background; list shows progress (a one-shot headless process ends it: use wait for scripts)"}));
                }
                let n = run::run_queue(&stop, true)?;
                let q = store.read();
                Ok(json!({"jobs_drawn": n, "jobs": run::queue_json(&q)}))
            }
            "notices" => Ok(json!({"notices": store.update(|q| q.drain_notices())?.iter().map(|n| json!({"job": n.job, "title": n.title, "body": n.body, "ok": n.ok})).collect::<Vec<_>>()})),
            "presets" => Ok(json!({"presets": store.read().presets.iter().map(|p| json!({"name": p.name, "delivery": p.delivery, "preset": p.preset, "qc": p.qc, "bundle": p.bundle, "distributed": p.distributed, "priority": p.priority})).collect::<Vec<_>>()})),
            "save_preset" => {
                let name = st(a, "name").ok_or("save_preset needs \"name\"")?.to_owned();
                let delivery = match st(a, "delivery").or_else(|| st(a, "spec")) {
                    Some(d) => specs::find(d).map(|s| s.id.to_owned()).ok_or_else(|| format!("No delivery spec \"{d}\""))?,
                    None => String::new(),
                };
                let p = QueuePreset { name: name.clone(), delivery, preset: st(a, "preset").unwrap_or("").to_owned(), qc: flag(a, "qc").unwrap_or(true), bundle: flag(a, "bundle").unwrap_or(false), distributed: flag(a, "distributed").unwrap_or(false), priority: a.get("priority").and_then(Value::as_i64).unwrap_or(0) as i32 };
                store.update(|q| q.save_preset(p))?;
                Ok(json!({"saved": name}))
            }
            "nodes" => {
                let nodes = run::load_nodes();
                let pack = std::sync::Arc::new(wire::ProjectPack::new("ping"));
                let out: Vec<Value> = nodes
                    .iter()
                    .map(|n| {
                        let addr = n.addr.parse::<std::net::SocketAddr>().ok().or_else(|| std::net::ToSocketAddrs::to_socket_addrs(&n.addr.as_str()).ok().and_then(|mut x| x.next()));
                        match addr.map(|a| wire::RemoteNode::new(a, &n.token, pack.clone()).ping()) {
                            Some(Ok(i)) => json!({"name": n.name, "addr": n.addr, "online": true, "cores": i.cores, "busy": i.busy}),
                            Some(Err(e)) => json!({"name": n.name, "addr": n.addr, "online": false, "error": e}),
                            None => json!({"name": n.name, "addr": n.addr, "online": false, "error": "cannot resolve the address"}),
                        }
                    })
                    .collect();
                Ok(json!({"nodes": out, "how": "on each render computer run: zornov node (it prints its address and pairing token); then queue {\"action\": \"add_node\", \"addr\": \"HOST:7878\", \"token\": \"…\"}; jobs with distributed: true share their chunks with every node that answers"}))
            }
            "add_node" => {
                let addr = st(a, "addr").ok_or("add_node needs \"addr\": HOST:PORT")?.to_owned();
                let token = st(a, "token").ok_or("add_node needs \"token\": the pairing token the node printed")?.to_owned();
                let sock = std::net::ToSocketAddrs::to_socket_addrs(&addr.as_str()).map_err(|e| format!("{addr}: {e}"))?.next().ok_or_else(|| format!("{addr}: no address"))?;
                let info = wire::RemoteNode::new(sock, &token, std::sync::Arc::new(wire::ProjectPack::new("ping"))).ping()?;
                let name = st(a, "name").map(str::to_owned).unwrap_or(info.name.clone());
                let mut nodes = run::load_nodes();
                nodes.retain(|n| n.addr != addr && n.name != name);
                nodes.push(run::NodeEntry { name: name.clone(), addr, token });
                run::save_nodes(&nodes)?;
                Ok(json!({"added": name, "cores": info.cores}))
            }
            "remove_node" => {
                let key = st(a, "name").or_else(|| st(a, "addr")).ok_or("remove_node needs \"name\" or \"addr\"")?;
                let mut nodes = run::load_nodes();
                let before = nodes.len();
                nodes.retain(|n| n.name != key && n.addr != key);
                run::save_nodes(&nodes)?;
                Ok(json!({"removed": before - nodes.len()}))
            }
            "discover" => {
                let wait = f(a, "wait").unwrap_or(1.5).clamp(0.3, 10.0);
                let found = wire::discover(wire::DISCOVERY_PORT, std::time::Duration::from_secs_f64(wait));
                Ok(json!({"found": found.iter().map(|(n, a)| json!({"name": n, "addr": a.to_string()})).collect::<Vec<_>>(), "note": "add one with add_node and the token printed on that computer"}))
            }
            other => Err(format!("unknown queue action \"{other}\"; actions: add, list, pause, resume, cancel, retry, priority, clear, run, notices, presets, save_preset, nodes, add_node, remove_node, discover")),
        }
    }

    // ------------------------------------------------------------------- qc

    fn qc_tool(&mut self, a: &Value) -> Result<Value, String> {
        let profile = |s: &Session, a: &Value| -> Result<qc::Profile, String> {
            match st(a, "spec") {
                Some(d) => Ok(specs::find(d).ok_or_else(|| format!("No delivery spec \"{d}\""))?.profile(s.doc.project.fps as f64, (s.doc.project.width, s.doc.project.height))),
                None => Ok(qc::Profile::default()),
            }
        };
        if flag(a, "preflight") == Some(true) || (st(a, "path").is_none() && self.last_export_path().is_none()) {
            return self.preflight(a, profile(self, a)?);
        }
        let path = match st(a, "path") {
            Some(p) => PathBuf::from(p),
            None => self.last_export_path().ok_or("qc needs \"path\": a video file (or none: it checks the last export)")?,
        };
        let range = (0, self.doc.project.duration());
        let extra = if flag(a, "project_context") == Some(false) { Default::default() } else { run::qc_extra(&self.doc, range) };
        let mut extra = extra;
        extra.max_secs = f(a, "max_seconds");
        let prof = profile(self, a)?;
        let (report, file) = run::qc_and_write(&path, &prof, &extra)?;
        Ok(report_json(&report, Some(&file)))
    }

    fn last_export_path(&self) -> Option<PathBuf> {
        self.exports.last().map(|(_, j)| j.request.path.clone()).filter(|p| p.is_file())
    }

    /// Checks that need no render: loudness of the mix, captions against speech, the canvas against the spec.
    fn preflight(&mut self, a: &Value, profile: qc::Profile) -> Result<Value, String> {
        let dur = self.doc.project.duration();
        let l = super::quality::loudness(&self.doc.model, 0, dur);
        let cues = run::caption_cues(&self.doc);
        let spans = run::speech_spans(&self.doc, 0, dur);
        let speech: f64 = spans.iter().map(|s| s.1 - s.0).sum();
        let facts = qc::Facts { file: self.doc.project.name.clone(), width: self.doc.project.width, height: self.doc.project.height, fps: self.doc.project.fps as f64, video_secs: secs(dur), audio_secs: if l.lufs.is_finite() { Some(secs(dur)) } else { None }, caption_cues: Some(cues.len()), speech_secs: Some(speech), ..Default::default() };
        // The export scales the canvas to the spec's size: only a different shape is a problem.
        let want = profile.expect_size;
        let mut profile = profile;
        profile.expect_size = None;
        profile.expect_fps = None;
        let mut r = qc::report(&profile, &facts, None, None);
        if let Some((w, h)) = want {
            let (have, need) = (self.doc.project.width as f64 / self.doc.project.height.max(1) as f64, w as f64 / h as f64);
            if (have / need - 1.0).abs() > 0.01 {
                r.findings.push(qc::Finding { check: "aspect".into(), severity: qc::Severity::Warning, start: 0.0, end: 0.0, message: format!("The canvas is {have:.3}:1 and {} wants {need:.3}:1; the picture will be fitted with bars or cropped.", profile.name), fix: "Reframe the project to the spec's shape first (cutdown with vertical, or reframe), or deliver at the canvas's own shape.".into() });
            }
        }
        // Loudness of the mix against the target, from the mix itself.
        if l.lufs.is_finite() {
            if let Some((target, tol, ceil)) = profile.loudness {
                if (l.lufs - target).abs() > tol {
                    r.findings.push(qc::Finding { check: "loudness".into(), severity: qc::Severity::Fail, start: 0.0, end: secs(dur), message: format!("The mix measures {:.1} LUFS; {} wants {target:.0} +-{tol:.1}.", l.lufs, profile.name), fix: "Normalise the master to the target (loudness tools of the sound desk), then export.".into() });
                }
                if (l.peak_db as f64) > ceil + 0.0 && l.peak_db > 0.0 {
                    r.findings.push(qc::Finding { check: "true_peak".into(), severity: qc::Severity::Fail, start: 0.0, end: secs(dur), message: format!("Sample peak {:.1} dBFS; the ceiling is {ceil:.1} dBTP.", l.peak_db), fix: "Limit the master.".into() });
                }
            }
        }
        // Titles cut by the frame edge or under the app buttons of vertical platforms: the same
        // judgement `check` makes on the rendered picture, reported as safe-area findings.
        if flag(a, "safe_area") != Some(false) {
            let q = self.quality(true);
            for p in q["problems"].as_array().cloned().unwrap_or_default() {
                if matches!(p["kind"].as_str(), Some("title_cut_off" | "title_under_app_ui")) {
                    let at = p["at"].as_f64().unwrap_or(0.0);
                    let cut = p["kind"] == "title_cut_off";
                    r.findings.push(qc::Finding { check: "safe_area".into(), severity: if cut { qc::Severity::Fail } else { qc::Severity::Warning }, start: at, end: at, message: p["problem"].as_str().unwrap_or("").to_owned(), fix: p["fix"].as_str().unwrap_or("").to_owned() });
                }
            }
        }
        r.findings.sort_by(|x, y| y.severity.cmp(&x.severity).then(x.start.total_cmp(&y.start)));
        r.fails = r.findings.iter().filter(|f| f.severity == qc::Severity::Fail).count();
        r.warnings = r.findings.iter().filter(|f| f.severity == qc::Severity::Warning).count();
        r.passed = r.fails == 0;
        r.verdict = if r.fails > 0 { "FAIL".into() } else if r.warnings > 0 { "PASS with warnings".into() } else { "PASS".into() };
        let mut out = report_json(&r, None);
        out["preflight"] = json!(true);
        out["note"] = json!("This looked at the project, not a file; the file QC (picture levels, flashes, black, frozen, dropouts) runs on the export: pass \"path\", or export first.");
        let _ = a;
        Ok(out)
    }

    /// QC for an export just written: the summary the export reply carries.
    pub(super) fn qc_after_export(&mut self, path: &Path, range: (view::Time, view::Time), spec: Option<&str>) -> Option<Value> {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        if !matches!(ext.as_str(), "mp4" | "mov" | "m4v") {
            return None;
        }
        let profile = match spec.and_then(specs::find) {
            Some(d) => d.profile(self.doc.project.fps as f64, (self.doc.project.width, self.doc.project.height)),
            None => qc::Profile::default(),
        };
        let extra = run::qc_extra(&self.doc, range);
        match run::qc_and_write(path, &profile, &extra) {
            Ok((r, file)) => {
                let worst: Vec<Value> = r.findings.iter().take(5).map(|f| json!({"check": f.check, "severity": f.severity, "at": r1(f.start), "problem": f.message, "fix": f.fix})).collect();
                Some(json!({"verdict": r.verdict, "fails": r.fails, "warnings": r.warnings, "worst": worst, "report": file.display().to_string()}))
            }
            Err(e) => Some(json!({"error": e})),
        }
    }

    // -------------------------------------------------------------- deliver

    fn deliver_spec_tool(&mut self, a: &Value) -> Result<Value, String> {
        if st(a, "spec").is_none() && st(a, "delivery").is_none() || flag(a, "list") == Some(true) {
            return Ok(json!({"specs": specs::SPECS.iter().map(|s| json!({"id": s.id, "name": s.name, "kind": match s.kind { specs::Kind::File => "file", specs::Kind::Dcp => "dcp", specs::Kind::Imf => "imf" }, "max_seconds": s.max_secs, "captions": s.captions, "strict_qc": s.strict, "notes": s.notes})).collect::<Vec<_>>(), "use": "deliver {\"spec\": \"youtube\"} renders that spec, checks it (QC) and reports; add queue: true to render in the background"}));
        }
        let spec = self.job_spec(a, None)?;
        if flag(a, "queue") == Some(true) {
            let store = run::store();
            let name = spec.name.clone();
            let id = store.update(|q| q.add(spec))?;
            return Ok(json!({"job": id, "name": name, "queued": true}));
        }
        let started = std::time::Instant::now();
        let kind = specs::find(&spec.delivery).map(|s| s.kind);
        let o = run::run_spec(&spec, &mut |_| {})?;
        let mut out = json!({"ok": true, "outputs": o.outputs.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(), "took_seconds": r1(started.elapsed().as_secs_f64()), "machines": o.machines});
        if let Some((verdict, file)) = &o.qc {
            if let Ok(b) = std::fs::read(file) {
                if let Ok(r) = serde_json::from_slice::<qc::Report>(&b) {
                    out["qc"] = report_json(&r, Some(file));
                    let _ = verdict;
                }
            }
        }
        match kind {
            Some(specs::Kind::Dcp) => out["note"] = json!("A DCP folder: picture and sound MXF, CPL, PKL, ASSETMAP. Unsigned and unencrypted; try it in a DCP player (DCP-o-matic, a cinema server) before sending it to a cinema."),
            Some(specs::Kind::Imf) => out["note"] = json!("An IMF folder (experimental): validate it with Netflix Photon or another IMF checker before delivering."),
            _ => {}
        }
        Ok(out)
    }

    // --------------------------------------------------------------- bundle

    fn bundle_tool(&mut self, a: &Value) -> Result<Value, String> {
        let root = self.project_root()?;
        let name = st(a, "name").map(str::to_owned).unwrap_or_else(|| self.doc.project.name.clone());
        let folder = match st(a, "out") {
            Some(p) => PathBuf::from(p),
            None => {
                let base = root.join("deliveries");
                let _ = std::fs::create_dir_all(&base);
                let mut n = 1;
                loop {
                    let p = base.join(if n == 1 { sanitize(&name) } else { format!("{} {n}", sanitize(&name)) });
                    if !p.exists() {
                        break p;
                    }
                    n += 1;
                }
            }
        };
        std::fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
        let mut man = Manifest::new(&name);
        let mut notes: Vec<Value> = Vec::new();
        let spec_id = st(a, "spec").or_else(|| st(a, "delivery")).map(str::to_owned);
        // Master.
        if flag(a, "master") != Some(false) {
            let dir = folder.join("master");
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let mut j = JobSpec::new(&name, &root);
            j.qc = true;
            j.size = size_arg(a);
            j.preset = if self.doc.project.height >= 2160 { "4K master".into() } else { "Social 1080p".into() };
            if let Some(d) = &spec_id {
                j.delivery = specs::find(d).map(|s| s.id.to_owned()).ok_or_else(|| format!("No delivery spec \"{d}\""))?;
            }
            let plan = run::plan_job(&self.doc, &j)?;
            if plan.kind == specs::Kind::File {
                j.out = dir.join(format!("{}.mp4", sanitize(&name)));
                let o = run::run_spec(&j, &mut |_| {})?;
                man.add(&folder, &format!("master/{}", j.out.file_name().unwrap().to_string_lossy()), "master", "the picture and mix").map_err(|e| e.to_string())?;
                for p in o.outputs.iter().skip(1) {
                    if let Ok(rel) = p.strip_prefix(&folder) {
                        man.add(&folder, &rel.to_string_lossy(), "qc", "quality-control report").map_err(|e| e.to_string())?;
                    }
                }
                if let Some((v, _)) = &o.qc {
                    notes.push(json!({"master_qc": v}));
                }
            } else {
                man.missing.push(format!("master: {} is a package; use deliver for it, then bundle with master: false", j.delivery));
            }
        }
        self.bundle_rest(a, &root, &folder, &name, &mut man, &mut notes)?;
        man.write(&folder).map_err(|e| e.to_string())?;
        Ok(json!({"folder": folder.display().to_string(), "files": man.items.len(), "missing": man.missing, "notes": notes, "manifest": folder.join("manifest.json").display().to_string()}))
    }

    /// Everything but the master: proxy, sound, captions, pictures, cutdowns.
    fn bundle_rest(&mut self, a: &Value, root: &Path, folder: &Path, name: &str, man: &mut Manifest, notes: &mut Vec<Value>) -> Result<(), String> {
        let io = |e: std::io::Error| e.to_string();
        let dur = self.doc.project.duration();
        // Proxy.
        if flag(a, "proxy") != Some(false) {
            let dir = folder.join("proxy");
            std::fs::create_dir_all(&dir).map_err(io)?;
            let mut j = JobSpec::new(name, root);
            j.delivery = "web-preview".into();
            j.qc = false;
            j.size = size_arg(a).map(|(w, h)| ((w / 2).max(2), (h / 2).max(2)));
            j.out = dir.join(format!("{}_proxy.mp4", sanitize(name)));
            run::run_spec(&j, &mut |_| {})?;
            man.add(folder, &format!("proxy/{}", j.out.file_name().unwrap().to_string_lossy()), "proxy", "720p review copy").map_err(io)?;
        }
        // Captions.
        if flag(a, "captions") != Some(false) {
            let cues: Vec<Cue> = run::caption_cues(&self.doc).into_iter().map(|(s, e, t)| Cue { start: s, end: e, text: t }).collect();
            std::fs::create_dir_all(folder.join("captions")).map_err(io)?;
            let desk_tracks = crate::sound_desk::subs::write_sidecars(&self.doc.model, &folder.join("captions").join(format!("{}.mp4", sanitize(name))), 0, dur);
            if cues.is_empty() && desk_tracks.is_empty() {
                man.missing.push("captions: the project has none (run ai captions to make them)".into());
            } else if cues.is_empty() {
                man.add(folder, "captions", "captions", "subtitle tracks from the sound department").map_err(io)?;
            } else {
                let dir = folder.join("captions");
                std::fs::create_dir_all(&dir).map_err(io)?;
                std::fs::write(dir.join(format!("{}.srt", sanitize(name))), bundle::srt(&cues)).map_err(io)?;
                std::fs::write(dir.join(format!("{}.vtt", sanitize(name))), bundle::vtt(&cues)).map_err(io)?;
                man.add(folder, "captions", "captions", &format!("{} cues", cues.len())).map_err(io)?;
            }
        }
        // Sound: the full mix, and the stems.
        if flag(a, "audio") != Some(false) && dur > 0 {
            let pcm = crate::sound_bridge::mix_for_export(&self.doc.model, 0, dur);
            if pcm.iter().any(|v| v.abs() > 1e-6) {
                let dir = folder.join("audio");
                std::fs::create_dir_all(&dir).map_err(io)?;
                std::fs::write(dir.join(format!("{}_mix.wav", sanitize(name))), bundle::wav24(&pcm, 2, crate::sound_bridge::RATE)).map_err(io)?;
                man.add(folder, &format!("audio/{}_mix.wav", sanitize(name)), "audio", "the full mix, 24-bit 48 kHz").map_err(io)?;
                // Stems from the sound department: DX, MX, FX, M&E and the full mix (and each dubbed language).
                if flag(a, "stems") != Some(false) {
                    let desk = crate::sound_desk::Desk::of(&self.doc.model);
                    match crate::sound_desk::render::export_stems(&self.doc.model, &dir.join("stems"), desk.layout(), false, name) {
                        Ok(w) if !w.is_empty() => {
                            man.add(folder, "audio/stems", "stems", &format!("{} stems, {}", w.len(), desk.layout().id())).map_err(io)?;
                        }
                        Ok(_) => {}
                        Err(e) => man.missing.push(format!("stems: {e}")),
                    }
                }
            } else {
                man.missing.push("audio: the project has no sound".into());
            }
        }
        // Pictures.
        if flag(a, "thumbnails") != Some(false) || flag(a, "poster") != Some(false) {
            let dir = folder.join("pictures");
            std::fs::create_dir_all(&dir).map_err(io)?;
            let n = a.get("thumbnails").and_then(Value::as_u64).unwrap_or(8).clamp(1, 24) as i64;
            if dur > 0 {
                if flag(a, "thumbnails") != Some(false) {
                    for i in 0..n {
                        let t = dur * (2 * i + 1) / (2 * n);
                        let jpeg = self.frames.frame(&self.doc, t, 640, 85)?;
                        std::fs::write(dir.join(format!("thumb_{:02}.jpg", i + 1)), jpeg).map_err(io)?;
                    }
                    man.add(folder, "pictures", "pictures", "thumbnails").map_err(io)?;
                }
                if flag(a, "poster") != Some(false) {
                    // The best of 24 frames: contrast, detail, good exposure.
                    let (mut best, mut best_t) = (-1.0f64, dur / 2);
                    for i in 0..24i64 {
                        let t = (dur * (2 * i + 1) / 48).min(dur - 1);
                        if let Ok(img) = self.frames.rgba(&self.doc, t, 320) {
                            let s = bundle::poster_score(&img.pixels, img.width as usize, img.height as usize);
                            if s > best {
                                best = s;
                                best_t = t;
                            }
                        }
                    }
                    let jpeg = self.frames.frame(&self.doc, best_t, self.doc.project.width, 95)?;
                    std::fs::write(dir.join("poster.jpg"), jpeg).map_err(io)?;
                    man.add(folder, "pictures/poster.jpg", "poster", &format!("frame at {:.1} s", secs(best_t))).map_err(io)?;
                    notes.push(json!({"poster_at": r1(secs(best_t))}));
                }
            }
        }
        // Cutdowns.
        let targets: Vec<f64> = match a.get("cutdowns") {
            Some(Value::Array(v)) => v.iter().filter_map(Value::as_f64).collect(),
            _ => Vec::new(),
        };
        if !targets.is_empty() {
            let dir = folder.join("cutdowns");
            std::fs::create_dir_all(&dir).map_err(io)?;
            let total = secs(dur);
            for t in targets {
                if t >= total - 0.5 {
                    man.missing.push(format!("cutdown {t:.0} s: the programme is only {total:.1} s"));
                    continue;
                }
                let mut ca = json!({"seconds": t, "render": true, "out": dir.display().to_string(), "name": format!("{}_{}s", sanitize(name), t as u32)});
                if let Some(d) = st(a, "spec") {
                    ca["spec"] = json!(d);
                }
                if let Some(sz) = a.get("size") {
                    ca["size"] = sz.clone();
                }
                match self.cutdown_tool(&ca) {
                    Ok(r) => {
                        if let Some(p) = r["file"].as_str() {
                            if let Ok(rel) = Path::new(p).strip_prefix(folder) {
                                man.add(folder, &rel.to_string_lossy(), "cutdown", &format!("{t:.0} s version")).map_err(io)?;
                            }
                        }
                        if flag(a, "verticals") == Some(true) {
                            let mut v = ca.clone();
                            v["vertical"] = json!(true);
                            v["name"] = json!(format!("{}_{}s_vertical", sanitize(name), t as u32));
                            match self.cutdown_tool(&v) {
                                Ok(r) => {
                                    if let Some(p) = r["file"].as_str() {
                                        if let Ok(rel) = Path::new(p).strip_prefix(folder) {
                                            man.add(folder, &rel.to_string_lossy(), "cutdown", &format!("{t:.0} s vertical 9:16")).map_err(io)?;
                                        }
                                    }
                                }
                                Err(e) => man.missing.push(format!("vertical {t:.0} s: {e}")),
                            }
                        }
                    }
                    Err(e) => man.missing.push(format!("cutdown {t:.0} s: {e}")),
                }
            }
        }
        Ok(())
    }

    // -------------------------------------------------------------- cutdown

    fn cutdown_tool(&mut self, a: &Value) -> Result<Value, String> {
        let target = f(a, "seconds").ok_or("cutdown needs \"seconds\": the length to cut to (15, 30, 60...)")?;
        if target < 2.0 {
            return Err("a cutdown needs at least 2 seconds".into());
        }
        let root = self.project_root()?;
        let vertical = flag(a, "vertical") == Some(true);
        let base = st(a, "name").map(str::to_owned).unwrap_or_else(|| format!("{} {}s{}", self.doc.project.name, target as u32, if vertical { " vertical" } else { "" }));
        let p = self.doc.project.clone();
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
        let spans = run::speech_spans(&self.doc, 0, dur);
        let named = |k: &str| -> Vec<u32> { a.get(k).and_then(Value::as_array).map(|v| v.iter().filter_map(Value::as_u64).map(|x| x as u32).collect()).unwrap_or_default() };
        let (keep_ids, drop_ids) = (named("keep"), named("drop"));
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
        let mut cp = Session::new(Doc::new(self.doc.model.clone()));
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
        let out_root = root.join("cutdowns");
        std::fs::create_dir_all(&out_root).map_err(|e| e.to_string())?;
        let dir = free_dir(&out_root, &sanitize(&base));
        let seq = cp.doc.seq();
        cp.doc.apply("Name", m::Edit::RenameSequence { id: seq, name: base.clone() });
        proj::save(&mut cp.doc, Some(&dir))?;
        let final_secs = secs(cp.doc.project.duration());
        let mut reply = json!({
            "project": dir.display().to_string(), "name": base, "seconds": r1(final_secs), "asked": target, "planned_seconds": r1(kept_secs),
            "kept": shots.iter().zip(&plan).filter(|(_, (_, a))| *a != Action::Drop).map(|(s, _)| s.id).collect::<Vec<_>>(),
            "dropped": dropped, "shortened": trimmed.iter().map(|(id, d)| json!({"clip": id, "to": r1(*d)})).collect::<Vec<_>>(),
            "vertical": vertical, "reframed_clips": converted,
            "next": "open it, look (contact_sheet), adjust with keep / drop and run again if a shot you need is gone",
        });
        if flag(a, "render") == Some(true) {
            let mut j = JobSpec::new(&base, &dir);
            j.qc = flag(a, "qc").unwrap_or(true);
            if let Some(d) = st(a, "spec") {
                j.delivery = specs::find(d).map(|s| s.id.to_owned()).unwrap_or_default();
            }
            if j.delivery.is_empty() {
                j.preset = if vertical { "Vertical".into() } else { "Social 1080p".into() };
            }
            j.size = size_arg(a).map(|(w, h)| if vertical { (w.min(h), w.max(h)) } else { (w, h) });
            let file_dir = st(a, "out").map(PathBuf::from).unwrap_or_else(|| dir.join("exports"));
            std::fs::create_dir_all(&file_dir).map_err(|e| e.to_string())?;
            j.out = file_dir.join(format!("{}.mp4", sanitize(&base)));
            let o = run::run_spec(&j, &mut |_| {})?;
            reply["file"] = json!(j.out.display().to_string());
            if let Some((v, _)) = o.qc {
                reply["qc"] = json!(v);
            }
        }
        Ok(reply)
    }
}

fn sanitize(s: &str) -> String {
    let t: String = s.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' { c } else { '_' }).collect();
    let t = t.trim().to_string();
    if t.is_empty() { "Untitled".into() } else { t }
}

fn free_dir(parent: &Path, stem: &str) -> PathBuf {
    (1..).map(|n| if n == 1 { parent.join(stem) } else { parent.join(format!("{stem} {n}")) }).find(|p| !p.exists()).expect("a free name")
}

/// The queue's `bundle` flag: everything but the master, around a master the job made.
pub(crate) fn bundle_for_job(doc: &Doc, spec: &JobSpec, master: &Path) -> Result<PathBuf, String> {
    let mut s = Session::new(Doc::new(doc.model.clone()));
    s.home = Some(spec.project.clone());
    let folder = master.parent().unwrap_or(Path::new(".")).join(format!("{} bundle", sanitize(&spec.name)));
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let mut man = Manifest::new(&spec.name);
    let dir = folder.join("master");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let target = dir.join(master.file_name().unwrap_or_default());
    std::fs::copy(master, &target).map_err(|e| e.to_string())?;
    man.add(&folder, &format!("master/{}", target.file_name().unwrap().to_string_lossy()), "master", "the picture and mix").map_err(|e| e.to_string())?;
    let mut notes = Vec::new();
    s.bundle_rest(&json!({}), &spec.project, &folder, &spec.name, &mut man, &mut notes)?;
    man.write(&folder).map_err(|e| e.to_string())?;
    Ok(folder)
}
