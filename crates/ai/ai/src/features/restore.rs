// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Picture restoration without a model: the classical steps a film lab or
//! a restoration suite runs, each on its own or together as "old film".
//!
//! * [`denoise`]: edge-preserving smoothing (a self-guided filter per
//!   channel), so grain and sensor noise go and edges stay.
//! * [`deblock`]: softens the 8-pixel block edges of over-compressed video
//!   and JPEGs where the step is small (a real edge is left alone).
//! * [`Deflicker`]: holds the exposure and colour of footage steady from
//!   frame to frame (the pumping of old film and timelapses).
//! * [`Dust`]: specks and blotches that are on one frame only (dirt on the
//!   print) replaced from the frames either side; a spatial version for
//!   stills.
//! * [`unscratch`]: thin vertical scratches running down the frame filled
//!   in from either side.
//! * [`restore_fade`]: faded or colour-cast prints stretched back to full
//!   range channel by channel.
//!
//! Super-resolution and the learned clean-up are in [`super::upscale`].

use crate::frame::{box_mean, guided_filter, Frame};

/// Edge-preserving noise reduction, `strength` 0..1.
pub fn denoise(f: &Frame, strength: f32) -> Frame {
    let s = strength.clamp(0.0, 1.0);
    if s <= 0.0 {
        return f.clone();
    }
    let (w, h) = (f.width, f.height);
    let r = 1 + (s * 3.0).round() as usize;
    // eps is the variance treated as noise: (0.02..0.08)².
    let eps = (0.02 + 0.06 * s).powi(2);
    let mut out = f.clone();
    for c in 0..3 {
        let p: Vec<f32> = f.rgba.chunks_exact(4).map(|px| px[c] as f32 / 255.0).collect();
        let q = guided_filter(&p, &p, w, h, r, eps);
        for (o, v) in out.rgba.chunks_exact_mut(4).zip(q) {
            o[c] = (v * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
        }
    }
    out
}

/// Softens 8×8 block edges whose step is below `threshold` (0..255).
pub fn deblock(f: &Frame, threshold: f32) -> Frame {
    let (w, h) = (f.width, f.height);
    let mut out = f.clone();
    let get = |img: &[u8], x: usize, y: usize, c: usize| img[(y * w + x) * 4 + c] as f32;
    // Vertical block edges (between x = 8k-1 and 8k), then horizontal.
    for pass in 0..2 {
        let src = out.rgba.clone();
        let (len, across) = if pass == 0 { (h, w) } else { (w, h) };
        for line in 0..len {
            let mut e = 8;
            while e + 1 < across {
                let at = |k: isize| if pass == 0 { (e as isize + k) as usize } else { line };
                let at_y = |k: isize| if pass == 0 { line } else { (e as isize + k) as usize };
                for c in 0..3 {
                    let p1 = get(&src, at(-2), at_y(-2), c);
                    let p0 = get(&src, at(-1), at_y(-1), c);
                    let q0 = get(&src, at(0), at_y(0), c);
                    let q1 = get(&src, at(1), at_y(1), c);
                    let step = (q0 - p0).abs();
                    // A block edge: a step at the boundary, flat on both sides.
                    if step > 0.5 && step < threshold && (p1 - p0).abs() < step && (q1 - q0).abs() < step {
                        let d = (q0 - p0) / 4.0;
                        let mut set = |k: isize, v: f32| {
                            let i = (at_y(k) * w + at(k)) * 4 + c;
                            out.rgba[i] = v.round().clamp(0.0, 255.0) as u8;
                        };
                        set(-2, p1 + d * 0.5);
                        set(-1, p0 + d * 1.5);
                        set(0, q0 - d * 1.5);
                        set(1, q1 - d * 0.5);
                    }
                }
                e += 8;
            }
        }
    }
    out
}

/// Stretches each channel from its 0.5 % to 99.5 % level to the full range
/// (by `amount` 0..1), which brings back a faded print and takes out a
/// colour cast.
pub fn restore_fade(f: &Frame, amount: f32) -> Frame {
    let levels = channel_levels(f);
    apply_levels(f, &levels, amount)
}

/// Per channel (low, high) at 0.5 % / 99.5 %.
pub fn channel_levels(f: &Frame) -> [(f32, f32); 3] {
    std::array::from_fn(|c| {
        let mut hist = [0u32; 256];
        for p in f.rgba.chunks_exact(4) {
            hist[p[c] as usize] += 1;
        }
        let n = (f.width * f.height) as f32;
        let at = |q: f32| {
            let mut acc = 0.0;
            for (v, &k) in hist.iter().enumerate() {
                acc += k as f32;
                if acc >= q * n {
                    return v as f32;
                }
            }
            255.0
        };
        (at(0.005), at(0.995))
    })
}

/// `f` with each channel's `levels` stretched to 0..255 (by `amount`).
pub fn apply_levels(f: &Frame, levels: &[(f32, f32); 3], amount: f32) -> Frame {
    let a = amount.clamp(0.0, 1.0);
    let mut out = f.clone();
    let luts: [[u8; 256]; 3] = std::array::from_fn(|c| {
        let (lo, hi) = levels[c];
        let span = (hi - lo).max(24.0);
        std::array::from_fn(|v| {
            let s = ((v as f32 - lo) / span * 255.0).clamp(0.0, 255.0);
            (v as f32 * (1.0 - a) + s * a).round() as u8
        })
    });
    for p in out.rgba.chunks_exact_mut(4) {
        for c in 0..3 {
            p[c] = luts[c][p[c] as usize];
        }
    }
    out
}

/// Thin vertical scratches (a line a few pixels wide, brighter or darker
/// than both sides down most of the frame) filled from the sides. Returns
/// the frame and how many columns were repaired.
pub fn unscratch(f: &Frame) -> (Frame, usize) {
    let (w, h) = (f.width, f.height);
    if w < 8 || h < 8 {
        return (f.clone(), 0);
    }
    let luma = f.luma();
    // Per column: the share of rows where it stands out from x±3 the same way.
    let mut bad = vec![false; w];
    for x in 3..w - 3 {
        let (mut up, mut down) = (0usize, 0usize);
        for y in 0..h {
            let v = luma[y * w + x];
            let side = (luma[y * w + x - 3] + luma[y * w + x + 3]) / 2.0;
            let flat = (luma[y * w + x - 3] - luma[y * w + x + 3]).abs() < 0.04;
            if flat && v - side > 0.06 {
                up += 1;
            } else if flat && side - v > 0.06 {
                down += 1;
            }
        }
        bad[x] = up.max(down) * 2 > h;
    }
    let mut out = f.clone();
    let mut fixed = 0;
    let mut x = 0;
    while x < w {
        if !bad[x] {
            x += 1;
            continue;
        }
        let x0 = x;
        while x < w && bad[x] {
            x += 1;
        }
        // Only thin lines are scratches; a wide band is the picture.
        if x - x0 > 4 {
            continue;
        }
        let (l, r) = (x0.saturating_sub(1), x.min(w - 1));
        for y in 0..h {
            for xx in x0..x {
                let t = (xx - l) as f32 / (r - l).max(1) as f32;
                for c in 0..3 {
                    let a = f.rgba[(y * w + l) * 4 + c] as f32;
                    let b = f.rgba[(y * w + r) * 4 + c] as f32;
                    out.rgba[(y * w + xx) * 4 + c] = (a * (1.0 - t) + b * t).round() as u8;
                }
            }
        }
        fixed += x - x0;
    }
    (out, fixed)
}

/// Holds exposure and colour steady across frames: each frame's channel
/// means are pulled towards their running average.
#[derive(Clone, Debug)]
pub struct Deflicker {
    /// How quickly the target follows real changes (0 = never, 1 = at once).
    pub follow: f32,
    target: Option<[f32; 3]>,
}

impl Default for Deflicker {
    fn default() -> Self {
        Deflicker { follow: 0.08, target: None }
    }
}

impl Deflicker {
    pub fn next(&mut self, f: &Frame) -> Frame {
        let n = (f.width * f.height) as f32;
        let mut mean = [0f32; 3];
        for p in f.rgba.chunks_exact(4) {
            for c in 0..3 {
                mean[c] += p[c] as f32;
            }
        }
        for m in &mut mean {
            *m = (*m / n).max(1.0);
        }
        let target = match self.target {
            None => mean,
            Some(t) => std::array::from_fn(|c| t[c] + (mean[c] - t[c]) * self.follow),
        };
        self.target = Some(target);
        let gain: [f32; 3] = std::array::from_fn(|c| (target[c] / mean[c]).clamp(0.6, 1.6));
        let mut out = f.clone();
        for p in out.rgba.chunks_exact_mut(4) {
            for c in 0..3 {
                p[c] = (p[c] as f32 * gain[c]).round().clamp(0.0, 255.0) as u8;
            }
        }
        out
    }
}

/// Dirt and dust: what is on one frame only.
#[derive(Clone, Debug)]
pub struct Dust {
    /// How far (0..255) a pixel must jump from both neighbours, which agree,
    /// to count as dirt.
    pub threshold: f32,
}

impl Default for Dust {
    fn default() -> Self {
        Dust { threshold: 40.0 }
    }
}

impl Dust {
    /// `cur` with what differs from both `prev` and `next` (where those two
    /// agree) replaced by their mean. Returns the share of pixels repaired.
    pub fn temporal(&self, prev: &Frame, cur: &Frame, next: &Frame) -> (Frame, f32) {
        let mut out = cur.clone();
        let mut fixed = 0usize;
        let n = cur.width * cur.height;
        let mut hit = vec![0f32; n];
        for i in 0..n {
            let o = i * 4;
            let mut jump = 0f32;
            let mut agree = 0f32;
            for c in 0..3 {
                let (a, b, x) = (prev.rgba[o + c] as f32, next.rgba[o + c] as f32, cur.rgba[o + c] as f32);
                jump = jump.max((x - a).abs().min((x - b).abs()));
                agree = agree.max((a - b).abs());
            }
            if jump > self.threshold && agree < self.threshold * 0.5 {
                hit[i] = 1.0;
            }
        }
        // Grow the hits by a pixel so a speck's soft edge goes too.
        let grown = box_mean(&hit, cur.width, cur.height, 1);
        for i in 0..n {
            if grown[i] > 0.0 {
                let o = i * 4;
                for c in 0..3 {
                    out.rgba[o + c] = ((prev.rgba[o + c] as u16 + next.rgba[o + c] as u16 + 1) / 2) as u8;
                }
                fixed += 1;
            }
        }
        (out, fixed as f32 / n as f32)
    }

    /// A still: isolated specks (a pixel far from the median of its 5×5
    /// neighbourhood, in a small spot) replaced by that median.
    pub fn spatial(&self, f: &Frame) -> Frame {
        let (w, h) = (f.width, f.height);
        let mut out = f.clone();
        let luma = f.luma();
        let mean = box_mean(&luma, w, h, 3);
        for y in 2..h.saturating_sub(2) {
            for x in 2..w.saturating_sub(2) {
                let i = y * w + x;
                if (luma[i] - mean[i]).abs() * 255.0 < self.threshold {
                    continue;
                }
                // The spot is small: few of the 5×5 neighbours share the jump.
                let mut like = 0;
                let mut vals: [Vec<u8>; 3] = Default::default();
                for dy in -2isize..=2 {
                    for dx in -2isize..=2 {
                        let j = (y as isize + dy) as usize * w + (x as isize + dx) as usize;
                        if (luma[j] - mean[i]).abs() * 255.0 >= self.threshold * 0.7 {
                            like += 1;
                        }
                        for (c, v) in vals.iter_mut().enumerate() {
                            v.push(f.rgba[j * 4 + c]);
                        }
                    }
                }
                if like > 6 {
                    continue;
                }
                for (c, v) in vals.iter_mut().enumerate() {
                    v.sort_unstable();
                    out.rgba[i * 4 + c] = v[v.len() / 2];
                }
            }
        }
        out
    }
}

/// The share (0..1) of 8-pixel block edges that show a step: high for
/// over-compressed pictures.
pub fn blockiness(f: &Frame) -> f32 {
    let (w, h) = (f.width, f.height);
    let luma = f.luma();
    let (mut at_edge, mut inside) = (0f32, 0f32);
    for y in 0..h {
        for x in 1..w {
            let d = (luma[y * w + x] - luma[y * w + x - 1]).abs();
            if x % 8 == 0 {
                at_edge += d;
            } else {
                inside += d;
            }
        }
    }
    let edges = (h * ((w - 1) / 8)) as f32;
    let others = (h * (w - 1)) as f32 - edges;
    if edges <= 0.0 {
        return 0.0;
    }
    let (e, i) = (at_edge / edges, inside / others.max(1.0));
    ((e - i) / (e + i + 1e-3)).clamp(0.0, 1.0)
}

/// Rough noise level (0..1 of full scale): the median absolute difference
/// between neighbouring pixels in flat areas.
pub fn noise_level(f: &Frame) -> f32 {
    let (w, h) = (f.width, f.height);
    let luma = f.luma();
    let mean = box_mean(&luma, w, h, 2);
    let mut d: Vec<f32> = (0..w * h).step_by(3).filter(|&i| i % w + 1 < w).map(|i| (luma[i] - mean[i]).abs()).collect();
    if d.is_empty() {
        return 0.0;
    }
    let k = d.len() / 2;
    d.select_nth_unstable_by(k, |a, b| a.total_cmp(b));
    d[k]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noisy(w: usize, h: usize) -> Frame {
        let mut f = Frame::filled(w, h, [128, 128, 128, 255]);
        let mut s = 12345u32;
        for p in f.rgba.chunks_exact_mut(4) {
            s = s.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let n = ((s >> 16) % 41) as i32 - 20;
            for c in 0..3 {
                p[c] = (128 + n).clamp(0, 255) as u8;
            }
        }
        f
    }

    #[test]
    fn denoise_lowers_noise() {
        let f = noisy(64, 48);
        let before = noise_level(&f);
        let g = denoise(&f, 0.8);
        assert!(noise_level(&g) < before * 0.6, "{} → {}", before, noise_level(&g));
    }

    #[test]
    fn deblock_flattens_block_steps() {
        let mut f = Frame::filled(32, 8, [100, 100, 100, 255]);
        for y in 0..8 {
            for x in 8..16 {
                f.rgba[(y * 32 + x) * 4..(y * 32 + x) * 4 + 3].copy_from_slice(&[110, 110, 110]);
            }
        }
        assert!(blockiness(&f) > 0.5);
        let g = deblock(&f, 20.0);
        assert!(blockiness(&g) < blockiness(&f));
        // A real edge (a big step) stays.
        let mut e = Frame::filled(32, 8, [0, 0, 0, 255]);
        for y in 0..8 {
            for x in 16..32 {
                e.rgba[(y * 32 + x) * 4..(y * 32 + x) * 4 + 3].copy_from_slice(&[250, 250, 250]);
            }
        }
        assert_eq!(deblock(&e, 20.0), e);
    }

    #[test]
    fn fade_is_stretched_and_flicker_held() {
        let mut f = Frame::filled(20, 10, [90, 80, 70, 255]);
        for p in f.rgba.chunks_exact_mut(8) {
            p[..3].copy_from_slice(&[160, 150, 140]);
        }
        let g = restore_fade(&f, 1.0);
        let lv = channel_levels(&g);
        assert!(lv[0].0 < 5.0 && lv[0].1 > 250.0, "{lv:?}");
        let mut d = Deflicker::default();
        let a = d.next(&Frame::filled(8, 8, [100, 100, 100, 255]));
        let b = d.next(&Frame::filled(8, 8, [140, 140, 140, 255]));
        assert_eq!(a.rgba[0], 100);
        assert!(b.rgba[0] < 115, "{}", b.rgba[0]);
    }

    #[test]
    fn dust_and_scratches_are_repaired() {
        let base = Frame::filled(16, 16, [100, 100, 100, 255]);
        let mut cur = base.clone();
        cur.rgba[(5 * 16 + 5) * 4] = 250;
        cur.rgba[(5 * 16 + 5) * 4 + 1] = 250;
        cur.rgba[(5 * 16 + 5) * 4 + 2] = 250;
        let (fixed, share) = Dust::default().temporal(&base, &cur, &base);
        assert!(share > 0.0);
        assert_eq!(&fixed.rgba[(5 * 16 + 5) * 4..(5 * 16 + 5) * 4 + 3], &[100, 100, 100]);
        let still = Dust::default().spatial(&cur);
        assert_eq!(still.rgba[(5 * 16 + 5) * 4], 100);
        let mut s = Frame::filled(32, 20, [90, 90, 90, 255]);
        for y in 0..20 {
            s.rgba[(y * 32 + 12) * 4..(y * 32 + 12) * 4 + 3].copy_from_slice(&[200, 200, 200]);
        }
        let (u, n) = unscratch(&s);
        assert_eq!(n, 1);
        assert_eq!(u.rgba[(3 * 32 + 12) * 4], 90);
    }
}
