// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! How faithful a model is to what was asked for: its silhouettes against
//! a reference's from every standard view (the worst view says whether one
//! side was guessed wrong), how far its surface strays from the
//! reference's (chamfer and Hausdorff, as a share of the size), and
//! whether a model meant to be symmetric is.

use crate::geo::{Nearest, diagonal, sample_surface};
use crate::raster::{View, render, standard_views};
use serde::Serialize;
use studio_mesh::{Mesh, V3, length, sub};

#[derive(Clone, Debug, Default, Serialize)]
pub struct Fidelity {
    /// Silhouette overlap (intersection over union), mean over the views.
    pub silhouette_iou: f32,
    /// The worst view's overlap.
    pub worst_view_iou: f32,
    /// Mean distance between the surfaces both ways, as a share of the
    /// reference's diagonal.
    pub chamfer: f32,
    /// Largest distance (99th percentile), as a share of the diagonal.
    pub hausdorff: f32,
    /// Mean angle between the surface and the reference surface under it
    /// (degrees): torn rims, ripples and folds show here first.
    pub normal_error: f32,
    /// Share of the surface turned more than 30° from the reference.
    pub torn: f32,
}

fn iou(a: &[bool], b: &[bool]) -> f32 {
    let (mut i, mut u) = (0usize, 0usize);
    for (x, y) in a.iter().zip(b) {
        if *x && *y {
            i += 1;
        }
        if *x || *y {
            u += 1;
        }
    }
    if u == 0 { 1.0 } else { i as f32 / u as f32 }
}

fn one_way(from: &Mesh, to: &Nearest, n: usize) -> Vec<f32> {
    sample_surface(from, n, 0x51ed_270b).iter().filter_map(|(p, _)| to.find(*p)).map(|h| h.distance).collect()
}

/// `m` against `reference` (both in the same place and units).
pub fn fidelity(m: &Mesh, reference: &Mesh) -> Fidelity {
    let (c, r) = crate::raster::frame(reference);
    let size = 128;
    let mut ious = Vec::new();
    for v in standard_views() {
        let a = render(m, v, size, c, r);
        let b = render(reference, v, size, c, r);
        ious.push(iou(&a.mask, &b.mask));
    }
    let mut f = Fidelity { silhouette_iou: ious.iter().sum::<f32>() / ious.len() as f32, worst_view_iou: ious.iter().copied().fold(1.0, f32::min), ..Default::default() };
    let diag = diagonal(reference);
    let (nm, nr) = (Nearest::new(m), Nearest::new(reference));
    let mut d = one_way(m, &nr, 3000);
    let back = one_way(reference, &nm, 3000);
    let mean = (d.iter().sum::<f32>() / d.len().max(1) as f32 + back.iter().sum::<f32>() / back.len().max(1) as f32) * 0.5;
    d.extend(back);
    d.sort_by(f32::total_cmp);
    let p99 = d.get(((d.len() as f32) * 0.99) as usize).copied().unwrap_or(0.0);
    f.chamfer = mean / diag;
    f.hausdorff = p99 / diag;
    let (ne, torn) = normal_error(m, &nr);
    f.normal_error = ne;
    f.torn = torn;
    f
}

/// Mean angle (degrees) between `m`'s faces and the reference's faces
/// nearest them, and the share turned more than 30°.
pub fn normal_error(m: &Mesh, reference: &Nearest) -> (f32, f32) {
    let (mut sum, mut torn, mut n) = (0.0f64, 0usize, 0usize);
    for (p, t) in sample_surface(m, 3000, 0x2545_f491) {
        let Some(h) = reference.find(p) else { continue };
        let a = m.face_normal(t);
        let b = reference.mesh.face_normal(h.triangle);
        if studio_mesh::dot(a, a) < 0.5 || studio_mesh::dot(b, b) < 0.5 {
            continue;
        }
        let ang = studio_mesh::dot(a, b).clamp(-1.0, 1.0).acos().to_degrees();
        sum += ang as f64;
        torn += usize::from(ang > 30.0);
        n += 1;
    }
    ((sum / n.max(1) as f64) as f32, torn as f32 / n.max(1) as f32)
}

/// Left–right symmetry (about the model's own centre in x): the front
/// silhouette against its mirror image (1 = symmetric).
pub fn symmetry(m: &Mesh) -> f32 {
    let (c, r) = crate::raster::frame(m);
    let size = 128;
    let mut total = 0.0;
    let views = [View { yaw: 0.0, pitch: 0.0 }, View { yaw: 0.0, pitch: 60.0 }, View { yaw: 180.0, pitch: 0.0 }];
    for v in views {
        let a = render(m, v, size, c, r);
        let mirrored: Vec<bool> = (0..size * size).map(|i| a.mask[(i / size) * size + (size - 1 - i % size)]).collect();
        total += iou(&a.mask, &mirrored);
    }
    total / views.len() as f32
}

/// Distance of a point to a mesh, for tests and repair checks.
pub fn distance_to(m: &Mesh, p: V3) -> f32 {
    Nearest::new(m).find(p).map_or(f32::INFINITY, |h| length(sub(h.point, p)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_model_matches_itself_and_not_a_cube() {
        let s = studio_mesh::prim::by_name("sphere", 200.0, 4).unwrap();
        let f = fidelity(&s, &s);
        assert!(f.silhouette_iou > 0.99 && f.chamfer < 1e-4, "{f:?}");
        let c = studio_mesh::prim::by_name("cube", 200.0, 0).unwrap();
        let g = fidelity(&c, &s);
        assert!(g.silhouette_iou < 0.95 && g.chamfer > 0.01, "{g:?}");
        assert!(symmetry(&s) > 0.97);
    }
}
