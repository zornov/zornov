// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Font discovery, matching and fallback.
//!
//! Three sources feed one library: the system's fonts (through `fontdb`),
//! Studio's bundled open-licence library (read from the content folder's
//! `catalog.json`, so the 15 000 files there are never parsed up front)
//! and files the user imports. Face data is read lazily and cached for the
//! life of the library.
//!
//! Matching follows the CSS font-matching idea (family, then nearest weight,
//! then style) with synthetic bold/italic when a family lacks the face.
//! Fallback goes script by script: a character the chosen face cannot show
//! is tried against a chain of families for its script, then the general
//! chain, then emoji fonts.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rustybuzz::ttf_parser;
use unicode_script::Script;

use crate::emoji::{EmojiSet, EmojiStyle};

/// Where a face came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FontSource {
    System,
    Bundled,
    User,
}

/// Index of a face in a [`FontLibrary`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FaceId(pub u32);

/// What the library knows about a face before loading it.
#[derive(Clone, Debug)]
pub struct FaceInfo {
    pub id: FaceId,
    /// Typographic family ("Inter", "Noto Sans Arabic").
    pub family: String,
    /// CSS weight 100..900 of this file; variable fonts report their default.
    pub weight: u16,
    pub italic: bool,
    pub path: PathBuf,
    pub index: u32,
    pub source: FontSource,
    /// Whether the file has a `wght` variation axis (known for bundled
    /// fonts from the catalogue; learnt on load for the rest).
    pub variable_weight: bool,
    /// Catalogue category for bundled fonts ("Sans", "Serif", "Display",
    /// "Handwriting", "Monospace"), empty otherwise.
    pub category: String,
    /// SPDX licence id for bundled fonts.
    pub licence: String,
    /// Attribution line for bundled fonts.
    pub attribution: String,
    /// Unicode subsets the catalogue lists ("latin", "arabic", "korean" …).
    pub subsets: Vec<String>,
    /// Fixed-width (every glyph the same advance), as the font file says.
    pub monospaced: bool,
    /// Narrower than normal (condensed, narrow), as the font file says.
    pub condensed: bool,
    /// The catalogue's own classifications for bundled fonts ("DISPLAY",
    /// "HANDWRITING", "MONOSPACE", "SYMBOLS"), empty otherwise.
    pub classifications: Vec<String>,
}

/// A face with its bytes in memory.
pub struct LoadedFace {
    pub info: FaceInfo,
    pub data: Arc<Vec<u8>>,
    pub units_per_em: f32,
    /// Typographic metrics in font units.
    pub ascent: f32,
    pub descent: f32,
    pub line_gap: f32,
    pub cap_height: f32,
    pub x_height: f32,
    pub underline_position: f32,
    pub underline_thickness: f32,
    pub strikeout_position: f32,
    pub strikeout_thickness: f32,
    /// Range of the `wght` axis when the font is variable.
    pub weight_range: Option<(f32, f32)>,
    pub has_colour: bool,
}

impl LoadedFace {
    /// A parsed `ttf-parser` view (cheap: tables are located, not copied).
    pub fn ttf(&self) -> ttf_parser::Face<'_> {
        // The data was validated when the face was loaded.
        ttf_parser::Face::parse(&self.data, self.info.index).expect("face validated on load")
    }

    /// A shaping face. Also cheap to construct; built per shaping call.
    pub fn shaper(&self) -> rustybuzz::Face<'_> {
        rustybuzz::Face::from_face(self.ttf())
    }

    pub fn swash(&self) -> swash::FontRef<'_> {
        swash::FontRef::from_index(&self.data, self.info.index as usize).expect("face validated on load")
    }

    pub fn has_glyph(&self, c: char) -> bool {
        self.ttf().glyph_index(c).is_some()
    }

    /// Glyph id for a character, if the face has one.
    pub fn glyph_index(&self, c: char) -> Option<u16> {
        self.ttf().glyph_index(c).map(|g| g.0)
    }
}

impl std::fmt::Debug for LoadedFace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "LoadedFace({} {} {})", self.info.family, self.info.weight, if self.info.italic { "italic" } else { "" })
    }
}

/// A family request from a style.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FontRequest {
    pub family: String,
    /// CSS weight 100..900.
    #[serde(default = "default_weight")]
    pub weight: u16,
    #[serde(default)]
    pub italic: bool,
}

fn default_weight() -> u16 {
    400
}

impl Default for FontRequest {
    fn default() -> Self {
        FontRequest { family: "Inter".into(), weight: 400, italic: false }
    }
}

impl FontRequest {
    pub fn new(family: &str, weight: u16, italic: bool) -> FontRequest {
        FontRequest { family: family.into(), weight, italic }
    }

    pub fn regular(family: &str) -> FontRequest {
        FontRequest::new(family, 400, false)
    }

    pub fn bold(family: &str) -> FontRequest {
        FontRequest::new(family, 700, false)
    }
}

/// A resolved face plus the adjustments needed to honour the request.
#[derive(Clone)]
pub struct FontMatch {
    pub face: Arc<LoadedFace>,
    /// `wght` axis value to set, for variable fonts.
    pub weight_variation: Option<f32>,
    /// Embolden outlines because no bold face exists.
    pub synthetic_bold: bool,
    /// Skew outlines because no italic face exists.
    pub synthetic_italic: bool,
}

impl FontMatch {
    pub fn id(&self) -> FaceId {
        self.face.info.id
    }

    /// Rustybuzz variations to apply when shaping.
    pub fn variations(&self) -> Vec<rustybuzz::Variation> {
        self.weight_variation
            .map(|w| vec![rustybuzz::Variation { tag: rustybuzz::ttf_parser::Tag::from_bytes(b"wght"), value: w }])
            .unwrap_or_default()
    }
}

impl std::fmt::Debug for FontMatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FontMatch({:?} wght={:?} sb={} si={})", self.face, self.weight_variation, self.synthetic_bold, self.synthetic_italic)
    }
}

/// The default content folder.
pub fn default_content_dir() -> PathBuf {
    if let Ok(p) = std::env::var("STUDIO_CONTENT_DIR") {
        return PathBuf::from(p);
    }
    studio_paths::studio().join("Content")
}

/// All known faces and a cache of the loaded ones.
pub struct FontLibrary {
    faces: Vec<FaceInfo>,
    by_family: HashMap<String, Vec<usize>>,
    loaded: Mutex<HashMap<FaceId, Arc<LoadedFace>>>,
    files: Mutex<HashMap<PathBuf, Arc<Vec<u8>>>>,
    /// Per (face, char) coverage answers, because fallback asks often.
    coverage: Mutex<HashMap<(FaceId, char), bool>>,
    /// Families that preferentially take over for a script.
    script_chains: HashMap<Script, Vec<String>>,
    general_chain: Vec<String>,
    emoji_chain: Vec<String>,
    /// Family for generic names ("sans-serif", "serif", "monospace", "cursive", "display").
    generic: HashMap<String, String>,
    emoji: Option<EmojiSet>,
    /// [`FontLibrary::families_shared`], built on first ask and dropped when a face is added.
    families_cache: std::sync::OnceLock<Arc<[String]>>,
}

impl Default for FontLibrary {
    fn default() -> Self {
        Self::new()
    }
}

impl FontLibrary {
    /// An empty library. Use [`FontLibrary::discover`] for the usual setup.
    pub fn new() -> FontLibrary {
        let mut lib = FontLibrary {
            faces: Vec::new(),
            families_cache: std::sync::OnceLock::new(),
            by_family: HashMap::new(),
            loaded: Mutex::new(HashMap::new()),
            files: Mutex::new(HashMap::new()),
            coverage: Mutex::new(HashMap::new()),
            script_chains: HashMap::new(),
            general_chain: vec![
                "Noto Sans".into(),
                "Inter".into(),
                "Noto Sans Symbols 2".into(),
                "Noto Sans Symbols".into(),
                "Noto Sans Math".into(),
                "Helvetica Neue".into(),
                "Apple Symbols".into(),
                "Arial Unicode MS".into(),
            ],
            emoji_chain: vec!["Noto Color Emoji".into(), "Apple Color Emoji".into(), "Noto Emoji".into()],
            generic: HashMap::new(),
            emoji: None,
        };
        let chains: &[(Script, &[&str])] = &[
            (Script::Arabic, &["Noto Sans Arabic", "IBM Plex Sans Arabic", "Geeza Pro"]),
            (Script::Hebrew, &["Noto Sans Hebrew", "IBM Plex Sans Hebrew", "Arial Hebrew"]),
            (Script::Devanagari, &["Noto Sans Devanagari", "IBM Plex Sans Devanagari", "Kohinoor Devanagari"]),
            (Script::Thai, &["Noto Sans Thai", "IBM Plex Sans Thai", "Thonburi"]),
            (Script::Han, &["Noto Sans SC", "Noto Sans JP", "Noto Sans KR", "PingFang SC", "Hiragino Sans"]),
            (Script::Hiragana, &["Noto Sans JP", "Hiragino Sans"]),
            (Script::Katakana, &["Noto Sans JP", "Hiragino Sans"]),
            (Script::Hangul, &["Noto Sans KR", "Apple SD Gothic Neo"]),
            (Script::Cyrillic, &["Noto Sans", "Inter", "Helvetica Neue"]),
            (Script::Greek, &["Noto Sans", "Inter", "Helvetica Neue"]),
            (Script::Bengali, &["Noto Sans Bengali", "Kohinoor Bangla"]),
            (Script::Tamil, &["Noto Sans Tamil", "Tamil Sangam MN"]),
            (Script::Telugu, &["Noto Sans Telugu", "Kohinoor Telugu"]),
            (Script::Gujarati, &["Noto Sans Gujarati", "Kohinoor Gujarati"]),
            (Script::Kannada, &["Noto Sans Kannada"]),
            (Script::Malayalam, &["Noto Sans Malayalam", "Malayalam Sangam MN"]),
            (Script::Georgian, &["Noto Sans Georgian"]),
            (Script::Armenian, &["Noto Sans Armenian"]),
            (Script::Ethiopic, &["Noto Sans Ethiopic", "Kefa"]),
            (Script::Khmer, &["Noto Sans Khmer", "Khmer Sangam MN"]),
            (Script::Lao, &["Noto Sans Lao", "Lao Sangam MN"]),
            (Script::Myanmar, &["Noto Sans Myanmar", "Myanmar Sangam MN"]),
            (Script::Sinhala, &["Noto Sans Sinhala", "Sinhala Sangam MN"]),
        ];
        for (s, names) in chains {
            lib.script_chains.insert(*s, names.iter().map(|n| n.to_string()).collect());
        }
        for (g, f) in [
            ("sans-serif", "Inter"),
            ("serif", "Playfair Display"),
            ("monospace", "JetBrains Mono"),
            ("cursive", "Caveat"),
            ("display", "Bebas Neue"),
        ] {
            lib.generic.insert(g.into(), f.into());
        }
        lib
    }

    /// System fonts plus the bundled library in the default content folder.
    /// Missing folders are skipped silently: the library then just has
    /// fewer faces.
    pub fn discover() -> FontLibrary {
        let mut lib = FontLibrary::new();
        let _ = lib.load_bundled(&default_content_dir());
        lib.load_system();
        lib
    }

    /// Only the bundled library (deterministic; used by tests).
    pub fn bundled_only() -> FontLibrary {
        let mut lib = FontLibrary::new();
        let _ = lib.load_bundled(&default_content_dir());
        lib
    }

    /// Registers every system font `fontdb` can find.
    pub fn load_system(&mut self) {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        for f in db.faces() {
            let path = match &f.source {
                fontdb::Source::File(p) => p.clone(),
                fontdb::Source::Binary(_) => continue,
            };
            let family = match f.families.first() {
                Some((n, _)) => n.clone(),
                None => continue,
            };
            // Hidden UI faces (".SF Compact" etc.) are not for titles.
            if family.starts_with('.') {
                continue;
            }
            self.push(FaceInfo {
                id: FaceId(0),
                family,
                weight: f.weight.0,
                italic: f.style != fontdb::Style::Normal,
                path,
                index: f.index,
                source: FontSource::System,
                variable_weight: false,
                category: String::new(),
                licence: String::new(),
                attribution: String::new(),
                subsets: Vec::new(),
                monospaced: f.monospaced,
                condensed: matches!(f.stretch, fontdb::Stretch::Condensed | fontdb::Stretch::ExtraCondensed | fontdb::Stretch::UltraCondensed | fontdb::Stretch::SemiCondensed),
                classifications: Vec::new(),
            });
        }
    }

    /// Registers the bundled library from `catalog.json` in `content_dir`.
    pub fn load_bundled(&mut self, content_dir: &Path) -> std::io::Result<usize> {
        if self.emoji.is_none() {
            self.emoji = EmojiSet::open(content_dir, EmojiStyle::OpenMoji);
        }
        let text = std::fs::read_to_string(content_dir.join("catalog.json"))?;
        let v: serde_json::Value = serde_json::from_str(&text).map_err(std::io::Error::other)?;
        let items = match v.pointer("/tabs/text/fonts/items").and_then(|i| i.as_object()) {
            Some(i) => i,
            None => return Ok(0),
        };
        let mut n = 0;
        let s = |v: Option<&serde_json::Value>| v.and_then(|x| x.as_str()).unwrap_or("").to_string();
        for item in items.values() {
            let family = s(item.get("family"));
            if family.is_empty() {
                continue;
            }
            let variable_weight = item
                .get("variable_axes")
                .and_then(|a| a.as_array())
                .map(|a| a.iter().any(|x| x.as_str() == Some("wght")))
                .unwrap_or(false);
            let subsets: Vec<String> = item
                .get("subsets")
                .and_then(|a| a.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                .unwrap_or_default();
            let category = s(item.get("category"));
            let classifications: Vec<String> = item.get("classifications").and_then(|a| a.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
            let licence = s(item.get("licence"));
            let attribution = s(item.get("attribution"));
            for file in item.get("files").and_then(|f| f.as_array()).into_iter().flatten() {
                let rel = s(file.get("path"));
                if rel.is_empty() {
                    continue;
                }
                let weight = file.get("weight").and_then(|w| w.as_u64()).unwrap_or(400) as u16;
                let italic = file.get("style").and_then(|x| x.as_str()) == Some("italic");
                self.push(FaceInfo {
                    id: FaceId(0),
                    family: family.clone(),
                    weight,
                    italic,
                    path: content_dir.join(rel),
                    index: 0,
                    source: FontSource::Bundled,
                    variable_weight,
                    category: category.clone(),
                    licence: licence.clone(),
                    attribution: attribution.clone(),
                    subsets: subsets.clone(),
                    monospaced: category.eq_ignore_ascii_case("monospace") || classifications.iter().any(|c| c.eq_ignore_ascii_case("monospace")),
                    condensed: family.to_lowercase().contains("condensed") || family.to_lowercase().contains("narrow"),
                    classifications: classifications.clone(),
                });
                n += 1;
            }
        }
        Ok(n)
    }

    /// Registers a user font file (every face in a collection). Returns the
    /// new ids.
    pub fn load_file(&mut self, path: &Path) -> std::io::Result<Vec<FaceId>> {
        let data = std::fs::read(path)?;
        let count = ttf_parser::fonts_in_collection(&data).unwrap_or(1);
        let mut ids = Vec::new();
        for index in 0..count {
            let face = match ttf_parser::Face::parse(&data, index) {
                Ok(f) => f,
                Err(_) => continue,
            };
            // The typographic family ("Inter") groups a family's weights;
            // the plain family name of a static file is often weight-
            // specific ("Inter SemiBold"), so it is only the second choice.
            let name_of = |id: u16| face.names().into_iter().filter(|n| n.name_id == id).find_map(|n| n.to_string());
            let family = name_of(ttf_parser::name_id::TYPOGRAPHIC_FAMILY)
                .or_else(|| name_of(ttf_parser::name_id::FAMILY))
                .unwrap_or_else(|| path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
            let variable_weight = face.variation_axes().into_iter().any(|a| a.tag == ttf_parser::Tag::from_bytes(b"wght"));
            let id = self.push(FaceInfo {
                id: FaceId(0),
                family,
                weight: face.weight().to_number(),
                italic: face.is_italic() || face.is_oblique(),
                path: path.to_path_buf(),
                index,
                source: FontSource::User,
                variable_weight,
                category: String::new(),
                licence: String::new(),
                attribution: String::new(),
                subsets: Vec::new(),
                monospaced: face.is_monospaced(),
                condensed: face.width().to_number() <= 4,
                classifications: Vec::new(),
            });
            ids.push(id);
        }
        self.files.lock().unwrap_or_else(|e| e.into_inner()).insert(path.to_path_buf(), Arc::new(data));
        Ok(ids)
    }

    fn push(&mut self, mut info: FaceInfo) -> FaceId {
        let id = FaceId(self.faces.len() as u32);
        info.id = id;
        self.by_family.entry(info.family.to_lowercase()).or_default().push(self.faces.len());
        self.faces.push(info);
        self.families_cache = std::sync::OnceLock::new();
        id
    }

    /// Uses a bundled emoji image set (or none, to draw emoji from fonts).
    pub fn set_emoji(&mut self, set: Option<EmojiSet>) {
        self.emoji = set;
    }

    pub fn emoji(&self) -> Option<&EmojiSet> {
        self.emoji.as_ref()
    }

    pub fn faces(&self) -> &[FaceInfo] {
        &self.faces
    }

    pub fn face_info(&self, id: FaceId) -> Option<&FaceInfo> {
        self.faces.get(id.0 as usize)
    }

    /// Family names, sorted, deduplicated; bundled and system together.
    pub fn families(&self) -> Vec<String> {
        self.families_shared().to_vec()
    }

    /// [`FontLibrary::families`] without rebuilding: the list is made once
    /// and kept until a face is added.
    pub fn families_shared(&self) -> Arc<[String]> {
        self.families_cache.get_or_init(|| self.build_families().into()).clone()
    }

    fn build_families(&self) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for f in &self.faces {
            if seen.insert(f.family.to_lowercase()) {
                v.push(f.family.clone());
            }
        }
        v.sort_by_cached_key(|a| a.to_lowercase());
        v
    }

    pub fn has_family(&self, family: &str) -> bool {
        self.by_family.contains_key(&self.resolve_generic(family).to_lowercase())
    }

    fn resolve_generic<'a>(&'a self, family: &'a str) -> &'a str {
        self.generic.get(&family.to_lowercase()).map(String::as_str).unwrap_or(family)
    }

    /// Loads a face (cached).
    pub fn load(&self, id: FaceId) -> Option<Arc<LoadedFace>> {
        if let Some(f) = self.loaded.lock().unwrap_or_else(|e| e.into_inner()).get(&id) {
            return Some(f.clone());
        }
        let info = self.faces.get(id.0 as usize)?.clone();
        let data = {
            let mut files = self.files.lock().unwrap_or_else(|e| e.into_inner());
            match files.get(&info.path) {
                Some(d) => d.clone(),
                None => {
                    let d = Arc::new(std::fs::read(&info.path).ok()?);
                    files.insert(info.path.clone(), d.clone());
                    d
                }
            }
        };
        let face = ttf_parser::Face::parse(&data, info.index).ok()?;
        let upem = face.units_per_em() as f32;
        let wght = ttf_parser::Tag::from_bytes(b"wght");
        let weight_range = face.variation_axes().into_iter().find(|a| a.tag == wght).map(|a| (a.min_value, a.max_value));
        let has_colour = face.tables().cbdt.is_some() || face.tables().sbix.is_some() || face.tables().colr.is_some() || face.tables().svg.is_some();
        let (ascent, descent, line_gap) = match face.tables().os2 {
            Some(os2) if os2.use_typographic_metrics() || os2.typographic_ascender() != 0 => {
                (os2.typographic_ascender() as f32, os2.typographic_descender() as f32, os2.typographic_line_gap() as f32)
            }
            _ => (face.ascender() as f32, face.descender() as f32, face.line_gap() as f32),
        };
        let loaded = Arc::new(LoadedFace {
            units_per_em: upem,
            ascent,
            descent,
            line_gap,
            cap_height: face.capital_height().map(|c| c as f32).unwrap_or(upem * 0.7),
            x_height: face.x_height().map(|c| c as f32).unwrap_or(upem * 0.5),
            underline_position: face.underline_metrics().map(|m| m.position as f32).unwrap_or(-upem * 0.1),
            underline_thickness: face.underline_metrics().map(|m| m.thickness as f32).unwrap_or(upem * 0.05),
            strikeout_position: face.strikeout_metrics().map(|m| m.position as f32).unwrap_or(upem * 0.3),
            strikeout_thickness: face.strikeout_metrics().map(|m| m.thickness as f32).unwrap_or(upem * 0.05),
            weight_range: if weight_range.is_some() { weight_range } else if info.variable_weight { Some((100.0, 900.0)) } else { None },
            has_colour,
            info: FaceInfo { variable_weight: weight_range.is_some() || info.variable_weight, ..info },
            data,
        });
        self.loaded.lock().unwrap_or_else(|e| e.into_inner()).insert(id, loaded.clone());
        Some(loaded)
    }

    /// Finds the face closest to a request within a family. `None` when the
    /// family is unknown (the caller then falls back to the default family).
    pub fn match_family(&self, req: &FontRequest) -> Option<FontMatch> {
        let family = self.resolve_generic(&req.family).to_lowercase();
        let idxs = self.by_family.get(&family)?;
        // Prefer bundled faces over system ones with the same name so that
        // results do not change between machines.
        let mut cands: Vec<&FaceInfo> = idxs.iter().map(|&i| &self.faces[i]).collect();
        cands.sort_by_key(|f| match f.source {
            FontSource::User => 0,
            FontSource::Bundled => 1,
            FontSource::System => 2,
        });
        let want_italic = req.italic;
        let italic_pool: Vec<&FaceInfo> = cands.iter().copied().filter(|f| f.italic == want_italic).collect();
        let (pool, synthetic_italic) = if italic_pool.is_empty() {
            (cands.iter().copied().filter(|f| !f.italic).collect::<Vec<_>>(), want_italic)
        } else {
            (italic_pool, false)
        };
        let pool = if pool.is_empty() { cands.clone() } else { pool };
        // Variable faces cover every weight exactly.
        if let Some(v) = pool.iter().find(|f| f.variable_weight) {
            let face = self.load(v.id)?;
            let (lo, hi) = face.weight_range.unwrap_or((100.0, 900.0));
            let w = (req.weight as f32).clamp(lo, hi);
            let synthetic_bold = req.weight as f32 > hi + 50.0;
            return Some(FontMatch { face, weight_variation: Some(w), synthetic_bold, synthetic_italic });
        }
        let best = pool.iter().copied().min_by_key(|f| weight_distance(req.weight, f.weight))?;
        let face = self.load(best.id)?;
        let synthetic_bold = req.weight >= 600 && best.weight < 600 && (req.weight - best.weight) >= 200;
        Some(FontMatch { face, weight_variation: None, synthetic_bold, synthetic_italic })
    }

    /// The face for a request, falling back to the general chain and then
    /// to any face at all. `None` only for an empty library.
    pub fn resolve(&self, req: &FontRequest) -> Option<FontMatch> {
        if let Some(m) = self.match_family(req) {
            return Some(m);
        }
        for fam in &self.general_chain {
            if let Some(m) = self.match_family(&FontRequest { family: fam.clone(), ..req.clone() }) {
                return Some(m);
            }
        }
        let first = self.faces.first()?;
        let face = self.load(first.id)?;
        Some(FontMatch { face, weight_variation: None, synthetic_bold: false, synthetic_italic: false })
    }

    /// Whether `face` can show `c` (cached).
    pub fn covers(&self, face: &LoadedFace, c: char) -> bool {
        let key = (face.info.id, c);
        if let Some(&v) = self.coverage.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
            return v;
        }
        let v = face.has_glyph(c);
        self.coverage.lock().unwrap_or_else(|e| e.into_inner()).insert(key, v);
        v
    }

    /// Picks a face for `c` when `primary` cannot show it. Emoji go to the
    /// emoji chain first; other characters to their script's chain, then
    /// the general chain, then the emoji chain. Returns `primary` when
    /// nothing better exists so that `.notdef` is at least consistent.
    pub fn fallback(&self, primary: &FontMatch, c: char, script: Script) -> FontMatch {
        if self.covers(&primary.face, c) {
            return primary.clone();
        }
        let req = |family: &str| FontRequest {
            family: family.to_string(),
            weight: primary.face.info.weight,
            italic: primary.face.info.italic,
        };
        let emoji = is_emoji_presentation(c);
        let mut chain: Vec<&str> = Vec::new();
        if emoji {
            chain.extend(self.emoji_chain.iter().map(String::as_str));
        }
        if let Some(names) = self.script_chains.get(&script) {
            chain.extend(names.iter().map(String::as_str));
        }
        let noto = format!("Noto Sans {}", script.full_name().replace('_', " "));
        chain.push(&noto);
        chain.extend(self.general_chain.iter().map(String::as_str));
        if !emoji {
            chain.extend(self.emoji_chain.iter().map(String::as_str));
        }
        for fam in chain {
            if let Some(m) = self.match_family(&req(fam)) {
                if self.covers(&m.face, c) {
                    return m;
                }
            }
        }
        primary.clone()
    }
}

fn weight_distance(want: u16, have: u16) -> u32 {
    // Lighter faces are preferred for light requests, heavier for bold
    // ones, like the CSS algorithm; a plain distance with a nudge does it.
    let d = (want as i32 - have as i32).abs() as u32;
    if (want < 450 && have > want) || (want > 550 && have < want) { d + 50 } else { d }
}

/// Characters that should be shown with an emoji font.
pub fn is_emoji_presentation(c: char) -> bool {
    let u = c as u32;
    matches!(u,
        0x1F000..=0x1FAFF | 0x2600..=0x27BF | 0x2B50 | 0x2B55 | 0x231A..=0x231B | 0x23E9..=0x23FA
        | 0x25AA..=0x25AB | 0x25FB..=0x25FE | 0x2934..=0x2935 | 0x2B05..=0x2B07 | 0x2B1B..=0x2B1C | 0x3030 | 0x303D | 0x3297 | 0x3299)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weight_matching_prefers_direction() {
        assert!(weight_distance(300, 200) < weight_distance(300, 400));
        assert!(weight_distance(700, 800) < weight_distance(700, 600));
    }

    #[test]
    fn emoji_classification() {
        assert!(is_emoji_presentation('😀'));
        assert!(is_emoji_presentation('🚀'));
        assert!(!is_emoji_presentation('a'));
        assert!(!is_emoji_presentation('中'));
    }

    fn face(family: &str) -> FaceInfo {
        FaceInfo {
            id: FaceId(0),
            family: family.into(),
            weight: 400,
            italic: false,
            path: PathBuf::new(),
            index: 0,
            source: FontSource::System,
            variable_weight: false,
            category: String::new(),
            licence: String::new(),
            attribution: String::new(),
            subsets: Vec::new(),
            monospaced: false,
            condensed: false,
            classifications: Vec::new(),
        }
    }

    /// The list as it was made before it was kept.
    fn old_families(lib: &FontLibrary) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for f in lib.faces() {
            if seen.insert(f.family.to_lowercase()) {
                v.push(f.family.clone());
            }
        }
        v.sort_by_key(|a| a.to_lowercase());
        v
    }

    #[test]
    fn families_are_kept_and_match_the_old_list() {
        let mut lib = FontLibrary::new();
        for f in ["zeta", "Alpha", "alpha", "Beta", "Éclair", "beta bold", "Zeta"] {
            lib.push(face(f));
        }
        assert_eq!(lib.families(), old_families(&lib));
        let a = lib.families_shared();
        assert!(Arc::ptr_eq(&a, &lib.families_shared()), "built once");
        lib.push(face("Aardvark"));
        let b = lib.families_shared();
        assert!(!Arc::ptr_eq(&a, &b), "rebuilt after a face is added");
        assert_eq!(b[0], "Aardvark");
        assert_eq!(&*b, old_families(&lib).as_slice());
    }
}
