// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Inter prediction sample interpolation (8.4.2.2) and weighted sample prediction (8.4.2.3).

use crate::picture::Plane;

#[cfg(test)]
#[inline(always)]
fn clip(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

#[cfg(test)]
#[inline(always)]
fn tap6(a: i32, b: i32, c: i32, d: i32, e: i32, f: i32) -> i32 {
    a - 5 * b + 20 * c + 20 * d - 5 * e + f
}

/// Luma prediction of a `w`x`h` block whose top-left integer sample is (`x`, `y`) in the
/// reference, with quarter-sample fraction (`fx`, `fy`). Output into `dst` (stride 16).
pub fn mc_luma(r: &Plane, x: i32, y: i32, fx: u32, fy: u32, w: usize, h: usize, dst: &mut [u8; 256]) {
    match w {
        16 => mc_luma_w::<16>(r, x, y, fx, fy, h, dst),
        8 => mc_luma_w::<8>(r, x, y, fx, fy, h, dst),
        _ => mc_luma_w::<4>(r, x, y, fx, fy, h, dst),
    }
}

/// Horizontal 6-tap of one row: `src` starts 2 samples left of the first output sample.
#[inline(always)]
fn hrow<const W: usize>(src: &[u8], out: &mut [i16]) {
    let src = &src[..W + 5];
    let out = &mut out[..W];
    for i in 0..W {
        out[i] = src[i] as i16 + src[i + 5] as i16 - 5 * (src[i + 1] as i16 + src[i + 4] as i16)
            + 20 * (src[i + 2] as i16 + src[i + 3] as i16);
    }
}

#[inline(always)]
fn clip_round5<const W: usize>(v: &[i16], out: &mut [u8]) {
    let v = &v[..W];
    let out = &mut out[..W];
    for i in 0..W {
        out[i] = ((v[i] + 16) >> 5).clamp(0, 255) as u8;
    }
}

#[inline(always)]
fn avg_row<const W: usize>(a: &[u8], b: &[u8], out: &mut [u8]) {
    let (a, b, out) = (&a[..W], &b[..W], &mut out[..W]);
    for i in 0..W {
        out[i] = ((a[i] as u16 + b[i] as u16 + 1) >> 1) as u8;
    }
}

/// Clipped horizontal half samples (b) for rows `row0..row0 + h` (relative to the block),
/// into `out` (stride 16).
#[inline(always)]
fn half_h<const W: usize>(d: &[u8], base: usize, s: usize, row0: usize, h: usize, out: &mut [u8; 256]) {
    let mut t = [0i16; 16];
    for j in 0..h {
        let o = base + (j + row0) * s - 2;
        hrow::<W>(&d[o..o + W + 5], &mut t);
        clip_round5::<W>(&t, &mut out[j * 16..]);
    }
}

/// Clipped vertical half samples (h) for columns `col0..col0 + W` (relative to the block).
#[inline(always)]
fn half_v<const W: usize>(d: &[u8], base: usize, s: usize, col0: usize, h: usize, out: &mut [u8; 256]) {
    for j in 0..h {
        let o = base + j * s + col0 - 2 * s;
        let r0 = &d[o..o + W];
        let r1 = &d[o + s..o + s + W];
        let r2 = &d[o + 2 * s..o + 2 * s + W];
        let r3 = &d[o + 3 * s..o + 3 * s + W];
        let r4 = &d[o + 4 * s..o + 4 * s + W];
        let r5 = &d[o + 5 * s..o + 5 * s + W];
        let out = &mut out[j * 16..j * 16 + W];
        for i in 0..W {
            let v = r0[i] as i16 + r5[i] as i16 - 5 * (r1[i] as i16 + r4[i] as i16) + 20 * (r2[i] as i16 + r3[i] as i16);
            out[i] = ((v + 16) >> 5).clamp(0, 255) as u8;
        }
    }
}

/// Centre half samples (j).
#[inline(always)]
fn half_j<const W: usize>(d: &[u8], base: usize, s: usize, h: usize, out: &mut [u8; 256]) {
    // Intermediate horizontal sums for rows -2..h+3 (fit in i16: -2550..=10710).
    let mut tmp = [0i16; 16 * 21];
    for j in 0..h + 5 {
        let o = base + j * s - 2 * s - 2;
        hrow::<W>(&d[o..o + W + 5], &mut tmp[j * 16..]);
    }
    for j in 0..h {
        let t0 = &tmp[j * 16..j * 16 + W];
        let t1 = &tmp[(j + 1) * 16..(j + 1) * 16 + W];
        let t2 = &tmp[(j + 2) * 16..(j + 2) * 16 + W];
        let t3 = &tmp[(j + 3) * 16..(j + 3) * 16 + W];
        let t4 = &tmp[(j + 4) * 16..(j + 4) * 16 + W];
        let t5 = &tmp[(j + 5) * 16..(j + 5) * 16 + W];
        let out = &mut out[j * 16..j * 16 + W];
        for i in 0..W {
            let v = t0[i] as i32 + t5[i] as i32 - 5 * (t1[i] as i32 + t4[i] as i32) + 20 * (t2[i] as i32 + t3[i] as i32);
            out[i] = ((v + 512) >> 10).clamp(0, 255) as u8;
        }
    }
}

#[inline(always)]
fn mc_luma_w<const W: usize>(r: &Plane, x: i32, y: i32, fx: u32, fy: u32, h: usize, dst: &mut [u8; 256]) {
    // Clamp the block position: outside the picture every needed sample equals the edge
    // sample, which the padded border already holds (see picture::PAD_Y).
    let x = x.clamp(-(W as i32) - 3, r.width as i32 + 2) as isize;
    let y = y.clamp(-(h as i32) - 3, r.height as i32 + 2) as isize;
    let s = r.stride;
    let d = &r.data[..];
    let base = r.idx(x, y);

    if fx == 0 && fy == 0 {
        for j in 0..h {
            let o = base + j * s;
            dst[j * 16..j * 16 + W].copy_from_slice(&d[o..o + W]);
        }
        return;
    }

    let mut a = [0u8; 256];
    let mut b = [0u8; 256];
    match (fx, fy) {
        (_, 0) => {
            half_h::<W>(d, base, s, 0, h, &mut a);
            if fx == 2 {
                for j in 0..h {
                    dst[j * 16..j * 16 + W].copy_from_slice(&a[j * 16..j * 16 + W]);
                }
            } else {
                let c = (fx == 3) as usize;
                for j in 0..h {
                    let o = base + j * s + c;
                    avg_row::<W>(&d[o..o + W], &a[j * 16..], &mut dst[j * 16..]);
                }
            }
        }
        (0, _) => {
            half_v::<W>(d, base, s, 0, h, &mut a);
            if fy == 2 {
                for j in 0..h {
                    dst[j * 16..j * 16 + W].copy_from_slice(&a[j * 16..j * 16 + W]);
                }
            } else {
                let rr = (fy == 3) as usize;
                for j in 0..h {
                    let o = base + (j + rr) * s;
                    avg_row::<W>(&d[o..o + W], &a[j * 16..], &mut dst[j * 16..]);
                }
            }
        }
        (2, 2) => half_j::<W>(d, base, s, h, dst),
        (2, _) => {
            // f (fy = 1) = avg(b, j); q (fy = 3) = avg(j, s)
            half_j::<W>(d, base, s, h, &mut a);
            half_h::<W>(d, base, s, (fy == 3) as usize, h, &mut b);
            for j in 0..h {
                avg_row::<W>(&b[j * 16..], &a[j * 16..], &mut dst[j * 16..]);
            }
        }
        (_, 2) => {
            // i (fx = 1) = avg(h, j); k (fx = 3) = avg(j, m)
            half_j::<W>(d, base, s, h, &mut a);
            half_v::<W>(d, base, s, (fx == 3) as usize, h, &mut b);
            for j in 0..h {
                avg_row::<W>(&b[j * 16..], &a[j * 16..], &mut dst[j * 16..]);
            }
        }
        _ => {
            // e, g, p, r: average of a horizontal (b or s) and a vertical (h or m) half sample.
            half_h::<W>(d, base, s, (fy == 3) as usize, h, &mut a);
            half_v::<W>(d, base, s, (fx == 3) as usize, h, &mut b);
            for j in 0..h {
                avg_row::<W>(&a[j * 16..], &b[j * 16..], &mut dst[j * 16..]);
            }
        }
    }
}

/// Chroma prediction (8.4.2.2.2) of a `w`x`h` block at integer position (`x`, `y`) with
/// eighth-sample fraction (`fx`, `fy`). Output into `dst` (stride 16).
pub fn mc_chroma(r: &Plane, x: i32, y: i32, fx: u32, fy: u32, w: usize, h: usize, dst: &mut [u8; 256]) {
    match w {
        8 => mc_chroma_w::<8>(r, x, y, fx, fy, h, dst),
        4 => mc_chroma_w::<4>(r, x, y, fx, fy, h, dst),
        2 => mc_chroma_w::<2>(r, x, y, fx, fy, h, dst),
        _ => mc_chroma_w::<16>(r, x, y, fx, fy, h, dst),
    }
}

#[inline(always)]
fn mc_chroma_w<const W: usize>(r: &Plane, x: i32, y: i32, fx: u32, fy: u32, h: usize, dst: &mut [u8; 256]) {
    let x = x.clamp(-(W as i32) - 1, r.width as i32) as isize;
    let y = y.clamp(-(h as i32) - 1, r.height as i32) as isize;
    let s = r.stride;
    let d = &r.data[..];
    let base = r.idx(x, y);
    if fx == 0 && fy == 0 {
        for j in 0..h {
            let o = base + j * s;
            dst[j * 16..j * 16 + W].copy_from_slice(&d[o..o + W]);
        }
        return;
    }
    // All sums fit in u16 (64 * 255 + 32).
    let (fx, fy) = (fx as u16, fy as u16);
    if fy == 0 {
        let (wa, wb) = (8 * (8 - fx), 8 * fx);
        for j in 0..h {
            let o = base + j * s;
            let r0 = &d[o..o + W + 1];
            let out = &mut dst[j * 16..j * 16 + W];
            for i in 0..W {
                out[i] = ((wa * r0[i] as u16 + wb * r0[i + 1] as u16 + 32) >> 6) as u8;
            }
        }
        return;
    }
    if fx == 0 {
        let (wa, wc) = (8 * (8 - fy), 8 * fy);
        for j in 0..h {
            let o = base + j * s;
            let r0 = &d[o..o + W];
            let r1 = &d[o + s..o + s + W];
            let out = &mut dst[j * 16..j * 16 + W];
            for i in 0..W {
                out[i] = ((wa * r0[i] as u16 + wc * r1[i] as u16 + 32) >> 6) as u8;
            }
        }
        return;
    }
    let wa = (8 - fx) * (8 - fy);
    let wb = fx * (8 - fy);
    let wc = (8 - fx) * fy;
    let wd = fx * fy;
    for j in 0..h {
        let o = base + j * s;
        let r0 = &d[o..o + W + 1];
        let r1 = &d[o + s..o + s + W + 1];
        let out = &mut dst[j * 16..j * 16 + W];
        for i in 0..W {
            out[i] = ((wa * r0[i] as u16 + wb * r0[i + 1] as u16 + wc * r1[i] as u16 + wd * r1[i + 1] as u16 + 32) >> 6)
                as u8;
        }
    }
}

/// Weighting parameters for one prediction direction combination.
#[derive(Clone, Copy, Debug)]
pub enum Weight {
    /// Default: copy (single list) or rounded average (bi).
    Default,
    /// Explicit/implicit weighting: (logWD, w0, w1, o0, o1). For single-list prediction only
    /// w0/o0 are used.
    Weighted { log_wd: i32, w0: i32, w1: i32, o0: i32, o1: i32 },
}

/// Store a single-list prediction into the destination plane.
pub fn put_single(p: &[u8; 256], w: usize, h: usize, wt: Weight, dst: &mut [u8], off: usize, stride: usize) {
    match wt {
        Weight::Default => {
            for j in 0..h {
                let o = off + j * stride;
                dst[o..o + w].copy_from_slice(&p[j * 16..j * 16 + w]);
            }
        }
        Weight::Weighted { log_wd, w0, o0, .. } => {
            if log_wd >= 1 {
                let rnd = 1 << (log_wd - 1);
                for j in 0..h {
                    let o = off + j * stride;
                    let out = &mut dst[o..o + w];
                    let src = &p[j * 16..j * 16 + w];
                    for (d, &v) in out.iter_mut().zip(src) {
                        *d = (((v as i32 * w0 + rnd) >> log_wd) + o0).clamp(0, 255) as u8;
                    }
                }
            } else {
                for j in 0..h {
                    let o = off + j * stride;
                    let out = &mut dst[o..o + w];
                    let src = &p[j * 16..j * 16 + w];
                    for (d, &v) in out.iter_mut().zip(src) {
                        *d = (v as i32 * w0 + o0).clamp(0, 255) as u8;
                    }
                }
            }
        }
    }
}

/// Store a bi-predicted block.
pub fn put_bi(p0: &[u8; 256], p1: &[u8; 256], w: usize, h: usize, wt: Weight, dst: &mut [u8], off: usize, stride: usize) {
    match wt {
        Weight::Default => {
            for j in 0..h {
                let o = off + j * stride;
                let out = &mut dst[o..o + w];
                let a = &p0[j * 16..j * 16 + w];
                let b = &p1[j * 16..j * 16 + w];
                for ((d, &x), &y) in out.iter_mut().zip(a).zip(b) {
                    *d = ((x as u16 + y as u16 + 1) >> 1) as u8;
                }
            }
        }
        Weight::Weighted { log_wd, w0, w1, o0, o1 } => {
            let o = (o0 + o1 + 1) >> 1;
            let rnd = 1 << log_wd;
            let sh = log_wd + 1;
            for j in 0..h {
                let ro = off + j * stride;
                let out = &mut dst[ro..ro + w];
                let a = &p0[j * 16..j * 16 + w];
                let b = &p1[j * 16..j * 16 + w];
                for ((d, &x), &y) in out.iter_mut().zip(a).zip(b) {
                    *d = (((x as i32 * w0 + y as i32 * w1 + rnd) >> sh) + o).clamp(0, 255) as u8;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_plane() -> Plane {
        let mut p = Plane::new(32, 32, crate::picture::PAD_Y, 0);
        for y in 0..32 {
            for x in 0..32 {
                let i = p.idx(x, y);
                p.data[i] = ((x * 7 + y * 13) % 256) as u8;
            }
        }
        p.extend_borders();
        p
    }

    /// Straight transcription of the spec equations for a single sample, used as reference.
    fn reference_luma(p: &Plane, xi: i32, yi: i32, fx: u32, fy: u32) -> u8 {
        let g = |x: i32, y: i32| {
            let x = x.clamp(0, p.width as i32 - 1) as isize;
            let y = y.clamp(0, p.height as i32 - 1) as isize;
            p.data[p.idx(x, y)] as i32
        };
        let b1 = |x: i32, y: i32| tap6(g(x - 2, y), g(x - 1, y), g(x, y), g(x + 1, y), g(x + 2, y), g(x + 3, y));
        let h1 = |x: i32, y: i32| tap6(g(x, y - 2), g(x, y - 1), g(x, y), g(x, y + 1), g(x, y + 2), g(x, y + 3));
        let b = |x, y| clip((b1(x, y) + 16) >> 5) as i32;
        let h = |x, y| clip((h1(x, y) + 16) >> 5) as i32;
        let j = |x: i32, y: i32| {
            let j1 = tap6(b1(x, y - 2), b1(x, y - 1), b1(x, y), b1(x, y + 1), b1(x, y + 2), b1(x, y + 3));
            clip((j1 + 512) >> 10) as i32
        };
        let (x, y) = (xi, yi);
        let av = |a: i32, c: i32| ((a + c + 1) >> 1) as u8;
        match (fx, fy) {
            (0, 0) => g(x, y) as u8,
            (0, 1) => av(g(x, y), h(x, y)),
            (0, 2) => h(x, y) as u8,
            (0, 3) => av(g(x, y + 1), h(x, y)),
            (1, 0) => av(g(x, y), b(x, y)),
            (1, 1) => av(b(x, y), h(x, y)),
            (1, 2) => av(h(x, y), j(x, y)),
            (1, 3) => av(h(x, y), b(x, y + 1)),
            (2, 0) => b(x, y) as u8,
            (2, 1) => av(b(x, y), j(x, y)),
            (2, 2) => j(x, y) as u8,
            (2, 3) => av(j(x, y), b(x, y + 1)),
            (3, 0) => av(g(x + 1, y), b(x, y)),
            (3, 1) => av(b(x, y), h(x + 1, y)),
            (3, 2) => av(j(x, y), h(x + 1, y)),
            _ => av(h(x + 1, y), b(x, y + 1)),
        }
    }

    #[test]
    fn luma_all_fractions_match_reference() {
        let p = test_plane();
        let mut dst = [0u8; 256];
        for &(bx, by) in &[(0, 0), (5, 3), (-7, 20), (30, -4), (-40, -40), (60, 70), (17, 9)] {
            for fx in 0..4 {
                for fy in 0..4 {
                    for &(w, h) in &[(16usize, 16usize), (8, 4), (4, 8), (4, 4)] {
                        mc_luma(&p, bx, by, fx, fy, w, h, &mut dst);
                        for j in 0..h {
                            for i in 0..w {
                                let e = reference_luma(&p, bx + i as i32, by + j as i32, fx, fy);
                                assert_eq!(dst[j * 16 + i], e, "pos ({bx},{by}) frac ({fx},{fy}) size {w}x{h} at ({i},{j})");
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn chroma_matches_reference() {
        let mut p = Plane::new(16, 16, crate::picture::PAD_C, 0);
        for y in 0..16 {
            for x in 0..16 {
                let i = p.idx(x, y);
                p.data[i] = ((x * 31 + y * 17) % 256) as u8;
            }
        }
        p.extend_borders();
        let g = |x: i32, y: i32| {
            let x = x.clamp(0, 15) as isize;
            let y = y.clamp(0, 15) as isize;
            p.data[p.idx(x, y)] as u32
        };
        let mut dst = [0u8; 256];
        for &(bx, by) in &[(0, 0), (-20, 3), (14, 14), (30, -30), (3, 5)] {
            for fx in 0..8 {
                for fy in 0..8 {
                    mc_chroma(&p, bx, by, fx, fy, 8, 8, &mut dst);
                    for j in 0..8 {
                        for i in 0..8 {
                            let (x, y) = (bx + i, by + j);
                            let e = ((8 - fx) * (8 - fy) * g(x, y)
                                + fx * (8 - fy) * g(x + 1, y)
                                + (8 - fx) * fy * g(x, y + 1)
                                + fx * fy * g(x + 1, y + 1)
                                + 32)
                                >> 6;
                            assert_eq!(dst[j as usize * 16 + i as usize] as u32, e);
                        }
                    }
                }
            }
        }
    }
}
