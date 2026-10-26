// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Soft bodies: jelly, rubber balls, squishy characters, a cushion sat on.
//!
//! Shape matching (Müller et al., "Meshless deformations based on shape
//! matching", 2005): points move freely under gravity and collisions, then
//! each is drawn back towards where the best rigid fit of the rest shape
//! puts it (the rotation found by polar decomposition), blended with a
//! stretchy linear fit for squash and stretch. Overlapping clusters let a
//! long body bend; one cluster keeps a ball a ball.

use crate::{Collider, V3, add, dot, length, scale, sub};
use studio_mesh::Mesh;

/// How a soft body behaves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Jelly {
    /// 0 a puddle – 1 nearly rigid: how fast it springs back each step.
    pub stiffness: f32,
    /// 0 keeps its volume, 1 squashes and stretches freely.
    pub squash: f32,
    /// 0 wobbles for ever – 1 settles at once.
    pub damping: f32,
    pub bounce: f32,
    pub friction: f32,
    /// Clusters along the body (1 = one piece; more = it bends).
    pub clusters: usize,
}

impl Default for Jelly {
    fn default() -> Jelly {
        Jelly { stiffness: 0.3, squash: 0.3, damping: 0.05, bounce: 0.2, friction: 0.5, clusters: 1 }
    }
}

struct Cluster {
    members: Vec<u32>,
    /// Rest positions relative to the rest centre.
    rest: Vec<V3>,
    /// (Σ q qᵀ)⁻¹ for the linear fit.
    aqq_inv: [V3; 3],
}

/// A soft body: welded points of a mesh.
pub struct Soft {
    pub pos: Vec<V3>,
    pub vel: Vec<V3>,
    ids: Vec<u32>,
    clusters: Vec<Cluster>,
    pub jelly: Jelly,
    radius: f32,
}

fn mat_mul(a: &[V3; 3], b: &[V3; 3]) -> [V3; 3] {
    std::array::from_fn(|r| std::array::from_fn(|c| a[r][0] * b[0][c] + a[r][1] * b[1][c] + a[r][2] * b[2][c]))
}

fn transpose(a: &[V3; 3]) -> [V3; 3] {
    std::array::from_fn(|r| std::array::from_fn(|c| a[c][r]))
}

fn mat_vec(m: &[V3; 3], v: V3) -> V3 {
    [dot(m[0], v), dot(m[1], v), dot(m[2], v)]
}

fn invert3(m: &[V3; 3]) -> [V3; 3] {
    let c0 = crate::cross(m[1], m[2]);
    let det = dot(m[0], c0);
    if det.abs() < 1e-20 {
        return [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    }
    let c1 = crate::cross(m[2], m[0]);
    let c2 = crate::cross(m[0], m[1]);
    let k = 1.0 / det;
    [[c0[0] * k, c1[0] * k, c2[0] * k], [c0[1] * k, c1[1] * k, c2[1] * k], [c0[2] * k, c1[2] * k, c2[2] * k]]
}

fn det3(m: &[V3; 3]) -> f32 {
    dot(m[0], crate::cross(m[1], m[2]))
}

/// The rotation in A's polar decomposition (iterated averaging with the
/// inverse transpose, which converges quickly for these matrices).
fn polar_rotation(a: &[V3; 3]) -> [V3; 3] {
    let mut r = *a;
    for _ in 0..20 {
        let it = transpose(&invert3(&r));
        let next: [V3; 3] = std::array::from_fn(|i| scale(add(r[i], it[i]), 0.5));
        let diff: f32 = (0..3).map(|i| length(sub(next[i], r[i]))).sum();
        r = next;
        if diff < 1e-6 {
            break;
        }
    }
    if det3(&r) < 0.0 {
        r = [scale(r[0], -1.0), scale(r[1], -1.0), scale(r[2], -1.0)];
    }
    r
}

impl Soft {
    pub fn new(mesh: &Mesh, jelly: Jelly) -> Soft {
        let (ids, unique) = mesh.position_ids();
        let (lo, hi) = mesh.bounds();
        let big = (0..3).map(|i| hi[i] - lo[i]).fold(1.0f32, f32::max);
        let axis = (0..3).max_by(|&a, &b| (hi[a] - lo[a]).total_cmp(&(hi[b] - lo[b]))).unwrap_or(0);
        let k = jelly.clusters.clamp(1, 16);
        let mut clusters = Vec::new();
        for c in 0..k {
            // Overlapping slabs along the longest side.
            let (a, b) = if k == 1 { (f32::NEG_INFINITY, f32::INFINITY) } else {
                let span = (hi[axis] - lo[axis]) / k as f32;
                (lo[axis] + span * (c as f32 - 0.6), lo[axis] + span * (c as f32 + 1.6))
            };
            let members: Vec<u32> = (0..unique.len() as u32).filter(|&i| (a..=b).contains(&unique[i as usize][axis])).collect();
            if members.len() < 4 {
                continue;
            }
            let centre = scale(members.iter().fold([0.0; 3], |s, &i| add(s, unique[i as usize])), 1.0 / members.len() as f32);
            let rest: Vec<V3> = members.iter().map(|&i| sub(unique[i as usize], centre)).collect();
            let mut aqq = [[0.0f32; 3]; 3];
            for q in &rest {
                for r in 0..3 {
                    for c2 in 0..3 {
                        aqq[r][c2] += q[r] * q[c2];
                    }
                }
            }
            clusters.push(Cluster { members, rest, aqq_inv: invert3(&aqq) });
        }
        let n = unique.len();
        Soft { pos: unique, vel: vec![[0.0; 3]; n], ids, clusters, jelly, radius: big * 0.01 }
    }

    pub fn vertices(&self) -> Vec<V3> {
        self.ids.iter().map(|&i| self.pos[i as usize]).collect()
    }

    /// Gives every point a velocity (a throw, a drop with a push).
    pub fn push(&mut self, v: V3) {
        for x in &mut self.vel {
            *x = add(*x, v);
        }
    }

    pub fn step(&mut self, dt: f32, substeps: usize, gravity: V3, colliders: &[Collider]) {
        let n = substeps.max(1);
        let h = dt / n as f32;
        let j = self.jelly;
        for _ in 0..n {
            let prev = self.pos.clone();
            for i in 0..self.pos.len() {
                self.vel[i] = scale(add(self.vel[i], scale(gravity, h)), 1.0 - j.damping.clamp(0.0, 1.0) * 0.1);
                self.pos[i] = add(self.pos[i], scale(self.vel[i], h));
            }
            // Back towards the fitted rest shape, cluster by cluster.
            let mut goal_sum = vec![[0.0f32; 3]; self.pos.len()];
            let mut goal_n = vec![0.0f32; self.pos.len()];
            for c in &self.clusters {
                let centre = scale(c.members.iter().fold([0.0; 3], |s, &i| add(s, self.pos[i as usize])), 1.0 / c.members.len() as f32);
                let mut apq = [[0.0f32; 3]; 3];
                for (k, &i) in c.members.iter().enumerate() {
                    let p = sub(self.pos[i as usize], centre);
                    let q = c.rest[k];
                    for r in 0..3 {
                        for col in 0..3 {
                            apq[r][col] += p[r] * q[col];
                        }
                    }
                }
                let rot = polar_rotation(&apq);
                // Squash and stretch: the linear fit, volume kept.
                let mut lin = mat_mul(&apq, &c.aqq_inv);
                let d = det3(&lin);
                if d.abs() > 1e-9 {
                    let k = d.abs().cbrt();
                    lin = [scale(lin[0], 1.0 / k), scale(lin[1], 1.0 / k), scale(lin[2], 1.0 / k)];
                } else {
                    lin = rot;
                }
                let s = j.squash.clamp(0.0, 1.0);
                let m: [V3; 3] = std::array::from_fn(|r| add(scale(lin[r], s), scale(rot[r], 1.0 - s)));
                for (k, &i) in c.members.iter().enumerate() {
                    let g = add(centre, mat_vec(&m, c.rest[k]));
                    goal_sum[i as usize] = add(goal_sum[i as usize], g);
                    goal_n[i as usize] += 1.0;
                }
            }
            let a = j.stiffness.clamp(0.0, 1.0);
            for i in 0..self.pos.len() {
                if goal_n[i] > 0.0 {
                    let g = scale(goal_sum[i], 1.0 / goal_n[i]);
                    self.pos[i] = add(self.pos[i], scale(sub(g, self.pos[i]), a));
                }
            }
            for i in 0..self.pos.len() {
                for col in colliders {
                    if let Some(c) = col.push_out(self.pos[i], self.radius) {
                        let moved = sub(c.point, prev[i]);
                        let normal_part = scale(c.normal, dot(moved, c.normal));
                        let slide = sub(moved, normal_part);
                        self.pos[i] = add(add(prev[i], normal_part), scale(slide, 1.0 - j.friction.clamp(0.0, 1.0)));
                        if let Some(c2) = col.push_out(self.pos[i], self.radius) {
                            self.pos[i] = c2.point;
                        }
                    }
                }
            }
            for i in 0..self.pos.len() {
                let v = scale(sub(self.pos[i], prev[i]), 1.0 / h);
                self.vel[i] = v;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_jelly_ball_squashes_on_landing_and_springs_back() {
        let ball = studio_mesh::prim::ico_sphere(50.0, 3);
        let mut s = Soft::new(&ball, Jelly { stiffness: 0.25, squash: 0.6, damping: 0.05, ..Default::default() });
        s.push([0.0, 0.0, 0.0]);
        let floor = Collider::Ground { y: 150.0, friction: 0.6 };
        let mut flattest = 1.0f32;
        for _ in 0..60 {
            s.step(1.0 / 30.0, 6, [0.0, 980.0, 0.0], std::slice::from_ref(&floor));
            let (lo, hi) = bounds(&s.pos);
            flattest = flattest.min((hi[1] - lo[1]) / (hi[0] - lo[0]));
        }
        assert!(flattest < 0.95, "it squashes: {flattest}");
        let (lo, hi) = bounds(&s.pos);
        assert!(hi[1] <= 151.0 && hi[1] > 140.0, "on the floor: {lo:?} {hi:?}");
        assert!((hi[1] - lo[1]) > 80.0, "keeps its shape: {lo:?} {hi:?}");
    }

    fn bounds(p: &[V3]) -> (V3, V3) {
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for v in p {
            for i in 0..3 {
                lo[i] = lo[i].min(v[i]);
                hi[i] = hi[i].max(v[i]);
            }
        }
        (lo, hi)
    }
}
