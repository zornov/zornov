// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Whether a model was designed or only stacked: what a reviewer sees in a
//! hero object before looking at a render. How many separate pieces it has,
//! how many materials, whether any edge is a razor-sharp 90 degree corner
//! (nothing real has one; a small bevel is what catches the light), whether
//! the surface is still visibly faceted, whether its silhouette is the plain
//! round-or-boxy outline of a primitive in every view, and how closely a
//! front-on render matches a reference picture.
//!
//! These are measurements, not verdicts: the caller decides what a hero
//! object needs and says it in words.

use crate::raster::{View, frame, render};
use serde::Serialize;
use std::collections::HashMap;
use studio_mesh::{Mesh, dot};

/// Edges that bend more than this are razor-sharp corners (degrees).
pub const SHARP: f32 = 60.0;
/// Edges that bend between this and [`SHARP`] show as facets on a surface
/// meant to be smooth (degrees).
pub const FACET: f32 = 18.0;

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct Craft {
    pub triangles: usize,
    /// Separate connected pieces (a body and a lid are two).
    pub shells: usize,
    pub materials: usize,
    /// Materials still at the stock grey plastic, or unset.
    pub default_materials: usize,
    pub emissive_materials: usize,
    pub textured_materials: usize,
    pub bones: usize,
    /// Share of the edge length that bends more than [`SHARP`] degrees.
    pub sharp_share: f32,
    /// Share of the edge length that bends between [`FACET`] and [`SHARP`] and
    /// is shaded as a crease (smooth-shaded bevel steps do not show).
    pub facet_share: f32,
    /// How far the plainest outline is from a circle, worst view (1 = a
    /// circle, 1.3 a square; a designed object with parts and cut-outs is
    /// well above 1.6).
    pub outline: f32,
    /// Size of the box, largest side first.
    pub size: [f32; 3],
}

impl Craft {
    /// The outline of a primitive in every view.
    pub fn plain_outline(&self) -> bool {
        self.outline < 1.55
    }
}

fn is_default(m: &studio_mesh::Material) -> bool {
    let d = studio_mesh::Material::default();
    m.texture.is_none() && m.base_colour == d.base_colour && m.metallic == d.metallic && m.roughness == d.roughness && m.emissive == [0.0; 3] && m.subsurface == 0.0 && m.transmission == 0.0
}

fn find(parent: &mut [u32], mut i: u32) -> u32 {
    while parent[i as usize] != i {
        parent[i as usize] = parent[parent[i as usize] as usize];
        i = parent[i as usize];
    }
    i
}

/// Pieces, and the bend of every edge shared by two faces.
fn edges(m: &Mesh) -> (usize, Vec<(f32, f32, bool)>) {
    let (ids, unique) = m.position_ids();
    let mut parent: Vec<u32> = (0..unique.len() as u32).collect();
    let mut shared: HashMap<(u32, u32), [u32; 3]> = HashMap::new();
    let mut used = vec![false; unique.len()];
    for t in 0..m.triangles() {
        let c = m.tri(t).map(|v| ids[v]);
        for (a, b) in [(c[0], c[1]), (c[1], c[2]), (c[2], c[0])] {
            if a == b {
                continue;
            }
            used[a as usize] = true;
            used[b as usize] = true;
            let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
            if ra != rb {
                parent[ra as usize] = rb;
            }
            let e = shared.entry((a.min(b), a.max(b))).or_insert([0, u32::MAX, u32::MAX]);
            match e[0] {
                0 => e[1] = t as u32,
                1 => e[2] = t as u32,
                _ => {}
            }
            e[0] += 1;
        }
    }
    let shells = (0..unique.len() as u32).filter(|&i| used[i as usize] && find(&mut parent, i) == i).count();
    let mut bends = Vec::new();
    for (k, e) in &shared {
        if e[0] != 2 {
            continue;
        }
        let (a, b) = (m.face_normal(e[1] as usize), m.face_normal(e[2] as usize));
        if dot(a, a) < 0.5 || dot(b, b) < 0.5 {
            continue;
        }
        let len = studio_mesh::length(studio_mesh::sub(unique[k.0 as usize], unique[k.1 as usize]));
        let angle = dot(a, b).clamp(-1.0, 1.0).acos().to_degrees();
        // Shaded as a crease: the two faces' normals at the edge differ.
        let corner = |t: u32, id: u32| m.tri(t as usize).into_iter().find(|&v| ids[v] == id);
        let shaded_hard = if m.normals.len() == m.positions.len() {
            [k.0, k.1].iter().any(|&id| match (corner(e[1], id), corner(e[2], id)) {
                (Some(x), Some(y)) => {
                    let (na, nb) = (m.normals[x], m.normals[y]);
                    dot(na, na) > 0.25 && dot(nb, nb) > 0.25 && dot(na, nb) / (dot(na, na).sqrt() * dot(nb, nb).sqrt()) < 0.985
                }
                _ => angle >= 30.0,
            })
        } else {
            angle >= 30.0
        };
        bends.push((angle, len, shaded_hard));
    }
    (shells, bends)
}

/// Boundary pixels squared over four pi times the area: 1 for a disc.
fn roundness(mask: &[bool], size: usize) -> f32 {
    let area = mask.iter().filter(|&&b| b).count();
    if area < 16 {
        return 1.0;
    }
    let mut edge = 0usize;
    for y in 0..size {
        for x in 0..size {
            if !mask[y * size + x] {
                continue;
            }
            let out = x == 0 || y == 0 || x + 1 == size || y + 1 == size || !mask[y * size + x - 1] || !mask[y * size + x + 1] || !mask[(y - 1) * size + x] || !mask[(y + 1) * size + x];
            edge += usize::from(out);
        }
    }
    // A digital circle has about 1.27 times as many edge pixels as its
    // perimeter in pixels (4-neighbour test): taken out so a disc reads 1.
    let p = edge as f32 / 1.27;
    p * p / (4.0 * std::f32::consts::PI * area as f32)
}

/// What a model is made of.
pub fn measure(m: &Mesh) -> Craft {
    let mut c = Craft { triangles: m.triangles(), materials: m.materials.len(), ..Default::default() };
    c.default_materials = m.materials.iter().filter(|x| is_default(x)).count();
    c.emissive_materials = m.materials.iter().filter(|x| x.emissive.iter().any(|&e| e > 0.01)).count();
    c.textured_materials = m.materials.iter().filter(|x| x.texture.is_some()).count();
    c.bones = m.rig.as_ref().map_or(0, |r| r.bones.len());
    if m.triangles() == 0 {
        return c;
    }
    let (lo, hi) = m.bounds();
    let mut size = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
    size.sort_by(|a, b| b.total_cmp(a));
    c.size = size;
    let (shells, bends) = edges(m);
    c.shells = shells;
    if !bends.is_empty() {
        // By length: the tiny patches where bevels meet do not count as razors.
        let total: f32 = bends.iter().map(|b| b.1).sum::<f32>().max(1e-6);
        c.sharp_share = bends.iter().filter(|b| b.0 >= SHARP).map(|b| b.1).sum::<f32>() / total;
        c.facet_share = bends.iter().filter(|b| (FACET..SHARP).contains(&b.0) && b.2).map(|b| b.1).sum::<f32>() / total;
    }
    let (centre, radius) = frame(m);
    c.outline = [View { yaw: 0.0, pitch: 0.0 }, View { yaw: 90.0, pitch: 0.0 }, View { yaw: 0.0, pitch: 80.0 }].iter().map(|&v| roundness(&render(m, v, 96, centre, radius).mask, 96)).fold(0.0, f32::max);
    c
}

const FIT: usize = 64;

/// A mask cut to its box and scaled to fill a square, so shapes compare
/// whatever their size and place.
fn fitted(mask: &[bool], w: usize, h: usize) -> Option<Vec<bool>> {
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for y in 0..h {
        for x in 0..w {
            if mask[y * w + x] {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let (bw, bh) = ((x1 - x0) as f32, (y1 - y0) as f32);
    let s = (FIT as f32 - 2.0) / bw.max(bh);
    let (ox, oy) = ((FIT as f32 - bw * s) / 2.0, (FIT as f32 - bh * s) / 2.0);
    let mut out = vec![false; FIT * FIT];
    for ty in 0..FIT {
        for tx in 0..FIT {
            let sx = (tx as f32 - ox) / s;
            let sy = (ty as f32 - oy) / s;
            if sx < 0.0 || sy < 0.0 || sx >= bw || sy >= bh {
                continue;
            }
            out[ty * FIT + tx] = mask[(y0 + sy as usize) * w + x0 + sx as usize];
        }
    }
    Some(out)
}

/// What a picture of one object on a plain or transparent background
/// shows of it: the pixels unlike the background.
pub fn picture_mask(w: usize, h: usize, rgba: &[u8]) -> Option<Vec<bool>> {
    if w < 8 || h < 8 || rgba.len() < w * h * 4 {
        return None;
    }
    let see_through = rgba.chunks_exact(4).any(|p| p[3] < 200);
    let mask: Vec<bool> = if see_through {
        rgba.chunks_exact(4).map(|p| p[3] >= 128).collect()
    } else {
        // The background is the colour round the edge.
        let mut ring: Vec<[u8; 3]> = Vec::new();
        for x in 0..w {
            for y in [0, h - 1] {
                ring.push([rgba[(y * w + x) * 4], rgba[(y * w + x) * 4 + 1], rgba[(y * w + x) * 4 + 2]]);
            }
        }
        for y in 0..h {
            for x in [0, w - 1] {
                ring.push([rgba[(y * w + x) * 4], rgba[(y * w + x) * 4 + 1], rgba[(y * w + x) * 4 + 2]]);
            }
        }
        let mid = |k: usize| {
            let mut v: Vec<u8> = ring.iter().map(|c| c[k]).collect();
            v.sort_unstable();
            v[v.len() / 2] as i32
        };
        let bg = [mid(0), mid(1), mid(2)];
        rgba.chunks_exact(4).map(|p| (p[0] as i32 - bg[0]).abs() + (p[1] as i32 - bg[1]).abs() + (p[2] as i32 - bg[2]).abs() > 90).collect()
    };
    let share = mask.iter().filter(|&&b| b).count() as f32 / mask.len() as f32;
    (0.01..0.92).contains(&share).then_some(mask)
}

/// How well the model's outline matches a reference picture's, 0..1: the
/// best overlap (intersection over union) over views round it, both cut to
/// their boxes. `None` when the picture has no clear object on a plain
/// background to compare with.
pub fn silhouette_match(m: &Mesh, w: usize, h: usize, rgba: &[u8]) -> Option<f32> {
    if m.triangles() == 0 {
        return Some(0.0);
    }
    let want = fitted(&picture_mask(w, h, rgba)?, w, h)?;
    let (centre, radius) = frame(m);
    let mut best = 0.0f32;
    for pitch in [0.0, 25.0] {
        for k in 0..8 {
            let p = render(m, View { yaw: k as f32 * 45.0, pitch }, 128, centre, radius);
            let Some(got) = fitted(&p.mask, 128, 128) else { continue };
            let (mut i, mut u) = (0usize, 0usize);
            for (a, b) in got.iter().zip(&want) {
                i += usize::from(*a && *b);
                u += usize::from(*a || *b);
            }
            if u > 0 {
                best = best.max(i as f32 / u as f32);
            }
        }
    }
    Some(best)
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_mesh::prim;

    #[test]
    fn a_cube_is_all_razor_edges_and_a_sphere_is_one_plain_piece() {
        let cube = measure(&prim::by_name("cube", 200.0, 0).unwrap());
        assert!(cube.sharp_share > 0.5, "{cube:?}");
        assert_eq!(cube.shells, 1);
        let ball = measure(&prim::by_name("sphere", 200.0, 4).unwrap());
        assert_eq!(ball.sharp_share, 0.0, "{ball:?}");
        assert!(ball.plain_outline(), "{ball:?}");
        assert!(cube.plain_outline(), "{cube:?}");
        assert_eq!(ball.default_materials, 1);
    }

    #[test]
    fn a_model_matches_a_picture_of_itself_and_not_of_a_cube() {
        let ball = prim::by_name("sphere", 200.0, 4).unwrap();
        let (c, r) = frame(&ball);
        let pic = render(&ball, View { yaw: 0.0, pitch: 0.0 }, 96, c, r);
        let mut rgba = vec![255u8; 96 * 96 * 4];
        for (i, on) in pic.mask.iter().enumerate() {
            if *on {
                rgba[i * 4..i * 4 + 3].copy_from_slice(&[20, 20, 20]);
            }
        }
        let same = silhouette_match(&ball, 96, 96, &rgba).unwrap();
        assert!(same > 0.9, "{same}");
        let torus = prim::by_name("torus", 200.0, 4).unwrap();
        let other = silhouette_match(&torus, 96, 96, &rgba).unwrap();
        assert!(other < same - 0.2, "{other} vs {same}");
        assert_eq!(silhouette_match(&ball, 96, 96, &vec![255u8; 96 * 96 * 4]), None, "a blank picture has nothing to compare");
    }

    #[test]
    fn facets_count_only_where_the_shading_shows_them() {
        let mut low = prim::by_name("sphere", 200.0, 1).unwrap();
        studio_mesh::ops::recompute_normals(&mut low, 1.0);
        let flat = measure(&low);
        studio_mesh::ops::recompute_normals(&mut low, 80.0);
        let smooth = measure(&low);
        assert!(flat.facet_share > 0.3, "{flat:?}");
        assert!(smooth.facet_share < 0.1, "{smooth:?}");
    }
}
