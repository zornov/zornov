// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Loader for the open-licence collection (`catalog.json`).
//!
//! The catalogue has tabs for emoji (Twemoji and OpenMoji), stickers
//! (Fluent Emoji, Kenney), shapes (Tabler Icons), music (Kevin MacLeod,
//! OpenGameArt, Kenney), sound effects (Kenney, OpenGameArt) and fonts
//! (Google Fonts). Everything becomes an [`Item`] with its upstream licence
//! and the exact credit line; names are made human with
//! [`crate::naming::humanise`].

use crate::item::{Details, FontFile, Index, Item, Kind, Licence, Source};
use crate::made::slug;
use crate::naming::{humanise, sentence_case};
use crate::search::tokens;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const AUDIO_EXTS: [&str; 4] = ["ogg", "wav", "mp3", "flac"];

/// Raw download leftovers (`_dl` folders) are not library content.
fn is_scratch(path: &str) -> bool {
    path.split('/').any(|p| p == "_dl")
}

fn s<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty())
}

fn strings(v: &Value, key: &str) -> Vec<String> {
    v.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect()).unwrap_or_default()
}

fn licence_of(v: &Value, fallback: Licence) -> Result<Licence, String> {
    match s(v, "licence") {
        Some(l) => Licence::parse(l).ok_or_else(|| format!("licence {l:?} is not on the approved list")),
        None => Ok(fallback),
    }
}

fn source_from(catalog: &Value, id: &str, name: &str, default_url: &str) -> Source {
    let src = catalog.get("sources").and_then(|m| m.get(id));
    let url = src.and_then(|v| s(v, "url")).unwrap_or(default_url);
    let version = src.and_then(|v| s(v, "version").or(s(v, "commit"))).unwrap_or("");
    Source::new(id, name, url, version)
}

/// `category id -> name` and `item id -> first category name`.
fn category_maps(tab: &Value) -> (Vec<String>, HashMap<String, String>) {
    let mut names = Vec::new();
    let mut by_item: HashMap<String, String> = HashMap::new();
    if let Some(cats) = tab.get("categories").and_then(Value::as_array) {
        for c in cats {
            let Some(name) = s(c, "name") else { continue };
            names.push(name.to_string());
            for id in strings(c, "items") {
                by_item.entry(id).or_insert_with(|| name.to_string());
            }
        }
    }
    (names, by_item)
}

fn tags(parts: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut t: Vec<String> = parts.into_iter().flat_map(|p| tokens(&p)).filter(|w| w.len() > 1).collect();
    t.sort();
    t.dedup();
    t
}

fn parse_duration(d: &str) -> Option<f64> {
    let parts: Vec<f64> = d.split(':').map(|p| p.trim().parse::<f64>().ok()).collect::<Option<Vec<_>>>()?;
    Some(parts.iter().fold(0.0, |acc, p| acc * 60.0 + p))
}

/// Loads the open-licence index rooted at `root` (the folder holding
/// `catalog.json`).
pub fn load(root: &Path) -> Result<Index, String> {
    let path = root.join("catalog.json");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let catalog: Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    load_value(root, &catalog)
}

/// Loads from an already parsed catalogue.
pub fn load_value(root: &Path, catalog: &Value) -> Result<Index, String> {
    let mut index = Index::new(root);
    let tabs = catalog.get("tabs").ok_or("catalog.json has no tabs")?;
    let twemoji = source_from(catalog, "twemoji", "Twemoji", "https://github.com/jdecked/twemoji");
    let openmoji = source_from(catalog, "openmoji", "OpenMoji", "https://openmoji.org");
    let fluent = source_from(catalog, "fluent-emoji", "Fluent Emoji", "https://github.com/microsoft/fluentui-emoji");
    let kenney = Source::new("kenney", "Kenney", "https://kenney.nl", "");
    let tabler = source_from(catalog, "tabler", "Tabler Icons", "https://tabler.io/icons");
    let google_fonts = source_from(catalog, "google-fonts", "Google Fonts", "https://fonts.google.com");
    let incompetech = Source::new("incompetech", "Kevin MacLeod (incompetech.com)", "https://incompetech.com", "");
    let opengameart = Source::new("opengameart", "OpenGameArt", "https://opengameart.org", "");

    // Emoji: one item per set variant.
    if let Some(items) = tabs.pointer("/emoji/items").and_then(Value::as_object) {
        for (id, v) in items {
            let Some(raw_name) = s(v, "name") else { continue };
            let group = s(v, "group").unwrap_or("Emoji");
            if group == "Component" || group == "Components" {
                continue;
            }
            let name = humanise(raw_name);
            let mut tag_src = strings(v, "tags");
            tag_src.push(raw_name.to_string());
            tag_src.push(s(v, "subgroup").unwrap_or("").replace('-', " "));
            let emoji_char = s(v, "char").map(str::to_string);
            let Some(sets) = v.get("sets").and_then(Value::as_object) else { continue };
            for (set_id, sv) in sets {
                let Some(svg) = s(sv, "svg") else { continue };
                let (source, set_name, licence, credit, licence_file) = match set_id.as_str() {
                    "twemoji" => (
                        twemoji.clone(),
                        "Twemoji",
                        Licence::CcBy4,
                        "Twemoji graphics by Twitter, Inc and other contributors, licensed under CC BY 4.0 (https://creativecommons.org/licenses/by/4.0/)".to_string(),
                        Some(PathBuf::from("emoji/twemoji/LICENSE-GRAPHICS")),
                    ),
                    "openmoji" => (
                        openmoji.clone(),
                        "OpenMoji",
                        Licence::CcBySa4,
                        match s(sv, "author") {
                            Some(a) => format!("OpenMoji - the open-source emoji and icon project (https://openmoji.org), emoji by {a}, licensed under CC BY-SA 4.0 (https://creativecommons.org/licenses/by-sa/4.0/)"),
                            None => "OpenMoji - the open-source emoji and icon project (https://openmoji.org), licensed under CC BY-SA 4.0 (https://creativecommons.org/licenses/by-sa/4.0/)".to_string(),
                        },
                        Some(PathBuf::from("emoji/openmoji/LICENSE.txt")),
                    ),
                    _ => continue,
                };
                let licence = licence_of(sv, licence)?;
                let preview = s(sv, "png618").or(s(sv, "png72")).map(PathBuf::from);
                index.items.push(Item {
                    id: format!("{set_id}:{}", slug(id.trim_start_matches("emoji:"))),
                    name: name.clone(),
                    kind: Kind::Emoji,
                    category: sentence_case(group),
                    set: set_name.into(),
                    tags: tags(tag_src.iter().cloned().chain([set_name.to_string()])),
                    file: PathBuf::from(svg),
                    preview,
                    licence,
                    attribution: credit,
                    source,
                    details: Details {
                        emoji: emoji_char.clone(),
                        author: s(sv, "author").map(str::to_string),
                        licence_file,
                        ..Details::default()
                    },
                });
            }
        }
    }

    // Stickers.
    if let Some(tab) = tabs.get("stickers") {
        let (_, by_item) = category_maps(tab);
        if let Some(items) = tab.get("items").and_then(Value::as_object) {
            for (id, v) in items {
                let Some(raw_name) = s(v, "name") else { continue };
                let licence = licence_of(v, Licence::Mit)?;
                let png = s(v, "png").map(PathBuf::from);
                let svg = s(v, "svg").map(PathBuf::from);
                let (file, preview) = match (&svg, &png) {
                    (Some(svg), png) => (svg.clone(), png.clone()),
                    (None, Some(png)) => (png.clone(), None),
                    (None, None) => continue,
                };
                let set_raw = s(v, "set").unwrap_or("Stickers");
                let is_fluent = id.starts_with("sticker:fluent");
                let (source, set, licence_file) = if is_fluent {
                    (fluent.clone(), "Fluent Emoji".to_string(), Some(PathBuf::from("stickers/fluent-emoji/LICENSE")))
                } else {
                    (kenney.clone(), humanise(set_raw), None)
                };
                let category = by_item.get(id).cloned().unwrap_or_else(|| "Stickers".to_string());
                let mut tag_src = strings(v, "tags");
                tag_src.push(raw_name.to_string());
                tag_src.push(category.clone());
                index.items.push(Item {
                    id: slug(id),
                    name: humanise(raw_name),
                    kind: Kind::Sticker,
                    category: sentence_case(&category),
                    set,
                    tags: tags(tag_src),
                    file,
                    preview,
                    licence,
                    attribution: s(v, "attribution").unwrap_or("").to_string(),
                    source,
                    details: Details { licence_file, ..Details::default() },
                });
            }
        }
    }

    // Shapes (Tabler icons: outline main file, filled variant).
    if let Some(tab) = tabs.get("shapes") {
        let (_, by_item) = category_maps(tab);
        if let Some(items) = tab.get("items").and_then(Value::as_object) {
            for (id, v) in items {
                let Some(raw_name) = s(v, "name") else { continue };
                let files = v.get("files").cloned().unwrap_or(Value::Null);
                let outline = s(&files, "outline").map(PathBuf::from);
                let filled = s(&files, "filled").map(PathBuf::from);
                let Some(file) = outline.clone().or(filled.clone()) else { continue };
                let category = by_item.get(id).cloned().unwrap_or_else(|| "Shapes".to_string());
                let mut tag_src = strings(v, "tags");
                tag_src.push(raw_name.to_string());
                tag_src.push(category.clone());
                index.items.push(Item {
                    id: slug(id),
                    name: humanise(raw_name),
                    kind: Kind::Shape,
                    category: sentence_case(&category),
                    set: "Tabler Icons".into(),
                    tags: tags(tag_src),
                    file,
                    preview: None,
                    licence: licence_of(v, Licence::Mit)?,
                    attribution: "Tabler Icons, Copyright (c) 2020-2024 Pawel Kuna, MIT License".into(),
                    source: tabler.clone(),
                    details: Details {
                        filled_variant: if outline.is_some() { filled } else { None },
                        licence_file: Some(PathBuf::from("icons/tabler/pkg/package/LICENSE")),
                        ..Details::default()
                    },
                });
            }
        }
    }

    // Music.
    if let Some(items) = tabs.pointer("/audio/music/items").and_then(Value::as_object) {
        for (id, v) in items {
            let Some(path) = s(v, "path") else { continue };
            let ext = Path::new(path).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            if !AUDIO_EXTS.contains(&ext.as_str()) || is_scratch(path) {
                continue;
            }
            let title = s(v, "title").unwrap_or("Track");
            let artist = s(v, "artist").unwrap_or("Unknown artist");
            let source = if path.starts_with("music/incompetech") {
                incompetech.clone()
            } else if path.starts_with("music/kenney") {
                kenney.clone()
            } else {
                opengameart.clone()
            };
            // Pack titles like "Pack name - Track" give the set; the track
            // name then comes from the file itself.
            let (set, name) = match title.split_once(" - ") {
                Some((pack, _)) => (humanise(pack), humanise(Path::new(path).file_stem().and_then(|f| f.to_str()).unwrap_or(title))),
                None => (artist.to_string(), humanise(title)),
            };
            let genre = s(v, "genre").unwrap_or("Music");
            let mut tag_src = strings(v, "moods");
            tag_src.extend(strings(v, "tags"));
            tag_src.push(genre.to_string());
            tag_src.push(title.to_string());
            tag_src.push(artist.to_string());
            if let Some(instr) = s(v, "instruments") {
                tag_src.push(instr.to_string());
            }
            index.items.push(Item {
                id: slug(id),
                name,
                kind: Kind::Music,
                category: sentence_case(genre),
                set,
                tags: tags(tag_src),
                file: PathBuf::from(path),
                preview: None,
                licence: licence_of(v, Licence::CcBy4)?,
                attribution: s(v, "attribution").unwrap_or("").to_string(),
                source,
                details: Details {
                    duration_secs: s(v, "duration").and_then(parse_duration),
                    bpm: v.get("bpm").and_then(Value::as_u64).map(|b| b as u32),
                    artist: Some(artist.to_string()),
                    source_url: s(v, "source_url").map(str::to_string),
                    ..Details::default()
                },
            });
        }
    }

    // Sound effects.
    if let Some(tab) = tabs.pointer("/audio/sound_effects") {
        if let Some(items) = tab.get("items").and_then(Value::as_object) {
            for (id, v) in items {
                let Some(path) = s(v, "path") else { continue };
                let ext = Path::new(path).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
                if !AUDIO_EXTS.contains(&ext.as_str()) || is_scratch(path) {
                    continue;
                }
                let title = s(v, "title").unwrap_or("Sound");
                if title.eq_ignore_ascii_case("preview") {
                    continue; // pack demo reels
                }
                let pack = s(v, "pack").unwrap_or("Sounds");
                let cats = strings(v, "categories");
                let category = cats.first().cloned().unwrap_or_else(|| "Sounds".to_string());
                let source = if path.starts_with("sfx/kenney") { kenney.clone() } else { opengameart.clone() };
                let mut tag_src = cats.clone();
                tag_src.push(title.to_string());
                tag_src.push(pack.to_string());
                index.items.push(Item {
                    id: slug(id),
                    name: humanise(title),
                    kind: Kind::Sound,
                    category: sentence_case(&category),
                    set: humanise(pack),
                    tags: tags(tag_src),
                    file: PathBuf::from(path),
                    preview: None,
                    licence: licence_of(v, Licence::Cc0)?,
                    attribution: s(v, "attribution").unwrap_or("").to_string(),
                    source,
                    details: Details { source_url: s(v, "source_url").map(str::to_string), ..Details::default() },
                });
            }
        }
    }

    // Fonts: one item per family.
    if let Some(items) = tabs.pointer("/text/fonts/items").and_then(Value::as_object) {
        for (id, v) in items {
            let Some(family) = s(v, "family") else { continue };
            let files: Vec<FontFile> = v
                .get("files")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|f| {
                            Some(FontFile {
                                file: PathBuf::from(s(f, "path")?),
                                style: s(f, "style").unwrap_or("normal").to_string(),
                                weight: f.get("weight").and_then(Value::as_u64).unwrap_or(400) as u16,
                                axes: strings(v, "variable_axes"),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            let Some(first) = files.first() else { continue };
            let category = s(v, "category").unwrap_or("Sans");
            let mut tag_src = strings(v, "classifications");
            tag_src.extend(strings(v, "subsets"));
            tag_src.push(category.to_string());
            tag_src.push(family.to_string());
            if let Some(d) = s(v, "designer") {
                tag_src.push(d.to_string());
            }
            index.items.push(Item {
                id: slug(id),
                name: family.to_string(),
                kind: Kind::Font,
                category: sentence_case(category),
                set: "Google Fonts".into(),
                tags: tags(tag_src),
                file: first.file.clone(),
                preview: None,
                licence: licence_of(v, Licence::Ofl11)?,
                attribution: s(v, "attribution").unwrap_or("").to_string(),
                source: google_fonts.clone(),
                details: Details {
                    font_files: files,
                    author: s(v, "designer").map(str::to_string),
                    licence_file: s(v, "licence_file").map(PathBuf::from),
                    ..Details::default()
                },
            });
        }
    }
    Ok(index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> Value {
        json!({
            "sources": {"twemoji": {"url": "https://github.com/jdecked/twemoji", "version": "v17.0.3"}, "openmoji": {"url": "https://github.com/hfg-gmuend/openmoji", "version": "17.0.0"}},
            "tabs": {
                "emoji": {"items": {"emoji:1F600": {"char": "😀", "name": "grinning face", "group": "Smileys & Feelings", "subgroup": "face-smiling", "tags": ["happy", "smile"],
                    "sets": {"openmoji": {"svg": "emoji/openmoji/svg-color/1F600.svg", "png618": "emoji/openmoji/618x618-color/1F600.png", "licence": "CC-BY-SA-4.0", "author": "Emily Jäger"},
                             "twemoji": {"svg": "emoji/twemoji/svg/1f600.svg", "png72": "emoji/twemoji/png72/1f600.png", "licence": "CC-BY-4.0"}}},
                    "emoji:1F3FB": {"char": "🏻", "name": "light skin tone", "group": "Component", "subgroup": "skin-tone", "tags": [], "sets": {}}}},
                "stickers": {"items": {"sticker:fluent:1st-place-medal": {"name": "1st place medal", "set": "Fluent 3D", "png": "stickers/fluent-emoji/assets/1st place medal/3D/1st_place_medal_3d.png", "svg": "stickers/fluent-emoji/assets/1st place medal/Color/1st_place_medal_color.svg", "tags": ["gold", "medal"], "licence": "MIT", "attribution": "Fluent Emoji, Copyright (c) Microsoft Corporation, MIT License"},
                    "sticker:kenney:emotes-pack:emote": {"name": "emote__", "set": "Kenney Emotes", "png": "stickers/kenney/emotes-pack/files/PNG/emote__.png", "svg": null, "tags": ["speech bubble"], "licence": "CC0-1.0", "attribution": "Kenney (www.kenney.nl), CC0"}},
                    "categories": [{"id": "celebrate-play", "name": "Celebrate & Play", "items": ["sticker:fluent:1st-place-medal"]}]},
                "shapes": {"items": {"icon:circle-check": {"name": "circle check", "files": {"outline": "icons/tabler/pkg/package/icons/outline/circle-check.svg", "filled": "icons/tabler/pkg/package/icons/filled/circle-check.svg"}, "tags": ["tick", "done"], "licence": "MIT"}},
                    "categories": [{"id": "shapes", "name": "Shapes", "items": ["icon:circle-check"]}]},
                "audio": {"music": {"items": {
                    "music:incompetech:galway": {"title": "Galway", "artist": "Kevin MacLeod", "path": "music/incompetech/tracks/Galway.mp3", "duration": "00:03:10", "bpm": 120, "genre": "World", "moods": ["Happy"], "instruments": "Fiddle, Guitar", "licence": "CC-BY-4.0", "attribution": "\"Galway\" Kevin MacLeod (incompetech.com) Licensed under Creative Commons: By Attribution 4.0 License http://creativecommons.org/licenses/by/4.0/", "source_url": "https://incompetech.com/x"},
                    "music:opengameart:chip1": {"title": "4 Chiptunes (Adventure) - Juhani Junkala [Chiptune Adventures] 1. Stage 1", "artist": "SubspaceAudio", "path": "music/opengameart/4-chiptunes-adventure/files/Juhani Junkala [Chiptune Adventures] 1. Stage 1.ogg", "duration": null, "bpm": null, "genre": "Retro & Chiptune", "moods": [], "tags": ["chiptune"], "licence": "CC0-1.0", "attribution": "\"4 Chiptunes (Adventure)\" by SubspaceAudio, CC0", "source_url": "https://opengameart.org/content/4-chiptunes-adventure"}}},
                    "sound_effects": {"items": {
                        "sfx:kenney:casino:preview": {"title": "Preview", "path": "sfx/kenney/casino-audio/files/Preview.ogg", "pack": "Kenney Casino Audio", "categories": ["Casino & Money"], "licence": "CC0-1.0", "attribution": "Kenney (www.kenney.nl), CC0"},
                        "sfx:kenney:casino:cards": {"title": "cards-pack-open-1", "path": "sfx/kenney/casino-audio/files/Audio/cards-pack-open-1.ogg", "pack": "Kenney Casino Audio", "categories": ["Casino & Money", "Everyday"], "licence": "CC0-1.0", "attribution": "Kenney (www.kenney.nl), CC0", "source_url": "https://kenney.nl/assets/casino-audio"},
                        "sfx:oga:coin": {"title": "coin1", "path": "sfx/opengameart/coins/files/coin1.wav", "pack": "10 8bit coin sounds", "categories": ["Retro Game"], "licence": "CC0-1.0", "attribution": "\"10 8bit coin sounds\" by Luke.RUSTLTD, CC0"},
                        "sfx:oga:project": {"title": "session", "path": "sfx/opengameart/x/files/session.flp", "pack": "x", "categories": ["Retro Game"], "licence": "CC0-1.0", "attribution": ""}}}},
                "text": {"fonts": {"items": {"font:abeezee": {"family": "ABeeZee", "category": "Sans", "classifications": [], "subsets": ["latin"], "designer": "Anja Meiners", "variable_axes": [],
                    "files": [{"path": "fonts/google-fonts/ofl/abeezee/ABeeZee-Regular.ttf", "style": "normal", "weight": 400}, {"path": "fonts/google-fonts/ofl/abeezee/ABeeZee-Italic.ttf", "style": "italic", "weight": 400}],
                    "licence": "OFL-1.1", "licence_file": "fonts/google-fonts/ofl/abeezee/OFL.txt", "attribution": "Copyright 2011 The ABeeZee Project Authors"}}}}
            }
        })
    }

    #[test]
    fn loads_every_tab_with_human_names_and_licences() {
        let ix = load_value(Path::new("/open"), &sample()).unwrap();
        let by_kind = |k: Kind| ix.by_kind(k).collect::<Vec<_>>();
        let emoji = by_kind(Kind::Emoji);
        assert_eq!(emoji.len(), 2, "two sets, component skipped");
        let tw = emoji.iter().find(|i| i.set == "Twemoji").unwrap();
        assert_eq!(tw.name, "Grinning face");
        assert_eq!(tw.category, "Smileys & feelings");
        assert_eq!(tw.licence, Licence::CcBy4);
        assert_eq!(tw.details.emoji.as_deref(), Some("😀"));
        assert!(tw.tags.contains(&"happy".to_string()));
        let om = emoji.iter().find(|i| i.set == "OpenMoji").unwrap();
        assert!(om.licence.share_alike());
        assert!(om.attribution.contains("Emily Jäger"));
        assert_eq!(om.preview.as_deref(), Some(Path::new("emoji/openmoji/618x618-color/1F600.png")));

        let stickers = by_kind(Kind::Sticker);
        assert_eq!(stickers.len(), 2);
        let fl = stickers.iter().find(|i| i.source.id == "fluent-emoji").unwrap();
        assert_eq!(fl.name, "1st place medal");
        assert_eq!(fl.category, "Celebrate & play");
        assert!(fl.is_vector() && fl.preview.is_some());
        let ke = stickers.iter().find(|i| i.source.id == "kenney").unwrap();
        assert_eq!(ke.name, "Emote");
        assert_eq!(ke.licence, Licence::Cc0);

        let shapes = by_kind(Kind::Shape);
        assert_eq!(shapes[0].name, "Circle check");
        assert!(shapes[0].details.filled_variant.is_some());

        let music = by_kind(Kind::Music);
        let galway = music.iter().find(|i| i.name == "Galway").unwrap();
        assert_eq!(galway.details.duration_secs, Some(190.0));
        assert_eq!(galway.details.bpm, Some(120));
        assert_eq!(galway.set, "Kevin MacLeod");
        assert!(galway.tags.contains(&"fiddle".to_string()));
        let chip = music.iter().find(|i| i.source.id == "opengameart").unwrap();
        assert_eq!(chip.set, "4 Chiptunes (Adventure)");
        assert_eq!(chip.name, "Juhani Junkala 1 Stage 1");

        let sounds = by_kind(Kind::Sound);
        assert_eq!(sounds.len(), 2, "preview reel and project file skipped");
        assert!(sounds.iter().any(|i| i.name == "Cards pack open 1" && i.category == "Casino & money"));
        assert!(sounds.iter().any(|i| i.name == "Coin 1" && i.set == "10 8bit coin sounds"));

        let fonts = by_kind(Kind::Font);
        assert_eq!(fonts[0].name, "ABeeZee");
        assert_eq!(fonts[0].details.font_files.len(), 2);
        assert_eq!(fonts[0].licence, Licence::Ofl11);

        assert!(crate::naming::audit(&ix).is_empty(), "{:?}", crate::naming::audit(&ix));
        assert!(ix.search("medal").iter().any(|h| h.item.kind == Kind::Sticker));
    }

    #[test]
    fn rejects_unknown_licences() {
        let mut v = sample();
        v["tabs"]["shapes"]["items"]["icon:circle-check"]["licence"] = json!("GPL-3.0");
        assert!(load_value(Path::new("/open"), &v).is_err());
    }
}
