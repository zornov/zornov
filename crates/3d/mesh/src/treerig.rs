// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Rigging a grown tree by branch: the bark is built one tube per branch
//! with every vertex tagged with its branch ([`bark`]), and the skin binds
//! by that tag ([`rig`]), so a fork never has to be welded or guessed from
//! distance. Branches down to `depth` forks from the trunk get bones (a long
//! one several, chained); finer twigs ride whole on the bone of the limb
//! they grow from, at the point where they fork.

use crate::autorig::closest_on_segment;
use crate::math::growth::Grown;
use crate::math::surface;
use crate::rig::{Bone, Rig};
use crate::{Mesh, V3, dot, length, sub};

/// Tag of a vertex that is not on any branch (leaves, say): it follows the
/// nearest bone.
pub const NO_BRANCH: u32 = u32::MAX;

/// Bark for every branch of `g` (`radii` per branch, per point, as
/// [`crate::math::growth::pipe_radii`] gives; `sides` the tube's sides from
/// the branch's base radius), placed by `place`. Returns the mesh and, per
/// vertex, the index of its branch.
pub fn bark(g: &Grown, radii: &[Vec<f64>], sides: &dyn Fn(f64) -> usize, place: &dyn Fn([f64; 3]) -> V3) -> (Mesh, Vec<u32>) {
    let mut mesh = Mesh::default();
    let mut ids = Vec::new();
    for (i, (b, r)) in g.branches.iter().zip(radii).enumerate() {
        let sheet = surface::tube(&b.points, r, sides(r.first().copied().unwrap_or(0.0)).max(3));
        let built = surface::build(&sheet, place, 0.0, None);
        crate::ops::merge(&mut mesh, &built.mesh);
        ids.resize(mesh.positions.len(), i as u32);
    }
    (mesh, ids)
}

/// Bones for the branches of `g` down to `depth` forks and the skin of
/// `mesh` bound by `ids` (a branch index per vertex; short or [`NO_BRANCH`]
/// entries go to the nearest bone). Bones are named `branch.<b>.<segment>`;
/// the parent of a branch's first bone is the bone at its fork.
pub fn rig(mesh: &Mesh, ids: &[u32], g: &Grown, place: &dyn Fn([f64; 3]) -> V3, depth: usize) -> Result<Rig, String> {
    let n = g.branches.len();
    if n == 0 {
        return Err("branches: the tree has no branches".into());
    }
    // Forks from the trunk, and the limb a twig belongs to (its ancestor at `depth`)
    // with the point index where its chain leaves that limb.
    let parent_of = |b: usize| g.branches[b].parent.filter(|&(p, _)| p < b);
    let mut level = vec![0usize; n];
    let mut limb = vec![(0usize, 0usize); n];
    for b in 0..n {
        match parent_of(b) {
            None => {
                level[b] = 0;
                limb[b] = (b, 0);
            }
            Some((p, k)) => {
                level[b] = level[p] + 1;
                limb[b] = if level[p] >= depth { limb[p] } else { (p, k) };
                if level[b] <= depth {
                    limb[b] = (b, 0);
                }
            }
        }
    }
    let mut r = Rig::default();
    // Per limb: its bones' first index, how many, and its placed points.
    let mut first = vec![usize::MAX; n];
    let mut segs = vec![0usize; n];
    let mut pts: Vec<Vec<V3>> = vec![Vec::new(); n];
    for b in 0..n {
        pts[b] = g.branches[b].points.iter().map(|p| place(*p)).collect();
        if level[b] > depth {
            continue;
        }
        let steps = pts[b].len() - 1;
        let count = steps.clamp(1, 4);
        segs[b] = count;
        first[b] = r.bones.len();
        let at = |s: usize| pts[b][(s * steps / count).min(steps)];
        let mut prev = parent_of(b).map(|(p, k)| bone_at(&first, &segs, p, k, g.branches[p].points.len()));
        for s in 0..count {
            let name = format!("branch.{b}.{s}");
            let bone = Bone::new(name, prev, at(s), at(s + 1));
            r.bones.push(bone);
            prev = Some(r.bones.len() - 1);
        }
    }
    // The skin: whole to a bone, blended across the joint between a long limb's segments.
    let all: Vec<usize> = (0..r.bones.len()).collect();
    let nearest = |p: V3| -> usize { all.iter().copied().min_by(|&a, &b| dist(&r.bones[a], p).total_cmp(&dist(&r.bones[b], p))).unwrap_or(0) };
    for (v, p) in mesh.positions.iter().enumerate() {
        let id = ids.get(v).copied().unwrap_or(NO_BRANCH) as usize;
        if id >= n {
            r.joints.push([nearest(*p) as u16, 0, 0, 0]);
            r.weights.push([1.0, 0.0, 0.0, 0.0]);
            continue;
        }
        let (l, k) = limb[id];
        if l != id {
            // A twig: whole on the bone at its fork.
            let bone = bone_at(&first, &segs, l, k, g.branches[l].points.len());
            r.joints.push([bone as u16, 0, 0, 0]);
            r.weights.push([1.0, 0.0, 0.0, 0.0]);
            continue;
        }
        // Where along the limb's polyline the vertex sits, in segments.
        let (mut best, mut at) = (f32::INFINITY, 0.0f32);
        for (i, w) in pts[l].windows(2).enumerate() {
            let c = closest_on_segment(w[0], w[1], *p);
            let d = length(sub(c, *p));
            if d < best {
                let ab = sub(w[1], w[0]);
                let t = (dot(sub(c, w[0]), ab) / dot(ab, ab).max(1e-12)).clamp(0.0, 1.0);
                best = d;
                at = i as f32 + t;
            }
        }
        let steps = (pts[l].len() - 1) as f32;
        let f = at / steps * segs[l] as f32;
        let seg = (f.floor() as usize).min(segs[l] - 1);
        let (mut j, mut w) = ([(first[l] + seg) as u16, 0, 0, 0], [1.0f32, 0.0, 0.0, 0.0]);
        // Within a quarter of a segment of a joint, share it with the neighbour.
        let edge = f.round();
        let d = f - edge;
        if d.abs() < 0.25 && edge >= 1.0 && (edge as usize) < segs[l] {
            let other = if d < 0.0 { edge as usize } else { edge as usize - 1 };
            let share = 0.5 - d.abs() * 2.0;
            j[1] = (first[l] + other) as u16;
            if other != seg {
                w = [1.0 - share, share, 0.0, 0.0];
            }
        }
        r.joints.push(j);
        r.weights.push(w);
    }
    Ok(r)
}

/// The bone of limb `b` that carries its point `k` of `points`.
fn bone_at(first: &[usize], segs: &[usize], b: usize, k: usize, points: usize) -> usize {
    let steps = (points - 1).max(1);
    let s = (k * segs[b] / steps).min(segs[b] - 1);
    first[b] + s
}

fn dist(b: &Bone, p: V3) -> f32 {
    length(sub(closest_on_segment(b.head, b.tail, p), p))
}

/// [`rig`], stored on the mesh.
pub fn rig_mesh(mesh: &mut Mesh, ids: &[u32], g: &Grown, place: &dyn Fn([f64; 3]) -> V3, depth: usize) -> Result<(), String> {
    let r = rig(mesh, ids, g, place, depth)?;
    mesh.rig = Some(std::sync::Arc::new(r));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::growth::{Turtle, pipe_radii, turtle};
    use crate::rig::PoseInput;
    use std::collections::HashMap;

    fn tree() -> (Grown, Mesh, Vec<u32>) {
        let g = turtle("FFFF[+FFF[+FF]-FF][-FFF]FF", &Turtle { angle: 35.0, step: 10.0, width: 2.0, ..Turtle::default() });
        let radii = pipe_radii(&g, 2.0, 2.0);
        let (m, ids) = bark(&g, &radii, &|_| 6, &|p| [p[0] as f32, -(p[2] as f32), p[1] as f32]);
        (g, m, ids)
    }

    #[test]
    fn every_vertex_knows_its_branch_and_binds_whole_by_it() {
        let (g, m, ids) = tree();
        assert_eq!(ids.len(), m.positions.len());
        assert!(g.branches.len() >= 4);
        for b in 0..g.branches.len() as u32 {
            assert!(ids.contains(&b), "branch {b} has vertices");
        }
        let r = rig(&m, &ids, &g, &|p| [p[0] as f32, -(p[2] as f32), p[1] as f32], 1).unwrap();
        assert!(r.bones.len() >= 3);
        assert_eq!(r.joints.len(), m.positions.len());
        assert!(r.weights.iter().all(|w| (w.iter().sum::<f32>() - 1.0).abs() < 1e-4));
        // Parents come before children.
        assert!(r.bones.iter().enumerate().all(|(i, b)| b.parent.is_none_or(|p| p < i)));
    }

    #[test]
    fn turning_a_branch_moves_it_and_leaves_its_parent_alone() {
        let (g, mut m, ids) = tree();
        let place = |p: [f64; 3]| [p[0] as f32, -(p[2] as f32), p[1] as f32];
        rig_mesh(&mut m, &ids, &g, &place, 3).unwrap();
        let r = m.rig.clone().unwrap();
        // The trunk is branch 0; find a child branch and turn its first bone.
        let child = (1..g.branches.len()).find(|&b| g.branches[b].parent.is_some_and(|(p, _)| p == 0)).unwrap();
        let bone = format!("branch.{child}.0");
        let posed = r.posed_mesh(&m, &PoseInput { rotations: HashMap::from([(bone, [60.0, 0.0, 0.0])]), ..Default::default() });
        let (mut moved_child, mut moved_trunk) = (0.0f32, 0.0f32);
        for v in 0..m.positions.len() {
            let d = length(sub(posed.positions[v], m.positions[v]));
            if ids[v] as usize == child {
                moved_child = moved_child.max(d);
            } else if ids[v] == 0 {
                moved_trunk = moved_trunk.max(d);
            }
        }
        assert!(moved_child > 5.0, "the branch swung: {moved_child}");
        assert!(moved_trunk < 1e-3, "the trunk did not: {moved_trunk}");
    }
}
