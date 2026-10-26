// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Models from pictures, and the clean-up every generated or scanned model
//! needs before it is usable.
//!
//! - [`carve`]: a solid from one or more views of an object — each view's
//!   silhouette (and, when known, its depth) carves a block of voxels the
//!   way a visual hull does; one front view with depth gives a relief with
//!   a mirrored back, front + side + back views give a full form.
//! - [`paint_from_views`]: texture painted onto an unwrapped model by
//!   projecting each view where it faces the surface best.
//! - [`clean_up`]: any raw triangle soup (a reconstruction, an imported
//!   model's file, a scan) made watertight, rebuilt as even quads, laid out
//!   for texturing, with the raw surface kept as detail to bake from.
//!
//! Making a model from words or from one picture alone is not available yet.

use crate::bake::Map;
use crate::poly::Poly;
use crate::remesh::{Grid, surface_nets};
use studio_mesh::{V3, dot, length, sub};

/// Which way a view looks at the object (the camera's side).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Front,
    Back,
    Left,
    Right,
    Top,
    Bottom,
}

impl Side {
    pub fn parse(s: &str) -> Option<Side> {
        Some(match s {
            "front" => Side::Front,
            "back" => Side::Back,
            "left" => Side::Left,
            "right" => Side::Right,
            "top" => Side::Top,
            "bottom" => Side::Bottom,
            _ => return None,
        })
    }

    /// For a point in the unit cube (−0.5…0.5, y down), its picture
    /// position (0–1 across, 0–1 down) and its depth into the view (0 at
    /// the near face, 1 at the far one).
    pub fn project(self, p: V3) -> ([f32; 2], f32) {
        let [x, y, z] = p;
        match self {
            // The camera at −z looking along +z.
            Side::Front => ([x + 0.5, y + 0.5], z + 0.5),
            Side::Back => ([0.5 - x, y + 0.5], 0.5 - z),
            // Camera at −x looking along +x (it sees the left side).
            Side::Left => ([0.5 - z, y + 0.5], x + 0.5),
            Side::Right => ([z + 0.5, y + 0.5], 0.5 - x),
            // Camera above (−y) looking down.
            Side::Top => ([x + 0.5, z + 0.5], y + 0.5),
            Side::Bottom => ([x + 0.5, 0.5 - z], 0.5 - y),
        }
    }

    /// The direction from the object toward the camera.
    pub fn toward(self) -> V3 {
        match self {
            Side::Front => [0.0, 0.0, -1.0],
            Side::Back => [0.0, 0.0, 1.0],
            Side::Left => [-1.0, 0.0, 0.0],
            Side::Right => [1.0, 0.0, 0.0],
            Side::Top => [0.0, -1.0, 0.0],
            Side::Bottom => [0.0, 1.0, 0.0],
        }
    }
}

/// One picture of the object.
#[derive(Clone, Debug)]
pub struct View {
    pub side: Side,
    pub width: u32,
    pub height: u32,
    /// Straight sRGB RGBA.
    pub rgba: Vec<u8>,
    /// How much each pixel is the object (0–1); from the picture's alpha
    /// when missing.
    pub mask: Option<Vec<f32>>,
    /// Relative nearness 0 (far) – 1 (near), as depth estimators give it.
    pub depth: Option<Vec<f32>>,
}

impl View {
    fn mask_at(&self, u: f32, v: f32) -> f32 {
        let x = ((u * self.width as f32) as i64).clamp(0, self.width as i64 - 1) as usize;
        let y = ((v * self.height as f32) as i64).clamp(0, self.height as i64 - 1) as usize;
        let i = y * self.width as usize + x;
        match &self.mask {
            Some(m) => m.get(i).copied().unwrap_or(0.0),
            None => self.rgba.get(i * 4 + 3).map_or(0.0, |a| *a as f32 / 255.0),
        }
    }

    fn depth_at(&self, u: f32, v: f32) -> Option<f32> {
        let d = self.depth.as_ref()?;
        let x = ((u * self.width as f32) as i64).clamp(0, self.width as i64 - 1) as usize;
        let y = ((v * self.height as f32) as i64).clamp(0, self.height as i64 - 1) as usize;
        d.get(y * self.width as usize + x).copied()
    }

    /// Linear RGB at (u, v), bilinear.
    pub fn colour_at(&self, u: f32, v: f32) -> V3 {
        let t = studio_mesh::Texture { width: self.width, height: self.height, rgba: self.rgba.clone() };
        let c = t.sample([u.clamp(0.0, 0.9999), v.clamp(0.0, 0.9999)]);
        [c[0], c[1], c[2]]
    }

    /// The object's box in the picture (u0, v0, u1, v1), from the mask.
    pub fn bounds(&self) -> Option<[f32; 4]> {
        let (w, h) = (self.width as usize, self.height as usize);
        let mut b = [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];
        for y in 0..h {
            for x in 0..w {
                let u = (x as f32 + 0.5) / w as f32;
                let v = (y as f32 + 0.5) / h as f32;
                if self.mask_at(u, v) > 0.5 {
                    b = [b[0].min(u), b[1].min(v), b[2].max(u), b[3].max(v)];
                }
            }
        }
        b[0].is_finite().then_some(b)
    }
}

/// How a single front view fills out its back.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Relief {
    /// Depth of the object as a share of its height (0.1 a coin – 1 a
    /// ball).
    pub thickness: f32,
    /// The back mirrors the front (true) or is flat (false).
    pub mirror: bool,
}

/// A solid from views of an object, `size` px tall, on a `cells`-high
/// voxel grid.
pub fn carve(views: &[View], size: f32, cells: usize, relief: Relief) -> Result<Poly, String> {
    if views.is_empty() {
        return Err("no views".into());
    }
    // Each view's box crops it to the object, so all views share one box.
    let crops: Vec<[f32; 4]> = views.iter().map(|v| v.bounds().ok_or_else(|| format!("the {:?} view shows no object (the mask is empty)", v.side))).collect::<Result<_, _>>()?;
    let front = views.iter().position(|v| v.side == Side::Front).unwrap_or(0);
    let aspect = {
        let c = crops[front];
        let (w, h) = ((c[2] - c[0]) * views[front].width as f32, (c[3] - c[1]) * views[front].height as f32);
        w / h.max(1.0)
    };
    let has_side = views.iter().any(|v| matches!(v.side, Side::Left | Side::Right));
    let depth_share = if has_side {
        let i = views.iter().position(|v| matches!(v.side, Side::Left | Side::Right)).unwrap();
        let c = crops[i];
        ((c[2] - c[0]) * views[i].width as f32) / ((c[3] - c[1]) * views[i].height as f32).max(1.0)
    } else {
        relief.thickness.clamp(0.02, 2.0)
    };
    // Box: x across aspect, y 1 tall, z depth_share deep (unit cube space
    // scaled per axis).
    let dims_f = [aspect.max(0.05), 1.0, depth_share.max(0.02)];
    let n = cells.clamp(16, 256);
    let cell = 1.0 / n as f32;
    let dims = [((dims_f[0] / cell).ceil() as usize + 4).max(6), n + 4, ((dims_f[2] / cell).ceil() as usize + 4).max(6)];
    let origin = [-dims_f[0] / 2.0 - 2.0 * cell, -0.5 - 2.0 * cell, -dims_f[2] / 2.0 - 2.0 * cell];
    let mut g = Grid { origin, cell, dims, d: vec![1.0; dims[0] * dims[1] * dims[2]] };
    for k in 0..dims[2] {
        for j in 0..dims[1] {
            for i in 0..dims[0] {
                let p = g.point(i, j, k);
                // Into the unit cube of each view.
                let unit = [p[0] / dims_f[0], p[1], p[2] / dims_f[2]];
                if unit.iter().any(|c| c.abs() > 0.5) {
                    continue;
                }
                let mut inside = 1.0f32;
                for (vi, v) in views.iter().enumerate() {
                    let (uv, depth_in) = v.side.project(unit);
                    let c = crops[vi];
                    let (u, w) = (c[0] + uv[0] * (c[2] - c[0]), c[1] + uv[1] * (c[3] - c[1]));
                    let m = v.mask_at(u, w);
                    // Silhouette: signed (inside positive).
                    let mut s = m - 0.5;
                    // Depth: where the view says the near surface is.
                    if let Some(near) = v.depth_at(u, w)
                        && m > 0.5
                    {
                        let front_at = (1.0 - near) * 0.5;
                        let back_at = if views.len() == 1 && relief.mirror { 1.0 - front_at } else if views.len() == 1 { 1.0 } else { 2.0 };
                        let ds = (depth_in - front_at).min(back_at - depth_in);
                        s = s.min(ds * 4.0);
                    }
                    inside = inside.min(s);
                }
                let at = g.index(i, j, k);
                g.d[at] = -inside;
            }
        }
    }
    let mut p = surface_nets(&g);
    if p.faces() == 0 {
        return Err("the views don't agree on any solid".into());
    }
    p.orient_outward();
    let all = vec![true; p.positions.len()];
    crate::modifiers::smooth(&mut p, 4, 0.5, &all);
    p.map_positions(|q| [q[0] * size, q[1] * size, q[2] * size]);
    Ok(p)
}

/// Paints a model's texture from the views: each texel takes the colour
/// of the view that faces it most squarely (and that sees it).
pub fn paint_from_views(low: &Poly, views: &[View], size: u32) -> Map {
    let texels = crate::bake::texels(low, size, 60.0);
    let (lo, hi) = low.bounds();
    let ext = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
    let mid = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
    let crops: Vec<Option<[f32; 4]>> = views.iter().map(View::bounds).collect();
    let mut map = Map::new(size, size, 4, 0.5);
    for t in &texels {
        let unit = [(t.pos[0] - mid[0]) / ext[0].max(1e-6), (t.pos[1] - mid[1]) / ext[1].max(1e-6), (t.pos[2] - mid[2]) / ext[2].max(1e-6)];
        let mut best: Option<(f32, V3)> = None;
        for (vi, v) in views.iter().enumerate() {
            let Some(c) = crops[vi] else { continue };
            let facing = dot(t.normal, v.side.toward());
            if facing <= 0.05 {
                continue;
            }
            let (uv, _) = v.side.project(unit);
            let (u, w) = (c[0] + uv[0] * (c[2] - c[0]), c[1] + uv[1] * (c[3] - c[1]));
            if v.mask_at(u, w) < 0.5 {
                continue;
            }
            if best.as_ref().is_none_or(|b| facing > b.0) {
                best = Some((facing, v.colour_at(u, w)));
            }
        }
        // Unseen texels take the nearest view's colour, darkened a little.
        let col = best.map(|b| b.1).unwrap_or_else(|| {
            let v = &views[0];
            let c = crops[0].unwrap_or([0.0, 0.0, 1.0, 1.0]);
            let (uv, _) = v.side.project(unit);
            let k = v.colour_at(c[0] + uv[0] * (c[2] - c[0]), c[1] + uv[1] * (c[3] - c[1]));
            [k[0] * 0.8, k[1] * 0.8, k[2] * 0.8]
        });
        map.set(t.x, t.y, &[col[0], col[1], col[2], 1.0]);
        map.covered[(t.y * size + t.x) as usize] = true;
    }
    map.dilate(8);
    map
}

/// What [`clean_up`] gives back.
pub struct Cleaned {
    /// Even quads, unwrapped.
    pub mesh: Poly,
    /// The raw surface, to bake detail and colour from.
    pub detail: Poly,
    pub notes: Vec<String>,
}

/// Makes a raw model usable: watertight, even quads near `faces`, laid
/// out for texturing; the raw surface kept as detail.
pub fn clean_up(raw: &Poly, faces: usize) -> Result<Cleaned, String> {
    let mut notes = Vec::new();
    let (lo, hi) = raw.bounds();
    let ext = length(sub(hi, lo)).max(1e-3);
    let open = crate::poly::Topology::of(raw).boundary_edges().len();
    if open > 0 {
        notes.push(format!("the raw model had {open} open edges; closed by remeshing"));
    }
    // Watertight first (also merges overlapping shells), then quads.
    let solid = crate::remesh::voxel(raw, ext / 200.0)?;
    let (mut mesh, field) = crate::retopo::quad_remesh_report(&solid, faces)?;
    if !field {
        notes.push("the surface was rebuilt as even voxel quads (the flow-following quads didn't read cleanly)".into());
    }
    let q = crate::uv::unwrap(&mut mesh, &crate::uv::Unwrap::default());
    notes.push(format!("{} quads in {} texture pieces", mesh.faces(), q.charts));
    Ok(Cleaned { mesh, detail: solid, notes })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A disc picture with a dome of depth.
    fn disc_view(side: Side, n: u32, with_depth: bool) -> View {
        let mut rgba = Vec::new();
        let mut depth = Vec::new();
        for y in 0..n {
            for x in 0..n {
                let (u, v) = ((x as f32 + 0.5) / n as f32 - 0.5, (y as f32 + 0.5) / n as f32 - 0.5);
                let r = (u * u + v * v).sqrt();
                let inside = r < 0.4;
                rgba.extend_from_slice(&[200, 60, 40, if inside { 255 } else { 0 }]);
                depth.push(if inside { (1.0 - (r / 0.4).powi(2)).sqrt() } else { 0.0 });
            }
        }
        View { side, width: n, height: n, rgba, mask: None, depth: with_depth.then_some(depth) }
    }

    #[test]
    fn three_round_views_carve_a_ball() {
        let views = vec![disc_view(Side::Front, 64, false), disc_view(Side::Left, 64, false), disc_view(Side::Top, 64, false)];
        let p = carve(&views, 100.0, 48, Relief { thickness: 1.0, mirror: true }).unwrap();
        assert!(crate::poly::Topology::of(&p).boundary_edges().is_empty());
        let (lo, hi) = p.bounds();
        for i in 0..3 {
            assert!((hi[i] - lo[i] - 100.0).abs() < 10.0, "axis {i}: {:?}", (lo, hi));
        }
    }

    #[test]
    fn one_view_with_depth_makes_a_mirrored_dome() {
        let p = carve(&[disc_view(Side::Front, 64, true)], 100.0, 48, Relief { thickness: 0.6, mirror: true }).unwrap();
        assert!(crate::poly::Topology::of(&p).boundary_edges().is_empty());
        let (lo, hi) = p.bounds();
        assert!(hi[2] - lo[2] > 30.0 && hi[2] - lo[2] < 70.0, "{:?}", (lo, hi));
        // Clean-up gives quads with a layout, and painting takes the red.
        let c = clean_up(&p, 300).unwrap();
        assert!(c.mesh.has_uvs());
        let map = paint_from_views(&c.mesh, &[disc_view(Side::Front, 64, true)], 64);
        let red: Vec<f32> = (0..64 * 64).filter(|&i| map.covered[i]).map(|i| map.data[i * 4]).collect();
        let mean = red.iter().sum::<f32>() / red.len() as f32;
        assert!(mean > 0.3, "{mean}");
    }
}
