// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `asset`: a model built the way a modeler builds one, as named parts.
//! Each part is a quad primitive with its own modifier stack (bevel,
//! subdivision, mirror, array, boolean, solidify, bend, taper…), placement
//! and material (a smart layered one by name, or colour, metal, rough,
//! glow); parts can carry a bone with a pivot, so what moves is rigged
//! whole in the same model. A `kit` starts from a built asset (rock, tree,
//! building, crate, barrel, column, stairs, bolt, pipe, table, chair).
//! The evaluated asset becomes one model clip; each material and bone is
//! named for its part.

use super::models::{field_v3, linear_colour, num, v3};
use super::*;
use studio_mesh::rig::{Bone, Rig};
use studio_mesh::{Material, Mesh, V3};
use studio_modeling::asset::{Asset, Part};
use studio_modeling::material::{self, Lobes, MaterialDef, Shading, Surface};
use studio_modeling::modifiers::Modifier;
use studio_modeling::poly::Poly;

/// Modifier kinds, for the error that lists them.
const KINDS: &str = "mirror, array, radial, solidify, bevel, subdivide, boolean, smooth, weld, triangulate, decimate, displace, bend, twist, taper, wave, cast, shrinkwrap, weighted_normals, remesh";

fn size3(v: Option<&Value>, default: f32) -> Result<V3, String> {
    match v {
        None | Some(Value::Null) => Ok([default; 3]),
        Some(x) if x.is_number() => Ok([x.as_f64().unwrap_or(0.0) as f32; 3]),
        Some(x) => v3(x).ok_or_else(|| "size is a number or [x, y, z] (px)".to_string()),
    }
}

fn primitive(spec: &Value) -> Result<Poly, String> {
    let name = spec.get("prim").or_else(|| spec.get("shape")).and_then(Value::as_str).unwrap_or("box");
    let size = size3(spec.get("size"), 100.0)?;
    let detail = spec.get("detail").and_then(Value::as_u64).unwrap_or(3).clamp(1, 8) as usize;
    if matches!(name, "box" | "cube") {
        return Ok(studio_modeling::prim::cube(size, spec.get("segments").and_then(Value::as_u64).unwrap_or(1).clamp(1, 32) as usize));
    }
    let biggest = size[0].max(size[1]).max(size[2]).max(1.0);
    let mut p = studio_modeling::prim::by_name(name, biggest, detail).ok_or_else(|| format!("prim \"{name}\": {}", studio_modeling::prim::NAMES.join(", ")))?;
    let k = [size[0] / biggest, size[1] / biggest, size[2] / biggest];
    p.map_positions(|v| [v[0] * k[0], v[1] * k[1], v[2] * k[2]]);
    Ok(p)
}

fn modifiers(spec: &Value, label: &str) -> Result<Vec<Modifier>, String> {
    let mut out = Vec::new();
    // Shorthands first: bevel width px (3 segments), subdivide levels.
    if let Some(w) = spec.get("bevel").and_then(Value::as_f64) {
        out.push(Modifier::Bevel { width: w as f32, segments: spec.get("bevel_segments").and_then(Value::as_u64).unwrap_or(3).clamp(1, 8) as usize, profile: 0.5, angle: 30.0, only_marked: false });
    }
    if let Some(l) = spec.get("subdivide").and_then(Value::as_u64) {
        out.push(Modifier::Subdivide { levels: l.min(3) as usize });
    }
    if let Some(list) = spec.get("modifiers").and_then(Value::as_array) {
        for (i, m) in list.iter().enumerate() {
            out.push(serde_json::from_value(m.clone()).map_err(|e| format!("{label}: modifier {} is {{\"kind\": …}} with the kind's fields ({e}); kinds: {KINDS}", i + 1))?);
        }
    }
    Ok(out)
}

/// A part's material: a smart layered one by name (`"material": "brushed_aluminium"`
/// with an optional `colour`), or a plain one from colour, metallic, roughness,
/// emissive, transmission. It is named for the part.
fn material_of(spec: &Value, part: &str, scale: f32) -> Result<MaterialDef, String> {
    let (smart, fields) = match spec.get("material") {
        Some(Value::String(n)) => (Some(n.as_str()), spec.clone()),
        Some(o @ Value::Object(_)) => (o.get("smart").and_then(Value::as_str), o.clone()),
        _ => (None, spec.clone()),
    };
    let colour = match fields.get("colour") {
        Some(c) => {
            let l = linear_colour(c)?;
            Some([l[0], l[1], l[2]])
        }
        None => None,
    };
    let mut def = match smart {
        Some(n) => material::smart(n, scale, colour).ok_or_else(|| format!("material \"{n}\": {}", material::SMART.join(", ")))?,
        None => MaterialDef { name: part.into(), base: Surface { colour: colour.unwrap_or([0.55, 0.55, 0.55]), metallic: 0.0, roughness: 0.5, pattern: None, height: 0.0 }, layers: Vec::new(), shading: Shading::Standard, lobes: Lobes::default(), micro: None },
    };
    if smart.is_some() && colour.is_some() && def.base.pattern.is_none() {
        def.base.colour = colour.unwrap_or(def.base.colour);
    }
    if let Some(v) = num(&fields, "metallic") {
        def.base.metallic = v.clamp(0.0, 1.0);
    }
    if let Some(v) = num(&fields, "roughness") {
        def.base.roughness = v.clamp(0.0, 1.0);
    }
    if let Some(e) = fields.get("emissive") {
        let l = linear_colour(e)?;
        def.lobes.emissive = Some([l[0], l[1], l[2]]);
    }
    if let Some(v) = num(&fields, "transmission") {
        def.lobes.transmission = v.clamp(0.0, 1.0);
        def.shading = Shading::Glass;
    }
    if let Some(v) = num(&fields, "subsurface") {
        def.lobes.subsurface = v.clamp(0.0, 1.0);
    }
    def.name = part.into();
    Ok(def)
}

struct BoneSpec {
    part: usize,
    parent: Option<String>,
    head: V3,
    tail: V3,
}

impl Session {
    /// `asset`: build a model from named parts (see the module notes).
    pub(super) fn asset(&mut self, op: &Value) -> Result<String, String> {
        // Places and styled buildings come from the architecture generators.
        if let Some(k) = op.get("kit").and_then(Value::as_str).map(str::to_lowercase)
            && op.get("parts").is_none()
            && (["facade", "street", "interior", "bridge", "fence"].contains(&k.as_str()) || (op.get("style").is_some() && ["building", "stairs"].contains(&k.as_str())))
        {
            let mut o = op.clone();
            o["kind"] = json!(k);
            return self.build_place(&o);
        }
        let name = op.get("name").and_then(Value::as_str).unwrap_or("Asset").to_owned();
        let mut asset = match op.get("kit").and_then(Value::as_str) {
            Some(k) => studio_modeling::kits::build(k, num(op, "size").unwrap_or(300.0), op.get("seed").and_then(Value::as_u64).unwrap_or(1)).map_err(|e| format!("{e}; kits: {}", studio_modeling::kits::KITS.iter().map(|k| k.0).collect::<Vec<_>>().join(", ")))?,
            None => Asset::new(&name),
        };
        asset.name = name.clone();
        let specs = op.get("parts").and_then(Value::as_array).cloned().unwrap_or_default();
        if specs.is_empty() && op.get("kit").is_none() {
            return Err("asset needs \"parts\": [{\"name\", \"prim\", \"size\", \"at\", \"bevel\", \"subdivide\", \"material\", \"bone\"}] or a \"kit\"".into());
        }
        if specs.len() > 200 {
            return Err("an asset holds up to 200 parts".into());
        }
        let biggest = specs.iter().filter_map(|s| size3(s.get("size"), 100.0).ok()).map(|s| s[0].max(s[1]).max(s[2])).fold(100.0f32, f32::max);
        let scale = num(op, "scale_px").unwrap_or(biggest);
        let mut bones: Vec<BoneSpec> = Vec::new();
        for (i, spec) in specs.iter().enumerate() {
            let pname = spec.get("name").and_then(Value::as_str).ok_or_else(|| format!("part {}: needs \"name\" (the part: body, lid, button…)", i + 1))?.to_owned();
            let label = format!("part \"{pname}\"");
            let mut part = Part::new(&asset.unique_name(&pname), primitive(spec).map_err(|e| format!("{label}: {e}"))?);
            part.position = field_v3(spec, "at")?.unwrap_or([0.0; 3]);
            part.rotation = field_v3(spec, "turn")?.unwrap_or([0.0; 3]);
            part.scale = size3(spec.get("scale"), 1.0)?;
            part.hidden = spec.get("hidden").and_then(Value::as_bool).unwrap_or(false);
            part.modifiers = modifiers(spec, &label)?;
            let def = material_of(spec, &part.name, scale).map_err(|e| format!("{label}: {e}"))?;
            part.materials = vec![def.name.clone()];
            asset.materials.push(def);
            if let Some(b) = spec.get("bone") {
                let head = field_v3(b, "head")?.ok_or_else(|| format!("{label}: bone needs \"head\" [x, y, z], the pivot (a lid's hinge)"))?;
                let tail = field_v3(b, "tail")?.ok_or_else(|| format!("{label}: bone needs \"tail\" [x, y, z], a point along the part from the pivot"))?;
                bones.push(BoneSpec { part: asset.parts.len(), parent: b.get("parent").and_then(Value::as_str).map(str::to_owned), head, tail });
            }
            asset.parts.push(part);
        }
        asset.texture_size = op.get("texture").and_then(Value::as_u64).unwrap_or(512).clamp(64, 2048) as u32;
        let quick = op.get("quick").and_then(Value::as_bool).unwrap_or(false);
        let out = asset.output_parts(quick)?;
        if out.is_empty() {
            return Err("every part is hidden, so there is nothing to show".into());
        }
        // The parts side by side as one model; each vertex remembers its part.
        let mut mesh = Mesh::default();
        let mut tri_mats: Vec<u32> = Vec::new();
        let mut materials: Vec<Material> = Vec::new();
        let mut owner: Vec<usize> = Vec::new();
        for p in &out {
            let m = p.poly.to_mesh(p.smooth_angle, p.mode);
            let base = mesh.positions.len() as u32;
            let first = materials.len() as u32;
            materials.extend(p.materials.iter().cloned());
            mesh.positions.extend_from_slice(&m.positions);
            mesh.normals.extend_from_slice(&m.normals);
            mesh.uvs.extend_from_slice(&m.uvs);
            mesh.indices.extend(m.indices.iter().map(|i| i + base));
            tri_mats.extend(m.tri_materials().iter().map(|s| first + (*s).min(p.materials.len().saturating_sub(1) as u32)));
            let idx = asset.parts.iter().position(|a| a.name == p.name).unwrap_or(0);
            owner.extend(std::iter::repeat_n(idx, m.positions.len()));
        }
        mesh.materials = materials;
        mesh.regroup(&tri_mats);
        mesh.tidy();
        let mut notes = vec![format!("{} part{}: {}", out.len(), if out.len() == 1 { "" } else { "s" }, out.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", "))];
        if !bones.is_empty() {
            let (lo, hi) = mesh.bounds();
            let mid = [(lo[0] + hi[0]) / 2.0, 0.0, (lo[2] + hi[2]) / 2.0];
            let mut rig = Rig::default();
            rig.bones.push(Bone::new("root", None, [mid[0], lo[1], mid[2]], [mid[0], hi[1], mid[2]]));
            let mut of_part: Vec<u16> = vec![0; asset.parts.len()];
            for b in &bones {
                let pname = asset.parts[b.part].name.clone();
                let parent = match &b.parent {
                    Some(p) => rig.bone(p).ok_or_else(|| format!("bone \"{pname}\": no bone \"{p}\" before it to hang from (parents come first; \"root\" always exists)"))?,
                    None => 0,
                };
                rig.bones.push(Bone::new(pname, Some(parent), b.head, b.tail));
                of_part[b.part] = (rig.bones.len() - 1) as u16;
            }
            rig.joints = owner.iter().map(|&p| [of_part[p], 0, 0, 0]).collect();
            rig.weights = owner.iter().map(|_| [1.0, 0.0, 0.0, 0.0]).collect();
            notes.push(format!("{} bones, each part bound whole to its own ({})", rig.bones.len(), rig.bones.iter().map(|b| b.name.as_str()).collect::<Vec<_>>().join(", ")));
            mesh.rig = Some(std::sync::Arc::new(rig));
        }
        // Kept editable: parts, stacks and materials open in edit mode.
        let keep = (asset.scatters.is_empty() && asset.grooms.is_empty()).then_some(&asset);
        self.place_model_with(op, &mesh, name, notes, keep)
    }
}
