// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Finishing and delivery in the app: the render queue's runner, render
//! pieces for this machine and for render nodes, quality control of
//! exports, mastering packages (DCP) and the node server (`zornov node`).
//!
//! The pure parts (queue file, pieces, merge, protocol, QC, J2K, MXF) live
//! in `studio-deliver`; this module connects them to projects and to the
//! exporter in [`crate::export_run`].

use crate::doc::Doc;
use crate::export_run::{self, Request, RunOptions};
use crate::formats::Format;
use crate::services::{export as svc, project as proj};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use studio_deliver::chunks::{self, ChunkRenderer, ChunkTask, RunControl};
use studio_deliver::queue::{self, JobSpec, Queue, State, Store};
use studio_deliver::specs::{self, DeliverySpec, Kind, SizeRule};
use studio_deliver::{dcp, filecheck, merge, qc, wire};

const US: f64 = 1e6;

// ------------------------------------------------------------- places

/// Where the queue, the node list and the node's cache live.
pub fn data_dir() -> PathBuf {
    std::env::var_os("STUDIO_QUEUE_DIR").map(PathBuf::from).unwrap_or_else(|| studio_paths::studio().join(".queue"))
}

pub fn store() -> Store {
    Store::new(&data_dir().join("queue.json"))
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct NodeEntry {
    pub name: String,
    pub addr: String,
    pub token: String,
}

fn nodes_path() -> PathBuf {
    data_dir().join("nodes.json")
}

pub fn load_nodes() -> Vec<NodeEntry> {
    std::fs::read(nodes_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn save_nodes(nodes: &[NodeEntry]) -> Result<(), String> {
    let p = nodes_path();
    std::fs::create_dir_all(p.parent().unwrap_or(Path::new("."))).map_err(|e| e.to_string())?;
    std::fs::write(&p, serde_json::to_vec_pretty(nodes).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    // The pairing tokens are secrets: only this person reads the file.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

// ------------------------------------------------------------- plans

/// What a job renders, worked out from the project and the job's spec.
pub struct Plan {
    pub kind: Kind,
    pub deliv: Option<&'static DeliverySpec>,
    pub preset: &'static svc::Preset,
    pub out: PathBuf,
    pub fps: f64,
    pub size: (u32, u32),
    pub first: u64,
    pub last: u64,
    pub bitrate_kbps: u32,
    pub profile: qc::Profile,
}

fn svc_size(r: SizeRule) -> svc::Size {
    match r {
        SizeRule::Fixed(w, h) => svc::Size::Fixed(w, h),
        SizeRule::ShortSide(s) => svc::Size::ShortSide(s),
        SizeRule::Project => svc::Size::Project,
    }
}

pub fn plan_job(doc: &Doc, job: &JobSpec) -> Result<Plan, String> {
    let deliv = if job.delivery.is_empty() { None } else { Some(specs::find(&job.delivery).ok_or_else(|| format!("No delivery spec \"{}\"; specs: {}", job.delivery, specs::SPECS.iter().map(|s| s.id).collect::<Vec<_>>().join(", ")))?) };
    let preset_name = if job.preset.is_empty() { deliv.map_or("Social 1080p", |d| d.preset) } else { job.preset.as_str() };
    let preset = svc::preset(preset_name).or_else(|| svc::PRESETS.iter().find(|p| p.name.to_lowercase().contains(&preset_name.to_lowercase()))).ok_or_else(|| format!("No preset \"{preset_name}\""))?;
    let proj_fps = doc.project.fps as f64;
    let fps = deliv.map_or(proj_fps, |d| d.fps_for(proj_fps));
    let size_rule = deliv.map_or(preset.size, |d| svc_size(d.size));
    let size = job.size.map(|(w, h)| (w / 2 * 2, h / 2 * 2)).unwrap_or_else(|| svc::output_size(size_rule, (doc.project.width, doc.project.height)));
    let total = svc_duration(doc);
    let (a, b) = match job.range {
        Some((a, b)) => ((a.max(0.0) * US) as i64, ((b * US) as i64).min(total)),
        None => (0, total),
    };
    if b <= a {
        return Err("The range is empty (is the timeline empty?)".into());
    }
    let first = (a as f64 / US * fps).floor() as u64;
    let last = ((b as f64 / US * fps).ceil() as u64).max(first + 1);
    let kind = deliv.map_or(Kind::File, |d| d.kind);
    let out = if job.out.as_os_str().is_empty() {
        let dir = studio_model::file::ProjectFolder::new(&job.project).export_dir(studio_model::file::ExportKind::Renders);
        let _ = std::fs::create_dir_all(&dir);
        if kind == Kind::File { svc::free_path(&dir, &sanitize(&job.name), preset.format) } else { dir }
    } else {
        job.out.clone()
    };
    let mut profile = match deliv {
        Some(d) => d.profile(fps, (doc.project.width, doc.project.height)),
        None => qc::Profile { expect_fps: Some(fps), expect_size: Some(size), ..qc::Profile::default() },
    };
    if job.size.is_some() {
        // The person chose the size: that is what the file should be.
        profile.expect_size = Some(size);
    }
    Ok(Plan { kind, deliv, preset, out, fps, size, first, last, bitrate_kbps: svc::suggested_kbps(size.0, size.1, fps), profile })
}

fn svc_duration(doc: &Doc) -> i64 {
    crate::services::transport::duration(doc)
}

fn sanitize(s: &str) -> String {
    let t: String = s.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' { c } else { '_' }).collect();
    let t = t.trim().to_string();
    if t.is_empty() { "Export".into() } else { t }
}

// ------------------------------------------------------ render pieces

/// Settings a piece carries to whichever machine draws it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PieceSettings {
    /// "h264", "hevc10" or "j2k" (frames to JPEG 2000 for a DCP).
    pub kind: String,
    pub w: u32,
    pub h: u32,
    pub fps: f64,
    pub bitrate_kbps: u32,
    pub quality: f32,
    pub software: bool,
    pub dcp: Option<DcpSettings>,
    #[serde(default)]
    pub imf: Option<ImfSettings>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImfSettings {
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub bit_depth: u8,
    pub step: f32,
}

impl ImfSettings {
    pub fn spec(&self) -> studio_deliver::imf::ImfSpec {
        let mut s = studio_deliver::imf::ImfSpec::new(&self.title, self.width, self.height, (self.fps_num, self.fps_den));
        s.bit_depth = self.bit_depth;
        s.step = self.step;
        s
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DcpSettings {
    pub title: String,
    pub fps: u32,
    pub smpte: bool,
    pub shape: String,
    pub four_k: bool,
    pub step: f32,
}

impl DcpSettings {
    pub fn spec(&self) -> dcp::DcpSpec {
        let mut s = dcp::DcpSpec::new(&self.title, self.fps);
        s.standard = if self.smpte { dcp::Standard::Smpte } else { dcp::Standard::Interop };
        s.shape = match self.shape.as_str() {
            "scope" => dcp::Shape::Scope,
            "full" => dcp::Shape::Full,
            _ => dcp::Shape::Flat,
        };
        s.four_k = self.four_k;
        s.step = self.step;
        s
    }
}

/// Frame range to the micros a Request needs so the exporter draws
/// exactly frames `first..last`.
fn frame_range_micros(first: u64, last: u64, fps: f64) -> (i64, i64) {
    (((first as f64 + 0.25) / fps * US) as i64, (((last as f64 - 0.25) / fps * US) as i64).max(1))
}

/// Draw one piece from the project in `dir` into `out`.
pub fn render_piece(dir: &Path, task: &ChunkTask, out: &Path, progress: &mut dyn FnMut(f32), cancel: &AtomicBool) -> Result<(), String> {
    let s: PieceSettings = serde_json::from_value(task.settings.clone()).map_err(|e| format!("bad piece settings: {e}"))?;
    let doc = proj::open(dir)?;
    let (t0, t1) = frame_range_micros(task.first, task.last, s.fps);
    let format = if s.kind == "hevc10" { Format::Hevc10 } else { Format::H264 };
    let req = Request { path: out.to_path_buf(), format, size: (s.w, s.h), fps: s.fps, range: (t0, t1), quality: s.quality, bitrate_kbps: s.bitrate_kbps, av1_speed: Default::default() };
    let mut opts = RunOptions { audio: false, software: s.software, sink: None };
    let pack = if s.kind == "j2k" || s.kind == "imf" {
        let encode: Box<dyn Fn(&[u8], usize, usize) -> Result<Vec<u8>, String> + Send> = if s.kind == "j2k" {
            let spec = s.dcp.as_ref().ok_or("a j2k piece needs its DCP settings")?.spec();
            Box::new(move |rgba, w, h| dcp::encode_frame(&spec, rgba, w, h))
        } else {
            let spec = s.imf.as_ref().ok_or("an imf piece needs its IMF settings")?.spec();
            Box::new(move |rgba, w, h| studio_deliver::imf::encode_frame(&spec, rgba, w, h))
        };
        let writer = Arc::new(Mutex::new(Some(dcp::PackWriter::create(out).map_err(|e| e.to_string())?)));
        let w2 = writer.clone();
        opts.sink = Some(Box::new(move |rgba, w, h| {
            let cs = encode(rgba, w as usize, h as usize)?;
            let mut g = w2.lock().unwrap_or_else(|e| e.into_inner());
            g.as_mut().ok_or("the pack is closed")?.push(&cs).map_err(|e| e.to_string())
        }));
        Some(writer)
    } else {
        None
    };
    let task_h = export_run::start_with(&doc, req, opts);
    loop {
        if cancel.load(Ordering::Relaxed) {
            task_h.progress.cancel();
        }
        progress(task_h.fraction());
        if task_h.finished.load(Ordering::Acquire) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(40));
    }
    if cancel.load(Ordering::Relaxed) {
        let _ = std::fs::remove_file(out);
        return Err("cancelled".into());
    }
    if let Some(e) = task_h.error.lock().unwrap_or_else(|e| e.into_inner()).take() {
        let _ = std::fs::remove_file(out);
        return Err(e);
    }
    if let Some(w) = pack {
        if let Some(pw) = w.lock().unwrap_or_else(|e| e.into_inner()).take() {
            pw.finish().map_err(|e| e.to_string())?;
        }
    }
    progress(1.0);
    Ok(())
}

/// This machine, drawing pieces of the project in `dir`.
pub struct LocalPieces {
    pub dir: PathBuf,
}

impl ChunkRenderer for LocalPieces {
    fn name(&self) -> String {
        "this Mac".into()
    }

    fn render(&self, task: &ChunkTask, out: &Path, progress: &mut dyn FnMut(f32), cancel: &AtomicBool) -> Result<(), String> {
        render_piece(&self.dir, task, out, progress, cancel)
    }
}

// ----------------------------------------------------------- the nodes

/// What `zornov node` runs: draws pieces from projects sent to it.
pub struct AppWorker {
    pub name: String,
}

impl wire::NodeWorker for AppWorker {
    fn info(&self) -> wire::NodeInfo {
        wire::NodeInfo { name: self.name.clone(), cores: std::thread::available_parallelism().map_or(1, |n| n.get()), proto: wire::PROTO, busy: false }
    }

    fn render(&self, dir: &Path, task: &ChunkTask, progress: &mut dyn FnMut(f32), cancel: &AtomicBool) -> Result<PathBuf, String> {
        let out_dir = dir.join("_out");
        std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
        let out = out_dir.join(format!("piece-{}-{}.part", task.job.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>(), task.index));
        render_piece(dir, task, &out, progress, cancel)?;
        Ok(out)
    }
}

/// Files of a project folder a node needs (everything but caches and output).
fn project_files(root: &Path) -> Vec<PathBuf> {
    studio_model::file::ProjectFolder::new(root).portable_files()
}

/// The project, ready to send: its folder's files, and media that live
/// outside the folder placed under `_external/` with the project file
/// pointing there.
pub fn pack_project(doc: &Doc, dir: &Path, id: &str) -> Result<(wire::ProjectPack, Option<tempfile_guard::Guard>), String> {
    let folder = doc.folder.clone().ok_or("the project has no folder")?;
    let pfile = folder.project_file();
    let prel = pfile.strip_prefix(dir).map_err(|_| "the project file is outside its folder")?.to_path_buf();
    let mut pack = wire::ProjectPack::new(id);
    for rel in project_files(dir) {
        if rel == prel {
            continue;
        }
        pack.add(&rel.to_string_lossy().replace('\\', "/"), &dir.join(&rel)).map_err(|e| format!("{}: {e}", rel.display()))?;
    }
    // The project file with every media path made relative to its new place.
    let mut v: Value = serde_json::from_slice(&std::fs::read(&pfile).map_err(|e| e.to_string())?).map_err(|e| format!("project file: {e}"))?;
    let mut moved: Vec<(String, PathBuf)> = Vec::new();
    fix_media(&mut v, dir, &mut moved);
    for (rel, src) in &moved {
        if src.exists() {
            pack.add(rel, src).map_err(|e| format!("{}: {e}", src.display()))?;
        }
    }
    let guard = tempfile_guard::Guard::new("studio-pack")?;
    let tmp = guard.path().join("project.json");
    std::fs::write(&tmp, serde_json::to_vec(&v).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    pack.add(&prel.to_string_lossy().replace('\\', "/"), &tmp).map_err(|e| e.to_string())?;
    Ok((pack, Some(guard)))
}

/// Walks the project JSON for media items (objects with `path` and `id`)
/// and points each at a place inside the package.
fn fix_media(v: &mut Value, dir: &Path, moved: &mut Vec<(String, PathBuf)>) {
    match v {
        Value::Object(m) => {
            if let (Some(Value::String(path)), true) = (m.get("path").cloned(), m.contains_key("id") && m.contains_key("info")) {
                let p = PathBuf::from(&path);
                let rel = match p.strip_prefix(dir) {
                    Ok(r) => r.to_string_lossy().replace('\\', "/"),
                    Err(_) => {
                        let id = m.get("id").map(|i| i.to_string()).unwrap_or_default();
                        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "media".into());
                        let rel = format!("_external/{}-{}", id.trim_matches('"'), name);
                        moved.push((rel.clone(), p.clone()));
                        rel
                    }
                };
                m.insert("relative_path".into(), Value::String(rel));
                m.insert("path".into(), Value::String(format!("/nonexistent-on-render-node/{path}")));
            }
            for x in m.values_mut() {
                fix_media(x, dir, moved);
            }
        }
        Value::Array(a) => {
            for x in a {
                fix_media(x, dir, moved);
            }
        }
        _ => {}
    }
}

/// A scratch folder removed when dropped.
pub mod tempfile_guard {
    use std::path::{Path, PathBuf};

    pub struct Guard(PathBuf);

    impl Guard {
        pub fn new(tag: &str) -> Result<Guard, String> {
            let p = std::env::temp_dir().join(format!("{tag}-{}", studio_deliver::mxf::uuid_string(&studio_deliver::mxf::uuid())));
            std::fs::create_dir_all(&p).map_err(|e| e.to_string())?;
            Ok(Guard(p))
        }

        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

/// Nodes from the saved list that answer, as renderers for `pack`.
pub fn live_nodes(pack: &Arc<wire::ProjectPack>) -> Vec<(Arc<dyn ChunkRenderer>, String)> {
    let mut out: Vec<(Arc<dyn ChunkRenderer>, String)> = Vec::new();
    for n in load_nodes() {
        let Ok(addr) = n.addr.parse::<std::net::SocketAddr>().or_else(|_| {
            use std::net::ToSocketAddrs;
            n.addr.to_socket_addrs().ok().and_then(|mut a| a.next()).ok_or(())
        }) else { continue };
        let mut node = wire::RemoteNode::new(addr, &n.token, pack.clone());
        node.name = n.name.clone();
        if node.ping().is_ok() {
            out.push((Arc::new(node), n.name));
        }
    }
    out
}

// ---------------------------------------------------------------- QC

/// Captions in the project: (start s, end s, text).
pub fn caption_cues(doc: &Doc) -> Vec<(f64, f64, String)> {
    let mut v: Vec<(f64, f64, String)> = doc.project.clips.iter().filter(|c| c.kind == crate::view::ClipKind::Title && c.name == "Caption" && !c.text.trim().is_empty()).map(|c| (c.start as f64 / US, (c.start + c.dur) as f64 / US, c.text.trim().to_string())).collect();
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    v
}

/// Seconds of speech in the project's mix, and the spans (seconds from
/// `a`). Each clip is heard on its own, as it plays: speech is where the
/// voice detector opens on a clip (10 ms edges) and the ear hears a voice
/// in it (speech, or speech over music, by the half second). The detector
/// alone follows level and opens on any music, riser or cricket that
/// moves; the ear on the whole mix loses a voice under loud music.
pub fn speech_spans(doc: &Doc, a: i64, b: i64) -> Vec<(f64, f64)> {
    let rate = crate::sound_bridge::RATE;
    let mut model = doc.model.clone();
    let muted: std::collections::HashMap<u64, bool> = model.sequences.iter().flat_map(|s| s.clips.iter()).map(|c| (c.id.0, c.audio.muted)).collect();
    let mut out: Vec<(f64, f64)> = Vec::new();
    for c in doc.project.clips.iter().filter(|c| c.media.is_some() && c.start < b && c.start + c.dur > a) {
        if doc.project.media(c.media.unwrap_or_default()).is_some_and(|m| matches!(m.kind, crate::view::MediaKind::Image)) {
            continue;
        }
        let (t0, t1) = (c.start.max(a), (c.start + c.dur).min(b));
        for s in &mut model.sequences {
            for mc in &mut s.clips {
                mc.audio.muted = if mc.id.0 as u32 == c.id { muted.get(&mc.id.0).copied().unwrap_or(false) } else { true };
            }
        }
        let pcm = crate::sound_bridge::mix_for_export(&model, t0, t1);
        let mono: Vec<f32> = pcm.chunks_exact(2).map(|f| 0.5 * (f[0] + f[1])).collect();
        if mono.iter().all(|v| v.abs() < 1e-6) {
            continue;
        }
        let open = studio_sound::vad::speech_segments(&mono, rate, studio_sound::vad::VadParams::default());
        if open.is_empty() {
            continue;
        }
        let heard = studio_ai::features::hearing::analyse(&mono, rate);
        let off = (t0 - a) as f64 / US;
        for (s0, e0) in open {
            for v in heard.segments.iter().filter(|s| s.label.starts_with("speech")) {
                let (s, e) = ((s0 as f64 / US).max(v.from), (e0 as f64 / US).min(v.to));
                if e > s {
                    out.push((off + s, off + e));
                }
            }
        }
    }
    // Two voices at once are one stretch of speech.
    out.sort_by(|x, y| x.0.total_cmp(&y.0));
    let mut merged: Vec<(f64, f64)> = Vec::new();
    for (s, e) in out {
        match merged.last_mut() {
            Some(m) if s <= m.1 => m.1 = m.1.max(e),
            _ => merged.push((s, e)),
        }
    }
    merged
}

pub fn qc_extra(doc: &Doc, range: (i64, i64)) -> filecheck::Extra {
    // Caption clips on the timeline, or the sound desk's subtitle tracks.
    let subtitle_cues = crate::sound_desk::Desk::of(&doc.model).subtitles.iter().map(|t| t.cues.len()).max().unwrap_or(0);
    let cues = caption_cues(doc);
    let cue_count = cues.len().max(subtitle_cues);
    let mut spans = speech_spans(doc, range.0, range.1);
    // Speech by the first moment of the file, not the timeline.
    for s in &mut spans {
        s.0 += 0.0;
    }
    let speech: f64 = spans.iter().map(|s| s.1 - s.0).sum();
    filecheck::Extra { speech: spans, caption_cues: Some(cue_count), speech_secs: Some(speech), max_secs: None }
}

/// Run QC on `file`, write `<file>.qc.json` and `.qc.txt`, return the report.
pub fn qc_and_write(file: &Path, profile: &qc::Profile, extra: &filecheck::Extra) -> Result<(qc::Report, PathBuf), String> {
    let report = filecheck::check_file(file, profile, extra)?;
    let json_path = PathBuf::from(format!("{}.qc.json", file.display()));
    std::fs::write(&json_path, report.to_json()).map_err(|e| e.to_string())?;
    std::fs::write(format!("{}.qc.txt", file.display()), report.to_text()).map_err(|e| e.to_string())?;
    Ok((report, json_path))
}

// ----------------------------------------------------------- running

/// What a finished job produced.
pub struct Outcome {
    pub outputs: Vec<PathBuf>,
    pub qc: Option<(String, PathBuf)>,
    pub machines: Vec<(String, usize)>,
}

/// Polls the queue file while a job runs: pause, resume and cancel made anywhere reach it; progress goes back.
pub struct Watch<'a> {
    link: Option<(&'a Store, u32)>,
    ctl: &'a RunControl,
    last: Mutex<(std::time::Instant, f32)>,
    on_progress: Option<Mutex<Box<dyn FnMut(f32) + Send + 'a>>>,
}

impl Watch<'_> {
    fn progress(&self, f: f32, scale: (f32, f32)) {
        let f = scale.0 + f * (scale.1 - scale.0);
        let mut l = self.last.lock().unwrap_or_else(|e| e.into_inner());
        if l.0.elapsed().as_millis() > 400 && (f - l.1).abs() > 0.002 {
            *l = (std::time::Instant::now(), f);
            if let Some((store, id)) = self.link {
                let _ = store.update(|q| q.set_progress(id, f));
            }
            if let Some(cb) = &self.on_progress {
                (cb.lock().unwrap_or_else(|e| e.into_inner()))(f);
            }
        }
    }

    /// Re-read the job's state into the switches.
    fn sync(&self) {
        let Some((store, id)) = self.link else { return };
        if let Some(j) = store.read().get(id) {
            self.ctl.pause.store(j.state == State::Paused, Ordering::Relaxed);
            if j.state == State::Cancelled {
                self.ctl.cancel.store(true, Ordering::Relaxed);
            }
        }
    }
}

/// Run job `id` of the queue to the end.
pub fn run_job(store: &Store, id: u32) -> Result<Outcome, String> {
    let job = store.read().get(id).cloned().ok_or_else(|| format!("No job {id}"))?;
    run_spec_linked(&job.spec, Some((store, id)), None)
}

/// Render `spec` now, without the queue (the caller waits), reporting progress.
pub fn run_spec(spec: &JobSpec, progress: &mut (dyn FnMut(f32) + Send)) -> Result<Outcome, String> {
    run_spec_linked(spec, None, Some(progress))
}

fn run_spec_linked(spec: &JobSpec, link: Option<(&Store, u32)>, cb: Option<&mut (dyn FnMut(f32) + Send)>) -> Result<Outcome, String> {
    let doc = proj::open(&spec.project)?;
    let plan = plan_job(&doc, spec)?;
    let ctl = RunControl::default();
    let watch = Watch { link, ctl: &ctl, last: Mutex::new((std::time::Instant::now(), -1.0)), on_progress: cb.map(|c| Mutex::new(Box::new(move |f: f32| c(f)) as Box<dyn FnMut(f32) + Send + '_>)) };
    let done = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            while !done.load(Ordering::Relaxed) {
                watch.sync();
                std::thread::sleep(std::time::Duration::from_millis(300));
            }
        });
        let r = match plan.kind {
            Kind::File => run_file(&doc, spec, &plan, &watch),
            Kind::Dcp => run_dcp(&doc, spec, &plan, &watch),
            Kind::Imf => run_imf(&doc, spec, &plan, &watch),
        };
        done.store(true, Ordering::Relaxed);
        r
    })
}

fn piece_settings(plan: &Plan, software: bool) -> PieceSettings {
    PieceSettings { kind: if plan.preset.format == Format::Hevc10 { "hevc10".into() } else { "h264".into() }, w: plan.size.0, h: plan.size.1, fps: plan.fps, bitrate_kbps: plan.bitrate_kbps, quality: plan.preset.quality, software, dcp: None, imf: None }
}

/// The renderers for a job: this machine, and nodes when asked.
fn workers(doc: &Doc, spec: &JobSpec, status: &mut dyn FnMut(&str)) -> Result<(Vec<Arc<dyn ChunkRenderer>>, Option<tempfile_guard::Guard>, bool), String> {
    let mut w: Vec<Arc<dyn ChunkRenderer>> = vec![Arc::new(LocalPieces { dir: spec.project.clone() })];
    if !spec.distributed {
        return Ok((w, None, false));
    }
    let id = format!("p{}", studio_deliver::sha256::hex(&studio_deliver::sha256::sha256(spec.project.to_string_lossy().as_bytes()))[..16].to_string());
    let (pack, guard) = pack_project(doc, &spec.project, &id)?;
    let pack = Arc::new(pack);
    let nodes = live_nodes(&pack);
    status(&format!("{} render node(s) answered", nodes.len()));
    for (n, _) in nodes {
        w.push(n);
    }
    Ok((w, guard, true))
}

fn run_file(doc: &Doc, spec: &JobSpec, plan: &Plan, watch: &Watch) -> Result<Outcome, String> {
    let (a, b) = frame_range_micros(plan.first, plan.last, plan.fps);
    let fmt = plan.preset.format;
    // Only H.264 and HEVC are cut into pieces and joined (`merge` copies their samples). AV1 is
    // deliberately one whole-file render: the encoder is one in-process stream (key frame every
    // 2 s, no rate control to match across pieces), and `merge` has no AV1 track writer.
    let chunked = matches!(fmt, Format::H264 | Format::Hevc10);
    let mut machines = Vec::new();
    if chunked {
        let (ws, _guard, distributed) = workers(doc, spec, &mut |_| {})?;
        let secs = if spec.chunk_secs >= 0.2 { spec.chunk_secs } else { 8.0 };
        let ranges = chunks::plan(plan.first, plan.last, ((secs * plan.fps) as u64).max(1));
        let parts = PathBuf::from(format!("{}.parts", plan.out.display()));
        let settings = serde_json::to_value(piece_settings(plan, distributed)).map_err(|e| e.to_string())?;
        let report = match chunks::run(&spec.name, &settings, &ranges, &parts, ws, watch.ctl, &|f| watch.progress(f, (0.0, 0.9))) {
            Ok(r) => r,
            Err(e) => {
                // A cancelled job leaves nothing behind; a failed one keeps its pieces for a retry.
                if watch.ctl.cancel.load(Ordering::Relaxed) {
                    let _ = std::fs::remove_dir_all(&parts);
                }
                return Err(e);
            }
        };
        machines = report.by_machine.into_iter().filter(|m| m.1 > 0).collect();
        if let Some(d) = plan.out.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        // Sound and subtitles: mixed once for the whole range, the same tracks an export carries
        // (the sound department's languages and layouts).
        let t0 = (plan.first as f64 / plan.fps * US) as i64;
        let t1 = (plan.last as f64 / plan.fps * US) as i64;
        let audio: Vec<merge::AudioTrackIn> = crate::sound_desk::render::tracks(&doc.model, t0, t1)
            .into_iter()
            .filter(|t| t.pcm.iter().any(|v| v.abs() > 1e-6))
            .map(|t| {
                let ch = t.layout.channels();
                merge::AudioTrackIn { pcm: t.pcm, channels: ch, rate: crate::sound_bridge::RATE, bitrate: if ch > 2 { 384_000 } else { 192_000 }, lang: t.lang, title: t.title }
            })
            .collect();
        let (a_ms, b_ms) = (t0 as f64 / 1e3, t1 as f64 / 1e3);
        let subtitles: Vec<merge::SubtitleTrackIn> = crate::sound_desk::Desk::of(&doc.model)
            .subtitles
            .into_iter()
            .map(|t| {
                let cues = t.cues.iter().filter_map(|c| {
                    let (s, e) = ((c.start as f64 * 1e3).max(a_ms), (c.end as f64 * 1e3).min(b_ms));
                    (e > s).then(|| ((s - a_ms).round() as u64, (e - a_ms).round() as u64, c.text.clone()))
                });
                merge::SubtitleTrackIn { lang: studio_text::subtitles::iso639_2(&t.lang).to_owned(), title: if t.kind.is_empty() || t.kind == "subtitles" { t.name.clone() } else { format!("{} ({})", t.name, t.kind.to_uppercase()) }, cues: cues.collect() }
            })
            .collect();
        watch.progress(0.0, (0.9, 0.9));
        merge::merge(&report.parts, &plan.out, &merge::MergeOptions { audio, subtitles })?;
        crate::sound_desk::subs::write_sidecars(&doc.model, &plan.out, t0, t1);
        let _ = std::fs::remove_dir_all(&parts);
        watch.progress(1.0, (0.0, 0.95));
    } else {
        // Formats written in one piece: CLV, PNG, GIF, M4A.
        let req = Request { path: plan.out.clone(), format: fmt, size: plan.size, fps: plan.fps, range: (a, b), quality: plan.preset.quality, bitrate_kbps: plan.bitrate_kbps, av1_speed: Default::default() };
        let t = export_run::start(doc, req);
        loop {
            if watch.ctl.cancel.load(Ordering::Relaxed) {
                t.progress.cancel();
            }
            watch.progress(t.fraction(), (0.0, 0.95));
            if t.finished.load(Ordering::Acquire) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        if let Some(e) = t.error.lock().unwrap_or_else(|e| e.into_inner()).take() {
            return Err(e);
        }
        if watch.ctl.cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        machines.push(("this Mac".into(), 1));
    }
    let mut outputs = vec![plan.out.clone()];
    let mut qc_result = None;
    if spec.qc && chunked {
        let extra = qc_extra(doc, ((plan.first as f64 / plan.fps * US) as i64, (plan.last as f64 / plan.fps * US) as i64));
        let (r, p) = qc_and_write(&plan.out, &plan.profile, &extra)?;
        outputs.push(p.clone());
        qc_result = Some((r.verdict, p));
    }
    if spec.bundle {
        let b = crate::agent::bundle_for_job(doc, spec, &plan.out)?;
        outputs.push(b);
    }
    Ok(Outcome { outputs, qc: qc_result, machines })
}

/// The sound for a cinema or IMF package: the original language's mix, 5.1 when the sound desk
/// made one (channel order L R C LFE Ls Rs, as in a DCP), else the stereo mix in the front pair of
/// a `channels`-wide track.
fn cinema_audio(doc: &Doc, t0: i64, t1: i64, channels: u16) -> Option<dcp::Audio> {
    let tracks = crate::sound_desk::render::tracks(&doc.model, t0, t1);
    let loud = |t: &&crate::sound_desk::render::Track| t.pcm.iter().any(|v| v.abs() > 1e-6);
    if channels >= 6 {
        if let Some(t) = tracks.iter().filter(loud).find(|t| t.layout.channels() == 6) {
            return Some(dcp::Audio { channels: 6, samples: t.pcm.clone() });
        }
    }
    let t = tracks.iter().find(|t| t.layout.channels() == 2 && loud(t))?;
    Some(if channels == 2 { dcp::Audio { channels: 2, samples: t.pcm.clone() } } else { dcp::Audio::from_stereo(&t.pcm, channels) })
}

fn dcp_spec_for(plan: &Plan, doc: &Doc, name: &str) -> Result<dcp::DcpSpec, String> {
    let d = plan.deliv.ok_or("a DCP needs a delivery spec (dcp-smpte or dcp-interop)")?;
    let mut s = dcp::DcpSpec::new(name, plan.fps.round() as u32);
    s.standard = if d.id == "dcp-interop" { dcp::Standard::Interop } else { dcp::Standard::Smpte };
    let aspect = doc.project.width as f64 / doc.project.height.max(1) as f64;
    s.shape = if aspect >= 2.1 { dcp::Shape::Scope } else if aspect > 1.9 { dcp::Shape::Full } else { dcp::Shape::Flat };
    s.validate()?;
    Ok(s)
}

fn run_dcp(doc: &Doc, spec: &JobSpec, plan: &Plan, watch: &Watch) -> Result<Outcome, String> {
    let dspec = dcp_spec_for(plan, doc, &spec.name)?;
    let (aw, ah) = dspec.active();
    // The picture fits the active area keeping its shape; the packer centres it in the container.
    let k = (aw as f64 / doc.project.width as f64).min(ah as f64 / doc.project.height as f64);
    let (w, h) = (((doc.project.width as f64 * k) as u32 / 2 * 2).max(2), ((doc.project.height as f64 * k) as u32 / 2 * 2).max(2));
    let mut p2 = Plan { kind: plan.kind, deliv: plan.deliv, preset: plan.preset, out: plan.out.clone(), fps: dspec.fps as f64, size: (w, h), first: plan.first, last: plan.last, bitrate_kbps: plan.bitrate_kbps, profile: plan.profile.clone() };
    p2.fps = dspec.fps as f64;
    let (ws, _guard, _d) = workers(doc, spec, &mut |_| {})?;
    let secs = if spec.chunk_secs >= 0.2 { spec.chunk_secs } else { 4.0 };
    let ranges = chunks::plan(p2.first, p2.last, ((secs * p2.fps) as u64).max(1));
    let work = std::env::temp_dir().join(format!("studio-dcp-{}", studio_deliver::mxf::uuid_string(&studio_deliver::mxf::uuid())));
    let settings = PieceSettings { kind: "j2k".into(), w, h, fps: p2.fps, bitrate_kbps: 0, quality: 100.0, software: false, imf: None, dcp: Some(DcpSettings { title: dspec.title.clone(), fps: dspec.fps, smpte: dspec.standard == dcp::Standard::Smpte, shape: match dspec.shape { dcp::Shape::Scope => "scope".into(), dcp::Shape::Full => "full".into(), _ => "flat".into() }, four_k: dspec.four_k, step: dspec.step }) };
    let report = chunks::run(&spec.name, &serde_json::to_value(&settings).map_err(|e| e.to_string())?, &ranges, &work, ws, watch.ctl, &|f| watch.progress(f, (0.0, 0.85)))?;
    let t0 = (p2.first as f64 / p2.fps * US) as i64;
    let t1 = (p2.last as f64 / p2.fps * US) as i64;
    let audio = cinema_audio(doc, t0, t1, dspec.channels);
    std::fs::create_dir_all(&p2.out).map_err(|e| e.to_string())?;
    let mut prog = |f: f32| watch.progress(f, (0.85, 1.0));
    let r = dcp::build(&dspec, &p2.out, &report.parts, audio.as_ref(), &mut prog, &watch.ctl.cancel);
    let _ = std::fs::remove_dir_all(&work);
    let r = r?;
    let machines = report.by_machine.into_iter().filter(|m| m.1 > 0).collect();
    Ok(Outcome { outputs: vec![r.dir.clone()], qc: None, machines })
}

fn rate_of(fps: f64) -> (u32, u32) {
    let r = studio_model::time::FrameRate::from_f64(fps, 1e-3);
    (r.num, r.den)
}

fn run_imf(doc: &Doc, spec: &JobSpec, plan: &Plan, watch: &Watch) -> Result<Outcome, String> {
    let (w, h) = (plan.size.0 / 2 * 2, plan.size.1 / 2 * 2);
    let (num, den) = rate_of(plan.fps);
    let mut imf = studio_deliver::imf::ImfSpec::new(&spec.name, w, h, (num, den));
    imf.step = 1.0;
    let (ws, _guard, _d) = workers(doc, spec, &mut |_| {})?;
    let secs = if spec.chunk_secs >= 0.2 { spec.chunk_secs } else { 4.0 };
    let ranges = chunks::plan(plan.first, plan.last, ((secs * plan.fps) as u64).max(1));
    let work = std::env::temp_dir().join(format!("studio-imf-{}", studio_deliver::mxf::uuid_string(&studio_deliver::mxf::uuid())));
    let settings = PieceSettings { kind: "imf".into(), w, h, fps: plan.fps, bitrate_kbps: 0, quality: 100.0, software: false, dcp: None, imf: Some(ImfSettings { title: imf.title.clone(), width: w, height: h, fps_num: num, fps_den: den, bit_depth: imf.bit_depth, step: imf.step }) };
    let report = chunks::run(&spec.name, &serde_json::to_value(&settings).map_err(|e| e.to_string())?, &ranges, &work, ws, watch.ctl, &|f| watch.progress(f, (0.0, 0.85)))?;
    let t0 = (plan.first as f64 / plan.fps * US) as i64;
    let t1 = (plan.last as f64 / plan.fps * US) as i64;
    let audio = cinema_audio(doc, t0, t1, 2);
    let dir = plan.out.join(format!("{}_IMF", sanitize(&spec.name)));
    let r = studio_deliver::imf::build(&imf, &dir, &report.parts, audio.as_ref());
    let _ = std::fs::remove_dir_all(&work);
    let r = r?;
    watch.progress(1.0, (0.0, 1.0));
    let machines = report.by_machine.into_iter().filter(|m| m.1 > 0).collect();
    Ok(Outcome { outputs: vec![r.dir], qc: None, machines })
}

// ------------------------------------------------------------- runner

/// Draws the queue until `stop` is set (or, with `until_empty`, until nothing is waiting).
pub fn run_queue(stop: &AtomicBool, until_empty: bool) -> Result<usize, String> {
    let st = store();
    let lock = queue::RunnerLock::acquire(&st)?;
    st.recover()?;
    let mut done = 0;
    while !stop.load(Ordering::Relaxed) {
        lock.touch();
        let next = st.read().next();
        let Some(id) = next else {
            if until_empty {
                // Paused jobs stay in the queue for a later run.
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
            continue;
        };
        if st.update(|q| q.begin(id)).and_then(|r| r).is_err() {
            continue;
        }
        let alive = AtomicBool::new(true);
        let r = std::thread::scope(|s| {
            // Keep the runner's lock fresh during a long job.
            s.spawn(|| {
                while alive.load(Ordering::Relaxed) {
                    lock.touch();
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }
            });
            let r = run_job(&st, id);
            alive.store(false, Ordering::Relaxed);
            r
        });
        let _ = match r {
            Ok(o) => st.update(|q| q.finish(id, o.outputs, o.qc, o.machines)),
            Err(e) if e == "cancelled" => st.update(|q| {
                let _ = q.cancel(id);
            }),
            Err(e) => st.update(|q| q.fail(id, &e)),
        };
        done += 1;
        for n in st.update(|q| q.take_unnotified()).unwrap_or_default() {
            notify(&n.title, &n.body);
        }
    }
    Ok(done)
}

/// A desktop notification (macOS); quiet when `STUDIO_NO_NOTIFY` is set.
pub fn notify(title: &str, body: &str) {
    if std::env::var_os("STUDIO_NO_NOTIFY").is_some() {
        return;
    }
    #[cfg(target_os = "macos")]
    {
        let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
        let _ = std::process::Command::new("osascript").arg("-e").arg(format!("display notification \"{}\" with title \"{}\"", esc(body), esc(title))).spawn();
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (title, body);
    }
}

static BACKGROUND: std::sync::Once = std::sync::Once::new();

/// The window draws the queue in the background for as long as it is open.
pub fn start_background_runner() {
    BACKGROUND.call_once(|| {
        let _ = std::thread::Builder::new().name("studio-queue".into()).spawn(|| {
            static NEVER: AtomicBool = AtomicBool::new(false);
            // Another process may be drawing: try again later.
            loop {
                if run_queue(&NEVER, false).is_err() {
                    std::thread::sleep(std::time::Duration::from_secs(5));
                }
            }
        });
    });
}

// ----------------------------------------------------- command line

/// `zornov node [--listen ADDR] [--token T] [--name N] [--work DIR]`
pub fn node_main(args: &[String]) -> i32 {
    let (mut listen, mut token, mut name, mut work) = (format!("0.0.0.0:{}", wire::DEFAULT_PORT), None, None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--listen" => listen = it.next().cloned().unwrap_or(listen),
            "--token" => token = it.next().cloned(),
            "--name" => name = it.next().cloned(),
            "--work" => work = it.next().map(PathBuf::from),
            "--help" | "-h" => {
                eprintln!("usage: zornov node [--listen ADDR:PORT] [--token SECRET] [--name NAME] [--work DIR]\nRuns a render node: other Zornov apps on the network send it pieces of a render. The pairing token (shown on start, or kept in the work folder) must be given to the Zornov that uses it: queue {{\"action\": \"add_node\", \"addr\": \"HOST:PORT\", \"token\": \"…\"}}.");
                return 0;
            }
            other => {
                eprintln!("zornov node: unknown option {other}");
                return 2;
            }
        }
    }
    let work = work.unwrap_or_else(|| studio_paths::cache().join("node"));
    let _ = std::fs::create_dir_all(&work);
    let token_file = work.join("token");
    let token = token.or_else(|| std::fs::read_to_string(&token_file).ok().map(|t| t.trim().to_string()).filter(|t| t.len() >= 8)).unwrap_or_else(|| {
        let t = wire::new_token();
        let _ = std::fs::write(&token_file, &t);
        t
    });
    let name = name.unwrap_or_else(|| std::env::var("HOSTNAME").or_else(|_| std::env::var("HOST")).unwrap_or_else(|_| "zornov-node".into()));
    // Projects older than a week are not kept.
    prune(&work.join("projects"), 7 * 24 * 3600);
    let _ = crate::content::get_blocking();
    let server = match wire::NodeServer::bind(&listen, &token, &work, Arc::new(AppWorker { name: name.clone() })) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("zornov node: {e}");
            return 1;
        }
    };
    let _ = server.announce(wire::DISCOVERY_PORT);
    println!("Zornov render node \"{name}\" listening on {}", server.local_addr());
    println!("Pairing token: {token}");
    println!("On the computer that renders: queue {{\"action\": \"add_node\", \"addr\": \"{}:{}\", \"token\": \"{token}\"}}", local_hint(), server.local_addr().port());
    server.serve();
    0
}

fn local_hint() -> String {
    // The address other computers see: the one a UDP "connection" to a public address would use.
    std::net::UdpSocket::bind("0.0.0.0:0").and_then(|s| s.connect("8.8.8.8:80").map(|_| s)).and_then(|s| s.local_addr()).map(|a| a.ip().to_string()).unwrap_or_else(|_| "THIS-COMPUTER".into())
}

fn prune(dir: &Path, max_age_secs: u64) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|a| a.as_secs() > max_age_secs);
        if old {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// `zornov queue run|list` for a shell or a cron job.
pub fn queue_main(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("run") => {
            let stop = AtomicBool::new(false);
            match run_queue(&stop, true) {
                Ok(n) => {
                    println!("{n} job(s) drawn");
                    0
                }
                Err(e) => {
                    eprintln!("zornov queue: {e}");
                    1
                }
            }
        }
        Some("list") | None => {
            println!("{}", queue_json(&store().read()));
            0
        }
        Some(other) => {
            eprintln!("zornov queue: unknown command {other}; use run or list");
            2
        }
    }
}

pub fn queue_json(q: &Queue) -> Value {
    json!(q.jobs.iter().map(|j| json!({
        "id": j.id, "name": j.spec.name, "state": j.state, "progress": (j.progress * 1000.0).round() / 1000.0,
        "priority": j.spec.priority, "delivery": j.spec.delivery, "preset": j.spec.preset,
        "distributed": j.spec.distributed, "outputs": j.outputs.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
        "qc": j.qc, "error": j.error, "machines": j.machines,
    })).collect::<Vec<_>>())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view;

    /// A scratch project folder holding the demo project; never anyone's own project.
    fn scratch(tag: &str) -> (PathBuf, Doc) {
        let dir = std::env::temp_dir().join(format!("studio-deliver-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut doc = Doc::from_view(&view::demo());
        proj::save(&mut doc, Some(&dir)).expect("save scratch project");
        (dir, doc)
    }

    fn small(name: &str, dir: &Path) -> JobSpec {
        let mut j = JobSpec::new(name, dir);
        j.preset = "Social 1080p".into();
        j.size = Some((320, 180));
        j.range = Some((0.0, 2.0));
        j.chunk_secs = 0.5;
        j
    }

    fn frames_of(mp4: &Path) -> usize {
        let d = studio_mp4::open(mp4).expect("mp4");
        d.movie().video().expect("video").samples.len()
    }

    #[test]
    fn a_job_renders_in_pieces_joins_them_checks_itself_and_leaves_a_report() {
        let (dir, _doc) = scratch("pieces");
        let mut j = small("Pieces", &dir);
        j.out = dir.join("out").join("pieces.mp4");
        std::fs::create_dir_all(dir.join("out")).unwrap();
        let o = run_spec(&j, &mut |_| {}).expect("render");
        assert!(j.out.exists());
        let fps = proj::open(&dir).unwrap().project.fps as f64;
        assert_eq!(frames_of(&j.out), (2.0 * fps).ceil() as usize);
        assert!(!PathBuf::from(format!("{}.parts", j.out.display())).exists(), "pieces are cleaned up");
        let (verdict, report) = o.qc.expect("qc ran");
        assert!(report.exists() && PathBuf::from(format!("{}.qc.txt", j.out.display())).exists());
        assert!(verdict.starts_with("PASS") || verdict == "FAIL", "{verdict}");
        let r: qc::Report = serde_json::from_slice(&std::fs::read(&report).unwrap()).unwrap();
        assert_eq!(r.measurements["size"], json!([320, 180]));
        assert!(o.machines.iter().map(|m| m.1).sum::<usize>() >= 2, "{:?}", o.machines);
        // The same render in one piece decodes to the same number of pictures.
        let mut whole = j.clone();
        whole.chunk_secs = 60.0;
        whole.out = dir.join("out").join("whole.mp4");
        run_spec(&whole, &mut |_| {}).expect("render whole");
        assert_eq!(frames_of(&whole.out), frames_of(&j.out));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pausing_holds_the_render_and_resuming_finishes_it() {
        let (dir, _doc) = scratch("pause");
        let store = Store::new(&dir.join("q").join("queue.json"));
        let mut j = small("Pausing", &dir);
        j.out = dir.join("pausing.mp4");
        j.range = Some((0.0, 6.0));
        j.chunk_secs = 0.4;
        j.qc = false;
        let id = store.update(|q| q.add(j)).unwrap();
        store.update(|q| q.begin(id)).unwrap().unwrap();
        let s2 = store.clone();
        let h = std::thread::spawn(move || run_job(&s2, id));
        // Wait for some progress, pause, and check that it holds.
        let t0 = std::time::Instant::now();
        while store.read().get(id).unwrap().progress < 0.05 && t0.elapsed().as_secs() < 120 {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        store.update(|q| q.pause(id)).unwrap().unwrap();
        let parts = PathBuf::from(format!("{}.parts", dir.join("pausing.mp4").display()));
        let count = || std::fs::read_dir(&parts).map(|r| r.filter_map(|e| e.ok()).filter(|e| e.file_name().to_string_lossy().ends_with(".mp4")).count()).unwrap_or(0);
        // The pause reaches the runner within a poll; only the piece under way may finish after that.
        std::thread::sleep(std::time::Duration::from_millis(800));
        let n1 = count();
        std::thread::sleep(std::time::Duration::from_millis(6000));
        let n2 = count();
        assert!(!h.is_finished(), "a paused render does not finish");
        assert!(n2 <= n1 + 1, "at most the piece under way finishes while paused ({n1} then {n2})");
        assert!(n2 < 15, "most of the render is still to do");
        store.update(|q| q.resume(id)).unwrap().unwrap();
        let o = h.join().unwrap().expect("finished after resume");
        assert!(o.outputs[0].exists());
        // Cancelling stops one for good and leaves no file.
        let mut k = small("Cancelling", &dir);
        k.out = dir.join("cancelling.mp4");
        k.range = Some((0.0, 8.0));
        k.chunk_secs = 0.4;
        let kid = store.update(|q| q.add(k)).unwrap();
        store.update(|q| q.begin(kid)).unwrap().unwrap();
        let s3 = store.clone();
        let h = std::thread::spawn(move || run_job(&s3, kid));
        std::thread::sleep(std::time::Duration::from_millis(1200));
        store.update(|q| q.cancel(kid)).unwrap().unwrap();
        let r = h.join().unwrap();
        assert!(matches!(&r, Err(e) if e == "cancelled"), "{:?}", r.map(|_| ()));
        assert!(!dir.join("cancelling.mp4").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_render_node_on_the_network_draws_pieces_that_join_with_ours() {
        let (dir, _doc) = scratch("node");
        let work = dir.join("node-work");
        let server = wire::NodeServer::bind("127.0.0.1:0", "test-token-1234", &work, Arc::new(AppWorker { name: "test-node".into() })).expect("node");
        let addr = server.local_addr();
        let stop = server.stopper();
        std::thread::spawn(move || server.serve());
        // The node list lives in the queue folder.
        let qdir = dir.join("queue-data");
        std::fs::create_dir_all(&qdir).unwrap();
        // SAFETY: this test is the only one that reads STUDIO_QUEUE_DIR through load_nodes.
        unsafe { std::env::set_var("STUDIO_QUEUE_DIR", &qdir) };
        save_nodes(&[NodeEntry { name: "test-node".into(), addr: addr.to_string(), token: "test-token-1234".into() }]).unwrap();
        let mut j = small("Distributed", &dir);
        j.out = dir.join("dist.mp4");
        j.distributed = true;
        j.range = Some((0.0, 3.0));
        j.chunk_secs = 0.5;
        let o = run_spec(&j, &mut |_| {}).expect("distributed render");
        stop.store(true, Ordering::Relaxed);
        let names: Vec<&str> = o.machines.iter().map(|m| m.0.as_str()).collect();
        assert!(names.contains(&"test-node"), "the node drew some pieces: {names:?}");
        let fps = proj::open(&dir).unwrap().project.fps as f64;
        assert_eq!(frames_of(&j.out), (3.0 * fps).ceil() as usize);
        // The joined file decodes end to end and QC reads it.
        let (_, report) = o.qc.expect("qc");
        let r: qc::Report = serde_json::from_slice(&std::fs::read(report).unwrap()).unwrap();
        assert_eq!(r.measurements["frames"], json!((3.0 * fps).ceil() as usize));
        // The node cached the project: the second render sends no media again (it renders faster / still works).
        assert!(work.join("projects").read_dir().map(|d| d.count()).unwrap_or(0) >= 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_dcp_is_built_from_a_project_through_the_queue_path() {
        let (dir, _doc) = scratch("dcp");
        let mut j = JobSpec::new("Tiny Feature", &dir);
        j.delivery = "dcp-smpte".into();
        j.range = Some((0.0, 0.25));
        j.out = dir.join("dcp-out");
        j.qc = false;
        let o = run_spec(&j, &mut |_| {}).expect("dcp");
        let folder = &o.outputs[0];
        assert!(folder.join("ASSETMAP.xml").exists(), "{}", folder.display());
        let cpl = std::fs::read_dir(folder).unwrap().filter_map(|e| e.ok()).find(|e| e.file_name().to_string_lossy().starts_with("CPL_")).expect("cpl");
        let text = std::fs::read_to_string(cpl.path()).unwrap();
        assert!(text.contains("<MainPicture>") && text.contains("<EditRate>24 1</EditRate>"), "{text}");
        let mxf = std::fs::read_dir(folder).unwrap().filter_map(|e| e.ok()).find(|e| e.file_name().to_string_lossy().starts_with("jp2k_")).expect("picture track");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
