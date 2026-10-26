// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Carrying a model's look onto its rebuilt surface: every texel of the
//! new texture is placed on the new surface, the nearest point of the
//! original found, and the original's texture read there. Texels round
//! each chart are grown outwards a few pixels so filtering and mip maps
//! never pull in the empty background at a seam.

use crate::geo::Nearest;
use studio_mesh::{Mesh, Texture};

fn to_srgb(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0 + 0.5) as u8
}

/// Bakes `src`'s textures onto `dst` (which has its own texture
/// coordinates) at `size`², one texture shared by `dst`'s textured
/// materials. Untextured originals are left as they are (their materials
/// already carry the colour, and stay editable). Returns whether it baked.
pub fn bake(src: &Mesh, dst: &mut Mesh, size: u32) -> bool {
    if !src.materials.iter().any(|m| m.texture.is_some()) || dst.uvs.len() != dst.positions.len() {
        return false;
    }
    let src_mats = src.tri_materials();
    let colour_at = |hit: &crate::geo::Hit| -> [u8; 4] {
        let mat = &src.materials[(src_mats.get(hit.triangle).copied().unwrap_or(0) as usize).min(src.materials.len() - 1)];
        let col = match &mat.texture {
            Some(tex) => {
                let [sa, sb, sc] = src.tri(hit.triangle);
                let suv = [0, 1].map(|k| src.uvs.get(sa).map_or(0.0, |u| u[k]) * hit.bary[0] + src.uvs.get(sb).map_or(0.0, |u| u[k]) * hit.bary[1] + src.uvs.get(sc).map_or(0.0, |u| u[k]) * hit.bary[2]);
                tex.sample(suv)
            }
            None => [1.0, 1.0, 1.0, 1.0],
        };
        [to_srgb(col[0]), to_srgb(col[1]), to_srgb(col[2]), (col[3].clamp(0.0, 1.0) * 255.0) as u8]
    };
    bake_core(src, dst, size, &colour_at, &|i| src.materials.get(i).is_some_and(|s| s.texture.is_some()))
}

/// Bakes colours given per vertex of `src` (sRGB 0..1, as a scan or an
/// image-to-3D result has them) into `dst`'s texture, `size`² texels. Every
/// material of `dst` that has faces gets the texture.
pub fn bake_colours(src: &Mesh, colours: &[[f32; 3]], dst: &mut Mesh, size: u32) -> bool {
    if colours.len() != src.positions.len() || dst.uvs.len() != dst.positions.len() {
        return false;
    }
    let colour_at = |hit: &crate::geo::Hit| -> [u8; 4] {
        let [a, b, c] = src.tri(hit.triangle);
        let px: [u8; 3] = std::array::from_fn(|k| ((colours[a][k] * hit.bary[0] + colours[b][k] * hit.bary[1] + colours[c][k] * hit.bary[2]).clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
        [px[0], px[1], px[2], 255]
    };
    bake_core(src, dst, size, &colour_at, &|_| true)
}

fn bake_core(src: &Mesh, dst: &mut Mesh, size: u32, colour_at: &dyn Fn(&crate::geo::Hit) -> [u8; 4], textured: &dyn Fn(usize) -> bool) -> bool {
    let near = Nearest::new(src);
    let s = size as usize;
    let mut rgba = vec![0u8; s * s * 4];
    let mut filled = vec![false; s * s];
    for t in 0..dst.triangles() {
        let [a, b, c] = dst.tri(t);
        let uv = [dst.uvs[a], dst.uvs[b], dst.uvs[c]].map(|u| [u[0] * s as f32, u[1] * s as f32]);
        let area = (uv[1][0] - uv[0][0]) * (uv[2][1] - uv[0][1]) - (uv[2][0] - uv[0][0]) * (uv[1][1] - uv[0][1]);
        if area.abs() < 1e-9 {
            continue;
        }
        let x0 = uv.iter().map(|p| p[0]).fold(f32::INFINITY, f32::min).floor().max(0.0) as usize;
        let x1 = (uv.iter().map(|p| p[0]).fold(f32::NEG_INFINITY, f32::max).ceil() as usize).min(s);
        let y0 = uv.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min).floor().max(0.0) as usize;
        let y1 = (uv.iter().map(|p| p[1]).fold(f32::NEG_INFINITY, f32::max).ceil() as usize).min(s);
        let (pa, pb, pc) = (dst.positions[a], dst.positions[b], dst.positions[c]);
        for y in y0..y1 {
            for x in x0..x1 {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let w0 = ((uv[2][0] - uv[1][0]) * (py - uv[1][1]) - (uv[2][1] - uv[1][1]) * (px - uv[1][0])) / area;
                let w1 = ((uv[0][0] - uv[2][0]) * (py - uv[2][1]) - (uv[0][1] - uv[2][1]) * (px - uv[2][0])) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < -0.01 || w1 < -0.01 || w2 < -0.01 {
                    continue;
                }
                let p: [f32; 3] = std::array::from_fn(|i| pa[i] * w0 + pb[i] * w1 + pc[i] * w2);
                let Some(hit) = near.find(p) else { continue };
                let px = colour_at(&hit);
                let i = y * s + x;
                rgba[i * 4..i * 4 + 4].copy_from_slice(&px);
                filled[i] = true;
            }
        }
    }
    // Grow the charts' edges outwards by 8 texels.
    for _ in 0..8 {
        let before = filled.clone();
        for y in 0..s {
            for x in 0..s {
                if before[y * s + x] {
                    continue;
                }
                let mut acc = [0u32; 4];
                let mut k = 0;
                for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    if nx < 0 || ny < 0 || nx >= s as i32 || ny >= s as i32 {
                        continue;
                    }
                    let j = ny as usize * s + nx as usize;
                    if before[j] {
                        for c in 0..4 {
                            acc[c] += rgba[j * 4 + c] as u32;
                        }
                        k += 1;
                    }
                }
                if k > 0 {
                    for c in 0..4 {
                        rgba[(y * s + x) * 4 + c] = (acc[c] / k) as u8;
                    }
                    filled[y * s + x] = true;
                }
            }
        }
    }
    let tex = Texture { width: size, height: size, rgba };
    let dst_mats = dst.tri_materials();
    let used: std::collections::HashSet<u32> = dst_mats.iter().copied().collect();
    for (i, m) in dst.materials.iter_mut().enumerate() {
        if used.contains(&(i as u32)) && textured(i) {
            m.texture = Some(tex.clone());
        }
    }
    true
}
