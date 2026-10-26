// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Cloth: capes, flags, curtains, tablecloths, banners, sails.
//!
//! Extended position-based dynamics with small steps: each step predicts
//! where every point goes (gravity, wind, drag), then pulls them back so
//! edges keep their length (stretch) and neighbouring triangles keep their
//! fold (bend), holds pinned points where they are told to be, and pushes
//! points out of colliders with friction. Wind pushes on each triangle by
//! how squarely it faces the moving air, with gusts.

use crate::{Collider, V3, add, cross, dot, length, scale, sub};
use studio_mesh::Mesh;

/// How a cloth behaves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fabric {
    /// 0 stretchy – 1 does not stretch.
    pub stretch: f32,
    /// 0 drapes like silk – 1 stiff like card.
    pub bend: f32,
    /// Grams per square pixel, relatively: heavier cloth swings slower in wind.
    pub weight: f32,
    /// Air resistance, 0–1.
    pub drag: f32,
    /// Distance kept from colliders (px).
    pub thickness: f32,
    pub friction: f32,
}

impl Fabric {
    pub fn named(name: &str) -> Option<Fabric> {
        Some(match name {
            "silk" => Fabric { stretch: 0.9, bend: 0.02, weight: 0.5, drag: 0.08, thickness: 1.5, friction: 0.2 },
            "cotton" | "cloth" => Fabric { stretch: 0.95, bend: 0.1, weight: 1.0, drag: 0.05, thickness: 2.0, friction: 0.5 },
            "denim" | "canvas" => Fabric { stretch: 0.99, bend: 0.35, weight: 1.8, drag: 0.04, thickness: 2.5, friction: 0.6 },
            "leather" => Fabric { stretch: 0.995, bend: 0.6, weight: 2.5, drag: 0.03, thickness: 3.0, friction: 0.7 },
            "rubber" => Fabric { stretch: 0.6, bend: 0.3, weight: 1.5, drag: 0.03, thickness: 2.0, friction: 0.9 },
            "flag" | "nylon" => Fabric { stretch: 0.97, bend: 0.05, weight: 0.6, drag: 0.1, thickness: 1.5, friction: 0.3 },
            _ => return None,
        })
    }
}

impl Default for Fabric {
    fn default() -> Fabric {
        Fabric::named("cotton").unwrap_or(Fabric { stretch: 0.95, bend: 0.1, weight: 1.0, drag: 0.05, thickness: 2.0, friction: 0.5 })
    }
}

/// Moving air: a steady push plus gusts (px/s).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Wind {
    pub velocity: V3,
    /// How much it gusts, 0–1 of its speed.
    pub gusts: f32,
    pub seed: u32,
}

impl Wind {
    pub fn at(&self, p: V3, t: f32) -> V3 {
        let speed = length(self.velocity);
        if speed <= 0.0 && self.gusts <= 0.0 {
            return [0.0; 3];
        }
        let size = 300.0;
        let drift = add(p, scale(self.velocity, -t));
        let g = crate::noise(scale(add(drift, [0.0, 0.0, t * 120.0]), 1.0 / size), self.seed);
        let swirl = crate::curl(add(drift, [t * 40.0, 0.0, 0.0]), size, self.seed.wrapping_add(7));
        add(scale(self.velocity, 1.0 + self.gusts * g), scale(swirl, speed.max(60.0) * self.gusts * 0.6))
    }
}

/// A cloth being simulated: points (welded vertices of its mesh).
pub struct Cloth {
    pub pos: Vec<V3>,
    prev: Vec<V3>,
    pub vel: Vec<V3>,
    inv_mass: Vec<f32>,
    /// Edges and their rest lengths.
    edges: Vec<(u32, u32, f32)>,
    /// Points across shared edges and their rest distances (bending).
    bends: Vec<(u32, u32, f32)>,
    tris: Vec<[u32; 3]>,
    /// Which welded point each mesh vertex is.
    ids: Vec<u32>,
    pub fabric: Fabric,
    /// Pinned points: index and where it should be (updated each frame).
    pub pins: Vec<(u32, V3)>,
}

impl Cloth {
    /// A cloth from a mesh (already where it starts, in the world).
    pub fn new(mesh: &Mesh, fabric: Fabric) -> Cloth {
        let (ids, unique) = mesh.position_ids();
        let tris: Vec<[u32; 3]> = (0..mesh.triangles()).map(|t| mesh.tri(t).map(|v| ids[v])).filter(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2]).collect();
        let mut edge_tris: std::collections::HashMap<(u32, u32), Vec<u32>> = std::collections::HashMap::new();
        for t in &tris {
            for (a, b, c) in [(t[0], t[1], t[2]), (t[1], t[2], t[0]), (t[2], t[0], t[1])] {
                edge_tris.entry((a.min(b), a.max(b))).or_default().push(c);
            }
        }
        let d = |a: u32, b: u32| length(sub(unique[a as usize], unique[b as usize]));
        // In a fixed order, so the same cloth always moves the same way.
        let mut keys: Vec<(u32, u32)> = edge_tris.keys().copied().collect();
        keys.sort_unstable();
        let edges = keys.iter().map(|&(a, b)| (a, b, d(a, b))).collect();
        let bends = keys.iter().filter_map(|k| edge_tris.get(k)).filter(|o| o.len() == 2).map(|o| (o[0], o[1], d(o[0], o[1]))).collect();
        // Mass from the area around each point.
        let mut mass = vec![0.0f32; unique.len()];
        for t in &tris {
            let a = length(cross(sub(unique[t[1] as usize], unique[t[0] as usize]), sub(unique[t[2] as usize], unique[t[0] as usize]))) * 0.5;
            for v in t {
                mass[*v as usize] += a / 3.0 * fabric.weight * 0.01;
            }
        }
        let inv_mass = mass.iter().map(|m| if *m > 0.0 { 1.0 / m } else { 0.0 }).collect();
        let n = unique.len();
        Cloth { prev: unique.clone(), pos: unique, vel: vec![[0.0; 3]; n], inv_mass, edges, bends, tris, ids, fabric, pins: Vec::new() }
    }

    /// Pins points within `radius` of `at` where they are (returns how
    /// many).
    pub fn pin_near(&mut self, at: V3, radius: f32) -> usize {
        let mut n = 0;
        for (i, p) in self.pos.iter().enumerate() {
            if length(sub(*p, at)) <= radius {
                self.pins.push((i as u32, *p));
                n += 1;
            }
        }
        n
    }

    /// Pins the points along one side: "top", "bottom", "left", "right",
    /// "front", "back" (within `band` px of that edge).
    pub fn pin_side(&mut self, side: &str, band: f32) -> usize {
        let (axis, low) = match side {
            "top" => (1, true),
            "bottom" => (1, false),
            "left" => (0, true),
            "right" => (0, false),
            "front" => (2, true),
            "back" => (2, false),
            _ => return 0,
        };
        let edge = self.pos.iter().map(|p| p[axis]).fold(if low { f32::INFINITY } else { f32::NEG_INFINITY }, |a, v| if low { a.min(v) } else { a.max(v) });
        let mut n = 0;
        for (i, p) in self.pos.iter().enumerate() {
            if (p[axis] - edge).abs() <= band {
                self.pins.push((i as u32, *p));
                n += 1;
            }
        }
        n
    }

    /// Mesh vertex positions (seam vertices follow their welded point).
    pub fn vertices(&self) -> Vec<V3> {
        self.ids.iter().map(|&i| self.pos[i as usize]).collect()
    }

    /// One frame forward (`dt` seconds) in `substeps` small steps.
    pub fn step(&mut self, dt: f32, substeps: usize, gravity: V3, wind: &Wind, t: f32, colliders: &[Collider]) {
        let n = substeps.max(1);
        let h = dt / n as f32;
        let pinned: std::collections::HashMap<u32, V3> = self.pins.iter().copied().collect();
        // Where pins start this frame (from where they were) to blend them in.
        let pin_from: Vec<(u32, V3, V3)> = self.pins.iter().map(|&(i, to)| (i, self.pos[i as usize], to)).collect();
        // Compliance: softer constraints give way (XPBD α / h²).
        let a_stretch = (1.0 - self.fabric.stretch.clamp(0.0, 1.0)) * 1e-3 / (h * h);
        let a_bend = (1.0 - self.fabric.bend.clamp(0.0, 1.0)).powi(2) * 0.5 / (h * h);
        for s in 0..n {
            let ts = t + h * s as f32;
            // Wind on each triangle by how squarely it meets the air.
            let mut force = vec![[0.0f32; 3]; self.pos.len()];
            for tr in &self.tris {
                let [a, b, c] = tr.map(|v| v as usize);
                let nrm = cross(sub(self.pos[b], self.pos[a]), sub(self.pos[c], self.pos[a]));
                let area2 = length(nrm);
                if area2 < 1e-9 {
                    continue;
                }
                let nu = scale(nrm, 1.0 / area2);
                let centre = scale(add(add(self.pos[a], self.pos[b]), self.pos[c]), 1.0 / 3.0);
                let v = scale(add(add(self.vel[a], self.vel[b]), self.vel[c]), 1.0 / 3.0);
                let rel = sub(wind.at(centre, ts), v);
                // Pressure on the face, plus a little skin friction along it
                // (what pulls a flag out even when the wind runs along it).
                let normal_part = dot(rel, nu);
                let along = sub(rel, scale(nu, normal_part));
                let k = area2 * 0.5 * self.fabric.drag * 0.6;
                let f = add(scale(nu, normal_part * k), scale(along, k * 0.15));
                for k in [a, b, c] {
                    force[k] = add(force[k], scale(f, 1.0 / 3.0));
                }
            }
            for i in 0..self.pos.len() {
                self.prev[i] = self.pos[i];
                if self.inv_mass[i] == 0.0 || pinned.contains_key(&(i as u32)) {
                    continue;
                }
                let acc = add(gravity, scale(force[i], self.inv_mass[i]));
                self.vel[i] = scale(add(self.vel[i], scale(acc, h)), 1.0 - self.fabric.drag * 0.02);
                self.pos[i] = add(self.pos[i], scale(self.vel[i], h));
            }
            // Pins slide to this frame's places over the substeps.
            let k = (s + 1) as f32 / n as f32;
            for &(i, from, to) in &pin_from {
                self.pos[i as usize] = crate::lerp(from, to, k);
            }
            let w = |i: u32| if pinned.contains_key(&i) { 0.0 } else { self.inv_mass[i as usize] };
            let solve = |list: &[(u32, u32, f32)], alpha: f32, pos: &mut Vec<V3>| {
                for &(a, b, rest) in list {
                    let (wa, wb) = (w(a), w(b));
                    let ws = wa + wb;
                    if ws <= 0.0 {
                        continue;
                    }
                    let d = sub(pos[a as usize], pos[b as usize]);
                    let l = length(d);
                    if l < 1e-9 {
                        continue;
                    }
                    let c = l - rest;
                    let lambda = -c / (ws + alpha);
                    let corr = scale(d, lambda / l);
                    pos[a as usize] = add(pos[a as usize], scale(corr, wa));
                    pos[b as usize] = sub(pos[b as usize], scale(corr, wb));
                }
            };
            let mut pos = std::mem::take(&mut self.pos);
            solve(&self.edges, a_stretch, &mut pos);
            solve(&self.bends, a_bend, &mut pos);
            solve(&self.edges, a_stretch, &mut pos);
            self.pos = pos;
            // Collisions, with friction against the surface.
            let r = self.fabric.thickness;
            for i in 0..self.pos.len() {
                if w(i as u32) == 0.0 {
                    continue;
                }
                for col in colliders {
                    if let Some(c) = col.push_out(self.pos[i], r) {
                        let moved = sub(c.point, self.prev[i]);
                        let normal_part = scale(c.normal, dot(moved, c.normal));
                        let slide = sub(moved, normal_part);
                        let keep = (1.0 - self.fabric.friction.max(c.friction)).clamp(0.0, 1.0);
                        self.pos[i] = add(add(self.prev[i], normal_part), scale(slide, keep));
                        if let Some(c2) = col.push_out(self.pos[i], r) {
                            self.pos[i] = c2.point;
                        }
                    }
                }
            }
            for i in 0..self.pos.len() {
                self.vel[i] = scale(sub(self.pos[i], self.prev[i]), 1.0 / h);
            }
        }
        // Pins exactly where asked at the end of the frame.
        for (i, to) in &self.pins {
            self.pos[*i as usize] = *to;
        }
    }
}

/// A rectangle of cloth `width` × `height` px with `cells` across its
/// longer side, hanging in the xy plane (facing the camera), centred.
pub fn sheet(width: f32, height: f32, cells: usize) -> Mesh {
    let big = width.max(height).max(1.0);
    let c = cells.clamp(2, 160) as f32;
    let nx = ((width / big * c).round() as usize).max(2);
    let ny = ((height / big * c).round() as usize).max(2);
    let mut m = Mesh::default();
    for y in 0..=ny {
        for x in 0..=nx {
            let u = x as f32 / nx as f32;
            let v = y as f32 / ny as f32;
            m.positions.push([(u - 0.5) * width, (v - 0.5) * height, 0.0]);
            m.normals.push([0.0, 0.0, -1.0]);
            m.uvs.push([u, v]);
        }
    }
    let at = |x: usize, y: usize| (y * (nx + 1) + x) as u32;
    for y in 0..ny {
        for x in 0..nx {
            // Alternate diagonals so the cloth folds evenly.
            if (x + y) % 2 == 0 {
                m.indices.extend_from_slice(&[at(x, y), at(x, y + 1), at(x + 1, y + 1), at(x, y), at(x + 1, y + 1), at(x + 1, y)]);
            } else {
                m.indices.extend_from_slice(&[at(x, y), at(x, y + 1), at(x + 1, y), at(x + 1, y), at(x, y + 1), at(x + 1, y + 1)]);
            }
        }
    }
    m.materials.push(studio_mesh::Material { name: "Cloth".into(), base_colour: [0.55, 0.08, 0.08, 1.0], roughness: 0.85, ..Default::default() });
    m.single_group();
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_curtain_hangs_from_its_top_and_drapes_over_a_ball() {
        // A tablecloth over a ball: it settles over the top, sags around.
        let mut m = sheet(300.0, 300.0, 30);
        studio_mesh::ops::transform(&mut m, &studio_mesh::region::Region::All, [0.0, -10.0, 0.0], [90.0, 0.0, 0.0], [1.0; 3], [0.0; 3]);
        let mut c = Cloth::new(&m, Fabric::default());
        let ball = Collider::Sphere { centre: [0.0, 100.0, 0.0], radius: 80.0 };
        for f in 0..90 {
            c.step(1.0 / 30.0, 8, [0.0, 980.0, 0.0], &Wind::default(), f as f32 / 30.0, std::slice::from_ref(&ball));
        }
        let top = c.pos.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min);
        assert!((top - (100.0 - 80.0 - 2.0)).abs() < 8.0, "rests on the ball: {top}");
        let corners_low = c.pos.iter().filter(|p| p[1] > 120.0).count();
        assert!(corners_low > c.pos.len() / 5, "the edges hang down");
        // A flag pinned at its left edge streams out in the wind.
        let mut f = Cloth::new(&sheet(200.0, 120.0, 20), Fabric::named("flag").unwrap());
        assert!(f.pin_side("left", 1.0) >= 3);
        let wind = Wind { velocity: [600.0, 0.0, 0.0], gusts: 0.4, seed: 2 };
        for k in 0..90 {
            f.step(1.0 / 30.0, 8, [0.0, 980.0, 0.0], &wind, k as f32 / 30.0, &[]);
        }
        let far = f.pos.iter().map(|p| p[0]).fold(f32::NEG_INFINITY, f32::max);
        assert!(far > 30.0, "streams out to the right: {far}");
        // Edges hardly stretch.
        let worst = f.edges.iter().map(|&(a, b, r)| (length(sub(f.pos[a as usize], f.pos[b as usize])) - r).abs() / r).fold(0.0, f32::max);
        assert!(worst < 0.25, "stretch {worst}");
    }
}
