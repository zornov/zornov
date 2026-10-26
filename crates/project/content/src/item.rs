// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Library index types.
//!
//! An [`Item`] is one finished asset: a sticker, an emoji, a sound, a font
//! family, a motion clip. Items live in an [`Index`] rooted at a folder on
//! disk; `Item::file` and the other paths are relative to that root so an
//! index can be written to `index.json` and loaded from wherever the pack
//! ends up. A [`Library`] is the set of indexes the app has open (the core
//! bundle plus any downloaded packs) and searches across all of them.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// What an item is, which decides where it shows in the interface and how
/// it is thumbnailed, gated and packaged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Sticker,
    Emoji,
    Shape,
    TextArt,
    Frame,
    Overlay,
    Background,
    Photo,
    GreenScreen,
    Motion,
    Sound,
    Music,
    Font,
    Look,
    /// A 3D model the person or the AI made and kept (`.smesh` with a
    /// `thumb.png` beside it).
    Model,
}

impl Kind {
    pub const ALL: [Kind; 15] = [
        Kind::Sticker,
        Kind::Emoji,
        Kind::Shape,
        Kind::TextArt,
        Kind::Frame,
        Kind::Overlay,
        Kind::Background,
        Kind::Photo,
        Kind::GreenScreen,
        Kind::Motion,
        Kind::Sound,
        Kind::Music,
        Kind::Font,
        Kind::Look,
        Kind::Model,
    ];

    /// Sentence-case label for the interface.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Sticker => "Sticker",
            Kind::Emoji => "Emoji",
            Kind::Shape => "Shape",
            Kind::TextArt => "Text art",
            Kind::Frame => "Frame",
            Kind::Overlay => "Overlay",
            Kind::Background => "Background",
            Kind::Photo => "Photo",
            Kind::GreenScreen => "Green screen",
            Kind::Motion => "Motion",
            Kind::Sound => "Sound",
            Kind::Music => "Music",
            Kind::Font => "Font",
            Kind::Look => "Look",
            Kind::Model => "Model",
        }
    }

    /// Stable folder name used inside a pack.
    pub fn folder(self) -> &'static str {
        match self {
            Kind::Sticker => "stickers",
            Kind::Emoji => "emoji",
            Kind::Shape => "shapes",
            Kind::TextArt => "text-art",
            Kind::Frame => "frames",
            Kind::Overlay => "overlays",
            Kind::Background => "backgrounds",
            Kind::Photo => "photos",
            Kind::GreenScreen => "green-screen",
            Kind::Motion => "motion",
            Kind::Sound => "sounds",
            Kind::Music => "music",
            Kind::Font => "fonts",
            Kind::Look => "looks",
            Kind::Model => "models",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        Kind::ALL.iter().copied().find(|k| k.folder() == s || k.label().eq_ignore_ascii_case(s))
    }

    /// A still picture (raster or vector).
    pub fn is_image(self) -> bool {
        matches!(
            self,
            Kind::Sticker
                | Kind::Emoji
                | Kind::Shape
                | Kind::TextArt
                | Kind::Frame
                | Kind::Overlay
                | Kind::Background
                | Kind::Photo
                | Kind::GreenScreen
        )
    }

    /// A picture on transparency whose edge quality matters (cut-outs).
    pub fn is_cutout(self) -> bool {
        matches!(self, Kind::Sticker | Kind::Emoji | Kind::Shape | Kind::TextArt | Kind::GreenScreen)
    }

    pub fn is_audio(self) -> bool {
        matches!(self, Kind::Sound | Kind::Music)
    }

    pub fn is_video(self) -> bool {
        matches!(self, Kind::Motion)
    }

    /// Items of this kind arrive on a green screen and are keyed on import.
    pub fn is_green_screen(self) -> bool {
        matches!(self, Kind::GreenScreen | Kind::Motion)
    }
}

/// The licences Studio content may carry. Anything else is rejected at
/// load time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Licence {
    /// Made for Studio (our own, generated under the owner's plan).
    Studio,
    CcBy4,
    /// ShareAlike: kept in its own clearly marked set.
    CcBySa4,
    Cc0,
    Mit,
    Ofl11,
    Apache2,
}

impl Licence {
    pub const ALL: [Licence; 7] =
        [Licence::Studio, Licence::CcBy4, Licence::CcBySa4, Licence::Cc0, Licence::Mit, Licence::Ofl11, Licence::Apache2];

    /// Parses an SPDX identifier as used in `catalog.json`.
    pub fn parse(spdx: &str) -> Option<Licence> {
        Some(match spdx.trim() {
            "Studio" | "studio" => Licence::Studio,
            "CC-BY-4.0" => Licence::CcBy4,
            "CC-BY-SA-4.0" => Licence::CcBySa4,
            "CC0-1.0" => Licence::Cc0,
            "MIT" => Licence::Mit,
            "OFL-1.1" => Licence::Ofl11,
            "Apache-2.0" => Licence::Apache2,
            _ => return None,
        })
    }

    pub fn spdx(self) -> &'static str {
        match self {
            Licence::Studio => "Studio",
            Licence::CcBy4 => "CC-BY-4.0",
            Licence::CcBySa4 => "CC-BY-SA-4.0",
            Licence::Cc0 => "CC0-1.0",
            Licence::Mit => "MIT",
            Licence::Ofl11 => "OFL-1.1",
            Licence::Apache2 => "Apache-2.0",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Licence::Studio => "Made for Studio",
            Licence::CcBy4 => "Creative Commons Attribution 4.0 International",
            Licence::CcBySa4 => "Creative Commons Attribution-ShareAlike 4.0 International",
            Licence::Cc0 => "CC0 1.0 Universal (Public Domain Dedication)",
            Licence::Mit => "MIT License",
            Licence::Ofl11 => "SIL Open Font License 1.1",
            Licence::Apache2 => "Apache License 2.0",
        }
    }

    pub fn url(self) -> &'static str {
        match self {
            Licence::Studio => "https://www.apache.org/licenses/LICENSE-2.0",
            Licence::CcBy4 => "https://creativecommons.org/licenses/by/4.0/",
            Licence::CcBySa4 => "https://creativecommons.org/licenses/by-sa/4.0/",
            Licence::Cc0 => "https://creativecommons.org/publicdomain/zero/1.0/",
            Licence::Mit => "https://opensource.org/license/mit",
            Licence::Ofl11 => "https://openfontlicense.org/open-font-license-official-text/",
            Licence::Apache2 => "https://www.apache.org/licenses/LICENSE-2.0",
        }
    }

    /// Derivatives must carry the same licence.
    pub fn share_alike(self) -> bool {
        matches!(self, Licence::CcBySa4)
    }

    /// The licence requires credit to be shown where the content is used.
    pub fn needs_credit(self) -> bool {
        matches!(self, Licence::CcBy4 | Licence::CcBySa4)
    }

    pub fn is_third_party(self) -> bool {
        !matches!(self, Licence::Studio)
    }
}

/// Where an item came from (one entry per upstream project).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Source {
    /// Short stable id: `made`, `twemoji`, `openmoji`, `fluent-emoji`,
    /// `kenney`, `tabler`, `google-fonts`, `incompetech`, `opengameart`.
    pub id: String,
    /// Display name for the Licences screen.
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
    /// Version, tag or commit of the upstream release, when known.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub version: String,
}

impl Source {
    pub fn new(id: &str, name: &str, url: &str, version: &str) -> Source {
        Source { id: id.into(), name: name.into(), url: url.into(), version: version.into() }
    }

    pub fn studio() -> Source {
        Source::new("made", "Made for Studio", "", "")
    }
}

/// One file of a font family.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FontFile {
    pub file: PathBuf,
    /// `normal` or `italic`.
    pub style: String,
    pub weight: u16,
    /// Variable-font axes (`wght`, `wdth`, ...), empty for static files.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub axes: Vec<String>,
}

/// Chroma-key settings that keyed an item on import. Stored with the item
/// so the inspector can show and edit them.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct KeySettings {
    /// Detected key colour (sRGB).
    pub key: [u8; 3],
    /// How far from the key colour still counts as background, 0..1.
    pub tolerance: f32,
    /// Width of the soft edge, 0..1.
    pub softness: f32,
    /// Spill suppression strength, 0..1.
    pub spill: f32,
}

impl Default for KeySettings {
    fn default() -> Self {
        KeySettings { key: [0, 255, 0], tolerance: 0.5, softness: 0.3, spill: 1.0 }
    }
}

/// Per-kind extras. Everything is optional and left out of `index.json`
/// when empty.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Details {
    /// Suggested blend mode for overlays (`screen`, `add`, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blend: Option<String>,
    /// Same picture without the sticker outline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plain_variant: Option<PathBuf>,
    /// Filled version of an outline shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filled_variant: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_secs: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bpm: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    /// The Unicode text of an emoji.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub font_files: Vec<FontFile>,
    /// Designer or author shown on the Licences screen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Licence text file shipped next to the item (relative to the root).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub licence_file: Option<PathBuf>,
    /// Upstream page for this exact item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    /// Chroma key applied on import (green-screen items).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<KeySettings>,
    /// Pixel size of a raster still, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// The item was resized or re-encoded for Studio (a modification that
    /// CC BY attribution has to mention).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub modified: bool,
}

/// One library item.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    /// Stable internal id, never shown to the user.
    pub id: String,
    /// Short human name ("Golden Retriever 03").
    pub name: String,
    pub kind: Kind,
    /// Sentence-case grouping shown in the panel ("Animals & nature").
    pub category: String,
    /// The set the item belongs to ("Animal Memes", "Twemoji").
    pub set: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Main file, relative to the index root.
    pub file: PathBuf,
    /// Optional ready-made preview picture, relative to the index root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<PathBuf>,
    pub licence: Licence,
    /// Exact credit line as the licence requires it, verbatim.
    pub attribution: String,
    pub source: Source,
    #[serde(default, skip_serializing_if = "is_default_details")]
    pub details: Details,
}

fn is_default_details(d: &Details) -> bool {
    *d == Details::default()
}

impl Item {
    /// The main file is a vector picture (SVG).
    pub fn is_vector(&self) -> bool {
        self.file.extension().is_some_and(|e| e.eq_ignore_ascii_case("svg"))
    }

    pub fn extension(&self) -> String {
        self.file.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
    }
}

/// A set of items under one root folder.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Index {
    pub root: PathBuf,
    pub items: Vec<Item>,
}

impl Index {
    pub fn new(root: impl Into<PathBuf>) -> Index {
        Index { root: root.into(), items: Vec::new() }
    }

    /// Absolute path of a path stored in an item.
    pub fn path(&self, rel: &Path) -> PathBuf {
        self.root.join(rel)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn by_kind(&self, kind: Kind) -> impl Iterator<Item = &Item> {
        self.items.iter().filter(move |i| i.kind == kind)
    }

    pub fn get(&self, id: &str) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    /// Categories of a kind, sorted.
    pub fn categories(&self, kind: Kind) -> Vec<String> {
        self.by_kind(kind).map(|i| i.category.clone()).collect::<BTreeSet<_>>().into_iter().collect()
    }

    /// Sets of a kind, sorted.
    pub fn sets(&self, kind: Kind) -> Vec<String> {
        self.by_kind(kind).map(|i| i.set.clone()).collect::<BTreeSet<_>>().into_iter().collect()
    }

    pub fn search(&self, query: &str) -> Vec<crate::search::Hit<'_>> {
        crate::search::search(std::iter::once(self), query)
    }

    /// Appends another index with the same root.
    pub fn extend(&mut self, other: Index) {
        self.items.extend(other.items);
    }
}

/// Every index the app has open.
#[derive(Clone, Debug, Default)]
pub struct Library {
    pub indexes: Vec<Index>,
}

impl Library {
    pub fn new() -> Library {
        Library::default()
    }

    pub fn add(&mut self, index: Index) {
        self.indexes.push(index);
    }

    pub fn items(&self) -> impl Iterator<Item = (&Index, &Item)> {
        self.indexes.iter().flat_map(|ix| ix.items.iter().map(move |it| (ix, it)))
    }

    pub fn len(&self) -> usize {
        self.indexes.iter().map(|ix| ix.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn search(&self, query: &str) -> Vec<crate::search::Hit<'_>> {
        crate::search::search(self.indexes.iter(), query)
    }

    /// Items of a kind across every index, with their resolved paths.
    pub fn by_kind(&self, kind: Kind) -> Vec<(&Index, &Item)> {
        self.items().filter(|(_, i)| i.kind == kind).collect()
    }

    /// All sources and licences in use, for the Licences screen.
    pub fn credits(&self) -> Vec<(Source, Licence)> {
        let mut set: BTreeSet<(Source, Licence)> = BTreeSet::new();
        for (_, it) in self.items() {
            set.insert((it.source.clone(), it.licence));
        }
        set.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_round_trips_through_folder_names() {
        for k in Kind::ALL {
            assert_eq!(Kind::parse(k.folder()), Some(k));
            assert_eq!(Kind::parse(k.label()), Some(k));
        }
    }

    #[test]
    fn licence_parse_matches_spdx() {
        for l in Licence::ALL {
            assert_eq!(Licence::parse(l.spdx()), Some(l));
        }
        assert_eq!(Licence::parse("GPL-3.0"), None);
        assert!(Licence::CcBySa4.share_alike());
        assert!(!Licence::Cc0.needs_credit());
    }

    #[test]
    fn item_serialises_without_empty_details() {
        let it = Item {
            id: "x".into(),
            name: "Thing 01".into(),
            kind: Kind::Sticker,
            category: "Things".into(),
            set: "Things".into(),
            tags: vec![],
            file: "stickers/Things/Thing 01.png".into(),
            preview: None,
            licence: Licence::Studio,
            attribution: "Made for Studio".into(),
            source: Source::studio(),
            details: Details::default(),
        };
        let s = serde_json::to_string(&it).unwrap();
        assert!(!s.contains("details"));
        assert!(!s.contains("tags"));
        let back: Item = serde_json::from_str(&s).unwrap();
        assert_eq!(back, it);
    }
}
