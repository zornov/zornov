// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Lens and camera nodes: lens distortion with lateral chromatic
//! aberration, depth of field from the depth channel, motion blur from the
//! motion channel, film grain and grain matching.

use std::sync::Arc;

use super::nodes::{BASE, NodeCategory, NodeDef, main_or_blank, slot, straight};
use super::{Node, NodeCtx, Plate, geo};
use crate::image::{Edge, Image, linear_to_srgb, premultiply, srgb_to_linear};
use crate::math::hash3;
use crate::params::{ParamDef, ParamSet, Unit};

pub static NODES: &[NodeDef] = &[
    NodeDef {
        id: "lens_distort",
        name: "Lens distortion",
        category: NodeCategory::Lens,
        description: "Removes or adds barrel/pincushion distortion and lateral chromatic aberration, to match CG to a real lens (undistort the plate, comp, redistort).",
        params: &[
            ParamDef::choice("mode", "Mode", &["Undistort", "Distort"], 0),
            ParamDef::float("k1", "Distortion k1", -1.0, 1.0, 0.0),
            ParamDef::float("k2", "Distortion k2", -1.0, 1.0, 0.0),
            ParamDef::point("centre", "Centre", [0.5, 0.5]),
            ParamDef::float("scale", "Scale", 0.25, 4.0, 1.0).unit(Unit::Times),
            ParamDef::float("chroma", "Chromatic aberration", -0.05, 0.05, 0.0),
            ParamDef::choice("edge", "Edges", &["Transparent", "Clamp", "Mirror"], 1),
        ],
        run: lens_distort,
        ..BASE
    },
    NodeDef {
        id: "zdefocus",
        name: "Depth of field",
        category: NodeCategory::Lens,
        description: "Lens blur by depth: sharp at the focus distance, softer away from it (without depth, an even lens blur).",
        params: &[
            ParamDef::float("focus", "Focus distance", 0.0, 100000.0, 10.0),
            ParamDef::bool("focus_at_point", "Focus on the point", false),
            ParamDef::point("focus_point", "Focus point", [0.5, 0.5]),
            ParamDef::float("depth_of_field", "In-focus range", 0.0, 100000.0, 0.0),
            ParamDef::float("size", "Maximum blur", 0.0, 200.0, 12.0),
            ParamDef::choice("shape", "Bokeh", &["Disc", "Gaussian"], 0),
            ParamDef::int("quality", "Samples", 8, 256, 48),
            ParamDef::choice("output", "Output", &["Result", "Focus map"], 0),
        ],
        run: zdefocus,
        ..BASE
    },
    NodeDef {
        id: "vector_blur",
        name: "Vector blur",
        category: NodeCategory::Lens,
        description: "Motion blur along the motion channel (from Motion vectors or a CG render).",
        params: &[
            ParamDef::float("shutter", "Shutter", 0.0, 4.0, 0.5).unit(Unit::Fraction),
            ParamDef::int("samples", "Samples", 2, 64, 16),
            ParamDef::choice("offset", "Shutter", &["Centred", "Trailing", "Leading"], 0),
        ],
        run: vector_blur,
        ..BASE
    },
    NodeDef {
        id: "grain",
        name: "Grain",
        category: NodeCategory::Lens,
        description: "Film or sensor grain, different every frame, shaped by brightness.",
        params: &[
            ParamDef::float("amount", "Amount", 0.0, 1.0, 0.04),
            ParamDef::float("size", "Size", 0.0, 8.0, 1.0),
            ParamDef::float("colour", "Colour", 0.0, 1.0, 0.3).unit(Unit::Fraction),
            ParamDef::float("shadows", "In shadows", 0.0, 4.0, 1.0),
            ParamDef::float("highlights", "In highlights", 0.0, 4.0, 0.6),
            ParamDef::bool("still", "Same every frame", false),
        ],
        run: grain,
        animated: grain_animated,
        ..BASE
    },
    NodeDef {
        id: "grain_match",
        name: "Grain match",
        category: NodeCategory::Lens,
        description: "Measures the reference plate's grain (strength per brightness and channel, size, colour) and adds what the input lacks, so CG and clean-ups sit in the footage.",
        inputs: &["in", "reference"],
        params: &[ParamDef::float("amount", "Amount", 0.0, 4.0, 1.0), ParamDef::bool("still", "Same every frame", false)],
        run: grain_match,
        animated: grain_animated,
        ..BASE
    },
];

fn grain_animated(n: &Node) -> bool {
    !n.params_at(0.0).b("still")
}

// --------------------------------------------------------- lens distortion

/// Brown radial distortion: maps an undistorted normalised position (from
/// the centre, half-diagonal = 1) to its distorted one.
pub fn distort(u: [f64; 2], k1: f64, k2: f64) -> [f64; 2] {
    let r2 = u[0] * u[0] + u[1] * u[1];
    let f = 1.0 + k1 * r2 + k2 * r2 * r2;
    [u[0] * f, u[1] * f]
}

/// The inverse of [`distort`] by Newton's method on the radius.
pub fn undistort(d: [f64; 2], k1: f64, k2: f64) -> [f64; 2] {
    let rd = (d[0] * d[0] + d[1] * d[1]).sqrt();
    if rd < 1e-12 {
        return d;
    }
    let mut r = rd;
    for _ in 0..20 {
        let f = r * (1.0 + k1 * r * r + k2 * r.powi(4)) - rd;
        let df = 1.0 + 3.0 * k1 * r * r + 5.0 * k2 * r.powi(4);
        if df.abs() < 1e-9 {
            break;
        }
        let step = f / df;
        r -= step;
        if step.abs() < 1e-10 {
            break;
        }
    }
    [d[0] * r / rd, d[1] * r / rd]
}

fn lens_distort(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], p: &ParamSet) -> Result<Plate, String> {
    let src = main_or_blank(ctx, inputs);
    let (w, h) = (ctx.width as f64, ctx.height as f64);
    let c = p.point("centre");
    let (cx, cy) = (c[0] as f64 * w, c[1] as f64 * h);
    let norm = 0.5 * (w * w + h * h).sqrt();
    let (k1, k2) = (p.f("k1") as f64, p.f("k2") as f64);
    let scale = p.f("scale") as f64;
    let ca = p.f("chroma") as f64;
    let undist = p.choice("mode") == 0;
    let edge = super::nodes::edge_mode(p.choice("edge"));
    // Output pixel → source pixel, for one channel's extra radial scale.
    let map = |x: f64, y: f64, chan: f64| -> [f64; 2] {
        let q = [(x - cx) / norm / scale, (y - cy) / norm / scale];
        let s = if undist { distort(q, k1, k2) } else { undistort(q, k1, k2) };
        let k = 1.0 + chan;
        [cx + s[0] * norm * k, cy + s[1] * norm * k]
    };
    let rgba = Image::from_fn(ctx.width, ctx.height, |x, y| {
        let (fx, fy) = (x as f64 + 0.5, y as f64 + 0.5);
        let g = map(fx, fy, 0.0);
        let mid = src.rgba.sample(g[0] as f32, g[1] as f32, edge);
        if ca == 0.0 {
            return mid;
        }
        let r = map(fx, fy, if undist { -ca } else { ca });
        let b = map(fx, fy, if undist { ca } else { -ca });
        let rr = src.rgba.sample(r[0] as f32, r[1] as f32, edge);
        let bb = src.rgba.sample(b[0] as f32, b[1] as f32, edge);
        [rr[0], mid[1], bb[2], mid[3]]
    });
    let mut out = src.with_rgba(rgba);
    let warp = |pl: &Vec<f32>| -> Vec<f32> {
        let mut o = Vec::with_capacity(pl.len());
        for y in 0..ctx.height {
            for x in 0..ctx.width {
                let s = map(x as f64 + 0.5, y as f64 + 0.5, 0.0);
                o.push(geo::sample_plane(pl, ctx.width, ctx.height, s[0] as f32, s[1] as f32));
            }
        }
        o
    };
    out.depth = src.depth.as_ref().map(warp);
    out.id = src.id.as_ref().map(warp);
    Ok(out)
}

// ----------------------------------------------------------- depth of field

/// Circle of confusion radius (pixels) at depth `z`.
#[inline]
pub(crate) fn coc(z: f32, focus: f32, dof: f32, size: f32) -> f32 {
    if !(z > 0.0) || !(focus > 0.0) {
        return 0.0;
    }
    let half = dof * 0.5;
    let zz = if (z - focus).abs() <= half { return 0.0 } else if z > focus { z - half } else { z + half };
    (size * (1.0 - focus / zz).abs()).min(size)
}

fn zdefocus(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], p: &ParamSet) -> Result<Plate, String> {
    let src = main_or_blank(ctx, inputs);
    let (w, h) = (ctx.width, ctx.height);
    let size = p.f("size");
    let dof = p.f("depth_of_field");
    let depth = src.depth.clone().unwrap_or_else(|| vec![f32::INFINITY; w * h]);
    let mut focus = p.f("focus");
    if p.b("focus_at_point") {
        let fp = p.point("focus_point");
        focus = geo::sample_plane(&depth, w, h, fp[0] * w as f32, fp[1] * h as f32);
    }
    let cocs: Vec<f32> = if src.depth.is_some() { depth.iter().map(|&z| coc(z, focus, dof, size)).collect() } else { vec![size; w * h] };
    if p.choice("output") == 1 {
        let rgba = Image::from_fn(w, h, |x, y| {
            let v = cocs[y * w + x] / size.max(1e-6);
            [v, v, v, 1.0]
        });
        return Ok(src.with_rgba(rgba));
    }
    if size <= 0.25 {
        return Ok(src);
    }
    let n = p.i("quality").max(8) as usize;
    let gaussian = p.choice("shape") == 1;
    // Vogel spiral over the unit disc.
    let golden = std::f32::consts::PI * (3.0 - 5f32.sqrt());
    let taps: Vec<[f32; 2]> = (0..n)
        .map(|i| {
            let r = ((i as f32 + 0.5) / n as f32).sqrt();
            let a = i as f32 * golden;
            [r * a.cos(), r * a.sin()]
        })
        .collect();
    let rgba = Image::from_fn(w, h, |x, y| {
        let i = y * w + x;
        let (cz, cc) = (depth[i], cocs[i]);
        let mut acc = [0.0f32; 4];
        let mut wsum = 0.0f32;
        let centre = src.rgba.pixels[i];
        let wc = 1.0 / (cc * cc + 1.0);
        for k in 0..4 {
            acc[k] += centre[k] * wc;
        }
        wsum += wc;
        for t in &taps {
            let (ox, oy) = (t[0] * size, t[1] * size);
            let d = (ox * ox + oy * oy).sqrt();
            let sx = (x as f32 + ox).round().clamp(0.0, (w - 1) as f32) as usize;
            let sy = (y as f32 + oy).round().clamp(0.0, (h - 1) as f32) as usize;
            let j = sy * w + sx;
            // A sample behind a sharper pixel cannot spread over it.
            let sc = if depth[j] > cz { cocs[j].min(cc) } else { cocs[j] };
            if sc < d {
                continue;
            }
            let mut wt = 1.0 / (sc * sc + 1.0);
            if gaussian {
                wt *= (-2.0 * (d / sc.max(1e-3)).powi(2)).exp();
            }
            let q = src.rgba.pixels[j];
            for k in 0..4 {
                acc[k] += q[k] * wt;
            }
            wsum += wt;
        }
        acc.map(|v| v / wsum.max(1e-9))
    });
    Ok(src.with_rgba(rgba))
}

// --------------------------------------------------------------- motion blur

fn vector_blur(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], p: &ParamSet) -> Result<Plate, String> {
    let src = main_or_blank(ctx, inputs);
    let Some(mv) = src.motion.clone() else { return Ok(src) };
    let (w, h) = (ctx.width, ctx.height);
    let shutter = p.f("shutter");
    let n = p.i("samples").max(2) as usize;
    let (a, b) = match p.choice("offset") {
        1 => (-1.0, 0.0),
        2 => (0.0, 1.0),
        _ => (-0.5, 0.5),
    };
    let rgba = Image::from_fn(w, h, |x, y| {
        let v = mv[y * w + x];
        if v[0].abs() + v[1].abs() < 1e-3 || shutter <= 0.0 {
            return src.rgba.get(x, y);
        }
        let mut acc = [0.0f32; 4];
        for k in 0..n {
            let t = a + (b - a) * (k as f32 / (n - 1) as f32);
            let s = src.rgba.sample(x as f32 + 0.5 + v[0] * shutter * t, y as f32 + 0.5 + v[1] * shutter * t, Edge::Clamp);
            for c in 0..4 {
                acc[c] += s[c];
            }
        }
        acc.map(|q| q / n as f32)
    });
    Ok(src.with_rgba(rgba))
}

// ------------------------------------------------------------------- grain

/// A unit-variance noise field of grain size `size` pixels (gaussian
/// correlated), one per channel plus a shared one.
fn noise_field(w: usize, h: usize, size: f32, seed: u32, frame: i32, chan: i32) -> Vec<f32> {
    let mut f: Vec<f32> = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            // Sum of two uniforms ≈ triangular; scaled to unit variance.
            let a = hash3(x as i32, y as i32, frame * 4 + chan, seed);
            let b = hash3(x as i32 + 7919, y as i32 - 104_729, frame * 4 + chan, seed ^ 0x5bd1_e995);
            f.push((a + b - 1.0) * 2.449_49);
        }
    }
    let sigma = size * 0.5;
    if sigma > 0.25 {
        f = geo::blur_plane(&f, w, h, sigma);
        // A gaussian blur of white noise divides its spread by 2σ√π.
        let k = 2.0 * sigma * std::f32::consts::PI.sqrt();
        f.iter_mut().for_each(|v| *v *= k);
    }
    f
}

fn grain(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], p: &ParamSet) -> Result<Plate, String> {
    let src = main_or_blank(ctx, inputs);
    let (w, h) = (ctx.width, ctx.height);
    let frame = if p.b("still") { 0 } else { ctx.frame() as i32 };
    let seed = ctx.seed ^ ctx.node.id.wrapping_mul(0x2c1b_3c6d);
    let size = p.f("size");
    let amount = p.f("amount");
    let colour = p.f("colour").clamp(0.0, 1.0);
    let (sh, hi) = (p.f("shadows"), p.f("highlights"));
    let shared = noise_field(w, h, size, seed, frame, 3);
    let chans: Vec<Vec<f32>> = (0..3).map(|c| noise_field(w, h, size, seed, frame, c)).collect();
    let (ks, kc) = ((1.0 - colour).sqrt(), colour.sqrt());
    let rgba = src.rgba.map_xy(|x, y, q| {
        let i = y * w + x;
        let (c, a) = straight(q);
        if a <= 1e-6 {
            return q;
        }
        let e = c.map(|v| linear_to_srgb(v.clamp(0.0, 1.0)));
        let l = 0.2126 * e[0] + 0.7152 * e[1] + 0.0722 * e[2];
        // Shadows → mids → highlights response.
        let resp = if l < 0.5 { sh + (1.0 - sh) * (l / 0.5) } else { 1.0 + (hi - 1.0) * ((l - 0.5) / 0.5) };
        let o: [f32; 3] = std::array::from_fn(|k| srgb_to_linear((e[k] + amount * resp * (ks * shared[i] + kc * chans[k][i])).max(0.0)) + (c[k] - c[k].clamp(0.0, 1.0)));
        premultiply(o, a)
    });
    Ok(src.with_rgba(rgba))
}

/// How grainy a picture is: spread of fine detail per brightness band
/// (shadows, mids, highlights) and channel, in display-encoded units, the
/// grain size in pixels and how alike the channels' grain is (1 = mono).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GrainProfile {
    pub spread: [[f32; 3]; 3],
    pub size: f32,
    pub mono: f32,
}

/// Measures the grain of `img` (where its alpha is solid).
pub fn measure_grain(img: &Image) -> GrainProfile {
    let (w, h) = (img.width, img.height);
    let enc: Vec<[f32; 3]> = img.pixels.iter().map(|q| straight(*q).0.map(|v| linear_to_srgb(v.clamp(0.0, 1.0)))).collect();
    let planes: Vec<Vec<f32>> = (0..3).map(|c| enc.iter().map(|e| e[c]).collect()).collect();
    let hp: Vec<Vec<f32>> = planes.iter().map(|pl| {
        let b = geo::blur_plane(pl, w, h, 1.5);
        pl.iter().zip(&b).map(|(a, b)| a - b).collect()
    }).collect();
    // Edges are detail, not grain: skip pixels with strong local contrast.
    let lum: Vec<f32> = enc.iter().map(|e| 0.2126 * e[0] + 0.7152 * e[1] + 0.0722 * e[2]).collect();
    let lb = geo::blur_plane(&lum, w, h, 3.0);
    let mut sums = [[0.0f64; 3]; 3];
    let mut counts = [0.0f64; 3];
    let (mut c01, mut c00, mut c11) = (0.0f64, 0.0f64, 0.0f64);
    let (mut srg, mut sbg, mut srr, mut sbb, mut sgg) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for y in 2..h.saturating_sub(2) {
        for x in 2..w.saturating_sub(2) {
            let i = y * w + x;
            if img.pixels[i][3] < 0.99 || (lum[i] - lb[i]).abs() > 0.06 {
                continue;
            }
            let band = ((lum[i] * 3.0) as usize).min(2);
            counts[band] += 1.0;
            for c in 0..3 {
                sums[band][c] += (hp[c][i] as f64).powi(2);
            }
            let g = hp[1][i] as f64;
            let gx = hp[1][i + 1] as f64;
            c01 += g * gx;
            c00 += g * g;
            c11 += gx * gx;
            let (r, b) = (hp[0][i] as f64, hp[2][i] as f64);
            (srg, sbg, srr, sbb, sgg) = (srg + r * g, sbg + b * g, srr + r * r, sbb + b * b, sgg + g * g);
        }
    }
    let mut spread = [[0.0f32; 3]; 3];
    for band in 0..3 {
        for c in 0..3 {
            spread[band][c] = if counts[band] > 16.0 { (sums[band][c] / counts[band]).sqrt() as f32 } else { f32::NAN };
        }
    }
    // Fill bands with no samples from their neighbours.
    for c in 0..3 {
        let known: Vec<f32> = (0..3).map(|b| spread[b][c]).filter(|v| v.is_finite()).collect();
        let fallback = if known.is_empty() { 0.0 } else { known.iter().sum::<f32>() / known.len() as f32 };
        for row in spread.iter_mut() {
            if !row[c].is_finite() {
                row[c] = fallback;
            }
        }
    }
    let rho = (c01 / (c00 * c11).sqrt().max(1e-12)).clamp(0.01, 0.95);
    // For high-passed gaussian-correlated noise the lag-1 correlation
    // grows with size; this inverse fits the noise_field generator.
    let size = (2.0 * (-1.0 / (4.0 * rho.ln())).sqrt()).clamp(0.0, 8.0) as f32;
    let corr = |a: f64, aa: f64| a / (aa * sgg).sqrt().max(1e-12);
    let mono = (0.5 * (corr(srg, srr) + corr(sbg, sbb))).clamp(0.0, 1.0) as f32;
    GrainProfile { spread, size, mono }
}

fn grain_match(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], p: &ParamSet) -> Result<Plate, String> {
    let src = main_or_blank(ctx, inputs);
    let Some(r) = slot(inputs, 1) else { return Ok(src) };
    let (w, h) = (ctx.width, ctx.height);
    let want = measure_grain(&r.rgba);
    let have = measure_grain(&src.rgba);
    let frame = if p.b("still") { 0 } else { ctx.frame() as i32 };
    let seed = ctx.seed ^ ctx.node.id.wrapping_mul(0x9e37_79b9);
    let size = want.size;
    let shared = noise_field(w, h, size, seed, frame, 3);
    let chans: Vec<Vec<f32>> = (0..3).map(|c| noise_field(w, h, size, seed, frame, c)).collect();
    // How much of a unit field survives the same high-pass the measure
    // uses, so the added grain measures as asked.
    // Probe at the wanted strength: the measure skips strong local
    // contrast as detail, which would bias a louder probe.
    let amp = want.spread[1][1].clamp(0.004, 0.05);
    let probe = Image { width: w, height: h, pixels: shared.iter().map(|&v| {
        let e = (0.5 + amp * v).clamp(0.0, 1.0);
        let l = srgb_to_linear(e);
        [l, l, l, 1.0]
    }).collect() };
    let kept = measure_grain(&probe).spread[1][1].max(1e-5) / amp;
    let mono = want.mono.clamp(0.0, 1.0);
    let (ks, kc) = (mono.sqrt(), (1.0 - mono).sqrt());
    let amount = p.f("amount");
    let rgba = src.rgba.map_xy(|x, y, q| {
        let i = y * w + x;
        let (c, a) = straight(q);
        if a <= 1e-6 {
            return q;
        }
        let e = c.map(|v| linear_to_srgb(v.clamp(0.0, 1.0)));
        let l = (0.2126 * e[0] + 0.7152 * e[1] + 0.0722 * e[2]).clamp(0.0, 1.0);
        // Interpolate the bands at their centres (1/6, 1/2, 5/6).
        let band_w = |b: usize| (1.0 - ((l - (b as f32 * 2.0 + 1.0) / 6.0).abs() * 3.0)).max(0.0);
        let o: [f32; 3] = std::array::from_fn(|k| {
            let (mut sw, mut s) = (0.0, 0.0);
            for b in 0..3 {
                let wt = band_w(b);
                let add = (want.spread[b][k].powi(2) - have.spread[b][k].powi(2)).max(0.0).sqrt();
                s += wt * add;
                sw += wt;
            }
            let sigma = amount * s / sw.max(1e-6) / kept;
            srgb_to_linear((e[k] + sigma * (ks * shared[i] + kc * chans[k][i])).max(0.0)) + (c[k] - c[k].clamp(0.0, 1.0))
        });
        premultiply(o, a)
    });
    Ok(src.with_rgba(rgba))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undistort_inverts_distort() {
        for &(k1, k2) in &[(0.1, 0.0), (-0.15, 0.02), (0.05, 0.05)] {
            for &u in &[[0.3, 0.2], [-0.7, 0.4], [0.0, 0.9]] {
                let d = distort(u, k1, k2);
                let back = undistort(d, k1, k2);
                assert!((back[0] - u[0]).abs() < 1e-8 && (back[1] - u[1]).abs() < 1e-8);
            }
        }
    }

    #[test]
    fn coc_is_zero_in_focus_and_grows_away() {
        assert_eq!(coc(10.0, 10.0, 0.0, 20.0), 0.0);
        assert_eq!(coc(11.0, 10.0, 4.0, 20.0), 0.0);
        assert!(coc(40.0, 10.0, 0.0, 20.0) > coc(20.0, 10.0, 0.0, 20.0));
        assert!((coc(5.0, 10.0, 0.0, 20.0) - 20.0).abs() < 1e-5);
    }
}
