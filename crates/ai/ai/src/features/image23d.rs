// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A picture of one object becomes a coloured 3D surface, on this Mac.
//!
//! The model is TripoSR (Tochilkin et al., 2024; MIT code and weights). A
//! transformer reads the picture and writes a triplane (three feature
//! planes); a small field network turns any 3D point into a density and a
//! colour. This module runs both through the native engine, finds where
//! the density crosses the surface level with a sparse coarse-to-fine
//! search (thin parts are kept: the coarse pass keeps every block that
//! holds any density at all), and extracts the surface with surface nets.
//! The result is a triangle surface with a colour per vertex; the app turns
//! it into a clean, unwrapped, textured model.
//!
//! The model is general: it was trained on a broad set of objects, and
//! nothing here depends on what the object is.

use crate::engine;
use crate::error::{invalid, Error, Result};
use crate::jobs::JobCtx;
use crate::registry::ModelId;
use crate::store::ModelStore;
use std::sync::Arc;
use studio_nn::{Session, Tensor};

/// Side of the picture the encoder reads.
pub const INPUT: usize = 512;
/// The cube the field is defined on spans `-RADIUS..RADIUS` on each axis.
const RADIUS: f32 = 0.87;

/// How to run.
#[derive(Clone, Copy, Debug)]
pub struct Params {
    /// Cells along each axis of the final grid (a multiple of 4, 64..=320).
    pub resolution: usize,
    /// Density that counts as the surface.
    pub threshold: f32,
    /// Share of the square picture the object fills (the model's training
    /// setting).
    pub foreground_ratio: f32,
}

impl Default for Params {
    fn default() -> Params {
        Params { resolution: 256, threshold: 25.0, foreground_ratio: 0.85 }
    }
}

/// A coloured triangle surface in the model's own frame (the field's cube,
/// `-0.87..0.87`; the object stands with +Y up and faces +Z once
/// [`Raw::upright`] has run).
#[derive(Clone, Debug, Default)]
pub struct Raw {
    pub positions: Vec<[f32; 3]>,
    /// sRGB 0..1 per vertex.
    pub colours: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
}

pub struct ImageTo3d {
    encoder: Arc<Session>,
    decoder: Arc<Session>,
}

impl ImageTo3d {
    pub fn load(store: &ModelStore) -> Result<ImageTo3d> {
        let dir = store.require(ModelId::TripoSr)?;
        Ok(ImageTo3d { encoder: engine::session(&dir.join("encoder.onnx"))?, decoder: engine::session(&dir.join("decoder.onnx"))? })
    }

    /// The whole job: `rgba` is the picture, its alpha the object (opaque
    /// where the object is). Progress goes to `ctx`.
    pub fn generate(&self, w: usize, h: usize, rgba: &[u8], p: &Params, ctx: &JobCtx) -> Result<Raw> {
        let input = prepare(w, h, rgba, p.foreground_ratio)?;
        ctx.set_message("Reading the picture");
        ctx.set_progress(0.02);
        let codes = self.triplane(input)?;
        ctx.check()?;
        ctx.set_message("Shaping the surface");
        let grid = self.field(&codes, p, &ctx.range(0.1, 0.85))?;
        ctx.check()?;
        ctx.set_message("Building the mesh");
        let mut raw = surface_nets(&grid, p.threshold);
        ctx.set_progress(0.88);
        if raw.triangles.is_empty() {
            return Err(Error::Inference("the model found no object in the picture".into()));
        }
        ctx.set_message("Colouring the surface");
        self.colour(&codes, &mut raw, &ctx.range(0.88, 1.0))?;
        raw.upright();
        Ok(raw)
    }

    /// The picture (1x3x512x512, 0..1) to the triplane (1x3x40x64x64).
    pub fn triplane(&self, input: Tensor) -> Result<Tensor> {
        let name = self.encoder.input_names().first().map(|s| s.to_string()).unwrap_or_else(|| "image".into());
        let mut o = engine::run(&self.encoder, vec![(name.as_str(), input)])?;
        let first = o.first_name().ok_or_else(|| Error::Inference("the encoder returned nothing".into()))?;
        o.take(&first)
    }

    /// Density and colour at `points` (flat xyz).
    pub fn query(&self, codes: &Tensor, points: &[f32]) -> Result<(Vec<f32>, Vec<f32>)> {
        let n = points.len() / 3;
        let names: Vec<String> = self.decoder.input_names().iter().map(|s| s.to_string()).collect();
        let (codes_name, points_name) = if names.len() == 2 {
            if names[0].contains("point") { (names[1].clone(), names[0].clone()) } else { (names[0].clone(), names[1].clone()) }
        } else {
            ("scene_codes".into(), "points".into())
        };
        let mut o = engine::run(&self.decoder, vec![(codes_name.as_str(), codes.clone()), (points_name.as_str(), Tensor::f32(&[1, n, 3], points.to_vec()))])?;
        let outs: Vec<String> = self.decoder.output_names().iter().map(|s| s.to_string()).collect();
        let dname = outs.iter().find(|s| s.contains("dens")).cloned().unwrap_or_else(|| outs[0].clone());
        let cname = outs.iter().find(|s| s.contains("col")).cloned().unwrap_or_else(|| outs[1].clone());
        Ok((o.take(&dname)?.into_f32_vec(), o.take(&cname)?.into_f32_vec()))
    }

    /// Density on a (res+1)^3 grid, evaluated only where it can matter.
    fn field(&self, codes: &Tensor, p: &Params, ctx: &JobCtx) -> Result<Grid> {
        let n = (p.resolution.clamp(64, 320) / 4) * 4;
        let step = 4usize;
        let c = n / step;
        let g = Grid::new(n);
        let at = |i: usize| -RADIUS + 2.0 * RADIUS * i as f32 / n as f32;
        // Coarse pass.
        let cn = c + 1;
        let mut pts = Vec::with_capacity(cn * cn * cn * 3);
        for z in 0..cn {
            for y in 0..cn {
                for x in 0..cn {
                    pts.extend_from_slice(&[at(x * step), at(y * step), at(z * step)]);
                }
            }
        }
        let coarse = self.query_chunked(codes, &pts, &ctx.range(0.0, 0.1))?;
        // Blocks that may hold the surface: some density, not solid inside.
        let mut active = vec![false; c * c * c];
        let cidx = |x: usize, y: usize, z: usize| (z * cn + y) * cn + x;
        for z in 0..c {
            for y in 0..c {
                for x in 0..c {
                    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
                    for k in 0..8 {
                        let v = coarse[cidx(x + (k & 1), y + ((k >> 1) & 1), z + (k >> 2))];
                        lo = lo.min(v);
                        hi = hi.max(v);
                    }
                    if hi > p.threshold * 0.02 && lo < p.threshold * 4.0 {
                        active[(z * c + y) * c + x] = true;
                    }
                }
            }
        }
        let grown = active.clone();
        for z in 0..c {
            for y in 0..c {
                for x in 0..c {
                    if grown[(z * c + y) * c + x] {
                        continue;
                    }
                    'near: for dz in -1i32..=1 {
                        for dy in -1i32..=1 {
                            for dx in -1i32..=1 {
                                let (nx, ny, nz) = (x as i32 + dx, y as i32 + dy, z as i32 + dz);
                                if nx >= 0 && ny >= 0 && nz >= 0 && (nx as usize) < c && (ny as usize) < c && (nz as usize) < c && grown[(nz as usize * c + ny as usize) * c + nx as usize] {
                                    active[(z * c + y) * c + x] = true;
                                    break 'near;
                                }
                            }
                        }
                    }
                }
            }
        }
        // Everything starts as the coarse field spread smoothly over the
        // grid; active blocks are then replaced with the real values.
        let mut grid = g;
        let m = n + 1;
        for z in 0..m {
            for y in 0..m {
                for x in 0..m {
                    let (fx, fy, fz) = (x as f32 / step as f32, y as f32 / step as f32, z as f32 / step as f32);
                    let (x0, y0, z0) = ((fx as usize).min(c - 1), (fy as usize).min(c - 1), (fz as usize).min(c - 1));
                    let (tx, ty, tz) = (fx - x0 as f32, fy - y0 as f32, fz - z0 as f32);
                    let mut v = 0.0;
                    for k in 0..8usize {
                        let (ox, oy, oz) = (k & 1, (k >> 1) & 1, k >> 2);
                        let w = (if ox == 1 { tx } else { 1.0 - tx }) * (if oy == 1 { ty } else { 1.0 - ty }) * (if oz == 1 { tz } else { 1.0 - tz });
                        v += w * coarse[cidx(x0 + ox, y0 + oy, z0 + oz)];
                    }
                    grid.v[(z * m + y) * m + x] = v;
                }
            }
        }
        let mut todo = vec![false; m * m * m];
        for z in 0..c {
            for y in 0..c {
                for x in 0..c {
                    if !active[(z * c + y) * c + x] {
                        continue;
                    }
                    for dz in 0..=step {
                        for dy in 0..=step {
                            for dx in 0..=step {
                                todo[((z * step + dz) * m + y * step + dy) * m + x * step + dx] = true;
                            }
                        }
                    }
                }
            }
        }
        let mut idx = Vec::new();
        let mut pts = Vec::new();
        for z in 0..m {
            for y in 0..m {
                for x in 0..m {
                    let i = (z * m + y) * m + x;
                    if todo[i] {
                        idx.push(i);
                        pts.extend_from_slice(&[at(x), at(y), at(z)]);
                    }
                }
            }
        }
        let vals = self.query_chunked(codes, &pts, &ctx.range(0.1, 1.0))?;
        for (i, v) in idx.into_iter().zip(vals) {
            grid.v[i] = v;
        }
        Ok(grid)
    }

    fn query_chunked(&self, codes: &Tensor, pts: &[f32], ctx: &JobCtx) -> Result<Vec<f32>> {
        const CHUNK: usize = 1 << 16;
        let total = pts.len() / 3;
        let mut out = Vec::with_capacity(total);
        let mut done = 0;
        for chunk in pts.chunks(CHUNK * 3) {
            ctx.check()?;
            let (d, _) = self.query(codes, chunk)?;
            out.extend(d);
            done += chunk.len() / 3;
            ctx.set_progress(done as f32 / total.max(1) as f32);
        }
        Ok(out)
    }

    fn colour(&self, codes: &Tensor, raw: &mut Raw, ctx: &JobCtx) -> Result<()> {
        const CHUNK: usize = 1 << 16;
        let flat: Vec<f32> = raw.positions.iter().flatten().copied().collect();
        let total = raw.positions.len();
        let mut done = 0;
        raw.colours.clear();
        for chunk in flat.chunks(CHUNK * 3) {
            ctx.check()?;
            let (_, c) = self.query(codes, chunk)?;
            raw.colours.extend(c.chunks_exact(3).map(|c| [c[0].clamp(0.0, 1.0), c[1].clamp(0.0, 1.0), c[2].clamp(0.0, 1.0)]));
            done += chunk.len() / 3;
            ctx.set_progress(done as f32 / total.max(1) as f32);
        }
        Ok(())
    }
}

/// The object of a picture, as the picture with its background made
/// see-through, and how it was found:
/// * a picture that already has a see-through background is used as it is;
/// * a picture on a plain background (its whole border one colour) has that
///   background flooded away from the border, so holes inside the object
///   (a handle, a glass's bowl) stay;
/// * any other picture is cut with the object cut-out model (a box over the
///   middle of the picture and a click at its centre), when it is installed.
///
/// `None` when none of these finds an object.
pub fn isolate(store: &ModelStore, w: usize, h: usize, rgba: &[u8]) -> Option<(Vec<u8>, &'static str)> {
    if w == 0 || h == 0 || rgba.len() != w * h * 4 {
        return None;
    }
    if rgba.chunks_exact(4).any(|p| p[3] < 200) {
        let share = rgba.chunks_exact(4).filter(|p| p[3] >= 128).count() as f32 / (w * h) as f32;
        if share > 0.005 {
            return Some((rgba.to_vec(), "its own see-through background"));
        }
    }
    if let Some(m) = plain_background_cut(w, h, rgba) {
        return Some((m, "plain background"));
    }
    cut_subject(store, w, h, rgba).map(|m| (m, "object cut-out model"))
}

/// The picture with a plain background (one colour all round the border)
/// flooded to see-through, or `None` when the border is not plain or
/// almost nothing (or almost everything) is left.
pub fn plain_background_cut(w: usize, h: usize, rgba: &[u8]) -> Option<Vec<u8>> {
    let px = |x: usize, y: usize| -> [i32; 3] {
        let i = (y * w + x) * 4;
        [rgba[i] as i32, rgba[i + 1] as i32, rgba[i + 2] as i32]
    };
    let mut border: Vec<[i32; 3]> = Vec::new();
    for x in 0..w {
        border.push(px(x, 0));
        border.push(px(x, h - 1));
    }
    for y in 1..h.saturating_sub(1) {
        border.push(px(0, y));
        border.push(px(w - 1, y));
    }
    let mut med = [0i32; 3];
    for k in 0..3 {
        let mut v: Vec<i32> = border.iter().map(|p| p[k]).collect();
        v.sort_unstable();
        med[k] = v[v.len() / 2];
    }
    let dist = |p: [i32; 3]| (0..3).map(|k| (p[k] - med[k]).abs()).max().unwrap_or(0);
    // A plain backdrop: nearly the whole border is close to one colour
    // (a seamless sheet, a gentle gradient or vignette may drift from it).
    let near = border.iter().filter(|&&p| dist(p) <= 48).count();
    if (near as f32) < 0.95 * border.len() as f32 {
        return None;
    }
    // The backdrop is whatever joins the border through small steps; the
    // object ends at its first real edge.
    let (step, reach) = (9, 80);
    let step_ok = |a: [i32; 3], b: [i32; 3]| (0..3).all(|k| (a[k] - b[k]).abs() <= step);
    let mut bg = vec![false; w * h];
    let mut stack: Vec<(usize, usize)> = Vec::new();
    for x in 0..w {
        for y in [0, h - 1] {
            if dist(px(x, y)) <= reach && !bg[y * w + x] {
                bg[y * w + x] = true;
                stack.push((x, y));
            }
        }
    }
    for y in 0..h {
        for x in [0, w - 1] {
            if dist(px(x, y)) <= reach && !bg[y * w + x] {
                bg[y * w + x] = true;
                stack.push((x, y));
            }
        }
    }
    while let Some((x, y)) = stack.pop() {
        let here = px(x, y);
        let mut visit = |nx: usize, ny: usize| {
            let i = ny * w + nx;
            if !bg[i] && dist(px(nx, ny)) <= reach && step_ok(here, px(nx, ny)) {
                bg[i] = true;
                stack.push((nx, ny));
            }
        };
        if x > 0 {
            visit(x - 1, y);
        }
        if y > 0 {
            visit(x, y - 1);
        }
        if x + 1 < w {
            visit(x + 1, y);
        }
        if y + 1 < h {
            visit(x, y + 1);
        }
    }
    let fg = bg.iter().filter(|&&b| !b).count() as f32 / (w * h) as f32;
    if !(0.01..=0.92).contains(&fg) {
        return None;
    }
    let mut out = rgba.to_vec();
    for (p, b) in out.chunks_exact_mut(4).zip(&bg) {
        p[3] = if *b { 0 } else { 255 };
    }
    Some(out)
}

/// The object cut-out model on a picture: a box over its middle and a click
/// at the centre; the mask goes into the alpha. `None` when that model is
/// not installed or finds nothing sizeable.
pub fn cut_subject(store: &ModelStore, w: usize, h: usize, rgba: &[u8]) -> Option<Vec<u8>> {
    use crate::features::segment::{ObjectCutout, Prompt};
    let cutter = ObjectCutout::load(store).ok()?;
    let frame = crate::Frame::new(w, h, rgba.to_vec()).ok()?;
    let (fw, fh) = (w as f32, h as f32);
    let prompts = [Prompt::Box { x0: fw * 0.04, y0: fh * 0.04, x1: fw * 0.96, y1: fh * 0.96 }, Prompt::Point { x: fw * 0.5, y: fh * 0.5, positive: true }];
    let emb = cutter.embed(&frame).ok()?;
    let share_of = |s: &crate::features::segment::Selection| s.mask.data.iter().filter(|&&v| v > 0.5).count() as f32 / (w * h) as f32;
    let mut sel = cutter.segment(&emb, &prompts).ok()?;
    // A box over most of the picture can come back as the backdrop round
    // the object: try the centre alone with the corners marked as not it.
    if share_of(&sel) > 0.8 {
        let alt = [Prompt::Point { x: fw * 0.5, y: fh * 0.5, positive: true }, Prompt::Point { x: fw * 0.03, y: fh * 0.03, positive: false }, Prompt::Point { x: fw * 0.97, y: fh * 0.03, positive: false }, Prompt::Point { x: fw * 0.03, y: fh * 0.97, positive: false }, Prompt::Point { x: fw * 0.97, y: fh * 0.97, positive: false }];
        if let Ok(s2) = cutter.segment(&emb, &alt) {
            sel = s2;
        }
    }
    let share = share_of(&sel);
    if !(0.01..=0.9).contains(&share) {
        return None;
    }
    let mut out = rgba.to_vec();
    for (px, m) in out.chunks_exact_mut(4).zip(&sel.mask.data) {
        px[3] = if *m > 0.5 { 255 } else { 0 };
    }
    Some(out)
}

impl Raw {
    /// The field's frame has the object lying on its back; turn it to
    /// stand with +Y up, facing +Z (rotate -90 degrees about X, then +90
    /// about Y).
    pub fn upright(&mut self) {
        for p in &mut self.positions {
            let [x, y, z] = *p;
            // Rx(-90): (x, y, z) -> (x, z, -y); Ry(+90): (x, y, z) -> (z, y, -x)
            let (x1, y1, z1) = (x, z, -y);
            *p = [z1, y1, -x1];
        }
    }
}

/// Picture preparation: the object is cropped to fill `ratio` of a square,
/// laid over mid-grey and scaled to the encoder's input.
pub fn prepare(w: usize, h: usize, rgba: &[u8], ratio: f32) -> Result<Tensor> {
    if w == 0 || h == 0 || rgba.len() != w * h * 4 {
        return invalid(format!("picture {w}x{h} with {} bytes", rgba.len()));
    }
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0usize, 0usize);
    for y in 0..h {
        for x in 0..w {
            if rgba[(y * w + x) * 4 + 3] > 127 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    if x1 <= x0 || y1 <= y0 {
        return invalid("no object in the picture (it is all see-through)");
    }
    let (bw, bh) = ((x1 - x0) as f32, (y1 - y0) as f32);
    let side = (bw.max(bh) / ratio.clamp(0.3, 1.0)).max(1.0);
    let (cx, cy) = ((x0 + x1) as f32 * 0.5, (y0 + y1) as f32 * 0.5);
    let (ox, oy) = (cx - side * 0.5, cy - side * 0.5);
    let scale = side / INPUT as f32;
    let taps = (scale.ceil() as usize).clamp(1, 8);
    let mut out = vec![0.5f32; 3 * INPUT * INPUT];
    for oyp in 0..INPUT {
        for oxp in 0..INPUT {
            let mut acc = [0.0f32; 3];
            for ty in 0..taps {
                for tx in 0..taps {
                    let sx = ox + (oxp as f32 + (tx as f32 + 0.5) / taps as f32) * scale - 0.5;
                    let sy = oy + (oyp as f32 + (ty as f32 + 0.5) / taps as f32) * scale - 0.5;
                    let px = sample(w, h, rgba, sx, sy);
                    for k in 0..3 {
                        acc[k] += px[k] * px[3] + 0.5 * (1.0 - px[3]);
                    }
                }
            }
            let n = (taps * taps) as f32;
            for k in 0..3 {
                out[k * INPUT * INPUT + oyp * INPUT + oxp] = acc[k] / n;
            }
        }
    }
    Ok(Tensor::f32(&[1, 3, INPUT, INPUT], out))
}

/// Bilinear RGBA (0..1, colour not premultiplied) with see-through outside.
fn sample(w: usize, h: usize, rgba: &[u8], x: f32, y: f32) -> [f32; 4] {
    let (fx, fy) = (x.floor(), y.floor());
    let (tx, ty) = (x - fx, y - fy);
    let mut acc = [0.0f32; 4];
    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
        let (ix, iy) = (fx as i64 + dx, fy as i64 + dy);
        let wt = (if dx == 1 { tx } else { 1.0 - tx }) * (if dy == 1 { ty } else { 1.0 - ty });
        if ix < 0 || iy < 0 || ix >= w as i64 || iy >= h as i64 {
            continue;
        }
        let i = (iy as usize * w + ix as usize) * 4;
        let a = rgba[i + 3] as f32 / 255.0;
        // Colour weighted by alpha so see-through pixels do not bleed in.
        for k in 0..3 {
            acc[k] += wt * a * rgba[i + k] as f32 / 255.0;
        }
        acc[3] += wt * a;
    }
    if acc[3] > 1e-6 {
        [acc[0] / acc[3], acc[1] / acc[3], acc[2] / acc[3], acc[3].min(1.0)]
    } else {
        [0.5, 0.5, 0.5, 0.0]
    }
}

/// Density on a cubic grid of `n + 1` points per side.
pub struct Grid {
    pub n: usize,
    pub v: Vec<f32>,
}

impl Grid {
    pub fn new(n: usize) -> Grid {
        Grid { n, v: vec![0.0; (n + 1) * (n + 1) * (n + 1)] }
    }
}

/// Surface nets over the grid at `level` (inside = above). One vertex per
/// cell the surface crosses, at the mean of its edge crossings; a quad for
/// every grid edge that crosses.
pub fn surface_nets(g: &Grid, level: f32) -> Raw {
    let n = g.n;
    let m = n + 1;
    let at = |i: usize| -RADIUS + 2.0 * RADIUS * i as f32 / n as f32;
    let val = |x: usize, y: usize, z: usize| g.v[(z * m + y) * m + x] - level;
    let mut cell = vec![u32::MAX; n * n * n];
    let mut raw = Raw::default();
    const EDGES: [(usize, usize); 12] = [(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)];
    for z in 0..n {
        for y in 0..n {
            for x in 0..n {
                let mut f = [0.0f32; 8];
                let mut inside = 0;
                for (k, fk) in f.iter_mut().enumerate() {
                    *fk = val(x + (k & 1), y + ((k >> 1) & 1), z + (k >> 2));
                    if *fk > 0.0 {
                        inside += 1;
                    }
                }
                if inside == 0 || inside == 8 {
                    continue;
                }
                let mut sum = [0.0f32; 3];
                let mut cnt = 0.0;
                for &(a, b) in &EDGES {
                    if (f[a] > 0.0) != (f[b] > 0.0) {
                        let t = f[a] / (f[a] - f[b]);
                        let pa = [(a & 1) as f32, ((a >> 1) & 1) as f32, (a >> 2) as f32];
                        let pb = [(b & 1) as f32, ((b >> 1) & 1) as f32, (b >> 2) as f32];
                        for k in 0..3 {
                            sum[k] += pa[k] + (pb[k] - pa[k]) * t;
                        }
                        cnt += 1.0;
                    }
                }
                let s = 2.0 * RADIUS / n as f32;
                let p = [at(x) + sum[0] / cnt * s, at(y) + sum[1] / cnt * s, at(z) + sum[2] / cnt * s];
                cell[(z * n + y) * n + x] = raw.positions.len() as u32;
                raw.positions.push(p);
            }
        }
    }
    let c = |x: usize, y: usize, z: usize| cell[(z * n + y) * n + x];
    let quad = |raw: &mut Raw, q: [u32; 4], flip: bool| {
        if q.contains(&u32::MAX) {
            return;
        }
        let q = if flip { [q[0], q[3], q[2], q[1]] } else { q };
        raw.triangles.push([q[0], q[1], q[2]]);
        raw.triangles.push([q[0], q[2], q[3]]);
    };
    for z in 1..n {
        for y in 1..n {
            for x in 0..n {
                let (a, b) = (val(x, y, z) > 0.0, val(x + 1, y, z) > 0.0);
                if a != b {
                    quad(&mut raw, [c(x, y - 1, z - 1), c(x, y, z - 1), c(x, y, z), c(x, y - 1, z)], !a);
                }
            }
        }
    }
    for z in 1..n {
        for y in 0..n {
            for x in 1..n {
                let (a, b) = (val(x, y, z) > 0.0, val(x, y + 1, z) > 0.0);
                if a != b {
                    quad(&mut raw, [c(x - 1, y, z - 1), c(x - 1, y, z), c(x, y, z), c(x, y, z - 1)], !a);
                }
            }
        }
    }
    for z in 0..n {
        for y in 1..n {
            for x in 1..n {
                let (a, b) = (val(x, y, z) > 0.0, val(x, y, z + 1) > 0.0);
                if a != b {
                    quad(&mut raw, [c(x - 1, y - 1, z), c(x, y - 1, z), c(x, y, z), c(x - 1, y, z)], !a);
                }
            }
        }
    }
    // Drop vertices no face uses.
    let mut used = vec![false; raw.positions.len()];
    for t in &raw.triangles {
        for &i in t {
            used[i as usize] = true;
        }
    }
    let mut remap = vec![u32::MAX; used.len()];
    let mut kept = Vec::new();
    for (i, u) in used.iter().enumerate() {
        if *u {
            remap[i] = kept.len() as u32;
            kept.push(raw.positions[i]);
        }
    }
    raw.positions = kept;
    for t in &mut raw.triangles {
        for i in t.iter_mut() {
            *i = remap[*i as usize];
        }
    }
    raw
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sphere_grid(n: usize, r: f32) -> Grid {
        let mut g = Grid::new(n);
        let m = n + 1;
        for z in 0..m {
            for y in 0..m {
                for x in 0..m {
                    let p = [x, y, z].map(|i| -RADIUS + 2.0 * RADIUS * i as f32 / n as f32);
                    g.v[(z * m + y) * m + x] = 100.0 * (r - (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt());
                }
            }
        }
        g
    }

    #[test]
    fn a_sphere_field_gives_a_closed_outward_surface() {
        let g = sphere_grid(40, 0.5);
        let raw = surface_nets(&g, 0.0);
        assert!(raw.triangles.len() > 1000);
        // Every edge is shared by exactly two faces: closed.
        let mut edges = std::collections::HashMap::new();
        for t in &raw.triangles {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                *edges.entry((a.min(b), a.max(b))).or_insert(0) += 1;
            }
        }
        assert!(edges.values().all(|&c| c == 2), "open or non-manifold edges");
        // Faces point away from the centre and the radius is right.
        let mut vol = 0.0f64;
        for t in &raw.triangles {
            let [a, b, c] = t.map(|i| raw.positions[i as usize]);
            let cr = [(b[1] - a[1]) * (c[2] - a[2]) - (b[2] - a[2]) * (c[1] - a[1]), (b[2] - a[2]) * (c[0] - a[0]) - (b[0] - a[0]) * (c[2] - a[2]), (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])];
            vol += (a[0] * cr[0] + a[1] * cr[1] + a[2] * cr[2]) as f64 / 6.0;
        }
        let want = 4.0 / 3.0 * std::f64::consts::PI * 0.125;
        assert!((vol - want).abs() < want * 0.05, "volume {vol} vs {want}");
    }

    #[test]
    fn the_picture_is_cropped_to_the_object_on_grey() {
        // A 100x40 picture with a red 20x10 object at the left.
        let (w, h) = (100, 40);
        let mut px = vec![0u8; w * h * 4];
        for y in 5..15 {
            for x in 10..30 {
                px[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&[255, 0, 0, 255]);
            }
        }
        let t = prepare(w, h, &px, 0.85).unwrap();
        assert_eq!(t.shape, vec![1, 3, INPUT, INPUT]);
        let d = t.as_f32().unwrap();
        let at = |c: usize, x: usize, y: usize| d[c * INPUT * INPUT + y * INPUT + x];
        // Corners are the grey background; the centre is the red object.
        assert!((at(0, 2, 2) - 0.5).abs() < 1e-3 && (at(2, 2, 2) - 0.5).abs() < 1e-3);
        assert!(at(0, 256, 256) > 0.99 && at(1, 256, 256) < 0.01);
        // It fills 85% of the width.
        let red = (0..INPUT).filter(|&x| at(1, x, 256) < 0.1).count();
        assert!((red as f32 / INPUT as f32 - 0.85).abs() < 0.03, "{red}");
        assert!(prepare(4, 4, &vec![0u8; 64], 0.85).is_err());
    }

    #[test]
    fn a_plain_or_gently_shaded_backdrop_is_cut_away_and_holes_stay() {
        // A grey wall that darkens to the right, a bright ring in the middle
        // (hole inside) and nothing else.
        let (w, h) = (80usize, 60usize);
        let mut px = vec![255u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let g = (200.0 - x as f32 * 0.4) as u8;
                let (dx, dy) = (x as f32 - 40.0, y as f32 - 30.0);
                let r = (dx * dx + dy * dy).sqrt();
                let c = if (14.0..22.0).contains(&r) { [220, 40, 40] } else { [g, g, g] };
                px[(y * w + x) * 4..(y * w + x) * 4 + 3].copy_from_slice(&c);
            }
        }
        let out = plain_background_cut(w, h, &px).expect("plain backdrop");
        let a = |x: usize, y: usize| out[(y * w + x) * 4 + 3];
        assert_eq!(a(2, 2), 0);
        assert_eq!(a(77, 57), 0);
        assert_eq!(a(40 + 18, 30), 255, "the ring is kept");
        // The middle of the ring is enclosed by it, so it is not reached from the border.
        assert_eq!(a(40, 30), 255);
        // A busy border is not a plain backdrop.
        let mut busy = px.clone();
        for (i, p) in busy.chunks_exact_mut(4).enumerate() {
            p[0] = (i * 37 % 255) as u8;
        }
        assert!(plain_background_cut(w, h, &busy).is_none());
    }

    #[test]
    fn the_model_is_registered_with_its_licence() {
        let s = ModelId::TripoSr.spec();
        assert_eq!(s.licence, "MIT");
        assert_eq!(s.files.len(), 2);
        assert_eq!(ModelId::from_key("triposr"), Some(ModelId::TripoSr));
    }
}
