// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The person's own 3D models, kept outside any project: what the AI
//! makes (an object, a character, a building, a prop, a place) is saved
//! here, shows in the Library's Assets panel under "Models", and can be
//! dragged into any project or found by the AI before it models the same
//! thing again.
//!
//! Each model is a folder in `Made/Models` of Studio's folder
//! (`studio_paths::studio()`, next to the AI's other kept work):
//!
//! ```text
//! castle/
//!   model.smesh   geometry, parts, materials (with textures) and rig
//!   thumb.png     a picture of it
//!   asset.json    name, kind, tags, hash and what it is made of
//! ```
//!
//! `asset.json` is written last, so a folder without one is unfinished
//! and ignored. A model is known by its content hash: saving the same
//! model again changes nothing, and changing a saved model (the AI
//! carrying on with it) replaces that entry instead of adding another.
//! `STUDIO_MY_MODELS` points elsewhere (tests, portable setups).

use crate::content::Entry;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use studio_content::item::{Kind, Licence};
use studio_mesh::Mesh;

/// What `asset.json` says about a kept model.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Meta {
    pub name: String,
    /// Character, Rigged or Object.
    pub category: String,
    pub tags: Vec<String>,
    /// Content hash of `model.smesh` (the name it has in projects).
    pub hash: String,
    /// Seconds since 1970 when it was first kept; entries stay in this
    /// order so the Library's item numbers never shift.
    pub created: u64,
    pub triangles: usize,
    /// The model's parts, by name, in order.
    pub parts: Vec<String>,
    pub materials: Vec<String>,
    pub bones: Vec<String>,
    pub shape_keys: usize,
    pub actions: usize,
    /// The folder's name; not in the file.
    #[serde(skip)]
    pub slug: String,
}

impl Meta {
    /// One line for the AI: what the model is made of.
    pub fn summary(&self) -> String {
        let mut s = format!("{} triangles", self.triangles);
        if !self.parts.is_empty() {
            s.push_str(&format!("; parts: {}", self.parts.join(", ")));
        }
        if !self.bones.is_empty() {
            s.push_str(&format!("; rigged with {} bones", self.bones.len()));
            if self.actions > 0 {
                s.push_str(&format!(" and {} motion(s)", self.actions));
            }
        }
        s
    }
}

/// What a save did.
#[derive(Clone, Debug, PartialEq)]
pub struct Saved {
    pub slug: String,
    pub name: String,
    pub hash: String,
    /// False when the same model was already kept.
    pub changed: bool,
}

/// Where the models are kept.
pub fn root() -> PathBuf {
    if let Some(p) = std::env::var_os("STUDIO_MY_MODELS").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    // Tests of the app never touch the person's folder.
    #[cfg(test)]
    {
        std::env::temp_dir().join(format!("studio-test-my-models-{}", std::process::id()))
    }
    #[cfg(not(test))]
    {
        studio_paths::made().join("Models")
    }
}

fn slug_of(name: &str) -> String {
    let s: String = name.trim().chars().map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    let s = s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
    if s.is_empty() { "model".into() } else { s }
}

/// Every kept model, oldest first.
///
/// The order only ever grows at the end: a model's place is its `created`
/// second, then when its folder was made (to the nanosecond), then its
/// slug. Library entries are found by position, so a model kept while an
/// earlier answer is still in use (two kept in the same second, say) must
/// not slide in before the ones already listed, and carrying on with a
/// model (which rewrites its files, not its folder) must not move it.
pub fn list_in(root: &Path) -> Vec<Meta> {
    let Ok(rd) = std::fs::read_dir(root) else { return Vec::new() };
    let mut out: Vec<(u128, Meta)> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join("model.smesh").is_file())
        .filter_map(|e| {
            let mut m: Meta = serde_json::from_slice(&std::fs::read(e.path().join("asset.json")).ok()?).ok()?;
            m.slug = e.file_name().to_string_lossy().into_owned();
            let made = e.metadata().ok().and_then(|d| d.created().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
            Some((made, m))
        })
        .collect();
    out.sort_by(|a, b| (a.1.created, a.0, &a.1.slug).cmp(&(b.1.created, b.0, &b.1.slug)));
    out.into_iter().map(|x| x.1).collect()
}

fn category_of(mesh: &Mesh) -> &'static str {
    match mesh.rig.as_deref() {
        Some(r) if r.bones.iter().any(|b| matches!(b.name.as_str(), "hips" | "head" | "spine")) => "Character",
        Some(_) => "Rigged",
        None => "Object",
    }
}

fn meta_of(name: &str, mesh: &Mesh, hash: &str, created: u64) -> Meta {
    let mut parts: Vec<String> = Vec::new();
    for g in &mesh.groups {
        if let Some(m) = mesh.materials.get(g.material as usize) {
            if !parts.contains(&m.name) {
                parts.push(m.name.clone());
            }
        }
    }
    let mut tags: Vec<String> = Vec::new();
    for w in name.split(|c: char| !c.is_alphanumeric()).chain(parts.iter().flat_map(|p| p.split(|c: char| !c.is_alphanumeric()))) {
        let w = w.to_lowercase();
        if w.len() > 1 && !tags.contains(&w) {
            tags.push(w);
        }
    }
    let rig = mesh.rig.as_deref();
    let category = category_of(mesh);
    for t in ["3d", "model", &category.to_lowercase()] {
        if !tags.iter().any(|x| x == t) {
            tags.push(t.to_owned());
        }
    }
    Meta {
        name: name.to_owned(),
        category: category.to_owned(),
        tags,
        hash: hash.to_owned(),
        created,
        triangles: mesh.triangles(),
        parts,
        materials: mesh.materials.iter().map(|m| m.name.clone()).collect(),
        bones: rig.map(|r| r.bones.iter().map(|b| b.name.clone()).collect()).unwrap_or_default(),
        shape_keys: rig.map_or(0, |r| r.shapes.len()),
        actions: rig.map_or(0, |r| r.actions.len()),
        slug: String::new(),
    }
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("part");
    std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Keeps `mesh` as `name` in `root`. The same model (by content) already
/// kept changes nothing. `replaces` is the hash of a kept model this one
/// was made from (the AI carried on with it, under the same name): that
/// entry takes the new content. Otherwise a name already taken by another model gets a number.
pub fn save_in(root: &Path, name: &str, mesh: &Mesh, replaces: Option<&str>) -> Result<Saved, String> {
    if mesh.triangles() == 0 {
        return Err("a model with no surface is not kept".into());
    }
    let name = name.trim();
    let name = if name.is_empty() { "Model" } else { name };
    let bytes = studio_mesh::io::to_bytes(mesh);
    let hash = studio_mesh::io::content_hash(&bytes);
    let kept = list_in(root);
    if let Some(m) = kept.iter().find(|m| m.hash == hash) {
        return Ok(Saved { slug: m.slug.clone(), name: m.name.clone(), hash, changed: false });
    }
    // Only an entry of the same name is carried on: another thing that
    // happens to share its shape is not overwritten.
    let replaced = replaces.and_then(|h| kept.iter().find(|m| m.hash == h && m.name.eq_ignore_ascii_case(name)));
    let (slug, name, created) = match replaced {
        Some(m) => (m.slug.clone(), name.to_owned(), m.created),
        None => {
            let taken = |n: &str| kept.iter().any(|m| m.slug == slug_of(n) || m.name.eq_ignore_ascii_case(n)) || root.join(slug_of(n)).exists();
            let mut n = name.to_owned();
            let mut k = 2;
            while taken(&n) {
                n = format!("{name} {k}");
                k += 1;
            }
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            (slug_of(&n), n, now)
        }
    };
    let dir = root.join(&slug);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    write_file(&dir.join("model.smesh"), &bytes)?;
    write_file(&dir.join("thumb.png"), &studio_media::png::encode(&thumbnail(mesh, 256)))?;
    let meta = meta_of(&name, mesh, &hash, created);
    write_file(&dir.join("asset.json"), &serde_json::to_vec_pretty(&meta).map_err(|e| e.to_string())?)?;
    Ok(Saved { slug, name, hash, changed: true })
}

/// A kept model's mesh.
pub fn load_in(root: &Path, slug: &str) -> Result<Mesh, String> {
    studio_mesh::io::load(&root.join(slug).join("model.smesh"))
}

/// Keeps a model in the person's folder; the Library and the AI's
/// catalogue show it from then on.
pub fn keep(name: &str, mesh: &Mesh, replaces: Option<&str>) -> Result<Saved, String> {
    let s = save_in(&root(), name, mesh, replaces)?;
    if s.changed {
        crate::content::models_changed();
    }
    Ok(s)
}

/// The kept model with this content hash, if any.
pub fn find_hash(hash: &str) -> Option<Meta> {
    list_in(&root()).into_iter().find(|m| m.hash == hash)
}

/// The kept models as Library entries.
pub fn entries() -> Vec<Entry> {
    let root = root();
    list_in(&root)
        .into_iter()
        .map(|m| Entry {
            name: m.name,
            kind: Kind::Model,
            category: m.category,
            set: "My models".into(),
            tags: m.tags,
            path: root.join(&m.slug).join("model.smesh"),
            duration: None,
            size: None,
            attribution: String::new(),
            licence: Licence::Studio.name(),
            credit: false,
        })
        .collect()
}

/// What a kept model is made of, from the `asset.json` beside its file.
pub fn describe(model_file: &Path) -> Option<Meta> {
    let dir = model_file.parent()?;
    let mut m: Meta = serde_json::from_slice(&std::fs::read(dir.join("asset.json")).ok()?).ok()?;
    m.slug = dir.file_name()?.to_string_lossy().into_owned();
    Some(m)
}

// ------------------------------------------------------------ thumbnail

/// A picture of the model, `size` pixels square on transparency: seen
/// from the front-left and a little above, lit from the upper left, in
/// its own colours and textures. (Up is -y and the front faces -z.)
pub fn thumbnail(mesh: &Mesh, size: u32) -> studio_media::rgba::RgbaImage {
    let size = size.max(8) as usize;
    let mut img = studio_media::rgba::RgbaImage::new(size as u32, size as u32);
    if mesh.triangles() == 0 {
        return img;
    }
    let (yaw, pitch) = (0.55f32, 0.35f32);
    let (sy, cy, sp, cp) = (yaw.sin(), yaw.cos(), pitch.sin(), pitch.cos());
    let turn = |v: [f32; 3]| {
        let x = v[0] * cy + v[2] * sy;
        let z = -v[0] * sy + v[2] * cy;
        [x, v[1] * cp - z * sp, v[1] * sp + z * cp]
    };
    let pos: Vec<[f32; 3]> = mesh.positions.iter().map(|&p| turn(p)).collect();
    let nor: Vec<[f32; 3]> = mesh.normals.iter().map(|&n| turn(n)).collect();
    let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
    for p in &pos {
        for i in 0..2 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    let span = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(1e-6);
    let scale = size as f32 * 0.88 / span;
    let off = [(size as f32 - (hi[0] - lo[0]) * scale) / 2.0 - lo[0] * scale, (size as f32 - (hi[1] - lo[1]) * scale) / 2.0 - lo[1] * scale];
    let screen = |p: [f32; 3]| [p[0] * scale + off[0], p[1] * scale + off[1]];
    let light = {
        let l = [-0.45f32, -0.6, -0.65];
        let n = (l[0] * l[0] + l[1] * l[1] + l[2] * l[2]).sqrt();
        [l[0] / n, l[1] / n, l[2] / n]
    };
    let mut depth = vec![f32::INFINITY; size * size];
    let mut tri_material = vec![0usize; mesh.triangles()];
    for g in &mesh.groups {
        for t in g.start..(g.start + g.count).min(mesh.triangles() as u32) {
            tri_material[t as usize] = g.material as usize;
        }
    }
    let fallback = studio_mesh::Material::default();
    for (t, tri) in mesh.indices.chunks_exact(3).enumerate() {
        let idx = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
        if idx.iter().any(|&i| i >= pos.len()) {
            continue;
        }
        let mat = mesh.materials.get(tri_material[t]).unwrap_or(&fallback);
        let s = [screen(pos[idx[0]]), screen(pos[idx[1]]), screen(pos[idx[2]])];
        let area = (s[1][0] - s[0][0]) * (s[2][1] - s[0][1]) - (s[2][0] - s[0][0]) * (s[1][1] - s[0][1]);
        if area.abs() < 1e-9 {
            continue;
        }
        let (x0, x1) = (s.iter().map(|p| p[0]).fold(f32::INFINITY, f32::min).floor().max(0.0) as usize, (s.iter().map(|p| p[0]).fold(f32::NEG_INFINITY, f32::max).ceil().max(0.0) as usize).min(size - 1));
        let (y0, y1) = (s.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min).floor().max(0.0) as usize, (s.iter().map(|p| p[1]).fold(f32::NEG_INFINITY, f32::max).ceil().max(0.0) as usize).min(size - 1));
        for y in y0..=y1 {
            for x in x0..=x1 {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let w0 = ((s[1][0] - px) * (s[2][1] - py) - (s[2][0] - px) * (s[1][1] - py)) / area;
                let w1 = ((s[2][0] - px) * (s[0][1] - py) - (s[0][0] - px) * (s[2][1] - py)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 {
                    continue;
                }
                let z = w0 * pos[idx[0]][2] + w1 * pos[idx[1]][2] + w2 * pos[idx[2]][2];
                if z >= depth[y * size + x] {
                    continue;
                }
                depth[y * size + x] = z;
                let mix = |a: &[[f32; 3]]| [0, 1, 2].map(|k| w0 * a[idx[0]][k] + w1 * a[idx[1]][k] + w2 * a[idx[2]][k]);
                let mut n = if nor.len() == pos.len() { mix(&nor) } else { [0.0; 3] };
                let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                if len < 1e-6 {
                    let (a, b, c) = (pos[idx[0]], pos[idx[1]], pos[idx[2]]);
                    let (u, v) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
                    n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
                }
                let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-9);
                let mut lit = (n[0] * light[0] + n[1] * light[1] + n[2] * light[2]) / len;
                // A surface seen from behind is lit as the side we see.
                if n[2] > 0.0 {
                    lit = -lit;
                }
                let k = 0.35 + 0.65 * lit.max(0.0);
                let mut c = [mat.base_colour[0], mat.base_colour[1], mat.base_colour[2]];
                if let Some(tex) = &mat.texture {
                    if mesh.uvs.len() == pos.len() {
                        let uv = [w0 * mesh.uvs[idx[0]][0] + w1 * mesh.uvs[idx[1]][0] + w2 * mesh.uvs[idx[2]][0], w0 * mesh.uvs[idx[0]][1] + w1 * mesh.uvs[idx[1]][1] + w2 * mesh.uvs[idx[2]][1]];
                        let t = tex.sample(uv);
                        c = [c[0] * t[0], c[1] * t[1], c[2] * t[2]];
                    }
                }
                let rgb = [0, 1, 2].map(|i| {
                    let lin = (c[i] * k * (1.0 - 0.5 * mat.metallic) + mat.emissive[i]).clamp(0.0, 1.0);
                    (lin.powf(1.0 / 2.2) * 255.0 + 0.5) as u8
                });
                let o = (y * size + x) * 4;
                img.pixels[o..o + 4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
            }
        }
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_mesh::rig::{Bone, Rig};
    use studio_mesh::{Group, Material};

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("studio-my-models-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    /// A rigged two-part model: a red box and a green one, a bone each.
    pub(crate) fn sample(shift: f32) -> Mesh {
        let cube = |at: [f32; 3]| {
            let mut m = Mesh::default();
            let c = [[-1.0, -1.0, -1.0], [1.0, -1.0, -1.0], [1.0, 1.0, -1.0], [-1.0, 1.0, -1.0], [-1.0, -1.0, 1.0], [1.0, -1.0, 1.0], [1.0, 1.0, 1.0], [-1.0, 1.0, 1.0f32]];
            m.positions = c.iter().map(|p| [p[0] * 20.0 + at[0], p[1] * 20.0 + at[1] + shift, p[2] * 20.0 + at[2]]).collect();
            m.indices = vec![0, 2, 1, 0, 3, 2, 4, 5, 6, 4, 6, 7, 0, 1, 5, 0, 5, 4, 3, 7, 6, 3, 6, 2, 0, 4, 7, 0, 7, 3, 1, 2, 6, 1, 6, 5];
            m
        };
        let (a, b) = (cube([0.0, 0.0, 0.0]), cube([60.0, 0.0, 0.0]));
        let mut m = a;
        let base = m.positions.len() as u32;
        m.positions.extend(b.positions);
        m.indices.extend(b.indices.iter().map(|i| i + base));
        m.materials = vec![Material { name: "Body".into(), base_colour: [0.8, 0.1, 0.1, 1.0], ..Material::default() }, Material { name: "Arm".into(), base_colour: [0.1, 0.7, 0.2, 1.0], metallic: 0.6, roughness: 0.3, ..Material::default() }];
        m.groups = vec![Group { start: 0, count: 12, material: 0 }, Group { start: 12, count: 12, material: 1 }];
        m.tidy();
        let mut rig = Rig::default();
        rig.bones.push(Bone::new("hips", None, [0.0, 0.0, 0.0], [0.0, -20.0, 0.0]));
        rig.bones.push(Bone::new("arm.L", Some(0), [60.0, 0.0, 0.0], [90.0, 0.0, 0.0]));
        rig.joints = (0..16).map(|i| [if i < 8 { 0 } else { 1 }, 0, 0, 0]).collect();
        rig.weights = vec![[1.0, 0.0, 0.0, 0.0]; 16];
        m.rig = Some(std::sync::Arc::new(rig));
        m
    }

    /// Library entries are found by position, so a model kept later must
    /// land after the ones listed, even in the same second and with a
    /// slug that sorts first, and carrying on with one must not move it.
    #[test]
    fn a_model_kept_later_never_slides_in_before_an_earlier_one() {
        let root = scratch("order");
        let named = save_in(&root, "Zq Named", &sample(0.0), None).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let lineage = save_in(&root, "Zq Lineage", &sample(1.0), None).unwrap();
        // The same second, as two tests in one run have.
        for slug in [&named.slug, &lineage.slug] {
            let f = root.join(slug).join("asset.json");
            let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&f).unwrap()).unwrap();
            v["created"] = serde_json::json!(1_700_000_000u64);
            std::fs::write(&f, serde_json::to_vec(&v).unwrap()).unwrap();
        }
        let order = |root: &Path| list_in(root).into_iter().map(|m| m.name).collect::<Vec<_>>();
        assert_eq!(order(&root), ["Zq Named", "Zq Lineage"], "kept first stays first");
        // Carrying on with Named rewrites its files; it keeps its place.
        std::thread::sleep(std::time::Duration::from_millis(5));
        let again = save_in(&root, "Zq Named", &sample(2.0), Some(&named.hash)).unwrap();
        assert_eq!(again.slug, named.slug);
        assert_eq!(order(&root), ["Zq Named", "Zq Lineage"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_kept_model_comes_back_with_geometry_materials_and_rig() {
        let root = scratch("whole");
        let mesh = sample(0.0);
        let saved = save_in(&root, "Robot Arm", &mesh, None).unwrap();
        assert!(saved.changed);
        let back = load_in(&root, &saved.slug).unwrap();
        assert_eq!(back, mesh, "geometry, groups, materials and rig survive");
        let kept = list_in(&root);
        assert_eq!(kept.len(), 1);
        let m = &kept[0];
        assert_eq!((m.name.as_str(), m.category.as_str()), ("Robot Arm", "Character"));
        assert_eq!(m.parts, vec!["Body", "Arm"]);
        assert_eq!(m.bones, vec!["hips", "arm.L"]);
        assert!(m.tags.contains(&"robot".to_owned()) && m.tags.contains(&"arm".to_owned()));
        assert!(m.summary().contains("rigged with 2 bones"));
        // The thumbnail is a picture with the model in it.
        let thumb = studio_media::png::read(&root.join(&saved.slug).join("thumb.png")).unwrap();
        assert_eq!((thumb.width, thumb.height), (256, 256));
        assert!(thumb.pixels.chunks_exact(4).filter(|p| p[3] == 255).count() > 2000);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn saving_twice_does_not_duplicate() {
        let root = scratch("twice");
        let mesh = sample(0.0);
        let a = save_in(&root, "Robot", &mesh, None).unwrap();
        let b = save_in(&root, "Robot", &mesh, None).unwrap();
        // The same model under another name is still the same model.
        let c = save_in(&root, "Other name", &mesh, None).unwrap();
        assert!(a.changed && !b.changed && !c.changed);
        assert_eq!((a.slug.as_str(), b.slug.as_str(), c.slug.as_str()), ("robot", "robot", "robot"));
        assert_eq!(list_in(&root).len(), 1);
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn carrying_on_with_a_model_replaces_it_and_a_new_one_with_the_same_name_is_numbered() {
        let root = scratch("replace");
        let first = save_in(&root, "Robot", &sample(0.0), None).unwrap();
        let second = save_in(&root, "Robot", &sample(5.0), Some(&first.hash)).unwrap();
        assert!(second.changed);
        assert_eq!(second.slug, first.slug);
        assert_eq!(list_in(&root).len(), 1);
        assert_eq!(load_in(&root, &second.slug).unwrap(), sample(5.0));
        // A model of another name that shares the shape is not overwritten.
        let stool = save_in(&root, "Stool", &sample(7.0), None).unwrap();
        let moved = save_in(&root, "Seat", &sample(8.0), Some(&stool.hash)).unwrap();
        assert_ne!(moved.slug, stool.slug);
        assert_eq!(list_in(&root).len(), 3);
        let other = save_in(&root, "Robot", &sample(9.0), None).unwrap();
        assert_eq!((other.slug.as_str(), other.name.as_str()), ("robot-2", "Robot 2"));
        assert_eq!(list_in(&root).len(), 4);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_unfinished_folder_is_ignored_and_empty_models_are_refused() {
        let root = scratch("unfinished");
        std::fs::create_dir_all(root.join("half")).unwrap();
        std::fs::write(root.join("half/model.smesh"), b"x").unwrap();
        assert!(list_in(&root).is_empty());
        assert!(save_in(&root, "Nothing", &Mesh::default(), None).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_thumbnail_shows_the_model_the_right_way_up() {
        // A tall box: up is -y, so the top of the picture has the model.
        let mut m = sample(0.0);
        for p in &mut m.positions {
            p[1] *= 3.0;
        }
        let t = thumbnail(&m, 64);
        let rows: Vec<usize> = (0..64).map(|y| (0..64).filter(|&x| t.pixels[(y * 64 + x) * 4 + 3] == 255).count()).collect();
        assert!(rows[32] > 0 && rows.iter().filter(|&&n| n > 0).count() > 40, "{rows:?}");
        assert_eq!(thumbnail(&Mesh::default(), 16).pixels.iter().filter(|&&b| b != 0).count(), 0);
    }
}
