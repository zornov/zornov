// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Keeping a rig whole while the surface under it changes.
//!
//! A rigged model is a surface plus, for every vertex, the bones that carry
//! it. Any edit that adds, removes or reorders vertices (a part added, a
//! boolean, a subdivision, a deletion) leaves those weights out of step
//! with the surface. This module puts them back so a rig is never
//! "damaged" by an edit:
//!
//! * [`Mesh::sync_rig`] after an edit: vertices that were there keep their
//!   weights; new geometry is skinned by one of the [`Bind`] modes (copy the
//!   smooth weights of the body under it, take the whole piece rigid to the
//!   bone that carries most of what is under it, or one named bone); a
//!   surface that was rebuilt gets the old weights carried over from the
//!   nearest old vertices.
//! * [`Mesh::repair_rig`] for a model already saved out of step: the same,
//!   guessing that the new vertices were added at the end.
//! * [`bind_to_bone`], [`bind_pieces`], [`rebind_smooth`]: skinning modes for
//!   chosen vertices (hard accessories, props, a region re-skinned).
//!
//! Bone names are matched forgivingly (see [`name_key`]).

use crate::rig::{PointGrid, Rig};
use crate::Mesh;

/// A name with case, separators and left/right spelling taken out, so
/// `upper_arm.L`, `UpperArm.l`, `left_upperarm` and `upper-arm_left` agree.
pub fn name_key(name: &str) -> String {
    let raw = name.rsplit([':', '|']).next().unwrap_or(name);
    let lower = raw.to_ascii_lowercase();
    let mut stem = String::new();
    let mut side = '-';
    for tok in lower.split(['.', '_', '-', ' ']) {
        match tok {
            "" => {}
            "l" | "left" => side = 'l',
            "r" | "right" => side = 'r',
            t if t.len() > 4 && t.starts_with("left") => {
                side = 'l';
                stem.push_str(&t[4..]);
            }
            t if t.len() > 5 && t.starts_with("right") => {
                side = 'r';
                stem.push_str(&t[5..]);
            }
            t => stem.push_str(t),
        }
    }
    format!("{stem}|{side}")
}

/// Edit distance between two short strings.
pub fn distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut prev = row[0];
        row[0] = i;
        for j in 1..=b.len() {
            let cur = row[j];
            row[j] = (prev + (a[i - 1] != b[j - 1]) as usize).min(row[j] + 1).min(row[j - 1] + 1);
            prev = cur;
        }
    }
    row[b.len()]
}

/// How new geometry is skinned.
#[derive(Clone, Debug, PartialEq)]
pub enum Bind {
    /// Each vertex takes the weights of the nearest body vertex: cloth and
    /// anything that should bend with the body.
    Smooth,
    /// Each separate new piece follows, whole and rigid, the one bone that
    /// carries most of the body under it: hats, glasses, props, armour.
    Piece,
    /// Every new vertex follows this bone, rigid.
    Bone(usize),
}

impl Bind {
    /// `smooth`, `piece` (also `rigid`, `per_part`, `parts`), or `bone`
    /// (given the resolved bone).
    pub fn parse(word: &str, bone: Option<usize>) -> Result<Bind, String> {
        Ok(match word.trim().to_ascii_lowercase().replace([' ', '-'], "_").as_str() {
            "smooth" | "auto" | "body" | "weights" | "soft" => Bind::Smooth,
            "piece" | "pieces" | "rigid" | "per_part" | "parts" | "part" | "per_piece" => Bind::Piece,
            "bone" | "by_bone" | "to_bone" => Bind::Bone(bone.ok_or("bind \"bone\" needs \"bone\": the bone name (e.g. \"head\")")?),
            other => return Err(format!("bind \"{other}\": smooth (copy the body's weights), piece (each separate piece rigid on the bone that carries the most under it) or bone (rigid on the bone you name, with \"bone\")")),
        })
    }
}

/// Connected pieces among triangles whose corners are all `>= from`:
/// the piece number of each vertex (`usize::MAX` for vertices below `from`).
fn pieces_from(mesh: &Mesh, from: usize) -> (Vec<usize>, usize) {
    let (ids, unique) = mesh.position_ids();
    let mut parent: Vec<u32> = (0..unique.len() as u32).collect();
    fn root(p: &mut [u32], mut i: u32) -> u32 {
        while p[i as usize] != i {
            p[i as usize] = p[p[i as usize] as usize];
            i = p[i as usize];
        }
        i
    }
    for t in 0..mesh.triangles() {
        let c = mesh.tri(t);
        if c.iter().any(|&v| v < from) {
            continue;
        }
        let (a, b, d) = (ids[c[0]], ids[c[1]], ids[c[2]]);
        for (x, y) in [(a, b), (b, d)] {
            let (rx, ry) = (root(&mut parent, x), root(&mut parent, y));
            if rx != ry {
                parent[rx as usize] = ry;
            }
        }
    }
    let mut number: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
    let mut out = vec![usize::MAX; mesh.positions.len()];
    for v in from..mesh.positions.len() {
        let r = root(&mut parent, ids[v]);
        let k = number.len();
        out[v] = *number.entry(r).or_insert(k);
    }
    (out, number.len())
}

/// Skins the vertices `from..` of `mesh`, which have no weights yet, from
/// the ones before them. Vertices below `from` must already be skinned.
pub fn skin_new(mesh: &Mesh, rig: &mut Rig, from: usize, how: &Bind) {
    let n = mesh.positions.len();
    rig.joints.truncate(from);
    rig.weights.truncate(from);
    if from == 0 || n <= from {
        rig.joints.resize(n, [0; 4]);
        rig.weights.resize(n, [1.0, 0.0, 0.0, 0.0]);
        return;
    }
    let grid = PointGrid::new(&mesh.positions[..from]);
    let nearest: Vec<usize> = (from..n).map(|v| grid.nearest(&mesh.positions[..from], mesh.positions[v])).collect();
    match how {
        Bind::Smooth => {
            for &o in &nearest {
                rig.joints.push(rig.joints[o]);
                rig.weights.push(rig.weights[o]);
            }
        }
        Bind::Bone(b) => {
            for _ in from..n {
                rig.joints.push([*b as u16, 0, 0, 0]);
                rig.weights.push([1.0, 0.0, 0.0, 0.0]);
            }
        }
        Bind::Piece => {
            let (piece, count) = pieces_from(mesh, from);
            // Per piece, the weight each bone carries under its vertices.
            let mut sums: Vec<std::collections::HashMap<u16, f32>> = vec![Default::default(); count];
            for (k, &o) in nearest.iter().enumerate() {
                let p = piece[from + k];
                for s in 0..4 {
                    if rig.weights[o][s] > 0.0 {
                        *sums[p].entry(rig.joints[o][s]).or_default() += rig.weights[o][s];
                    }
                }
            }
            let winner: Vec<u16> = sums.iter().map(|m| m.iter().max_by(|a, b| a.1.total_cmp(b.1).then(b.0.cmp(a.0))).map_or(0, |(b, _)| *b)).collect();
            for v in from..n {
                rig.joints.push([winner[piece[v]], 0, 0, 0]);
                rig.weights.push([1.0, 0.0, 0.0, 0.0]);
            }
        }
    }
}

/// Binds these vertices whole to one bone.
pub fn bind_to_bone(rig: &mut Rig, vertices: &[usize], bone: usize) {
    for &v in vertices {
        if v < rig.joints.len() {
            rig.joints[v] = [bone as u16, 0, 0, 0];
            rig.weights[v] = [1.0, 0.0, 0.0, 0.0];
        }
    }
}

/// Binds every connected piece (among the triangles with all corners in
/// `mask`) whole to the bone that carries most of it now. Returns the
/// pieces' bones, for the report.
pub fn bind_pieces(mesh: &Mesh, rig: &mut Rig, mask: &[bool]) -> Vec<usize> {
    let (ids, unique) = mesh.position_ids();
    let mut parent: Vec<u32> = (0..unique.len() as u32).collect();
    fn root(p: &mut [u32], mut i: u32) -> u32 {
        while p[i as usize] != i {
            p[i as usize] = p[p[i as usize] as usize];
            i = p[i as usize];
        }
        i
    }
    for t in 0..mesh.triangles() {
        let c = mesh.tri(t);
        if !c.iter().all(|&v| mask.get(v).copied().unwrap_or(false)) {
            continue;
        }
        let (a, b, d) = (ids[c[0]], ids[c[1]], ids[c[2]]);
        for (x, y) in [(a, b), (b, d)] {
            let (rx, ry) = (root(&mut parent, x), root(&mut parent, y));
            if rx != ry {
                parent[rx as usize] = ry;
            }
        }
    }
    let mut sums: std::collections::HashMap<u32, std::collections::HashMap<u16, f32>> = Default::default();
    for v in 0..mesh.positions.len() {
        if !mask.get(v).copied().unwrap_or(false) || v >= rig.joints.len() {
            continue;
        }
        let r = root(&mut parent, ids[v]);
        let e = sums.entry(r).or_default();
        for s in 0..4 {
            if rig.weights[v][s] > 0.0 {
                *e.entry(rig.joints[v][s]).or_default() += rig.weights[v][s];
            }
        }
    }
    let winner: std::collections::HashMap<u32, u16> = sums.iter().map(|(r, m)| (*r, m.iter().max_by(|a, b| a.1.total_cmp(b.1).then(b.0.cmp(a.0))).map_or(0, |(b, _)| *b))).collect();
    let mut used = Vec::new();
    for v in 0..mesh.positions.len() {
        if mask.get(v).copied().unwrap_or(false) && v < rig.joints.len() {
            let b = winner[&root(&mut parent, ids[v])];
            rig.joints[v] = [b, 0, 0, 0];
            rig.weights[v] = [1.0, 0.0, 0.0, 0.0];
            if !used.contains(&(b as usize)) {
                used.push(b as usize);
            }
        }
    }
    used
}

/// Skins the chosen vertices afresh by the automatic smooth bind (as for a
/// whole model), leaving the others alone.
pub fn rebind_smooth(mesh: &Mesh, rig: &mut Rig, mask: &[bool]) -> usize {
    // The sub-surface of the chosen vertices, bound on its own against the
    // rig's bones.
    let mut remap = vec![u32::MAX; mesh.positions.len()];
    let mut sub = Mesh { materials: mesh.materials.clone(), ..Default::default() };
    let mut back = Vec::new();
    for t in 0..mesh.triangles() {
        let c = mesh.tri(t);
        if !c.iter().all(|&v| mask.get(v).copied().unwrap_or(false)) {
            continue;
        }
        for &v in &c {
            if remap[v] == u32::MAX {
                remap[v] = sub.positions.len() as u32;
                sub.positions.push(mesh.positions[v]);
                sub.normals.push(mesh.normals.get(v).copied().unwrap_or([0.0; 3]));
                back.push(v);
            }
            sub.indices.push(remap[v]);
        }
    }
    if sub.positions.is_empty() {
        return 0;
    }
    let mut trial = Rig { bones: rig.bones.clone(), ..Default::default() };
    crate::autorig::bind(&sub, &mut trial);
    if rig.joints.len() < mesh.positions.len() {
        rig.joints.resize(mesh.positions.len(), [0; 4]);
        rig.weights.resize(mesh.positions.len(), [1.0, 0.0, 0.0, 0.0]);
    }
    for (k, &v) in back.iter().enumerate() {
        rig.joints[v] = trial.joints[k];
        rig.weights[v] = trial.weights[k];
    }
    back.len()
}

impl Mesh {
    /// Brings the rig back in step after an edit that changed the surface,
    /// given the model as it was before. Returns what it did, when it did
    /// anything.
    pub fn sync_rig(&mut self, before: &Mesh, how: &Bind) -> Option<String> {
        let n = self.positions.len();
        let old = before.rig.as_ref()?;
        let have = match &self.rig {
            None => None,
            Some(r) => Some(r.clone()),
        };
        let Some(rig) = have else {
            // The surface was rebuilt (a subdivision, a boolean): the old
            // weights carried to the new vertices from the nearest old ones.
            let carried = old.transferred(before, self);
            self.rig = Some(std::sync::Arc::new(carried));
            return Some(format!("rig carried over to the new surface ({n} vertices)"));
        };
        if !rig.is_skinned() {
            return None;
        }
        if rig.joints.len() == n && rig.weights.len() == n {
            return None;
        }
        let k = rig.joints.len().min(before.positions.len());
        // New vertices added after the old ones: keep the old, skin the new.
        let appended = n > k && k > 0 && self.positions[..k] == before.positions[..k] && rig.joints.len() == k;
        if appended {
            let mut r = (*rig).clone();
            skin_new(self, &mut r, k, how);
            let words = match how {
                Bind::Smooth => "with the body's smooth weights".to_string(),
                Bind::Piece => "each piece rigid on the bone that carries most of what is under it".to_string(),
                Bind::Bone(b) => format!("rigid on bone {}", r.bones.get(*b).map_or("?", |x| x.name.as_str())),
            };
            self.rig = Some(std::sync::Arc::new(r));
            return Some(format!("{} new vertices skinned {words}", n - k));
        }
        let carried = old.transferred(before, self);
        self.rig = Some(std::sync::Arc::new(carried));
        Some(format!("rig carried over to the changed surface ({n} vertices)"))
    }

    /// Puts a rig whose skin is out of step with the surface right: the
    /// vertices it has weights for keep them (when the surface starts with
    /// the same vertices, as after an added part), the rest are skinned by
    /// piece; weights that cannot belong to this surface are dropped and the
    /// bones, shape keys and motion kept.
    pub fn repair_rig(&mut self) -> Option<String> {
        let n = self.positions.len();
        let rig = self.rig.as_ref()?;
        if !rig.is_skinned() || (rig.joints.len() == n && rig.weights.len() == n) {
            return None;
        }
        let have = rig.joints.len().min(rig.weights.len());
        let mut r = (**rig).clone();
        if have > 0 && have < n {
            r.joints.truncate(have);
            r.weights.truncate(have);
            skin_new(self, &mut r, have, &Bind::Piece);
            self.rig = Some(std::sync::Arc::new(r));
            return Some(format!("rig repaired: {} vertices had no skin weights and were skinned by piece", n - have));
        }
        r.joints.clear();
        r.weights.clear();
        self.rig = Some(std::sync::Arc::new(r));
        Some("rig repaired: the skin weights did not match the surface and were dropped; bind again with rig {\"skinning\": \"smooth\"}".into())
    }

    /// Whether the rig's weights cover exactly the vertices.
    pub fn rig_in_step(&self) -> bool {
        match &self.rig {
            Some(r) if r.is_skinned() => r.joints.len() == self.positions.len() && r.weights.len() == self.positions.len(),
            _ => true,
        }
    }
}

/// The bone carrying a vertex most.
pub fn main_bone(rig: &Rig, v: usize) -> Option<usize> {
    let (j, w) = (rig.joints.get(v)?, rig.weights.get(v)?);
    (0..4).max_by(|&a, &b| w[a].total_cmp(&w[b])).filter(|&k| w[k] > 0.0).map(|k| j[k] as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rig::{Bone, PoseInput};
    use std::collections::HashMap;

    fn leg() -> Mesh {
        let mut m = crate::prim::cylinder(30.0, 30.0, 200.0, 24);
        crate::ops::subdivide(&mut m, 1);
        let mut rig = crate::autorig::chain(&m, 2, "top", "seg");
        crate::autorig::bind(&m, &mut rig);
        m.rig = Some(std::sync::Arc::new(rig));
        m
    }

    #[test]
    fn names_match_in_any_case_and_spelling() {
        assert_eq!(name_key("upper_arm.L"), name_key("UpperArm.l"));
        assert_eq!(name_key("upper_arm.L"), name_key("left_upperarm"));
        assert_ne!(name_key("upper_arm.L"), name_key("upper_arm.R"));
        let m = crate::character::character(300.0, &crate::character::Build { detail: 1, ..Default::default() });
        let rig = m.rig.as_ref().unwrap();
        for n in ["upper_arm.l", "UPPER_ARM.L", "UpperArm.L", "left_upperarm", "mixamorig:LeftArm", "L_arm", "thigh_r", "calf.R", "Head", "eye.l"] {
            assert!(rig.bone(n).is_some(), "{n}");
        }
        assert_eq!(rig.bone("upper_arm.l"), rig.bone("upper_arm.L"));
        assert_ne!(rig.bone("upper_arm.l"), rig.bone("upper_arm.R"));
        let err = rig.resolve_bone("tentacle").unwrap_err();
        assert!(err.contains("all bones:") && err.contains("upper_arm.L") && err.contains("closest"), "{err}");
    }

    #[test]
    fn an_added_part_is_skinned_not_damaged() {
        let before = leg();
        let mut m = before.clone();
        // A hat-like cap on the top, merged in: the rig is now out of step.
        let mut cap = crate::prim::cube([80.0, 20.0, 80.0]);
        crate::ops::transform(&mut cap, &crate::region::Region::All, [0.0, -110.0, 0.0], [0.0; 3], [1.0; 3], [0.0; 3]);
        crate::ops::merge(&mut m, &cap);
        assert!(!m.rig_in_step());
        let said = m.sync_rig(&before, &Bind::Piece).unwrap();
        assert!(said.contains("skinned"), "{said}");
        assert!(m.rig_in_step());
        let rig = m.rig.clone().unwrap();
        // Every vertex of the cap follows the same bone, whole.
        let k = before.positions.len();
        assert!((k..m.positions.len()).all(|v| rig.joints[v][0] == rig.joints[k][0] && rig.weights[v][0] == 1.0));
        // The old vertices kept their weights.
        let old = before.rig.clone().unwrap();
        assert_eq!(&rig.joints[..k], &old.joints[..]);
        // And a saved model comes back whole.
        let bytes = crate::io::to_bytes(&m);
        let back = crate::io::from_bytes(&bytes).unwrap();
        assert!(back.rig_in_step());
        // Bending moves the cap with its bone.
        let posed = rig.posed_mesh(&m, &PoseInput { rotations: HashMap::from([("seg.0".to_string(), [0.0, 0.0, 40.0])]), ..Default::default() });
        let a = crate::sub(posed.positions[k], posed.positions[k + 1]);
        let b = crate::sub(m.positions[k], m.positions[k + 1]);
        assert!((crate::length(a) - crate::length(b)).abs() < 0.01, "rigid: no stretch");
    }

    #[test]
    fn a_damaged_model_on_disk_is_repaired_on_load() {
        let before = leg();
        let mut m = before.clone();
        let cap = crate::prim::cube([40.0, 10.0, 40.0]);
        crate::ops::merge(&mut m, &cap);
        // Written out of step, as an edit once left it.
        let mut bytes = crate::io::to_bytes(&before);
        let _ = &mut bytes;
        let mut damaged = m.clone();
        damaged.rig = before.rig.clone();
        let raw = crate::io::to_bytes_unrepaired(&damaged);
        let back = crate::io::from_bytes(&raw).expect("loads");
        assert!(back.rig_in_step(), "repaired on load");
    }

    #[test]
    fn a_rebuilt_surface_keeps_the_rig() {
        let before = leg();
        let mut m = before.clone();
        crate::ops::subdivide(&mut m, 1);
        m.rig = None;
        let said = m.sync_rig(&before, &Bind::Smooth).unwrap();
        assert!(said.contains("carried"), "{said}");
        assert!(m.rig_in_step() && m.rig.as_ref().unwrap().is_skinned());
        let _ = Bone::new("x", None, [0.0; 3], [0.0; 3]);
    }
}
