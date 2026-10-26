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

/// The model files an import takes.
pub fn is_model_file(p: &std::path::Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| studio_mesh::import::EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// This project's models folder (a shared one until it is saved).
pub fn dir(doc: &Doc) -> std::path::PathBuf {
    match doc.folder.as_ref() {
        Some(f) => f.root().join("Models"),
        None => std::env::temp_dir().join("Studio Models"),
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
        std::fs::write(&path, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(path.to_string_lossy().into_owned())
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
        std::fs::write(&path, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
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
    let models = root.join("Models");
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
        match studio_mesh::import::load(p, fit) {
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
