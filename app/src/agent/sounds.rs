// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What every short library sound is, by ear: measured once (hit, boom,
//! swell, swoosh, and its length) and kept in the cache folder, so a
//! search for "whoosh" finds sounds shaped like one whatever they are
//! called.

use crate::content;
use studio_content::item::Kind;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Heard {
    pub kind: String,
    pub secs: f32,
}

fn cache_file() -> PathBuf {
    studio_paths::cache().join("sound-kinds.json")
}

/// Path → what it sounds like, for every library sound effect.
pub fn heard() -> &'static HashMap<PathBuf, Heard> {
    static MAP: OnceLock<HashMap<PathBuf, Heard>> = OnceLock::new();
    MAP.get_or_init(|| {
        let mut map: HashMap<PathBuf, Heard> = std::fs::read(cache_file()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let Some(c) = content::get_blocking() else { return map };
        let todo: Vec<PathBuf> = c.entries.iter().filter(|e| e.kind == Kind::Sound && e.duration.is_none_or(|d| d < 8.0) && !map.contains_key(&e.path)).map(|e| e.path.clone()).collect();
        if !todo.is_empty() {
            let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
            let chunks: Vec<&[PathBuf]> = todo.chunks(todo.len().div_ceil(threads).max(1)).collect();
            let found: Vec<(PathBuf, Heard)> = std::thread::scope(|sc| {
                let hs: Vec<_> = chunks
                    .iter()
                    .map(|ps| {
                        sc.spawn(move || {
                            ps.iter()
                                .filter_map(|p| {
                                    let d = crate::sound_bridge::decode_uncached(p)?;
                                    let mono: Vec<f32> = d.left.iter().zip(d.right.iter()).map(|(l, r)| 0.5 * (l + r)).collect();
                                    let secs = mono.len() as f32 / d.rate.max(1) as f32;
                                    let kind = if secs >= 8.0 { "long".to_owned() } else { super::listen::sound_kind(&mono, d.rate).to_owned() };
                                    Some((p.clone(), Heard { kind, secs }))
                                })
                                .collect::<Vec<_>>()
                        })
                    })
                    .collect();
                hs.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
            });
            map.extend(found);
            if let Ok(json) = serde_json::to_vec(&map) {
                let _ = std::fs::create_dir_all(studio_paths::cache());
                let _ = std::fs::write(cache_file(), json);
            }
        }
        map
    })
}

/// The kind of sound a search word asks for, if it names one.
pub fn wanted(word: &str) -> Option<&'static str> {
    Some(match word {
        "whoosh" | "swoosh" | "woosh" | "swish" | "swosh" | "transition" | "pass" | "flyby" => "swoosh",
        "riser" | "rise" | "swell" | "build" | "buildup" | "sweep" | "uplifter" => "swell",
        "boom" | "explosion" | "drop" | "bass" | "thunder" | "blast" | "cinematic" => "boom",
        "hit" | "impact" | "punch" | "thud" | "kick" | "knock" | "slam" | "smack" => "hit",
        _ => return None,
    })
}
