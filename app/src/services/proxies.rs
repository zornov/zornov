// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Proxies: Apple ProRes 422 LT copies of heavy media at half or
//! quarter size (`studio_render::proxy`), kept in the project's `Proxies`
//! folder (or the Proxies folder in Settings), recorded on each media item
//! (`proxy_path`, and `proxy_input` in its extra fields when the proxy's
//! colour needs naming), and used by the viewer when the project's
//! `use_proxies` setting is on. Export always reads the originals; a proxy
//! stands in wherever its original file is missing.

use crate::doc::Doc;
use std::path::{Path, PathBuf};
use studio_model as m;
use std::sync::Arc;
use studio_render::source::Proxy;

/// Project setting (in the settings' extra fields) that turns proxies on.
pub const USE_KEY: &str = "use_proxies";
const INPUT_KEY: &str = "proxy_input";

/// Whether the project plays its proxies.
pub fn enabled(p: &m::Project) -> bool {
    p.settings.extra.get(USE_KEY).and_then(|v| v.as_bool()) == Some(true)
}

/// The proxy to register for `item`: when proxies are on, or when the
/// original is missing.
pub fn proxy_for(item: &m::MediaItem, on: bool) -> Option<Proxy> {
    let path = PathBuf::from(item.proxy_path.as_deref()?);
    let original_missing = !Path::new(&item.path).exists();
    if !(on || original_missing) || !path.exists() {
        return None;
    }
    let (w, h) = item.info.video.as_ref().map_or((0, 0), |v| (v.width, v.height));
    if w == 0 || h == 0 {
        return None;
    }
    let input = item.extra.get(INPUT_KEY).and_then(|v| v.as_str()).and_then(|s| studio_render::colour_space::InputColour::parse(s, None).ok());
    Some(Proxy { path, size: (w, h), input })
}

/// The edit turning proxies on or off.
pub fn set_enabled(p: &m::Project, on: bool) -> m::Edit {
    let mut s = p.settings.clone();
    if on {
        s.extra.insert(USE_KEY.into(), serde_json::json!(true));
    } else {
        s.extra.remove(USE_KEY);
    }
    m::Edit::SetSettings { settings: Box::new(s) }
}

/// Where proxies of this project go.
pub fn folder(doc: &Doc) -> PathBuf {
    match &doc.folder {
        Some(f) => f.proxies_dir(),
        None => PathBuf::from(crate::prefs::Prefs::load(None).proxy_dir),
    }
}

/// One proxy made.
#[derive(Clone, Debug)]
pub struct Made {
    pub media: m::MediaId,
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
}

/// Makes proxies for `ids` (video only; ones already made are kept unless
/// `again`) at 1/`scale` size. Returns what was made and one line per
/// file that failed; the caller applies [`record`] as one undo step.
pub fn make(doc: &Doc, ids: &[m::MediaId], scale: u32, again: bool, progress: &mut dyn FnMut(&str, u64, u64) -> bool) -> (Vec<(Made, Option<String>)>, Vec<String>) {
    make_in(&doc.model, folder(doc), ids, scale, again, progress)
}

type Made2 = (Vec<(Made, Option<String>)>, Vec<String>);

/// [`make`] from a copy of the project and the folder, so it can run on a
/// worker thread.
pub fn make_in(model: &m::Project, dir: PathBuf, ids: &[m::MediaId], scale: u32, again: bool, progress: &mut dyn FnMut(&str, u64, u64) -> bool) -> Made2 {
    let mut made = Vec::new();
    let mut failed = Vec::new();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        failed.push(format!("{}: {e}", dir.display()));
        return (made, failed);
    }
    for id in ids {
        let Some(item) = model.media_item(*id) else { continue };
        if item.info.kind != m::MediaKind::Video || item.path.starts_with("offline:") || item.path.starts_with("sample:") {
            continue;
        }
        if !again && item.proxy_path.as_deref().is_some_and(|p| Path::new(p).exists()) {
            continue;
        }
        let stem = Path::new(&item.path).file_stem().map_or_else(|| format!("media{}", id.0), |s| s.to_string_lossy().replace('#', ""));
        let out = dir.join(format!("{stem} (proxy {}).mov", id.0));
        let tc = item.timecode.as_deref().and_then(|t| {
            let rate = item.timecode_rate(model.settings.frame_rate);
            let tcd = m::Timecode::new(rate, t.contains(';'));
            Some(studio_mov::StartTimecode { frame: tcd.parse_lenient(t)? as u32, fps: tcd.nominal_fps() as u32, drop_frame: tcd.is_drop_frame(), reel: item.reel.clone() })
        });
        let name = item.name.clone();
        match studio_render::proxy::make_proxy(Path::new(&item.path), &out, scale, tc, &mut |d, t| progress(&name, d, t)) {
            Ok(info) => made.push((Made { media: *id, path: out, width: info.width, height: info.height }, info.input)),
            Err(e) => failed.push(format!("{name}: {e}")),
        }
    }
    (made, failed)
}

/// The edit recording made proxies on their media items.
pub fn record(p: &m::Project, made: &[(Made, Option<String>)]) -> m::Edit {
    let mut edits = Vec::new();
    for (mk, input) in made {
        let Some(item) = p.media_item(mk.media) else { continue };
        let mut it = item.clone();
        it.proxy_path = Some(mk.path.display().to_string());
        match input {
            Some(s) => {
                it.extra.insert(INPUT_KEY.into(), serde_json::json!(s));
            }
            None => {
                it.extra.remove(INPUT_KEY);
            }
        }
        edits.push(m::Edit::SetMedia { item: Box::new(it) });
    }
    m::Edit::Batch { edits }
}

/// The edit forgetting proxies (the files stay on disk).
pub fn forget(p: &m::Project, ids: &[m::MediaId]) -> m::Edit {
    let edits = ids
        .iter()
        .filter_map(|id| p.media_item(*id))
        .filter(|i| i.proxy_path.is_some())
        .map(|i| {
            let mut it = i.clone();
            it.proxy_path = None;
            it.extra.remove(INPUT_KEY);
            m::Edit::SetMedia { item: Box::new(it) }
        })
        .collect();
    m::Edit::Batch { edits }
}

// --- proxy jobs ----------------------------------------------------------

/// A proxy being made in the background: progress for the Library tile and
/// a way to cancel.
pub struct Job {
    media: Vec<m::MediaId>,
    done: std::sync::atomic::AtomicU64,
    total: std::sync::atomic::AtomicU64,
    cancel: std::sync::atomic::AtomicBool,
    result: std::sync::Mutex<Option<Made2>>,
}

impl Job {
    /// 0..1 (a sliver before the first picture is counted).
    pub fn fraction(&self) -> f32 {
        use std::sync::atomic::Ordering::Relaxed;
        let (d, t) = (self.done.load(Relaxed), self.total.load(Relaxed));
        if t == 0 { 0.02 } else { (d as f32 / t as f32).clamp(0.02, 1.0) }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    fn finished(&self) -> bool {
        self.result.lock().unwrap_or_else(|e| e.into_inner()).is_some()
    }
}

fn jobs() -> std::sync::MutexGuard<'static, Vec<Arc<Job>>> {
    static JOBS: std::sync::Mutex<Vec<Arc<Job>>> = std::sync::Mutex::new(Vec::new());
    JOBS.lock().unwrap_or_else(|e| e.into_inner())
}



/// Runs `work` on a thread as a proxy job for `media`; `work` gets a
/// `(done, total) -> keep_going` callback to call as it goes.
pub fn spawn(media: Vec<m::MediaId>, work: impl FnOnce(&mut dyn FnMut(u64, u64) -> bool) -> Made2 + Send + 'static) -> Arc<Job> {
    use std::sync::atomic::Ordering::Relaxed;
    let job = Arc::new(Job { media, done: Default::default(), total: Default::default(), cancel: Default::default(), result: Default::default() });
    jobs().push(job.clone());
    let j = job.clone();
    std::thread::spawn(move || {
        let r = work(&mut |d, t| {
            j.done.store(d, Relaxed);
            j.total.store(t, Relaxed);
            !j.cancel.load(Relaxed)
        });
        let r = if j.cancel.load(Relaxed) { (Vec::new(), vec!["Cancelled".to_string()]) } else { r };
        *j.result.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
    });
    job
}

/// Start making proxies for `ids` at 1/`scale` size in the background.
pub fn start(doc: &Doc, ids: Vec<m::MediaId>, scale: u32) -> Arc<Job> {
    let (model, dir) = (doc.model.clone(), folder(doc));
    let wanted = ids.clone();
    spawn(ids, move |progress| make_in(&model, dir, &wanted, scale, true, &mut |_, d, t| progress(d, t)))
}

/// The share (0..1) of media `id`'s proxy made so far, if one is being made.
pub fn progress_of(id: u32) -> Option<f32> {
    jobs().iter().find(|j| !j.finished() && j.media.iter().any(|m| m.0 == id as u64)).map(|j| j.fraction())
}

/// Stop the proxy being made for media `id`.
pub fn cancel(id: u32) {
    jobs().iter().filter(|j| j.media.iter().any(|m| m.0 == id as u64)).for_each(|j| j.cancel());
}

/// Whether any proxy is being made (the caller keeps repainting).
pub fn busy() -> bool {
    jobs().iter().any(|j| !j.finished())
}

/// Take in finished jobs: record their proxies as one undo step. Returns
/// one line per job for a toast, and whether it went wrong.
pub fn collect(doc: &mut Doc) -> Vec<(String, bool)> {
    let done: Vec<Arc<Job>> = {
        let mut all = jobs();
        let (fin, running): (Vec<_>, Vec<_>) = all.drain(..).partition(|j| j.finished());
        *all = running;
        fin
    };
    let mut out = Vec::new();
    for j in done {
        let Some((made, failed)) = j.result.lock().unwrap_or_else(|e| e.into_inner()).take() else { continue };
        if !made.is_empty() {
            let edit = record(&doc.model, &made);
            if !doc.apply("Make proxy", edit) {
                out.push((doc.error.take().unwrap_or_else(|| "Could not record the proxy".into()), true));
                continue;
            }
            let n = made.len();
            out.push((format!("Made {n} proxy{}", if n == 1 { "" } else { "ies" }).replace("proxyies", "proxies"), !failed.is_empty()));
        }
        if let Some(f) = failed.first() {
            out.push((if f == "Cancelled" { "Cancelled the proxy".to_string() } else { format!("Could not make a proxy: {f}") }, f != "Cancelled"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A background proxy job shows progress, can be cancelled, and a
    /// cancelled one records nothing.
    #[test]
    fn a_proxy_job_reports_progress_and_cancels() {
        let id = 987_654u32;
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let job = spawn(vec![m::MediaId(id as u64)], move |progress| {
            let _ = rx.recv();
            // Runs until the callback says stop.
            while progress(5, 10) {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            (Vec::new(), Vec::new())
        });
        assert!(busy());
        assert!((0.02..=1.0).contains(&progress_of(id).unwrap()));
        tx.send(()).unwrap();
        cancel(id);
        let mut doc = crate::agent::Session::new(crate::services::project::new_custom("Proxy", 640, 360, 30.0)).doc;
        let rev = doc.history.revision();
        let mut lines = Vec::new();
        for _ in 0..400 {
            lines = collect(&mut doc);
            if !lines.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(lines, vec![("Cancelled the proxy".to_string(), false)]);
        assert_eq!(doc.history.revision(), rev, "a cancelled proxy changes nothing");
        assert!(progress_of(id).is_none());
        drop(job);
    }
}
