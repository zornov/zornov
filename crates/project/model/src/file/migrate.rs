// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Format version upgrades.
//!
//! Each format change gets a function `migrate_vN_to_vN+1` that rewrites
//! the raw JSON document of version N into version N+1, and is appended to
//! [`MIGRATIONS`]. Loading runs every step from the file's version up to
//! [`CURRENT_VERSION`] in order, so each function only ever deals with
//! the version just before it. Migrations work on `serde_json::Value`
//! rather than on Rust types because the old types no longer exist.
//!
//! To change the format:
//! 1. bump [`CURRENT_VERSION`];
//! 2. write `migrate_vN_to_vN+1(doc: &mut Value) -> Result<(), String>`
//!    that also sets `doc["version"]`;
//! 3. append it to [`MIGRATIONS`];
//! 4. add a test that loads a saved version-N document.

use super::FileError;
use serde_json::Value;

/// The format version this build writes.
pub const CURRENT_VERSION: u64 = 1;

/// A step that upgrades a document by exactly one version.
pub type Migration = fn(&mut Value) -> Result<(), String>;

/// `MIGRATIONS[i]` upgrades version `i + 1` to `i + 2`. Version 1 is the
/// first released format, so the list starts empty.
pub const MIGRATIONS: &[Migration] = &[];

/// Upgrades `doc` from version `from` to [`CURRENT_VERSION`].
pub fn migrate(doc: &mut Value, from: u64) -> Result<(), FileError> {
    migrate_with(doc, from, CURRENT_VERSION, MIGRATIONS)
}

/// Upgrades `doc` from `from` to `to` using `steps` (`steps[i]` upgrades
/// version `i + 1`). Exposed so the chain logic can be tested with sample
/// migrations before real ones exist.
pub fn migrate_with(doc: &mut Value, from: u64, to: u64, steps: &[Migration]) -> Result<(), FileError> {
    if from == 0 {
        return Err(FileError::Migration("version 0 does not exist".into()));
    }
    for v in from..to {
        let step = steps.get((v - 1) as usize).ok_or_else(|| FileError::Migration(format!("no upgrade from version {v}")))?;
        step(doc).map_err(|e| FileError::Migration(format!("version {v} to {}: {e}", v + 1)))?;
        let written = doc.get("version").and_then(Value::as_u64);
        if written != Some(v + 1) {
            return Err(FileError::Migration(format!("upgrade from version {v} did not set version {}", v + 1)));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Sample migrations showing the intended shape of real ones.

    /// v1 → v2: `canvas_size: [w, h]` became `width` and `height`.
    fn migrate_v1_to_v2(doc: &mut Value) -> Result<(), String> {
        let settings = doc.pointer_mut("/project/settings").ok_or("missing settings")?;
        if let Some(size) = settings.as_object_mut().and_then(|o| o.remove("canvas_size")) {
            let w = size.get(0).cloned().ok_or("bad canvas_size")?;
            let h = size.get(1).cloned().ok_or("bad canvas_size")?;
            settings["width"] = w;
            settings["height"] = h;
        }
        doc["version"] = json!(2);
        Ok(())
    }

    /// v2 → v3: lanes' `mute` was renamed `muted`.
    fn migrate_v2_to_v3(doc: &mut Value) -> Result<(), String> {
        for seq in doc.pointer_mut("/project/sequences").and_then(Value::as_array_mut).ok_or("missing sequences")? {
            for lane in seq["lanes"].as_array_mut().ok_or("missing lanes")? {
                if let Some(m) = lane.as_object_mut().and_then(|o| o.remove("mute")) {
                    lane["muted"] = m;
                }
            }
        }
        doc["version"] = json!(3);
        Ok(())
    }

    fn v1_doc() -> Value {
        json!({
            "format": "studio-project",
            "version": 1,
            "project": {
                "settings": {"canvas_size": [1280, 720]},
                "sequences": [{"lanes": [{"id": 2, "mute": true}]}]
            }
        })
    }

    #[test]
    fn runs_each_step_in_order() {
        let steps: &[Migration] = &[migrate_v1_to_v2, migrate_v2_to_v3];
        let mut doc = v1_doc();
        migrate_with(&mut doc, 1, 3, steps).unwrap();
        assert_eq!(doc["version"], 3);
        assert_eq!(doc["project"]["settings"]["width"], 1280);
        assert_eq!(doc["project"]["settings"]["height"], 720);
        assert_eq!(doc["project"]["sequences"][0]["lanes"][0]["muted"], true);

        // Starting part-way only runs the remaining steps.
        let mut doc2 = v1_doc();
        migrate_v1_to_v2(&mut doc2).unwrap();
        migrate_with(&mut doc2, 2, 3, steps).unwrap();
        assert_eq!(doc2, doc);
    }

    #[test]
    fn missing_or_sloppy_steps_fail() {
        let mut doc = v1_doc();
        assert!(migrate_with(&mut doc, 1, 3, &[migrate_v1_to_v2]).is_err());
        fn forgets_version(_: &mut Value) -> Result<(), String> {
            Ok(())
        }
        assert!(migrate_with(&mut v1_doc(), 1, 2, &[forgets_version]).is_err());
        assert!(migrate_with(&mut v1_doc(), 0, 1, &[]).is_err());
    }

    #[test]
    fn current_version_needs_no_steps() {
        assert_eq!(MIGRATIONS.len() as u64, CURRENT_VERSION - 1);
        let mut doc = v1_doc();
        migrate(&mut doc, CURRENT_VERSION).unwrap();
        assert_eq!(doc, v1_doc());
    }
}
