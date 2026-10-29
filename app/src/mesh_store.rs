// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Where a project keeps its 3D models: `.smesh` files in its `Models`
//! folder, each named by its content. A change to a model writes a new
//! file and points the clip at it, so the old one is still there for undo.
//!
//! A model modelled by hand (or built from parts by the AI) also keeps
//! its editable source: a `.smodel` beside it (polygons, modifier stack,
//! materials), named by content too, in the clip's `source` parameter.
//! The `.smesh` is what that source evaluates to.

use crate::doc::Doc;
use studio_mesh::Mesh;
use studio_model as m;

/// USD text: `.usda` (and `.usd` saved as text) and a `.usdz` holding a
/// text layer, read by `studio_modeling::formats` (binary USD is not).
const USD: &[&str] = &["usda", "usd", "usdz"];

/// Every model file Zornov reads: the mesh importers' (glTF, OBJ, STL,
/// PLY, FBX and `.smesh`), its own editable source (`.smodel`) and USD text.
pub fn model_extensions() -> Vec<&'static str> {
    let mut v = studio_mesh::import::EXTENSIONS.to_vec();
    v.push("smodel");
    v.extend(USD);
    v
}

/// The formats a model exports to, in the order the dialog lists them.
pub const MODEL_EXPORTS: &[&str] = &["glb", "usdz", "usda", "fbx", "obj", "stl", "ply"];

/// The export dialog's choices, one per [`MODEL_EXPORTS`] entry.
pub const EXPORT_FILTERS: &[(&str, &[&str])] = &[("glTF binary", &["glb"]), ("USDZ", &["usdz"]), ("USD text", &["usda"]), ("FBX", &["fbx"]), ("OBJ", &["obj"]), ("STL", &["stl"]), ("PLY", &["ply"])];

/// The model files an import takes.
pub fn is_model_file(p: &std::path::Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| model_extensions().contains(&e.to_ascii_lowercase().as_str()))
}

/// Whether a file is Zornov's own editable model source (`.smodel`), by
/// its name or its first bytes.
pub fn is_smodel(path: &std::path::Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("smodel"))
        || std::fs::File::open(path).ok().is_some_and(|mut f| {
            use std::io::Read;
            let mut magic = [0u8; 8];
            f.read_exact(&mut magic).is_ok() && &magic == b"SMODEL1\0"
        })
}

/// Reads any model file Zornov reads ([`model_extensions`]), centred and
/// with its largest side `fit` pixels (0 keeps its size). The editable
/// source and USD are built from their parts, faces as they are; the rest
/// go through `studio_mesh::import`.
pub fn read_model(path: &std::path::Path, fit: f32) -> Result<studio_mesh::import::Imported, String> {
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    let (asset, note) = if is_smodel(path) {
        let a = studio_modeling::asset::load(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let n = format!("read the editable source ({} parts)", a.parts.len());
        (a, n)
    } else if USD.contains(&ext.as_str()) {
        let parts = studio_modeling::formats::import(path, 0.0).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut a = studio_modeling::asset::Asset::new(path.file_stem().and_then(|s| s.to_str()).unwrap_or("Model"));
        let n = format!("read {} USD mesh{}", parts.len(), if parts.len() == 1 { "" } else { "es" });
        a.parts.extend(parts.into_iter().map(|(name, poly)| studio_modeling::asset::Part::new(&name, poly)));
        (a, n)
    } else {
        return studio_mesh::import::load(path, fit);
    };
    let mut mesh = asset.to_mesh(false)?;
    if mesh.triangles() == 0 {
        return Err(format!("{}: no triangles in the model", path.display()));
    }
    mesh.tidy();
    if fit > 0.0 {
        studio_mesh::ops::centre_and_fit(&mut mesh, fit);
    }
    Ok(studio_mesh::import::Imported { mesh, notes: vec![note] })
}

/// Writes a model clip to `path` by its extension: glTF (`.glb`) and OBJ
/// with their textures from the triangles; USD (`.usdz`, `.usda`), FBX,
/// STL and PLY from its editable source, faces as they are; `.smesh` and
/// `.smodel`, Zornov's own.
pub fn export_model(c: &m::Clip, mesh: &Mesh, path: &std::path::Path) -> Result<(), String> {
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    let w = |p: &std::path::Path, bytes: &[u8]| std::fs::write(p, bytes).map_err(|e| format!("{}: {e}", p.display()));
    match ext.as_str() {
        "glb" => w(path, &studio_mesh::export::glb(mesh)),
        "obj" => {
            let mtl = path.with_extension("mtl");
            let name = mtl.file_name().and_then(|n| n.to_str()).unwrap_or("model.mtl").to_owned();
            let (o, mt, pictures) = studio_mesh::export::obj(mesh, &name);
            w(path, o.as_bytes())?;
            w(&mtl, mt.as_bytes())?;
            pictures.into_iter().try_for_each(|(file, png)| w(&path.with_file_name(file), &png))
        }
        "smesh" => studio_mesh::io::save(mesh, path),
        "smodel" => studio_modeling::asset::save(&load_asset(c)?.0, path),
        "usdz" | "usda" | "usd" | "fbx" | "stl" | "ply" => {
            let (asset, _) = load_asset(c)?;
            studio_modeling::formats::write(mesh, &asset, path)
        }
        _ => Err(format!("export a model to {}, .smesh or .smodel", MODEL_EXPORTS.iter().map(|e| format!(".{e}")).collect::<Vec<_>>().join(", "))),
    }
}

/// This project's models folder (a shared one until it is saved).
pub fn dir(doc: &Doc) -> std::path::PathBuf {
    match doc.folder.as_ref() {
        Some(f) => f.media_of(m::file::MediaKind::Models3d),
        None => std::env::temp_dir().join("Zornov Models"),
    }
}

/// Saves a model by its content; returns the file's path.
pub fn save(doc: &Doc, mesh: &Mesh) -> Result<String, String> {
    let bytes = studio_mesh::io::to_bytes(mesh);
    let path = dir(doc).join(format!("{}.smesh", studio_mesh::io::content_hash(&bytes)));
    if !path.exists() {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        crate::safety::room_for(&path, bytes.len() as u64)?;
        // Whole or not at all: the name says what the content is, so a
        // file cut short by a crash would be trusted for ever after.
        m::file::write_atomic(&path, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(path.to_string_lossy().into_owned())
}

/// Removes model files in the project's `Models` folder that nothing
/// refers to any more: not the project, its saved versions, nor its undo
/// history (in memory or kept on disk). Every change to a model writes a
/// new content-named file, so a long session of `act` calls left gigabytes
/// of superseded `.smesh` files behind. Files younger than a minute stay
/// (one may be written and not yet pointed at). Returns bytes freed.
pub fn prune(root: &std::path::Path, history: &str) -> u64 {
    let pf = m::file::ProjectFolder::new(root);
    let models = pf.media_of(m::file::MediaKind::Models3d);
    let Ok(entries) = std::fs::read_dir(&models) else { return 0 };
    let mut refs = String::from(history);
    let mut read = |p: &std::path::Path| {
        if let Ok(t) = std::fs::read_to_string(p) {
            refs.push_str(&t);
        }
    };
    for d in pf.reference_dirs() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "json" || x == "jsonl") {
                read(&p);
            }
        }
    }
    let mut freed = 0;
    let now = std::time::SystemTime::now();
    for e in entries.flatten() {
        let p = e.path();
        let Some(name) = p.file_name().and_then(|n| n.to_str()) else { continue };
        if !is_content_named(name) || refs.contains(name) {
            continue;
        }
        let Ok(md) = e.metadata() else { continue };
        if md.modified().ok().and_then(|t| now.duration_since(t).ok()).is_none_or(|age| age.as_secs() < 60) {
            continue;
        }
        if std::fs::remove_file(&p).is_ok() {
            freed += md.len();
        }
    }
    freed
}

/// Whether `name` is one this module wrote: the content hash, then
/// `.smesh` or `.smodel`. In a package the folder also holds models the
/// person imported, which are never judged unused.
fn is_content_named(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".smesh").or_else(|| name.strip_suffix(".smodel")) else { return false };
    stem.len() == 16 && stem.chars().all(|c| c.is_ascii_hexdigit())
}

/// The model file a clip shows, if it is a model clip.
pub fn file_of(c: &m::Clip) -> Option<String> {
    match &c.source {
        m::ClipSource::Generator(g) if g.kind == m::GeneratorKind::Model => match g.params.get("file") {
            Some(m::Value::Text(s)) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// The mesh of a model clip.
pub fn load(c: &m::Clip) -> Result<Mesh, String> {
    let file = file_of(c).ok_or_else(|| format!("clip {} is not a 3D model", c.id.0))?;
    studio_mesh::io::load(std::path::Path::new(&file)).map_err(|e| format!("clip {}'s model: {e}", c.id.0))
}

/// Saves a model's editable source by its content; returns the path.
pub fn save_source(doc: &Doc, asset: &studio_modeling::asset::Asset) -> Result<String, String> {
    let bytes = studio_modeling::asset::to_bytes(asset);
    let path = dir(doc).join(format!("{}.smodel", studio_mesh::io::content_hash(&bytes)));
    if !path.exists() {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        m::file::write_atomic(&path, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(path.to_string_lossy().into_owned())
}

/// The editable source a model clip keeps, if any.
pub fn source_of(c: &m::Clip) -> Option<String> {
    match &c.source {
        m::ClipSource::Generator(g) if g.kind == m::GeneratorKind::Model => match g.params.get("source") {
            Some(m::Value::Text(s)) if !s.is_empty() => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// Points a model clip at a new source (`None`: the model is triangles
/// only, as after a change made on the triangles).
pub fn set_source(c: &mut m::Clip, source: Option<String>) {
    if let m::ClipSource::Generator(g) = &mut c.source {
        match source {
            Some(s) => {
                g.params.insert("source".into(), m::Value::Text(s));
            }
            None => {
                g.params.remove("source");
            }
        }
    }
}

/// An editable source from triangles: welded, joined back into quads
/// where triangles lie flat enough together, shading kept.
pub fn asset_from_mesh(mesh: &Mesh, name: &str) -> studio_modeling::asset::Asset {
    use studio_modeling::asset::{Asset, Part};
    let mut poly = studio_modeling::poly::Poly::from_mesh(mesh);
    studio_modeling::edit::tris_to_quads(&mut poly, 25.0);
    // Shading: a model whose first triangle is shaded flat stays flat.
    let flat = mesh.normals.len() == mesh.positions.len() && mesh.triangles() > 0 && {
        let n = mesh.face_normal(0);
        let [a, b, c] = mesh.tri(0);
        [a, b, c].iter().all(|&v| studio_mesh::dot(mesh.normals[v], n) > 0.999)
    };
    for s in &mut poly.face_smooth {
        *s = !flat;
    }
    poly.ensure_materials();
    let mut a = Asset::new(if name.is_empty() { "Model" } else { name });
    let mut part = Part::new("Body", poly);
    part.smooth_angle = if flat { 0.0 } else { 180.0 };
    a.parts.push(part);
    a
}

/// The editable source kept beside a model in My Models, for a model file
/// of the same content (`<hash>.smesh` here, `source-<hash>.smodel` there).
pub fn kept_source(file: &str) -> Option<std::path::PathBuf> {
    let stem = std::path::Path::new(file).file_stem()?.to_str()?.to_owned();
    if stem.len() < 8 {
        return None;
    }
    let name = format!("source-{stem}.smodel");
    std::fs::read_dir(crate::my_models::root()).ok()?.flatten().map(|d| d.path().join(&name)).find(|p| p.is_file())
}

/// Keeps a model clip's editable source with its entry in My Models.
pub fn keep_source(saved: &crate::my_models::Saved, c: &m::Clip) {
    let Some(src) = source_of(c) else { return };
    let to = crate::my_models::root().join(&saved.slug).join(format!("source-{}.smodel", saved.hash));
    if !to.exists() {
        let _ = std::fs::copy(&src, &to);
    }
}

/// A model clip's editable source and its triangles: the `.smodel` it
/// keeps (or that was kept with it in My Models), or one made from the
/// triangles.
pub fn load_asset(c: &m::Clip) -> Result<(studio_modeling::asset::Asset, Mesh), String> {
    let mesh = load(c)?;
    let kept = || file_of(c).and_then(|f| kept_source(&f)).map(|p| p.to_string_lossy().into_owned());
    if let Some(src) = source_of(c).or_else(kept) {
        match studio_modeling::asset::load(std::path::Path::new(&src)) {
            Ok(a) if !a.parts.is_empty() => return Ok((a, mesh)),
            Ok(_) => {}
            Err(e) => return Err(format!("{}'s editable source: {e}", c.name)),
        }
    }
    Ok((asset_from_mesh(&mesh, &c.name), mesh))
}

/// Makes a modifier part of the mesh (see `Asset::apply_modifier`).
pub fn apply_modifier(a: &mut studio_modeling::asset::Asset, part: usize, i: usize) -> Result<(), String> {
    a.apply_modifier(part, i)
}

/// Points a model clip at a new file.
pub fn set_file(c: &mut m::Clip, file: String) {
    if let m::ClipSource::Generator(g) = &mut c.source {
        g.params.insert("file".into(), m::Value::Text(file));
    }
}

/// A new model clip showing `file`: in 3D, throwing shadows.
pub fn new_clip(id: u64, lane: u64, at: i64, dur: i64, file: String, name: &str) -> m::Clip {
    let mut g = m::Generator::new(m::GeneratorKind::Model);
    g.params.insert("file".into(), m::Value::Text(file));
    let mut c = m::Clip::new(m::ClipId(id), m::LaneId(lane), at, dur, m::ClipSource::Generator(g));
    c.name = name.to_owned();
    c.transform.three_d = true;
    let mut material = c.material.clone().unwrap_or_default();
    material.casts_shadows = true;
    c.material = Some(material);
    c
}

/// After a project moves, model files are found again in its own
/// `Models` folder by name.
pub fn relink(project: &mut m::Project, root: &std::path::Path) {
    let models = m::file::ProjectFolder::new(root).media_of(m::file::MediaKind::Models3d);
    for seq in &mut project.sequences {
        for c in &mut seq.clips {
            let Some(file) = file_of(c) else { continue };
            let p = std::path::Path::new(&file);
            if !p.exists()
                && let Some(name) = p.file_name()
            {
                let here = models.join(name);
                if here.exists() {
                    set_file(c, here.to_string_lossy().into_owned());
                }
            }
            if let Some(src) = source_of(c)
                && !std::path::Path::new(&src).exists()
                && let Some(name) = std::path::Path::new(&src).file_name()
            {
                let here = models.join(name);
                if here.exists() {
                    set_source(c, Some(here.to_string_lossy().into_owned()));
                }
            }
        }
    }
}

/// A new 3D model on a lane of its own at the top, over the whole
/// sequence; selected.
pub fn add_model(app: &mut crate::app::StudioApp, mesh: &studio_mesh::Mesh, name: &str) -> Result<u32, String> {
    let id = add_model_at(&mut app.doc, mesh, name, 0)?;
    app.editor.selection = vec![id];
    Ok(id)
}

/// A new 3D model on a lane of its own at the top, from `at` to the end
/// of the sequence (at least a second). Returns the clip's id.
pub fn add_model_at(doc: &mut Doc, mesh: &studio_mesh::Mesh, name: &str, at: i64) -> Result<u32, String> {
    let file = save(doc, mesh)?;
    let seq = doc.seq();
    let lane = doc.alloc_id();
    let id = doc.alloc_id();
    let dur = (doc.project.duration() - at).max(crate::view::SEC);
    let c = new_clip(id, lane, at, dur, file, name);
    let edits = vec![m::Edit::AddLane { seq, lane: m::Lane::new(m::LaneId(lane), m::LaneKind::Visual, name), index: Some(0) }, m::Edit::InsertClip { seq, clip: Box::new(c) }];
    if !doc.apply(&format!("Add {}", name.to_lowercase()), m::Edit::Batch { edits }) {
        return Err("could not add the model".into());
    }
    Ok(id as u32)
}

/// Model files added to the project as 3D models (each its own layer),
/// sized to fit the canvas.
pub fn import_models(app: &mut crate::app::StudioApp, ctx: &eframe::egui::Context, paths: &[std::path::PathBuf]) {
    let fit = app.doc.project.width.min(app.doc.project.height) as f32 * 0.6;
    for p in paths {
        let name = p.file_stem().and_then(|s| s.to_str()).unwrap_or("Model").to_owned();
        match read_model(p, fit) {
            Ok(mut got) => {
                if got.mesh.triangles() > 1_500_000 {
                    studio_mesh::decimate::decimate(&mut got.mesh, 1_500_000);
                }
                let note = match add_model(app, &got.mesh, &name) {
                    Ok(_) if got.notes.is_empty() => format!("{name} added as a 3D model"),
                    Ok(_) => format!("{name} added; {}", got.notes.join("; ")),
                    Err(e) => format!("Could not add {name}: {e}"),
                };
                crate::kit::push(ctx, note, None, Some(crate::icons::Icon::Shapes));
            }
            Err(e) => {
                crate::kit::push(ctx, format!("Could not import {name}: {e}"), None, Some(crate::icons::Icon::Warning));
            }
        }
    }
}

#[cfg(test)]
mod name_tests {
    use super::is_content_named;

    #[test]
    fn only_names_this_module_wrote_are_judged_unused() {
        assert!(is_content_named("ba8912fc457eb160.smesh"));
        assert!(is_content_named("0123456789abcdef.smodel"));
        assert!(!is_content_named("chair.smesh"), "a model the person imported");
        assert!(!is_content_named("ba8912fc457eb160.glb"));
        assert!(!is_content_named("ba8912fc457eb16.smesh"));
    }
}
