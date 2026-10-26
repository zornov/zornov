// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Several models as one model with named parts, and back.
//!
//! A place made of forty separate model clips is forty layers on a
//! timeline; the same place as one model is one layer with forty named
//! parts. [`combine`] merges models into one mesh: each model becomes a
//! part, and the part's name is carried by its materials (`"Part/Brass"`,
//! or just `"Part"` when the material is called the same), so the name
//! survives every later mesh edit. Bones come along with their pivots
//! (named `"Part/hinge"`), each part's vertices still bound to its own
//! bones, and anything without a rig hangs from a root bone that is made
//! for the group. [`split`] undoes it: it finds the parts by their
//! materials and gives each back as a model with its own materials, bones
//! and pivots.

use crate::rig::{Bone, Rig};
use crate::{Mesh, V3, add, cross, dot, length, normalise, scale, sub};
use std::sync::Arc;

/// A 3x4 placement: rows of rotation/scale and the translation.
pub type Xf3 = [[f32; 4]; 3];

pub const IDENTITY: Xf3 = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];

/// One model going into a group.
pub struct PartIn<'a> {
    pub name: &'a str,
    pub mesh: &'a Mesh,
    pub xf: Xf3,
}

fn apply(m: &Xf3, p: V3) -> V3 {
    std::array::from_fn(|r| m[r][0] * p[0] + m[r][1] * p[1] + m[r][2] * p[2] + m[r][3])
}

fn apply_dir(m: &Xf3, v: V3) -> V3 {
    std::array::from_fn(|r| m[r][0] * v[0] + m[r][1] * v[1] + m[r][2] * v[2])
}

fn det(m: &Xf3) -> f32 {
    let (a, b, c) = ([m[0][0], m[1][0], m[2][0]], [m[0][1], m[1][1], m[2][1]], [m[0][2], m[1][2], m[2][2]]);
    dot(a, cross(b, c))
}

/// Normals go through the inverse transpose.
fn apply_normal(m: &Xf3, n: V3) -> V3 {
    let (c0, c1, c2) = ([m[0][0], m[1][0], m[2][0]], [m[0][1], m[1][1], m[2][1]], [m[0][2], m[1][2], m[2][2]]);
    let rows = [cross(c1, c2), cross(c2, c0), cross(c0, c1)];
    // Cofactor columns are the rows of the inverse transpose.
    let v = [rows[0][0] * n[0] + rows[1][0] * n[1] + rows[2][0] * n[2], rows[0][1] * n[0] + rows[1][1] * n[1] + rows[2][1] * n[2], rows[0][2] * n[0] + rows[1][2] * n[1] + rows[2][2] * n[2]];
    let l = length(v);
    if l < 1e-9 { n } else { scale(v, 1.0 / l) }
}

/// The part a material belongs to: the text before the first `/`, or the
/// whole name.
pub fn part_of(material: &str) -> &str {
    material.split_once('/').map_or(material, |(p, _)| p)
}

/// The material's own name inside its part.
fn own_name<'a>(material: &'a str, part: &str) -> &'a str {
    match material.split_once('/') {
        Some((_, rest)) => rest,
        None => {
            let _ = part;
            material
        }
    }
}

/// The names of a model's parts in order, with their triangle counts.
pub fn parts_of(mesh: &Mesh) -> Vec<(String, usize)> {
    let tri = mesh.tri_materials();
    let mut out: Vec<(String, usize)> = Vec::new();
    for (i, m) in mesh.materials.iter().enumerate() {
        let n = tri.iter().filter(|&&t| t as usize == i).count();
        if n == 0 {
            continue;
        }
        let p = part_of(&m.name);
        match out.iter_mut().find(|(name, _)| name == p) {
            Some(e) => e.1 += n,
            None => out.push((p.to_owned(), n)),
        }
    }
    out
}

fn unique(taken: &[String], base: &str) -> String {
    if !taken.iter().any(|t| t == base) {
        return base.to_owned();
    }
    (2..).map(|i| format!("{base} {i}")).find(|n| !taken.iter().any(|t| t == n)).unwrap()
}

/// Merges models into one with named parts. Fails if one carries a
/// simulation volume. Returns the mesh and the final part names (made
/// unique).
pub fn combine(parts: &[PartIn]) -> Result<(Mesh, Vec<String>), String> {
    if parts.is_empty() {
        return Err("nothing to combine".into());
    }
    let mut out = Mesh::default();
    let mut tri_mats: Vec<u32> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    let any_rig = parts.iter().any(|p| p.mesh.rig.is_some());
    let mut rig = Rig::default();
    let mut joints: Vec<[u16; 4]> = Vec::new();
    let mut weights: Vec<[f32; 4]> = Vec::new();
    // Bone 0 is the group's own root when anything is rigged.
    if any_rig {
        rig.bones.push(Bone::new("root", None, [0.0; 3], [0.0, -100.0, 0.0]));
    }
    for p in parts {
        if p.mesh.volume.is_some() {
            return Err(format!("\"{}\" is a smoke or fire volume, not a solid model", p.name));
        }
        let name = unique(&names, if p.name.trim().is_empty() { "Part" } else { p.name.trim() });
        let name = name.replace('/', "-");
        names.push(name.clone());
        let base = out.positions.len() as u32;
        let first_mat = out.materials.len() as u32;
        let flip = det(&p.xf) < 0.0;
        let mesh = p.mesh;
        for (i, v) in mesh.positions.iter().enumerate() {
            out.positions.push(apply(&p.xf, *v));
            out.normals.push(mesh.normals.get(i).map_or([0.0, -1.0, 0.0], |n| apply_normal(&p.xf, *n)));
            out.uvs.push(mesh.uvs.get(i).copied().unwrap_or([0.0, 0.0]));
        }
        for t in 0..mesh.triangles() {
            let [a, b, c] = mesh.tri(t);
            let (a, b, c) = (a as u32 + base, b as u32 + base, c as u32 + base);
            if flip {
                out.indices.extend_from_slice(&[a, c, b]);
            } else {
                out.indices.extend_from_slice(&[a, b, c]);
            }
        }
        for m in &mesh.materials {
            let mut m = m.clone();
            let own = own_name(&m.name, &name).to_owned();
            m.name = if own == name { name.clone() } else { format!("{name}/{own}") };
            out.materials.push(m);
        }
        if mesh.materials.is_empty() {
            out.materials.push(crate::Material { name: name.clone(), ..Default::default() });
        }
        let src = mesh.tri_materials();
        tri_mats.extend(src.iter().map(|m| first_mat + (*m).min(mesh.materials.len().saturating_sub(1) as u32)));
        if any_rig {
            let n = mesh.positions.len();
            match &mesh.rig {
                Some(r) => {
                    let mut r = (**r).clone();
                    let k = {
                        let s = (length(apply_dir(&p.xf, [1.0, 0.0, 0.0])) + length(apply_dir(&p.xf, [0.0, 1.0, 0.0])) + length(apply_dir(&p.xf, [0.0, 0.0, 1.0]))) / 3.0;
                        s
                    };
                    r.map_points(|q| apply(&p.xf, q), |d| apply_dir(&p.xf, d), k);
                    let off = rig.bones.len();
                    for b in &r.bones {
                        let mut b = b.clone();
                        b.name = format!("{name}/{}", b.name);
                        b.parent = Some(b.parent.map_or(0, |q| q + off));
                        rig.bones.push(b);
                    }
                    for v in 0..n {
                        match (r.joints.get(v), r.weights.get(v)) {
                            (Some(j), Some(w)) => {
                                joints.push(j.map(|x| x + off as u16));
                                weights.push(*w);
                            }
                            _ => {
                                joints.push([0; 4]);
                                weights.push([1.0, 0.0, 0.0, 0.0]);
                            }
                        }
                    }
                }
                None => {
                    joints.extend(std::iter::repeat_n([0u16; 4], n));
                    weights.extend(std::iter::repeat_n([1.0, 0.0, 0.0, 0.0], n));
                }
            }
        }
    }
    if any_rig {
        let (lo, hi) = {
            let mut lo = [f32::INFINITY; 3];
            let mut hi = [f32::NEG_INFINITY; 3];
            for p in &out.positions {
                for i in 0..3 {
                    lo[i] = lo[i].min(p[i]);
                    hi[i] = hi[i].max(p[i]);
                }
            }
            (lo, hi)
        };
        let mid = [(lo[0] + hi[0]) / 2.0, hi[1], (lo[2] + hi[2]) / 2.0];
        rig.bones[0].head = mid;
        rig.bones[0].tail = [mid[0], lo[1], mid[2]];
        rig.joints = joints;
        rig.weights = weights;
        out.rig = Some(Arc::new(rig));
    }
    out.regroup(&tri_mats);
    out.tidy();
    Ok((out, names))
}

/// One part taken back out.
pub struct PartOut {
    pub name: String,
    pub mesh: Mesh,
}

/// Splits a model into its parts (see [`part_of`]). Each comes back with
/// its materials (their own names again), its triangles and the bones
/// named for it, with their pivots.
pub fn split(mesh: &Mesh) -> Vec<PartOut> {
    let names: Vec<String> = parts_of(mesh).into_iter().map(|p| p.0).collect();
    names.iter().map(|n| take(mesh, n)).collect()
}

/// The part called `name`.
pub fn take(mesh: &Mesh, name: &str) -> PartOut {
    let tri = mesh.tri_materials();
    let mats: Vec<usize> = (0..mesh.materials.len()).filter(|&i| part_of(&mesh.materials[i].name) == name).collect();
    let mut remap: Vec<i64> = vec![-1; mesh.positions.len()];
    let mut out = Mesh::default();
    let mut new_mats: Vec<u32> = Vec::new();
    for t in 0..mesh.triangles() {
        let Some(slot) = mats.iter().position(|&m| m == tri[t] as usize) else { continue };
        for &v in &mesh.tri(t) {
            if remap[v] < 0 {
                remap[v] = out.positions.len() as i64;
                out.positions.push(mesh.positions[v]);
                out.normals.push(mesh.normals.get(v).copied().unwrap_or([0.0, -1.0, 0.0]));
                out.uvs.push(mesh.uvs.get(v).copied().unwrap_or([0.0, 0.0]));
            }
            out.indices.push(remap[v] as u32);
        }
        new_mats.push(slot as u32);
    }
    for &m in &mats {
        let mut mat = mesh.materials[m].clone();
        mat.name = own_name(&mat.name, name).to_owned();
        out.materials.push(mat);
    }
    if out.materials.is_empty() {
        out.materials.push(crate::Material::default());
    }
    out.regroup(&new_mats);
    out.tidy();
    // The part's bones, with the group root and other parts' bones cut away.
    if let Some(r) = &mesh.rig {
        let prefix = format!("{name}/");
        let mine: Vec<usize> = (0..r.bones.len()).filter(|&i| r.bones[i].name.starts_with(&prefix)).collect();
        if !mine.is_empty() {
            let mut new_rig = Rig::default();
            let mut of: Vec<Option<usize>> = vec![None; r.bones.len()];
            for &i in &mine {
                let mut b = r.bones[i].clone();
                b.name = b.name[prefix.len()..].to_owned();
                b.parent = b.parent.and_then(|p| of.get(p).copied().flatten());
                of[i] = Some(new_rig.bones.len());
                new_rig.bones.push(b);
            }
            let mut kept: Vec<usize> = (0..mesh.positions.len()).filter(|&v| remap[v] >= 0).collect();
            kept.sort_by_key(|&v| remap[v]);
            for v in kept {
                let (j, w) = (r.joints.get(v).copied().unwrap_or([0; 4]), r.weights.get(v).copied().unwrap_or([1.0, 0.0, 0.0, 0.0]));
                let mut nj = [0u16; 4];
                let mut nw = [0.0f32; 4];
                let mut k = 0;
                for s in 0..4 {
                    if let Some(Some(nb)) = of.get(j[s] as usize).copied().filter(|_| w[s] > 0.0) {
                        nj[k] = nb as u16;
                        nw[k] = w[s];
                        k += 1;
                    }
                }
                let sum: f32 = nw.iter().sum();
                if sum > 1e-6 {
                    for x in &mut nw {
                        *x /= sum;
                    }
                } else {
                    nj = [0; 4];
                    nw = [1.0, 0.0, 0.0, 0.0];
                }
                new_rig.joints.push(nj);
                new_rig.weights.push(nw);
            }
            out.rig = Some(Arc::new(new_rig));
        }
    }
    PartOut { name: name.to_owned(), mesh: out }
}

/// A placement that only moves.
pub fn shift(by: V3) -> Xf3 {
    [[1.0, 0.0, 0.0, by[0]], [0.0, 1.0, 0.0, by[1]], [0.0, 0.0, 1.0, by[2]]]
}

/// A placement from a rotation matrix (rows), scales on each axis and a move.
pub fn placement(rot: [V3; 3], k: V3, by: V3) -> Xf3 {
    std::array::from_fn(|r| [rot[r][0] * k[0], rot[r][1] * k[1], rot[r][2] * k[2], by[r]])
}

#[allow(dead_code)]
fn unused(_: V3) -> V3 {
    normalise(add(sub([0.0; 3], [0.0; 3]), [1.0, 0.0, 0.0]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Material, prim};

    fn coloured(name: &str, c: [f32; 3]) -> Mesh {
        let mut m = prim::cube([40.0, 40.0, 40.0]);
        m.materials = vec![Material { name: name.into(), base_colour: [c[0], c[1], c[2], 1.0], metallic: 0.3, roughness: 0.2, ..Default::default() }];
        m
    }

    fn centroids(m: &Mesh) -> Vec<[i32; 3]> {
        let mut v: Vec<[i32; 3]> = (0..m.triangles()).map(|t| m.centroid(t).map(|x| (x * 10.0).round() as i32)).collect();
        v.sort();
        v
    }

    #[test]
    fn combine_and_split_keep_geometry_and_materials() {
        let a = coloured("Brass", [0.8, 0.6, 0.1]);
        let b = coloured("Door", [0.2, 0.3, 0.9]);
        let (g, names) = combine(&[PartIn { name: "Lamp", mesh: &a, xf: IDENTITY }, PartIn { name: "Door", mesh: &b, xf: shift([100.0, 0.0, 0.0]) }]).unwrap();
        assert_eq!(names, ["Lamp", "Door"]);
        assert_eq!(g.triangles(), a.triangles() + b.triangles());
        assert_eq!(parts_of(&g).iter().map(|p| p.0.as_str()).collect::<Vec<_>>(), ["Lamp", "Door"]);
        let back = split(&g);
        assert_eq!(back.len(), 2);
        let lamp = back.iter().find(|p| p.name == "Lamp").unwrap();
        let door = back.iter().find(|p| p.name == "Door").unwrap();
        assert_eq!(centroids(&lamp.mesh), centroids(&a));
        let mut moved = b.clone();
        for p in &mut moved.positions {
            p[0] += 100.0;
        }
        assert_eq!(centroids(&door.mesh), centroids(&moved));
        assert_eq!(lamp.mesh.materials[0].name, "Brass");
        assert_eq!(door.mesh.materials[0].name, "Door");
        assert_eq!(lamp.mesh.materials[0].base_colour, a.materials[0].base_colour);
        assert_eq!(door.mesh.materials[0].metallic, 0.3);
    }

    #[test]
    fn pivots_survive_the_round_trip() {
        let mut a = coloured("Lid", [0.5; 3]);
        let mut rig = Rig::default();
        rig.bones.push(Bone::new("hinge", None, [-20.0, -20.0, 20.0], [20.0, -20.0, 20.0]));
        rig.joints = vec![[0; 4]; a.positions.len()];
        rig.weights = vec![[1.0, 0.0, 0.0, 0.0]; a.positions.len()];
        a.rig = Some(Arc::new(rig));
        let b = coloured("Base", [0.1; 3]);
        let (g, _) = combine(&[PartIn { name: "Lid", mesh: &a, xf: shift([0.0, -50.0, 0.0]) }, PartIn { name: "Base", mesh: &b, xf: IDENTITY }]).unwrap();
        let r = g.rig.as_ref().unwrap();
        assert!(r.bone("Lid/hinge").is_some() && r.bone("root").is_some());
        // The pivot moved with its part.
        assert_eq!(r.bones[r.bone("Lid/hinge").unwrap()].head, [-20.0, -70.0, 20.0]);
        let back = split(&g);
        let lid = back.iter().find(|p| p.name == "Lid").unwrap();
        let lr = lid.mesh.rig.as_ref().expect("lid keeps its bone");
        assert_eq!(lr.bones.len(), 1);
        assert_eq!(lr.bones[0].name, "hinge");
        assert_eq!(lr.joints.len(), lid.mesh.positions.len());
        assert!(back.iter().find(|p| p.name == "Base").unwrap().mesh.rig.is_none());
    }

    #[test]
    fn a_mirrored_part_keeps_its_outside_out() {
        let a = coloured("A", [0.5; 3]);
        let flipped = placement([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], [-1.0, 1.0, 1.0], [0.0; 3]);
        let (g, _) = combine(&[PartIn { name: "A", mesh: &a, xf: flipped }]).unwrap();
        // Every face still looks away from the centre.
        for t in 0..g.triangles() {
            assert!(dot(g.face_normal(t), g.centroid(t)) > 0.0, "triangle {t} points in");
        }
    }
}
