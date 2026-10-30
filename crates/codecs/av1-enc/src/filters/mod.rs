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

/// Wall-clock spans of the filter stages, for profiling (`take_profile`).
pub mod profile {
    use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};
    use std::time::Instant;

    #[derive(Debug, Clone, Copy, Default)]
    pub struct Spans {
        pub deblock_search_ns: u64,
        pub cdef_search_ns: u64,
        pub lr_search_ns: u64,
        /// `apply` calls made outside the searches (the final filtered reconstruction).
        pub apply_ns: u64,
    }

    static DEBLOCK: AtomicU64 = AtomicU64::new(0);
    static CDEF: AtomicU64 = AtomicU64::new(0);
    static LR: AtomicU64 = AtomicU64::new(0);
    static APPLY: AtomicU64 = AtomicU64::new(0);
    /// Searches running (on any thread): `apply` calls inside them are part of the search's span.
    static DEPTH: AtomicU32 = AtomicU32::new(0);

    pub(super) struct Span {
        counter: &'static AtomicU64,
        start: Instant,
        search: bool,
        record: bool,
    }

    pub(super) fn span(which: u8) -> Span {
        let top = DEPTH.load(Relaxed) == 0;
        let (counter, search) = match which {
            0 => (&DEBLOCK, true),
            1 => (&CDEF, true),
            2 => (&LR, true),
            _ => (&APPLY, false),
        };
        if search {
            DEPTH.fetch_add(1, Relaxed);
        }
        Span { counter, start: Instant::now(), search, record: search || top }
    }

    impl Drop for Span {
        fn drop(&mut self) {
            if self.search {
                DEPTH.fetch_sub(1, Relaxed);
            }
            if self.record {
                self.counter.fetch_add(self.start.elapsed().as_nanos() as u64, Relaxed);
            }
        }
    }

    /// The spans since the last call, reset to zero.
    pub fn take() -> Spans {
        Spans { deblock_search_ns: DEBLOCK.swap(0, Relaxed), cdef_search_ns: CDEF.swap(0, Relaxed), lr_search_ns: LR.swap(0, Relaxed), apply_ns: APPLY.swap(0, Relaxed) }
    }
}

/// `f(0..n)` in index order, spread over up to `threads` workers. The result does not depend on the
/// worker count.
pub(crate) fn par_map<T: Send>(n: usize, threads: usize, f: impl Fn(usize) -> T + Sync) -> Vec<T> {
    let workers = threads.min(n);
    if workers <= 1 {
        return (0..n).map(f).collect();
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let parts: Vec<Vec<(usize, T)>> = std::thread::scope(|sc| {
        let hs: Vec<_> = (0..workers)
            .map(|_| {
                sc.spawn(|| {
                    let mut out = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if i >= n {
                            return out;
                        }
                        out.push((i, f(i)));
                    }
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().expect("filter worker")).collect()
    });
    let mut slots: Vec<Option<T>> = (0..n).map(|_| None).collect();
    for (i, t) in parts.into_iter().flatten() {
        slots[i] = Some(t);
    }
    slots.into_iter().map(|t| t.expect("every index was evaluated")).collect()
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
    let _t = profile::span(3);
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
    search_deblock_mt(seq, recon, mi, src, 1)
}

/// [`search_deblock`] with the two probes of every round (and the final candidates) evaluated on up
/// to `threads` workers. The levels visited, and so the result, do not depend on `threads`.
pub fn search_deblock_mt(seq: &SequenceHeader, recon: &FrameBuf, mi: &MiInfo, src: &Frame, threads: usize) -> [u32; 2] {
    let _t = profile::span(0);
    let mut memo: std::collections::HashMap<u32, u64> = Default::default();
    let eval = |levels: &[u32], memo: &mut std::collections::HashMap<u32, u64>| {
        let todo: Vec<u32> = levels.iter().copied().filter(|l| !memo.contains_key(l)).collect();
        let sses = par_map(todo.len(), threads, |i| sse(&apply(seq, recon, mi, &FilterParams { lf_level: [todo[i]; 2], cdef: None, lr: None }), src));
        memo.extend(todo.into_iter().zip(sses));
    };
    let (mut lo, mut hi) = (1u32, 63u32);
    while hi - lo > 2 {
        let a = lo + (hi - lo) / 3;
        let b = hi - (hi - lo) / 3;
        eval(&[a, b], &mut memo);
        if memo[&a] <= memo[&b] {
            hi = b;
        } else {
            lo = a;
        }
    }
    let last: Vec<u32> = (lo..=hi).collect();
    eval(&last, &mut memo);
    let best = last.iter().copied().min_by_key(|l| memo[l]).unwrap();
    if memo[&best] < sse(recon, src) { [best, best] } else { [0, 0] }
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

/// Squared error per bit for the filter decisions: the mode decision's multiplier (a multiple of
/// 1/4 to 4 of it was tried; the equal-PSNR gate was best at 1).
fn filter_lambda(base_q_idx: u8) -> f64 {
    crate::mode::rdcost::lambda(base_q_idx, 8)
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
    search_cdef_mt(seq, deblocked, mi, src, base_q_idx, 1)
}

/// [`search_cdef`] with the candidate strengths evaluated on up to `threads` workers; the result
/// does not depend on `threads`.
pub fn search_cdef_mt(seq: &SequenceHeader, deblocked: &FrameBuf, mi: &MiInfo, src: &Frame, base_q_idx: u8, threads: usize) -> Option<CdefChoice> {
    let _t = profile::span(1);
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
    let table: Vec<Vec<u64>> = par_map(CDEF_CANDIDATES.len(), threads, |i| {
        let (pri, sec) = CDEF_CANDIDATES[i];
        let mut params = CdefParams { damping, ..Default::default() };
        (params.y_pri[0], params.y_sec[0], params.uv_pri[0], params.uv_sec[0]) = (pri, sec, pri, sec);
        let choice = CdefChoice { params, idx: vec![0; n], cols };
        let out = apply(seq, deblocked, mi, &FilterParams { lf_level: [0, 0], cdef: Some(choice), lr: None });
        block_sse(&out, src, cols, rows)
    });
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

/// Squared error of every restoration unit of every plane against `src`.
fn unit_sse(planes: &[studio_av1::frame::Plane; 3], src: &Frame, params: &LrParams, rows: [usize; 3], cols: [usize; 3]) -> [Vec<u64>; 3] {
    let mut out: [Vec<u64>; 3] = std::array::from_fn(|p| vec![0; rows[p] * cols[p]]);
    for p in 0..3 {
        let sub = if p == 0 { 0 } else { 1 };
        let unit = params.unit_size(p);
        let (w, h) = if p == 0 { (src.width as usize, src.height as usize) } else { ((src.width as usize + 1) >> 1, (src.height as usize + 1) >> 1) };
        for y in 0..h {
            let luma_y = ((y << sub) >> 2) << 2;
            let ur = (rows[p] - 1).min(((luma_y + 8) >> sub) / unit);
            let a = &planes[p].row(y)[..w];
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

/// Least share of the frame's squared error restoration has to save (before its bits) to be used.
const MIN_LR_GAIN: f64 = 0.03;

/// Indices into `candidates()` tried first: four Wiener filters and four self-guided sets.
fn probe_indices(cands: &[LrUnit]) -> Vec<usize> {
    let wiener = [0usize, 1, 4, 5];
    let mut out: Vec<usize> = wiener.to_vec();
    for set in [1u8, 5, 9, 13] {
        if let Some(i) = cands.iter().position(|u| u.kind as usize == RESTORE_SGRPROJ && u.sgr_set == set) {
            out.push(i);
        }
    }
    out
}

/// Loop restoration for the frame after deblocking (`lf`) and CDEF (`cdef`): every unit of every
/// plane picks none, one of the Wiener filters or a self-guided set by squared error plus an
/// estimate of its bits. None when no plane gains.
pub fn search_lr(seq: &SequenceHeader, recon: &FrameBuf, mi: &MiInfo, src: &Frame, lf: [u32; 2], cdef: Option<&CdefChoice>, base_q_idx: u8, threads: usize) -> Option<LrChoice> {
    let _t = profile::span(2);
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
    let none = unit_sse(&cdef_out.planes, src, &params, rows, cols);
    let cands = candidates();
    let (dec_deblocked, dec_cdef) = (to_decoder(&deblocked), to_decoder(&cdef_out));
    let mut tables: Vec<Option<[Vec<u64>; 3]>> = vec![None; cands.len()];
    // each candidate is run over the whole frame (every unit set to it); they are independent, so
    // they are spread over `threads` workers, each with its own filter state
    let mut eval = |tables: &mut Vec<Option<[Vec<u64>; 3]>>, todo: Vec<usize>| {
        let todo: Vec<usize> = todo.into_iter().filter(|&c| tables[c].is_none()).collect();
        let workers = threads.min(todo.len()).max(1);
        let chunks: Vec<Vec<usize>> = (0..workers).map(|w| todo.iter().copied().skip(w).step_by(workers).collect()).collect();
        let run = |chunk: &[usize]| -> Vec<(usize, [Vec<u64>; 3])> {
            let mut pf = PostFilter::new(seq, &fh, to_decoder(recon), mi.clone());
            chunk
                .iter()
                .map(|&c| {
                    let choice = LrChoice { params, units: std::array::from_fn(|p| vec![cands[c]; rows[p] * cols[p]]), unit_cols: cols, unit_rows: rows };
                    set_lr_units(&mut pf, &choice);
                    let out = pf.restore(&dec_deblocked, &dec_cdef);
                    (c, unit_sse(&out.planes, src, &params, rows, cols))
                })
                .collect()
        };
        let done: Vec<Vec<(usize, [Vec<u64>; 3])>> = if workers == 1 {
            chunks.iter().map(|c| run(c)).collect()
        } else {
            std::thread::scope(|sc| {
                let hs: Vec<_> = chunks.iter().map(|c| sc.spawn(|| run(c))).collect();
                hs.into_iter().map(|h| h.join().expect("restoration worker")).collect()
            })
        };
        for (c, t) in done.into_iter().flatten() {
            tables[c] = Some(t);
        }
    };
    // a few well spread candidates first: when the best unit-by-unit choice among them saves under
    // a few percent of the frame's error, restoration is not worth its bits (nor the rest of the
    // search); this is also where nearly all frames stop
    eval(&mut tables, probe_indices(&cands));
    let gain_of = |tables: &[Option<[Vec<u64>; 3]>]| -> u64 {
        let mut gain = 0u64;
        for p in 0..3 {
            for i in 0..rows[p] * cols[p] {
                let best = tables.iter().flatten().map(|t| t[p][i]).min().unwrap_or(u64::MAX);
                gain += none[p][i].saturating_sub(best);
            }
        }
        gain
    };
    let total_none: u64 = none.iter().flatten().sum();
    if (gain_of(&tables) as f64) < MIN_LR_GAIN * total_none as f64 {
        return None;
    }
    // refine around the probes that did best: the other Wiener filters, and the weight variants
    // and neighbouring sets of the two best self-guided sets
    let total = |c: usize| -> u64 { tables[c].as_ref().map_or(u64::MAX, |t| (0..3).map(|p| (0..rows[p] * cols[p]).map(|i| t[p][i].min(none[p][i])).sum::<u64>()).sum()) };
    let mut sgr: Vec<usize> = probe_indices(&cands).into_iter().filter(|&c| cands[c].kind as usize == RESTORE_SGRPROJ).collect();
    sgr.sort_by_key(|&c| total(c));
    let mut more: Vec<usize> = (0..cands.len()).filter(|&c| cands[c].kind as usize == RESTORE_WIENER).collect();
    for &best in sgr.iter().take(2) {
        let set = cands[best].sgr_set as i32;
        more.extend((0..cands.len()).filter(|&c| cands[c].kind as usize == RESTORE_SGRPROJ && (cands[c].sgr_set as i32 - set).abs() <= 1));
    }
    eval(&mut tables, more);
    let tables: Vec<Option<[Vec<u64>; 3]>> = tables;
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
                let Some(t) = &tables[c] else { continue };
                let cost = t[p][i] as f64 + lambda * (1.5 + bits(cand));
                if cost < best_cost {
                    (best, best_cost) = (*cand, cost);
                }
            }
            units.push(best);
            total_none += n + lambda * 1.5;
            total_best += best_cost;
        }
        // a plane that keeps its units pays for the header and the unit symbols
        // and a plane keeps restoration only when it saves a clear share of its error
        if total_best + lambda * 6.0 < total_none && (total_none - total_best) >= MIN_LR_GAIN * none[p].iter().sum::<u64>() as f64 {
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
