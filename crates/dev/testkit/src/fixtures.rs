// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Generated fixtures live under `target-qa/fixtures` and are made on
//! first use. A committed manifest (`crates/dev/testkit/fixtures/manifest.json`)
//! holds the SHA-256 of every fixture, so a generator that stops being
//! deterministic, or changes by accident, fails a test instead of silently
//! moving every downstream golden.
//!
//! Rules: nothing is written outside `target-qa`; generation holds a lock so
//! parallel tests do not race; `UPDATE_FIXTURES=1` rewrites the manifest.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::sha256;

static LOCK: Mutex<()> = Mutex::new(());
static SCRATCH_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The workspace root (the folder holding the root `Cargo.toml`).
pub fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().expect("workspace root")
}

/// The QA build directory: `$CARGO_TARGET_DIR`, else `<workspace>/target-qa`.
/// Every file a test writes goes below this folder.
pub fn qa_target_dir() -> PathBuf {
    match std::env::var_os("CARGO_TARGET_DIR") {
        Some(p) => {
            let p = PathBuf::from(p);
            if p.is_absolute() { p } else { workspace_root().join(p) }
        }
        None => workspace_root().join("target-qa"),
    }
}

/// Where generated fixtures live.
pub fn fixtures_dir() -> PathBuf {
    let d = qa_target_dir().join("fixtures");
    fs::create_dir_all(&d).expect("create fixtures dir");
    d
}

/// A fresh empty folder for one test under `target-qa/tmp`. The caller
/// owns it; [`MockFs`](crate::mocks::MockFs) removes its folder on drop.
pub fn scratch_dir(label: &str) -> PathBuf {
    let n = SCRATCH_COUNTER.fetch_add(1, Ordering::Relaxed);
    let clean: String = label.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    let d = qa_target_dir().join("tmp").join(format!("{clean}-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).expect("create scratch dir");
    d
}

fn manifest_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/manifest.json")
}

fn read_manifest() -> BTreeMap<String, String> {
    match fs::read_to_string(manifest_path()) {
        Ok(s) => serde_json::from_str(&s).expect("fixtures/manifest.json is valid JSON"),
        Err(_) => BTreeMap::new(),
    }
}

fn write_manifest(m: &BTreeMap<String, String>) {
    let p = manifest_path();
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    let mut text = serde_json::to_string_pretty(m).unwrap();
    text.push('\n');
    fs::write(p, text).expect("write manifest");
}

fn update_mode() -> bool {
    std::env::var("UPDATE_FIXTURES").map(|v| v == "1").unwrap_or(false)
}

/// Returns the path of fixture `name`, generating it with `generate` on
/// first use. `generate` receives the final path's parent-relative target;
/// it must write exactly that file (and may write `<name>.json` next to it
/// for ground truth). The file's hash is checked against the manifest.
pub fn fixture(name: &str, generate: impl FnOnce(&Path) -> io::Result<()>) -> PathBuf {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = fixtures_dir().join(name);
    let mut manifest = read_manifest();
    let expected = manifest.get(name).cloned();
    let fresh = !path.exists();
    if fresh {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("fixture folder");
        }
        let tmp = path.with_extension(format!("{}.part", path.extension().and_then(|e| e.to_str()).unwrap_or("bin")));
        generate(&tmp).unwrap_or_else(|e| panic!("generating fixture {name}: {e}"));
        // Ground truth may have been written next to the temp file.
        let truth_tmp = tmp.with_extension("json");
        if truth_tmp.exists() {
            fs::rename(&truth_tmp, truth_path(&path)).expect("move ground truth");
        }
        fs::rename(&tmp, &path).expect("move fixture into place");
    }
    let actual = sha256::hex(&sha256::digest(&fs::read(&path).expect("read fixture")));
    match expected {
        Some(h) if h == actual => {}
        Some(h) if update_mode() => {
            manifest.insert(name.to_string(), actual.clone());
            write_manifest(&manifest);
            eprintln!("fixture {name}: manifest updated {h} -> {actual}");
        }
        Some(h) => {
            if fresh {
                panic!(
                    "fixture {name} was generated with hash {actual} but the manifest says {h}.\n\
                     The generator (or an encoder it uses) is no longer deterministic or has changed.\n\
                     If the change is intended run with UPDATE_FIXTURES=1 and commit crates/dev/testkit/fixtures/manifest.json."
                );
            } else {
                panic!("fixture {name} on disk has hash {actual}, manifest says {h}. Delete {} to regenerate.", path.display());
            }
        }
        None if update_mode() => {
            manifest.insert(name.to_string(), actual.clone());
            write_manifest(&manifest);
            eprintln!("fixture {name}: added to manifest ({actual})");
        }
        None => panic!("fixture {name} has no manifest entry (hash {actual}). Run once with UPDATE_FIXTURES=1 and commit the manifest."),
    }
    path
}

/// Path of the ground-truth JSON that belongs to a fixture file.
pub fn truth_path(fixture: &Path) -> PathBuf {
    let stem = fixture.file_name().unwrap().to_string_lossy().to_string();
    fixture.with_file_name(format!("{stem}.json"))
}

/// Reads a fixture's ground truth.
pub fn read_truth<T: serde::de::DeserializeOwned>(fixture: &Path) -> T {
    let p = truth_path(fixture);
    let text = fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", p.display()))
}

/// Writes a fixture's ground truth next to the file being generated.
pub fn write_truth<T: serde::Serialize>(fixture_tmp: &Path, truth: &T) -> io::Result<()> {
    let text = serde_json::to_string_pretty(truth).map_err(io::Error::other)?;
    fs::write(fixture_tmp.with_extension("json"), text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qa_dir_is_inside_workspace_or_env() {
        let d = qa_target_dir();
        assert!(d.is_absolute());
        let s = scratch_dir("probe");
        assert!(s.starts_with(&d));
        fs::remove_dir_all(s).unwrap();
    }
}

#[cfg(test)]
mod standard_fixtures {
    //! Generates every standard fixture so the manifest covers them all.
    use crate::images::{ColourType, jpeg_fixture, png_fixture};
    use crate::movie::standard;

    #[test]
    fn all_standard_movies_generate() {
        for p in [standard::small(), standard::silent(), standard::odd_size(), standard::tiny(), standard::drop_frame(), standard::rotated_90(), standard::shapes_60(), standard::baseline(), standard::mono_44k()] {
            let len = std::fs::metadata(&p).unwrap().len();
            assert!(len > 0 && len < 2 << 20, "{} is {len} bytes", p.display());
            let truth: crate::movie::MovieTruth = crate::fixtures::read_truth(&p);
            assert_eq!(truth.frames.len() as u32, truth.spec.frames);
        }
    }

    #[test]
    fn all_standard_images_generate() {
        for ty in [ColourType::Gray, ColourType::Rgb, ColourType::Palette, ColourType::GrayAlpha, ColourType::Rgba] {
            let depths: &[u8] = match ty {
                ColourType::Gray => &[1, 2, 4, 8, 16],
                ColourType::Palette => &[1, 2, 4, 8],
                _ => &[8, 16],
            };
            for &d in depths {
                for il in [false, true] {
                    png_fixture(37, 23, ty, d, il);
                }
            }
        }
        for o in [None, Some(1u16), Some(3), Some(6), Some(8)] {
            jpeg_fixture(40, 24, 85, o);
        }
    }
}
