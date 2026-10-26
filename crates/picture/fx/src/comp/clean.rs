// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Cleanup and set extension: object removal by temporal fill (the
//! background seen in other frames, aligned) and spatial inpainting
//! (multi-scale exemplar synthesis with patch voting after Wexler et al.
//! 2007; matches are found by exhaustive search at the coarsest level,
//! then neighbour coherence and local refinement), clean plates,
//! wire and rig removal, paint strokes over time, sky replacement and
//! matte refinement.
//!
//! Nothing here uses a learned model: every method is classical, so there
//! is no model licence to honour.

use std::sync::Arc;

use serde_json::Value as Json;

use super::nodes::{BASE, NodeCategory, NodeDef, fit_into, main_or_blank, slot, straight};
use super::{Node, NodeCtx, Plate, geo};
use crate::features::{Pyramid, fast_corners, klt, spread_corners};
use crate::image::{Edge, Image, Px, linear_to_srgb, premultiply};
use crate::math::smoothstep;
use crate::ops::Gray;
use crate::params::{ParamDef, ParamSet, Unit};

pub static NODES: &[NodeDef] = &[
    NodeDef {
        id: "inpaint",
        name: "Inpaint",
        category: NodeCategory::Cleanup,
        description: "Fills the matte's area from the rest of the picture (texture synthesis): removes small objects, logos, blemishes, marks.",
        inputs: &["in", "matte"],
        params: &[
            ParamDef::int("patch", "Patch size", 3, 15, 7),
            ParamDef::int("iterations", "Iterations", 1, 20, 5),
            ParamDef::float("grow", "Grow matte", 0.0, 50.0, 2.0),
            ParamDef::choice("method", "Method", &["Texture", "Smooth"], 0),
        ],
        run: inpaint,
        ..BASE
    },
    NodeDef {
        id: "temporal_fill",
        name: "Temporal fill",
        category: NodeCategory::Cleanup,
        description: "Removes a moving object (or one the camera moves past) with the background seen in nearby frames, aligned to this one; what no frame shows is inpainted.",
        inputs: &["in", "matte"],
        params: &[
            ParamDef::int("range", "Frames each way", 1, 60, 12),
            ParamDef::int("step", "Frame step", 1, 10, 2),
            ParamDef::choice("camera", "Camera", &["Moving (align frames)", "Locked off"], 0),
            ParamDef::float("grow", "Grow matte", 0.0, 50.0, 3.0),
            ParamDef::bool("fill_rest", "Inpaint what no frame shows", true),
            ParamDef::bool("match_light", "Match brightness", true),
        ],
        run: temporal_fill,
        offsets: neighbour_offsets,
        ..BASE
    },
    NodeDef {
        id: "clean_plate",
        name: "Clean plate",
        category: NodeCategory::Cleanup,
        description: "The background alone: the median of nearby frames (aligned when the camera moves), so passers-by and moving objects disappear. Matte what should be ignored.",
        inputs: &["in", "matte"],
        params: &[
            ParamDef::int("range", "Frames each way", 1, 60, 24),
            ParamDef::int("step", "Frame step", 1, 10, 4),
            ParamDef::choice("camera", "Camera", &["Moving (align frames)", "Locked off"], 1),
        ],
        run: clean_plate,
        offsets: neighbour_offsets,
        ..BASE
    },
    NodeDef {
        id: "wire_remove",
        name: "Wire removal",
        category: NodeCategory::Cleanup,
        description: "Paints out a wire or rig along a line (or data.points polyline) from the picture on either side, keeping texture.",
        params: &[
            ParamDef::point("from", "From", [0.3, 0.2]),
            ParamDef::point("to", "To", [0.7, 0.8]),
            ParamDef::float("width", "Width", 0.5, 200.0, 6.0),
            ParamDef::float("softness", "Softness", 0.0, 50.0, 2.0),
            ParamDef::float("texture", "Restore texture", 0.0, 1.0, 0.7).unit(Unit::Fraction),
        ],
        data: "points: optional [[x,y], …] polyline in picture pixels instead of from/to",
        run: wire_remove,
        ..BASE
    },
    NodeDef {
        id: "paint",
        name: "Paint",
        category: NodeCategory::Cleanup,
        description: "Paint strokes that live over a range of frames: colour, clone (from an offset or another frame), reveal (from input B), erase and blur.",
        inputs: &["in", "reveal"],
        data: "strokes: [{tool: colour|clone|reveal|erase|blur, points: [[x,y],…], radius, softness 0..1, opacity, colour, offset: [dx,dy], frame_offset, from, to}] in picture pixels and seconds",
        run: paint,
        offsets: paint_offsets,
        animated: paint_animated,
        ..BASE
    },
    NodeDef {
        id: "sky_replace",
        name: "Sky replacement",
        category: NodeCategory::Cleanup,
        description: "Finds the sky (or uses a matte), refines its edge through hair and leaves, puts in the new sky and relights the foreground to match.",
        inputs: &["plate", "sky", "matte"],
        params: &[
            ParamDef::float("threshold", "Sky detection", 0.0, 1.0, 0.5).unit(Unit::Fraction),
            ParamDef::float("edge", "Edge refine", 0.0, 100.0, 8.0),
            ParamDef::float("feather", "Feather", 0.0, 50.0, 1.0),
            ParamDef::choice("fit", "Sky fit", &["Fit", "Fill", "Stretch", "None"], 1),
            ParamDef::float("harmonise", "Relight foreground", 0.0, 1.0, 0.5).unit(Unit::Fraction),
            ParamDef::float("wrap", "Light wrap", 0.0, 1.0, 0.3).unit(Unit::Fraction),
            ParamDef::float("haze", "Horizon haze", 0.0, 1.0, 0.15).unit(Unit::Fraction),
            ParamDef::choice("output", "Output", &["Comp", "Sky matte"], 0),
        ],
        run: sky_replace,
        ..BASE
    },
    NodeDef {
        id: "matte_refine",
        name: "Matte refine",
        category: NodeCategory::Matte,
        description: "Rebuilds a rough matte's edge from the picture's colours (hair, fur, leaves): a soft band is unmixed against local foreground and background colours.",
        inputs: &["in", "matte"],
        params: &[ParamDef::float("radius", "Edge band", 1.0, 200.0, 10.0), ParamDef::choice("output", "Output", super::nodes::MATTE_OUT, 0)],
        run: matte_refine,
        ..BASE
    },
];

fn neighbour_offsets(n: &Node) -> Vec<i32> {
    let p = n.params_at(0.0);
    let (range, step) = (p.i("range").max(1), p.i("step").max(1));
    // Nearest frames first: they look most like this one.
    let mut out = Vec::new();
    let mut k = step;
    while k <= range {
        out.push(-k);
        out.push(k);
        k += step;
    }
    out
}

/// A plane of 0/1 from a matte input's alpha, grown by `grow` pixels.
fn hole_of(m: &Plate, grow: f32) -> Vec<f32> {
    let a: Vec<f32> = m.alpha().iter().map(|&v| if v > 0.1 { 1.0 } else { 0.0 }).collect();
    geo::erode_plane(&a, m.width(), m.height(), -grow)
}

// ------------------------------------------------------- spatial inpainting

struct Level {
    w: usize,
    h: usize,
    px: Vec<Px>,
    hole: Vec<bool>,
}

fn down(l: &Level) -> Level {
    let (w, h) = ((l.w / 2).max(1), (l.h / 2).max(1));
    let mut px = vec![[0.0; 4]; w * h];
    let mut hole = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0.0f32; 4];
            let mut n = 0.0;
            let mut any = false;
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (sx, sy) = ((2 * x + dx).min(l.w - 1), (2 * y + dy).min(l.h - 1));
                let i = sy * l.w + sx;
                if l.hole[i] {
                    any = true;
                } else {
                    for c in 0..4 {
                        acc[c] += l.px[i][c];
                    }
                    n += 1.0;
                }
            }
            px[y * w + x] = if n > 0.0 { acc.map(|v| v / n) } else { [0.0; 4] };
            hole[y * w + x] = any;
        }
    }
    Level { w, h, px, hole }
}

/// Smooth fill: repeated averaging of the hole from its border inwards.
fn diffuse(l: &mut Level, iterations: usize) {
    let (w, h) = (l.w, l.h);
    // Start from the mean border colour so the result settles quickly.
    let mut sum = [0.0f32; 4];
    let mut n = 0.0;
    for i in 0..w * h {
        if !l.hole[i] {
            for c in 0..4 {
                sum[c] += l.px[i][c];
            }
            n += 1.0;
        }
    }
    let mean = if n > 0.0 { sum.map(|v| v / n) } else { [0.0; 4] };
    for i in 0..w * h {
        if l.hole[i] {
            l.px[i] = mean;
        }
    }
    for _ in 0..iterations {
        let src = l.px.clone();
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                if !l.hole[i] {
                    continue;
                }
                let mut acc = [0.0f32; 4];
                let mut k = 0.0f32;
                for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                    let (xx, yy) = (x as i32 + dx, y as i32 + dy);
                    if xx < 0 || yy < 0 || xx >= w as i32 || yy >= h as i32 {
                        continue;
                    }
                    let q = src[yy as usize * w + xx as usize];
                    for c in 0..4 {
                        acc[c] += q[c];
                    }
                    k += 1.0;
                }
                l.px[i] = acc.map(|v| v / k.max(1.0));
            }
        }
    }
}

fn patch_dist(l: &Level, a: (i32, i32), b: (i32, i32), half: i32, limit: f32) -> f32 {
    let mut d = 0.0f32;
    for dy in -half..=half {
        let (ay, by) = (a.1 + dy, b.1 + dy);
        if ay < 0 || ay >= l.h as i32 {
            continue;
        }
        for dx in -half..=half {
            let ax = a.0 + dx;
            if ax < 0 || ax >= l.w as i32 {
                continue;
            }
            let p = l.px[ay as usize * l.w + ax as usize];
            let q = l.px[by as usize * l.w + (b.0 + dx) as usize];
            d += (p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2) + 0.5 * (p[3] - q[3]).powi(2);
        }
        if d > limit {
            return d;
        }
    }
    d
}

/// Multi-scale exemplar fill of `hole` (true = fill) in `img`.
pub fn inpaint_image(img: &Image, hole: &[bool], patch: usize, iterations: usize) -> Image {
    let (w, h) = (img.width, img.height);
    if !hole.iter().any(|&b| b) {
        return img.clone();
    }
    let half = (patch / 2).max(1) as i32;
    let mut pyr = vec![Level { w, h, px: img.pixels.clone(), hole: hole.to_vec() }];
    while pyr.last().is_some_and(|l| l.w > 4 * patch && l.h > 4 * patch && l.hole.iter().filter(|&&b| b).count() > 64) {
        let next = down(pyr.last().unwrap());
        pyr.push(next);
    }
    let top = pyr.len() - 1;
    diffuse(&mut pyr[top], 200);
    let mut nnf: Vec<(i32, i32)> = Vec::new();
    for lvl in (0..pyr.len()).rev() {
        if lvl < top {
            // Up-sample the coarser fill into this level's hole.
            let (cw, ch) = (pyr[lvl + 1].w, pyr[lvl + 1].h);
            let coarse = pyr[lvl + 1].px.clone();
            let cimg = Image { width: cw, height: ch, pixels: coarse };
            let l = &mut pyr[lvl];
            for y in 0..l.h {
                for x in 0..l.w {
                    if l.hole[y * l.w + x] {
                        l.px[y * l.w + x] = cimg.sample((x as f32 + 0.5) * cw as f32 / l.w as f32, (y as f32 + 0.5) * ch as f32 / l.h as f32, Edge::Clamp);
                    }
                }
            }
        }
        let l = &mut pyr[lvl];
        let (lw, lh) = (l.w as i32, l.h as i32);
        // Valid source centres: whole patch outside the hole.
        let mut valid = vec![false; l.w * l.h];
        let hole_grown = geo::erode_plane(&l.hole.iter().map(|&b| if b { 1.0 } else { 0.0 }).collect::<Vec<_>>(), l.w, l.h, -(half as f32));
        for y in half..lh - half {
            for x in half..lw - half {
                valid[(y * lw + x) as usize] = hole_grown[(y * lw + x) as usize] < 0.5;
            }
        }
        let sources: Vec<(i32, i32)> = (0..l.w * l.h).filter(|&i| valid[i]).map(|i| ((i % l.w) as i32, (i / l.w) as i32)).collect();
        if sources.is_empty() {
            continue;
        }
        // Targets: centres whose patch touches the hole.
        let targets: Vec<(i32, i32)> = (0..l.w * l.h).filter(|&i| hole_grown[i] > 0.5).map(|i| ((i % l.w) as i32, (i / l.w) as i32)).collect();
        // Seed the field: from the coarser level's field, else spread.
        let prev = std::mem::take(&mut nnf);
        let (pw, _) = if lvl < top { (pyr[lvl + 1].w, pyr[lvl + 1].h) } else { (0, 0) };
        let l = &mut pyr[lvl];
        let mut field = vec![(0i32, 0i32); l.w * l.h];
        let mut rng = crate::math::Rng::new(lvl as u32 * 7919 + 13);
        for &(x, y) in &targets {
            let seed = if !prev.is_empty() {
                let (cx, cy) = ((x / 2).min(pw as i32 - 1), (y / 2).min((prev.len() / pw.max(1)) as i32 - 1));
                let s = prev[(cy as usize) * pw + cx as usize];
                (s.0 * 2 + (x & 1), s.1 * 2 + (y & 1))
            } else {
                sources[rng.below(sources.len())]
            };
            let s = if seed.0 >= half && seed.1 >= half && seed.0 < lw - half && seed.1 < lh - half && valid[(seed.1 * lw + seed.0) as usize] { seed } else { sources[rng.below(sources.len())] };
            field[(y * lw + x) as usize] = s;
        }
        let iters = if lvl == top { iterations + 3 } else { iterations };
        for it in 0..iters {
            // Search: keep, coherence from neighbours, local jitter, a few random.
            let order: Vec<(i32, i32)> = if it % 2 == 0 { targets.clone() } else { targets.iter().rev().copied().collect() };
            for &(x, y) in &order {
                let i = (y * lw + x) as usize;
                let mut best = field[i];
                let mut bd = patch_dist(l, (x, y), best, half, f32::INFINITY);
                let try_src = |c: (i32, i32), best: &mut (i32, i32), bd: &mut f32| {
                    if c.0 < half || c.1 < half || c.0 >= lw - half || c.1 >= lh - half || !valid[(c.1 * lw + c.0) as usize] {
                        return;
                    }
                    let d = patch_dist(l, (x, y), c, half, *bd);
                    if d < *bd {
                        *bd = d;
                        *best = c;
                    }
                };
                let step = if it % 2 == 0 { 1 } else { -1 };
                for (dx, dy) in [(-step, 0), (0, -step)] {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx >= 0 && ny >= 0 && nx < lw && ny < lh && hole_grown[(ny * lw + nx) as usize] > 0.5 {
                        let s = field[(ny * lw + nx) as usize];
                        try_src((s.0 - dx, s.1 - dy), &mut best, &mut bd);
                    }
                }
                if lvl == top && it == 0 {
                    // The coarsest level is small: look everywhere once.
                    for &c in &sources {
                        try_src(c, &mut best, &mut bd);
                    }
                } else {
                    // Finer levels refine the match found above them.
                    let c0 = best;
                    for dy in -2..=2 {
                        for dx in -2..=2 {
                            try_src((c0.0 + dx, c0.1 + dy), &mut best, &mut bd);
                        }
                    }
                }
                field[i] = best;
            }
            // Vote: each hole pixel averages the patches that cover it.
            let mut acc = vec![[0.0f32; 5]; l.w * l.h];
            for &(x, y) in &targets {
                let s = field[(y * lw + x) as usize];
                for dy in -half..=half {
                    for dx in -half..=half {
                        let (tx, ty) = (x + dx, y + dy);
                        if tx < 0 || ty < 0 || tx >= lw || ty >= lh || !l.hole[(ty * lw + tx) as usize] {
                            continue;
                        }
                        let q = l.px[((s.1 + dy) * lw + s.0 + dx) as usize];
                        let a = &mut acc[(ty * lw + tx) as usize];
                        for c in 0..4 {
                            a[c] += q[c];
                        }
                        a[4] += 1.0;
                    }
                }
            }
            for (i, a) in acc.iter().enumerate() {
                if a[4] > 0.0 {
                    l.px[i] = [a[0] / a[4], a[1] / a[4], a[2] / a[4], a[3] / a[4]];
                }
            }
        }
        nnf = field;
    }
    Image { width: w, height: h, pixels: pyr.swap_remove(0).px }
}

fn smooth_fill(img: &Image, hole: &[bool]) -> Image {
    let mut l = Level { w: img.width, h: img.height, px: img.pixels.clone(), hole: hole.to_vec() };
    // Coarse-to-fine diffusion converges fast at any hole size.
    let mut pyr = vec![];
    let mut cur = Level { w: l.w, h: l.h, px: l.px.clone(), hole: l.hole.clone() };
    while cur.w > 8 && cur.h > 8 {
        let next = down(&cur);
        pyr.push(cur);
        cur = next;
    }
    diffuse(&mut cur, 100);
    let mut filled = cur;
    while let Some(mut fine) = pyr.pop() {
        let cimg = Image { width: filled.w, height: filled.h, pixels: filled.px };
        for y in 0..fine.h {
            for x in 0..fine.w {
                if fine.hole[y * fine.w + x] {
                    fine.px[y * fine.w + x] = cimg.sample((x as f32 + 0.5) * cimg.width as f32 / fine.w as f32, (y as f32 + 0.5) * cimg.height as f32 / fine.h as f32, Edge::Clamp);
                }
            }
        }
        let (pxs, holes) = (fine.px.clone(), fine.hole.clone());
        let mut t = Level { w: fine.w, h: fine.h, px: pxs, hole: holes };
        diffuse_keep(&mut t, 20);
        filled = t;
    }
    l.px = filled.px;
    Image { width: l.w, height: l.h, pixels: l.px }
}

/// Averaging passes that keep the current fill as the start.
fn diffuse_keep(l: &mut Level, iterations: usize) {
    let (w, h) = (l.w, l.h);
    for _ in 0..iterations {
        let src = l.px.clone();
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                if !l.hole[i] {
                    continue;
                }
                let mut acc = [0.0f32; 4];
                let mut k = 0.0f32;
                for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                    let (xx, yy) = (x as i32 + dx, y as i32 + dy);
                    if xx >= 0 && yy >= 0 && xx < w as i32 && yy < h as i32 {
                        let q = src[yy as usize * w + xx as usize];
                        for c in 0..4 {
                            acc[c] += q[c];
                        }
                        k += 1.0;
                    }
                }
                l.px[i] = acc.map(|v| v / k.max(1.0));
            }
        }
    }
}

/// Blends `filled` into `orig` by a soft matte.
fn blend_fill(orig: &Image, filled: &Image, soft: &[f32]) -> Image {
    orig.map_xy(|x, y, q| crate::image::lerp_px(q, filled.get(x, y), soft[y * orig.width + x].clamp(0.0, 1.0)))
}

fn inpaint(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], p: &ParamSet) -> Result<Plate, String> {
    let src = main_or_blank(ctx, inputs);
    let Some(m) = slot(inputs, 1) else { return Ok(src) };
    let grow = p.f("grow");
    let hole = hole_of(m, grow);
    let mask: Vec<bool> = hole.iter().map(|&v| v > 0.5).collect();
    let filled = if p.choice("method") == 1 { smooth_fill(&src.rgba, &mask) } else { inpaint_image(&src.rgba, &mask, p.i("patch") as usize, p.i("iterations") as usize) };
    let soft = geo::blur_plane(&hole, ctx.width, ctx.height, 1.0);
    Ok(src.with_rgba(blend_fill(&src.rgba, &filled, &soft)))
}

// ------------------------------------------------------------ temporal fill

/// The homography taking `from` (another frame) onto `to` (this frame),
/// from corners tracked outside both mattes.
pub fn align(from: &Image, to: &Image, ignore_to: &[f32], ignore_from: Option<&[f32]>) -> Option<geo::Mat3> {
    let (w, h) = (to.width, to.height);
    let (gt, gf) = (Gray::from_image(to), Gray::from_image(from));
    let levels = ((w.min(h) as f32 / 24.0).log2().floor() as usize).clamp(1, 5);
    let (pt, pf) = (Pyramid::new(&gt, levels), Pyramid::new(&gf, levels));
    let corners: Vec<(f32, f32, f32)> = fast_corners(&gt, 0.03)
        .into_iter()
        .filter(|c| {
            let i = (c.1 as usize).min(h - 1) * w + (c.0 as usize).min(w - 1);
            ignore_to[i] < 0.5
        })
        .collect();
    let pts = spread_corners(corners, w, h, 400);
    let mut pairs = Vec::new();
    for p in pts {
        let Some(t) = klt(&pt, &pf, p, [0.0, 0.0], 6) else { continue };
        if t.residual > 0.08 {
            continue;
        }
        let q = t.position;
        if let Some(ig) = ignore_from {
            let i = (q[1] as usize).min(h - 1) * w + (q[0] as usize).min(w - 1);
            if ig[i] > 0.5 {
                continue;
            }
        }
        // Pairs map the other frame onto this one.
        pairs.push(([q[0] as f64, q[1] as f64], [p[0] as f64, p[1] as f64]));
    }
    if pairs.len() < 12 {
        return None;
    }
    let (m, n) = geo::ransac_homography(&pairs, 1.5, 400, 11)?;
    (n >= 10 && n * 2 >= pairs.len()).then_some(m)
}

/// Aligned versions of the neighbouring frames: their pictures warped onto
/// this frame and a "usable" plane (inside the frame, outside their own
/// matte).
fn aligned_neighbours(ctx: &NodeCtx, cur: &Plate, hole_now: &[f32], grow: f32, locked: bool) -> Vec<(i32, Image, Vec<f32>)> {
    let (w, h) = (ctx.width, ctx.height);
    let mut out = Vec::new();
    for (o, at) in &ctx.neighbours {
        let Some(img) = at.first().cloned().flatten() else { continue };
        let their_hole = at.get(1).cloned().flatten().map(|m| hole_of(&m, grow));
        let m = if locked {
            geo::IDENTITY
        } else {
            match align(&img.rgba, &cur.rgba, hole_now, their_hole.as_deref()) {
                Some(m) => m,
                None => continue,
            }
        };
        // Output (this frame) → their pixels.
        let Some(inv) = geo::inverse3(&m) else { continue };
        let warped = geo::warp_h(&img.rgba, &inv, Edge::Transparent);
        let mut usable = vec![0.0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                let s = geo::apply_h(&inv, [x as f64 + 0.5, y as f64 + 0.5]);
                let inside = s[0] >= 0.5 && s[1] >= 0.5 && s[0] <= w as f64 - 0.5 && s[1] <= h as f64 - 0.5;
                if !inside {
                    continue;
                }
                let blocked = their_hole.as_ref().is_some_and(|hh| geo::sample_plane(hh, w, h, s[0] as f32, s[1] as f32) > 0.1);
                usable[y * w + x] = if blocked { 0.0 } else { 1.0 };
            }
        }
        out.push((*o, warped, usable));
    }
    out
}

/// Per-channel median of the samples.
fn median_px(samples: &mut [Px]) -> Px {
    let n = samples.len();
    let mut out = [0.0; 4];
    let mut v: Vec<f32> = Vec::with_capacity(n);
    for (c, o) in out.iter_mut().enumerate() {
        v.clear();
        v.extend(samples.iter().map(|s| s[c]));
        v.sort_by(f32::total_cmp);
        *o = if n % 2 == 1 { v[n / 2] } else { 0.5 * (v[n / 2 - 1] + v[n / 2]) };
    }
    out
}

fn temporal_fill(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], p: &ParamSet) -> Result<Plate, String> {
    let src = main_or_blank(ctx, inputs);
    let Some(m) = slot(inputs, 1) else { return Ok(src) };
    let (w, h) = (ctx.width, ctx.height);
    let grow = p.f("grow");
    let hole = hole_of(m, grow);
    let locked = p.choice("camera") == 1;
    let mut neigh = aligned_neighbours(ctx, &src, &hole, grow, locked);
    // Brightness: scale each neighbour to this frame in a ring around the hole.
    if p.b("match_light") {
        let ring: Vec<f32> = geo::erode_plane(&hole, w, h, -12.0).iter().zip(&hole).map(|(g, h)| if *g > 0.5 && *h < 0.5 { 1.0 } else { 0.0 }).collect();
        for (_, img, usable) in neigh.iter_mut() {
            let (mut a, mut b) = ([0.0f64; 3], [0.0f64; 3]);
            for i in 0..w * h {
                if ring[i] > 0.5 && usable[i] > 0.5 {
                    for c in 0..3 {
                        a[c] += src.rgba.pixels[i][c] as f64;
                        b[c] += img.pixels[i][c] as f64;
                    }
                }
            }
            let k: [f32; 3] = std::array::from_fn(|c| if b[c] > 1e-6 { (a[c] / b[c]).clamp(0.5, 2.0) as f32 } else { 1.0 });
            for q in img.pixels.iter_mut() {
                for c in 0..3 {
                    q[c] *= k[c];
                }
            }
        }
    }
    let mut out = src.rgba.clone();
    let mut missing = vec![false; w * h];
    let mut samples: Vec<Px> = Vec::with_capacity(neigh.len());
    for i in 0..w * h {
        if hole[i] < 0.5 {
            continue;
        }
        samples.clear();
        for (_, img, usable) in &neigh {
            if usable[i] > 0.5 {
                samples.push(img.pixels[i]);
            }
        }
        if samples.is_empty() {
            missing[i] = true;
        } else {
            out.pixels[i] = median_px(&mut samples);
        }
    }
    if missing.iter().any(|&b| b) && p.b("fill_rest") {
        out = inpaint_image(&out, &missing, 7, 4);
    }
    let soft = geo::blur_plane(&hole, w, h, 1.0);
    Ok(src.with_rgba(blend_fill(&src.rgba, &out, &soft)))
}

fn clean_plate(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], p: &ParamSet) -> Result<Plate, String> {
    let src = main_or_blank(ctx, inputs);
    let (w, h) = (ctx.width, ctx.height);
    let own = slot(inputs, 1).map(|m| hole_of(m, 2.0)).unwrap_or_else(|| vec![0.0; w * h]);
    let neigh = aligned_neighbours(ctx, &src, &own, 2.0, p.choice("camera") == 1);
    let mut samples: Vec<Px> = Vec::with_capacity(neigh.len() + 1);
    let rgba = Image::from_fn(w, h, |x, y| {
        let i = y * w + x;
        let mut s: Vec<Px> = Vec::with_capacity(neigh.len() + 1);
        if own[i] < 0.5 {
            s.push(src.rgba.pixels[i]);
        }
        for (_, img, usable) in &neigh {
            if usable[i] > 0.5 {
                s.push(img.pixels[i]);
            }
        }
        if s.is_empty() { src.rgba.pixels[i] } else { median_px(&mut s) }
    });
    samples.clear();
    Ok(src.with_rgba(rgba))
}

// ------------------------------------------------------------- wire removal

/// Distance from `p` to segment `a`–`b`, the position along it (0..1) and
/// the unit normal.
fn seg(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> (f32, f32, [f32; 2], f32) {
    let d = [b[0] - a[0], b[1] - a[1]];
    let len = (d[0] * d[0] + d[1] * d[1]).sqrt().max(1e-6);
    let u = [d[0] / len, d[1] / len];
    let n = [-u[1], u[0]];
    let q = [p[0] - a[0], p[1] - a[1]];
    let along = (q[0] * u[0] + q[1] * u[1]) / len;
    let side = q[0] * n[0] + q[1] * n[1];
    let t = along.clamp(0.0, 1.0);
    let c = [a[0] + d[0] * t, a[1] + d[1] * t];
    (((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2)).sqrt(), along, n, side)
}

fn wire_remove(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], p: &ParamSet) -> Result<Plate, String> {
    let src = main_or_blank(ctx, inputs);
    let (w, h) = (ctx.width as f32, ctx.height as f32);
    let pts: Vec<[f32; 2]> = match ctx.node.data.get("points").and_then(Json::as_array) {
        Some(a) if a.len() >= 2 => a.iter().filter_map(|q| Some([q.get(0)?.as_f64()? as f32, q.get(1)?.as_f64()? as f32])).collect(),
        _ => {
            let (a, b) = (p.point("from"), p.point("to"));
            vec![[a[0] * w, a[1] * h], [b[0] * w, b[1] * h]]
        }
    };
    let half = p.f("width") * 0.5;
    let soft = p.f("softness");
    let tex = p.f("texture");
    let img = &src.rgba;
    let rgba = img.map_xy(|x, y, q| {
        let pp = [x as f32 + 0.5, y as f32 + 0.5];
        // Nearest segment of the polyline.
        let mut best: Option<(f32, [f32; 2], f32)> = None;
        for s in pts.windows(2) {
            let (d, _, n, side) = seg(pp, s[0], s[1]);
            if best.is_none_or(|b| d < b.0) {
                best = Some((d, n, side));
            }
        }
        let Some((d, n, side)) = best else { return q };
        if d > half + soft + 0.5 {
            return q;
        }
        let k = 1.0 - smoothstep(half, half + soft.max(0.5), d);
        let edge = half + soft + 2.0;
        // Colours just outside the wire on both sides, across the normal.
        let pa = [pp[0] - n[0] * (side + edge), pp[1] - n[1] * (side + edge)];
        let pb = [pp[0] + n[0] * (edge - side), pp[1] + n[1] * (edge - side)];
        let ca = img.sample(pa[0], pa[1], Edge::Clamp);
        let cb = img.sample(pb[0], pb[1], Edge::Clamp);
        let t = ((side + edge) / (2.0 * edge)).clamp(0.0, 1.0);
        let mut fill: Px = std::array::from_fn(|c| ca[c] + (cb[c] - ca[c]) * t);
        if tex > 0.0 {
            // Fine texture borrowed from a strip beside the wire.
            let off = 2.0 * edge + 2.0;
            let s = [pp[0] + n[0] * off, pp[1] + n[1] * off];
            let c0 = img.sample(s[0], s[1], Edge::Mirror);
            let mut avg = [0.0f32; 4];
            for (dx, dy) in [(-1.5f32, 0.0f32), (1.5, 0.0), (0.0, -1.5), (0.0, 1.5)] {
                let v = img.sample(s[0] + dx, s[1] + dy, Edge::Mirror);
                for c in 0..4 {
                    avg[c] += 0.25 * v[c];
                }
            }
            for c in 0..3 {
                fill[c] = (fill[c] + tex * (c0[c] - avg[c])).max(0.0);
            }
        }
        crate::image::lerp_px(q, fill, k)
    });
    Ok(src.with_rgba(rgba))
}

// --------------------------------------------------------------------- paint

/// One paint stroke.
#[derive(Clone, Debug)]
pub struct Stroke {
    pub tool: String,
    pub points: Vec<[f32; 2]>,
    pub radius: f32,
    pub softness: f32,
    pub opacity: f32,
    pub colour: [f32; 4],
    pub offset: [f32; 2],
    pub frame_offset: i32,
    pub from: Option<f64>,
    pub to: Option<f64>,
}

/// The strokes in a paint node's data.
pub fn strokes(data: &Json) -> Vec<Stroke> {
    let Some(a) = data.get("strokes").and_then(Json::as_array) else { return Vec::new() };
    a.iter()
        .map(|s| {
            let f = |k: &str, d: f64| s.get(k).and_then(Json::as_f64).unwrap_or(d) as f32;
            let colour = match s.get("colour") {
                Some(Json::String(h)) => super::hex_colour(h).unwrap_or([1.0; 4]),
                Some(Json::Array(c)) if c.len() >= 3 => std::array::from_fn(|i| c.get(i).and_then(Json::as_f64).unwrap_or(1.0) as f32),
                _ => [1.0, 1.0, 1.0, 1.0],
            };
            let pair = |k: &str| s.get(k).and_then(Json::as_array).map(|a| [a.first().and_then(Json::as_f64).unwrap_or(0.0) as f32, a.get(1).and_then(Json::as_f64).unwrap_or(0.0) as f32]);
            Stroke {
                tool: s.get("tool").and_then(Json::as_str).unwrap_or("colour").to_owned(),
                points: s.get("points").and_then(Json::as_array).map(|p| p.iter().filter_map(|q| Some([q.get(0)?.as_f64()? as f32, q.get(1)?.as_f64()? as f32])).collect()).unwrap_or_default(),
                radius: f("radius", 10.0).max(0.5),
                softness: f("softness", 0.5).clamp(0.0, 1.0),
                opacity: f("opacity", 1.0).clamp(0.0, 1.0),
                colour,
                offset: pair("offset").unwrap_or([0.0, 0.0]),
                frame_offset: s.get("frame_offset").and_then(Json::as_i64).unwrap_or(0) as i32,
                from: s.get("from").and_then(Json::as_f64),
                to: s.get("to").and_then(Json::as_f64),
            }
        })
        .collect()
}

fn paint_offsets(n: &Node) -> Vec<i32> {
    let mut o: Vec<i32> = strokes(&n.data).iter().filter(|s| s.tool == "clone" && s.frame_offset != 0).map(|s| s.frame_offset).collect();
    o.sort_unstable();
    o.dedup();
    o
}

fn paint_animated(n: &Node) -> bool {
    strokes(&n.data).iter().any(|s| s.from.is_some() || s.to.is_some())
}

fn stroke_cover(s: &Stroke, x: f32, y: f32) -> f32 {
    let mut d = f32::INFINITY;
    if s.points.len() == 1 {
        let p = s.points[0];
        d = ((x - p[0]).powi(2) + (y - p[1]).powi(2)).sqrt();
    }
    for seg_pts in s.points.windows(2) {
        d = d.min(seg([x, y], seg_pts[0], seg_pts[1]).0);
    }
    let inner = s.radius * (1.0 - s.softness);
    s.opacity * (1.0 - smoothstep(inner, s.radius.max(inner + 1e-3), d))
}

fn paint(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], _: &ParamSet) -> Result<Plate, String> {
    let src = main_or_blank(ctx, inputs);
    let reveal = slot(inputs, 1);
    let (w, h) = (ctx.width, ctx.height);
    let mut img = src.rgba.clone();
    for s in strokes(&ctx.node.data) {
        if s.from.is_some_and(|f| ctx.time < f - 1e-6) || s.to.is_some_and(|e| ctx.time >= e - 1e-6) || s.points.is_empty() {
            continue;
        }
        let pad = s.radius + 1.0;
        let x0 = (s.points.iter().map(|p| p[0]).fold(f32::INFINITY, f32::min) - pad).max(0.0) as usize;
        let x1 = ((s.points.iter().map(|p| p[0]).fold(f32::NEG_INFINITY, f32::max) + pad).max(0.0) as usize).min(w);
        let y0 = (s.points.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min) - pad).max(0.0) as usize;
        let y1 = ((s.points.iter().map(|p| p[1]).fold(f32::NEG_INFINITY, f32::max) + pad).max(0.0) as usize).min(h);
        let clone_src: Option<Arc<Plate>> = if s.tool == "clone" && s.frame_offset != 0 {
            ctx.neighbours.iter().find(|(o, _)| *o == s.frame_offset).and_then(|(_, at)| at.first().cloned().flatten())
        } else {
            None
        };
        let before = img.clone();
        let blurred = if s.tool == "blur" { Some(crate::ops::gaussian_blur(&before, s.radius * 0.3, s.radius * 0.3, Edge::Clamp)) } else { None };
        let col = premultiply([s.colour[0], s.colour[1], s.colour[2]], 1.0);
        for y in y0..y1 {
            for x in x0..x1 {
                let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                let k = stroke_cover(&s, fx, fy) * if s.tool == "colour" { s.colour[3] } else { 1.0 };
                if k <= 0.0 {
                    continue;
                }
                let q = before.get(x, y);
                let target: Px = match s.tool.as_str() {
                    "clone" => {
                        let from = clone_src.as_ref().map_or(&src.rgba, |p| &p.rgba);
                        from.sample(fx - s.offset[0], fy - s.offset[1], Edge::Clamp)
                    }
                    "reveal" => reveal.map_or(q, |r| r.rgba.get(x, y)),
                    "erase" => [0.0; 4],
                    "blur" => blurred.as_ref().map_or(q, |b| b.get(x, y)),
                    _ => col,
                };
                img.set(x, y, crate::image::lerp_px(q, target, k));
            }
        }
    }
    Ok(src.with_rgba(img))
}

// ---------------------------------------------------------- sky and mattes

/// Sky likelihood from colour and texture, kept only where connected to
/// the top of the frame.
pub fn detect_sky(img: &Image, sensitivity: f32) -> Vec<f32> {
    let (w, h) = (img.width, img.height);
    let enc: Vec<[f32; 3]> = img.pixels.iter().map(|q| straight(*q).0.map(|v| linear_to_srgb(v.clamp(0.0, 1.0)))).collect();
    let lum: Vec<f32> = enc.iter().map(|e| 0.2126 * e[0] + 0.7152 * e[1] + 0.0722 * e[2]).collect();
    let mean = geo::blur_plane(&lum, w, h, 3.0);
    let sq: Vec<f32> = lum.iter().map(|v| v * v).collect();
    let mean_sq = geo::blur_plane(&sq, w, h, 3.0);
    let thr = 0.25 + 0.5 * (1.0 - sensitivity.clamp(0.0, 1.0));
    let score: Vec<f32> = (0..w * h)
        .map(|i| {
            let e = enc[i];
            let var = (mean_sq[i] - mean[i] * mean[i]).max(0.0).sqrt();
            let smooth = 1.0 - smoothstep(0.01, 0.06, var);
            let bright = smoothstep(0.25, 0.6, lum[i]);
            let bluish = smoothstep(-0.05, 0.12, e[2] - 0.5 * (e[0] + e[1]));
            let greyish = 1.0 - smoothstep(0.03, 0.15, (e[0] - e[1]).abs() + (e[1] - e[2]).abs());
            smooth * bright * bluish.max(greyish * 0.8)
        })
        .collect();
    // Flood from the top row through likely sky.
    let mut sky = vec![0.0f32; w * h];
    let mut stack: Vec<usize> = (0..w).filter(|&x| score[x] > thr).collect();
    for &i in &stack {
        sky[i] = 1.0;
    }
    while let Some(i) = stack.pop() {
        let (x, y) = (i % w, i / w);
        let mut push = |j: usize| {
            if sky[j] == 0.0 && score[j] > thr * 0.85 {
                sky[j] = 1.0;
                stack.push(j);
            }
        };
        if x > 0 {
            push(i - 1);
        }
        if x + 1 < w {
            push(i + 1);
        }
        if y > 0 {
            push(i - w);
        }
        if y + 1 < h {
            push(i + w);
        }
    }
    sky
}

/// Rebuilds a matte's edge from colour: inside a band around the edge each
/// pixel's coverage is how much nearer it is to the local foreground colour
/// than to the local background colour.
pub fn refine_matte(img: &Image, matte: &[f32], radius: f32) -> Vec<f32> {
    let (w, h) = (img.width, img.height);
    let c: Vec<[f32; 3]> = img.pixels.iter().map(|q| straight(*q).0).collect();
    let hard: Vec<f32> = matte.iter().map(|&v| if v > 0.5 { 1.0 } else { 0.0 }).collect();
    let inner = geo::erode_plane(&hard, w, h, radius * 0.5);
    let outer = geo::erode_plane(&hard, w, h, -radius * 0.5);
    let wf: Vec<f32> = inner.clone();
    let wb: Vec<f32> = outer.iter().map(|v| 1.0 - v).collect();
    let blur = |p: &[f32]| geo::blur_plane(p, w, h, radius);
    let (bf, bb) = (blur(&wf), blur(&wb));
    let mut fgm = [Vec::new(), Vec::new(), Vec::new()];
    let mut bgm = [Vec::new(), Vec::new(), Vec::new()];
    for k in 0..3 {
        let pf: Vec<f32> = (0..w * h).map(|i| c[i][k] * wf[i]).collect();
        let pb: Vec<f32> = (0..w * h).map(|i| c[i][k] * wb[i]).collect();
        fgm[k] = blur(&pf);
        bgm[k] = blur(&pb);
    }
    (0..w * h)
        .map(|i| {
            if inner[i] > 0.5 {
                return 1.0;
            }
            if outer[i] < 0.5 {
                return 0.0;
            }
            if bf[i] < 1e-3 || bb[i] < 1e-3 {
                return matte[i];
            }
            let f: [f32; 3] = std::array::from_fn(|k| fgm[k][i] / bf[i]);
            let b: [f32; 3] = std::array::from_fn(|k| bgm[k][i] / bb[i]);
            // Project the pixel onto the line from background to foreground.
            let d: [f32; 3] = std::array::from_fn(|k| f[k] - b[k]);
            let dd = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
            if dd < 1e-5 {
                return matte[i];
            }
            let t = ((c[i][0] - b[0]) * d[0] + (c[i][1] - b[1]) * d[1] + (c[i][2] - b[2]) * d[2]) / dd;
            t.clamp(0.0, 1.0)
        })
        .collect()
}

fn matte_refine(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], p: &ParamSet) -> Result<Plate, String> {
    let src = main_or_blank(ctx, inputs);
    let m = slot(inputs, 1).map(|m| m.alpha()).unwrap_or_else(|| src.alpha());
    let r = refine_matte(&src.rgba, &m, p.f("radius"));
    // The input's own alpha is the matte when none is plugged in.
    let base = if slot(inputs, 1).is_none() { src.with_rgba(src.rgba.map(|q| premultiply(straight(q).0, 1.0))) } else { src };
    Ok(super::nodes::matte_output(&base, &r, p.choice("output")))
}

fn sky_replace(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], p: &ParamSet) -> Result<Plate, String> {
    let plate = main_or_blank(ctx, inputs);
    let (w, h) = (ctx.width, ctx.height);
    let rough = match slot(inputs, 2) {
        Some(m) => m.alpha(),
        None => detect_sky(&plate.rgba, p.f("threshold")),
    };
    let edge = p.f("edge");
    let mut m = if edge > 0.5 { refine_matte(&plate.rgba, &rough, edge) } else { rough };
    m = geo::blur_plane(&m, w, h, p.f("feather"));
    if p.choice("output") == 1 {
        return Ok(plate.with_rgba(Image::from_fn(w, h, |x, y| {
            let v = m[y * w + x];
            [v, v, v, 1.0]
        })));
    }
    let Some(sky) = slot(inputs, 1) else { return Err("sky replacement needs a sky (input 2)".into()) };
    let sky = fit_into(&sky.rgba, w, h, p.choice("fit"));
    // Old and new sky colour, to relight the foreground.
    let mean = |img: &Image, wt: &dyn Fn(usize) -> f32| -> [f32; 3] {
        let (mut s, mut n) = ([0.0f64; 3], 0.0f64);
        for i in 0..w * h {
            let k = wt(i) as f64;
            if k <= 0.0 {
                continue;
            }
            let c = straight(img.pixels[i]).0;
            for j in 0..3 {
                s[j] += c[j] as f64 * k;
            }
            n += k;
        }
        if n < 1.0 { [0.5; 3] } else { s.map(|v| (v / n) as f32) }
    };
    let old = mean(&plate.rgba, &|i| m[i]);
    let new = mean(&sky, &|i| m[i].max(0.05));
    let harm = p.f("harmonise");
    let gain: [f32; 3] = std::array::from_fn(|k| ((new[k] / old[k].max(1e-3)).clamp(0.5, 2.0)).powf(harm * 0.5));
    let haze = p.f("haze");
    let near_sky = geo::blur_plane(&m, w, h, 0.04 * h as f32);
    let wrap = p.f("wrap");
    let sky_soft = crate::ops::gaussian_blur(&sky, 0.02 * h as f32, 0.02 * h as f32, Edge::Clamp);
    let rgba = Image::from_fn(w, h, |x, y| {
        let i = y * w + x;
        let k = m[i].clamp(0.0, 1.0);
        let (c, a) = straight(plate.rgba.pixels[i]);
        let mut f: [f32; 3] = std::array::from_fn(|j| c[j] * gain[j]);
        let hz = haze * near_sky[i] * (1.0 - k);
        let wr = wrap * near_sky[i] * (1.0 - k);
        let ss = straight(sky_soft.pixels[i]).0;
        for j in 0..3 {
            f[j] = f[j] + (new[j] - f[j]) * hz + ss[j] * wr * 0.5;
        }
        let fg = premultiply(f, a);
        let s = sky.pixels[i];
        std::array::from_fn(|j| s[j] * k + fg[j] * (1.0 - k))
    });
    Ok(plate.with_rgba(rgba))
}
