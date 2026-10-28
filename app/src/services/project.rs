// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Projects: new from a canvas preset, open and save project folders
//! (`studio_model::file`), import media files and timelines (OTIO,
//! FCPXML), export OTIO. Everything returns data or an error message; the
//! caller decides what to show. File dialogs stay in the UI.
//!
//! Probing media reads file headers on the calling thread (milliseconds per
//! file); call `import_media` off the UI thread for large batches.

use crate::doc::Doc;
use crate::media_io;
use crate::view;
use std::path::{Path, PathBuf};
use studio_model as m;

/// A canvas preset on the Start screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Canvas {
    pub name: &'static str,
    pub ratio: &'static str,
    pub width: u32,
    pub height: u32,
    pub hint: &'static str,
}

pub const CANVASES: &[Canvas] = &[
    Canvas { name: "Landscape", ratio: "16:9", width: 1920, height: 1080, hint: "YouTube, vlogs" },
    Canvas { name: "Vertical", ratio: "9:16", width: 1080, height: 1920, hint: "Reels, shorts" },
    Canvas { name: "Square", ratio: "1:1", width: 1080, height: 1080, hint: "Feeds" },
    Canvas { name: "Portrait", ratio: "4:5", width: 1080, height: 1350, hint: "Feed posts" },
];

pub fn canvas(name: &str) -> Option<&'static Canvas> {
    CANVASES.iter().find(|c| c.name == name)
}

/// A new, empty project on a preset canvas.
pub fn new_project(c: &Canvas) -> Doc {
    new_custom(&format!("Untitled {}", c.name.to_lowercase()), c.width, c.height, 30.0)
}

pub fn new_custom(name: &str, width: u32, height: u32, fps: f32) -> Doc {
    let mut p = view::empty();
    p.name = name.into();
    p.width = width.max(16) / 2 * 2;
    p.height = height.max(16) / 2 * 2;
    p.fps = fps;
    Doc::from_view(&p)
}

/// Open a project folder.
pub fn open(dir: &Path) -> Result<Doc, String> {
    let folder = m::file::ProjectFolder::new(dir);
    let (file, stamp) = folder.load_stamped().map_err(|e| format!("Could not open {}: {e:?}", dir.display()))?;
    let mut project = file.project;
    folder.resolve_media(&mut project);
    crate::mesh_store::relink(&mut project, dir);
    let synced = crate::doc::Synced { stamp, base: std::sync::Arc::new(project.clone()) };
    let mut doc = Doc::new(project);
    doc.synced = Some(synced);
    doc.open_hold = folder.hold_open().ok();
    restore_history(&mut doc, &folder);
    doc.folder = Some(folder);
    // The team's roles and locks, for whoever is at this computer.
    doc.guard = crate::collab::team::guard_in(dir);
    Ok(doc)
}

/// Undo history kept beside the project, tied to the exact project file
/// it belongs to: if the file changed since (another app, a hand edit),
/// the history is stale and is dropped rather than misapplied.
#[derive(serde::Serialize, serde::Deserialize)]
struct KeptHistory {
    project_hash: u64,
    history: m::SavedHistory,
}

fn history_file(folder: &m::file::ProjectFolder) -> PathBuf {
    folder.cache_dir().join("history.json")
}

/// The append-only journal that replaced `history.json`.
fn journal_file(folder: &m::file::ProjectFolder) -> PathBuf {
    folder.cache_dir().join("history.jsonl")
}

fn file_hash(path: &Path) -> Option<u64> {
    let bytes = std::fs::read(path).ok()?;
    // FNV-1a: stable across runs and builds.
    Some(bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x0100_0000_01b3)))
}

fn keep_history(doc: &mut Doc, folder: &m::file::ProjectFolder) {
    let Some(project_hash) = file_hash(&folder.project_file()) else { return };
    if super::journal::keep(&journal_file(folder), project_hash, &doc.history.snapshot()).is_ok() {
        // The journal holds it now; the old whole-file copy would go stale.
        let _ = std::fs::remove_file(history_file(folder));
    }
}

fn restore_history(doc: &mut Doc, folder: &m::file::ProjectFolder) {
    let Some(project_hash) = file_hash(&folder.project_file()) else { return };
    if let Some(saved) = super::journal::load(&journal_file(folder), project_hash) {
        doc.history.restore(saved);
        return;
    }
    // A project saved before the journal: its whole-file history.
    let Ok(bytes) = std::fs::read(history_file(folder)) else { return };
    let Ok(kept) = serde_json::from_slice::<KeptHistory>(&bytes) else { return };
    if kept.project_hash == project_hash {
        doc.history.restore(kept.history);
    }
}

/// Superseded model files go, at most every few minutes (it reads the
/// history and versions to see what is still referred to).
fn prune_models(doc: &mut Doc, root: &Path) {
    // Another session on this folder may point at files this one cannot
    // see (its unsaved edits, its undo): nothing is judged unused then.
    if m::file::ProjectFolder::new(root).open_elsewhere(&mut doc.open_hold) {
        return;
    }
    static LAST: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if last.is_some_and(|t| t.elapsed().as_secs() < 300) {
        return;
    }
    *last = Some(std::time::Instant::now());
    let history = serde_json::to_string(&doc.history.snapshot()).unwrap_or_default();
    // Without the history in words nothing can be shown unused.
    if !history.is_empty() {
        crate::mesh_store::prune(root, &history);
    }
}

/// Save to the document's folder, or create `dir` as its folder first.
/// Returns the folder.
pub fn save(doc: &mut Doc, dir: Option<&Path>) -> Result<PathBuf, String> {
    let folder = match (dir, &doc.folder) {
        (Some(d), _) => m::file::ProjectFolder::create(d).map_err(|e| format!("Could not create the folder: {e}"))?,
        (None, Some(f)) => f.clone(),
        (None, None) => return Err("Choose where to save the project first".into()),
    };
    save_merging(doc, &folder)?;
    doc.history.mark_saved();
    keep_history(doc, &folder);
    let root = folder.root().to_path_buf();
    if doc.open_hold.is_none() || doc.folder.as_ref().is_none_or(|f| f.root() != folder.root()) {
        doc.open_hold = folder.hold_open().ok();
    }
    prune_models(doc, &root);
    doc.folder = Some(folder);
    Ok(root)
}

/// Writes the document to `folder`. When someone else saved the project
/// since this document last read or wrote it (another session on the same
/// folder, a window and an agent), their work is taken in first, as one
/// undo step ("Changes saved elsewhere"), and then the whole is saved: the
/// last save no longer silently wins. Where both changed the same thing
/// this document's change is kept and `doc.error` says so.
fn save_merging(doc: &mut Doc, folder: &m::file::ProjectFolder) -> Result<(), String> {
    let same_folder = doc.folder.as_ref().is_some_and(|f| f.root() == folder.root());
    let mut expected = doc.synced.as_ref().filter(|_| same_folder).map(|s| s.stamp);
    for _ in 0..4 {
        match folder.save_if(&doc.model, &Default::default(), expected) {
            Ok(stamp) => {
                doc.synced = Some(crate::doc::Synced { stamp, base: std::sync::Arc::new(doc.model.clone()) });
                return Ok(());
            }
            Err(m::file::FileError::Changed { .. }) => {
                let (file, stamp) = folder.load_stamped().map_err(|e| format!("Could not read the project saved elsewhere: {e}"))?;
                let mut theirs = file.project;
                folder.resolve_media(&mut theirs);
                crate::mesh_store::relink(&mut theirs, folder.root());
                let base = doc.synced.as_ref().map(|s| s.base.clone()).unwrap_or_else(|| std::sync::Arc::new(theirs.clone()));
                let out = crate::collab::merge::three_way(&base, &doc.model, &theirs)?;
                if !out.merged.content_eq(&doc.model) {
                    doc.take_in("Changes saved elsewhere", &out.merged)?;
                }
                if !out.conflicts.is_empty() {
                    let kept: Vec<String> = out.conflicts.iter().take(4).map(|c| format!("{} {}", c.what, if c.name.is_empty() { c.id.to_string() } else { c.name.clone() })).collect();
                    doc.error = Some(format!("The project was also saved elsewhere; both changed {} and this one's change was kept", kept.join(", ")));
                }
                // What is on disk now is the common ground for the next save.
                doc.synced = Some(crate::doc::Synced { stamp, base: std::sync::Arc::new(theirs) });
                expected = Some(stamp);
            }
            Err(e) => return Err(format!("Could not save: {e}")),
        }
    }
    Err("Could not save: the project keeps being saved elsewhere at the same moment; try again".into())
}

/// Result of importing files into the bin.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImportReport {
    /// New media ids, in the order of the paths.
    pub ids: Vec<u32>,
    /// "name: reason" for each file that could not be read.
    pub failed: Vec<String>,
    /// Files that were already in the project (their ids are in `ids`).
    pub reused: usize,
}

/// Probe `paths` and add the readable ones to the bin as one undo step.
pub fn import_media(doc: &mut Doc, paths: &[PathBuf]) -> ImportReport {
    let mut edits = Vec::new();
    let mut report = ImportReport::default();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let mut ids = Vec::new();
    for path in paths {
        // A file already in the project is that same media, not a copy.
        let same = |p: &Path| std::fs::canonicalize(p).ok();
        let want = same(path);
        if let Some(mi) = doc.model.media.iter().find(|mi| want.is_some() && same(Path::new(&mi.path)) == want) {
            ids.push(mi.id.0 as u32);
            report.reused += 1;
            continue;
        }
        match media_io::probe(path) {
            Ok(pr) => {
                let id = doc.alloc_id();
                let info = m::MediaInfo { kind: pr.kind, container: pr.container, duration: pr.duration, video: pr.video, audio: pr.audio, extra: Default::default() };
                let mut item = m::MediaItem::new(m::MediaId(id), path.display().to_string(), info);
                item.name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
                item.imported_at = now;
                crate::services::media_meta::enrich(&mut item, path);
                edits.push(m::Edit::InsertMedia { item: Box::new(item), index: None });
                ids.push(id as u32);
            }
            Err(e) => report.failed.push(format!("{}: {e}", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())),
        }
    }
    if edits.is_empty() || doc.apply("Import media", m::Edit::Batch { edits }) {
        report.ids = ids;
    } else if !ids.is_empty() {
        report.failed.push(doc.error.take().unwrap_or_else(|| "The project refused the import".into()));
    }
    report
}

/// Import media and put each file on the main lane, in order, after what is
/// there. Returns the new clip ids.
pub fn import_to_timeline(doc: &mut Doc, paths: &[PathBuf]) -> Result<(ImportReport, Vec<u32>), String> {
    let report = import_media(doc, paths);
    let mut clips = Vec::new();
    for &id in &report.ids {
        let index = doc.project.media.iter().position(|m| m.id == id).ok_or("imported item missing")?;
        let end = doc.project.spine().map_or(0, |s| doc.project.clips.iter().filter(|c| c.lane == s).map(|c| c.end()).max().unwrap_or(0));
        let out = super::library::add(doc, super::library::Panel::Media, index, end, None).map_err(|r| r.0)?;
        clips.extend(out.select);
    }
    Ok((report, clips))
}

/// Import an OTIO or FCPXML file as a new project. Returns the project and
/// the importer's notes ("item: message").
pub fn import_timeline(path: &Path) -> Result<(Doc, Vec<String>), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("Could not read the file: {e}"))?;
    let mut project = m::Project::new(m::ProjectSettings::default());
    let is_otio = path.extension().is_some_and(|e| e == "otio");
    let imp = if is_otio { m::interchange::otio::import(&mut project, &text) } else { m::interchange::fcpxml::import(&mut project, &text) }.map_err(|e| format!("Could not import: {e:?}"))?;
    project.apply(imp.edit).map_err(|e| format!("Import failed: {e:?}"))?;
    if project.sequences.iter().any(|s| s.id == imp.sequence) {
        project.active_sequence = imp.sequence;
    }
    let notes = imp.report.issues.iter().map(|i| format!("{}: {}", i.item, i.message)).collect();
    Ok((Doc::new(project), notes))
}

/// Write the active sequence as OpenTimelineIO. Returns the notes.
pub fn export_otio(doc: &Doc, path: &Path) -> Result<Vec<String>, String> {
    let (json, report) = m::interchange::otio::export(&doc.model, doc.seq()).map_err(|e| format!("Could not export: {e:?}"))?;
    m::file::write_atomic(path, json.as_bytes()).map_err(|e| format!("Could not write: {e}"))?;
    Ok(report.issues.iter().map(|i| format!("{}: {}", i.item, i.message)).collect())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn undo_survives_closing_and_reopening() {
        let dir = std::env::temp_dir().join(format!("studio-history-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut doc = new_custom("H", 1920, 1080, 30.0);
        doc.edit("Rename", |p| p.name = "Renamed".into());
        doc.edit("Rename again", |p| p.name = "Twice".into());
        let rev = doc.history.revision();
        save(&mut doc, Some(&dir)).unwrap();

        let mut back = open(&dir).unwrap();
        assert_eq!(back.history.revision(), rev, "revision carries over");
        assert_eq!(back.undo().as_deref(), Some("Rename again"));
        assert_eq!(back.project.name, "Renamed");

        // A project file changed elsewhere makes the kept history stale.
        save(&mut back, None).unwrap();
        let file = m::file::ProjectFolder::new(&dir).project_file();
        let text = std::fs::read_to_string(&file).unwrap().replace("Renamed", "Hand edited");
        std::fs::write(&file, text).unwrap();
        let fresh = open(&dir).unwrap();
        assert!(!fresh.can_undo(), "stale history is dropped");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Two sessions on one project folder (two `zornov mcp --project`, or
    /// a window and an agent): each saves after its edit, and neither
    /// loses the other's work.
    #[test]
    fn two_sessions_on_one_project_keep_each_others_work() {
        let dir = std::env::temp_dir().join(format!("studio-two-sessions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut a = crate::agent::Session::at(&dir).unwrap();
        let mut b = crate::agent::Session::at(&dir).unwrap();
        let card = |c: &str| serde_json::json!({"ops": [{"op": "card", "colour": c, "at": 0, "duration": 2}]});
        a.run("edit", &card("#ff0000")).unwrap();
        b.run("edit", &card("#0000ff")).unwrap();
        // And once more each way: the merged state is what the next save builds on.
        a.run("edit", &card("#00ff00")).unwrap();
        let on_disk = open(&dir).unwrap();
        let cards = on_disk.model.active().clips.iter().filter(|c| !c.is_gap()).count();
        assert_eq!(cards, 3, "every session's cards are in the saved project");
        // The session that took in the other's work sees it too.
        assert_eq!(a.doc.model.active().clips.iter().filter(|c| !c.is_gap()).count(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    pub(crate) fn testdata(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../testdata/media").join(name)
    }

    #[test]
    fn vertical_preset_makes_a_vertical_project() {
        let doc = new_project(canvas("Vertical").unwrap());
        assert_eq!((doc.project.width, doc.project.height), (1080, 1920));
        assert!(doc.project.clips.is_empty());
        assert!(doc.project.spine().is_some());
    }

    #[test]
    fn import_reports_bad_files_and_keeps_good_ones() {
        let mut doc = new_project(&CANVASES[0]);
        let bad = std::env::temp_dir().join(format!("studio-not-media-{}.mp4", std::process::id()));
        std::fs::write(&bad, b"not a movie").unwrap();
        let r = import_media(&mut doc, &[testdata("h264_high_cabac.mp4"), bad.clone()]);
        assert_eq!(r.ids.len(), 1);
        assert_eq!(r.failed.len(), 1);
        assert_eq!(doc.project.media.len(), 1);
        assert!(doc.project.media[0].dur > 0);
        // One undo step.
        doc.undo();
        assert!(doc.project.media.is_empty());
        let _ = std::fs::remove_file(bad);
    }

    #[test]
    fn save_and_open_round_trip() {
        let mut doc = new_project(canvas("Square").unwrap());
        import_to_timeline(&mut doc, &[testdata("h264_high_cabac.mp4")]).unwrap();
        let dir = std::env::temp_dir().join(format!("studio-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(save(&mut doc, None).is_err());
        let root = save(&mut doc, Some(&dir)).unwrap();
        assert!(!doc.dirty());
        let back = open(&root).unwrap();
        assert_eq!(back.project.clips.len(), 1);
        assert_eq!((back.project.width, back.project.height), (1080, 1080));
        assert_eq!(super::super::transport::duration(&back), super::super::transport::duration(&doc));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn otio_round_trip() {
        let doc = Doc::from_view(&view::demo());
        let path = std::env::temp_dir().join(format!("studio-otio-{}.otio", std::process::id()));
        export_otio(&doc, &path).unwrap();
        let (back, _notes) = import_timeline(&path).unwrap();
        assert!(!back.project.clips.is_empty());
        let _ = std::fs::remove_file(path);
    }
}
