// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Dense optical flow: how far every pixel moved between two frames.
//!
//! Coarse-to-fine Lucas–Kanade over an image pyramid with windowed
//! structure tensors and a smoothness pass per level (the classic
//! pyramidal approach; Lucas and Kanade 1981, Bouguet 2000). It feeds the
//! motion-vector channel of the compositor, vector motion blur and
//! temporal cleanup.

use crate::image::Image;
use crate::ops::Gray;

/// Motion of every pixel of `cur` since `prev`, in pixels: the content at
/// `x` in `cur` was at `x - motion` in `prev`. `levels` is the pyramid
/// depth (more follows bigger moves), `smoothness` 0..1 how much the field
/// is evened out.
pub fn dense_flow(prev: &Image, cur: &Image, levels: usize, smoothness: f32) -> Vec<[f32; 2]> {
    let (w, h) = (cur.width, cur.height);
    if w == 0 || h == 0 || (prev.width, prev.height) != (w, h) {
        return vec![[0.0, 0.0]; w * h];
    }
    let mut pa = vec![Gray::from_image(prev)];
    let mut pb = vec![Gray::from_image(cur)];
    while pa.len() < levels.max(1) && pa.last().is_some_and(|g| g.width >= 24 && g.height >= 24) {
        let (a, b) = (pa.last().unwrap().blurred(1.0).half(), pb.last().unwrap().blurred(1.0).half());
        pa.push(a);
        pb.push(b);
    }
    let mut flow: Vec<[f32; 2]> = Vec::new();
    let (mut fw, mut fh) = (0usize, 0usize);
    for lvl in (0..pa.len()).rev() {
        let (a, b) = (&pa[lvl], &pb[lvl]);
        let (lw, lh) = (a.width, a.height);
        // Up-sample the coarser field (or start at rest).
        flow = if flow.is_empty() {
            vec![[0.0, 0.0]; lw * lh]
        } else {
            let (kx, ky) = (fw as f32 / lw as f32, fh as f32 / lh as f32);
            let fx: Vec<f32> = flow.iter().map(|v| v[0]).collect();
            let fy: Vec<f32> = flow.iter().map(|v| v[1]).collect();
            let mut out = Vec::with_capacity(lw * lh);
            for y in 0..lh {
                for x in 0..lw {
                    let (sx, sy) = ((x as f32 + 0.5) * kx, (y as f32 + 0.5) * ky);
                    out.push([
                        crate::comp::geo::sample_plane(&fx, fw, fh, sx, sy) / kx,
                        crate::comp::geo::sample_plane(&fy, fw, fh, sx, sy) / ky,
                    ]);
                }
            }
            out
        };
        (fw, fh) = (lw, lh);
        for _ in 0..3 {
            refine(a, b, &mut flow, 3);
            smooth(&mut flow, lw, lh, 0.5 + 2.5 * smoothness.clamp(0.0, 1.0));
        }
    }
    flow
}

/// One Lucas–Kanade step at every pixel: `prev` sampled at `x - flow`
/// against `cur`, with sums over a `(2r+1)²` window.
fn refine(prev: &Gray, cur: &Gray, flow: &mut [[f32; 2]], r: usize) {
    let (w, h) = (cur.width, cur.height);
    let n = w * h;
    // Warped previous frame and its gradients.
    let mut warped = vec![0.0f32; n];
    for y in 0..h {
        for x in 0..w {
            let f = flow[y * w + x];
            warped[y * w + x] = prev.sample(x as f32 + 0.5 - f[0], y as f32 + 0.5 - f[1]);
        }
    }
    let at = |p: &[f32], x: isize, y: isize| p[(y.clamp(0, h as isize - 1) as usize) * w + x.clamp(0, w as isize - 1) as usize];
    let mut ixx = vec![0.0f32; n];
    let mut ixy = vec![0.0f32; n];
    let mut iyy = vec![0.0f32; n];
    let mut ixt = vec![0.0f32; n];
    let mut iyt = vec![0.0f32; n];
    for y in 0..h as isize {
        for x in 0..w as isize {
            // Average gradient of both frames, as in symmetric LK.
            let gx = 0.25 * (at(&warped, x + 1, y) - at(&warped, x - 1, y) + at(&cur.data, x + 1, y) - at(&cur.data, x - 1, y));
            let gy = 0.25 * (at(&warped, x, y + 1) - at(&warped, x, y - 1) + at(&cur.data, x, y + 1) - at(&cur.data, x, y - 1));
            let gt = at(&cur.data, x, y) - at(&warped, x, y);
            let i = y as usize * w + x as usize;
            (ixx[i], ixy[i], iyy[i], ixt[i], iyt[i]) = (gx * gx, gx * gy, gy * gy, gx * gt, gy * gt);
        }
    }
    let boxed = |p: &[f32]| box_sum(p, w, h, r);
    let (sxx, sxy, syy, sxt, syt) = (boxed(&ixx), boxed(&ixy), boxed(&iyy), boxed(&ixt), boxed(&iyt));
    for i in 0..n {
        // Tikhonov term keeps flat regions still.
        let reg = 1e-4;
        let (a, b, d) = (sxx[i] + reg, sxy[i], syy[i] + reg);
        let det = a * d - b * b;
        if det.abs() < 1e-12 {
            continue;
        }
        // cur(x) ≈ prev(x - f - du) ⇒ ∇·du = -(cur - warped) ... solved
        // for the increment that brings warped onto cur.
        let (ex, ey) = (sxt[i], syt[i]);
        let du = (d * ex - b * ey) / det;
        let dv = (a * ey - b * ex) / det;
        let step = [du.clamp(-2.0, 2.0), dv.clamp(-2.0, 2.0)];
        // warped(x) = prev(x - f); moving f by +δ samples prev further
        // back, so the content shifts forward by δ: cur - warped ≈ -∇·δ.
        flow[i][0] -= step[0];
        flow[i][1] -= step[1];
    }
}

fn box_sum(p: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut tmp = vec![0.0f32; p.len()];
    for y in 0..h {
        let row = &p[y * w..(y + 1) * w];
        let mut acc = 0.0f32;
        for x in 0..=r.min(w - 1) {
            acc += row[x];
        }
        for x in 0..w {
            tmp[y * w + x] = acc;
            if x + r + 1 < w {
                acc += row[x + r + 1];
            }
            if x >= r {
                acc -= row[x - r];
            }
        }
    }
    let mut out = vec![0.0f32; p.len()];
    for x in 0..w {
        let mut acc = 0.0f32;
        for y in 0..=r.min(h - 1) {
            acc += tmp[y * w + x];
        }
        for y in 0..h {
            out[y * w + x] = acc;
            if y + r + 1 < h {
                acc += tmp[(y + r + 1) * w + x];
            }
            if y >= r {
                acc -= tmp[(y - r) * w + x];
            }
        }
    }
    out
}

fn smooth(flow: &mut [[f32; 2]], w: usize, h: usize, sigma: f32) {
    let fx: Vec<f32> = flow.iter().map(|v| v[0]).collect();
    let fy: Vec<f32> = flow.iter().map(|v| v[1]).collect();
    let (fx, fy) = (crate::comp::geo::blur_plane(&fx, w, h, sigma), crate::comp::geo::blur_plane(&fy, w, h, sigma));
    for (i, v) in flow.iter_mut().enumerate() {
        *v = [fx[i], fy[i]];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texture(w: usize, h: usize, dx: f32, dy: f32) -> Image {
        Image::from_fn(w, h, |x, y| {
            let (u, v) = (x as f32 - dx, y as f32 - dy);
            let k = 0.5 + 0.25 * (u * 0.21).sin() * (v * 0.17).cos() + 0.2 * crate::math::value_noise(u / 7.0, v / 7.0, 3);
            [k, k, k, 1.0]
        })
    }

    #[test]
    fn a_shifted_texture_measures_its_shift() {
        let (w, h) = (128, 96);
        let prev = texture(w, h, 0.0, 0.0);
        let cur = texture(w, h, 5.0, -3.0);
        let f = dense_flow(&prev, &cur, 4, 0.5);
        // Away from the borders the field is the shift.
        let mut sx = 0.0;
        let mut sy = 0.0;
        let mut n = 0.0;
        for y in 20..h - 20 {
            for x in 20..w - 20 {
                sx += f[y * w + x][0];
                sy += f[y * w + x][1];
                n += 1.0;
            }
        }
        let (mx, my) = (sx / n, sy / n);
        assert!((mx - 5.0).abs() < 0.4 && (my + 3.0).abs() < 0.4, "measured {mx}, {my}");
    }
}
