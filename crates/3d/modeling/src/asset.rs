// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A 3D asset as a modeler keeps it: named parts, each a polygon mesh with
//! its own modifier stack, placement, materials and (optionally) a dense
//! sculpted version to bake from; layered materials; hair grooms. Saved as
//! a `.smodel` file (a JSON description followed by the meshes in binary),
//! and evaluated into the triangles, textures and materials Studio draws.

use crate::bake::{self, BakeSettings};
use crate::groom::{self, Groom};
use crate::material::{self, MaterialDef};
use crate::modifiers::{self, Modifier};
use crate::poly::{Edge, NormalMode, Poly};
use crate::select::Faces;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use studio_mesh::{Material, Mesh, V3, add, length, sub};

fn one3() -> V3 {
    [1.0; 3]
}
fn angle() -> f32 {
    30.0
}

/// One part of an asset.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Part {
    pub name: String,
    #[serde(skip)]
    pub mesh: Poly,
    /// A dense (sculpted) version of the part, baked onto this one's
    /// textures.
    #[serde(skip)]
    pub detail: Option<Poly>,
    #[serde(default)]
    pub modifiers: Vec<Modifier>,
    /// Modifiers switched off (by place in the stack; missing ones are
    /// on): kept in the stack, left out when the part is evaluated.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disabled: Vec<bool>,
    #[serde(default)]
    pub position: V3,
    /// Degrees about x, y, z (applied in that order).
    #[serde(default)]
    pub rotation: V3,
    #[serde(default = "one3")]
    pub scale: V3,
    /// Material names for the mesh's material slots (slot = index).
    #[serde(default)]
    pub materials: Vec<String>,
    /// Hidden parts don't draw (boolean cutters, wrap targets).
    #[serde(default)]
    pub hidden: bool,
    /// Edges sharper than this shade hard.
    #[serde(default = "angle")]
    pub smooth_angle: f32,
    /// What was done to it, in words (for the AI and the history panel).
    #[serde(default)]
    pub history: Vec<String>,
    /// A procedural recipe the mesh is made from each time (when set).
    #[serde(default)]
    pub recipe: Option<crate::nodes::Graph>,
}

impl Part {
    pub fn new(name: &str, mesh: Poly) -> Part {
        Part { name: name.into(), mesh, detail: None, modifiers: Vec::new(), disabled: Vec::new(), position: [0.0; 3], rotation: [0.0; 3], scale: [1.0; 3], materials: Vec::new(), hidden: false, smooth_angle: 30.0, history: Vec::new(), recipe: None }
    }

    /// The modifiers that are switched on, in stack order.
    pub fn active_modifiers(&self) -> Vec<Modifier> {
        self.modifiers.iter().enumerate().filter(|(i, _)| !self.disabled.get(*i).copied().unwrap_or(false)).map(|(_, m)| m.clone()).collect()
    }

    /// Model → asset placement.
    pub fn place(&self, p: V3) -> V3 {
        let r = studio_mesh::ops::rotation(self.rotation);
        let s = [p[0] * self.scale[0], p[1] * self.scale[1], p[2] * self.scale[2]];
        let q = [studio_mesh::dot(r[0], s), studio_mesh::dot(r[1], s), studio_mesh::dot(r[2], s)];
        add(q, self.position)
    }
}

/// A groom on a part.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GroomOn {
    pub part: String,
    /// Which faces it grows from (a face set, a material slot, or all).
    #[serde(default)]
    pub face_set: Option<u32>,
    #[serde(default)]
    pub material_slot: Option<u32>,
    /// Or faces looking within 70° of a direction (the top of a head).
    #[serde(default)]
    pub facing: Option<V3>,
    pub groom: Groom,
}

/// A part ready to draw or export.
pub struct OutPart {
    pub name: String,
    /// Placed in the asset, modifiers run.
    pub poly: Poly,
    pub materials: Vec<Material>,
    pub smooth_angle: f32,
    pub mode: NormalMode,
}

/// The whole asset.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Asset {
    pub name: String,
    #[serde(default)]
    pub parts: Vec<Part>,
    #[serde(default)]
    pub materials: Vec<MaterialDef>,
    #[serde(default)]
    pub grooms: Vec<GroomOn>,
    /// Copies scattered over parts.
    #[serde(default)]
    pub scatters: Vec<crate::scatter::Scatter>,
    /// Texture size for baked materials (px per side).
    #[serde(default = "tex")]
    pub texture_size: u32,
    /// Pixels per metre, for measurements in real units (100 by default:
    /// a 1 px grid is a centimetre).
    #[serde(default = "ppm")]
    pub px_per_metre: f32,
}

fn tex() -> u32 {
    1024
}
fn ppm() -> f32 {
    100.0
}

impl Asset {
    pub fn new(name: &str) -> Asset {
        Asset { name: name.into(), parts: Vec::new(), materials: Vec::new(), grooms: Vec::new(), scatters: Vec::new(), texture_size: 1024, px_per_metre: 100.0 }
    }

    pub fn part(&self, name: &str) -> Option<&Part> {
        self.parts.iter().find(|p| p.name == name)
    }

    pub fn part_mut(&mut self, name: &str) -> Option<&mut Part> {
        self.parts.iter_mut().find(|p| p.name == name)
    }

    pub fn material(&self, name: &str) -> Option<&MaterialDef> {
        self.materials.iter().find(|m| m.name == name)
    }

    /// A free part name based on `base`.
    pub fn unique_name(&self, base: &str) -> String {
        if self.part(base).is_none() {
            return base.into();
        }
        (2..).map(|i| format!("{base} {i}")).find(|n| self.part(n).is_none()).unwrap()
    }

    /// Each part with its modifier stack run (other parts as booleans and
    /// wrap targets see the stacks of the parts before them).
    pub fn evaluated(&self) -> Result<Vec<(String, modifiers::Evaluated)>, String> {
        let mut done: HashMap<String, Poly> = HashMap::new();
        // Parts are evaluated in an order where referenced parts come first.
        let mut order: Vec<usize> = Vec::new();
        let mut placed: HashSet<usize> = HashSet::new();
        let refs = |p: &Part| -> Vec<String> {
            p.active_modifiers().into_iter().filter_map(|m| match m {
                Modifier::Boolean { with, .. } => Some(with),
                Modifier::Shrinkwrap { target, .. } => Some(target),
                _ => None,
            }).collect()
        };
        for _ in 0..self.parts.len() + 1 {
            for (i, p) in self.parts.iter().enumerate() {
                if placed.contains(&i) {
                    continue;
                }
                let ready = refs(p).iter().all(|n| self.parts.iter().position(|q| &q.name == n).is_none_or(|j| placed.contains(&j)));
                if ready {
                    order.push(i);
                    placed.insert(i);
                }
            }
        }
        if order.len() != self.parts.len() {
            return Err("parts refer to each other in a loop (a boolean or wrap that leads back to itself)".into());
        }
        let mut out: Vec<Option<(String, modifiers::Evaluated)>> = (0..self.parts.len()).map(|_| None).collect();
        for i in order {
            let p = &self.parts[i];
            let base = match &p.recipe {
                Some(g) => g.eval().map_err(|e| format!("part \"{}\"'s recipe: {e}", p.name))?,
                None => p.mesh.clone(),
            };
            let ev = modifiers::apply(&base, &p.active_modifiers(), &done).map_err(|e| format!("part \"{}\": {e}", p.name))?;
            // Other parts see it placed in the asset.
            let mut placed_poly = ev.poly.clone();
            placed_poly.map_positions(|v| p.place(v));
            done.insert(p.name.clone(), placed_poly);
            out[i] = Some((p.name.clone(), ev));
        }
        Ok(out.into_iter().flatten().collect())
    }

    /// Every visible part as it will be drawn: modifiers run, placed in
    /// the asset, with its final materials (smart and layered ones baked
    /// into textures, unwrapping parts that have no texture layout, unless
    /// `quick`, which uses each material's average colour and finish).
    pub fn output_parts(&self, quick: bool) -> Result<Vec<OutPart>, String> {
        let mut out = Vec::new();
        for (name, ev) in self.evaluated()? {
            let part = self.part(&name).unwrap();
            if part.hidden {
                continue;
            }
            let mut poly = ev.poly;
            let needs_bake = !quick && part.materials.iter().any(|m| self.material(m).is_some_and(|d| !d.layers.is_empty() || d.base.pattern.is_some() || d.micro.is_some()));
            if needs_bake && !poly.has_uvs() {
                crate::uv::unwrap(&mut poly, &crate::uv::Unwrap::default());
            }
            let baked: Vec<Material> = if needs_bake { self.bake_part(part, &poly)? } else { Vec::new() };
            poly.ensure_materials();
            let materials: Vec<Material> = (0..poly.materials.len().max(part.materials.len()))
                .map(|slot| match baked.get(slot) {
                    Some(m) => m.clone(),
                    None => match part.materials.get(slot).and_then(|n| self.material(n)) {
                        // A plain definition over a textured slot keeps the
                        // texture (an imported model's colours).
                        Some(def) => {
                            let mut m = flat_material(def);
                            m.texture = poly.materials.get(slot).and_then(|m| m.texture.clone());
                            m
                        }
                        None => poly.materials.get(slot).cloned().unwrap_or_default(),
                    },
                })
                .collect();
            poly.map_positions(|v| part.place(v));
            if part.scale[0] * part.scale[1] * part.scale[2] < 0.0 {
                for f in 0..poly.faces() {
                    poly.flip_face(f);
                }
            }
            poly.materials = materials.clone();
            let mode = if ev.weighted_normals { NormalMode::Area } else { NormalMode::Angle };
            out.push(OutPart { name, poly, materials, smooth_angle: part.smooth_angle, mode });
        }
        Ok(out)
    }

    /// Hair strands as drawn, per groom: (groom, ribbons).
    pub fn output_hair(&self, parts: &[OutPart]) -> Vec<(Groom, Mesh)> {
        let mut out = Vec::new();
        for g in &self.grooms {
            let Some(op) = parts.iter().find(|p| p.name == g.part) else { continue };
            let m = op.poly.to_mesh(op.smooth_angle, op.mode);
            let mut tri_face = Vec::with_capacity(m.triangles());
            for f in 0..op.poly.faces() {
                for _ in 0..op.poly.face(f).len().saturating_sub(2) {
                    tri_face.push(f);
                }
            }
            let mats = m.tri_materials();
            let faces: Vec<usize> = (0..m.triangles())
                .filter(|&t| match (g.facing, g.material_slot, g.face_set) {
                    (Some(d), _, _) => studio_mesh::dot(m.face_normal(t), studio_mesh::normalise(d)) > 70f32.to_radians().cos(),
                    (None, Some(s), _) => mats[t] == s,
                    (None, None, Some(set)) => tri_face.get(t).is_some_and(|&f| op.poly.face_set[f] == set),
                    _ => true,
                })
                .collect();
            let strands = groom::grow(&m, &faces, &g.groom);
            out.push((g.groom.clone(), groom::ribbons(&strands, &g.groom)));
        }
        out
    }

    /// Triangles, materials and textures for drawing (see
    /// [`Asset::output_parts`]).
    pub fn to_mesh(&self, quick: bool) -> Result<Mesh, String> {
        let parts = self.output_parts(quick)?;
        let mut out = Mesh::default();
        let mut tri_mats: Vec<u32> = Vec::new();
        let mut materials: Vec<Material> = Vec::new();
        let mut add = |out: &mut Mesh, mesh: &Mesh, mats: &[Material], tri_mats: &mut Vec<u32>| {
            let base = out.positions.len() as u32;
            let first = materials.len() as u32;
            materials.extend(mats.iter().cloned());
            out.positions.extend_from_slice(&mesh.positions);
            out.normals.extend_from_slice(&mesh.normals);
            out.uvs.extend_from_slice(&mesh.uvs);
            out.indices.extend(mesh.indices.iter().map(|i| i + base));
            tri_mats.extend(mesh.tri_materials().iter().map(|s| first + (*s).min(mats.len().saturating_sub(1) as u32)));
        };
        for p in &parts {
            let mesh = p.poly.to_mesh(p.smooth_angle, p.mode);
            add(&mut out, &mesh, &p.materials, &mut tri_mats);
        }
        for (_, r) in self.output_hair(&parts) {
            add(&mut out, &r, &r.materials.clone(), &mut tri_mats);
        }
        if !self.scatters.is_empty() {
            let evals = self.evaluated()?;
            for s in self.scatters.iter().filter(|s| s.visible) {
                let Some(on) = parts.iter().find(|p| p.name == s.on) else { continue };
                let surface = on.poly.to_mesh(on.smooth_angle, on.mode);
                let source = match evals.iter().find(|(n, _)| *n == s.source) {
                    Some((n, ev)) => {
                        let part = self.part(n).unwrap();
                        let mut m = ev.poly.to_mesh(part.smooth_angle, if ev.weighted_normals { NormalMode::Area } else { NormalMode::Angle });
                        m.materials = (0..m.materials.len().max(1)).map(|slot| part.materials.get(slot).and_then(|x| self.material(x)).map(flat_material).unwrap_or_else(|| m.materials.get(slot).cloned().unwrap_or_default())).collect();
                        m
                    }
                    None if s.source.ends_with(".smodel") => load(std::path::Path::new(&s.source))?.to_mesh(true)?,
                    None => studio_mesh::import::load(std::path::Path::new(&s.source), 0.0).map(|g| g.mesh).map_err(|e| format!("scatter source \"{}\": {e}", s.source))?,
                };
                let (lo, hi) = source.bounds();
                let inst = crate::scatter::place(&surface, s, hi[1] - lo[1]);
                let m = crate::scatter::build(&source, &inst);
                add(&mut out, &m, &m.materials.clone(), &mut tri_mats);
            }
        }
        if materials.is_empty() {
            materials.push(Material::default());
        }
        out.materials = materials;
        out.regroup(&tri_mats);
        out.tidy();
        Ok(out)
    }

    /// Bakes a part's materials into textures (one texture set per slot).
    fn bake_part(&self, part: &Part, poly: &Poly) -> Result<Vec<Material>, String> {
        let size = self.texture_size.clamp(64, 4096);
        let (lo, hi) = poly.bounds();
        let extent = length(sub(hi, lo)).max(1.0);
        let settings = BakeSettings { size, reach: extent * 0.02, ao_samples: 16, ao_distance: extent * 0.15, padding: 8, smooth_angle: part.smooth_angle };
        let shape = match &part.detail {
            Some(d) => bake::bake(poly, &bake::Source::new(d.to_mesh(180.0, NormalMode::Angle)), &settings),
            None => bake::bake_self(poly, &settings),
        };
        let texels = bake::texels(poly, size, part.smooth_angle);
        let area: f32 = (0..poly.faces()).map(|f| poly.face_area(f)).sum();
        let uv_area = crate::uv::quality(poly).coverage.max(1e-6);
        let px_per_texel = (area / uv_area).sqrt() / size as f32;
        let nslots = poly.face_material.iter().copied().max().map_or(1, |m| m as usize + 1).max(part.materials.len());
        let mut out = Vec::new();
        for slot in 0..nslots {
            let def = part.materials.get(slot).and_then(|n| self.material(n));
            let Some(def) = def else {
                out.push(poly.materials.get(slot).cloned().unwrap_or_default());
                continue;
            };
            // Texels of this slot only.
            let mine: Vec<bake::Texel> = texels.iter().copied().filter(|t| poly.face_material[t.face as usize] as usize == slot).collect();
            let set = material::bake_material(def, &mine, &shape, size, px_per_texel);
            let mut m = flat_material(def);
            m.base_colour = [1.0, 1.0, 1.0, 1.0];
            // Until renderers read every map, occlusion is multiplied into
            // the colour so crevices read.
            let mut colour = set.base_colour.clone();
            for i in 0..(size * size) as usize {
                let ao = 0.35 + 0.65 * set.orm.data[i * 3];
                for k in 0..3 {
                    colour.data[i * 4 + k] *= ao;
                }
            }
            m.texture = Some(colour.to_rgba8(true));
            let avg_r: f32 = mine.iter().map(|t| set.orm.get(t.x, t.y)[1]).sum::<f32>() / mine.len().max(1) as f32;
            let avg_m: f32 = mine.iter().map(|t| set.orm.get(t.x, t.y)[2]).sum::<f32>() / mine.len().max(1) as f32;
            m.roughness = avg_r;
            m.metallic = avg_m;
            out.push(m);
        }
        Ok(out)
    }

    /// Makes one modifier's change part of a part's base mesh (other parts
    /// it refers to as they are drawn) and takes it off the stack.
    pub fn apply_modifier(&mut self, part: usize, i: usize) -> Result<(), String> {
        let md = self.parts.get(part).and_then(|p| p.modifiers.get(i)).cloned().ok_or("no such modifier")?;
        let mut done: HashMap<String, Poly> = HashMap::new();
        for (name, ev) in self.evaluated()? {
            if name != self.parts[part].name {
                let pp = self.part(&name).unwrap();
                let mut placed = ev.poly.clone();
                placed.map_positions(|v| pp.place(v));
                done.insert(name, placed);
            }
        }
        let ev = modifiers::apply(&self.parts[part].mesh, &[md], &done)?;
        let p = &mut self.parts[part];
        let mats = p.mesh.materials.clone();
        p.mesh = ev.poly;
        if p.mesh.materials.len() < mats.len() {
            p.mesh.materials = mats;
        }
        p.mesh.ensure_materials();
        p.modifiers.remove(i);
        if i < p.disabled.len() {
            p.disabled.remove(i);
        }
        Ok(())
    }

    /// The parts' edges flagged in words, for describing.
    pub fn describe(&self) -> serde_json::Value {
        let parts: Vec<serde_json::Value> = self
            .parts
            .iter()
            .map(|p| {
                let (lo, hi) = p.mesh.bounds();
                serde_json::json!({
                    "name": p.name,
                    "mesh": p.mesh.stats(),
                    "box_min": [lo[0].round(), lo[1].round(), lo[2].round()],
                    "box_max": [hi[0].round(), hi[1].round(), hi[2].round()],
                    "position": p.position,
                    "modifiers": p.modifiers.iter().map(|m| m.name()).collect::<Vec<_>>(),
                    "materials": p.materials,
                    "hidden": p.hidden,
                    "uvs": p.mesh.has_uvs(),
                    "creased_edges": p.mesh.creases.len(),
                    "sharp_edges": p.mesh.sharp.len(),
                    "detail": p.detail.as_ref().map(|d| d.stats()),
                })
            })
            .collect();
        serde_json::json!({"name": self.name, "parts": parts, "materials": self.materials.iter().map(|m| &m.name).collect::<Vec<_>>(), "grooms": self.grooms.iter().map(|g| format!("{} on {}", g.groom.name, g.part)).collect::<Vec<_>>()})
    }
}

/// A material's flat stand-in.
pub fn flat_material(def: &MaterialDef) -> Material {
    let a = def.average();
    let l = &def.lobes;
    Material {
        name: def.name.clone(),
        base_colour: [a.colour[0], a.colour[1], a.colour[2], if def.shading == material::Shading::Glass { 0.25 } else { 1.0 }],
        metallic: a.metallic,
        roughness: a.roughness,
        emissive: l.emissive.unwrap_or([0.0; 3]),
        subsurface: l.subsurface,
        ..Material::default()
    }
}

// ---------------------------------------------------------------- files

const MAGIC: &[u8; 8] = b"SMODEL1\0";
/// After the parts: each part's own flat materials (with their textures),
/// as a mesh file holding materials only. Older readers stop before it.
const MATERIALS: &[u8; 8] = b"SMATS1\0\0";

fn put_u32(b: &mut Vec<u8>, v: u32) {
    b.extend_from_slice(&v.to_le_bytes());
}
fn put_f32(b: &mut Vec<u8>, v: f32) {
    b.extend_from_slice(&v.to_le_bytes());
}

/// A polygon mesh as bytes (materials are kept by the asset, not here).
pub fn poly_bytes(p: &Poly) -> Vec<u8> {
    let mut b = Vec::new();
    put_u32(&mut b, p.positions.len() as u32);
    for v in &p.positions {
        for x in v {
            put_f32(&mut b, *x);
        }
    }
    put_u32(&mut b, p.faces() as u32);
    for f in 0..p.faces() {
        put_u32(&mut b, p.face(f).len() as u32);
        for &v in p.face(f) {
            put_u32(&mut b, v);
        }
        put_u32(&mut b, p.face_material[f]);
        put_u32(&mut b, p.face_set[f]);
        b.push(p.face_smooth[f] as u8);
    }
    put_u32(&mut b, if p.has_uvs() { p.uvs.len() as u32 } else { 0 });
    if p.has_uvs() {
        for u in &p.uvs {
            put_f32(&mut b, u[0]);
            put_f32(&mut b, u[1]);
        }
    }
    let mut creases: Vec<(&Edge, &f32)> = p.creases.iter().collect();
    creases.sort_by_key(|(e, _)| **e);
    put_u32(&mut b, creases.len() as u32);
    for (e, w) in creases {
        put_u32(&mut b, e.0);
        put_u32(&mut b, e.1);
        put_f32(&mut b, *w);
    }
    for set in [&p.sharp, &p.seams] {
        let mut v: Vec<&Edge> = set.iter().collect();
        v.sort();
        put_u32(&mut b, v.len() as u32);
        for e in v {
            put_u32(&mut b, e.0);
            put_u32(&mut b, e.1);
        }
    }
    b
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn u32(&mut self) -> Result<u32, String> {
        let s = self.b.get(self.at..self.at + 4).ok_or("the model file ends early")?;
        self.at += 4;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn f32(&mut self) -> Result<f32, String> {
        Ok(f32::from_bits(self.u32()?))
    }
    fn u8(&mut self) -> Result<u8, String> {
        let v = *self.b.get(self.at).ok_or("the model file ends early")?;
        self.at += 1;
        Ok(v)
    }
    fn count(&mut self, max: usize) -> Result<usize, String> {
        let n = self.u32()? as usize;
        if n > max {
            return Err("the model file is damaged (a count is impossibly large)".into());
        }
        Ok(n)
    }
}

pub fn poly_from_bytes(b: &[u8]) -> Result<Poly, String> {
    let mut r = Reader { b, at: 0 };
    let limit = b.len();
    let nv = r.count(limit / 12)?;
    let mut p = Poly::new();
    for _ in 0..nv {
        p.positions.push([r.f32()?, r.f32()?, r.f32()?]);
    }
    let nf = r.count(limit / 4)?;
    let mut faces = Vec::with_capacity(nf);
    for _ in 0..nf {
        let n = r.count(limit / 4)?;
        let mut vs = Vec::with_capacity(n);
        for _ in 0..n {
            let v = r.u32()?;
            if v as usize >= nv {
                return Err("the model file is damaged (a face uses a missing point)".into());
            }
            vs.push(v);
        }
        let (m, s, sm) = (r.u32()?, r.u32()?, r.u8()? != 0);
        faces.push((vs, m, s, sm));
    }
    let nuv = r.count(limit / 8)?;
    let mut uvs = Vec::with_capacity(nuv);
    for _ in 0..nuv {
        uvs.push([r.f32()?, r.f32()?]);
    }
    for (vs, m, s, sm) in faces {
        p.add_face_like(&vs, None, m, s, sm);
    }
    if nuv == p.corners.len() {
        p.uvs = uvs;
    } else {
        p.uvs.clear();
    }
    let nc = r.count(limit / 12)?;
    for _ in 0..nc {
        let (a, b2, w) = (r.u32()?, r.u32()?, r.f32()?);
        p.creases.insert((a, b2), w);
    }
    for k in 0..2 {
        let n = r.count(limit / 8)?;
        for _ in 0..n {
            let e = (r.u32()?, r.u32()?);
            if k == 0 {
                p.sharp.insert(e);
            } else {
                p.seams.insert(e);
            }
        }
    }
    p.ensure_materials();
    Ok(p)
}

/// The asset as `.smodel` bytes.
pub fn to_bytes(a: &Asset) -> Vec<u8> {
    let json = serde_json::to_vec(a).unwrap_or_default();
    let mut b = Vec::with_capacity(json.len() + 1024);
    b.extend_from_slice(MAGIC);
    put_u32(&mut b, json.len() as u32);
    b.extend_from_slice(&json);
    for p in &a.parts {
        let m = poly_bytes(&p.mesh);
        put_u32(&mut b, m.len() as u32);
        b.extend_from_slice(&m);
        match &p.detail {
            Some(d) => {
                let m = poly_bytes(d);
                put_u32(&mut b, m.len() as u32);
                b.extend_from_slice(&m);
            }
            None => put_u32(&mut b, 0),
        }
    }
    if a.parts.iter().any(|p| !p.mesh.materials.is_empty()) {
        b.extend_from_slice(MATERIALS);
        for p in &a.parts {
            let holder = Mesh { materials: p.mesh.materials.clone(), ..Mesh::default() };
            let m = studio_mesh::io::to_bytes(&holder);
            put_u32(&mut b, m.len() as u32);
            b.extend_from_slice(&m);
        }
    }
    b
}

pub fn from_bytes(b: &[u8]) -> Result<Asset, String> {
    if b.len() < 12 || &b[..8] != MAGIC {
        return Err("not a Zornov model file (.smodel)".into());
    }
    let mut r = Reader { b, at: 8 };
    let n = r.count(b.len())?;
    let json = b.get(r.at..r.at + n).ok_or("the model file ends early")?;
    r.at += n;
    let mut a: Asset = serde_json::from_slice(json).map_err(|e| format!("the model file's description is damaged: {e}"))?;
    for p in &mut a.parts {
        let n = r.count(b.len())?;
        p.mesh = poly_from_bytes(b.get(r.at..r.at + n).ok_or("the model file ends early")?)?;
        r.at += n;
        let n = r.count(b.len())?;
        if n > 0 {
            p.detail = Some(poly_from_bytes(b.get(r.at..r.at + n).ok_or("the model file ends early")?)?);
            r.at += n;
        }
    }
    if b.get(r.at..r.at + 8) == Some(&MATERIALS[..]) {
        r.at += 8;
        for p in &mut a.parts {
            let n = r.count(b.len())?;
            let m = studio_mesh::io::from_bytes(b.get(r.at..r.at + n).ok_or("the model file ends early")?)?;
            r.at += n;
            p.mesh.materials = m.materials;
            p.mesh.ensure_materials();
        }
    }
    Ok(a)
}

pub fn save(a: &Asset, path: &std::path::Path) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    std::fs::write(path, to_bytes(a)).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn load(path: &std::path::Path) -> Result<Asset, String> {
    from_bytes(&std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?)
}

/// Faces of a part picked in words (for steps that take a pick).
pub fn faces_of(p: &Poly, f: &Faces) -> Vec<bool> {
    f.mask(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prim;

    #[test]
    fn an_asset_saves_loads_and_draws() {
        let mut a = Asset::new("Crate");
        let mut body = Part::new("Body", prim::cube([100.0; 3], 1));
        body.modifiers.push(Modifier::Bevel { width: 4.0, segments: 2, profile: 0.5, angle: 30.0, only_marked: false });
        body.materials = vec!["wood".into()];
        a.parts.push(body);
        let mut hole = Part::new("Hole", prim::cylinder(20.0, 20.0, 200.0, 24, 1, prim::Cap::Ngon));
        hole.hidden = true;
        hole.rotation = [90.0, 0.0, 0.0];
        a.parts.push(hole);
        a.parts[0].modifiers.push(Modifier::Boolean { op: "subtract".into(), with: "Hole".into() });
        a.materials.push(material::smart("wood", 100.0, None).unwrap());
        a.texture_size = 128;
        let bytes = to_bytes(&a);
        let b = from_bytes(&bytes).unwrap();
        assert_eq!(b.parts.len(), 2);
        assert_eq!(b.parts[0].mesh, a.parts[0].mesh);
        assert_eq!(b.parts[0].modifiers, a.parts[0].modifiers);
        let m = b.to_mesh(false).unwrap();
        assert!(m.triangles() > 50);
        assert!(m.materials.iter().any(|m| m.texture.is_some()), "the wood was baked");
        let quick = b.to_mesh(true).unwrap();
        assert!(quick.materials.iter().all(|m| m.texture.is_none()));
        assert!(from_bytes(&bytes[..bytes.len() / 2]).is_err());
    }

    #[test]
    fn materials_with_textures_and_switched_off_modifiers_survive() {
        let mut a = Asset::new("Box");
        let mut body = Part::new("Body", prim::cube([100.0; 3], 1));
        body.mesh.face_material[0] = 1;
        body.mesh.ensure_materials();
        body.mesh.materials[1].name = "label".into();
        body.mesh.materials[1].texture = Some(studio_mesh::Texture { width: 2, height: 1, rgba: vec![255, 0, 0, 255, 0, 255, 0, 255] });
        body.modifiers.push(Modifier::Subdivide { levels: 1 });
        body.disabled = vec![true];
        a.parts.push(body);
        let b = from_bytes(&to_bytes(&a)).unwrap();
        assert_eq!(b.parts[0].mesh.materials.len(), 2);
        assert_eq!(b.parts[0].mesh.materials[1].name, "label");
        assert!(b.parts[0].mesh.materials[1].texture.is_some());
        assert_eq!(b.parts[0].disabled, vec![true]);
        let m = b.to_mesh(true).unwrap();
        assert_eq!(m.triangles(), 12, "the subdivision is switched off");
        assert!(m.materials.iter().any(|x| x.texture.is_some()));
        let mut on = b.clone();
        on.parts[0].disabled.clear();
        assert_eq!(on.to_mesh(true).unwrap().triangles(), 48);
    }

    #[test]
    fn loops_between_parts_are_refused() {
        let mut a = Asset::new("Loop");
        let mut x = Part::new("A", prim::cube([10.0; 3], 1));
        x.modifiers.push(Modifier::Boolean { op: "union".into(), with: "B".into() });
        let mut y = Part::new("B", prim::cube([10.0; 3], 1));
        y.modifiers.push(Modifier::Boolean { op: "union".into(), with: "A".into() });
        a.parts = vec![x, y];
        assert!(a.evaluated().unwrap_err().contains("loop"));
    }
}
