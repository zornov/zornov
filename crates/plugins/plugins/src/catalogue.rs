// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Plugins that can be installed: the ones that ship with Studio, and the
//! ones listed by plugin indexes the person adds.
//!
//! An index is a JSON file at an https URL:
//!
//! ```json
//! {"plugins": [{"id": "film-grain", "name": "Film grain", "version": "1.2.0",
//!   "description": "...", "provides": "1 effect", "items": ["Film grain"],
//!   "url": "https://example.com/film-grain.wasm", "sha256": "…64 hex…"}]}
//! ```
//!
//! Index URLs live in `<Studio>/Plugins/sources.json` as
//! `{"indexes": ["https://…"]}`; [`refresh`] fetches them into
//! `index-cache.json`. A download is installed only when its SHA-256
//! matches the index.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::store::{Host, Installed, inspect, sha256};

/// Plugins built from `plugins/examples/` and shipped inside Studio.
const BUNDLED: &[(&str, &[u8])] = &[("bundled:duotone", include_bytes!("../../../../plugins/dist/duotone.wasm")), ("bundled:toolkit", include_bytes!("../../../../plugins/dist/toolkit.wasm"))];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub license: String,
    /// "1 effect, 1 transition".
    #[serde(default)]
    pub provides: String,
    /// Item names, for search.
    #[serde(default)]
    pub items: Vec<String>,
    /// `bundled:<name>` or an https URL.
    pub url: String,
    #[serde(default)]
    pub sha256: String,
}

impl Entry {
    pub fn bundled(&self) -> bool {
        self.url.starts_with("bundled:")
    }

    pub fn json(&self, installed: Option<&Installed>) -> Value {
        let mut v = json!({"id": self.id, "name": self.name, "version": self.version, "description": self.description, "author": self.author, "license": self.license, "provides": self.provides, "items": self.items, "from": if self.bundled() { "ships with Zornov".to_owned() } else { self.url.clone() }});
        if let Some(p) = installed {
            v["installed"] = json!(p.manifest.version);
            v["enabled"] = json!(p.enabled);
        }
        v
    }
}

fn bundled() -> &'static [Entry] {
    static B: OnceLock<Vec<Entry>> = OnceLock::new();
    B.get_or_init(|| {
        BUNDLED
            .iter()
            .filter_map(|(url, bytes)| {
                let (m, _) = inspect(bytes, Default::default()).ok()?;
                Some(Entry {
                    items: m.items().iter().map(|(_, _, n)| (*n).to_owned()).collect(),
                    provides: m.summary(),
                    id: m.id,
                    name: m.name,
                    version: m.version,
                    description: m.description,
                    author: m.author,
                    license: m.license,
                    url: (*url).to_owned(),
                    sha256: sha256(bytes),
                })
            })
            .collect()
    })
}

#[derive(Default, Serialize, Deserialize)]
struct Sources {
    #[serde(default)]
    indexes: Vec<String>,
}

#[derive(Default, Serialize, Deserialize)]
struct Index {
    #[serde(default)]
    plugins: Vec<Entry>,
}

/// Every plugin that can be installed, bundled first, then the cached
/// indexes (newest version of each id wins).
pub fn available(host: &Host) -> Vec<Entry> {
    let mut out: Vec<Entry> = bundled().to_vec();
    let cached: Index = std::fs::read(host.dir().join("index-cache.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    for e in cached.plugins {
        if !e.url.starts_with("https://") || e.sha256.len() != 64 {
            continue;
        }
        match out.iter().position(|o| o.id == e.id) {
            Some(i) if newer(&e.version, &out[i].version) => out[i] = e,
            Some(_) => {}
            None => out.push(e),
        }
    }
    out
}

/// Whether semver `a` is newer than `b` (numeric parts only).
pub fn newer(a: &str, b: &str) -> bool {
    let parts = |s: &str| s.split(['.', '-', '+']).map(|p| p.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    parts(a) > parts(b)
}

/// Adds an index URL and fetches every index.
pub fn add_index(host: &Host, url: &str) -> Result<usize, String> {
    if !url.starts_with("https://") {
        return Err("plugin indexes must be https links".into());
    }
    let path = host.dir().join("sources.json");
    let mut s: Sources = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    if !s.indexes.iter().any(|u| u == url) {
        s.indexes.push(url.to_owned());
    }
    std::fs::create_dir_all(host.dir()).map_err(|e| e.to_string())?;
    std::fs::write(&path, serde_json::to_vec_pretty(&s).unwrap_or_default()).map_err(|e| e.to_string())?;
    refresh(host)
}

/// Fetches every index into the cache. Returns how many plugins they list.
pub fn refresh(host: &Host) -> Result<usize, String> {
    let s: Sources = std::fs::read(host.dir().join("sources.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let mut all = Index::default();
    let mut errors = Vec::new();
    for url in &s.indexes {
        match fetch(url, 8 << 20) {
            Ok((200, body)) => match serde_json::from_slice::<Index>(&body) {
                Ok(ix) => all.plugins.extend(ix.plugins),
                Err(e) => errors.push(format!("{url}: not a plugin index ({e})")),
            },
            Ok((status, _)) => errors.push(format!("{url}: HTTP {status}")),
            Err(e) => errors.push(format!("{url}: {e}")),
        }
    }
    std::fs::create_dir_all(host.dir()).map_err(|e| e.to_string())?;
    std::fs::write(host.dir().join("index-cache.json"), serde_json::to_vec_pretty(&all).unwrap_or_default()).map_err(|e| e.to_string())?;
    if !errors.is_empty() && all.plugins.is_empty() {
        return Err(errors.join("; "));
    }
    Ok(all.plugins.len())
}

/// Entries matching any word of `query` (id, name, description, items),
/// best first. An empty query lists everything.
pub fn search<'a>(entries: &'a [Entry], query: &str) -> Vec<&'a Entry> {
    let words: Vec<String> = query.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() > 1).map(str::to_owned).collect();
    if words.is_empty() {
        return entries.iter().collect();
    }
    let mut scored: Vec<(usize, &Entry)> = entries
        .iter()
        .map(|e| {
            let name = format!("{} {}", e.id, e.name).to_lowercase();
            let items = e.items.join(" ").to_lowercase();
            let desc = e.description.to_lowercase();
            let score = words.iter().map(|w| 3 * name.contains(w.as_str()) as usize + 2 * items.contains(w.as_str()) as usize + desc.contains(w.as_str()) as usize).sum();
            (score, e)
        })
        .filter(|(s, _)| *s > 0)
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored.into_iter().map(|(_, e)| e).collect()
}

/// Installs a catalogue plugin by id.
pub fn install(host: &Host, id: &str) -> Result<Installed, String> {
    let entries = available(host);
    let e = entries.iter().find(|e| e.id == id).ok_or_else(|| format!("no plugin `{id}` in the catalogue; search it first"))?;
    if let Some(rest) = e.url.strip_prefix("bundled:") {
        let (_, bytes) = BUNDLED.iter().find(|(u, _)| u.strip_prefix("bundled:") == Some(rest)).ok_or("bundled plugin missing")?;
        return host.install_bytes(bytes, "ships with Zornov");
    }
    let (status, body) = fetch(&e.url, 256 << 20).map_err(|err| format!("could not download {}: {err}", e.url))?;
    if status != 200 {
        return Err(format!("could not download {}: HTTP {status}", e.url));
    }
    let got = sha256(&body);
    if !got.eq_ignore_ascii_case(&e.sha256) {
        return Err(format!("the download of {} does not match its index (SHA-256 {got}, expected {}); nothing was installed", e.name, e.sha256));
    }
    host.install_bytes(&body, &e.url)
}

/// GETs a URL, reading at most `limit` bytes.
fn fetch(url: &str, limit: u64) -> Result<(u16, Vec<u8>), String> {
    use std::io::Read;
    let r = studio_net::get(url, &[], std::time::Duration::from_secs(30))?;
    let mut body = Vec::new();
    r.body.take(limit).read_to_end(&mut body).map_err(|e| e.to_string())?;
    Ok((r.status, body))
}
