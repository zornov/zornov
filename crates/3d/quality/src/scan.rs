// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Making a raw coloured surface (what an image-to-3D model or a scan
//! gives: a dense triangle surface with a colour per vertex) fit for the
//! production finish: loose specks dropped, the grid's stair-steps
//! smoothed away without shrinking the form, placed in Studio's frame at
//! the size asked for.

use std::collections::HashMap;
use studio_mesh::{Material, Mesh, V3};

pub struct Scan {
    pub mesh: Mesh,
    /// sRGB 0..1 per vertex of `mesh`.
    pub colours: Vec<[f32; 3]>,
    /// Pieces dropped because they were small and not joined to the main body.
    pub specks: usize,
}

/// `positions` has +Y up and the object facing +Z; the result is in
/// Studio's frame (Y down, the front toward the camera), centred, its
/// longest side `size`.
pub fn from_surface(positions: &[V3], colours: &[[f32; 3]], triangles: &[[u32; 3]], size: f32, smoothing: usize) -> Scan {
    let n = positions.len();
    // Connected pieces.
    let mut parent: Vec<u32> = (0..n as u32).collect();
    fn find(p: &mut [u32], mut x: u32) -> u32 {
        while p[x as usize] != x {
            p[x as usize] = p[p[x as usize] as usize];
            x = p[x as usize];
        }
        x
    }
    for t in triangles {
        let (a, b, c) = (find(&mut parent, t[0]), find(&mut parent, t[1]), find(&mut parent, t[2]));
        parent[b as usize] = a;
        let c = find(&mut parent, c);
        let a = find(&mut parent, a);
        parent[c as usize] = a;
    }
    let mut count: HashMap<u32, usize> = HashMap::new();
    for t in triangles {
        *count.entry(find(&mut parent, t[0])).or_default() += 1;
    }
    let biggest = count.values().copied().max().unwrap_or(0);
    // A piece survives when it is at least 6% of the main body.
    let keep: Vec<bool> = triangles.iter().map(|t| count[&find(&mut parent, t[0])] * 16 >= biggest).collect();
    let specks = count.values().filter(|&&c| c * 16 < biggest).count();
    let mut remap = vec![u32::MAX; n];
    let mut pos: Vec<V3> = Vec::new();
    let mut col: Vec<[f32; 3]> = Vec::new();
    let mut idx: Vec<u32> = Vec::new();
    for (t, k) in triangles.iter().zip(&keep) {
        if !*k {
            continue;
        }
        for &i in t {
            if remap[i as usize] == u32::MAX {
                remap[i as usize] = pos.len() as u32;
                pos.push(positions[i as usize]);
                col.push(colours.get(i as usize).copied().unwrap_or([0.5; 3]));
            }
            idx.push(remap[i as usize]);
        }
    }
    // Taubin smoothing: a shrinking step then an inflating one.
    let mut nbr: Vec<Vec<u32>> = vec![Vec::new(); pos.len()];
    for t in idx.chunks_exact(3) {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            nbr[a as usize].push(b);
            nbr[b as usize].push(a);
        }
    }
    for l in &mut nbr {
        l.sort_unstable();
        l.dedup();
    }
    for _ in 0..smoothing {
        for lambda in [0.5f32, -0.53] {
            let old = pos.clone();
            for (i, l) in nbr.iter().enumerate() {
                if l.is_empty() {
                    continue;
                }
                let mut avg = [0.0f32; 3];
                for &j in l {
                    for k in 0..3 {
                        avg[k] += old[j as usize][k];
                    }
                }
                for k in 0..3 {
                    pos[i][k] += lambda * (avg[k] / l.len() as f32 - old[i][k]);
                }
            }
        }
    }
    // Studio's frame: turn half a revolution about X (Y up becomes Y down,
    // +Z facing becomes -Z, toward the camera), centre, scale.
    for p in &mut pos {
        *p = [p[0], -p[1], -p[2]];
    }
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in &pos {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let ext = (0..3).map(|k| hi[k] - lo[k]).fold(1e-6, f32::max);
    let s = size / ext;
    for p in &mut pos {
        for k in 0..3 {
            p[k] = (p[k] - (lo[k] + hi[k]) * 0.5) * s;
        }
    }
    let mut mesh = Mesh { positions: pos, indices: idx, materials: vec![Material { name: "Surface".into(), roughness: 0.6, ..Default::default() }], ..Default::default() };
    mesh.groups = vec![studio_mesh::Group { start: 0, count: (mesh.indices.len() / 3) as u32, material: 0 }];
    Scan { mesh, colours: col, specks }
}

/// How to finish a raw surface.
#[derive(Clone, Copy, Debug)]
pub struct ModelOptions {
    /// Longest side of the result, model px.
    pub size: f32,
    /// Quads to aim for.
    pub quads: usize,
    /// Texture side.
    pub texture: u32,
    /// What kind of thing it is, for the critics' budgets (any: a prop by
    /// default).
    pub class: crate::score::Class,
    /// Smoothing rounds against the grid's stair-steps.
    pub smoothing: usize,
}

impl Default for ModelOptions {
    fn default() -> ModelOptions {
        ModelOptions { size: 100.0, quads: 10_000, texture: 1024, class: crate::score::Class::Prop, smoothing: 3 }
    }
}

/// The whole finish for a raw coloured surface: specks dropped, smoothed,
/// rebuilt as clean quads, unwrapped, the colours baked into a texture and
/// a PBR material set. Returns the model and what was done.
pub fn to_model(positions: &[V3], colours: &[[f32; 3]], triangles: &[[u32; 3]], o: &ModelOptions) -> (Mesh, Vec<String>) {
    let scan = from_surface(positions, colours, triangles, o.size, o.smoothing);
    let mut notes = vec![format!("{} triangles from the model", scan.mesh.triangles())];
    if scan.specks > 0 {
        notes.push(format!("{} loose pieces removed", scan.specks));
    }
    let opts = crate::production::Options { quads: Some(o.quads), texture: o.texture, candidates: 2, retries: 1, class: o.class };
    let done = crate::production::finish_with(&scan.mesh, Some(&scan.colours), None, &opts);
    notes.push(format!("clean-up: {}", done.report.summary()));
    let mut mesh = done.mesh;
    for m in &mut mesh.materials {
        if m.texture.is_some() {
            m.base_colour = [1.0, 1.0, 1.0, 1.0];
            m.metallic = 0.0;
            m.roughness = 0.6;
        }
    }
    (mesh, notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_coloured_sphere_surface_becomes_a_closed_unwrapped_textured_model_and_specks_go() {
        let ball = studio_mesh::prim::by_name("sphere", 1.0, 3).unwrap();
        let mut pos: Vec<V3> = ball.positions.clone();
        let mut tris: Vec<[u32; 3]> = ball.indices.chunks_exact(3).map(|t| [t[0], t[1], t[2]]).collect();
        // A loose speck far away.
        let base = pos.len() as u32;
        pos.extend([[5.0, 0.0, 0.0], [5.1, 0.0, 0.0], [5.0, 0.1, 0.0]]);
        tris.push([base, base + 1, base + 2]);
        let colours: Vec<[f32; 3]> = pos.iter().map(|p| [0.5 + 0.4 * p[0].clamp(-1.0, 1.0) * 0.5, 0.3, 0.2]).collect();
        let (mesh, notes) = to_model(&pos, &colours, &tris, &ModelOptions { size: 100.0, quads: 600, texture: 128, ..Default::default() });
        assert!(notes.iter().any(|n| n.contains("loose")), "{notes:?}");
        let t = crate::topo::analyse(&mesh);
        assert!(t.watertight && t.manifold, "{t:?}");
        assert_eq!(mesh.uvs.len(), mesh.positions.len());
        let tex = mesh.materials.iter().find_map(|m| m.texture.as_ref()).expect("texture");
        assert!(tex.rgba.chunks_exact(4).any(|p| p[3] == 255));
        let (lo, hi) = mesh.bounds();
        assert!(((0..3).map(|k| hi[k] - lo[k]).fold(0.0, f32::max) - 100.0).abs() < 8.0);
    }
}
