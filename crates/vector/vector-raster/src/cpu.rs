// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The CPU renderer. Coverage is the exact area of each pixel inside the
//! path: every line segment adds, to each pixel of each row it crosses,
//! its height in that row times the part of the pixel to its right
//! (integrated exactly along the segment), and a running sum along the
//! row turns those into signed winding area. The fill rule then folds the
//! winding: non-zero clamps it, even-odd reflects it.
//!
//! Rows are rendered in bands, in parallel, each band compositing every
//! draw and isolated group in order into its own floating-point buffer.

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::blend::blend;
use crate::filter;
use crate::prepare::{IRect, Lines, PGroup, PNode, Prepass, Prepared};
use crate::scene::{BlendMode, FillRule, MaskKind};
use crate::shade::{self, PrepImage};

/// Rows per band.
const BAND: i32 = 32;

#[inline]
fn phi(u: f64) -> f64 {
    if u <= 0.0 {
        0.0
    } else if u >= 1.0 {
        u - 0.5
    } else {
        0.5 * u * u
    }
}

/// Adds segment `s` to the accumulation buffer of rect `r` (`w + 1`
/// entries per row). Entries hold differences along the row; a running
/// sum gives the winding area of each pixel.
#[inline]
pub(crate) fn accumulate(acc: &mut [f32], r: IRect, s: [f32; 4]) {
    let (x0, y0, x1, y1) = (s[0] as f64, s[1] as f64, s[2] as f64, s[3] as f64);
    if y0 == y1 {
        return;
    }
    let (sign, xa, ya, xb, yb) = if y1 > y0 { (1.0, x0, y0, x1, y1) } else { (-1.0, x1, y1, x0, y0) };
    let ys = ya.max(r.y0 as f64);
    let ye = yb.min(r.y1 as f64);
    if ys >= ye {
        return;
    }
    let w = r.width() as usize;
    let stride = w + 1;
    let dxdy = (xb - xa) / (yb - ya);
    let rx0 = r.x0 as f64;
    let mut row = ys.floor();
    while row < ye {
        let ry0 = ys.max(row);
        let ry1 = ye.min(row + 1.0);
        let dy = (ry1 - ry0) * sign;
        if dy != 0.0 {
            let xs = xa + (ry0 - ya) * dxdy - rx0;
            let xe = xa + (ry1 - ya) * dxdy - rx0;
            let (lo, hi) = if xs < xe { (xs, xe) } else { (xe, xs) };
            let base = (row as i32 - r.y0) as usize * stride;
            let racc = &mut acc[base..base + stride];
            if hi <= 0.0 {
                racc[0] += dy as f32;
            } else if lo < w as f64 {
                let c0 = lo.floor();
                let c1 = hi.floor();
                if c0 == c1 {
                    // Within one column: the part right of the segment's middle.
                    let cov = c0 + 1.0 - 0.5 * (lo + hi);
                    add(racc, c0 as i64, dy * cov, w);
                    add(racc, c0 as i64 + 1, dy * (1.0 - cov), w);
                } else {
                    let inv = 1.0 / (hi - lo);
                    let g = |c: f64| (phi(c + 1.0 - lo) - phi(c + 1.0 - hi)) * inv;
                    let mut prev = 0.0;
                    let mut c = c0;
                    if c < 0.0 {
                        prev = g(-1.0);
                        racc[0] += (dy * prev) as f32;
                        c = 0.0;
                    }
                    let last = c1.min(w as f64 - 1.0);
                    while c <= last {
                        let gc = g(c);
                        racc[c as usize] += (dy * (gc - prev)) as f32;
                        prev = gc;
                        c += 1.0;
                    }
                    if c1 < w as f64 - 1.0 {
                        add(racc, c1 as i64 + 1, dy * (1.0 - prev), w);
                    }
                }
            }
        }
        row += 1.0;
    }
}

#[inline]
fn add(racc: &mut [f32], c: i64, v: f64, w: usize) {
    if c < 0 {
        racc[0] += v as f32;
    } else if (c as usize) < w {
        racc[c as usize] += v as f32;
    }
}

#[inline]
pub(crate) fn fold(rule: FillRule, w: f32) -> f32 {
    match rule {
        FillRule::NonZero => w.abs().min(1.0),
        FillRule::EvenOdd => {
            let t = w.abs() % 2.0;
            if t > 1.0 { 2.0 - t } else { t }
        }
    }
}

/// Coverage of `lines` over `r`, row-major, `0..=1`.
pub(crate) fn coverage_into(lines: &Lines, r: IRect, acc: &mut Vec<f32>, out: &mut Vec<f32>) {
    let (w, h) = (r.width() as usize, r.height() as usize);
    acc.clear();
    acc.resize((w + 1) * h, 0.0);
    for s in &lines.segs {
        if s[1].max(s[3]) <= r.y0 as f32 || s[1].min(s[3]) >= r.y1 as f32 || s[0].min(s[2]) >= r.x1 as f32 {
            continue;
        }
        accumulate(acc, r, *s);
    }
    out.clear();
    out.resize(w * h, 0.0);
    for y in 0..h {
        let mut sum = 0.0f32;
        let a = &acc[y * (w + 1)..y * (w + 1) + w];
        let o = &mut out[y * w..y * w + w];
        for x in 0..w {
            sum += a[x];
            let c = fold(lines.rule, sum);
            o[x] = if lines.invert { 1.0 - c } else { c };
        }
    }
}

pub(crate) fn coverage_full(lines: &Lines, r: IRect) -> Vec<f32> {
    let (mut acc, mut out) = (Vec::new(), Vec::new());
    coverage_into(lines, r, &mut acc, &mut out);
    out
}

/// A band's pixels: `rect` of premultiplied colours.
struct Buf {
    rect: IRect,
    px: Vec<[f32; 4]>,
}

impl Buf {
    fn at(&mut self, x: i32, y: i32) -> &mut [f32; 4] {
        let w = self.rect.width();
        &mut self.px[((y - self.rect.y0) * w + (x - self.rect.x0)) as usize]
    }
}

/// Per-thread reusable memory.
#[derive(Default)]
struct Scratch {
    acc: Vec<f32>,
    cov: Vec<f32>,
    cov2: Vec<f32>,
    bufs: Vec<Vec<[f32; 4]>>,
}

struct Ctx<'a> {
    pre: &'a [PrepImage<'a>],
}

fn draw_group(g: &PGroup, ctx: &Ctx, buf: &mut Buf, sc: &mut Scratch) {
    for child in &g.children {
        match child {
            PNode::Draw(d) => {
                let r = d.lines.bbox.intersect(&buf.rect);
                if r.is_empty() {
                    continue;
                }
                let mut cov = std::mem::take(&mut sc.cov);
                coverage_into(&d.lines, r, &mut sc.acc, &mut cov);
                let w = r.width() as usize;
                for y in r.y0..r.y1 {
                    let row = &cov[(y - r.y0) as usize * w..(y - r.y0) as usize * w + w];
                    let fy = y as f32 + 0.5;
                    for (i, &c) in row.iter().enumerate() {
                        if c <= 0.0 {
                            continue;
                        }
                        let x = r.x0 + i as i32;
                        let col = shade::eval(&d.paint, x as f32 + 0.5, fy, ctx.pre);
                        let k = c * d.opacity;
                        let s = [col[0] * k, col[1] * k, col[2] * k, col[3] * k];
                        if s[3] <= 0.0 && d.blend != BlendMode::Plus {
                            continue;
                        }
                        let p = buf.at(x, y);
                        *p = blend(*p, s, d.blend);
                    }
                }
                sc.cov = cov;
            }
            PNode::Group(sub) => {
                let r = sub.bbox.intersect(&buf.rect);
                if r.is_empty() {
                    continue;
                }
                let n = r.width() as usize * r.height() as usize;
                let mut px = sc.bufs.pop().unwrap_or_default();
                px.clear();
                px.resize(n, [0.0; 4]);
                let mut tmp = Buf { rect: r, px };
                draw_group(sub, ctx, &mut tmp, sc);
                // Clip and mask scale the group's pixels before it is combined.
                let w = r.width() as usize;
                let mut clip = std::mem::take(&mut sc.cov2);
                if let Some(cl) = &sub.clip {
                    coverage_into(cl, r, &mut sc.acc, &mut clip);
                }
                for y in r.y0..r.y1 {
                    for x in r.x0..r.x1 {
                        let i = (y - r.y0) as usize * w + (x - r.x0) as usize;
                        let mut k = sub.opacity;
                        if sub.clip.is_some() {
                            k *= clip[i];
                        }
                        if let Some(m) = &sub.mask {
                            k *= mask_value(&ctx.pre[m.prepass], m.kind, m.invert, x, y);
                        }
                        if k <= 0.0 {
                            continue;
                        }
                        let s = tmp.px[i];
                        let s = [s[0] * k, s[1] * k, s[2] * k, s[3] * k];
                        let p = buf.at(x, y);
                        *p = blend(*p, s, sub.blend);
                    }
                }
                sc.cov2 = clip;
                sc.bufs.push(tmp.px);
            }
        }
    }
}

pub(crate) fn mask_value(img: &PrepImage, kind: MaskKind, invert: bool, x: i32, y: i32) -> f32 {
    let r = img.region;
    let v = if x >= r.x0 && x < r.x1 && y >= r.y0 && y < r.y1 {
        let p = img.pixels[((y - r.y0) * r.width() + (x - r.x0)) as usize];
        match kind {
            // Luminance of the premultiplied colour is luminance × alpha.
            MaskKind::Luminance => 0.2125 * p[0] + 0.7154 * p[1] + 0.0721 * p[2],
            MaskKind::Alpha => p[3],
        }
    } else {
        0.0
    };
    let v = v.clamp(0.0, 1.0);
    if invert { 1.0 - v } else { v }
}

/// Renders `root` over `region` in bands and hands each finished band to
/// `sink` (band rect, premultiplied pixels).
fn render_group(root: &PGroup, region: IRect, bg: Option<[f32; 4]>, pre: &[PrepImage], nthreads: usize, sink: &(dyn Fn(IRect, &[[f32; 4]]) + Sync)) {
    if region.is_empty() {
        return;
    }
    let bands = ((region.height() + BAND - 1) / BAND) as usize;
    let next = AtomicUsize::new(0);
    let work = || {
        let mut sc = Scratch::default();
        let ctx = Ctx { pre };
        loop {
            let b = next.fetch_add(1, Ordering::Relaxed);
            if b >= bands {
                break;
            }
            let y0 = region.y0 + b as i32 * BAND;
            let rect = IRect::new(region.x0, y0, region.x1, (y0 + BAND).min(region.y1));
            let n = rect.width() as usize * rect.height() as usize;
            let mut buf = Buf { rect, px: vec![bg.unwrap_or([0.0; 4]); n] };
            draw_group(root, &ctx, &mut buf, &mut sc);
            sink(rect, &buf.px);
        }
    };
    let n = crate::threads_for(nthreads).min(bands);
    if n <= 1 {
        work();
    } else {
        std::thread::scope(|s| {
            for _ in 0..n {
                s.spawn(work);
            }
        });
    }
}

/// Renders every prepass (in order: later ones may use earlier ones), then
/// the scene, handing finished bands to `sink`.
pub(crate) fn render(prep: &Prepared, nthreads: usize, sink: &(dyn Fn(IRect, &[[f32; 4]]) + Sync)) {
    let mut images: Vec<std::sync::Arc<Vec<[f32; 4]>>> = Vec::with_capacity(prep.prepasses.len());
    for p in &prep.prepasses {
        let px = match p {
            Prepass::Ready { pixels, .. } => pixels.clone(),
            Prepass::Scene { region, root, filters } => {
                let views: Vec<PrepImage> = prep.prepasses[..images.len()].iter().zip(&images).map(|(p, px)| PrepImage { region: p.region(), pixels: px }).collect();
                let w = region.width() as usize;
                let out = std::sync::Mutex::new(vec![[0.0f32; 4]; w * region.height() as usize]);
                render_group(root, *region, None, &views, nthreads, &|band, px| {
                    let mut o = out.lock().unwrap_or_else(|e| e.into_inner());
                    let off = (band.y0 - region.y0) as usize * w;
                    o[off..off + px.len()].copy_from_slice(px);
                });
                let mut px = out.into_inner().unwrap_or_else(|e| e.into_inner());
                filter::apply(&mut px, region.width() as usize, region.height() as usize, filters, nthreads);
                std::sync::Arc::new(px)
            }
        };
        images.push(px);
    }
    let views: Vec<PrepImage> = prep.prepasses.iter().zip(&images).map(|(p, px)| PrepImage { region: p.region(), pixels: px }).collect();
    render_group(&prep.root, prep.region, prep.background, &views, nthreads, sink);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect_cov(x0: f32, y0: f32, x1: f32, y1: f32, r: IRect) -> Vec<f32> {
        let lines = Lines { segs: vec![[x0, y0, x0, y1], [x1, y1, x1, y0]], bbox: r, rule: FillRule::NonZero, invert: false };
        coverage_full(&lines, r)
    }

    #[test]
    fn fractional_rectangle_is_exact() {
        let c = rect_cov(0.25, 0.5, 2.75, 1.0, IRect::wh(3, 2));
        let want = [0.375, 0.5, 0.375, 0.0, 0.0, 0.0];
        for (a, b) in c.iter().zip(want) {
            assert!((a - b).abs() < 1e-6, "{c:?}");
        }
    }

    #[test]
    fn diagonal_edge_is_exact() {
        // Triangle (0,0) (4,0) (0,4): the diagonal halves the pixels it crosses.
        let segs = vec![[0.0, 0.0, 4.0, 0.0], [4.0, 0.0, 0.0, 4.0], [0.0, 4.0, 0.0, 0.0]];
        let r = IRect::wh(4, 4);
        let c = coverage_full(&Lines { segs, bbox: r, rule: FillRule::NonZero, invert: false }, r);
        let total: f32 = c.iter().sum();
        assert!((total - 8.0).abs() < 1e-5);
        assert!((c[3] - 0.5).abs() < 1e-6 && (c[0] - 1.0).abs() < 1e-6 && c[15] == 0.0, "{c:?}");
    }

    #[test]
    fn even_odd_hole() {
        let r = IRect::wh(10, 10);
        let mut segs = vec![[0.0, 0.0, 0.0, 10.0], [10.0, 10.0, 10.0, 0.0]];
        segs.extend([[2.0, 2.0, 2.0, 8.0], [8.0, 8.0, 8.0, 2.0]]);
        let nz = coverage_full(&Lines { segs: segs.clone(), bbox: r, rule: FillRule::NonZero, invert: false }, r);
        let eo = coverage_full(&Lines { segs, bbox: r, rule: FillRule::EvenOdd, invert: false }, r);
        assert_eq!(nz[55], 1.0);
        assert_eq!(eo[55], 0.0);
        assert_eq!(eo[11], 1.0);
    }

    #[test]
    fn edges_left_of_the_region_still_count() {
        let r = IRect::new(10, 0, 12, 1);
        let c = rect_cov(-5.0, 0.0, 11.5, 1.0, r);
        assert!((c[0] - 1.0).abs() < 1e-6 && (c[1] - 0.5).abs() < 1e-6, "{c:?}");
    }
}
