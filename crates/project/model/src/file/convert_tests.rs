// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Conversion tests. Every project here is built in a temporary folder to
//! match the shape of a real one; no real project is read or written.

use super::*;
use crate::file::{self, ProjectFolder};
use std::collections::BTreeMap;

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let p = std::env::temp_dir().join(format!("zornov-convert-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        Self(std::fs::canonicalize(p).unwrap())
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn put(root: &Path, rel: &str, bytes: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

/// A project document with these media: (name, path, relative_path), and
/// `extra` keys added to the project object.
fn document(media: &[(&str, String, Option<String>)], extra: Value) -> String {
    let mut project = crate::Project::new(crate::ProjectSettings::default());
    for (name, path, rel) in media {
        let id = crate::MediaId(project.alloc_id());
        let mut item = crate::MediaItem::new(id, path.clone(), crate::MediaInfo::default());
        item.name = (*name).to_owned();
        item.relative_path = rel.clone();
        project.media.push(item);
    }
    let file = file::ProjectFile { format: file::FORMAT.into(), version: file::CURRENT_VERSION, project, extra: Default::default() };
    let mut v: Value = serde_json::from_str(&file::to_json(&file)).unwrap();
    if let Value::Object(o) = extra {
        for (k, val) in o {
            v["project"][k] = val;
        }
    }
    serde_json::to_string_pretty(&v).unwrap()
}

/// Shaped like `/Users/Shared/Zornov/Road trip`.
fn road_trip(parent: &Path) -> PathBuf {
    let root = parent.join("Road trip");
    let names = ["Grand Canyon", "Burney Falls", "Arches National Park", "Yosemite Valley"];
    let mut media = Vec::new();
    for n in names {
        put(&root, &format!("media/{n}.jpg"), format!("jpg {n}").as_bytes());
        media.push((format!("{n}.jpg"), root.join(format!("media/{n}.jpg")).display().to_string(), Some(format!("media/{n}.jpg"))));
    }
    put(&root, "music/music-warm-acoustic-0ecc.wav", b"RIFF music");
    media.push(("music-warm-acoustic-0ecc.wav".into(), root.join("music/music-warm-acoustic-0ecc.wav").display().to_string(), None));
    put(&root, "cache/check.json", b"{}");
    let m: Vec<_> = media.iter().map(|(a, b, c)| (a.as_str(), b.clone(), c.clone())).collect();
    put(&root, "project.json", document(&m, json!({})).as_bytes());
    root
}

/// Shaped like `.../flight-recorder/project`: a state file and a model at the
/// top, traces in `.zornov`, and a media path that names nothing.
fn flight_recorder(parent: &Path) -> PathBuf {
    let root = parent.join("project");
    put(&root, "music/music-dark-0-df58.wav", b"RIFF dark");
    put(&root, "agent-usage.jsonl", b"{\"in\":1}\n");
    put(&root, "Models/ba8912fc457eb160.smesh", b"mesh");
    put(&root, ".zornov/agent-trace/1793-4583.jsonl", b"");
    std::fs::create_dir_all(root.join("media")).unwrap();
    let media = [("music-dark-0-df58.wav", "showcase-project/music/music-dark-0-df58.wav".to_owned(), None)];
    let extra = json!({"clip_models": [{"file": root.join("Models/ba8912fc457eb160.smesh").display().to_string()}]});
    put(&root, "project.json", document(&media, extra).as_bytes());
    root
}

/// Every file under `dir` with its bytes, keyed by relative path (folders
/// show as keys with no bytes), to compare whole trees.
fn tree(dir: &Path) -> BTreeMap<String, Option<Vec<u8>>> {
    fn go(dir: &Path, root: &Path, out: &mut BTreeMap<String, Option<Vec<u8>>>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            let rel = p.strip_prefix(root).unwrap().to_string_lossy().into_owned();
            if e.file_type().unwrap().is_dir() {
                out.insert(format!("{rel}/"), None);
                go(&p, root, out);
            } else {
                out.insert(rel, Some(std::fs::read(&p).unwrap()));
            }
        }
    }
    let mut out = BTreeMap::new();
    go(dir, dir, &mut out);
    out
}

fn media_of(package: &Path) -> Vec<(String, Option<String>)> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(package.join("project.json")).unwrap()).unwrap();
    v["project"]["media"].as_array().unwrap().iter().map(|m| (m["path"].as_str().unwrap().to_owned(), m["relative_path"].as_str().map(str::to_owned))).collect()
}

#[test]
fn plan_writes_nothing() {
    let t = Tmp::new("plan");
    let root = road_trip(&t.0);
    let before = tree(&t.0);
    let p = plan(&root).unwrap();
    assert_eq!(before, tree(&t.0));
    assert!(p.areas().contains_key("Media") && p.areas().contains_key("Generated") && p.areas().contains_key("Cache"));
    assert!(p.moves.iter().any(|m| m.from == Path::new("media/Grand Canyon.jpg") && m.to == Path::new("Media/Pictures/Grand Canyon.jpg")));
}

#[test]
fn road_trip_becomes_a_package() {
    let t = Tmp::new("roadtrip");
    let root = road_trip(&t.0);
    let r = convert(&root, Options::default(), &Hooks::default()).unwrap();
    assert_eq!(r.package, t.0.join("Road trip.zornov"));
    assert!(!root.exists(), "the emptied old folder is gone");
    let pkg = ProjectFolder::new(&r.package);
    assert_eq!(pkg.layout(), file::Layout::Package);
    assert!(r.package.join("Media/Pictures/Grand Canyon.jpg").is_file());
    assert!(r.package.join("Generated/Music/music-warm-acoustic-0ecc.wav").is_file());
    assert!(r.package.join("Cache/check.json").is_file());
    assert!(r.package.join(".zornov/legacy/project.json").is_file(), "the old project file is kept");
    let loaded = pkg.load().unwrap();
    assert!(loaded.project.media.iter().all(|m| Path::new(&m.path).is_file()), "every media path resolves: {:?}", media_of(&r.package));
    assert!(loaded.project.media.iter().all(|m| Path::new(&m.path).starts_with(&r.package)));
    assert!(r.unresolved.is_empty() && r.relinked.is_empty());
}

#[test]
fn flight_recorder_moves_models_state_and_relinks_by_name() {
    let t = Tmp::new("flight");
    let root = flight_recorder(&t.0);
    let r = convert(&root, Options::default(), &Hooks::default()).unwrap();
    let pkg = &r.package;
    assert!(pkg.join("Media/3D/ba8912fc457eb160.smesh").is_file());
    assert!(pkg.join(".zornov/agent-usage.jsonl").is_file());
    assert!(pkg.join(".zornov/agent-trace/1793-4583.jsonl").is_file(), "the traces already in .zornov are kept");
    assert_eq!(r.relinked.len(), 1, "the dangling media path is matched by name: {r:?}");
    let media = media_of(pkg);
    assert_eq!(media[0].0, pkg.join("Generated/Music/music-dark-0-df58.wav").display().to_string());
    // A model path in another part of the document moves too.
    let text = std::fs::read_to_string(pkg.join("project.json")).unwrap();
    assert!(text.contains(&pkg.join("Media/3D/ba8912fc457eb160.smesh").display().to_string()), "{text}");
    assert!(!text.contains("/Models/"));
}

#[test]
fn unknown_files_are_kept_in_legacy() {
    let t = Tmp::new("unknown");
    let root = road_trip(&t.0);
    put(&root, "README.txt", b"my notes");
    put(&root, "scratch/ideas/a.txt", b"deep");
    let r = convert(&root, Options::default(), &Hooks::default()).unwrap();
    assert_eq!(std::fs::read(r.package.join(".zornov/legacy/README.txt")).unwrap(), b"my notes");
    assert_eq!(std::fs::read(r.package.join(".zornov/legacy/scratch/ideas/a.txt")).unwrap(), b"deep");
    assert_eq!(r.unknown.len(), 2);
}

#[test]
fn a_clip_called_models_is_not_rewritten() {
    let t = Tmp::new("models");
    let root = flight_recorder(&t.0);
    // A word that happens to be a folder's name, and a path elsewhere.
    let mut doc: Value = serde_json::from_str(&std::fs::read_to_string(root.join("project.json")).unwrap()).unwrap();
    doc["project"]["title"] = json!("Models");
    doc["project"]["elsewhere"] = json!("/somewhere/else/Models/x.smesh");
    put(&root, "project.json", doc.to_string().as_bytes());
    let r = convert(&root, Options::default(), &Hooks::default()).unwrap();
    let v: Value = serde_json::from_str(&std::fs::read_to_string(r.package.join("project.json")).unwrap()).unwrap();
    assert_eq!(v["project"]["title"], "Models");
    assert_eq!(v["project"]["elsewhere"], "/somewhere/else/Models/x.smesh");
}

#[test]
fn saved_versions_get_their_paths_rewritten() {
    let t = Tmp::new("versions");
    let root = road_trip(&t.0);
    let doc = std::fs::read_to_string(root.join("project.json")).unwrap();
    put(&root, "backups/autosave-0000000000000001.json", doc.as_bytes());
    let r = convert(&root, Options::default(), &Hooks::default()).unwrap();
    let kept = std::fs::read_to_string(r.package.join("Versions/autosave-0000000000000001.json")).unwrap();
    assert!(kept.contains("Media/Pictures/Grand Canyon.jpg") && !kept.contains("/Road trip/media/"), "{kept}");
    assert!(r.rewritten.iter().any(|p| p.starts_with("Versions/")));
}

#[test]
fn deliverables_leave_the_package() {
    let t = Tmp::new("exports");
    let root = road_trip(&t.0);
    put(&root, "exports/final.mp4", b"video");
    put(&root, "Stems/dx.wav", b"stem");
    std::fs::create_dir_all(t.0.join("Road trip Exports")).unwrap();
    put(&t.0, "Road trip Exports/final.mp4", b"older render");
    let r = convert(&root, Options::default(), &Hooks::default()).unwrap();
    let ex = t.0.join("Road trip Exports");
    assert_eq!(std::fs::read(ex.join("final.mp4")).unwrap(), b"older render", "an earlier render is not overwritten");
    assert_eq!(std::fs::read(ex.join("final 2.mp4")).unwrap(), b"video");
    assert_eq!(std::fs::read(ex.join("Stems/dx.wav")).unwrap(), b"stem");
    assert!(!r.package.join("exports").exists());
}

#[test]
fn name_collisions_get_numbers_and_sidecars_follow_their_media() {
    let t = Tmp::new("names");
    let root = road_trip(&t.0);
    put(&root, "media/shot.mp4", b"video");
    put(&root, "media/shot.words.json", b"[]");
    put(&root, "Pictures/Grand Canyon.jpg", b"painted");
    let r = convert(&root, Options::default(), &Hooks::default()).unwrap();
    assert!(r.package.join("Media/Footage/shot.words.json").is_file(), "spoken words stay beside the media");
    assert!(r.package.join("Media/Pictures/Grand Canyon.jpg").is_file() && r.package.join("Media/Pictures/Grand Canyon 2.jpg").is_file());
}

#[test]
fn copy_mode_leaves_the_original_exactly_as_it_was() {
    let t = Tmp::new("copy");
    let root = road_trip(&t.0);
    let before = tree(&root);
    let r = convert(&root, Options { copy: true }, &Hooks::default()).unwrap();
    assert_eq!(before, tree(&root));
    assert!(ProjectFolder::new(&r.package).load().is_ok());
    assert!(r.package.join("Media/Pictures/Grand Canyon.jpg").is_file());
}

#[test]
fn a_package_is_not_converted_again() {
    let t = Tmp::new("again");
    let pkg = t.0.join("A.zornov");
    ProjectFolder::create_package(&pkg).unwrap().save(&crate::Project::new(crate::ProjectSettings::default())).unwrap();
    let before = tree(&t.0);
    assert!(matches!(convert(&pkg, Options::default(), &Hooks::default()), Err(ConvertError::AlreadyPackage)));
    assert_eq!(before, tree(&t.0));
}

#[test]
fn an_open_project_is_not_converted() {
    let t = Tmp::new("busy");
    let root = road_trip(&t.0);
    let hold = ProjectFolder::new(&root).hold_open().unwrap();
    let before = tree(&t.0);
    assert!(matches!(convert(&root, Options::default(), &Hooks::default()), Err(ConvertError::Busy)));
    assert_eq!(before, tree(&t.0));
    drop(hold);
}

#[test]
fn any_failed_move_leaves_the_project_as_it_was() {
    let t = Tmp::new("fail");
    let root = flight_recorder(&t.0);
    put(&root, "exports/a.mp4", b"x");
    put(&root, "backups/autosave-0000000000000001.json", b"{}");
    let n = plan(&root).unwrap().moves.len();
    let before = tree(&t.0);
    for k in 0..n {
        let fail = |i: usize| if i == k { Err(io::Error::other("disk gone")) } else { Ok(()) };
        let r = convert(&root, Options::default(), &Hooks { before_move: Some(&fail), stop_after: None });
        assert!(matches!(r, Err(ConvertError::Failed { rolled_back: true, .. })), "move {k}: {r:?}");
        assert_eq!(before, tree(&t.0), "after failing move {k}");
        assert!(ProjectFolder::new(&root).load().is_ok());
    }
}

#[test]
fn a_crash_at_any_stage_settles_to_all_old_or_all_new() {
    for stage in [Stage::Journaled, Stage::Moved, Stage::Written, Stage::Marked, Stage::Committed] {
        let t = Tmp::new("crash");
        let root = road_trip(&t.0);
        put(&root, "exports/a.mp4", b"x");
        let before = tree(&t.0);
        let r = convert(&root, Options::default(), &Hooks { before_move: None, stop_after: Some(stage) });
        assert!(matches!(r, Err(ConvertError::Stopped(s)) if s == stage));
        let found = recover(&root);
        if stage == Stage::Committed {
            assert_eq!(found, Recovered::Finished, "{stage:?}");
            assert!(!root.exists() && ProjectFolder::new(t.0.join("Road trip.zornov")).load().is_ok());
        } else {
            assert_eq!(found, Recovered::RolledBack, "{stage:?}");
            assert_eq!(before, tree(&t.0), "{stage:?}");
        }
        assert_eq!(recover(&root), Recovered::Nothing, "recovering twice changes nothing ({stage:?})");
    }
}

#[test]
fn an_unconverted_project_is_never_touched_by_saving() {
    let t = Tmp::new("legacy");
    let root = road_trip(&t.0);
    let folder = ProjectFolder::new(&root);
    assert_eq!(folder.layout(), file::Layout::Legacy);
    let loaded = folder.load().unwrap();
    folder.save(&loaded.project).unwrap();
    crate::file::autosave::write_backup(&folder, &loaded.project, std::time::SystemTime::now(), 5).unwrap();
    let names: Vec<String> = tree(&root).into_keys().collect();
    assert!(names.iter().any(|n| n.starts_with("backups/autosave-")), "autosaves still go to backups/");
    assert!(!names.iter().any(|n| n.starts_with("Media") || n.starts_with("Versions") || n.starts_with(".zornov")), "{names:?}");
}

#[test]
fn layout_detection_ignores_a_bare_state_folder_and_refuses_newer_layouts() {
    let t = Tmp::new("detect");
    let root = flight_recorder(&t.0);
    assert_eq!(ProjectFolder::new(&root).layout(), file::Layout::Legacy, ".zornov/agent-trace alone does not make a package");
    let pkg = t.0.join("N.zornov");
    ProjectFolder::create_package(&pkg).unwrap();
    assert_eq!(ProjectFolder::new(&pkg).layout(), file::Layout::Package);
    for d in ["Media/3D", "Media/Pictures", "Generated", "Worlds", "Versions", "Cache", ".zornov"] {
        assert!(pkg.join(d).is_dir(), "{d}");
    }
    put(&pkg, ".zornov/package.json", br#"{"layout": 99}"#);
    assert!(matches!(ProjectFolder::new(&pkg).load(), Err(file::FileError::LayoutTooNew { found: 99, .. })));
    assert!(matches!(ProjectFolder::new(&pkg).save(&crate::Project::new(crate::ProjectSettings::default())), Err(file::FileError::LayoutTooNew { .. })));
}

#[test]
fn a_package_saves_and_locks_inside_its_state_folder() {
    let t = Tmp::new("package");
    let pkg = t.0.join("P.zornov");
    let f = ProjectFolder::create_package(&pkg).unwrap();
    let project = crate::Project::new(crate::ProjectSettings::default());
    f.save_if(&project, &Default::default(), None).unwrap();
    let _hold = f.hold_open().unwrap();
    assert!(pkg.join(".zornov/lock").exists() && pkg.join(".zornov/open").exists());
    assert!(!pkg.join(".project.lock").exists() && !pkg.join(".project.open").exists());
    crate::file::autosave::write_backup(&f, &crate::Project::new(crate::ProjectSettings::default()), std::time::SystemTime::now(), 5).unwrap();
    assert_eq!(std::fs::read_dir(pkg.join("Versions")).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("autosave-")).count(), 1);
    // A second project cannot be created over it.
    assert!(ProjectFolder::create_package(&pkg).is_err());
}

/// Run by hand against a copy of a real project:
/// `ZORNOV_REAL_PROJECT=/path/to/project cargo test -p studio-model real_project -- --ignored`.
/// The folder is copied into a temporary one first; the original is only read.
#[test]
#[ignore]
fn real_project_copy_converts() {
    let Some(src) = std::env::var_os("ZORNOV_REAL_PROJECT").map(PathBuf::from) else { return };
    let t = Tmp::new("real");
    let dst = t.0.join(src.file_name().unwrap());
    fn copy_dir(a: &Path, b: &Path) {
        std::fs::create_dir_all(b).unwrap();
        for e in std::fs::read_dir(a).unwrap().flatten() {
            let (p, q) = (e.path(), b.join(e.file_name()));
            if e.file_type().unwrap().is_dir() { copy_dir(&p, &q) } else if e.file_type().unwrap().is_file() { std::fs::copy(&p, &q).unwrap(); }
        }
    }
    copy_dir(&src, &dst);
    let r = convert(&dst, Options::default(), &Hooks::default()).unwrap();
    eprintln!("{r:#?}");
    assert!(ProjectFolder::new(&r.package).load().is_ok());
}
