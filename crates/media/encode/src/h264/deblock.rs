// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! In-loop deblocking filter (8.7) for progressive frames with one reference
//! picture, run on the reconstructed frame after all its macroblocks are coded
//! (identical to the decoder's per-macroblock order).

use super::tables::{ALPHA, BETA, TC0};

/// What the filter needs to know about a coded macroblock.
#[derive(Clone, Copy, Default, Debug)]
pub struct DbMb {
    pub intra: bool,
    pub qp: u8,
    pub qpc: u8,
    /// Bit (y * 4 + x): the 4x4 luma block has non-zero coefficients.
    pub nz: u16,
    /// Motion vector (quarter samples) of an inter / skipped macroblock.
    pub mv: [i16; 2],
}

#[inline(always)]
fn filter_line(d: &mut [u8], q0i: usize, step: usize, bs: u8, alpha: i32, beta: i32, tc0: i32, chroma: bool) {
    let p0 = d[q0i - step] as i32;
    let q0 = d[q0i] as i32;
    let p1 = d[q0i - 2 * step] as i32;
    let q1 = d[q0i + step] as i32;
    if (p0 - q0).abs() >= alpha || (p1 - p0).abs() >= beta || (q1 - q0).abs() >= beta {
        return;
    }
    let clip1 = |v: i32| v.clamp(0, 255) as u8;
    if chroma {
        if bs < 4 {
            let tc = tc0 + 1;
            let delta = ((((q0 - p0) << 2) + (p1 - q1) + 4) >> 3).clamp(-tc, tc);
            d[q0i - step] = clip1(p0 + delta);
            d[q0i] = clip1(q0 - delta);
        } else {
            d[q0i - step] = ((2 * p1 + p0 + q1 + 2) >> 2) as u8;
            d[q0i] = ((2 * q1 + q0 + p1 + 2) >> 2) as u8;
        }
        return;
    }
    let p2 = d[q0i - 3 * step] as i32;
    let q2 = d[q0i + 2 * step] as i32;
    let ap = (p2 - p0).abs();
    let aq = (q2 - q0).abs();
    if bs < 4 {
        let tc = tc0 + (ap < beta) as i32 + (aq < beta) as i32;
        let delta = ((((q0 - p0) << 2) + (p1 - q1) + 4) >> 3).clamp(-tc, tc);
        d[q0i - step] = clip1(p0 + delta);
        d[q0i] = clip1(q0 - delta);
        if ap < beta {
            d[q0i - 2 * step] = (p1 + ((p2 + ((p0 + q0 + 1) >> 1) - (p1 << 1)) >> 1).clamp(-tc0, tc0)) as u8;
        }
        if aq < beta {
            d[q0i + step] = (q1 + ((q2 + ((p0 + q0 + 1) >> 1) - (q1 << 1)) >> 1).clamp(-tc0, tc0)) as u8;
        }
    } else {
        let strong = (p0 - q0).abs() < ((alpha >> 2) + 2);
        if ap < beta && strong {
            let p3 = d[q0i - 4 * step] as i32;
            d[q0i - step] = ((p2 + 2 * p1 + 2 * p0 + 2 * q0 + q1 + 4) >> 3) as u8;
            d[q0i - 2 * step] = ((p2 + p1 + p0 + q0 + 2) >> 2) as u8;
            d[q0i - 3 * step] = ((2 * p3 + 3 * p2 + p1 + p0 + q0 + 4) >> 3) as u8;
        } else {
            d[q0i - step] = ((2 * p1 + p0 + q1 + 2) >> 2) as u8;
        }
        if aq < beta && strong {
            let q3 = d[q0i + 3 * step] as i32;
            d[q0i] = ((p1 + 2 * p0 + 2 * q0 + 2 * q1 + q2 + 4) >> 3) as u8;
            d[q0i + step] = ((p0 + q0 + q1 + q2 + 2) >> 2) as u8;
            d[q0i + 2 * step] = ((2 * q3 + 3 * q2 + q1 + q0 + p0 + 4) >> 3) as u8;
        } else {
            d[q0i] = ((2 * q1 + q0 + p1 + 2) >> 2) as u8;
        }
    }
}

#[inline]
fn params(qp_av: i32, bs: u8) -> Option<(i32, i32, i32)> {
    let index = qp_av.clamp(0, 51) as usize;
    let alpha = ALPHA[index] as i32;
    let beta = BETA[index] as i32;
    if alpha == 0 || beta == 0 {
        return None;
    }
    let tc0 = if bs > 0 && bs < 4 { TC0[index][bs as usize - 1] as i32 } else { 0 };
    Some((alpha, beta, tc0))
}

/// bS between 4x4 block `pb` (raster y*4+x) of `p` and `qb` of `q`.
#[inline]
fn bs(p: &DbMb, pb: usize, q: &DbMb, qb: usize, mb_edge: bool) -> u8 {
    if p.intra || q.intra {
        return if mb_edge { 4 } else { 3 };
    }
    if (p.nz >> pb) & 1 != 0 || (q.nz >> qb) & 1 != 0 {
        return 2;
    }
    ((p.mv[0] as i32 - q.mv[0] as i32).abs() >= 4 || (p.mv[1] as i32 - q.mv[1] as i32).abs() >= 4) as u8
}

/// Filter a whole frame in macroblock order. Planes are the coded size
/// (`mb_w * 16` x `mb_h * 16` luma, tightly packed).
pub fn deblock_frame(y: &mut [u8], u: &mut [u8], v: &mut [u8], mbs: &[DbMb], mb_w: usize, mb_h: usize) {
    let ys = mb_w * 16;
    let cs = mb_w * 8;
    for my in 0..mb_h {
        for mx in 0..mb_w {
            let qi = my * mb_w + mx;
            let qm = mbs[qi];
            let (x0, y0) = (mx * 16, my * 16);
            // Vertical edges.
            let mut bs_v = [[0u8; 4]; 4];
            for e in 0..4 {
                if e == 0 && mx == 0 {
                    continue;
                }
                let pm = if e == 0 { mbs[qi - 1] } else { qm };
                for r in 0..4 {
                    let pb = if e == 0 { r * 4 + 3 } else { r * 4 + e - 1 };
                    bs_v[e][r] = bs(&pm, pb, &qm, r * 4 + e, e == 0);
                }
                let qp_av = (pm.qp as i32 + qm.qp as i32 + 1) >> 1;
                for k in 0..16 {
                    let b = bs_v[e][k / 4];
                    if b == 0 {
                        continue;
                    }
                    if let Some((alpha, beta, tc0)) = params(qp_av, b) {
                        filter_line(y, (y0 + k) * ys + x0 + 4 * e, 1, b, alpha, beta, tc0, false);
                    }
                }
            }
            for e in [0usize, 2] {
                if e == 0 && mx == 0 {
                    continue;
                }
                let pm = if e == 0 { mbs[qi - 1] } else { qm };
                let qp_av = (pm.qpc as i32 + qm.qpc as i32 + 1) >> 1;
                for k in 0..8 {
                    let b = bs_v[e][k / 2];
                    if b == 0 {
                        continue;
                    }
                    if let Some((alpha, beta, tc0)) = params(qp_av, b) {
                        let i = (y0 / 2 + k) * cs + x0 / 2 + 2 * e;
                        filter_line(u, i, 1, b, alpha, beta, tc0, true);
                        filter_line(v, i, 1, b, alpha, beta, tc0, true);
                    }
                }
            }
            // Horizontal edges.
            for e in 0..4 {
                if e == 0 && my == 0 {
                    continue;
                }
                let pm = if e == 0 { mbs[qi - mb_w] } else { qm };
                let mut b4 = [0u8; 4];
                for (s, b) in b4.iter_mut().enumerate() {
                    let pb = if e == 0 { 12 + s } else { (e - 1) * 4 + s };
                    *b = bs(&pm, pb, &qm, e * 4 + s, e == 0);
                }
                if b4 == [0; 4] {
                    continue;
                }
                let qp_av = (pm.qp as i32 + qm.qp as i32 + 1) >> 1;
                let base = (y0 + 4 * e) * ys + x0;
                for k in 0..16 {
                    let b = b4[k / 4];
                    if b == 0 {
                        continue;
                    }
                    if let Some((alpha, beta, tc0)) = params(qp_av, b) {
                        filter_line(y, base + k, ys, b, alpha, beta, tc0, false);
                    }
                }
                if e == 0 || e == 2 {
                    let qp_av = (pm.qpc as i32 + qm.qpc as i32 + 1) >> 1;
                    let base = (y0 / 2 + 2 * e) * cs + x0 / 2;
                    for k in 0..8 {
                        let b = b4[k / 2];
                        if b == 0 {
                            continue;
                        }
                        if let Some((alpha, beta, tc0)) = params(qp_av, b) {
                            filter_line(u, base + k, cs, b, alpha, beta, tc0, true);
                            filter_line(v, base + k, cs, b, alpha, beta, tc0, true);
                        }
                    }
                }
            }
        }
    }
}
