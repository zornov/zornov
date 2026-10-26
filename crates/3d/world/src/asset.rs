// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Things a world is made of: a shape with levels of detail, a size, and
//! how it blocks a walker. Models arrive from Studio's mesh crate (pixels,
//! y down) and are turned into the world's metres, y up.

use crate::math::{Aabb, V3};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use studio_mesh::Mesh;
use studio_mesh::bvh::Bvh;

/// How an asset stops a walker.
#[derive(Clone, Debug)]
pub enum ColliderShape {
    None,
    /// An upright cylinder from the origin up (a trunk, a pillar, a rock).
    Cylinder { radius: f32, height: f32 },
    /// A box in the asset's own space.
    Box(Aabb),
    /// The triangles of the asset's coarsest level, in its own space.
    Mesh(Arc<Bvh>),
}

#[derive(Clone, Debug)]
pub struct Asset {
    pub name: String,
    /// Finest first.
    pub lods: Vec<Arc<Mesh>>,
    /// Distance where each coarser level takes over (`lods.len() - 1`
    /// entries, rising).
    pub lod_starts: Vec<f32>,
    /// Beyond this (metres, to the asset's centre) nothing is drawn; 0
    /// means always.
    pub cull_distance: f32,
    pub bounds: Aabb,
    pub collider: ColliderShape,
    pub casts_shadow: bool,
    /// Wind sway at the top, metres (0 for rigid things).
    pub sway: f32,
    /// Identity for GPU caches: changes whenever the shapes do.
    pub key: u64,
    /// A rigged model (studio units, y down), its metres per unit and the
    /// height its feet were lifted by: posed per frame for characters.
    pub rigged: Option<(Arc<Mesh>, f32, f32)>,
}

fn next_key() -> u64 {
    static K: AtomicU64 = AtomicU64::new(1);
    K.fetch_add(1, Ordering::Relaxed)
}

/// A studio model (pixels, y down) as metres, y up: the winding is
/// reversed with the mirror so faces still point out.
pub fn mesh_from_studio(m: &Mesh, metres_per_unit: f32) -> Mesh {
    let mut out = m.clone();
    for p in out.positions.iter_mut() {
        *p = [p[0] * metres_per_unit, -p[1] * metres_per_unit, p[2] * metres_per_unit];
    }
    for n in out.normals.iter_mut() {
        *n = [n[0], -n[1], n[2]];
    }
    for t in out.indices.chunks_mut(3) {
        t.swap(1, 2);
    }
    // Groups are runs of triangles; order is unchanged.
    out.rig = None;
    out
}

pub fn mesh_bounds(m: &Mesh) -> Aabb {
    let mut b = Aabb::EMPTY;
    for p in &m.positions {
        b.grow(*p);
    }
    if b.is_empty() { Aabb::new([0.0; 3], [0.0; 3]) } else { b }
}

impl Asset {
    /// An asset from a mesh already in metres, y up, with coarser levels
    /// made by edge collapse to roughly `lod_ratios` of the triangles.
    pub fn new(name: &str, mesh: Mesh, lod_ratios: &[f32]) -> Asset {
        let tris = mesh.triangles();
        let bounds = mesh_bounds(&mesh);
        let mut lods = vec![Arc::new(mesh.clone())];
        for r in lod_ratios {
            let target = ((tris as f32 * r) as usize).max(12);
            if target + 8 >= lods.last().map_or(0, |l| l.triangles()) {
                continue;
            }
            let mut m = (**lods.last().unwrap()).clone();
            // Decimation works in the mesh's own (y-down) convention only
            // through geometry, so it is safe on y-up meshes.
            studio_mesh::decimate::decimate(&mut m, target);
            lods.push(Arc::new(m));
        }
        let size = bounds.radius().max(0.1);
        let lod_starts = (1..lods.len()).map(|i| size * 14.0 * (i as f32).powf(1.6)).collect();
        Asset { name: name.into(), lods, lod_starts, cull_distance: 0.0, bounds, collider: ColliderShape::None, casts_shadow: true, sway: 0.0, key: next_key(), rigged: None }
    }

    /// From a studio mesh (pixels, y down), `metres_per_unit` converting.
    pub fn from_studio(name: &str, mesh: &Mesh, metres_per_unit: f32, lod_ratios: &[f32]) -> Asset {
        Asset::new(name, mesh_from_studio(mesh, metres_per_unit), lod_ratios)
    }

    pub fn with_collider(mut self, c: ColliderShape) -> Asset {
        self.collider = c;
        self
    }
    pub fn with_cull(mut self, d: f32) -> Asset {
        self.cull_distance = d;
        self
    }
    pub fn with_sway(mut self, s: f32) -> Asset {
        self.sway = s;
        self
    }

    /// A box around the whole shape as its collider.
    pub fn boxed(mut self) -> Asset {
        self.collider = ColliderShape::Box(self.bounds);
        self
    }

    /// A cylinder around the lower part (a trunk).
    pub fn trunk(mut self, radius: f32, height: f32) -> Asset {
        self.collider = ColliderShape::Cylinder { radius, height };
        self
    }

    /// Its walls and floors as the collider (for rooms and structures).
    pub fn solid_mesh(mut self) -> Asset {
        let m = self.lods.last().unwrap();
        let tris: Vec<[V3; 3]> = (0..m.triangles()).map(|t| m.tri(t).map(|i| m.positions[i])).collect();
        self.collider = ColliderShape::Mesh(Arc::new(Bvh::new(tris)));
        self
    }

    /// Level of detail for a distance.
    pub fn lod_at(&self, dist: f32) -> usize {
        self.lod_starts.iter().filter(|s| dist >= **s).count()
    }

    /// Height of the whole shape.
    pub fn height(&self) -> f32 {
        self.bounds.max[1]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn studio_models_turn_up_and_keep_their_faces_out() {
        // A cube in studio space, a bit above its floor (y down: -y up).
        let mut cube = studio_mesh::prim::cube([2.0, 2.0, 2.0]);
        studio_mesh::ops::transform(&mut cube, &studio_mesh::region::Region::All, [0.0, -3.0, 0.0], [0.0; 3], [1.0; 3], [0.0; 3]);
        let m = mesh_from_studio(&cube, 1.0);
        let b = mesh_bounds(&m);
        assert!((b.min[1] - 2.0).abs() < 1e-4 && (b.max[1] - 4.0).abs() < 1e-4, "{b:?}");
        // Every face points away from the centre.
        let c = b.centre();
        for t in 0..m.triangles() {
            let [a, bb, cc] = m.tri(t).map(|i| m.positions[i]);
            let n = crate::math::cross(crate::math::sub(bb, a), crate::math::sub(cc, a));
            let mid = crate::math::mul(crate::math::add(crate::math::add(a, bb), cc), 1.0 / 3.0);
            assert!(crate::math::dot(n, crate::math::sub(mid, c)) > 0.0, "face {t} points in");
        }
    }

    #[test]
    fn coarser_levels_have_fewer_triangles_and_start_farther_out() {
        let s = studio_mesh::prim::uv_sphere(2.0, 32, 24);
        let a = Asset::from_studio("ball", &s, 1.0, &[0.4, 0.1]);
        assert_eq!(a.lods.len(), 3);
        assert!(a.lods[0].triangles() > a.lods[1].triangles() && a.lods[1].triangles() > a.lods[2].triangles());
        assert!(a.lod_starts[0] < a.lod_starts[1]);
        assert_eq!(a.lod_at(0.5), 0);
        assert_eq!(a.lod_at(1e4), 2);
    }
}
