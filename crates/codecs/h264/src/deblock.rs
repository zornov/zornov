// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Deblocking filter process (8.7) for frames, field pictures and MBAFF frames.
//!
//! Sample positions are handled geometrically: for every filtered line the macroblock
//! containing p0 is found from its location in the picture, which covers the mixed
//! frame/field macroblock pair edges of MBAFF frames (8.7.1, 8.7.2).

use crate::picture::{MbMotion, Planes};
use crate::tables::{ALPHA, BETA, TC0};

/// Per-macroblock information the filter needs.
#[derive(Clone, Copy, Default)]
pub struct DbMb {
    /// Index into the slice parameter list (u16::MAX = not decoded).
    pub slice: u16,
    pub intra: bool,
    pub transform_8x8: bool,
    /// Field macroblock (field picture or MBAFF field pair).
    pub field: bool,
    /// QPY (0 for I_PCM).
    pub qp: u8,
    /// QPc for Cb and Cr.
    pub qpc: [u8; 2],
    /// Bit (y * 4 + x): the 4x4 luma block has non-zero transform coefficients
    /// (for 8x8 transform macroblocks: its 8x8 block has).
    pub nz: u16,
}

#[derive(Clone, Copy, Default)]
pub struct DbSlice {
    pub disable_idc: u32,
    pub offset_a: i32,
    pub offset_b: i32,
    /// SP or SI slice (treated like intra for bS).
    pub switching: bool,
}

#[inline(always)]
fn clip3(lo: i32, hi: i32, v: i32) -> i32 {
    v.clamp(lo, hi)
}

#[inline(always)]
fn clip1(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// Filter one line of samples across an edge. `q0i` indexes q0; `step` is the distance
/// between samples across the edge.
///
/// # Safety
/// `pl` must be valid for reads and writes of the samples `q0i - 4 * step ..= q0i + 3 * step`
/// (`- 2 / + 1` for chroma) and no other thread may access them concurrently.
#[inline(always)]
#[allow(clippy::too_many_arguments)]
unsafe fn filter_line(pl: PlanePtr, q0i: usize, step: usize, bs: u8, alpha: i32, beta: i32, tc0: i32, chroma: bool) {
    debug_assert!(q0i >= if chroma { 2 } else { 4 } * step && q0i + if chroma { 1 } else { 3 } * step < pl.len);
    let base = unsafe { pl.ptr.add(q0i) };
    // Sample at signed offset `k * step` from q0.
    let at = |k: isize| unsafe { base.offset(k * step as isize) };
    let rd = |k: isize| unsafe { *at(k) as i32 };
    let wr = |k: isize, v: u8| unsafe { *at(k) = v };
    let p0 = rd(-1);
    let q0 = rd(0);
    let p1 = rd(-2);
    let q1 = rd(1);
    if (p0 - q0).abs() >= alpha || (p1 - p0).abs() >= beta || (q1 - q0).abs() >= beta {
        return;
    }
    if chroma {
        if bs < 4 {
            let tc = tc0 + 1;
            let delta = clip3(-tc, tc, (((q0 - p0) << 2) + (p1 - q1) + 4) >> 3);
            wr(-1, clip1(p0 + delta));
            wr(0, clip1(q0 - delta));
        } else {
            wr(-1, ((2 * p1 + p0 + q1 + 2) >> 2) as u8);
            wr(0, ((2 * q1 + q0 + p1 + 2) >> 2) as u8);
        }
        return;
    }
    let p2 = rd(-3);
    let q2 = rd(2);
    let ap = (p2 - p0).abs();
    let aq = (q2 - q0).abs();
    if bs < 4 {
        let tc = tc0 + (ap < beta) as i32 + (aq < beta) as i32;
        let delta = clip3(-tc, tc, (((q0 - p0) << 2) + (p1 - q1) + 4) >> 3);
        wr(-1, clip1(p0 + delta));
        wr(0, clip1(q0 - delta));
        if ap < beta {
            wr(-2, (p1 + clip3(-tc0, tc0, (p2 + ((p0 + q0 + 1) >> 1) - (p1 << 1)) >> 1)) as u8);
        }
        if aq < beta {
            wr(1, (q1 + clip3(-tc0, tc0, (q2 + ((p0 + q0 + 1) >> 1) - (q1 << 1)) >> 1)) as u8);
        }
    } else {
        let strong = (p0 - q0).abs() < ((alpha >> 2) + 2);
        if ap < beta && strong {
            let p3 = rd(-4);
            wr(-1, ((p2 + 2 * p1 + 2 * p0 + 2 * q0 + q1 + 4) >> 3) as u8);
            wr(-2, ((p2 + p1 + p0 + q0 + 2) >> 2) as u8);
            wr(-3, ((2 * p3 + 3 * p2 + p1 + p0 + q0 + 4) >> 3) as u8);
        } else {
            wr(-1, ((2 * p1 + p0 + q1 + 2) >> 2) as u8);
        }
        if aq < beta && strong {
            let q3 = rd(3);
            wr(0, ((p1 + 2 * p0 + 2 * q0 + 2 * q1 + q2 + 4) >> 3) as u8);
            wr(1, ((p0 + q0 + q1 + q2 + 2) >> 2) as u8);
            wr(2, ((2 * q3 + 3 * q2 + q1 + q0 + p0 + 4) >> 3) as u8);
        } else {
            wr(0, ((2 * q1 + q0 + p1 + 2) >> 2) as u8);
        }
    }
}

/// Everything the filter needs about the picture.
pub struct DeblockCtx<'a> {
    pub mbs: &'a [DbMb],
    pub motion: &'a [MbMotion],
    pub slices: &'a [DbSlice],
    pub mb_w: usize,
    pub mb_h: usize,
    pub mbaff: bool,
    pub field_pic: bool,
    pub chroma: bool,
}

/// Geometry of a macroblock in the target planes: luma origin and row step.
#[derive(Clone, Copy)]
struct Geo {
    x0: usize,
    y0: usize,
    cy0: usize,
    step: usize,
}

impl DeblockCtx<'_> {
    fn geo(&self, addr: usize) -> Geo {
        if !self.mbaff {
            let (x, y) = (addr % self.mb_w, addr / self.mb_w);
            return Geo { x0: x * 16, y0: y * 16, cy0: y * 8, step: 1 };
        }
        let p = addr / 2;
        let (x, y) = (p % self.mb_w, p / self.mb_w);
        let b = addr % 2;
        if self.mbs[addr].field {
            Geo { x0: x * 16, y0: y * 32 + b, cy0: y * 16 + b, step: 2 }
        } else {
            Geo { x0: x * 16, y0: y * 32 + 16 * b, cy0: y * 16 + 8 * b, step: 1 }
        }
    }

    /// Macroblock covering luma (chroma when `c`) sample (x, y) and the line inside it.
    fn mb_at(&self, x: usize, y: usize, c: bool) -> (usize, usize) {
        let s = if c { 8 } else { 16 };
        if !self.mbaff {
            return ((y / s) * self.mb_w + x / s, y % s);
        }
        let pair = (y / (2 * s)) * self.mb_w + x / s;
        let r = y % (2 * s);
        let top = 2 * pair;
        if self.mbs[top].field { (top + r % 2, r / 2) } else { (top + r / s, r % s) }
    }

    fn usable(&self, q: &DbMb, p: usize) -> bool {
        let pm = &self.mbs[p];
        pm.slice != u16::MAX && (self.slices[q.slice as usize].disable_idc != 2 || pm.slice == q.slice)
    }

    /// bS for the edge between 4x4 luma block (pbx, pby) of MB `p` and (qbx, qby) of `q`.
    #[allow(clippy::too_many_arguments)]
    fn bs(&self, p: usize, pbx: usize, pby: usize, q: usize, qbx: usize, qby: usize, mb_edge: bool, vertical: bool) -> u8 {
        let pm = &self.mbs[p];
        let qm = &self.mbs[q];
        let mixed = self.mbaff && mb_edge && pm.field != qm.field;
        let switching = self.slices[pm.slice as usize].switching || self.slices[qm.slice as usize].switching;
        if pm.intra || qm.intra || switching {
            if mb_edge {
                if vertical || (!self.field_pic && !pm.field && !qm.field) {
                    return 4;
                }
                return 3;
            }
            return 3;
        }
        if pm.nz >> (pby * 4 + pbx) & 1 != 0 || qm.nz >> (qby * 4 + qbx) & 1 != 0 {
            return 2;
        }
        if mixed {
            return 1;
        }
        let mvy_limit = if qm.field { 2 } else { 4 };
        let a = &self.motion[p];
        let b = &self.motion[q];
        let p8 = (pby >> 1) * 2 + (pbx >> 1);
        let q8 = (qby >> 1) * 2 + (qbx >> 1);
        let p4 = pby * 4 + pbx;
        let q4 = qby * 4 + qbx;
        let rp = [a.ref_pic[0][p8], a.ref_pic[1][p8]];
        let rq = [b.ref_pic[0][q8], b.ref_pic[1][q8]];
        let up = [a.ref_idx[0][p8] >= 0, a.ref_idx[1][p8] >= 0];
        let uq = [b.ref_idx[0][q8] >= 0, b.ref_idx[1][q8] >= 0];
        let np = up[0] as u32 + up[1] as u32;
        let nq = uq[0] as u32 + uq[1] as u32;
        if np != nq {
            return 1;
        }
        let mvd = |x: [i16; 2], y: [i16; 2]| {
            (x[0] as i32 - y[0] as i32).abs() >= 4 || (x[1] as i32 - y[1] as i32).abs() >= mvy_limit
        };
        let mp = [a.mv[0][p4], a.mv[1][p4]];
        let mq = [b.mv[0][q4], b.mv[1][q4]];
        let differs = if np == 1 {
            let lp = if up[0] { 0 } else { 1 };
            let lq = if uq[0] { 0 } else { 1 };
            rp[lp] != rq[lq] || mvd(mp[lp], mq[lq])
        } else if np == 0 {
            false
        } else {
            let same_set = (rp[0] == rq[0] && rp[1] == rq[1]) || (rp[0] == rq[1] && rp[1] == rq[0]);
            if !same_set {
                true
            } else if rp[0] != rp[1] {
                if rp[0] == rq[0] {
                    mvd(mp[0], mq[0]) || mvd(mp[1], mq[1])
                } else {
                    mvd(mp[0], mq[1]) || mvd(mp[1], mq[0])
                }
            } else {
                (mvd(mp[0], mq[0]) || mvd(mp[1], mq[1])) && (mvd(mp[0], mq[1]) || mvd(mp[1], mq[0]))
            }
        };
        differs as u8
    }
}

#[inline]
fn params(qp_av: i32, sl: &DbSlice, bs: u8) -> Option<(i32, i32, i32)> {
    let index_a = (qp_av + sl.offset_a).clamp(0, 51) as usize;
    let index_b = (qp_av + sl.offset_b).clamp(0, 51) as usize;
    let alpha = ALPHA[index_a] as i32;
    let beta = BETA[index_b] as i32;
    if alpha == 0 || beta == 0 {
        return None;
    }
    let tc0 = if bs > 0 && bs < 4 { TC0[index_a][bs as usize - 1] as i32 } else { 0 };
    Some((alpha, beta, tc0))
}

/// A plane's sample buffer shared between the deblocking threads. Every thread only touches
/// the samples around the macroblock it filters; the wavefront schedule in
/// [`deblock_picture`] keeps those regions disjoint between threads.
#[derive(Clone, Copy)]
struct PlanePtr {
    ptr: *mut u8,
    len: usize,
    stride: usize,
    pad: usize,
}

// SAFETY: see the type documentation; the pointer is only dereferenced under the
// wavefront ordering (or by a single thread).
unsafe impl Send for PlanePtr {}
unsafe impl Sync for PlanePtr {}

impl PlanePtr {
    fn new(p: &mut crate::picture::Plane) -> PlanePtr {
        PlanePtr { ptr: p.data.as_mut_ptr(), len: p.data.len(), stride: p.stride, pad: p.pad }
    }

    #[inline(always)]
    fn idx(&self, x: usize, y: usize) -> usize {
        (y + self.pad) * self.stride + x + self.pad
    }
}

/// Pictures smaller than this many macroblock rows are filtered on the calling thread.
const PARALLEL_MIN_ROWS: usize = 8;
/// Upper bound on deblocking threads.
const MAX_THREADS: usize = 8;

/// Apply the deblocking filter to a decoded picture (frame, field or MBAFF frame).
///
/// Progressive frames are filtered as a wavefront over macroblock rows: row `r` filters
/// macroblock `x` only after row `r - 1` has finished macroblock `x + 1`. Filtering a
/// macroblock reads and writes samples of its own macroblock and at most four lines of its
/// left and upper neighbours; with that lag every sample is filtered in exactly the order of
/// the sequential raster scan, so the result is bit-identical.
pub fn deblock_picture(planes: &mut Planes, cx: &DeblockCtx) {
    let parallel = !cx.mbaff && !cx.field_pic;
    deblock_rows_from(planes, cx, 0, parallel);
}

/// Check that the planes cover every macroblock of `cx` (the raw-pointer filter relies on it).
fn check_fit(planes: &Planes, cx: &DeblockCtx) {
    let fits = |p: &crate::picture::Plane, w: usize, h: usize| p.width >= w && p.height >= h && p.pad >= 4;
    assert!(
        cx.mbs.len() >= cx.mb_w * cx.mb_h
            && fits(&planes.y, cx.mb_w * 16, cx.mb_h * 16)
            && (!cx.chroma || (fits(&planes.cb, cx.mb_w * 8, cx.mb_h * 8) && fits(&planes.cr, cx.mb_w * 8, cx.mb_h * 8))),
        "deblocking context does not match the picture"
    );
}

/// Filter the macroblock rows `rows` of a non-MBAFF picture in raster order. Rows before
/// `rows.start` must already be filtered (and rows after it not yet) for the result to equal
/// filtering the whole picture.
pub(crate) fn deblock_rows(planes: &mut Planes, cx: &DeblockCtx, rows: std::ops::Range<usize>) {
    assert!(!cx.mbaff && rows.end <= cx.mb_h);
    check_fit(planes, cx);
    let pl = [PlanePtr::new(&mut planes.y), PlanePtr::new(&mut planes.cb), PlanePtr::new(&mut planes.cr)];
    for q in rows.start * cx.mb_w..rows.end * cx.mb_w {
        // SAFETY: single thread; `planes` is exclusively borrowed for this call.
        unsafe { deblock_mb(&pl, cx, q) };
    }
}

/// Filter all macroblocks from row `r0` on (earlier rows already filtered), as a wavefront
/// over rows when `parallel` (progressive frames only) and the picture is large enough.
pub(crate) fn deblock_rows_from(planes: &mut Planes, cx: &DeblockCtx, r0: usize, parallel: bool) {
    check_fit(planes, cx);
    let total = cx.mb_w * cx.mb_h;
    let pl = [PlanePtr::new(&mut planes.y), PlanePtr::new(&mut planes.cb), PlanePtr::new(&mut planes.cr)];
    let rows = cx.mb_h.saturating_sub(r0);
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).min(MAX_THREADS).min(rows);
    if !parallel || cx.mbaff || cx.field_pic || rows < PARALLEL_MIN_ROWS || threads < 2 {
        for q in r0 * cx.mb_w..total {
            // SAFETY: single thread; `planes` is exclusively borrowed for this call.
            unsafe { deblock_mb(&pl, cx, q) };
        }
        return;
    }
    use std::sync::atomic::{AtomicUsize, Ordering};
    let (mb_w, mb_h) = (cx.mb_w, cx.mb_h);
    // progress[r - r0]: number of macroblocks of row r already filtered.
    let progress: Vec<AtomicUsize> = (r0..mb_h).map(|_| AtomicUsize::new(0)).collect();
    let next_row = AtomicUsize::new(r0);
    let worker = || {
        loop {
            let r = next_row.fetch_add(1, Ordering::Relaxed);
            if r >= mb_h {
                return;
            }
            for x in 0..mb_w {
                if r > r0 {
                    let need = (x + 2).min(mb_w);
                    let mut spins = 0u32;
                    while progress[r - 1 - r0].load(Ordering::Acquire) < need {
                        spins += 1;
                        if spins < 64 {
                            std::hint::spin_loop();
                        } else {
                            std::thread::yield_now();
                        }
                    }
                }
                // SAFETY: rows are claimed in increasing order, so row r - 1 is always being
                // (or has been) filtered and the wait terminates. Macroblock (x, r) touches
                // luma columns >= 16x - 4 and rows >= 16r - 4 up to its own extent; row r - 1
                // has finished everything up to x + 1 and only touches columns >= 16(x + 2) - 4
                // from now on, row r + 1 lags at least two macroblocks behind (columns
                // < 16x - 16). Chroma is the same at half scale.
                unsafe { deblock_mb(&pl, cx, r * mb_w + x) };
                progress[r - r0].store(x + 1, Ordering::Release);
            }
        }
    };
    std::thread::scope(|s| {
        for _ in 1..threads {
            s.spawn(worker);
        }
        worker();
    });
}

/// Filter the edges of macroblock `q` (8.7, in macroblock order).
///
/// # Safety
/// The planes behind `pl` must be valid and nothing else may access the samples within four
/// lines of this macroblock's left and top edges, or inside it, during the call.
unsafe fn deblock_mb(pl: &[PlanePtr; 3], cx: &DeblockCtx, q: usize) {
    let ys = pl[0].stride;
    let cs = pl[1].stride;
    {
        let qm = cx.mbs[q];
        if qm.slice == u16::MAX {
            return;
        }
        let sl = cx.slices[qm.slice as usize];
        if sl.disable_idc == 1 {
            return;
        }
        let g = cx.geo(q);
        // ---- vertical edges ----
        let mut bs_v = [[0u8; 16]; 4];
        let mut pmb_v = [[q; 16]; 4];
        for e in 0..4 {
            if e & 1 == 1 && qm.transform_8x8 {
                continue;
            }
            if e == 0 && g.x0 == 0 {
                continue;
            }
            let mut any = false;
            for k in 0..16 {
                let y = g.y0 + k * g.step;
                let (p, pline, pbx) = if e == 0 {
                    let (p, l) = cx.mb_at(g.x0 - 1, y, false);
                    (p, l, 3)
                } else {
                    (q, k, e - 1)
                };
                if e == 0 && !cx.usable(&qm, p) {
                    continue;
                }
                // Outside MBAFF all lines of a 4-line segment share bS.
                let b = if e > 0 || !cx.mbaff {
                    if k % 4 == 0 { cx.bs(p, pbx, pline / 4, q, e, k / 4, e == 0, true) } else { bs_v[e][k - 1] }
                } else {
                    cx.bs(p, pbx, pline / 4, q, e, k / 4, true, true)
                };
                bs_v[e][k] = b;
                pmb_v[e][k] = p;
                if b == 0 {
                    continue;
                }
                any = true;
                let qp_av = (cx.mbs[p].qp as i32 + qm.qp as i32 + 1) >> 1;
                if let Some((alpha, beta, tc0)) = params(qp_av, &sl, b) {
                    let i = pl[0].idx(g.x0 + 4 * e, y);
                    unsafe { filter_line(pl[0], i, 1, b, alpha, beta, tc0, false) };
                }
            }
            let _ = any;
        }
        if cx.chroma {
            for e in [0usize, 2] {
                if e == 0 && g.x0 == 0 {
                    continue;
                }
                for k in 0..8 {
                    let b = bs_v[e][2 * k];
                    if b == 0 {
                        continue;
                    }
                    let y = g.cy0 + k * g.step;
                    let p = if e == 0 { cx.mb_at(g.x0 / 2 - 1, y, true).0 } else { q };
                    let _ = pmb_v;
                    for c in 0..2 {
                        let qp_av = (cx.mbs[p].qpc[c] as i32 + qm.qpc[c] as i32 + 1) >> 1;
                        if let Some((alpha, beta, tc0)) = params(qp_av, &sl, b) {
                            let plane = pl[1 + c];
                            let i = plane.idx(g.x0 / 2 + 2 * e, y);
                            unsafe { filter_line(plane, i, 1, b, alpha, beta, tc0, true) };
                        }
                    }
                }
            }
        }
        // ---- horizontal edges ----
        for e in 0..4 {
            if e & 1 == 1 && qm.transform_8x8 {
                continue;
            }
            // Passes: (q0 luma row, q0 chroma row, row step, p MB, line of p0 in p).
            let mut passes: [(usize, usize, usize, usize, usize); 2] = [(0, 0, 0, 0, 0); 2];
            let mut npass = 0;
            if e > 0 {
                passes[0] = (g.y0 + 4 * e * g.step, g.cy0 + 2 * e * g.step, g.step, q, 4 * e - 1);
                npass = 1;
            } else if !cx.mbaff {
                if g.y0 > 0 {
                    passes[0] = (g.y0, g.cy0, 1, q - cx.mb_w, 15);
                    npass = 1;
                }
            } else if !qm.field {
                if q % 2 == 1 {
                    passes[0] = (g.y0, g.cy0, 1, q - 1, 15);
                    npass = 1;
                } else if q / 2 >= cx.mb_w {
                    let above = 2 * (q / 2 - cx.mb_w);
                    if cx.mbs[above].field {
                        // Frame MB below a field MB pair: the top edge is filtered once per
                        // field, in field mode (8.7.1).
                        passes[0] = (g.y0, g.cy0, 2, above, 15);
                        passes[1] = (g.y0 + 1, g.cy0 + 1, 2, above + 1, 15);
                        npass = 2;
                    } else {
                        passes[0] = (g.y0, g.cy0, 1, above + 1, 15);
                        npass = 1;
                    }
                }
            } else if q / 2 >= cx.mb_w {
                let (p, l) = cx.mb_at(g.x0, g.y0 - 2, false);
                passes[0] = (g.y0, g.cy0, 2, p, l);
                npass = 1;
            }
            for &(qy, qcy, step, p, pline) in &passes[..npass] {
                if e == 0 && !cx.usable(&qm, p) {
                    continue;
                }
                let qby = if e == 0 { 0 } else { e };
                let mut bs = [0u8; 4];
                for (s, b) in bs.iter_mut().enumerate() {
                    *b = cx.bs(p, s, pline / 4, q, s, qby, e == 0, false);
                }
                if bs == [0; 4] {
                    continue;
                }
                let pm = cx.mbs[p];
                let qp_av = (pm.qp as i32 + qm.qp as i32 + 1) >> 1;
                let base = pl[0].idx(g.x0, qy);
                for k in 0..16 {
                    let b = bs[k / 4];
                    if b == 0 {
                        continue;
                    }
                    if let Some((alpha, beta, tc0)) = params(qp_av, &sl, b) {
                        unsafe { filter_line(pl[0], base + k, step * ys, b, alpha, beta, tc0, false) };
                    }
                }
                if cx.chroma && (e == 0 || e == 2) {
                    for c in 0..2 {
                        let qp_av = (pm.qpc[c] as i32 + qm.qpc[c] as i32 + 1) >> 1;
                        let plane = pl[1 + c];
                        let base = plane.idx(g.x0 / 2, qcy);
                        for k in 0..8 {
                            let b = bs[k / 2];
                            if b == 0 {
                                continue;
                            }
                            if let Some((alpha, beta, tc0)) = params(qp_av, &sl, b) {
                                unsafe { filter_line(plane, base + k, step * cs, b, alpha, beta, tc0, true) };
                            }
                        }
                    }
                }
            }
        }
    }
}
