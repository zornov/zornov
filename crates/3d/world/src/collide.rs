// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What a walker bumps into and stands on: the terrain, boxes, cylinders
//! and triangle meshes placed in the world, kept in a grid so a query
//! looks only at what is near.
//!
//! The queries are the building blocks of the character controller and the
//! camera: the highest floor under a point ([`World::ground_at`]), how far
//! to push a standing circle out of walls ([`World::push_out`]), what is
//! overhead ([`World::ceiling_above`]) and a ray through everything
//! ([`World::raycast`]).

use crate::asset::ColliderShape;
use crate::math::{Aabb, V3, add, dot, len, mul, norm, sub};
use crate::world::World;
use std::collections::HashMap;
use std::sync::Arc;
use studio_mesh::bvh::Bvh;

#[derive(Clone, Debug)]
pub enum Solid {
    /// A box turned `yaw` degrees about the vertical.
    Box { centre: V3, half: V3, yaw: f32 },
    /// An upright cylinder standing on `base`.
    Cylinder { base: V3, radius: f32, height: f32 },
    /// Triangles in their own space, placed.
    Mesh { bvh: Arc<Bvh>, at: V3, yaw: f32, scale: f32 },
}

fn rot(p: V3, yaw_deg: f32) -> V3 {
    let (s, c) = yaw_deg.to_radians().sin_cos();
    [p[0] * c + p[2] * s, p[1], -p[0] * s + p[2] * c]
}
fn unrot(p: V3, yaw_deg: f32) -> V3 {
    rot(p, -yaw_deg)
}

impl Solid {
    pub fn bounds(&self) -> Aabb {
        match self {
            Solid::Box { centre, half, yaw } => Aabb::around([0.0; 3], *half).placed(*centre, *yaw, 1.0),
            Solid::Cylinder { base, radius, height } => Aabb::new([base[0] - radius, base[1], base[2] - radius], [base[0] + radius, base[1] + height, base[2] + radius]),
            Solid::Mesh { bvh, at, yaw, scale } => {
                let n = &bvh.nodes[0];
                Aabb::new(n.lo, n.hi).placed(*at, *yaw, *scale)
            }
        }
    }
}

const GRID: f32 = 8.0;

#[derive(Clone, Debug, Default)]
pub struct SolidGrid {
    pub solids: Vec<Solid>,
    cells: HashMap<(i32, i32), Vec<u32>>,
}

impl SolidGrid {
    pub fn push(&mut self, s: Solid) {
        let b = s.bounds();
        let id = self.solids.len() as u32;
        let (x0, x1) = ((b.min[0] / GRID).floor() as i32, (b.max[0] / GRID).floor() as i32);
        let (z0, z1) = ((b.min[2] / GRID).floor() as i32, (b.max[2] / GRID).floor() as i32);
        // Huge solids (a mesh covering a city) take every cell they touch,
        // capped so a degenerate box cannot fill memory.
        if (x1 - x0 + 1) as i64 * (z1 - z0 + 1) as i64 > 40_000 {
            return;
        }
        for x in x0..=x1 {
            for z in z0..=z1 {
                self.cells.entry((x, z)).or_default().push(id);
            }
        }
        self.solids.push(s);
    }

    fn near(&self, b: &Aabb) -> Vec<u32> {
        let (x0, x1) = ((b.min[0] / GRID).floor() as i32, (b.max[0] / GRID).floor() as i32);
        let (z0, z1) = ((b.min[2] / GRID).floor() as i32, (b.max[2] / GRID).floor() as i32);
        let mut out = Vec::new();
        for x in x0..=x1 {
            for z in z0..=z1 {
                if let Some(v) = self.cells.get(&(x, z)) {
                    out.extend_from_slice(v);
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// The blockers of a world: from its objects and scatters that collide,
/// and its extra solids.
pub fn build_grid(w: &World) -> SolidGrid {
    let mut g = SolidGrid::default();
    let add_asset = |g: &mut SolidGrid, shape: &ColliderShape, pos: V3, yaw: f32, scale: f32| match shape {
        ColliderShape::None => {}
        ColliderShape::Cylinder { radius, height } => g.push(Solid::Cylinder { base: pos, radius: radius * scale, height: height * scale }),
        ColliderShape::Box(b) => {
            let c = rot(mul(b.centre(), scale), yaw);
            g.push(Solid::Box { centre: add(pos, c), half: mul(b.half(), scale), yaw });
        }
        ColliderShape::Mesh(bvh) => g.push(Solid::Mesh { bvh: bvh.clone(), at: pos, yaw, scale }),
    };
    for o in &w.objects {
        if o.collide {
            add_asset(&mut g, &w.assets[o.asset].collider, o.inst.pos, o.inst.yaw, o.inst.scale);
        }
    }
    for s in &w.scatters {
        if s.collide {
            for i in &s.instances {
                add_asset(&mut g, &w.assets[s.asset].collider, i.pos, i.yaw, i.scale);
            }
        }
    }
    for s in &w.extra_solids {
        g.push(s.clone());
    }
    g
}

/// A floor under a point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ground {
    pub y: f32,
    pub normal: V3,
    /// Whether it is the terrain (else a placed solid).
    pub terrain: bool,
}

/// Where a ray hit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub t: f32,
    pub point: V3,
    pub normal: V3,
    pub terrain: bool,
}

impl World {
    /// The highest surface at (x, z) that is at or below `y_max`.
    pub fn ground_at(&self, x: f32, z: f32, y_max: f32) -> Option<Ground> {
        let mut best: Option<Ground> = None;
        let mut take = |g: Ground| {
            if g.y <= y_max + 1e-4 && best.is_none_or(|b| g.y > b.y) {
                best = Some(g);
            }
        };
        if let Some(t) = &self.terrain {
            take(Ground { y: t.height_at(x, z), normal: t.normal_at(x, z), terrain: true });
        }
        let probe = Aabb::new([x - 0.01, -1e6, z - 0.01], [x + 0.01, 1e6, z + 0.01]);
        for id in self.solids.near(&probe) {
            match &self.solids.solids[id as usize] {
                Solid::Box { centre, half, yaw } => {
                    let l = unrot(sub([x, centre[1], z], *centre), *yaw);
                    if l[0].abs() <= half[0] && l[2].abs() <= half[2] {
                        take(Ground { y: centre[1] + half[1], normal: [0.0, 1.0, 0.0], terrain: false });
                    }
                }
                Solid::Cylinder { base, radius, height } => {
                    if (x - base[0]).powi(2) + (z - base[2]).powi(2) <= radius * radius {
                        take(Ground { y: base[1] + height, normal: [0.0, 1.0, 0.0], terrain: false });
                    }
                }
                Solid::Mesh { bvh, at, yaw, scale } => {
                    let o = mul(unrot(sub([x, y_max, z], *at), *yaw), 1.0 / scale);
                    if let Some(h) = bvh.ray(o, [0.0, -1.0, 0.0], 0.0, f32::INFINITY) {
                        let tri = bvh.tris[h.tri as usize];
                        let n = norm(crate::math::cross(sub(tri[1], tri[0]), sub(tri[2], tri[0])));
                        let n = if n[1] < 0.0 { mul(n, -1.0) } else { n };
                        take(Ground { y: y_max - h.t * scale, normal: rot(n, *yaw), terrain: false });
                    }
                }
            }
        }
        best
    }

    /// How far to move a standing circle (`radius`, centred on `p` in x
    /// and z) horizontally to clear everything whose height overlaps
    /// `y_lo..y_hi`. Anything lower than `y_lo` is something to step on,
    /// not a wall.
    pub fn push_out(&self, p: V3, radius: f32, y_lo: f32, y_hi: f32) -> [f32; 2] {
        let mut push = [0.0f32; 2];
        let probe = Aabb::new([p[0] - radius, y_lo, p[2] - radius], [p[0] + radius, y_hi, p[2] + radius]);
        for id in self.solids.near(&probe) {
            let s = &self.solids.solids[id as usize];
            if !s.bounds().overlaps(&probe) {
                continue;
            }
            match s {
                Solid::Box { centre, half, yaw } => {
                    // The span the box occupies must reach above y_lo.
                    if centre[1] + half[1] <= y_lo || centre[1] - half[1] >= y_hi {
                        continue;
                    }
                    let l = unrot(sub(p, *centre), *yaw);
                    let q = [l[0].clamp(-half[0], half[0]), 0.0, l[2].clamp(-half[2], half[2])];
                    let d = [l[0] - q[0], l[2] - q[2]];
                    let dd = (d[0] * d[0] + d[1] * d[1]).sqrt();
                    let out_l = if dd > 1e-5 {
                        if dd >= radius {
                            continue;
                        }
                        let k = (radius - dd) / dd;
                        [d[0] * k, 0.0, d[1] * k]
                    } else {
                        // Centre inside: leave by the nearest face.
                        let (ox, oz) = (half[0] - l[0].abs(), half[2] - l[2].abs());
                        if ox < oz { [(l[0].signum()) * (ox + radius), 0.0, 0.0] } else { [0.0, 0.0, (l[2].signum()) * (oz + radius)] }
                    };
                    let w = rot(out_l, *yaw);
                    push[0] += w[0];
                    push[1] += w[2];
                }
                Solid::Cylinder { base, radius: r, height } => {
                    if base[1] + height <= y_lo || base[1] >= y_hi {
                        continue;
                    }
                    let d = [p[0] - base[0], p[2] - base[2]];
                    let dd = (d[0] * d[0] + d[1] * d[1]).sqrt();
                    if dd >= r + radius {
                        continue;
                    }
                    let n = if dd > 1e-5 { [d[0] / dd, d[1] / dd] } else { [1.0, 0.0] };
                    let k = r + radius - dd;
                    push[0] += n[0] * k;
                    push[1] += n[1] * k;
                }
                Solid::Mesh { bvh, at, yaw, scale } => {
                    let step = (radius * 1.5).max(0.1);
                    let mut y = y_lo + radius * 0.5;
                    while y <= y_hi - radius * 0.5 + 1e-4 {
                        let q = mul(unrot(sub([p[0], y, p[2]], *at), *yaw), 1.0 / scale);
                        if let Some((c, _, d)) = bvh.nearest(q, radius / scale) {
                            let away = sub(q, c);
                            let h = [away[0], 0.0, away[2]];
                            let hl = len(h);
                            if hl > 1e-5 {
                                let k = (radius / scale - d).max(0.0) * scale;
                                let w = rot(mul(h, 1.0 / hl), *yaw);
                                push[0] += w[0] * k;
                                push[1] += w[2] * k;
                            }
                        }
                        y += step;
                    }
                }
            }
        }
        // The edge of the land is a wall.
        if let Some(t) = &self.terrain {
            let s = t.size();
            let (lo, hi) = ([t.origin[0] + radius, t.origin[1] + radius], [t.origin[0] + s[0] - radius, t.origin[1] + s[1] - radius]);
            if p[0] + push[0] < lo[0] {
                push[0] = lo[0] - p[0];
            }
            if p[0] + push[0] > hi[0] {
                push[0] = hi[0] - p[0];
            }
            if p[2] + push[1] < lo[1] {
                push[1] = lo[1] - p[2];
            }
            if p[2] + push[1] > hi[1] {
                push[1] = hi[1] - p[2];
            }
        }
        push
    }

    /// The lowest underside above `y_from` at (x, z), if anything.
    pub fn ceiling_above(&self, x: f32, z: f32, y_from: f32) -> Option<f32> {
        let mut best: Option<f32> = None;
        let probe = Aabb::new([x - 0.01, y_from, z - 0.01], [x + 0.01, 1e6, z + 0.01]);
        for id in self.solids.near(&probe) {
            let c = match &self.solids.solids[id as usize] {
                Solid::Box { centre, half, yaw } => {
                    let l = unrot(sub([x, centre[1], z], *centre), *yaw);
                    (l[0].abs() <= half[0] && l[2].abs() <= half[2] && centre[1] - half[1] >= y_from).then_some(centre[1] - half[1])
                }
                Solid::Cylinder { base, radius, .. } => ((x - base[0]).powi(2) + (z - base[2]).powi(2) <= radius * radius && base[1] >= y_from).then_some(base[1]),
                Solid::Mesh { bvh, at, yaw, scale } => {
                    let o = mul(unrot(sub([x, y_from, z], *at), *yaw), 1.0 / scale);
                    bvh.ray(o, [0.0, 1.0, 0.0], 1e-4, f32::INFINITY).map(|h| y_from + h.t * scale)
                }
            };
            if let Some(c) = c {
                best = Some(best.map_or(c, |b| b.min(c)));
            }
        }
        best
    }

    /// The nearest thing a ray meets (terrain and solids), within `max`.
    pub fn raycast(&self, o: V3, dir: V3, max: f32) -> Option<Hit> {
        let d = norm(dir);
        let mut best: Option<Hit> = None;
        let mut consider = |t: f32, n: V3, terrain: bool| {
            if t >= 0.0 && t <= max && best.is_none_or(|b| t < b.t) {
                best = Some(Hit { t, point: add(o, mul(d, t)), normal: n, terrain });
            }
        };
        if let Some(t) = &self.terrain
            && let Some(h) = t.raycast(o, d, max)
        {
            let p = add(o, mul(d, h));
            consider(h, t.normal_at(p[0], p[2]), true);
        }
        let end = add(o, mul(d, max));
        let seg = Aabb::new(std::array::from_fn(|i| o[i].min(end[i])), std::array::from_fn(|i| o[i].max(end[i])));
        for id in self.solids.near(&seg) {
            match &self.solids.solids[id as usize] {
                Solid::Box { centre, half, yaw } => {
                    let lo = unrot(sub(o, *centre), *yaw);
                    let ld = unrot(d, *yaw);
                    let b = Aabb::around([0.0; 3], *half);
                    if let Some(t) = b.ray(lo, ld, max) {
                        let p = add(lo, mul(ld, t));
                        // Face by the largest normalised coordinate.
                        let q = [p[0] / half[0].max(1e-6), p[1] / half[1].max(1e-6), p[2] / half[2].max(1e-6)];
                        let k = (0..3).max_by(|a, c| q[*a].abs().total_cmp(&q[*c].abs())).unwrap_or(1);
                        let mut n = [0.0; 3];
                        n[k] = q[k].signum();
                        consider(t, rot(n, *yaw), false);
                    }
                }
                Solid::Cylinder { base, radius, height } => {
                    let (ox, oz) = (o[0] - base[0], o[2] - base[2]);
                    let a = d[0] * d[0] + d[2] * d[2];
                    if a > 1e-9 {
                        let b = 2.0 * (ox * d[0] + oz * d[2]);
                        let c = ox * ox + oz * oz - radius * radius;
                        let disc = b * b - 4.0 * a * c;
                        if disc >= 0.0 {
                            let t = (-b - disc.sqrt()) / (2.0 * a);
                            let y = o[1] + d[1] * t;
                            if t >= 0.0 && y >= base[1] && y <= base[1] + height {
                                let n = norm([ox + d[0] * t, 0.0, oz + d[2] * t]);
                                consider(t, n, false);
                            }
                        }
                    }
                    if d[1].abs() > 1e-9 {
                        let t = (base[1] + height - o[1]) / d[1];
                        let (px, pz) = (ox + d[0] * t, oz + d[2] * t);
                        if t >= 0.0 && px * px + pz * pz <= radius * radius {
                            consider(t, [0.0, 1.0, 0.0], false);
                        }
                    }
                }
                Solid::Mesh { bvh, at, yaw, scale } => {
                    let lo = mul(unrot(sub(o, *at), *yaw), 1.0 / scale);
                    let ld = unrot(d, *yaw);
                    if let Some(h) = bvh.ray(lo, ld, 0.0, max / scale) {
                        let tri = bvh.tris[h.tri as usize];
                        let mut n = norm(crate::math::cross(sub(tri[1], tri[0]), sub(tri[2], tri[0])));
                        if dot(n, ld) > 0.0 {
                            n = mul(n, -1.0);
                        }
                        consider(h.t * scale, rot(n, *yaw), false);
                    }
                }
            }
        }
        best
    }

    /// The surface of water over (x, z), if there is some.
    pub fn water_at(&self, x: f32, z: f32) -> Option<f32> {
        self.water.iter().filter(|w| x >= w.min[0] && x <= w.max[0] && z >= w.min[1] && z <= w.max[1]).map(|w| w.level).reduce(f32::max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::Terrain;

    fn arena() -> World {
        let mut w = World::new("t");
        w.terrain = Some(Terrain::flat([128.0, 128.0], 1.0));
        w
    }

    #[test]
    fn floors_are_the_highest_surface_under_the_point() {
        let mut w = arena();
        w.extra_solids.push(Solid::Box { centre: [0.0, 1.0, 0.0], half: [2.0, 1.0, 2.0], yaw: 30.0 });
        w.extra_solids.push(Solid::Box { centre: [0.0, 5.0, 0.0], half: [3.0, 0.1, 3.0], yaw: 0.0 });
        w.finish();
        assert_eq!(w.ground_at(10.0, 10.0, 5.0).unwrap().y, 0.0);
        assert_eq!(w.ground_at(0.0, 0.0, 3.0).unwrap().y, 2.0);
        assert!((w.ground_at(0.0, 0.0, 6.0).unwrap().y - 5.1).abs() < 1e-5, "a floor above");
        assert!(w.ground_at(0.0, 0.0, 6.0).unwrap().terrain == false);
        // Turned box: its corner reaches further along x than its half.
        assert_eq!(w.ground_at(2.3, 0.7, 3.0).unwrap().y, 2.0);
        assert_eq!(w.ground_at(2.5, 2.5, 1.0).unwrap().y, 0.0);
    }

    #[test]
    fn walls_push_out_by_the_overlap_and_low_things_are_not_walls() {
        let mut w = arena();
        w.extra_solids.push(Solid::Box { centre: [5.0, 1.0, 0.0], half: [0.5, 1.0, 5.0], yaw: 0.0 });
        w.extra_solids.push(Solid::Cylinder { base: [-5.0, 0.0, 0.0], radius: 0.5, height: 3.0 });
        w.extra_solids.push(Solid::Box { centre: [0.0, 0.1, 5.0], half: [3.0, 0.1, 0.5], yaw: 0.0 });
        w.finish();
        // Standing 0.2 into the wall's face (wall face at x = 4.5).
        let p = w.push_out([4.3, 0.0, 0.0], 0.4, 0.5, 1.8);
        assert!((p[0] + 0.2).abs() < 1e-4 && p[1].abs() < 1e-4, "{p:?}");
        let c = w.push_out([-4.6, 0.0, 0.0], 0.4, 0.5, 1.8);
        assert!((c[0] + 0.0).abs() < 1.0 && c[0] > 0.0 - 1e-6 || c[0] < 1e-6);
        assert!(w.push_out([0.0, 0.0, 5.0], 0.4, 0.5, 1.8) == [0.0, 0.0], "a low kerb is stepped on");
        assert!(w.push_out([20.0, 0.0, 20.0], 0.4, 0.5, 1.8) == [0.0, 0.0]);
        // The edge of the land stops you.
        let e = w.push_out([63.9, 0.0, 0.0], 0.4, 0.5, 1.8);
        assert!(e[0] < -0.2);
    }

    #[test]
    fn rays_hit_terrain_boxes_and_cylinders_and_ceilings_are_found() {
        let mut w = arena();
        w.extra_solids.push(Solid::Box { centre: [0.0, 1.5, 10.0], half: [1.0, 1.5, 1.0], yaw: 45.0 });
        w.extra_solids.push(Solid::Cylinder { base: [10.0, 0.0, 0.0], radius: 1.0, height: 4.0 });
        w.extra_solids.push(Solid::Box { centre: [-10.0, 3.0, -10.0], half: [3.0, 0.1, 3.0], yaw: 0.0 });
        w.finish();
        let h = w.raycast([0.0, 1.0, 0.0], [0.0, 0.0, 1.0], 50.0).unwrap();
        assert!(h.t > 8.0 && h.t < 9.5 && h.normal[2] < 0.0, "{h:?}");
        let c = w.raycast([0.0, 2.0, 0.0], [1.0, 0.0, 0.0], 50.0).unwrap();
        assert!((c.t - 9.0).abs() < 1e-3 && c.normal[0] < -0.99);
        let g = w.raycast([30.0, 10.0, 30.0], [0.0, -1.0, 0.0], 50.0).unwrap();
        assert!(g.terrain && (g.t - 10.0).abs() < 0.05);
        assert!(w.raycast([0.0, 1.0, 0.0], [0.0, 0.0, -1.0], 20.0).is_none() || w.raycast([0.0, 1.0, 0.0], [0.0, 0.0, -1.0], 20.0).unwrap().terrain);
        assert_eq!(w.ceiling_above(-10.0, -10.0, 1.8), Some(2.9));
        assert_eq!(w.ceiling_above(0.0, 0.0, 1.8), None);
    }

    #[test]
    fn a_triangle_mesh_is_a_floor_and_a_wall() {
        // A step: a box of triangles 4 x 1 x 4 as a mesh solid.
        let m = studio_mesh::prim::cube([4.0, 1.0, 4.0]);
        let tris: Vec<[V3; 3]> = (0..m.triangles()).map(|t| m.tri(t).map(|i| m.positions[i])).collect();
        let bvh = Arc::new(Bvh::new(tris));
        let mut w = arena();
        w.extra_solids.push(Solid::Mesh { bvh, at: [0.0, 0.5, 0.0], yaw: 0.0, scale: 1.0 });
        w.finish();
        let g = w.ground_at(0.0, 0.0, 5.0).unwrap();
        assert!((g.y - 1.0).abs() < 1e-3, "{g:?}");
        let p = w.push_out([2.2, 0.0, 0.0], 0.5, 1.1, 1.9);
        assert!(p[0].abs() < 1e-6 || p[0] > 0.0, "above the block there is no wall: {p:?}");
        let q = w.push_out([2.1, 0.0, 0.0], 0.5, 0.2, 1.9);
        assert!(q[0] > 0.3, "{q:?}");
    }
}
