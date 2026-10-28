// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Project file round trips, unknown-field preservation, atomic writes,
//! moved folders and autosave backups.

mod common;
use common::*;
use serde_json::{Value, json};
use std::path::PathBuf;
use studio_model::file::{self, FileError, ProjectFolder, autosave};
use studio_model::*;

/// A fresh temporary directory, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let p = std::env::temp_dir().join(format!("studio-model-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn rich_fixture() -> Fx {
    let mut f = fixture();
    let seq = f.seq;
    let e = EffectId(f.p.alloc_id());
    f.p.apply(Edit::AddEffect { seq, clip: f.a, stack: Stack::Video, effect: Effect::new(e, "glow"), index: None }).unwrap();
    f.p.apply(Edit::SetKeyframe { seq, clip: f.a, property: format!("effect.{}.amount", e.0), key: Keyframe::new(0, 0.25, Interp::Ease) }).unwrap();
    f.p.apply(Edit::SetKeyframe { seq, clip: f.title, property: "transform.position".into(), key: Keyframe::new(S, Vec2::new(10.0, -20.0), Interp::Auto) }).unwrap();
    f.p.apply(Edit::AddMarker { seq, clip: None, marker: Marker::new(MarkerId(0), 2 * S, "drop") }).unwrap();
    f.p.apply(Edit::AddMask { seq, clip: f.title, mask: Mask::new(MaskId(0), MaskShape::Heart), index: None }).unwrap();
    f.p.apply(Edit::SetSpeed { seq, clip: f.b, speed: Speed::Curve(Track::new(vec![Keyframe::new(0, 1.0, Interp::Linear), Keyframe::new(S, 0.5, Interp::Hold)])) }).unwrap();
    f
}

#[test]
fn save_load_round_trip() {
    let dir = TempDir::new("roundtrip");
    let f = rich_fixture();
    let folder = ProjectFolder::create(dir.0.join("My Video")).unwrap();
    folder.save(&f.p).unwrap();
    assert!(folder.media_dir().is_dir() && folder.cache_dir().is_dir() && folder.backups_dir().is_dir());
    let loaded = folder.load().unwrap();
    assert_eq!(loaded.format, file::FORMAT);
    assert_eq!(loaded.version, file::CURRENT_VERSION);
    assert!(loaded.project.content_eq(&folder.document(&f.p, &Extra::new()).project));
    assert_eq!(loaded.project.next_id, f.p.next_id);
    // No temporary files are left behind.
    let names: Vec<String> = std::fs::read_dir(folder.root()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert!(names.iter().all(|n| !n.contains(".tmp")), "{names:?}");
}

/// Adds `"zz_future": <tag>` to an object.
fn mark(v: &mut Value, tag: &str) {
    v.as_object_mut().unwrap_or_else(|| panic!("not an object for {tag}")).insert("zz_future".into(), json!({ "tag": tag, "n": [1, 2.5, null] }));
}

#[test]
fn unknown_fields_survive_load_and_save() {
    let dir = TempDir::new("unknown");
    let f = rich_fixture();
    let folder = ProjectFolder::create(&dir.0).unwrap();
    folder.save(&f.p).unwrap();
    let mut doc: Value = serde_json::from_str(&std::fs::read_to_string(folder.project_file()).unwrap()).unwrap();
    let paths = [
        "",
        "/project",
        "/project/settings",
        "/project/media/0",
        "/project/media/0/info",
        "/project/sequences/0",
        "/project/sequences/0/lanes/0",
        "/project/sequences/0/clips/0",
        "/project/sequences/0/clips/0/source",
        "/project/sequences/0/clips/0/effects/0",
        "/project/sequences/0/transitions/0",
        "/project/sequences/0/markers/0",
    ];
    for p in paths {
        mark(doc.pointer_mut(p).unwrap_or_else(|| panic!("missing {p}")), p);
    }
    std::fs::write(folder.project_file(), serde_json::to_string(&doc).unwrap()).unwrap();

    // Load, edit, save.
    let loaded = folder.load().unwrap();
    let mut p = loaded.project;
    let seq = p.active_sequence;
    p.apply(Edit::RenameSequence { id: seq, name: "Edited".into() }).unwrap();
    folder.save_with(&p, &loaded.extra).unwrap();

    let out: Value = serde_json::from_str(&std::fs::read_to_string(folder.project_file()).unwrap()).unwrap();
    for path in paths {
        let v = out.pointer(&format!("{path}/zz_future/tag")).unwrap_or_else(|| panic!("unknown field lost at `{path}`"));
        assert_eq!(v, path);
    }
    assert_eq!(out["project"]["sequences"][0]["name"], "Edited");
}

#[test]
fn atomic_write_replaces_whole_file() {
    let dir = TempDir::new("atomic");
    let path = dir.0.join("data.json");
    file::write_atomic(&path, b"first version, quite long").unwrap();
    file::write_atomic(&path, b"second").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"second");
    assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 1);
    // A write into a missing folder fails and leaves nothing.
    assert!(file::write_atomic(&dir.0.join("missing/x.json"), b"x").is_err());
}

#[test]
fn moved_folder_finds_its_media() {
    let dir = TempDir::new("moved");
    let original = dir.0.join("Project A");
    let folder = ProjectFolder::create(&original).unwrap();
    let clip_path = folder.media_dir().join("clip.mp4");
    std::fs::write(&clip_path, b"not really video").unwrap();
    let mut p = Project::default();
    let id = MediaId(p.alloc_id());
    p.apply(Edit::InsertMedia { item: Box::new(MediaItem::new(id, clip_path.to_string_lossy(), video_info(S))), index: None }).unwrap();
    folder.save(&p).unwrap();

    let moved = dir.0.join("Renamed");
    std::fs::rename(&original, &moved).unwrap();
    let loaded = ProjectFolder::new(&moved).load().unwrap();
    let m = loaded.project.media_item(id).unwrap();
    assert_eq!(m.relative_path.as_deref(), Some("media/clip.mp4"));
    assert_eq!(PathBuf::from(&m.path), moved.join("media/clip.mp4"));
}

#[test]
fn damaged_and_foreign_files_are_reported() {
    let dir = TempDir::new("damaged");
    let folder = ProjectFolder::create(&dir.0).unwrap();
    std::fs::write(folder.project_file(), "{ not json").unwrap();
    assert!(matches!(folder.load(), Err(FileError::Json(_))));
    let f = fixture();
    let mut doc: Value = serde_json::to_value(folder.document(&f.p, &Extra::new())).unwrap();
    // Overlap two spine clips by hand.
    doc["project"]["sequences"][0]["clips"][1]["start"] = json!(1_000_000);
    std::fs::write(folder.project_file(), doc.to_string()).unwrap();
    assert!(matches!(folder.load(), Err(FileError::Invalid(_))));
    assert!(matches!(file::from_json(r#"{"format":"studio-project","version":99,"project":{}}"#), Err(FileError::TooNew { found: 99, .. })));
    assert!(matches!(file::from_json(r#"{"hello":1}"#), Err(FileError::NotAProject)));
}

#[test]
fn autosave_keeps_twenty_backups() {
    let dir = TempDir::new("autosave");
    let folder = ProjectFolder::create(&dir.0).unwrap();
    let mut p = Project::default();
    let seq = p.active_sequence;
    let base = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    for i in 0..25u64 {
        p.apply(Edit::RenameSequence { id: seq, name: format!("v{i}") }).unwrap();
        autosave::write_backup(&folder, &p, base + std::time::Duration::from_secs(i), autosave::KEEP).unwrap();
    }
    let list = autosave::list_backups(&folder).unwrap();
    assert_eq!(list.len(), 20);
    let latest = autosave::load_latest_backup(&folder).unwrap().unwrap();
    assert_eq!(latest.active().name, "v24");
    // A damaged newest backup is skipped.
    std::fs::write(&list[0], "garbage").unwrap();
    assert_eq!(autosave::load_latest_backup(&folder).unwrap().unwrap().active().name, "v23");

    // The idle timer writes once, then waits for the next change.
    let mut a = autosave::Autosave::new();
    let t0 = std::time::Instant::now();
    a.changed(t0);
    assert!(a.poll(t0 + std::time::Duration::from_secs(5), &folder, &p).unwrap().is_none());
    assert!(a.poll(t0 + autosave::IDLE, &folder, &p).unwrap().is_some());
    assert!(!a.pending());
    assert!(a.poll(t0 + autosave::IDLE * 3, &folder, &p).unwrap().is_none());
    assert_eq!(autosave::list_backups(&folder).unwrap().len(), 20);
}

#[test]
fn file_shape_matches_spec() {
    let f = fixture();
    let doc = ProjectFolder::new("/p").document(&f.p, &Extra::new());
    let v: Value = serde_json::from_str(&file::to_json(&doc)).unwrap();
    assert_eq!(v["format"], "studio-project");
    assert_eq!(v["version"], 1);
    assert!(v["project"]["sequences"].is_array());
    assert_eq!(v["project"]["sequences"][0]["clips"][0]["source"]["type"], "media");
}

/// Saves from several threads of one process (the window's autosave and a
/// save from the menu or an agent) each finish whole: no save fails
/// because another took its temporary file, and the file is one of them.
#[test]
fn saves_from_several_threads_never_clash() {
    let dir = TempDir::new("threads");
    let path = dir.0.join("project.json");
    let payloads: Vec<Vec<u8>> = (0..8).map(|k| format!("{{\"writer\": {k}, \"pad\": \"{}\"}}", "x".repeat(200_000)).into_bytes()).collect();
    let failures = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|s| {
        for p in &payloads {
            let (path, failures) = (&path, &failures);
            s.spawn(move || {
                for _ in 0..40 {
                    if file::write_atomic(path, p).is_err() {
                        failures.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            });
        }
    });
    assert_eq!(failures.into_inner(), 0, "a save failed");
    let got = std::fs::read(&path).unwrap();
    assert!(payloads.contains(&got), "the file is not any one save whole");
    let left: Vec<_> = std::fs::read_dir(&dir.0).unwrap().flatten().map(|e| e.file_name()).filter(|n| n != "project.json").collect();
    assert!(left.is_empty(), "temporary files left: {left:?}");
}

/// A save that expects the file as it was loaded is refused once someone
/// else saved over it, and says what is on disk now.
#[test]
fn a_save_over_someone_elses_is_refused() {
    let dir = TempDir::new("stamp");
    let folder = ProjectFolder::create(&dir.0).unwrap();
    let mut p = Project::default();
    folder.save(&p).unwrap();
    let (loaded, stamp) = folder.load_stamped().unwrap();
    assert!(loaded.project.content_eq(&p));
    // Another process saves first.
    let mut theirs = p.clone();
    theirs.settings.width = 1280;
    let after_theirs = folder.save_if(&theirs, &Extra::new(), Some(stamp)).unwrap();
    assert_ne!(after_theirs, stamp);
    // Ours, still expecting the old file, is refused.
    p.settings.width = 640;
    match folder.save_if(&p, &Extra::new(), Some(stamp)) {
        Err(FileError::Changed { on_disk }) => assert_eq!(on_disk, after_theirs),
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert_eq!(folder.load().unwrap().project.settings.width, 1280);
    // Expecting what is there now, it saves.
    folder.save_if(&p, &Extra::new(), Some(after_theirs)).unwrap();
    assert_eq!(folder.load().unwrap().project.settings.width, 640);
    assert_eq!(folder.stamp(), Some(file::stamp_of(&std::fs::read(folder.project_file()).unwrap())));
}

/// A project held open by one session is seen as shared by another, and
/// no longer once the first lets go.
#[test]
fn sessions_see_each_other_hold_a_project_open() {
    let dir = TempDir::new("open");
    let folder = ProjectFolder::create(&dir.0).unwrap();
    let mut a = Some(folder.hold_open().unwrap());
    let mut b = Some(folder.hold_open().unwrap());
    assert!(folder.open_elsewhere(&mut a), "b has it open");
    assert!(a.is_some(), "a holds it again after asking");
    drop(b.take());
    assert!(!folder.open_elsewhere(&mut a), "only a now");
    assert!(folder.open_elsewhere(&mut b), "b, which let go, sees a");
}
