// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The 3D library: reusable models (whole assets and kit pieces),
//! materials, environment pictures (HDRIs) and scatter sets, kept in a
//! folder with a small description beside each (name, kind, tags, size in
//! metres, triangles, licence, source) and a thumbnail, so people and the
//! AI can find "a weathered wooden crate" or "a forest-floor scatter" and
//! drop it in.

use crate::asset::Asset;
use crate::material::MaterialDef;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// What an entry is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Model,
    Kit,
    Material,
    Hdri,
    Scatter,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Kind> {
        Some(match s {
            "model" | "models" | "asset" | "assets" => Kind::Model,
            "kit" | "kits" => Kind::Kit,
            "material" | "materials" => Kind::Material,
            "hdri" | "hdris" | "environment" | "environments" | "sky" => Kind::Hdri,
            "scatter" | "scatters" => Kind::Scatter,
            _ => return None,
        })
    }
    fn folder(self) -> &'static str {
        match self {
            Kind::Model => "Models",
            Kind::Kit => "Kits",
            Kind::Material => "Materials",
            Kind::Hdri => "Environments",
            Kind::Scatter => "Scatter",
        }
    }
}

/// An entry's description (the `.json` beside it).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub name: String,
    pub kind: Kind,
    #[serde(default)]
    pub tags: Vec<String>,
    /// What it is in a line ("a 60 cm oak crate with iron corners").
    #[serde(default)]
    pub about: String,
    /// Real size (metres: width, height, depth).
    #[serde(default)]
    pub size_m: Option<[f32; 3]>,
    #[serde(default)]
    pub triangles: usize,
    #[serde(default = "own")]
    pub licence: String,
    #[serde(default)]
    pub source: String,
    /// The file it describes (relative to the entry's folder).
    pub file: String,
    #[serde(default)]
    pub thumbnail: Option<String>,
}

fn own() -> String {
    "made in Studio".into()
}

/// A library folder.
pub struct Library {
    pub root: PathBuf,
}

fn slug(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    let s = s.split('-').filter(|w| !w.is_empty()).collect::<Vec<_>>().join("-");
    if s.is_empty() { "item".into() } else { s }
}

impl Library {
    pub fn at(root: &Path) -> Library {
        Library { root: root.to_path_buf() }
    }

    fn dir(&self, kind: Kind) -> PathBuf {
        self.root.join(kind.folder())
    }

    fn write_entry(&self, e: &Entry) -> Result<PathBuf, String> {
        let dir = self.dir(e.kind);
        std::fs::create_dir_all(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
        let p = dir.join(format!("{}.json", slug(&e.name)));
        std::fs::write(&p, serde_json::to_vec_pretty(e).map_err(|err| err.to_string())?).map_err(|err| format!("{}: {err}", p.display()))?;
        Ok(p)
    }

    /// Saves a model (or a kit piece) with its description and thumbnail
    /// (PNG bytes).
    pub fn save_model(&self, a: &Asset, kind: Kind, tags: &[String], about: &str, thumbnail_png: Option<&[u8]>) -> Result<Entry, String> {
        let dir = self.dir(kind);
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let s = slug(&a.name);
        let file = format!("{s}.smodel");
        crate::asset::save(a, &dir.join(&file))?;
        let thumbnail = match thumbnail_png {
            Some(b) => {
                let t = format!("{s}.png");
                std::fs::write(dir.join(&t), b).map_err(|e| format!("{}: {e}", dir.display()))?;
                Some(t)
            }
            None => None,
        };
        let mesh = a.to_mesh(true).ok();
        let size_m = mesh.as_ref().map(|m| {
            let (lo, hi) = m.bounds();
            [(hi[0] - lo[0]) / a.px_per_metre, (hi[1] - lo[1]) / a.px_per_metre, (hi[2] - lo[2]) / a.px_per_metre]
        });
        let e = Entry { name: a.name.clone(), kind, tags: tags.to_vec(), about: about.into(), size_m, triangles: mesh.map_or(0, |m| m.triangles()), licence: own(), source: String::new(), file, thumbnail };
        self.write_entry(&e)?;
        Ok(e)
    }

    pub fn save_material(&self, m: &MaterialDef, tags: &[String], about: &str) -> Result<Entry, String> {
        let dir = self.dir(Kind::Material);
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let file = format!("{}.material.json", slug(&m.name));
        std::fs::write(dir.join(&file), serde_json::to_vec_pretty(m).map_err(|e| e.to_string())?).map_err(|e| format!("{}: {e}", dir.display()))?;
        let e = Entry { name: m.name.clone(), kind: Kind::Material, tags: tags.to_vec(), about: about.into(), size_m: None, triangles: 0, licence: own(), source: String::new(), file, thumbnail: None };
        self.write_entry(&e)?;
        Ok(e)
    }

    /// Adds an environment picture (copied in) with where it came from and
    /// its licence (CC0 skies from public libraries, or the person's own).
    pub fn add_hdri(&self, picture: &Path, name: &str, licence: &str, source: &str, tags: &[String]) -> Result<Entry, String> {
        let dir = self.dir(Kind::Hdri);
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let ext = picture.extension().and_then(|e| e.to_str()).unwrap_or("hdr").to_ascii_lowercase();
        if !matches!(ext.as_str(), "hdr" | "exr" | "png" | "jpg" | "jpeg") {
            return Err("environment pictures are .hdr, .exr, .png or .jpg (an equirectangular panorama)".into());
        }
        let file = format!("{}.{ext}", slug(name));
        std::fs::copy(picture, dir.join(&file)).map_err(|e| format!("{}: {e}", picture.display()))?;
        let e = Entry { name: name.into(), kind: Kind::Hdri, tags: tags.to_vec(), about: String::new(), size_m: None, triangles: 0, licence: licence.into(), source: source.into(), file, thumbnail: None };
        self.write_entry(&e)?;
        Ok(e)
    }

    /// Every entry of a kind (or all), best matches for `query` first.
    pub fn search(&self, kind: Option<Kind>, query: &str) -> Vec<(Entry, PathBuf)> {
        let kinds: Vec<Kind> = match kind {
            Some(k) => vec![k],
            None => vec![Kind::Model, Kind::Kit, Kind::Material, Kind::Hdri, Kind::Scatter],
        };
        let words: Vec<String> = query.to_lowercase().split_whitespace().map(str::to_owned).collect();
        let mut out: Vec<(usize, Entry, PathBuf)> = Vec::new();
        for k in kinds {
            let Ok(rd) = std::fs::read_dir(self.dir(k)) else { continue };
            for f in rd.flatten() {
                let p = f.path();
                if p.extension().and_then(|e| e.to_str()) != Some("json") || p.to_string_lossy().ends_with(".material.json") {
                    continue;
                }
                let Ok(bytes) = std::fs::read(&p) else { continue };
                let Ok(e) = serde_json::from_slice::<Entry>(&bytes) else { continue };
                let hay = format!("{} {} {}", e.name, e.tags.join(" "), e.about).to_lowercase();
                let score = words.iter().filter(|w| hay.contains(w.as_str())).count();
                if words.is_empty() || score > 0 {
                    let file = self.dir(k).join(&e.file);
                    out.push((score, e, file));
                }
            }
        }
        out.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.name.cmp(&b.1.name)));
        out.into_iter().map(|(_, e, p)| (e, p)).collect()
    }

    pub fn load_model(&self, name: &str) -> Result<Asset, String> {
        let hit = self.search(Some(Kind::Model), name).into_iter().chain(self.search(Some(Kind::Kit), name)).find(|(e, _)| e.name.eq_ignore_ascii_case(name)).ok_or_else(|| format!("no library model \"{name}\""))?;
        crate::asset::load(&hit.1)
    }

    pub fn load_material(&self, name: &str) -> Result<MaterialDef, String> {
        let hit = self.search(Some(Kind::Material), name).into_iter().find(|(e, _)| e.name.eq_ignore_ascii_case(name)).ok_or_else(|| format!("no library material \"{name}\""))?;
        let bytes = std::fs::read(&hit.1).map_err(|e| format!("{}: {e}", hit.1.display()))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", hit.1.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset::Part;
    use crate::prim;

    #[test]
    fn save_find_and_load() {
        let dir = std::env::temp_dir().join(format!("studio-3d-library-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let lib = Library::at(&dir);
        let mut a = Asset::new("Oak Crate");
        a.parts.push(Part::new("Body", prim::cube([60.0; 3], 1)));
        lib.save_model(&a, Kind::Model, &["crate".into(), "wood".into(), "prop".into()], "a 60 cm oak crate", None).unwrap();
        lib.save_material(&crate::material::smart("rusty_iron", 100.0, None).unwrap(), &["metal".into()], "old iron").unwrap();
        let hits = lib.search(None, "wooden crate");
        assert_eq!(hits[0].0.name, "Oak Crate");
        assert!((hits[0].0.size_m.unwrap()[0] - 0.6).abs() < 1e-3);
        assert_eq!(lib.load_model("oak crate").unwrap().parts.len(), 1);
        assert_eq!(lib.load_material("rusty_iron").unwrap().name, "rusty_iron");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
