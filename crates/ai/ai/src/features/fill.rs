// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Filling without a model: a hole painted in from its surroundings
//! ([`inpaint`]: a wire, a sign, a small blemish), and a picture made wider
//! or taller ([`extend`]: the edges reflected outwards and softened, so a
//! pan that runs a little off the end of a backdrop never shows the
//! black). Both pull colour in smoothly from the known pixels with a
//! push-pull pyramid; neither invents new objects, which is what a generative
//! fill or expand model does (Codex, from the app).

use crate::frame::Frame;

/// Smooth fill of the pixels where `hole` is true, from the rest.
pub fn inpaint(f: &Frame, hole: &[bool]) -> Frame {
    assert_eq!(hole.len(), f.width * f.height);
    let known: Vec<f32> = hole.iter().map(|&h| if h { 0.0 } else { 1.0 }).collect();
    let filled = push_pull(f, &known);
    let mut out = f.clone();
    for (i, &h) in hole.iter().enumerate() {
        if h {
            out.rgba[i * 4..i * 4 + 4].copy_from_slice(&filled.rgba[i * 4..i * 4 + 4]);
        }
    }
    out
}

/// A rectangle as a hole mask (`x, y, w, h` as shares of the frame, 0..1).
pub fn rect_hole(w: usize, h: usize, rect: [f32; 4]) -> Vec<bool> {
    let x0 = (rect[0].clamp(0.0, 1.0) * w as f32) as usize;
    let y0 = (rect[1].clamp(0.0, 1.0) * h as f32) as usize;
    let x1 = ((rect[0] + rect[2]).clamp(0.0, 1.0) * w as f32).ceil() as usize;
    let y1 = ((rect[1] + rect[3]).clamp(0.0, 1.0) * h as f32).ceil() as usize;
    let mut m = vec![false; w * h];
    for y in y0..y1.min(h) {
        for x in x0..x1.min(w) {
            m[y * w + x] = true;
        }
    }
    m
}

/// Where the original sits on an extended canvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub width: usize,
    pub height: usize,
    pub x: usize,
    pub y: usize,
}

/// The canvas for extending `w`×`h` by `left, right, top, bottom` shares
/// of its own size.
pub fn placement(w: usize, h: usize, left: f32, right: f32, top: f32, bottom: f32) -> Placement {
    let px = |s: f32, n: usize| (s.clamp(0.0, 3.0) * n as f32).round() as usize;
    let (l, r, t, b) = (px(left, w), px(right, w), px(top, h), px(bottom, h));
    Placement { width: w + l + r, height: h + t + b, x: l, y: t }
}

/// `f` on a larger canvas, the new area reflected from the picture and
/// softened with distance from the old edge. Also returns the mask of
/// new pixels (for a model to paint over).
pub fn extend(f: &Frame, at: Placement) -> (Frame, Vec<bool>) {
    let (w, h) = (f.width as isize, f.height as isize);
    let mut out = Frame::filled(at.width, at.height, [0, 0, 0, 255]);
    let mut new = vec![false; at.width * at.height];
    let reflect = |v: isize, n: isize| {
        let p = 2 * n;
        let m = v.rem_euclid(p);
        if m < n { m } else { p - 1 - m }
    };
    for y in 0..at.height {
        for x in 0..at.width {
            let (sx, sy) = (x as isize - at.x as isize, y as isize - at.y as isize);
            let inside = sx >= 0 && sy >= 0 && sx < w && sy < h;
            let p = f.px(reflect(sx, w), reflect(sy, h));
            let o = (y * at.width + x) * 4;
            out.rgba[o..o + 4].copy_from_slice(&p);
            out.rgba[o + 3] = 255;
            new[y * at.width + x] = !inside;
        }
    }
    // Soften the reflection away from the seam: blend towards a blurred
    // fill of the known picture, more the further out.
    let known: Vec<f32> = new.iter().map(|&n| if n { 0.0 } else { 1.0 }).collect();
    let mut seeded = out.clone();
    for (i, &n) in new.iter().enumerate() {
        if n {
            seeded.rgba[i * 4..i * 4 + 3].copy_from_slice(&[0, 0, 0]);
        }
    }
    let soft = push_pull(&seeded, &known);
    let span = (at.width.max(at.height) as f32 * 0.25).max(1.0);
    for y in 0..at.height {
        for x in 0..at.width {
            let i = y * at.width + x;
            if !new[i] {
                continue;
            }
            let dx = (at.x as isize - x as isize).max(x as isize - (at.x as isize + w - 1)).max(0) as f32;
            let dy = (at.y as isize - y as isize).max(y as isize - (at.y as isize + h - 1)).max(0) as f32;
            let t = ((dx * dx + dy * dy).sqrt() / span).clamp(0.0, 1.0) * 0.8;
            for c in 0..3 {
                let a = out.rgba[i * 4 + c] as f32;
                let b = soft.rgba[i * 4 + c] as f32;
                out.rgba[i * 4 + c] = (a * (1.0 - t) + b * t + 0.5) as u8;
            }
        }
    }
    (out, new)
}

/// Push-pull: averages the known pixels down a pyramid and fills each
/// level's gaps from the coarser one.
fn push_pull(f: &Frame, known: &[f32]) -> Frame {
    let (w, h) = (f.width, f.height);
    let mut col: Vec<[f32; 4]> = f.rgba.chunks_exact(4).zip(known).map(|(p, &k)| [p[0] as f32 * k, p[1] as f32 * k, p[2] as f32 * k, k]).collect();
    let mut levels = vec![(w, h, col.clone())];
    let (mut lw, mut lh) = (w, h);
    while lw > 1 || lh > 1 {
        let (nw, nh) = (lw.div_ceil(2), lh.div_ceil(2));
        let mut next = vec![[0f32; 4]; nw * nh];
        for y in 0..lh {
            for x in 0..lw {
                let s = col[y * lw + x];
                let d = &mut next[(y / 2) * nw + x / 2];
                for c in 0..4 {
                    d[c] += s[c];
                }
            }
        }
        // Weights saturate so a fully known cell counts as known.
        for d in &mut next {
            if d[3] > 1.0 {
                let k = 1.0 / d[3];
                for c in 0..4 {
                    d[c] *= k;
                }
            }
        }
        col = next;
        lw = nw;
        lh = nh;
        levels.push((lw, lh, col.clone()));
    }
    // Pull: from the coarsest up, fill what each level does not know.
    for li in (0..levels.len() - 1).rev() {
        let (cw, ch, coarse) = levels[li + 1].clone();
        let (fw, fh, fine) = &mut levels[li];
        for y in 0..*fh {
            for x in 0..*fw {
                // Bilinear from the coarse level.
                let gx = ((x as f32 + 0.5) / 2.0 - 0.5).clamp(0.0, (cw - 1) as f32);
                let gy = ((y as f32 + 0.5) / 2.0 - 0.5).clamp(0.0, (ch - 1) as f32);
                let (x0, y0) = (gx.floor() as usize, gy.floor() as usize);
                let (x1, y1) = ((x0 + 1).min(cw - 1), (y0 + 1).min(ch - 1));
                let (tx, ty) = (gx - x0 as f32, gy - y0 as f32);
                let p = |xx: usize, yy: usize| coarse[yy * cw + xx];
                let mut c = [0f32; 4];
                for k in 0..4 {
                    c[k] = (p(x0, y0)[k] * (1.0 - tx) + p(x1, y0)[k] * tx) * (1.0 - ty) + (p(x0, y1)[k] * (1.0 - tx) + p(x1, y1)[k] * tx) * ty;
                }
                let v = &mut fine[y * *fw + x];
                let a = v[3].min(1.0);
                // Coarse colour normalised by its weight.
                let cn = if c[3] > 1e-6 { [c[0] / c[3], c[1] / c[3], c[2] / c[3]] } else { [0.0; 3] };
                let vn = if v[3] > 1e-6 { [v[0] / v[3], v[1] / v[3], v[2] / v[3]] } else { [0.0; 3] };
                for k in 0..3 {
                    v[k] = vn[k] * a + cn[k] * (1.0 - a);
                }
                v[3] = 1.0;
            }
        }
    }
    let (_, _, top) = &levels[0];
    let mut out = f.clone();
    for (o, v) in out.rgba.chunks_exact_mut(4).zip(top) {
        for c in 0..3 {
            o[c] = (v[c] + 0.5).clamp(0.0, 255.0) as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hole_takes_its_surroundings() {
        let mut f = Frame::filled(32, 32, [40, 160, 90, 255]);
        let hole = rect_hole(32, 32, [0.4, 0.4, 0.2, 0.2]);
        for (i, &h) in hole.iter().enumerate() {
            if h {
                f.rgba[i * 4..i * 4 + 3].copy_from_slice(&[255, 0, 255]);
            }
        }
        let g = inpaint(&f, &hole);
        let p = g.px(16, 16);
        assert!((p[0] as i32 - 40).abs() < 3 && (p[1] as i32 - 160).abs() < 3, "{p:?}");
    }

    #[test]
    fn extending_keeps_the_picture_and_fills_the_rest() {
        let f = Frame::filled(20, 10, [200, 100, 50, 255]);
        let at = placement(20, 10, 0.5, 0.0, 0.0, 0.0);
        assert_eq!((at.width, at.height, at.x, at.y), (30, 10, 10, 0));
        let (g, new) = extend(&f, at);
        assert_eq!(new.iter().filter(|&&n| n).count(), 100);
        assert_eq!(g.px(15, 5), [200, 100, 50, 255]);
        let p = g.px(2, 5);
        assert!((p[0] as i32 - 200).abs() < 4, "{p:?}");
    }
}
