// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Whether a model's materials are physically based and ready to be lit
//! in any shot: base colours inside what real surfaces reflect (no coal
//! blacker than coal, no white whiter than snow), metal or not metal
//! rather than in between, no light painted into the colour (a texture
//! brighter on the faces that look up than on those that look down has
//! the generator's light baked in, and looks wrong under any other), and
//! enough texture detail for the size it is seen at.

use crate::geo::{sample_surface, surface_area};
use serde::Serialize;
use studio_mesh::{Mesh, dot, normalise};

#[derive(Clone, Debug, Default, Serialize)]
pub struct Materials {
    pub materials: usize,
    pub textured: usize,
    /// Share of the surface whose base colour is outside the physical
    /// range (sRGB about 30–243 for non-metals).
    pub albedo_out_of_range: f32,
    /// Share of the surface with a metallic value between 0.1 and 0.9.
    pub half_metal: f32,
    /// Correlation between texture brightness and facing up (0 none, 1 all
    /// light baked in).
    pub baked_light: f32,
    /// Texture pixels per pixel of surface size (1 = a texel per pixel at
    /// the model's own size).
    pub texel_density: f32,
    /// Largest texture side.
    pub texture_size: u32,
    /// Every material named (not "Default", not empty).
    pub named: bool,
}

fn luminance(c: [f32; 4]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

pub fn analyse(m: &Mesh) -> Materials {
    let mut r = Materials { materials: m.materials.len(), ..Default::default() };
    r.textured = m.materials.iter().filter(|x| x.texture.is_some()).count();
    r.texture_size = m.materials.iter().filter_map(|x| x.texture.as_ref()).map(|t| t.width.max(t.height)).max().unwrap_or(0);
    r.named = !m.materials.is_empty() && m.materials.iter().all(|x| !x.name.trim().is_empty() && x.name != "Default");
    let n = m.triangles();
    if n == 0 {
        return r;
    }
    let mats = m.tri_materials();
    let samples = sample_surface(m, 4000, 0x9e37_79b9);
    // Linear reflectance bounds: sRGB 30 ≈ 0.012, 243 ≈ 0.896.
    let (lo, hi) = (0.012f32, 0.90f32);
    let mut out = 0usize;
    let mut half = 0usize;
    let mut ys = Vec::new();
    let mut ls = Vec::new();
    for &(p, t) in &samples {
        let mat = &m.materials[(mats[t] as usize).min(m.materials.len() - 1)];
        let mut c = mat.base_colour;
        if let Some(tex) = &mat.texture {
            let [a, b, cc] = m.tri(t);
            // Barycentric of p in its triangle, for the texture coordinate.
            let (pa, pb, pc) = (m.positions[a], m.positions[b], m.positions[cc]);
            let (_, w) = crate::geo::closest_on_triangle(p, pa, pb, pc);
            let uv = [0, 1].map(|k| m.uvs[a][k] * w[0] + m.uvs[b][k] * w[1] + m.uvs[cc][k] * w[2]);
            let s = tex.sample(uv);
            c = [c[0] * s[0], c[1] * s[1], c[2] * s[2], c[3] * s[3]];
            // Up is -y in Studio's world.
            let nrm = normalise(m.face_normal(t));
            ys.push(-dot(nrm, [0.0, 1.0, 0.0]));
            ls.push(luminance(c));
        }
        if mat.metallic < 0.5 {
            let l = luminance(c);
            if l < lo || c[0].max(c[1]).max(c[2]) > hi {
                out += 1;
            }
        }
        if mat.metallic > 0.1 && mat.metallic < 0.9 {
            half += 1;
        }
    }
    r.albedo_out_of_range = out as f32 / samples.len().max(1) as f32;
    r.half_metal = half as f32 / samples.len().max(1) as f32;
    if ys.len() > 50 {
        let k = ys.len() as f32;
        let (my, ml) = (ys.iter().sum::<f32>() / k, ls.iter().sum::<f32>() / k);
        let cov: f32 = ys.iter().zip(&ls).map(|(y, l)| (y - my) * (l - ml)).sum::<f32>() / k;
        let sy = (ys.iter().map(|y| (y - my) * (y - my)).sum::<f32>() / k).sqrt();
        let sl = (ls.iter().map(|l| (l - ml) * (l - ml)).sum::<f32>() / k).sqrt();
        r.baked_light = if sy > 1e-6 && sl > 1e-6 { (cov / (sy * sl)).max(0.0) } else { 0.0 };
    }
    if r.textured > 0 {
        // Texels over the textured surface: texture area × used share vs surface area.
        let area = surface_area(m).max(1e-6);
        let tex_px: f32 = m.materials.iter().filter_map(|x| x.texture.as_ref()).map(|t| (t.width * t.height) as f32).sum();
        let used = crate::uv::analyse(m).packing.max(0.05);
        r.texel_density = (tex_px * used / area).sqrt();
    }
    r
}
