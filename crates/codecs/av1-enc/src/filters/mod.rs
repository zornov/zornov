// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M9 (phase c): in-loop filters (deblock, CDEF, loop restoration): parameter search and the
//! filtered reconstruction.
//!
//! The filters themselves are the decoder's ([`studio_av1::PostFilter`]), run on the encoder's
//! reconstruction, so the reference frames equal what a decoder outputs. Tile data does not depend
//! on the filter parameters, so the search runs after the tile is coded and the frame header is
//! written with the winners.

use studio_av1::frame::{FrameBuf as DecBuf, MiInfo};
use studio_av1::spec_tables::{NUM_4X4_BLOCKS_HIGH, NUM_4X4_BLOCKS_WIDE};
use studio_av1::{FrameHeader, PostFilter, SequenceHeader};

use crate::recon::FrameBuf;
use crate::syntax::uv_aware_tx_size;
use crate::types::{CdefParams, Frame, LrChoice, LrParams, LrUnit};
use studio_av1::spec_tables::{RESTORE_NONE, RESTORE_SGRPROJ, RESTORE_SWITCHABLE, RESTORE_WIENER, SGR_PARAMS};

/// Which in-loop filters the encoder may use (all by default; off for A/B measurements).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilterSet {
    pub deblock: bool,
    pub cdef: bool,
    pub restoration: bool,
}

impl Default for FilterSet {
    fn default() -> FilterSet {
        FilterSet { deblock: true, cdef: true, restoration: true }
    }
}

/// The decoder-side view of the sequence header the encoder writes.
pub fn decoder_sequence(payload: &[u8]) -> SequenceHeader {
    SequenceHeader::parse(payload).expect("the encoder's own sequence header parses")
}

/// CDEF as coded: the strength sets and the index of each 64x64 block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CdefChoice {
    pub params: CdefParams,
    /// Row-major, `cols` per row.
    pub idx: Vec<i8>,
    pub cols: usize,
}

/// The parts of a frame header the post-filters read.
#[derive(Default)]
pub struct FilterParams {
    pub lf_level: [u32; 2],
    pub cdef: Option<CdefChoice>,
    pub lr: Option<LrChoice>,
}

fn frame_header(width: u32, height: u32, p: &FilterParams) -> FrameHeader {
    let mut fh = FrameHeader::default();
    fh.frame_width = width;
    fh.frame_height = height;
    fh.upscaled_width = width;
    fh.mi_cols = 2 * ((width + 7) >> 3);
    fh.mi_rows = 2 * ((height + 7) >> 3);
    fh.lf.level = [p.lf_level[0], p.lf_level[1], p.lf_level[0], p.lf_level[1]];
    if let Some(c) = &p.cdef {
        let d = &mut fh.cdef;
        d.damping = c.params.damping as u32;
        d.bits = c.params.bits as u32;
        for i in 0..8 {
            d.y_pri[i] = c.params.y_pri[i] as u32;
            d.y_sec[i] = c.params.y_sec[i] as u32;
            d.uv_pri[i] = c.params.uv_pri[i] as u32;
            d.uv_sec[i] = c.params.uv_sec[i] as u32;
        }
    }
    if let Some(l) = &p.lr {
        for plane in 0..3 {
            fh.lr.frame_restoration_type[plane] = if l.params.switchable[plane] { RESTORE_SWITCHABLE as u8 } else { RESTORE_NONE as u8 };
            fh.lr.loop_restoration_size[plane] = l.params.unit_size(plane) as u32;
        }
        fh.lr.uses_lr = l.params.uses_lr();
    }
    fh
}

/// Fills the loop filter's transform sizes from the mode info: luma per block, chroma for the
/// block that carries it.
fn fill_tx_sizes(pf: &mut PostFilter, mi: &MiInfo) {
    for r in 0..mi.rows {
        for c in 0..mi.cols {
            let i = r * mi.stride + c;
            let ms = mi.mi_size[i] as usize;
            let (w4, h4) = (NUM_4X4_BLOCKS_WIDE[ms] as usize, NUM_4X4_BLOCKS_HIGH[ms] as usize);
            if r % h4 != 0 || c % w4 != 0 {
                continue;
            }
            let tx = mi.tx_size[i] as usize;
            for y in r..(r + h4).min(mi.rows + 16) {
                for x in c..(c + w4).min(mi.cols + 16) {
                    pf.set_lf_tx_size(0, y, x, tx as u8);
                }
            }
            let has_chroma = !(w4 == 1 && c & 1 == 0) && !(h4 == 1 && r & 1 == 0);
            if has_chroma {
                let ctx = uv_aware_tx_size(ms, 1, tx) as u8;
                for y in (r >> 1)..(r >> 1) + (h4 >> 1).max(1) {
                    for x in (c >> 1)..(c >> 1) + (w4 >> 1).max(1) {
                        pf.set_lf_tx_size(1, y, x, ctx);
                        pf.set_lf_tx_size(2, y, x, ctx);
                    }
                }
            }
        }
    }
}

fn to_decoder(buf: &FrameBuf) -> DecBuf {
    DecBuf { planes: buf.planes.clone(), num_planes: 3, subsampling_x: 1, subsampling_y: 1, bit_depth: buf.bit_depth, width: buf.width as usize, height: buf.height as usize }
}

/// `recon` (the reconstruction before filtering) run through the decoder's post-filters.
pub fn apply(seq: &SequenceHeader, recon: &FrameBuf, mi: &MiInfo, p: &FilterParams) -> FrameBuf {
    if p.lf_level == [0, 0] && p.cdef.is_none() && p.lr.is_none() {
        return recon.clone();
    }
    let fh = frame_header(recon.width, recon.height, p);
    let mut pf = PostFilter::new(seq, &fh, to_decoder(recon), mi.clone());
    fill_tx_sizes(&mut pf, mi);
    if let Some(c) = &p.cdef {
        for r in (0..fh.mi_rows as usize).step_by(16) {
            for col in (0..fh.mi_cols as usize).step_by(16) {
                pf.set_cdef_idx(r, col, c.idx[(r >> 4) * c.cols + (col >> 4)]);
            }
        }
    }
    if let Some(l) = &p.lr {
        set_lr_units(&mut pf, l);
    }
    let out = pf.run();
    let mut filtered = recon.clone();
    filtered.planes = out.planes;
    filtered
}

/// Sum of squared differences between the picture area of `buf` and `src`, all planes.
pub fn sse(buf: &FrameBuf, src: &Frame) -> u64 {
    let mut total = 0u64;
    for p in 0..3 {
        let (w, h) = if p == 0 { (src.width as usize, src.height as usize) } else { ((src.width as usize + 1) >> 1, (src.height as usize + 1) >> 1) };
        for y in 0..h {
            let a = &buf.planes[p].row(y)[..w];
            let b = &src.planes[p][y * src.stride[p]..y * src.stride[p] + w];
            total += a.iter().zip(b).map(|(&x, &y)| (x as i64 - y as i64).pow(2) as u64).sum::<u64>();
        }
    }
    total
}

/// The deblocking level (the same for both directions) that minimises the squared error against
/// `src`; 0 when filtering does not help. A ternary search over 0..=63 assumes the error is
/// unimodal in the level, which holds well enough in practice, and a 0 is always compared.
pub fn search_deblock(seq: &SequenceHeader, recon: &FrameBuf, mi: &MiInfo, src: &Frame) -> [u32; 2] {
    let eval = |l: u32| sse(&apply(seq, recon, mi, &FilterParams { lf_level: [l, l], cdef: None, lr: None }), src);
    let (mut lo, mut hi) = (1u32, 63u32);
    while hi - lo > 2 {
        let a = lo + (hi - lo) / 3;
        let b = hi - (hi - lo) / 3;
        if eval(a) <= eval(b) {
            hi = b;
        } else {
            lo = a;
        }
    }
    let best = (lo..=hi).min_by_key(|&l| eval(l)).unwrap();
    if eval(best) < sse(recon, src) { [best, best] } else { [0, 0] }
}

/// Squared error of every 64x64 block (luma and its chroma) of `buf` against `src`, row-major.
fn block_sse(buf: &FrameBuf, src: &Frame, cols: usize, rows: usize) -> Vec<u64> {
    let mut out = vec![0u64; cols * rows];
    for p in 0..3 {
        let (sx, bs) = if p == 0 { (0, 64) } else { (1, 32) };
        let (w, h) = if p == 0 { (src.width as usize, src.height as usize) } else { ((src.width as usize + 1) >> 1, (src.height as usize + 1) >> 1) };
        let _ = sx;
        for y in 0..h {
            let a = &buf.planes[p].row(y)[..w];
            let b = &src.planes[p][y * src.stride[p]..y * src.stride[p] + w];
            for (bx, (ca, cb)) in a.chunks(bs).zip(b.chunks(bs)).enumerate() {
                out[(y / bs) * cols + bx] += ca.iter().zip(cb).map(|(&x, &y)| (x as i64 - y as i64).pow(2) as u64).sum::<u64>();
            }
        }
    }
    out
}

/// Squared error per bit for the filter decisions. Measured on the rate/error curves of coded
/// frames this is about a sixteenth of the mode decision's multiplier (most coefficients are
/// zero at the rates in use, which flattens the curve).
fn filter_lambda(base_q_idx: u8) -> f64 {
    crate::mode::rdcost::lambda(base_q_idx, 8) / 16.0
}

/// Candidate strengths `(pri, sec)`.
const CDEF_CANDIDATES: [(u8, u8); 20] = [
    (0, 0), (0, 1), (0, 2), (0, 4),
    (2, 0), (2, 1), (2, 2), (2, 4),
    (4, 0), (4, 1), (4, 2), (4, 4),
    (7, 0), (7, 1), (7, 2), (7, 4),
    (11, 0), (11, 1), (11, 2), (11, 4),
];

/// The CDEF strength sets and per-64x64 indices that minimise squared error plus the cost of
/// coding the indices, on top of the deblocked `deblocked`; none when it does not pay.
pub fn search_cdef(seq: &SequenceHeader, deblocked: &FrameBuf, mi: &MiInfo, src: &Frame, base_q_idx: u8) -> Option<CdefChoice> {
    let (cols, rows) = (mi.cols.div_ceil(16), mi.rows.div_ceil(16));
    let n = cols * rows;
    let damping = (3 + base_q_idx as u32 / 64).min(6) as u8;
    // blocks with something to filter: at least one 4x4 that is not skipped
    let active: Vec<bool> = (0..n)
        .map(|b| {
            let (r0, c0) = ((b / cols) * 16, (b % cols) * 16);
            (r0..(r0 + 16).min(mi.rows)).any(|r| (c0..(c0 + 16).min(mi.cols)).any(|c| !mi.skip[mi.idx(r, c)]))
        })
        .collect();
    if !active.iter().any(|&a| a) {
        return None;
    }
    let table: Vec<Vec<u64>> = CDEF_CANDIDATES
        .iter()
        .map(|&(pri, sec)| {
            let mut params = CdefParams { damping, ..Default::default() };
            (params.y_pri[0], params.y_sec[0], params.uv_pri[0], params.uv_sec[0]) = (pri, sec, pri, sec);
            let choice = CdefChoice { params, idx: vec![0; n], cols };
            let out = apply(seq, deblocked, mi, &FilterParams { lf_level: [0, 0], cdef: Some(choice), lr: None });
            block_sse(&out, src, cols, rows)
        })
        .collect();
    let lambda = filter_lambda(base_q_idx);
    let n_active = active.iter().filter(|&&a| a).count();
    let total = |best: &[u64]| -> u64 { (0..n).filter(|&b| active[b]).map(|b| best[b]).sum() };
    let mut best_pick: Option<(f64, Vec<usize>, u32)> = None;
    let mut chosen: Vec<usize> = Vec::new();
    let mut best = vec![u64::MAX; n];
    for bits in 0..=3u32 {
        // greedy: grow the set to 2^bits entries
        while chosen.len() < 1 << bits {
            let mut pick: Option<(usize, u64)> = None;
            for (c, t) in table.iter().enumerate() {
                if chosen.contains(&c) {
                    continue;
                }
                let cost = total(&(0..n).map(|b| best[b].min(t[b])).collect::<Vec<_>>());
                if pick.map_or(true, |(_, pc)| cost < pc) {
                    pick = Some((c, cost));
                }
            }
            let Some((c, _)) = pick else { break };
            chosen.push(c);
            for b in 0..n {
                best[b] = best[b].min(table[c][b]);
            }
        }
        if chosen.len() < 1 << bits {
            break;
        }
        let cost = total(&best) as f64 + lambda * (bits as usize * n_active) as f64;
        if best_pick.as_ref().map_or(true, |(bc, _, _)| cost < *bc) {
            best_pick = Some((cost, chosen.clone(), bits));
        }
    }
    let (_, set, bits) = best_pick?;
    let mut params = CdefParams { damping, bits: bits as u8, ..Default::default() };
    for (i, &c) in set.iter().enumerate() {
        let (pri, sec) = CDEF_CANDIDATES[c];
        (params.y_pri[i], params.y_sec[i], params.uv_pri[i], params.uv_sec[i]) = (pri, sec, pri, sec);
    }
    let idx: Vec<i8> = (0..n).map(|b| if active[b] { (0..set.len()).min_by_key(|&i| table[set[i]][b]).unwrap() as i8 } else { 0 }).collect();
    // only worth signalling when it beats the unfiltered frame
    let off = block_sse(deblocked, src, cols, rows);
    let on: u64 = (0..n).filter(|&b| active[b]).map(|b| table[set[idx[b] as usize]][b]).sum();
    let off_total: u64 = (0..n).filter(|&b| active[b]).map(|b| off[b]).sum();
    (on < off_total).then_some(CdefChoice { params, idx, cols })
}

fn set_lr_units(pf: &mut PostFilter, l: &LrChoice) {
    for plane in 0..3 {
        if !l.params.switchable[plane] {
            continue;
        }
        for (i, u) in l.units[plane].iter().enumerate() {
            let mut wiener = u.wiener;
            if plane > 0 {
                wiener[0][0] = 0;
                wiener[1][0] = 0;
            }
            pf.set_lr_unit(plane, i, u.kind, wiener, u.sgr_set, u.sgr_xqd);
        }
    }
}

/// Units per row and column of every plane for restoration unit size `params`.
fn unit_counts(params: &LrParams, width: u32, height: u32) -> ([usize; 3], [usize; 3]) {
    let (mut rows, mut cols) = ([0; 3], [0; 3]);
    for p in 0..3 {
        let sub = if p == 0 { 0 } else { 1 };
        let unit = params.unit_size(p);
        let count = |size: usize| ((size + (unit >> 1)) / unit).max(1);
        rows[p] = count((height as usize + sub) >> sub);
        cols[p] = count((width as usize + sub) >> sub);
    }
    (rows, cols)
}

/// Squared error of every restoration unit of every plane of `buf` against `src`.
fn unit_sse(buf: &FrameBuf, src: &Frame, params: &LrParams, rows: [usize; 3], cols: [usize; 3]) -> [Vec<u64>; 3] {
    let mut out: [Vec<u64>; 3] = std::array::from_fn(|p| vec![0; rows[p] * cols[p]]);
    for p in 0..3 {
        let sub = if p == 0 { 0 } else { 1 };
        let unit = params.unit_size(p);
        let (w, h) = if p == 0 { (src.width as usize, src.height as usize) } else { ((src.width as usize + 1) >> 1, (src.height as usize + 1) >> 1) };
        for y in 0..h {
            let luma_y = ((y << sub) >> 2) << 2;
            let ur = (rows[p] - 1).min(((luma_y + 8) >> sub) / unit);
            let a = &buf.planes[p].row(y)[..w];
            let b = &src.planes[p][y * src.stride[p]..y * src.stride[p] + w];
            for x in 0..w {
                let luma_x = ((x << sub) >> 2) << 2;
                let uc = (cols[p] - 1).min((luma_x >> sub) / unit);
                out[p][ur * cols[p] + uc] += (a[x] as i64 - b[x] as i64).pow(2) as u64;
            }
        }
    }
    out
}

/// Wiener filters tried for every unit: `(c0, c1, c2)` for both passes.
const WIENER_BANK: [[i8; 3]; 8] = [[3, -7, 15], [1, -3, 10], [0, 0, 0], [2, -5, 12], [4, -10, 22], [5, -12, 30], [1, -4, 12], [-2, 2, 5]];
/// Self-guided projection weights tried with every parameter set.
const SGR_BANK: [[i8; 2]; 3] = [[-32, 31], [-16, 48], [-64, 24]];

fn candidates() -> Vec<LrUnit> {
    let mut c: Vec<LrUnit> = WIENER_BANK.iter().map(|&w| LrUnit { kind: RESTORE_WIENER as u8, wiener: [w, w], ..Default::default() }).collect();
    for set in 0..16u8 {
        let (r0, r1) = (SGR_PARAMS[set as usize][0], SGR_PARAMS[set as usize][2]);
        for x in SGR_BANK {
            let xqd = if r0 == 0 {
                [0, x[1]]
            } else if r1 == 0 {
                [x[0], (128 - x[0] as i32).clamp(-32, 95) as i8]
            } else {
                x
            };
            let u = LrUnit { kind: RESTORE_SGRPROJ as u8, sgr_set: set, sgr_xqd: xqd, ..Default::default() };
            if !c.contains(&u) {
                c.push(u);
            }
        }
    }
    c
}

/// Loop restoration for the frame after deblocking (`lf`) and CDEF (`cdef`): every unit of every
/// plane picks none, one of the Wiener filters or a self-guided set by squared error plus an
/// estimate of its bits. None when no plane gains.
pub fn search_lr(seq: &SequenceHeader, recon: &FrameBuf, mi: &MiInfo, src: &Frame, lf: [u32; 2], cdef: Option<&CdefChoice>, base_q_idx: u8) -> Option<LrChoice> {
    let deblocked = apply(seq, recon, mi, &FilterParams { lf_level: lf, cdef: None, lr: None });
    let cdef_out = match cdef {
        Some(c) => apply(seq, &deblocked, mi, &FilterParams { lf_level: [0, 0], cdef: Some(c.clone()), lr: None }),
        None => deblocked.clone(),
    };
    let area = src.width as usize * src.height as usize;
    let unit_shift = if area <= 256 * 256 { 0 } else if area <= 640 * 360 { 1 } else { 2 };
    let params = LrParams { switchable: [true; 3], unit_shift, uv_shift: 0 };
    let (rows, cols) = unit_counts(&params, src.width, src.height);
    let fh = frame_header(src.width, src.height, &FilterParams { lf_level: [0, 0], cdef: None, lr: Some(LrChoice { params, units: Default::default(), unit_cols: cols, unit_rows: rows }) });
    let none = unit_sse(&cdef_out, src, &params, rows, cols);
    let cands = candidates();
    let mut tables: Vec<[Vec<u64>; 3]> = Vec::new();
    for cand in &cands {
        let choice = LrChoice { params, units: std::array::from_fn(|p| vec![*cand; rows[p] * cols[p]]), unit_cols: cols, unit_rows: rows };
        let mut pf = PostFilter::new(seq, &fh, to_decoder(recon), mi.clone());
        set_lr_units(&mut pf, &choice);
        let out = pf.restore(&to_decoder(&deblocked), &to_decoder(&cdef_out));
        let mut filtered = recon.clone();
        filtered.planes = out.planes;
        tables.push(unit_sse(&filtered, src, &params, rows, cols));
    }
    let lambda = filter_lambda(base_q_idx);
    let bits = |u: &LrUnit| -> f64 { if u.kind as usize == RESTORE_WIENER { 30.0 } else { 20.0 } };
    let mut choice = LrChoice { params, units: Default::default(), unit_cols: cols, unit_rows: rows };
    let mut any = false;
    for p in 0..3 {
        let mut units = Vec::with_capacity(rows[p] * cols[p]);
        let (mut total_none, mut total_best) = (0.0f64, 0.0f64);
        for i in 0..rows[p] * cols[p] {
            let n = none[p][i] as f64;
            let (mut best, mut best_cost) = (LrUnit::default(), n + lambda * 1.5);
            for (c, cand) in cands.iter().enumerate() {
                let cost = tables[c][p][i] as f64 + lambda * (1.5 + bits(cand));
                if cost < best_cost {
                    (best, best_cost) = (*cand, cost);
                }
            }
            units.push(best);
            total_none += n + lambda * 1.5;
            total_best += best_cost;
        }
        // a plane that keeps its units pays for the header and the unit symbols
        if total_best + lambda * 6.0 < total_none {
            choice.params.switchable[p] = true;
            choice.units[p] = units;
            any = true;
        } else {
            choice.params.switchable[p] = false;
            choice.units[p] = vec![LrUnit::default(); rows[p] * cols[p]];
        }
    }
    any.then_some(choice)
}
