// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Deblocking filter (8.7.2): boundary strength derivation, luma strong/weak
//! filters and the chroma filter (4:2:0).

use crate::ctu::{chroma_qp_table, PicState, EDGE_TU, F_INTRA, F_NOFILTER, F_NZ};
use crate::params::{Pps, Sps, TileLayout};
use crate::picture::{MvField, Picture, SliceRefs};
use crate::pool::Pool;
use crate::slice::SliceHeader;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

pub const BETA_TABLE: [u8; 52] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 20,
    22, 24, 26, 28, 30, 32, 34, 36, 38, 40, 42, 44, 46, 48, 50, 52, 54, 56, 58, 60, 62, 64,
];

pub const TC_TABLE: [u8; 54] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 3,
    3, 3, 3, 4, 4, 4, 5, 5, 6, 6, 7, 8, 9, 10, 11, 13, 14, 16, 18, 20, 22, 24,
];

fn mv_diff(a: crate::picture::Mv, b: crate::picture::Mv) -> bool {
    (a.x as i32 - b.x as i32).abs() >= 4 || (a.y as i32 - b.y as i32).abs() >= 4
}

fn motion_bs(p: &MvField, q: &MvField, rp: &SliceRefs, rq: &SliceRefs) -> u8 {
    let np = p.pred_flag.count_ones();
    let nq = q.pred_flag.count_ones();
    if np != nq {
        return 1;
    }
    let poc = |f: &MvField, r: &SliceRefs, l: usize| -> i32 { r.poc[l][(f.ref_idx[l].max(0) as usize).min(15)] };
    if np == 1 {
        let lp = if p.uses(0) { 0 } else { 1 };
        let lq = if q.uses(0) { 0 } else { 1 };
        if poc(p, rp, lp) != poc(q, rq, lq) {
            return 1;
        }
        return mv_diff(p.mv[lp], q.mv[lq]) as u8;
    }
    let (p0, p1) = (poc(p, rp, 0), poc(p, rp, 1));
    let (q0, q1) = (poc(q, rq, 0), poc(q, rq, 1));
    if !((p0 == q0 && p1 == q1) || (p0 == q1 && p1 == q0)) {
        return 1;
    }
    if p0 != p1 {
        if p0 == q0 {
            (mv_diff(p.mv[0], q.mv[0]) || mv_diff(p.mv[1], q.mv[1])) as u8
        } else {
            (mv_diff(p.mv[0], q.mv[1]) || mv_diff(p.mv[1], q.mv[0])) as u8
        }
    } else {
        let c1 = mv_diff(p.mv[0], q.mv[0]) || mv_diff(p.mv[1], q.mv[1]);
        let c2 = mv_diff(p.mv[0], q.mv[1]) || mv_diff(p.mv[1], q.mv[0]);
        (c1 && c2) as u8
    }
}

struct Ctx<'a> {
    sps: &'a Sps,
    pps: &'a Pps,
    st: &'a PicState,
    headers: &'a [SliceHeader],
    layout: &'a TileLayout,
}

impl<'a> Ctx<'a> {
    fn ctb(&self, x: usize, y: usize) -> usize {
        (y >> self.sps.log2_ctb) * self.sps.pic_width_in_ctbs + (x >> self.sps.log2_ctb)
    }
    fn hdr(&self, x: usize, y: usize) -> Option<&'a SliceHeader> {
        let i = self.st.ctb_hdr[self.ctb(x, y)];
        self.headers.get(i as usize)
    }
    /// Boundary strength of the edge between p = (xp, yp) and q = (xq, yq).
    fn bs(&self, pic: &Picture, xp: usize, yp: usize, xq: usize, yq: usize, edge: u8) -> u8 {
        let st = self.st;
        let ip = (yp >> 2) * st.w4 + (xp >> 2);
        let iq = (yq >> 2) * st.w4 + (xq >> 2);
        let (cp, cq) = (self.ctb(xp, yp), self.ctb(xq, yq));
        let hq = match self.hdr(xq, yq) {
            Some(h) => h,
            None => return 0,
        };
        if self.st.ctb_hdr[cp] == u16::MAX {
            return 0;
        }
        if hq.deblocking_filter_disabled {
            return 0;
        }
        if cp != cq {
            if st.ctb_slice_addr[cp] != st.ctb_slice_addr[cq] && !hq.loop_filter_across_slices_enabled {
                return 0;
            }
            if !self.pps.loop_filter_across_tiles_enabled
                && self.layout.tile_id[self.layout.rs_to_ts[cp]] != self.layout.tile_id[self.layout.rs_to_ts[cq]]
            {
                return 0;
            }
        }
        let fp = st.flags[ip];
        let fq = st.flags[iq];
        if (fp | fq) & F_INTRA != 0 {
            return 2;
        }
        if edge == EDGE_TU && (fp | fq) & F_NZ != 0 {
            return 1;
        }
        let mp = pic.mvf_at(xp, yp);
        let mq = pic.mvf_at(xq, yq);
        motion_bs(mp, mq, pic.slice_refs_at(xp, yp), pic.slice_refs_at(xq, yq))
    }
}

/// Filters one 4-line luma edge segment. `base` indexes q0 of line 0,
/// `xs` steps across the edge, `ls` steps along it.
#[allow(clippy::too_many_arguments)]
fn filter_luma(
    d: &mut [u16],
    base: usize,
    xs: usize,
    ls: usize,
    beta: i32,
    tc: i32,
    no_p: bool,
    no_q: bool,
    maxv: i32,
) {
    // gather p3..p0 q0..q3 for the 4 lines: l[k][0..8] = p3 p2 p1 p0 q0 q1 q2 q3
    let start = base - 4 * xs;
    let mut l = [[0i32; 8]; 4];
    for (k, line) in l.iter_mut().enumerate() {
        let b = start + k * ls;
        for (i, v) in line.iter_mut().enumerate() {
            *v = d[b + i * xs] as i32;
        }
    }
    let dp0 = (l[0][1] - 2 * l[0][2] + l[0][3]).abs();
    let dp3 = (l[3][1] - 2 * l[3][2] + l[3][3]).abs();
    let dq0 = (l[0][6] - 2 * l[0][5] + l[0][4]).abs();
    let dq3 = (l[3][6] - 2 * l[3][5] + l[3][4]).abs();
    let dpq0 = dp0 + dq0;
    let dpq3 = dp3 + dq3;
    let dp = dp0 + dp3;
    let dq = dq0 + dq3;
    if dpq0 + dpq3 >= beta {
        return;
    }
    let strong_line = |s: &[i32; 8], dpq: i32| -> bool {
        2 * dpq < (beta >> 2) && (s[0] - s[3]).abs() + (s[4] - s[7]).abs() < (beta >> 3) && (s[3] - s[4]).abs() < ((5 * tc + 1) >> 1)
    };
    let strong = strong_line(&l[0], dpq0) && strong_line(&l[3], dpq3);
    let de_p = dp < ((beta + (beta >> 1)) >> 3);
    let de_q = dq < ((beta + (beta >> 1)) >> 3);
    for (k, s) in l.iter().enumerate() {
        let b = start + k * ls;
        let [p3, p2, p1, p0, q0, q1, q2, q3] = *s;
        if strong {
            let tc2 = 2 * tc;
            if !no_p {
                d[b + 3 * xs] = ((p2 + 2 * p1 + 2 * p0 + 2 * q0 + q1 + 4) >> 3).clamp(p0 - tc2, p0 + tc2) as u16;
                d[b + 2 * xs] = ((p2 + p1 + p0 + q0 + 2) >> 2).clamp(p1 - tc2, p1 + tc2) as u16;
                d[b + xs] = ((2 * p3 + 3 * p2 + p1 + p0 + q0 + 4) >> 3).clamp(p2 - tc2, p2 + tc2) as u16;
            }
            if !no_q {
                d[b + 4 * xs] = ((p1 + 2 * p0 + 2 * q0 + 2 * q1 + q2 + 4) >> 3).clamp(q0 - tc2, q0 + tc2) as u16;
                d[b + 5 * xs] = ((p0 + q0 + q1 + q2 + 2) >> 2).clamp(q1 - tc2, q1 + tc2) as u16;
                d[b + 6 * xs] = ((p0 + q0 + q1 + 3 * q2 + 2 * q3 + 4) >> 3).clamp(q2 - tc2, q2 + tc2) as u16;
            }
        } else {
            let mut delta = (9 * (q0 - p0) - 3 * (q1 - p1) + 8) >> 4;
            if delta.abs() < tc * 10 {
                delta = delta.clamp(-tc, tc);
                if !no_p {
                    d[b + 3 * xs] = (p0 + delta).clamp(0, maxv) as u16;
                    if de_p {
                        let dlt = ((((p2 + p0 + 1) >> 1) - p1 + delta) >> 1).clamp(-(tc >> 1), tc >> 1);
                        d[b + 2 * xs] = (p1 + dlt).clamp(0, maxv) as u16;
                    }
                }
                if !no_q {
                    d[b + 4 * xs] = (q0 - delta).clamp(0, maxv) as u16;
                    if de_q {
                        let dlt = ((((q2 + q0 + 1) >> 1) - q1 - delta) >> 1).clamp(-(tc >> 1), tc >> 1);
                        d[b + 5 * xs] = (q1 + dlt).clamp(0, maxv) as u16;
                    }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn filter_chroma(d: &mut [u16], base: usize, xs: isize, ls: isize, lines: usize, tc: i32, no_p: bool, no_q: bool, maxv: i32) {
    for line in 0..lines as isize {
        let idx = |i: isize| -> usize { (base as isize + line * ls + i * xs) as usize };
        let p0 = d[idx(-1)] as i32;
        let p1 = d[idx(-2)] as i32;
        let q0 = d[idx(0)] as i32;
        let q1 = d[idx(1)] as i32;
        let delta = ((((q0 - p0) << 2) + p1 - q1 + 4) >> 3).clamp(-tc, tc);
        if !no_p {
            d[idx(-1)] = (p0 + delta).clamp(0, maxv) as u16;
        }
        if !no_q {
            d[idx(0)] = (q0 - delta).clamp(0, maxv) as u16;
        }
    }
}

/// Splits rows `[0, height)` of a plane into consecutive bands starting at
/// the given (increasing) row boundaries; returns (first row, band) pairs.
fn split_rows<'a>(data: &'a mut [u16], stride: usize, height: usize, bounds: &[usize]) -> Vec<(usize, &'a mut [u16])> {
    let mut out = Vec::with_capacity(bounds.len());
    let mut rest = &mut data[..height * stride];
    let mut row = 0usize;
    for (i, &b) in bounds.iter().enumerate() {
        let b = b.min(height);
        let end = bounds.get(i + 1).map(|&e| e.min(height)).unwrap_or(height);
        debug_assert!(b == row);
        let (band, tail) = std::mem::take(&mut rest).split_at_mut((end - b) * stride);
        out.push((b, band));
        rest = tail;
        row = end;
    }
    out
}

/// Luma band height used for parallel deblocking (multiple of 16).
const BAND: usize = 64;

struct Filt<'a> {
    cx: Ctx<'a>,
    bs_v: &'a [u8],
    bs_h: &'a [u8],
}

impl<'a> Filt<'a> {
    #[inline]
    fn qp(&self, x: usize, y: usize) -> i32 {
        self.cx.st.qp_y[(y >> 2) * self.cx.st.w4 + (x >> 2)] as i32
    }
    #[inline]
    fn nofilter(&self, x: usize, y: usize) -> bool {
        let st = self.cx.st;
        st.any_nofilter && st.flags[(y >> 2) * st.w4 + (x >> 2)] & F_NOFILTER != 0
    }

    /// Luma edges of direction `dir` whose position lies in luma rows
    /// `[y_lo, y_hi)`. `d` holds plane rows starting at `row0`.
    fn luma(&self, dir: usize, d: &mut [u16], row0: usize, stride: usize, y_lo: usize, y_hi: usize) {
        let sps = self.cx.sps;
        let st = self.cx.st;
        let w4 = st.w4;
        let (pw, ph) = (sps.pic_width, sps.pic_height);
        let bdy = sps.bit_depth_luma as i32;
        let maxy = (1 << bdy) - 1;
        let bs = if dir == 0 { self.bs_v } else { self.bs_h };
        let (xs, ls) = if dir == 0 { (1usize, stride) } else { (stride, 1usize) };
        for by in (y_lo >> 2)..(y_hi.div_ceil(4)).min(st.h4) {
            let y = by * 4;
            if dir == 1 && (by == 0 || by % 2 != 0) {
                continue;
            }
            for bx in 0..w4 {
                let b = bs[by * w4 + bx];
                if b == 0 {
                    continue;
                }
                let x = bx * 4;
                if x + 4 > pw || y + 4 > ph {
                    continue;
                }
                let (xp, yp) = if dir == 0 { (x - 1, y) } else { (x, y - 1) };
                let h = match self.cx.hdr(x, y) {
                    Some(h) => h,
                    None => continue,
                };
                let qpl = (self.qp(x, y) + self.qp(xp, yp) + 1) >> 1;
                let qb = (qpl + (h.beta_offset_div2 << 1)).clamp(0, 51);
                let beta = BETA_TABLE[qb as usize] as i32 * (1 << (bdy - 8));
                let qt = (qpl + 2 * (b as i32 - 1) + (h.tc_offset_div2 << 1)).clamp(0, 53);
                let tc = TC_TABLE[qt as usize] as i32 * (1 << (bdy - 8));
                if tc == 0 && beta == 0 {
                    continue;
                }
                let base = (y - row0) * stride + x;
                filter_luma(d, base, xs, ls, beta, tc, self.nofilter(xp, yp), self.nofilter(x, y), maxy);
            }
        }
    }

    /// Chroma edges (4:2:0, 8x8 chroma grid, bS == 2) of direction `dir` whose
    /// position lies in chroma rows `[yc_lo, yc_hi)`. `d` holds rows from `row0`.
    #[allow(clippy::too_many_arguments)]
    fn chroma(&self, dir: usize, c: usize, d: &mut [u16], row0: usize, stride: usize, cw: usize, chh: usize, yc_lo: usize, yc_hi: usize) {
        let sps = self.cx.sps;
        let pps = self.cx.pps;
        let w4 = self.cx.st.w4;
        let (pw, ph) = (sps.pic_width, sps.pic_height);
        let bdc = sps.bit_depth_chroma as i32;
        let maxc = (1 << bdc) - 1;
        let bs = if dir == 0 { self.bs_v } else { self.bs_h };
        let (xs, ls) = if dir == 0 { (1isize, stride as isize) } else { (stride as isize, 1isize) };
        let qp_off = if c == 1 { pps.cb_qp_offset } else { pps.cr_qp_offset };
        let ystep = if dir == 0 { 4 } else { 8 };
        let mut yc = yc_lo.div_ceil(ystep) * ystep;
        while yc < yc_hi.min(chh) {
            for xc in (0..cw).step_by(if dir == 0 { 8 } else { 4 }) {
                if dir == 0 && xc == 0 || dir == 1 && yc == 0 {
                    continue;
                }
                let (x, y) = (xc * 2, yc * 2);
                if x >= pw || y >= ph {
                    continue;
                }
                let b = bs[(y >> 2) * w4 + (x >> 2)];
                if b != 2 {
                    continue;
                }
                let (xp, yp) = if dir == 0 { (x - 1, y) } else { (x, y - 1) };
                let h = match self.cx.hdr(x, y) {
                    Some(h) => h,
                    None => continue,
                };
                let qpi = ((self.qp(x, y) + self.qp(xp, yp) + 1) >> 1) + qp_off;
                let qpc = chroma_qp_table(qpi);
                let qt = (qpc + 2 + (h.tc_offset_div2 << 1)).clamp(0, 53);
                let tc = TC_TABLE[qt as usize] as i32 * (1 << (bdc - 8));
                if tc == 0 {
                    continue;
                }
                let lines = if dir == 0 { 4.min(chh - yc) } else { 4.min(cw - xc) };
                let base = (yc - row0) * stride + xc;
                filter_chroma(d, base, xs, ls, lines, tc, self.nofilter(xp, yp), self.nofilter(x, y), maxc);
            }
            yc += ystep;
        }
    }
}

pub fn deblock_picture(
    pic: &mut Picture,
    st: &PicState,
    sps: &Sps,
    pps: &Pps,
    headers: &[SliceHeader],
    layout: &TileLayout,
    pool: Option<&Pool>,
) {
    let cx = Ctx { sps, pps, st, headers, layout };
    let w4 = st.w4;
    let h4 = st.h4;
    let run = |n: usize, f: &(dyn Fn(usize) + Sync)| match pool {
        Some(p) => p.run(n, f),
        None => (0..n).for_each(f),
    };
    // Boundary strengths, in bands of 4x4 block rows.
    let mut bs_v = vec![0u8; w4 * h4];
    let mut bs_h = vec![0u8; w4 * h4];
    let any = AtomicBool::new(false);
    {
        let rows_per = BAND / 4;
        let bands: Vec<Mutex<(usize, &mut [u8], &mut [u8])>> = bs_v
            .chunks_mut(rows_per * w4)
            .zip(bs_h.chunks_mut(rows_per * w4))
            .enumerate()
            .map(|(i, (v, h))| Mutex::new((i * rows_per, v, h)))
            .collect();
        let pic_ro: &Picture = pic;
        run(bands.len(), &|i| {
            let mut g = bands[i].lock().unwrap_or_else(|e| e.into_inner());
            let (by0, ref mut bv, ref mut bh) = *g;
            let mut a = false;
            for (j, (rv, rh)) in bv.chunks_mut(w4).zip(bh.chunks_mut(w4)).enumerate() {
                let by = by0 + j;
                let y = by * 4;
                let row = by * w4;
                if by > 0 && by % 2 == 0 {
                    for bx in 0..w4 {
                        let e = st.edge_h[row + bx];
                        if e != 0 {
                            let x = bx * 4;
                            rh[bx] = cx.bs(pic_ro, x, y - 1, x, y, e);
                            a |= rh[bx] != 0;
                        }
                    }
                }
                for bx in (2..w4).step_by(2) {
                    let e = st.edge_v[row + bx];
                    if e != 0 {
                        let x = bx * 4;
                        rv[bx] = cx.bs(pic_ro, x - 1, y, x, y, e);
                        a |= rv[bx] != 0;
                    }
                }
            }
            if a {
                any.store(true, Ordering::Relaxed);
            }
        });
    }
    if !any.load(Ordering::Relaxed) {
        return;
    }
    let f = Filt { cx, bs_v: &bs_v, bs_h: &bs_h };
    let chroma = sps.chroma_format_idc == 1;
    for dir in 0..2 {
        // dir 0: vertical edges (horizontal filtering): every band owns its rows.
        // dir 1: horizontal edges at y in [Y0, Y1) touch rows [Y0 - 4, Y1 - 4)
        // (luma) / [Yc0 - 2, Yc1 - 2) (chroma), so bands are shifted up.
        let mut tasks: Vec<Mutex<Band>> = Vec::new();
        for (c, plane) in pic.planes.iter_mut().enumerate() {
            if c > 0 && !chroma {
                break;
            }
            let (band, guard) = if c == 0 { (BAND, 4) } else { (BAND / 2, 2) };
            let (width, height, stride) = (plane.width, plane.height, plane.stride);
            let nb = height.div_ceil(band);
            let bounds: Vec<usize> =
                (0..nb).map(|k| if dir == 0 || k == 0 { k * band } else { k * band - guard }).collect();
            for (k, (row0, d)) in split_rows(&mut plane.data, stride, height, &bounds).into_iter().enumerate() {
                let lo = k * band;
                let hi = ((k + 1) * band).min(height);
                tasks.push(Mutex::new(Band { c, row0, d, lo, hi, width, height, stride }));
            }
        }
        run(tasks.len(), &|i| {
            let mut g = tasks[i].lock().unwrap_or_else(|e| e.into_inner());
            let b = &mut *g;
            if b.c == 0 {
                f.luma(dir, b.d, b.row0, b.stride, b.lo, b.hi);
            } else {
                f.chroma(dir, b.c, b.d, b.row0, b.stride, b.width, b.height, b.lo, b.hi);
            }
        });
    }
}

struct Band<'a> {
    c: usize,
    row0: usize,
    d: &'a mut [u16],
    lo: usize,
    hi: usize,
    width: usize,
    height: usize,
    stride: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables() {
        assert_eq!(BETA_TABLE[51], 64);
        assert_eq!(BETA_TABLE[16], 6);
        assert_eq!(TC_TABLE[53], 24);
        assert_eq!(TC_TABLE[18], 1);
        assert_eq!(TC_TABLE[17], 0);
    }

    #[test]
    fn weak_filter_smooths_step() {
        // 8 samples across a vertical edge between columns 3 and 4: small step.
        let mut d = vec![0u16; 8 * 4];
        for line in 0..4 {
            for i in 0..8 {
                d[line * 8 + i] = if i < 4 { 100 } else { 104 };
            }
        }
        filter_luma(&mut d, 4, 1, 8, 64, 4, false, false, 255);
        // p0 and q0 move towards each other
        assert!(d[3] > 100 && d[4] < 104);
    }
}
