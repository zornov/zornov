// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Hair and fur: strands grown from part of a model's surface (a scalp,
//! a pelt, a mane, grass on a field), each a chain of points rooted where
//! it grows. They follow the surface as it moves (a turning head, a
//! walking character), swing with momentum, sag under gravity, blow in
//! the wind and lie on whatever they touch.
//!
//! Each strand keeps its segment lengths and its curve (distance
//! constraints to the next point and the one after, position-based); the
//! strands are drawn as thin crossed ribbons, tapering to the tip, in a
//! material named "Hair" (rendered with a hair sheen by the path tracer).

use crate::{Collider, Rng, V3, add, cross, dot, length, normalise, scale, sub};
use studio_mesh::Mesh;

/// How the hair is grown.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Groom {
    pub count: usize,
    /// Strand length (px).
    pub length: f32,
    pub segments: usize,
    /// Width at the root (px).
    pub width: f32,
    /// 0 straight – 1 curly.
    pub curl: f32,
    /// 0 floppy – 1 springy: how much a strand holds its shape.
    pub stiffness: f32,
    /// How much strands lie down the way gravity pulls at rest, 0–1.
    pub comb: f32,
    pub seed: u64,
    /// Linear RGB.
    pub colour: V3,
}

impl Default for Groom {
    fn default() -> Groom {
        Groom { count: 1500, length: 60.0, segments: 6, width: 1.6, curl: 0.0, stiffness: 0.5, comb: 0.5, seed: 1, colour: [0.08, 0.05, 0.03] }
    }
}

/// Where a strand grows: a triangle of the model and a place on it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Root {
    pub tri: u32,
    pub bary: [f32; 3],
}

/// Grown hair being simulated.
pub struct Hair {
    pub roots: Vec<Root>,
    /// Points of every strand, strand by strand (segments + 1 each).
    pub pos: Vec<V3>,
    prev: Vec<V3>,
    /// Rest shape of each strand relative to its root frame.
    rest: Vec<V3>,
    seg_len: Vec<f32>,
    pub groom: Groom,
}

/// Where a root is and which way the surface faces there, on a posed model.
pub fn root_frame(positions: &[V3], mesh: &Mesh, r: &Root) -> (V3, V3) {
    let [a, b, c] = mesh.tri(r.tri as usize).map(|v| positions[v]);
    let p = add(add(scale(a, r.bary[0]), scale(b, r.bary[1])), scale(c, r.bary[2]));
    (p, normalise(cross(sub(b, a), sub(c, a))))
}

impl Hair {
    /// Grows strands on the faces `faces` of `mesh` (area-weighted).
    pub fn grow(mesh: &Mesh, faces: &[usize], groom: Groom, gravity: V3) -> Hair {
        let mut rng = Rng::new(groom.seed);
        let areas: Vec<f32> = faces.iter().map(|&t| {
            let [a, b, c] = mesh.tri(t).map(|v| mesh.positions[v]);
            length(cross(sub(b, a), sub(c, a))) * 0.5
        }).collect();
        let total: f32 = areas.iter().sum();
        let segs = groom.segments.clamp(2, 24);
        let mut roots = Vec::with_capacity(groom.count);
        let mut pos = Vec::new();
        let mut rest = Vec::new();
        let mut seg_len = Vec::new();
        let down = normalise(gravity);
        if total <= 0.0 {
            return Hair { roots, prev: pos.clone(), pos, rest, seg_len, groom };
        }
        // Cumulative areas for picking faces.
        let mut acc = Vec::with_capacity(areas.len());
        let mut s = 0.0;
        for a in &areas {
            s += a;
            acc.push(s);
        }
        for _ in 0..groom.count.min(200_000) {
            let x = rng.next() * total;
            let k = acc.partition_point(|&v| v < x).min(faces.len() - 1);
            let (mut u, mut v) = (rng.next(), rng.next());
            if u + v > 1.0 {
                u = 1.0 - u;
                v = 1.0 - v;
            }
            let root = Root { tri: faces[k] as u32, bary: [1.0 - u - v, u, v] };
            let (p, n) = root_frame(&mesh.positions, mesh, &root);
            let len = groom.length * rng.range(0.75, 1.15);
            let l = len / segs as f32;
            // Out from the surface, bending the way it is combed.
            let mut dir = normalise(add(n, scale(down, groom.comb * 2.0)));
            if dir == [0.0; 3] {
                dir = n;
            }
            let side = normalise(cross(n, if dot(n, [0.0, 0.0, 1.0]).abs() > 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 0.0, 1.0] }));
            let side2 = cross(dir, side);
            let phase = rng.range(0.0, std::f32::consts::TAU);
            let mut q = p;
            pos.push(p);
            rest.push([0.0; 3]);
            for i in 1..=segs {
                let curl = groom.curl * l * 0.9;
                let a = phase + i as f32 * 1.7;
                let step = add(scale(dir, l), add(scale(side, curl * a.cos()), scale(side2, curl * a.sin())));
                // Gradually more under gravity towards the tip.
                let step = add(step, scale(down, l * groom.comb * 0.4 * i as f32 / segs as f32));
                let step = scale(normalise(step), l);
                q = add(q, step);
                pos.push(q);
                rest.push(sub(q, p));
            }
            roots.push(root);
            seg_len.push(l);
        }
        Hair { roots, prev: pos.clone(), pos, rest, seg_len, groom }
    }

    fn per(&self) -> usize {
        self.groom.segments.clamp(2, 24) + 1
    }

    /// One frame: roots follow the (posed) surface, the rest swings.
    pub fn step(&mut self, positions: &[V3], mesh: &Mesh, dt: f32, substeps: usize, gravity: V3, wind: &crate::cloth::Wind, t: f32, colliders: &[Collider]) {
        let per = self.per();
        let n = substeps.max(1);
        let h = dt / n as f32;
        let frames: Vec<(V3, V3)> = self.roots.iter().map(|r| root_frame(positions, mesh, r)).collect();
        let starts: Vec<V3> = (0..self.roots.len()).map(|s| self.pos[s * per]).collect();
        let stiff = self.groom.stiffness.clamp(0.0, 1.0);
        let radius = self.groom.width;
        for k in 0..n {
            let blend = (k + 1) as f32 / n as f32;
            for s in 0..self.roots.len() {
                let base = s * per;
                let root = crate::lerp(starts[s], frames[s].0, blend);
                self.pos[base] = root;
                self.prev[base] = root;
                let l = self.seg_len[s];
                for i in 1..per {
                    let j = base + i;
                    let v = sub(self.pos[j], self.prev[j]);
                    self.prev[j] = self.pos[j];
                    let w = wind.at(self.pos[j], t + h * k as f32);
                    let air = scale(sub(w, scale(v, 1.0 / h)), 0.02);
                    self.pos[j] = add(add(self.pos[j], scale(v, 0.985)), scale(add(gravity, air), h * h));
                }
                // Lengths, from the root out (the root does not move).
                for _ in 0..2 {
                    for i in 1..per {
                        let (a, b) = (base + i - 1, base + i);
                        let d = sub(self.pos[b], self.pos[a]);
                        let dl = length(d).max(1e-6);
                        let fix = scale(d, (dl - l) / dl);
                        if i == 1 {
                            self.pos[b] = sub(self.pos[b], fix);
                        } else {
                            self.pos[a] = add(self.pos[a], scale(fix, 0.5));
                            self.pos[b] = sub(self.pos[b], scale(fix, 0.5));
                        }
                    }
                    // Shape: towards the rest curve hung from the root (so
                    // springy hair stands up, soft hair falls).
                    if stiff > 0.0 {
                        let (root_p, _) = frames[s];
                        for i in 1..per {
                            let goal = add(root_p, self.rest[base + i]);
                            let k2 = stiff * stiff * 0.08 / i as f32;
                            self.pos[base + i] = add(self.pos[base + i], scale(sub(goal, self.pos[base + i]), k2));
                        }
                    }
                }
                for i in 1..per {
                    for c in colliders {
                        if let Some(hit) = c.push_out(self.pos[base + i], radius) {
                            self.pos[base + i] = hit.point;
                        }
                    }
                }
            }
        }
    }

    /// The strands as ribbons: the mesh (positions as they are now).
    pub fn geometry(&self) -> Mesh {
        let per = self.per();
        let mut m = Mesh::default();
        m.positions = self.vertices();
        m.normals = vec![[0.0, 0.0, -1.0]; m.positions.len()];
        m.uvs = vec![[0.0; 2]; m.positions.len()];
        for s in 0..self.roots.len() {
            let base = (s * per * 4) as u32;
            for i in 0..per as u32 {
                let v = (i as f32) / (per - 1) as f32;
                for k in 0..4 {
                    m.uvs[(base + i * 4 + k) as usize] = [if k % 2 == 0 { 0.0 } else { 1.0 }, v];
                }
            }
            for i in 0..per as u32 - 1 {
                for rib in 0..2u32 {
                    let a = base + i * 4 + rib * 2;
                    let b = a + 4;
                    m.indices.extend_from_slice(&[a, b, a + 1, a + 1, b, b + 1]);
                }
            }
        }
        m.materials.push(studio_mesh::Material { name: "Hair".into(), base_colour: [self.groom.colour[0], self.groom.colour[1], self.groom.colour[2], 1.0], roughness: 0.42, ..Default::default() });
        m.single_group();
        studio_mesh::ops::smooth_normals_in_place(&mut m);
        m
    }

    /// Ribbon corner positions for the strands as they are now (four per
    /// strand point: two crossed ribbons, tapering to the tip).
    pub fn vertices(&self) -> Vec<V3> {
        let per = self.per();
        let mut out = Vec::with_capacity(self.pos.len() * 4);
        for s in 0..self.roots.len() {
            let base = s * per;
            for i in 0..per {
                let p = self.pos[base + i];
                let dir = if i + 1 < per { sub(self.pos[base + i + 1], p) } else { sub(p, self.pos[base + i - 1]) };
                let dir = normalise(dir);
                let side = normalise(cross(dir, if dir[1].abs() > 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] }));
                let side2 = cross(dir, side);
                let w = self.groom.width * 0.5 * (1.0 - 0.85 * i as f32 / (per - 1) as f32);
                out.push(add(p, scale(side, -w)));
                out.push(add(p, scale(side, w)));
                out.push(add(p, scale(side2, -w)));
                out.push(add(p, scale(side2, w)));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fur_grows_on_top_and_falls_when_the_head_tilts() {
        let ball = studio_mesh::prim::ico_sphere(60.0, 3);
        let top = studio_mesh::region::Region::Facing { direction: [0.0, -1.0, 0.0], degrees: 50.0 }.faces(&ball);
        let g = Groom { count: 200, length: 50.0, segments: 5, stiffness: 0.2, comb: 0.0, ..Default::default() };
        let mut h = Hair::grow(&ball, &top, g, [0.0, 980.0, 0.0]);
        assert_eq!(h.roots.len(), 200);
        let geo = h.geometry();
        assert_eq!(geo.positions.len(), 200 * 6 * 4);
        // Tips start above the ball and sag once gravity has its way.
        let tip_y = |h: &Hair| (0..h.roots.len()).map(|s| h.pos[s * 6 + 5][1]).sum::<f32>() / h.roots.len() as f32;
        let start = tip_y(&h);
        assert!(start < -60.0, "{start}");
        let col = vec![Collider::Sphere { centre: [0.0; 3], radius: 60.0 }];
        for f in 0..30 {
            h.step(&ball.positions, &ball, 1.0 / 30.0, 4, [0.0, 980.0, 0.0], &crate::cloth::Wind::default(), f as f32 / 30.0, &col);
        }
        let end = tip_y(&h);
        assert!(end > start + 10.0, "sags: {start} → {end}");
        // Strands keep their length and stay outside the head.
        for s in 0..h.roots.len() {
            for i in 1..6 {
                let l = length(sub(h.pos[s * 6 + i], h.pos[s * 6 + i - 1]));
                assert!((l - h.seg_len[s]).abs() < h.seg_len[s] * 0.25, "{l}");
                assert!(length(h.pos[s * 6 + i]) > 58.0);
            }
        }
        assert_eq!(h.vertices().len(), geo.positions.len());
    }
}
