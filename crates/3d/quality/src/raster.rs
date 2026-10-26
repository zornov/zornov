// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A small headless renderer for judging models: orthographic views from
//! any direction to a coverage mask, a depth buffer and a plainly lit
//! picture (base colour, one key light, a fill), and PNG writing. The
//! critics compare views, not the film renderer, so a model is judged the
//! same wherever it came from.

use studio_mesh::{Mesh, V3, cross, dot, normalise, sub};

/// A view direction: turned `yaw` degrees about the vertical, then
/// tilted `pitch` degrees (positive looks down from above).
#[derive(Clone, Copy, Debug)]
pub struct View {
    pub yaw: f32,
    pub pitch: f32,
}

/// The views every model is judged from: eight round it at eye level,
/// three from above and one from below.
pub fn standard_views() -> Vec<View> {
    let mut v: Vec<View> = (0..8).map(|i| View { yaw: i as f32 * 45.0, pitch: 0.0 }).collect();
    v.extend([View { yaw: 30.0, pitch: 35.0 }, View { yaw: 150.0, pitch: 35.0 }, View { yaw: 270.0, pitch: 60.0 }, View { yaw: 0.0, pitch: -60.0 }]);
    v
}

pub struct Picture {
    pub size: usize,
    pub mask: Vec<bool>,
    pub depth: Vec<f32>,
    /// Straight sRGB RGBA.
    pub rgba: Vec<u8>,
}

/// Camera basis: right, up and forward (into the screen), in Studio's
/// world (y down).
fn basis(v: View) -> (V3, V3, V3) {
    let (y, p) = (v.yaw.to_radians(), v.pitch.to_radians());
    // Looking along +z at yaw 0 (Studio's camera looks into the screen).
    let fwd = normalise([y.sin() * p.cos(), p.sin(), y.cos() * p.cos()]);
    let world_up = [0.0, -1.0, 0.0];
    let right = normalise(cross(world_up, fwd));
    let right = if dot(right, right) < 0.5 { [1.0, 0.0, 0.0] } else { right };
    let up = normalise(cross(fwd, right));
    (right, up, fwd)
}

fn to_srgb(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0 + 0.5) as u8
}

/// Renders `m` seen from `view`, framed so a sphere of `radius` round
/// `centre` fills the picture.
pub fn render(m: &Mesh, view: View, size: usize, centre: V3, radius: f32) -> Picture {
    let (right, up, fwd) = basis(view);
    let mut pic = Picture { size, mask: vec![false; size * size], depth: vec![f32::INFINITY; size * size], rgba: vec![0; size * size * 4] };
    let s = size as f32 / (2.0 * radius.max(1e-6));
    let project = |p: V3| -> [f32; 3] {
        let d = sub(p, centre);
        [(dot(d, right) * s) + size as f32 * 0.5, size as f32 * 0.5 - dot(d, up) * s, dot(d, fwd)]
    };
    let key = normalise([-0.4, -0.8, -0.45]);
    let mats = m.tri_materials();
    let screen: Vec<[f32; 3]> = m.positions.iter().map(|&p| project(p)).collect();
    for t in 0..m.triangles() {
        let [a, b, c] = m.tri(t);
        let (pa, pb, pc) = (screen[a], screen[b], screen[c]);
        let area = (pb[0] - pa[0]) * (pc[1] - pa[1]) - (pc[0] - pa[0]) * (pb[1] - pa[1]);
        if area.abs() < 1e-12 {
            continue;
        }
        let nrm = m.face_normal(t);
        let mat = &m.materials[(mats.get(t).copied().unwrap_or(0) as usize).min(m.materials.len().saturating_sub(1))];
        let lit = 0.25 + 0.75 * dot(nrm, scale_neg(key)).max(0.0) + 0.2 * dot(nrm, scale_neg(fwd)).max(0.0);
        let x0 = pa[0].min(pb[0]).min(pc[0]).floor().max(0.0) as usize;
        let x1 = (pa[0].max(pb[0]).max(pc[0]).ceil() as usize).min(size);
        let y0 = pa[1].min(pb[1]).min(pc[1]).floor().max(0.0) as usize;
        let y1 = (pa[1].max(pb[1]).max(pc[1]).ceil() as usize).min(size);
        for y in y0..y1 {
            for x in x0..x1 {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let w0 = ((pc[0] - pb[0]) * (py - pb[1]) - (pc[1] - pb[1]) * (px - pb[0])) / area;
                let w1 = ((pa[0] - pc[0]) * (py - pc[1]) - (pa[1] - pc[1]) * (px - pc[0])) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let z = w0 * pa[2] + w1 * pb[2] + w2 * pc[2];
                let i = y * size + x;
                if z >= pic.depth[i] {
                    continue;
                }
                pic.depth[i] = z;
                pic.mask[i] = true;
                let mut col = mat.base_colour;
                if let Some(tex) = &mat.texture
                    && m.uvs.len() == m.positions.len()
                {
                    let uv = [0, 1].map(|k| m.uvs[a][k] * w0 + m.uvs[b][k] * w1 + m.uvs[c][k] * w2);
                    let s = tex.sample(uv);
                    col = [col[0] * s[0], col[1] * s[1], col[2] * s[2], 1.0];
                }
                let shade = lit.min(1.2);
                pic.rgba[i * 4..i * 4 + 4].copy_from_slice(&[to_srgb(col[0] * shade + mat.emissive[0]), to_srgb(col[1] * shade + mat.emissive[1]), to_srgb(col[2] * shade + mat.emissive[2]), 255]);
            }
        }
    }
    pic
}

fn scale_neg(v: V3) -> V3 {
    [-v[0], -v[1], -v[2]]
}

/// The sphere round a mesh's box (centre, radius) with a little room.
pub fn frame(m: &Mesh) -> (V3, f32) {
    let (lo, hi) = m.bounds();
    let c = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5, (lo[2] + hi[2]) * 0.5];
    (c, studio_mesh::length(sub(hi, lo)) * 0.5 * 1.05)
}

/// Writes straight RGBA as a PNG.
pub fn write_png(path: &std::path::Path, width: usize, height: usize, rgba: &[u8]) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), width as u32, height as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().map_err(|e| e.to_string())?;
    w.write_image_data(rgba).map_err(|e| e.to_string())
}

/// Reads a PNG as straight RGBA (width, height, pixels).
pub fn read_png(path: &std::path::Path) -> Result<(usize, usize, Vec<u8>), String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut dec = png::Decoder::new(std::io::BufReader::new(file));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut r = dec.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; r.output_buffer_size()];
    let info = r.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let (w, h) = (info.width as usize, info.height as usize);
    let px = w * h;
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf[..px * 4].to_vec(),
        png::ColorType::Rgb => buf[..px * 3].chunks(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => buf[..px * 2].chunks(2).flat_map(|c| [c[0], c[0], c[0], c[1]]).collect(),
        png::ColorType::Grayscale => buf[..px].iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return Err("indexed PNG left unexpanded".into()),
    };
    Ok((w, h, rgba))
}

/// A contact sheet: pictures side by side in rows of `per_row`.
pub fn sheet(pics: &[&Picture], per_row: usize) -> (usize, usize, Vec<u8>) {
    let s = pics.first().map_or(1, |p| p.size);
    let cols = per_row.min(pics.len()).max(1);
    let rows = pics.len().div_ceil(cols);
    let (w, h) = (cols * s, rows * s);
    let mut out = vec![0u8; w * h * 4];
    for px in out.chunks_mut(4) {
        px.copy_from_slice(&[40, 42, 46, 255]);
    }
    for (k, p) in pics.iter().enumerate() {
        let (ox, oy) = ((k % cols) * s, (k / cols) * s);
        for y in 0..s {
            for x in 0..s {
                if p.mask[y * s + x] {
                    let o = ((oy + y) * w + ox + x) * 4;
                    out[o..o + 4].copy_from_slice(&p.rgba[(y * s + x) * 4..(y * s + x) * 4 + 4]);
                }
            }
        }
    }
    (w, h, out)
}
