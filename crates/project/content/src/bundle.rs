// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Reading and writing built packs.
//!
//! A built pack folder holds `index.json` (schema, pack spec, items with
//! paths relative to the folder), the files, and `LICENSES.md`. The app
//! opens the core bundle plus every pack under `packs/` into a [`Library`].

use crate::item::{Index, Item, Library};
use crate::pack::{Pack, PackSpec};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::Path;

pub const SCHEMA: &str = "studio-content/1";
pub const INDEX_FILE: &str = "index.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BundleIndex {
    pub schema: String,
    pub pack: PackSpec,
    #[serde(default)]
    pub bytes: u64,
    pub items: Vec<Item>,
}

/// Writes `index.json` for a pack into `dir` (files are written by the pack
/// tool).
pub fn write_index(dir: &Path, pack: &Pack) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let idx = BundleIndex { schema: SCHEMA.into(), pack: pack.spec.clone(), bytes: pack.bytes, items: pack.items.clone() };
    // Compact: the core index is several megabytes and counts against the
    // bundle's size budget.
    let text = serde_json::to_string(&idx).map_err(io::Error::other)?;
    let tmp = dir.join("index.json.part");
    fs::write(&tmp, text)?;
    fs::rename(tmp, dir.join(INDEX_FILE))
}

/// Loads one pack folder.
pub fn load(dir: &Path) -> Result<(PackSpec, Index), String> {
    let path = dir.join(INDEX_FILE);
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let idx: BundleIndex = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    if idx.schema != SCHEMA {
        return Err(format!("{}: unsupported schema {:?}", path.display(), idx.schema));
    }
    Ok((idx.pack, Index { root: dir.to_path_buf(), items: idx.items }))
}

/// Loads `content_dir/core` and every `content_dir/packs/*` that has an index.
/// Missing packs are skipped; a broken index is an error.
pub fn load_all(content_dir: &Path) -> Result<Library, String> {
    let mut lib = Library::new();
    let core = content_dir.join("core");
    if core.join(INDEX_FILE).exists() {
        lib.add(load(&core)?.1);
    }
    let packs = content_dir.join("packs");
    if let Ok(rd) = fs::read_dir(&packs) {
        let mut dirs: Vec<_> = rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.join(INDEX_FILE).exists()).collect();
        dirs.sort();
        for d in dirs {
            lib.add(load(&d)?.1);
        }
    }
    Ok(lib)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::{Details, Kind, Licence, Source};

    #[test]
    fn round_trips_a_pack_folder() {
        let dir = std::env::temp_dir().join(format!("studio-bundle-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut pack = Pack::new(PackSpec { id: "core".into(), name: "Core".into(), description: String::new(), core: true, share_alike: false });
        pack.items.push(Item {
            id: "made:x".into(),
            name: "Thing 01".into(),
            kind: Kind::Sticker,
            category: "Things".into(),
            set: "Things".into(),
            tags: vec!["thing".into()],
            file: "stickers/Things/Thing 01.png".into(),
            preview: None,
            licence: Licence::Studio,
            attribution: "Made for Studio".into(),
            source: Source::studio(),
            details: Details::default(),
        });
        pack.bytes = 123;
        write_index(&dir.join("core"), &pack).unwrap();
        write_index(&dir.join("packs/extra"), &pack).unwrap();
        fs::create_dir_all(dir.join("packs/empty")).unwrap();
        let lib = load_all(&dir).unwrap();
        assert_eq!(lib.indexes.len(), 2);
        assert_eq!(lib.len(), 2);
        let hit = &lib.search("thing")[0];
        assert_eq!(hit.path(), dir.join("core/stickers/Things/Thing 01.png"));
        let (spec, _) = load(&dir.join("core")).unwrap();
        assert!(spec.core);
        fs::write(dir.join("packs/extra/index.json"), "{}").unwrap();
        assert!(load_all(&dir).is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
