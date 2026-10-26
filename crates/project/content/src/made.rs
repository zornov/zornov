// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Loader for our own assets (the `made` folder).
//!
//! The folder carries a `manifest.json` listing every still (name, set,
//! kind, category, file, optional outline-free variant and blend mode) and
//! `Motion/manifest.json` listing the green-screen motion clips.

use crate::item::{Details, Index, Item, Kind, Licence, Source};
use crate::naming::{sentence_case, title_case};
use crate::search::tokens;
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Credit line for everything we made.
pub const ATTRIBUTION: &str = "Made for Zornov";

#[derive(Deserialize)]
struct Manifest {
    #[serde(default)]
    source: String,
    items: Vec<MadeItem>,
}

#[derive(Deserialize)]
struct MadeItem {
    name: String,
    set: String,
    kind: String,
    #[serde(default)]
    category: String,
    file: String,
    #[serde(default)]
    variant_no_outline: Option<String>,
    #[serde(default)]
    blend: Option<String>,
}

#[derive(Deserialize)]
struct MotionItem {
    name: String,
    set: String,
    file: String,
    #[serde(default)]
    category: String,
}

fn kind_of(s: &str) -> Option<Kind> {
    Some(match s {
        "sticker" => Kind::Sticker,
        "emoji" => Kind::Emoji,
        "text_art" | "text-art" => Kind::TextArt,
        "frame" => Kind::Frame,
        "green_screen" | "green-screen" => Kind::GreenScreen,
        "overlay" => Kind::Overlay,
        "background" => Kind::Background,
        "photo" | "sample" => Kind::Photo,
        "motion" => Kind::Motion,
        _ => return None,
    })
}

/// Stable id from a relative path: lower-case, non-alphanumerics folded to
/// single dashes.
pub fn slug(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut dash = false;
    for c in s.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    out.trim_end_matches('-').to_string()
}

/// Search tags for a set name and kind.
fn tags_for(set: &str, kind: Kind, extra: &[&str]) -> Vec<String> {
    let mut t: Vec<String> = tokens(set);
    t.extend(tokens(kind.label()));
    t.extend(extra.iter().map(|s| s.to_string()));
    t.sort();
    t.dedup();
    t
}

/// Loads the made-for-Studio index rooted at `root`.
pub fn load(root: &Path) -> Result<Index, String> {
    let source = Source::studio();
    let mut index = Index::new(root);
    let manifest_path = root.join("manifest.json");
    let text = std::fs::read_to_string(&manifest_path).map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    let manifest: Manifest = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    let _ = manifest.source; // provenance note; the licence is ours either way
    for m in manifest.items {
        let kind = kind_of(&m.kind).ok_or_else(|| format!("{}: unknown kind {:?}", m.file, m.kind))?;
        let set = title_case(&m.set);
        let category = sentence_case(if m.category.is_empty() { &m.set } else { &m.category });
        let extra: Vec<&str> = match kind {
            Kind::GreenScreen => vec!["green", "screen", "keyed", "transparent"],
            Kind::Overlay => vec!["light", "texture"],
            Kind::Photo => vec!["sample", "stock"],
            _ => vec![],
        };
        index.items.push(Item {
            id: format!("made:{}", slug(&m.file)),
            name: m.name.trim().to_string(),
            kind,
            category,
            set,
            tags: tags_for(&m.set, kind, &extra),
            file: PathBuf::from(&m.file),
            preview: None,
            licence: Licence::Studio,
            attribution: ATTRIBUTION.into(),
            source: source.clone(),
            details: Details {
                blend: m.blend,
                plain_variant: m.variant_no_outline.map(PathBuf::from),
                ..Details::default()
            },
        });
    }
    let motion_path = root.join("Motion").join("manifest.json");
    if motion_path.exists() {
        let text = std::fs::read_to_string(&motion_path).map_err(|e| format!("{}: {e}", motion_path.display()))?;
        let items: Vec<MotionItem> = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", motion_path.display()))?;
        for m in items {
            let set = title_case(&m.set);
            let _ = m.category; // always "motion"
            index.items.push(Item {
                id: format!("made:{}", slug(&m.file)),
                name: m.name.trim().to_string(),
                kind: Kind::Motion,
                category: sentence_case(&m.set),
                set,
                tags: tags_for(&m.set, Kind::Motion, &["green", "screen", "clip", "video", "loop"]),
                file: PathBuf::from(&m.file),
                preview: None,
                licence: Licence::Studio,
                attribution: ATTRIBUTION.into(),
                source: source.clone(),
                details: Details::default(),
            });
        }
    }
    Ok(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_a_manifest_and_motion_list() {
        let dir = std::env::temp_dir().join(format!("studio-made-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Motion")).unwrap();
        std::fs::write(
            dir.join("manifest.json"),
            r#"{"source":"x","items":[
                {"name":"Animal Memes 001","set":"Animal Memes","kind":"sticker","category":"Animal Memes","file":"Stickers/Animal Memes/Animal Memes 001.png","variant_no_outline":"Stickers/Animal Memes/Plain/Animal Memes 001.png"},
                {"name":"Bokeh Sparkle 24","set":"Bokeh Sparkle","kind":"overlay","category":"Bokeh Sparkle","file":"Overlays/Bokeh Sparkle/Bokeh Sparkle 24.png","blend":"screen"},
                {"name":"Butterflies 01","set":"Butterflies","kind":"green_screen","category":"Butterflies","file":"Green Screen/Butterflies/Butterflies 01.png"}
            ]}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("Motion/manifest.json"),
            r#"[{"name":"Confetti Loop 1","set":"Confetti Loop","file":"Motion/Confetti Loop/Confetti Loop 1.mp4","category":"motion","note":"green screen video - key in the app"}]"#,
        )
        .unwrap();
        let ix = load(&dir).unwrap();
        assert_eq!(ix.len(), 4);
        let s = &ix.items[0];
        assert_eq!(s.kind, Kind::Sticker);
        assert_eq!(s.category, "Animal memes");
        assert_eq!(s.set, "Animal Memes");
        assert_eq!(s.id, "made:stickers-animal-memes-animal-memes-001-png");
        assert!(s.tags.contains(&"memes".to_string()) && s.tags.contains(&"sticker".to_string()));
        assert_eq!(s.details.plain_variant.as_deref(), Some(Path::new("Stickers/Animal Memes/Plain/Animal Memes 001.png")));
        assert_eq!(ix.items[1].details.blend.as_deref(), Some("screen"));
        assert_eq!(ix.items[2].kind, Kind::GreenScreen);
        assert_eq!(ix.items[3].kind, Kind::Motion);
        assert_eq!(ix.path(&ix.items[3].file), dir.join("Motion/Confetti Loop/Confetti Loop 1.mp4"));
        assert!(ix.search("confetti").iter().any(|h| h.item.kind == Kind::Motion));
        assert!(crate::naming::audit(&ix).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn slug_folds() {
        assert_eq!(slug("Stickers/Animal Memes/Animal Memes 001.png"), "stickers-animal-memes-animal-memes-001-png");
        assert_eq!(slug("  --x "), "x");
    }
}
