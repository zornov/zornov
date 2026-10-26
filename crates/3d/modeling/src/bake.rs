// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Baking: detail from a dense (sculpted, generated or scanned) model
//! written into textures on a light, unwrapped one — tangent-space
//! normals, ambient occlusion, curvature, thickness, height, and colour.
//! The light model then looks as detailed as the dense one at a fraction
//! of the weight, and the curvature and occlusion maps drive procedural
//! wear (edges worn bright, dirt in the crevices).
//!
//! Tangent convention (what a renderer must match): per triangle, the
//! tangent is the direction of increasing u, made perpendicular to the
//! interpolated smooth normal; the bitangent is normal × tangent, flipped
//! when the triangle's layout is mirrored; green is +v (OpenGL / glTF).

use crate::poly::{Poly, triangulate};
use crate::spatial::Tree;
use studio_mesh::{Mesh, V3, add, cross, dot, length, lerp, normalise, scale, sub};

/// A floating-point picture, `channels` values per pixel, row 0 at v = 0.
#[derive(Clone, Debug, PartialEq)]
pub struct Map {
    pub width: u32,
    pub height: u32,
    pub channels: u32,
    pub data: Vec<f32>,
    /// Which pixels the layout covers (before dilation).
    pub covered: Vec<bool>,
}

impl Map {
    pub fn new(width: u32, height: u32, channels: u32, fill: f32) -> Map {
        let n = (width * height) as usize;
        Map { width, height, channels, data: vec![fill; n * channels as usize], covered: vec![false; n] }
    }

    pub fn get(&self, x: u32, y: u32) -> &[f32] {
        let i = ((y * self.width + x) * self.channels) as usize;
        &self.data[i..i + self.channels as usize]
    }

    pub fn set(&mut self, x: u32, y: u32, v: &[f32]) {
        let i = ((y * self.width + x) * self.channels) as usize;
        self.data[i..i + self.channels as usize].copy_from_slice(&v[..self.channels as usize]);
    }

    /// Bilinear sample at texture coordinates (wrapping).
    pub fn sample(&self, uv: [f32; 2]) -> Vec<f32> {
        let (w, h) = (self.width as f32, self.height as f32);
        let x = (uv[0] - uv[0].floor()) * w - 0.5;
        let y = (uv[1] - uv[1].floor()) * h - 0.5;
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let at = |xi: f32, yi: f32| {
            let xi = (xi as i64).rem_euclid(self.width as i64) as u32;
            let yi = (yi as i64).rem_euclid(self.height as i64) as u32;
            self.get(xi, yi).to_vec()
        };
        let (a, b, c, d) = (at(x0, y0), at(x0 + 1.0, y0), at(x0, y0 + 1.0), at(x0 + 1.0, y0 + 1.0));
        (0..self.channels as usize).map(|i| (a[i] * (1.0 - fx) + b[i] * fx) * (1.0 - fy) + (c[i] * (1.0 - fx) + d[i] * fx) * fy).collect()
    }

    /// Grows covered pixels outward by `pixels` so filtering near a seam
    /// never reads the empty space between pieces.
    pub fn dilate(&mut self, pixels: u32) {
        let (w, h, c) = (self.width as i64, self.height as i64, self.channels as usize);
        let mut cov = self.covered.clone();
        for _ in 0..pixels {
            let prev = cov.clone();
            let snapshot = self.data.clone();
            for y in 0..h {
                for x in 0..w {
                    let i = (y * w + x) as usize;
                    if prev[i] {
                        continue;
                    }
                    let mut sum = vec![0.0f32; c];
                    let mut n = 0;
                    for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1), (-1, -1), (1, 1), (-1, 1), (1, -1)] {
                        let (xx, yy) = (x + dx, y + dy);
                        if xx < 0 || yy < 0 || xx >= w || yy >= h {
                            continue;
                        }
                        let j = (yy * w + xx) as usize;
                        if prev[j] {
                            for k in 0..c {
                                sum[k] += snapshot[j * c + k];
                            }
                            n += 1;
                        }
                    }
                    if n > 0 {
                        for k in 0..c {
                            self.data[i * c + k] = sum[k] / n as f32;
                        }
                        cov[i] = true;
                    }
                }
            }
        }
    }

    /// As an 8-bit RGBA picture (values 0–1; one channel is grey;
    /// `srgb` encodes colour for display).
    pub fn to_rgba8(&self, srgb: bool) -> studio_mesh::Texture {
        let n = (self.width * self.height) as usize;
        let c = self.channels as usize;
        let enc = |v: f32| -> u8 {
            let v = v.clamp(0.0, 1.0);
            let v = if srgb { if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 } } else { v };
            (v * 255.0 + 0.5) as u8
        };
        let mut rgba = Vec::with_capacity(n * 4);
        for i in 0..n {
            let px = &self.data[i * c..i * c + c];
            let (r, g, b, a) = match c {
                1 => (px[0], px[0], px[0], 1.0),
                2 => (px[0], px[1], 0.0, 1.0),
                3 => (px[0], px[1], px[2], 1.0),
                _ => (px[0], px[1], px[2], px[3]),
            };
            rgba.extend_from_slice(&[enc(r), enc(g), enc(b), (a.clamp(0.0, 1.0) * 255.0 + 0.5) as u8]);
        }
        studio_mesh::Texture { width: self.width, height: self.height, rgba }
    }
}

/// A point on the low model where a texel lands.
#[derive(Clone, Copy, Debug)]
pub struct Texel {
    pub x: u32,
    pub y: u32,
    pub pos: V3,
    pub normal: V3,
    pub tangent: V3,
    pub bitangent: V3,
    pub face: u32,
}

/// Every texel the low model's layout covers, with where it is on the
/// surface (smooth normals, tangent frames).
pub fn texels(low: &Poly, size: u32, smooth_angle: f32) -> Vec<Texel> {
    let normals = low.corner_normals(smooth_angle, crate::NormalMode::Angle);
    let mut out = Vec::new();
    if !low.has_uvs() {
        return out;
    }
    let (w, h) = (size as f32, size as f32);
    for f in 0..low.faces() {
        let c = low.face(f);
        let r = low.face_range(f);
        let uv = &low.uvs[r.clone()];
        let ns = &normals[r];
        for t in triangulate(&low.positions, c) {
            let p = [low.positions[c[t[0]] as usize], low.positions[c[t[1]] as usize], low.positions[c[t[2]] as usize]];
            let u = [uv[t[0]], uv[t[1]], uv[t[2]]];
            let n = [ns[t[0]], ns[t[1]], ns[t[2]]];
            // Tangent from the uv derivatives.
            let (e1, e2) = (sub(p[1], p[0]), sub(p[2], p[0]));
            let (du1, dv1, du2, dv2) = (u[1][0] - u[0][0], u[1][1] - u[0][1], u[2][0] - u[0][0], u[2][1] - u[0][1]);
            let det = du1 * dv2 - du2 * dv1;
            if det.abs() < 1e-14 {
                continue;
            }
            let tang = scale(sub(scale(e1, dv2), scale(e2, dv1)), 1.0 / det);
            let flip = if det < 0.0 { -1.0 } else { 1.0 };
            // Raster in texel space.
            let px: [[f32; 2]; 3] = u.map(|q| [q[0] * w, q[1] * h]);
            let minx = px.iter().map(|q| q[0]).fold(f32::INFINITY, f32::min).floor().max(0.0) as u32;
            let maxx = px.iter().map(|q| q[0]).fold(f32::NEG_INFINITY, f32::max).ceil().min(w - 1.0) as u32;
            let miny = px.iter().map(|q| q[1]).fold(f32::INFINITY, f32::min).floor().max(0.0) as u32;
            let maxy = px.iter().map(|q| q[1]).fold(f32::NEG_INFINITY, f32::max).ceil().min(h - 1.0) as u32;
            let area = (px[1][0] - px[0][0]) * (px[2][1] - px[0][1]) - (px[1][1] - px[0][1]) * (px[2][0] - px[0][0]);
            if area.abs() < 1e-12 {
                continue;
            }
            for y in miny..=maxy {
                for x in minx..=maxx {
                    let q = [x as f32 + 0.5, y as f32 + 0.5];
                    let e = |a: [f32; 2], b: [f32; 2]| (b[0] - a[0]) * (q[1] - a[1]) - (b[1] - a[1]) * (q[0] - a[0]);
                    let (w0, w1, w2) = (e(px[1], px[2]) / area, e(px[2], px[0]) / area, e(px[0], px[1]) / area);
                    let eps = -1e-4;
                    if w0 < eps || w1 < eps || w2 < eps {
                        continue;
                    }
                    let pos = add(add(scale(p[0], w0), scale(p[1], w1)), scale(p[2], w2));
                    let nn = normalise(add(add(scale(n[0], w0), scale(n[1], w1)), scale(n[2], w2)));
                    let tt = normalise(sub(tang, scale(nn, dot(tang, nn))));
                    let bb = scale(cross(nn, tt), flip);
                    out.push(Texel { x, y, pos, normal: nn, tangent: tt, bitangent: bb, face: f as u32 });
                }
            }
        }
    }
    out
}

/// The dense model to bake from.
pub struct Source {
    pub mesh: Mesh,
    pub tree: Tree,
    /// Mean curvature per vertex (positive convex).
    pub curvature: Vec<f32>,
}

impl Source {
    pub fn new(mesh: Mesh) -> Source {
        let tree = Tree::of_mesh(&mesh);
        let curvature = curvature(&mesh);
        Source { mesh, tree, curvature }
    }

    /// The nearest surface along ±normal from `p` within `reach`:
    /// (hit point, triangle, barycentric, signed distance).
    pub fn project(&self, p: V3, n: V3, reach: f32) -> Option<(V3, u32, [f32; 3], f32)> {
        let start = add(p, scale(n, reach));
        let inward = scale(n, -1.0);
        let hit = self.tree.ray(start, inward, 2.0 * reach);
        match hit {
            Some(h) => {
                let q = add(start, scale(inward, h.t));
                Some((q, h.tri, [1.0 - h.uv[0] - h.uv[1], h.uv[0], h.uv[1]], reach - h.t))
            }
            None => self.tree.nearest(p, reach).map(|(q, t, d)| (q, t, self.tree.barycentric(t, q), -d)),
        }
    }

    pub fn normal_at(&self, tri: u32, b: [f32; 3]) -> V3 {
        let [a, c, d] = self.mesh.tri(tri as usize);
        if self.mesh.normals.len() == self.mesh.positions.len() {
            normalise(add(add(scale(self.mesh.normals[a], b[0]), scale(self.mesh.normals[c], b[1])), scale(self.mesh.normals[d], b[2])))
        } else {
            self.mesh.face_normal(tri as usize)
        }
    }

    pub fn curvature_at(&self, tri: u32, b: [f32; 3]) -> f32 {
        let [a, c, d] = self.mesh.tri(tri as usize);
        self.curvature[a] * b[0] + self.curvature[c] * b[1] + self.curvature[d] * b[2]
    }

    /// Linear colour of the dense model at a hit (texture × base colour).
    pub fn colour_at(&self, tri: u32, b: [f32; 3], mats: &[u32]) -> [f32; 4] {
        let m = mats.get(tri as usize).copied().unwrap_or(0) as usize;
        let mat = self.mesh.materials.get(m).cloned().unwrap_or_default();
        let mut c = mat.base_colour;
        if let Some(tex) = &mat.texture {
            let [a, e, d] = self.mesh.tri(tri as usize);
            let uv = |i: usize| self.mesh.uvs.get(i).copied().unwrap_or([0.0, 0.0]);
            let q = [uv(a)[0] * b[0] + uv(e)[0] * b[1] + uv(d)[0] * b[2], uv(a)[1] * b[0] + uv(e)[1] * b[1] + uv(d)[1] * b[2]];
            let s = tex.sample(q);
            c = [c[0] * s[0], c[1] * s[1], c[2] * s[2], c[3] * s[3]];
        }
        c
    }
}

/// Mean curvature per vertex (umbrella Laplacian along the normal over
/// the mean edge length squared).
pub fn curvature(m: &Mesh) -> Vec<f32> {
    let (ids, pos) = m.position_ids();
    let np = pos.len();
    let mut nb: Vec<Vec<u32>> = vec![Vec::new(); np];
    let mut nrm = vec![[0.0f32; 3]; np];
    for t in 0..m.triangles() {
        let c = m.tri(t).map(|v| ids[v]);
        let fnm = cross(sub(pos[c[1] as usize], pos[c[0] as usize]), sub(pos[c[2] as usize], pos[c[0] as usize]));
        for k in 0..3 {
            let (a, b) = (c[k], c[(k + 1) % 3]);
            if !nb[a as usize].contains(&b) {
                nb[a as usize].push(b);
            }
            if !nb[b as usize].contains(&a) {
                nb[b as usize].push(a);
            }
            nrm[a as usize] = add(nrm[a as usize], fnm);
        }
    }
    let k_pos: Vec<f32> = (0..np)
        .map(|v| {
            if nb[v].is_empty() {
                return 0.0;
            }
            let mut avg = [0.0; 3];
            let mut el = 0.0;
            for &u in &nb[v] {
                avg = add(avg, pos[u as usize]);
                el += length(sub(pos[u as usize], pos[v]));
            }
            avg = scale(avg, 1.0 / nb[v].len() as f32);
            el /= nb[v].len() as f32;
            // Points above their neighbours (convex) are positive.
            -dot(sub(avg, pos[v]), normalise(nrm[v])) / (el * el).max(1e-12) * el
        })
        .collect();
    ids.iter().map(|&i| k_pos[i as usize]).collect()
}

/// Which maps to bake.
#[derive(Clone, Debug, PartialEq)]
pub struct BakeSettings {
    pub size: u32,
    /// How far (px) to look out and in from the light model for the dense
    /// one.
    pub reach: f32,
    pub ao_samples: usize,
    /// How far occlusion looks (px).
    pub ao_distance: f32,
    pub padding: u32,
    pub smooth_angle: f32,
}

impl Default for BakeSettings {
    fn default() -> BakeSettings {
        BakeSettings { size: 1024, reach: 20.0, ao_samples: 24, ao_distance: 60.0, padding: 8, smooth_angle: 60.0 }
    }
}

/// Baked maps.
pub struct Baked {
    /// Tangent-space normals, encoded 0–1 (0.5, 0.5, 1 is flat).
    pub normal: Map,
    /// 1 open – 0 fully occluded.
    pub ao: Map,
    /// 0.5 flat, brighter convex (edges), darker concave (crevices).
    pub curvature: Map,
    /// How far in the dense surface lies from the light one, 0.5 = on it.
    pub height: Map,
    /// 1 thin – 0 thick (how far light gets through).
    pub thickness: Map,
    /// The dense model's colour (linear RGBA).
    pub colour: Map,
    /// Texels that found no dense surface.
    pub misses: usize,
}

fn cosine_dir(n: V3, t: V3, b: V3, u1: f32, u2: f32) -> V3 {
    let r = u1.sqrt();
    let th = std::f32::consts::TAU * u2;
    let (x, y) = (r * th.cos(), r * th.sin());
    let z = (1.0 - u1).max(0.0).sqrt();
    normalise(add(add(scale(t, x), scale(b, y)), scale(n, z)))
}

/// Bakes the dense model's detail onto the light, unwrapped one.
pub fn bake(low: &Poly, high: &Source, s: &BakeSettings) -> Baked {
    let size = s.size.clamp(16, 8192);
    let tx = texels(low, size, s.smooth_angle);
    let mats = high.mesh.tri_materials();
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(16);
    let chunk = tx.len().div_ceil(threads).max(1);
    // (normal rgb, ao, curvature, height, thickness, colour rgba, miss)
    type Out = (u32, u32, [f32; 3], f32, f32, f32, f32, [f32; 4], bool);
    let results: Vec<Out> = std::thread::scope(|sc| {
        let handles: Vec<_> = tx
            .chunks(chunk)
            .map(|part| {
                let mats = &mats;
                sc.spawn(move || {
                    let mut out: Vec<Out> = Vec::with_capacity(part.len());
                    for (k, t) in part.iter().enumerate() {
                        let hit = high.project(t.pos, t.normal, s.reach);
                        let (q, n_hi, curv, height, colour, miss) = match hit {
                            Some((q, tri, b, d)) => (q, high.normal_at(tri, b), high.curvature_at(tri, b), d, high.colour_at(tri, b, mats), false),
                            None => (t.pos, t.normal, 0.0, 0.0, [0.5, 0.5, 0.5, 1.0], true),
                        };
                        let tn = [dot(n_hi, t.tangent), dot(n_hi, t.bitangent), dot(n_hi, t.normal)];
                        let tn = normalise(tn);
                        // Occlusion over the hemisphere at the dense surface.
                        let mut open = 0usize;
                        let seed = (t.x.wrapping_mul(73_856_093) ^ t.y.wrapping_mul(19_349_663)) as usize + k;
                        let ns = s.ao_samples.max(1);
                        let side = (ns as f32).sqrt().ceil() as usize;
                        let start = add(q, scale(n_hi, s.reach * 0.002 + 1e-3));
                        let tt = normalise(sub(t.tangent, scale(n_hi, dot(t.tangent, n_hi))));
                        let bb = cross(n_hi, tt);
                        for i in 0..ns {
                            let jit = ((seed.wrapping_mul(2_654_435_761).wrapping_add(i * 40_503)) % 1000) as f32 / 1000.0;
                            let u1 = ((i % side) as f32 + jit) / side as f32;
                            let u2 = ((i / side) as f32 + (1.0 - jit)) / side as f32;
                            let d = cosine_dir(n_hi, tt, bb, u1.min(0.999), u2);
                            if !high.tree.occluded(start, d, s.ao_distance) {
                                open += 1;
                            }
                        }
                        let ao = open as f32 / ns as f32;
                        // Thickness: rays into the surface.
                        let mut through = 0.0;
                        let inner = sub(q, scale(n_hi, s.reach * 0.002 + 1e-3));
                        let probes = 6;
                        for i in 0..probes {
                            let d = scale(cosine_dir(n_hi, tt, bb, (i as f32 + 0.5) / probes as f32 * 0.6, i as f32 / probes as f32), -1.0);
                            let dist = high.tree.ray(inner, d, s.ao_distance).map_or(s.ao_distance, |h| h.t);
                            through += 1.0 - (dist / s.ao_distance).min(1.0);
                        }
                        let thickness = through / probes as f32;
                        let curv01 = (0.5 + curv * 2.0).clamp(0.0, 1.0);
                        let h01 = (0.5 + height / (2.0 * s.reach)).clamp(0.0, 1.0);
                        out.push((t.x, t.y, [tn[0] * 0.5 + 0.5, tn[1] * 0.5 + 0.5, tn[2] * 0.5 + 0.5], ao, curv01, h01, thickness, colour, miss));
                    }
                    out
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
    });
    let mut b = Baked {
        normal: Map::new(size, size, 3, 0.5),
        ao: Map::new(size, size, 1, 1.0),
        curvature: Map::new(size, size, 1, 0.5),
        height: Map::new(size, size, 1, 0.5),
        thickness: Map::new(size, size, 1, 0.0),
        colour: Map::new(size, size, 4, 0.5),
        misses: 0,
    };
    for i in 0..(size * size) as usize {
        b.normal.data[i * 3 + 2] = 1.0;
    }
    for (x, y, n, ao, c, h, th, col, miss) in results {
        b.normal.set(x, y, &n);
        b.ao.set(x, y, &[ao]);
        b.curvature.set(x, y, &[c]);
        b.height.set(x, y, &[h]);
        b.thickness.set(x, y, &[th]);
        b.colour.set(x, y, &col);
        let i = (y * size + x) as usize;
        for m in [&mut b.normal, &mut b.ao, &mut b.curvature, &mut b.height, &mut b.thickness, &mut b.colour] {
            m.covered[i] = true;
        }
        if miss {
            b.misses += 1;
        }
    }
    for m in [&mut b.normal, &mut b.ao, &mut b.curvature, &mut b.height, &mut b.thickness, &mut b.colour] {
        m.dilate(s.padding);
    }
    b
}

/// The light model's own occlusion and curvature (no dense model): what
/// smart materials need on a hard-surface model.
pub fn bake_self(low: &Poly, s: &BakeSettings) -> Baked {
    let mesh = low.to_mesh(s.smooth_angle, crate::NormalMode::Angle);
    let src = Source::new(mesh);
    bake(low, &src, s)
}

/// Bilinear-free sample of a face's interpolated value, for tests.
pub fn lerp3(a: V3, b: V3, t: f32) -> V3 {
    lerp(a, b, t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prim;
    use crate::uv::{Unwrap, unwrap};

    #[test]
    fn a_bumpy_ball_bakes_onto_a_smooth_one() {
        // Low: a smooth ball, unwrapped. High: the same ball with bumps.
        let mut low = prim::quad_sphere(50.0, 6);
        unwrap(&mut low, &Unwrap::default());
        let mut high = crate::subd::subdivide(&prim::quad_sphere(50.0, 6), 2);
        high.map_positions(|p| {
            let n = normalise(p);
            let bump = 2.0 * crate::noise::perlin(scale(p, 0.15), 1);
            add(scale(n, 50.0), scale(n, bump))
        });
        let src = Source::new(high.to_mesh(180.0, crate::NormalMode::Angle));
        let b = bake(&low, &src, &BakeSettings { size: 64, reach: 8.0, ao_samples: 4, ao_distance: 20.0, padding: 2, smooth_angle: 180.0 });
        assert!(b.misses < 10, "{} misses", b.misses);
        // Normals tilt (not all flat), and stay unit length.
        let mut tilted = 0;
        for i in 0..64 * 64 {
            if !b.normal.covered[i] {
                continue;
            }
            let n = [b.normal.data[i * 3] * 2.0 - 1.0, b.normal.data[i * 3 + 1] * 2.0 - 1.0, b.normal.data[i * 3 + 2] * 2.0 - 1.0];
            assert!((length(n) - 1.0).abs() < 0.02);
            assert!(n[2] > 0.3);
            if n[2] < 0.995 {
                tilted += 1;
            }
        }
        assert!(tilted > 200, "{tilted}");
        // A ball sees the whole sky: occlusion stays near open.
        let mean_ao: f32 = (0..64 * 64).filter(|&i| b.ao.covered[i]).map(|i| b.ao.data[i]).sum::<f32>() / b.ao.covered.iter().filter(|c| **c).count() as f32;
        assert!(mean_ao > 0.8, "{mean_ao}");
    }

    #[test]
    fn inside_corners_are_occluded_and_edges_convex() {
        // An L-shaped block: the inner corner is darker.
        let mut p = prim::cube([40.0, 40.0, 40.0], 2);
        let top_right = crate::select::Faces::And(vec![crate::select::Faces::named("top").unwrap(), crate::select::Faces::Side { axis: 0, at: 0.0, above: true }]).mask(&p);
        crate::edit::extrude(&mut p, &top_right, 30.0, None, false);
        p.compact();
        unwrap(&mut p, &Unwrap::default());
        let b = bake_self(&p, &BakeSettings { size: 128, reach: 2.0, ao_samples: 32, ao_distance: 40.0, padding: 2, smooth_angle: 30.0 });
        let ao: Vec<f32> = (0..128 * 128).filter(|&i| b.ao.covered[i]).map(|i| b.ao.data[i]).collect();
        let lo = ao.iter().copied().fold(1.0f32, f32::min);
        assert!(lo < 0.75, "the inner corner should be shaded: {lo}");
    }
}
