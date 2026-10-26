// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The font directory: every family the library knows, with a category and
//! style tags, searchable by name, category and look ("art deco", "script",
//! "mono", "heavy slab").
//!
//! Bundled fonts carry the catalogue's category; system and imported fonts
//! have none, so a family is classified from what the file says (fixed
//! width, condensed, weights, italic) and from its name. A short list of
//! well-known looks (art deco, blackletter, pixel, stencil, retro,
//! futuristic, western, horror, comic, elegant) is matched by family name
//! so a request such as "an Art Deco font" finds something even though no
//! font file says so. The result is a hint for choosing, not a verdict.

use std::collections::BTreeMap;

use crate::font::{FaceInfo, FontLibrary, FontSource};

/// One family in the directory.
#[derive(Clone, Debug, PartialEq)]
pub struct FontEntry {
    pub family: String,
    /// Where the best face came from (bundled beats imported beats system,
    /// as the matcher prefers them).
    pub source: FontSource,
    /// One of `sans`, `serif`, `display`, `script`, `handwritten`, `mono`,
    /// `symbols`, `other`.
    pub category: &'static str,
    /// Category plus style tags, lower case, sorted ("art deco", "bold",
    /// "condensed", "italic", "variable"…).
    pub tags: Vec<String>,
    /// CSS weights the family has files for, ascending (a variable font
    /// lists every hundred it spans).
    pub weights: Vec<u16>,
    pub italic: bool,
    pub variable: bool,
    pub licence: String,
    /// Number of faces (files) in the family.
    pub faces: usize,
}

/// A directory entry with how well it matched a search.
#[derive(Clone, Debug, PartialEq)]
pub struct FontHit {
    pub entry: FontEntry,
    pub score: u32,
}

/// Families known for a look, matched by lower-case family name.
const ART_DECO: &[&str] = &[
    "broadway", "poiret one", "limelight", "monoton", "federo", "gruppo", "megrim", "italiana", "cinzel decorative", "fascinate", "fascinate inline", "vast shadow", "julius sans one", "josefin sans", "josefin slab", "marcellus", "tenor sans", "bellefair", "forum", "bungee inline", "bungee shade", "wire one", "metamorphous", "lemon", "gloock", "unica one", "bigshot one", "parisienne", "erica one", "abril fatface", "playfair display sc", "bodoni moda", "didot", "bauhaus 93", "parisian", "peignot", "futura", "avant garde",
];
const BLACKLETTER: &[&str] = &["unifraktur maguntia", "unifraktur cook", "pirata one", "grenze gotisch", "jacquard 12", "jacquard 24", "new rocker", "fruktur", "texturina", "old london", "blackadder", "fraktur"];
const PIXEL: &[&str] = &["press start 2p", "silkscreen", "pixelify sans", "vt323", "jersey 10", "jersey 15", "jersey 20", "jersey 25", "tiny5", "micro 5", "bitcount", "handjet", "doto", "workbench", "dotgothic16", "geo"];
const STENCIL: &[&str] = &["stardos stencil", "big shoulders stencil", "saira stencil one", "allerta stencil", "black ops one", "stencil", "stalinist one"];
const FUTURISTIC: &[&str] = &["orbitron", "audiowide", "michroma", "oxanium", "exo", "exo 2", "rajdhani", "russo one", "electrolize", "syncopate", "major mono display", "share tech", "tomorrow", "zen dots", "quantico", "aldrich", "bruno ace", "krona one", "jura", "xanh mono"];
const WESTERN: &[&str] = &["rye", "sancreek", "ewert", "smokum", "diplomata", "diplomata sc", "ranchers", "holtwood one sc", "bigelow rules", "alfa slab one", "wellfleet"];
const HORROR: &[&str] = &["creepster", "nosifer", "eater", "metal mania", "butcherman", "frijole", "chilanka", "special elite", "new rocker", "irish grover", "grenze gotisch", "jolly lodger", "fontdiner swanky"];
const COMIC: &[&str] = &["bangers", "comic neue", "comic sans ms", "bubblegum sans", "chewy", "luckiest guy", "boogaloo", "londrina solid", "fredoka", "baloo 2", "patrick hand", "gochi hand", "schoolbell", "sniglet", "carter one", "lilita one"];
const ELEGANT: &[&str] = &["cormorant", "cormorant garamond", "cormorant infant", "playfair display", "bodoni moda", "cinzel", "cinzel decorative", "didot", "bodoni 72", "libre bodoni", "libre caslon display", "prata", "gilda display", "marcellus", "italiana", "bellefair", "tenor sans", "lora", "dm serif display", "ibarra real nova", "fraunces", "garamond", "baskerville", "optima", "zapfino", "snell roundhand", "pinyon script", "great vibes", "allura", "alex brush", "tangerine", "playball", "parisienne", "petit formal script"];
const RETRO: &[&str] = &["pacifico", "lobster", "lobster two", "fontdiner swanky", "shrikhand", "bungee", "bungee shade", "bungee inline", "monoton", "righteous", "limelight", "fascinate", "rye", "sonsie one", "oleo script", "yellowtail", "kaushan script", "chango", "bowlby one", "bowlby one sc", "abril fatface", "alfa slab one", "ultra", "titan one", "gloria hallelujah", "vast shadow", "cherry cream soda", "kavoon", "limelight", "sarina"];
const SLAB_HINTS: &[&str] = &["slab", "rockwell", "clarendon", "courier", "roboto slab", "zilla", "arvo", "bitter", "crete round", "josefin slab", "aleo", "glegoo", "rokkitt", "enriqueta", "kreon", "sanchez", "egyptienne", "serifa", "museo slab", "bevan", "alfa slab"];
const ROUNDED_HINTS: &[&str] = &["rounded", "nunito", "varela round", "quicksand", "fredoka", "baloo", "comfortaa", "m plus rounded", "dongle", "mochiy", "kosugi maru", "arial rounded", "sf pro rounded", "hiragino maru", "bubblegum", "chewy"];
const SCRIPT_HINTS: &[&str] = &["script", "brush", "calligraph", "cursive", "chancery", "zapfino", "snell", "bradley hand", "apple chancery", "pinyon", "great vibes", "allura", "alex brush", "tangerine", "playball", "pacifico", "dancing script", "satisfy", "sacramento", "yellowtail", "lobster", "kaushan", "parisienne", "petit formal", "italianno", "monte carlo", "mrs saint delafield", "herr von muellerhoff", "marck script", "arizonia", "rouge script", "norican", "courgette", "cookie", "oleo script", "playwrite", "style script", "lavishly yours"];
const HAND_HINTS: &[&str] = &["hand", "marker", "noteworthy", "chalk", "comic", "caveat", "shadows into light", "permanent marker", "reenie", "indie flower", "gloria", "kalam", "patrick", "schoolbell", "gochi", "covered by your grace", "just another hand", "rock salt", "walter turncoat", "nanum pen", "nothing you could do", "homemade apple", "handlee", "architects daughter", "amatic", "coming soon", "short stack", "sue ellen", "zeyada", "bradley", "segoe print", "ink free", "bangers"];
const SERIF_HINTS: &[&str] = &["serif", "times", "georgia", "garamond", "baskerville", "didot", "bodoni", "palatino", "caslon", "minion", "cambria", "book antiqua", "playfair", "merriweather", "lora", "charter", "cormorant", "libre", "crimson", "spectral", "source serif", "noto serif", "pt serif", "gelasio", "bookman", "century", "constantia", "hoefler", "iowan", "new york", "cochin", "didot", "rockwell", "prata", "dm serif", "fraunces", "newsreader", "eb garamond", "vollkorn", "literata", "alegreya", "cardo", "gentium", "old standard", "bitter"];
const SANS_HINTS: &[&str] = &["sans", "helvetica", "arial", "verdana", "tahoma", "gothic", "avenir", "futura", "gill", "optima", "inter", "roboto", "segoe", "lato", "open sans", "montserrat", "poppins", "manrope", "work sans", "nunito", "raleway", "ubuntu", "grotesk", "grotesque", "calibri", "trebuchet", "geneva", "lucida grande", "sf pro", "san francisco", "dm sans", "outfit", "jost", "figtree", "karla", "rubik", "barlow", "oswald", "bebas", "league spartan", "mulish", "hind", "cabin", "fira sans", "ibm plex sans", "source sans", "pt sans", "noto sans", "pretendard", "kanit", "prompt", "sarabun", "heebo", "assistant"];
const DISPLAY_HINTS: &[&str] = &["display", "black", "impact", "poster", "headline", "bebas", "anton", "bungee", "stencil", "chunk", "ultra", "fat", "shadow", "inline", "outline", "rounded mt bold", "bauhaus", "showcard", "jokerman", "algerian", "broadway", "copperplate", "papyrus", "luminari"];
const MONO_HINTS: &[&str] = &["mono", "code", "courier", "consolas", "menlo", "monaco", "typewriter", "terminal", "console", "fixed", "lucida console", "andale", "inconsolata", "fira code", "hack", "cousine", "space mono", "anonymous"];

fn listed(list: &[&str], name: &str) -> bool {
    list.iter().any(|f| name == *f || (f.len() >= 6 && name.starts_with(f)))
}

fn has_hint(list: &[&str], name: &str) -> bool {
    list.iter().any(|h| name.contains(h))
}

/// Category and tags of a family from its faces. Pure, so it can be tested
/// without any font files.
pub fn classify(family: &str, faces: &[&FaceInfo]) -> (&'static str, Vec<String>) {
    let name = family.to_lowercase();
    let bundled = faces.iter().find(|f| !f.category.is_empty());
    let from_catalogue = bundled.map(|f| f.category.to_lowercase());
    let class = |c: &str| faces.iter().any(|f| f.classifications.iter().any(|x| x.eq_ignore_ascii_case(c)));
    let mono = faces.iter().any(|f| f.monospaced) || class("monospace") || from_catalogue.as_deref() == Some("monospace");
    let script = (has_hint(SCRIPT_HINTS, &name) && !name.contains("sans")) || listed(SCRIPT_HINTS, &name);
    let mut category: &'static str = match from_catalogue.as_deref() {
        Some("sans") => "sans",
        Some("serif") => "serif",
        Some("display") => "display",
        Some("handwritten") | Some("handwriting") => "handwritten",
        Some("monospace") => "mono",
        _ => "other",
    };
    if from_catalogue.is_none() || category == "other" {
        category = if mono || has_hint(MONO_HINTS, &name) && !name.contains("sans mono") {
            "mono"
        } else if script {
            "script"
        } else if has_hint(HAND_HINTS, &name) {
            "handwritten"
        } else if has_hint(DISPLAY_HINTS, &name) {
            "display"
        } else if name.contains("sans") || has_hint(SANS_HINTS, &name) && !name.contains("serif") {
            "sans"
        } else if has_hint(SERIF_HINTS, &name) {
            "serif"
        } else if name.contains("symbol") || name.contains("emoji") || name.contains("dingbat") || name.contains("wingding") || name.contains("webding") || name.contains("icon") {
            "symbols"
        } else {
            "other"
        };
    }
    // A bundled handwriting face that is a formal script is both.
    let mut tags: Vec<String> = vec![category.to_owned()];
    if script && category != "script" {
        tags.push("script".into());
    }
    if category == "handwritten" || (script && category == "handwritten") {
        tags.push("handwriting".into());
    }
    if class("symbols") && category != "symbols" {
        tags.push("symbols".into());
    }
    if mono && category != "mono" {
        tags.push("mono".into());
    }
    let mut add = |on: bool, t: &str| {
        if on && !tags.iter().any(|x| x == t) {
            tags.push(t.to_owned());
        }
    };
    add(listed(ART_DECO, &name) || name.contains("deco"), "art deco");
    add(listed(BLACKLETTER, &name) || name.contains("fraktur") || name.contains("blackletter"), "blackletter");
    add(listed(PIXEL, &name) || name.contains("pixel") || name.contains("8-bit") || name.contains("bitmap"), "pixel");
    add(listed(STENCIL, &name) || name.contains("stencil"), "stencil");
    add(listed(FUTURISTIC, &name), "futuristic");
    add(listed(WESTERN, &name), "western");
    add(listed(HORROR, &name), "horror");
    add(listed(COMIC, &name), "comic");
    add(listed(ELEGANT, &name), "elegant");
    add(listed(RETRO, &name), "retro");
    add(has_hint(SLAB_HINTS, &name) && !name.contains("sans"), "slab");
    add(has_hint(ROUNDED_HINTS, &name), "rounded");
    add(faces.iter().any(|f| f.condensed) || name.contains("condensed") || name.contains("narrow") || name.contains("compressed"), "condensed");
    add(name.contains("outline") || name.contains("inline") || name.contains("shadow"), "decorative");
    add(faces.iter().any(|f| f.italic), "italic");
    add(faces.iter().any(|f| f.variable_weight), "variable");
    let heaviest = faces.iter().map(|f| f.weight).max().unwrap_or(400);
    let lightest = faces.iter().map(|f| f.weight).min().unwrap_or(400);
    add(heaviest >= 600 || name.contains("black") || name.contains("heavy"), "bold");
    add(lightest <= 300 || name.contains("thin") || name.contains("light"), "light");
    add(heaviest >= 900 && category == "display", "heavy");
    tags.sort();
    tags.dedup();
    (category, tags)
}

/// Words people use for a category or look, mapped to the tag they mean.
fn tag_for(word: &str) -> Option<&'static str> {
    Some(match word {
        "sans" | "sans-serif" | "sansserif" | "grotesque" | "grotesk" | "gothic" | "geometric" | "clean" | "modern" | "neutral" => "sans",
        "serif" | "roman" | "bookish" | "classic" | "oldstyle" | "didone" | "transitional" => "serif",
        "display" | "headline" | "headlines" | "title" | "titles" | "poster" | "decorative" | "fancy" | "ornate" => "display",
        "script" | "calligraphy" | "calligraphic" | "cursive" | "formal-script" | "signature" | "wedding" | "brush" => "script",
        "handwritten" | "handwriting" | "hand" | "handdrawn" | "marker" | "sketch" | "casual" | "informal" => "handwritten",
        "mono" | "monospace" | "monospaced" | "code" | "coding" | "typewriter" | "terminal" | "fixed" => "mono",
        "symbols" | "symbol" | "dingbats" | "icons" => "symbols",
        "artdeco" | "deco" | "gatsby" | "1920s" | "twenties" | "roaring" | "streamline" => "art deco",
        "blackletter" | "fraktur" | "gothic-blackletter" | "medieval" | "oldenglish" => "blackletter",
        "pixel" | "pixelated" | "8bit" | "8-bit" | "bitmap" | "arcade" | "retro-game" => "pixel",
        "stencil" | "military" => "stencil",
        "futuristic" | "scifi" | "sci-fi" | "tech" | "techno" | "cyber" | "space" => "futuristic",
        "western" | "wildwest" | "cowboy" | "saloon" => "western",
        "horror" | "spooky" | "scary" | "creepy" | "halloween" => "horror",
        "comic" | "cartoon" | "playful" | "fun" | "kids" => "comic",
        "elegant" | "luxury" | "luxurious" | "refined" | "fashion" | "upscale" | "chic" => "elegant",
        "retro" | "vintage" | "70s" | "60s" | "50s" | "nostalgic" => "retro",
        "slab" | "slab-serif" | "egyptian" | "western-slab" => "slab",
        "rounded" | "soft" | "friendly" | "bubbly" => "rounded",
        "condensed" | "narrow" | "compressed" | "tall" => "condensed",
        "italic" | "oblique" | "slanted" => "italic",
        "variable" => "variable",
        "bold" | "heavy" | "strong" | "thick" | "black" | "fat" => "bold",
        "light" | "thin" | "hairline" | "delicate" => "light",
        _ => return None,
    })
}

/// A search query split into the tags and name words it asks for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Wanted {
    pub tags: Vec<&'static str>,
    pub words: Vec<String>,
}

/// Reads a free-text query: "art deco", "script wedding", "heavy slab",
/// "mono". Two-word looks are joined first; every other word that names
/// no tag is a piece of a family name.
pub fn parse_query(query: &str) -> Wanted {
    let mut q = format!(" {} ", query.to_lowercase().replace([',', ';', '/'], " "));
    for (phrase, joined) in [
        (" art deco ", " artdeco "),
        (" art-deco ", " artdeco "),
        (" sans serif ", " sans "),
        (" sans-serif ", " sans "),
        (" old english ", " oldenglish "),
        (" hand written ", " handwritten "),
        (" hand-written ", " handwritten "),
        (" hand drawn ", " handdrawn "),
        (" hand-drawn ", " handdrawn "),
        (" wild west ", " wildwest "),
        (" sci fi ", " scifi "),
        (" slab serif ", " slab "),
        (" slab-serif ", " slab "),
        (" 8 bit ", " 8bit "),
        (" mono space ", " mono "),
    ] {
        q = q.replace(phrase, joined);
    }
    let mut w = Wanted::default();
    for word in q.split_whitespace() {
        if matches!(word, "font" | "fonts" | "typeface" | "typefaces" | "a" | "an" | "the" | "for" | "style" | "look" | "type" | "with" | "and" | "in" | "of") {
            continue;
        }
        match tag_for(word) {
            Some(t) => {
                if !w.tags.contains(&t) {
                    w.tags.push(t);
                }
            }
            None => w.words.push(word.to_owned()),
        }
    }
    w
}

impl FontLibrary {
    /// Every family with its category, tags, weights and source.
    pub fn directory(&self) -> Vec<FontEntry> {
        let mut by: BTreeMap<String, (String, Vec<&FaceInfo>)> = BTreeMap::new();
        for f in self.faces() {
            by.entry(f.family.to_lowercase()).or_insert_with(|| (f.family.clone(), Vec::new())).1.push(f);
        }
        by.into_values()
            .map(|(family, faces)| {
                let (category, tags) = classify(&family, &faces);
                let source = faces.iter().map(|f| f.source).min_by_key(|s| match s {
                    FontSource::User => 0,
                    FontSource::Bundled => 1,
                    FontSource::System => 2,
                });
                let mut weights: Vec<u16> = Vec::new();
                for f in &faces {
                    if f.variable_weight {
                        weights.extend((1..=9).map(|k| k * 100));
                    } else {
                        weights.push(f.weight);
                    }
                }
                weights.sort_unstable();
                weights.dedup();
                FontEntry {
                    source: source.unwrap_or(FontSource::System),
                    category,
                    tags,
                    weights,
                    italic: faces.iter().any(|f| f.italic),
                    variable: faces.iter().any(|f| f.variable_weight),
                    licence: faces.iter().map(|f| f.licence.clone()).find(|l| !l.is_empty()).unwrap_or_default(),
                    faces: faces.len(),
                    family,
                }
            })
            .collect()
    }

    /// Families matching `query` (and `category`, if given), best first.
    /// Every wanted tag and name word must match (a word matches the family
    /// name or a tag); with several terms and no such family, families
    /// matching any of them follow, flagged by a lower score.
    pub fn find_fonts(&self, query: &str, category: Option<&str>, limit: usize) -> Vec<FontHit> {
        let mut want = parse_query(query);
        if let Some(c) = category.map(|c| c.trim().to_lowercase()).filter(|c| !c.is_empty()) {
            let parsed = parse_query(&c);
            for t in parsed.tags {
                if !want.tags.contains(&t) {
                    want.tags.push(t);
                }
            }
            want.words.extend(parsed.words);
        }
        let dir = self.directory();
        let total = want.tags.len() + want.words.len();
        let score = |e: &FontEntry| -> (usize, u32) {
            let name = e.family.to_lowercase();
            let (mut hit, mut s) = (0usize, 0u32);
            for t in &want.tags {
                if e.tags.iter().any(|x| x == t) {
                    hit += 1;
                    s += if e.category == *t { 120 } else { 100 };
                }
            }
            for w in &want.words {
                if name == *w {
                    hit += 1;
                    s += 1000;
                } else if name.starts_with(w.as_str()) {
                    hit += 1;
                    s += 400;
                } else if name.contains(w.as_str()) {
                    hit += 1;
                    s += 250;
                } else if e.tags.iter().any(|x| x.contains(w.as_str())) {
                    hit += 1;
                    s += 60;
                }
            }
            (hit, s)
        };
        let mut all: Vec<FontHit> = Vec::new();
        if total == 0 {
            all = dir.into_iter().map(|entry| FontHit { entry, score: 0 }).collect();
        } else {
            let mut some: Vec<FontHit> = Vec::new();
            for e in dir {
                let (hit, s) = score(&e);
                if hit == total {
                    all.push(FontHit { entry: e, score: s + 10_000 });
                } else if hit > 0 {
                    some.push(FontHit { entry: e, score: s });
                }
            }
            if all.is_empty() {
                all = some;
            }
        }
        // Bundled and imported first (they render the same everywhere),
        // then fuller families, then by name.
        all.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| rank(a.entry.source).cmp(&rank(b.entry.source)))
                .then_with(|| b.entry.weights.len().cmp(&a.entry.weights.len()))
                .then_with(|| a.entry.family.to_lowercase().cmp(&b.entry.family.to_lowercase()))
        });
        all.truncate(limit);
        all
    }

    /// How many families per category, for an overview.
    pub fn category_counts(&self) -> Vec<(&'static str, usize)> {
        let mut m: BTreeMap<&'static str, usize> = BTreeMap::new();
        for e in self.directory() {
            *m.entry(e.category).or_default() += 1;
        }
        m.into_iter().collect()
    }

    /// Family names closest to `name` (for "did you mean"): shared prefix,
    /// containment, then small edit distance.
    pub fn similar_families(&self, name: &str, limit: usize) -> Vec<String> {
        let n = name.to_lowercase();
        let mut scored: Vec<(u32, String)> = Vec::new();
        for fam in self.families() {
            let f = fam.to_lowercase();
            let s = if f == n {
                0
            } else if f.starts_with(&n) || n.starts_with(&f) {
                1
            } else if f.contains(&n) || n.contains(&f) {
                2
            } else {
                let d = edit_distance(&f, &n) as u32;
                if d <= (n.len() as u32 / 3).max(2) { 3 + d } else { continue }
            };
            scored.push((s, fam));
        }
        scored.sort();
        scored.into_iter().take(limit).map(|(_, f)| f).collect()
    }
}

fn rank(s: FontSource) -> u8 {
    match s {
        FontSource::User => 0,
        FontSource::Bundled => 1,
        FontSource::System => 2,
    }
}

fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i];
        for j in 1..=b.len() {
            let sub = prev[j - 1] + usize::from(a[i - 1] != b[j - 1]);
            cur.push(sub.min(prev[j] + 1).min(cur[j - 1] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::FaceId;

    fn face(family: &str, weight: u16, italic: bool) -> FaceInfo {
        FaceInfo {
            id: FaceId(0),
            family: family.into(),
            weight,
            italic,
            path: Default::default(),
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

    #[test]
    fn queries_split_into_tags_and_name_words() {
        let w = parse_query("An Art Deco font");
        assert_eq!(w.tags, vec!["art deco"]);
        assert!(w.words.is_empty());
        let w = parse_query("heavy slab-serif, poiret");
        assert_eq!(w.tags, vec!["bold", "slab"]);
        assert_eq!(w.words, vec!["poiret"]);
        assert_eq!(parse_query("Hand written marker").tags, vec!["handwritten"]);
        assert_eq!(parse_query("monospaced").tags, vec!["mono"]);
    }

    #[test]
    fn classification_uses_name_file_flags_and_catalogue() {
        let (c, t) = classify("Broadway", &[&face("Broadway", 400, false)]);
        assert_eq!(c, "display", "{t:?}");
        assert!(t.contains(&"art deco".to_owned()));
        let mut m = face("Whatever Code", 400, false);
        m.monospaced = true;
        assert_eq!(classify("Whatever Code", &[&m]).0, "mono");
        let (c, t) = classify("Zapfino", &[&face("Zapfino", 400, false)]);
        assert_eq!(c, "script");
        assert!(t.contains(&"elegant".to_owned()) || t.contains(&"script".to_owned()));
        let mut b = face("Roboto Condensed", 700, false);
        b.condensed = true;
        let (c, t) = classify("Roboto Condensed", &[&b, &face("Roboto Condensed", 300, true)]);
        assert_eq!(c, "sans");
        for tag in ["condensed", "bold", "light", "italic"] {
            assert!(t.contains(&tag.to_owned()), "{tag} in {t:?}");
        }
        let mut cat = face("Caveat", 400, false);
        cat.category = "Handwritten".into();
        assert_eq!(classify("Caveat", &[&cat]).0, "handwritten");
    }

    #[test]
    fn similar_names_for_a_misspelling() {
        let mut lib = FontLibrary::new();
        for p in ["Inter-Regular.ttf", "JetBrainsMono-Regular.ttf"] {
            let _ = lib.load_file(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../paint/pixel-text/assets/fonts").join(p));
        }
        assert_eq!(lib.similar_families("Intr", 3).first().map(String::as_str), Some("Inter"));
        assert!(lib.similar_families("zzzzzzzzz", 3).is_empty());
    }
}
