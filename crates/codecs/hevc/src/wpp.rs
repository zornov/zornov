// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Wavefront parallel processing (WPP): decodes the CTU rows of a slice
//! segment on several threads (9.3.1, 9.3.2.4).
//!
//! With `entropy_coding_sync_enabled_flag` every CTU row of a slice segment
//! is a separate CABAC substream whose start is given by the entry points of
//! the slice header, and whose context variables are initialised from the
//! state after the second CTU of the row above. Row `r` may therefore decode
//! CTU `x` as soon as row `r - 1` has finished CTU `x + 1`: everything CTU
//! decoding reads from outside the current CTU (left, above-left, above,
//! above-right neighbours) is then complete, and locations below-left are
//! not yet decoded in either the sequential or the parallel order.
//!
//! All row decoders share the picture through `PicView` / `StView`; each
//! writes only the CTUs of its own row, and reads of other rows' data are
//! ordered by the release/acquire row progress counters.
//!
//! The parallel result is identical to sequential decoding: every row must
//! end exactly at the next row's entry point (checked through the
//! end_of_subset_one_bit alignment) and decode without error; otherwise the
//! segment's state is reset and the caller decodes it sequentially.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

use crate::cabac::{Ctx, NUM_CTX};
use crate::ctu::{PicState, SliceDecoder, StScalars, StView};
use crate::error::{Error, Result};
use crate::params::{Pps, ScalingFactors, Sps, TileLayout};
use crate::picture::{PicView, Picture};
use crate::pool::Pool;
use crate::slice::SliceHeader;

/// Counters of parallel segment decodes (successful, fallen back to
/// sequential decoding); used by tests and for diagnostics.
pub static STATS_PARALLEL: AtomicUsize = AtomicUsize::new(0);
pub static STATS_FALLBACK: AtomicUsize = AtomicUsize::new(0);

/// Byte positions (in the RBSP) of the emulation prevention bytes removed
/// from `nal`, expressed as NAL byte indices.
fn epb_positions(nal: &[u8]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut zeros = 0usize;
    for (i, &b) in nal.iter().enumerate() {
        if zeros >= 2 && b == 3 {
            out.push(i);
            zeros = 0;
            continue;
        }
        if b == 0 {
            zeros += 1;
        } else {
            zeros = 0;
        }
    }
    out
}

/// RBSP byte offsets of the substreams of a slice segment (entry points are
/// counted in NAL bytes including emulation prevention bytes, 7.4.7.1).
pub fn substream_starts(nal: &[u8], data_offset: usize, offsets: &[u32], rbsp_len: usize) -> Option<Vec<usize>> {
    let epb = epb_positions(nal);
    // NAL index of the RBSP byte `data_offset`.
    let mut k = 0usize;
    for &e in &epb {
        if e <= data_offset + k {
            k += 1;
        } else {
            break;
        }
    }
    let mut n = data_offset + k;
    let mut out = Vec::with_capacity(offsets.len() + 1);
    out.push(data_offset);
    for &o in offsets {
        n = n.checked_add(o as usize)?;
        let before = epb.partition_point(|&e| e < n);
        let r = n - before;
        if r <= *out.last()? || r >= rbsp_len {
            return None;
        }
        out.push(r);
    }
    Some(out)
}

/// Shared synchronisation state of one parallel segment decode.
struct RowSync {
    /// CTUs completed per row.
    progress: Vec<AtomicUsize>,
    /// Context variables stored after the 2nd CTU of each row.
    wpp_ctx: Vec<Mutex<Option<[Ctx; NUM_CTX]>>>,
    /// The thread (if any) blocked waiting for progress of each row.
    waiter: Vec<Mutex<Option<std::thread::Thread>>>,
    parked: Vec<AtomicBool>,
    abort: AtomicBool,
}

/// Busy-wait iterations before a waiting row blocks (a CTU takes some 10 us).
const SPINS: u32 = 2000;

impl RowSync {
    fn new(n: usize) -> RowSync {
        RowSync {
            progress: (0..n).map(|_| AtomicUsize::new(0)).collect(),
            wpp_ctx: (0..n).map(|_| Mutex::new(None)).collect(),
            waiter: (0..n).map(|_| Mutex::new(None)).collect(),
            parked: (0..n).map(|_| AtomicBool::new(false)).collect(),
            abort: AtomicBool::new(false),
        }
    }

    /// Waits until row `r` has completed `n` CTUs. Returns false on abort.
    fn wait(&self, r: usize, n: usize) -> bool {
        let p = &self.progress[r];
        for _ in 0..SPINS {
            if p.load(Ordering::Acquire) >= n {
                return true;
            }
            if self.abort.load(Ordering::Relaxed) {
                return false;
            }
            std::hint::spin_loop();
        }
        // Block: register as the waiter of row r, then re-check (the
        // producer checks `parked` after publishing progress; SeqCst on
        // both sides prevents lost wake-ups).
        *self.waiter[r].lock().unwrap_or_else(|e| e.into_inner()) = Some(std::thread::current());
        self.parked[r].store(true, Ordering::SeqCst);
        let ok = loop {
            if p.load(Ordering::SeqCst) >= n {
                break true;
            }
            if self.abort.load(Ordering::SeqCst) {
                break false;
            }
            std::thread::park_timeout(std::time::Duration::from_millis(5));
        };
        self.parked[r].store(false, Ordering::SeqCst);
        // Pairs with the Release/SeqCst stores of the producer.
        std::sync::atomic::fence(Ordering::Acquire);
        ok
    }

    /// Publishes that row `r` has completed `n` CTUs.
    fn publish(&self, r: usize, n: usize) {
        self.progress[r].store(n, Ordering::SeqCst);
        if self.parked[r].load(Ordering::SeqCst) {
            self.wake(r);
        }
    }

    fn wake(&self, r: usize) {
        if let Some(t) = self.waiter[r].lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            t.unpark();
        }
    }

    /// Stops all rows (after an error) and wakes blocked waiters.
    fn abort(&self) {
        self.abort.store(true, Ordering::SeqCst);
        for r in 0..self.parked.len() {
            if self.parked[r].load(Ordering::SeqCst) {
                self.wake(r);
            }
        }
    }
}

/// Result of decoding one row.
struct RowOut {
    sc: StScalars,
    /// Whether the row stored WPP contexts.
    stored_ctx: bool,
}

/// Inputs shared by all row decoders.
pub struct SegmentJob<'a> {
    pub sps: &'a Sps,
    pub pps: &'a Pps,
    pub sh: &'a SliceHeader,
    pub layout: &'a TileLayout,
    pub scaling: Option<&'a ScalingFactors>,
    pub rbsp: &'a [u8],
    pub nal: &'a [u8],
    pub dpb: &'a [Picture],
    pub ref_dpb: [[usize; 16]; 2],
    pub hdr_idx: u16,
}

/// Decodes the slice segment with one task per CTU row if it qualifies.
/// Returns `None` if the segment must be decoded sequentially instead (the
/// picture state is then unchanged).
pub fn decode_segment(job: &SegmentJob, pool: &Pool, pic: &mut Picture, st: &mut PicState) -> Option<Result<()>> {
    let sps = job.sps;
    let pps = job.pps;
    let sh = job.sh;
    let w = sps.pic_width_in_ctbs;
    let h = sps.pic_height_in_ctbs;
    if !pps.entropy_coding_sync_enabled || pps.tiles_enabled || w < 2 || sh.entry_point_offsets.is_empty() {
        return None;
    }
    if sh.segment_address % w != 0 {
        return None;
    }
    let first_row = sh.segment_address / w;
    let n_rows = sh.entry_point_offsets.len() + 1;
    if first_row + n_rows > h {
        return None;
    }
    let starts = substream_starts(job.nal, sh.data_offset, &sh.entry_point_offsets, job.rbsp.len())?;
    // Preconditions checked by the sequential decoder: leave errors to it.
    let rs0 = sh.segment_address;
    if st.ctb_hdr[rs0] != u16::MAX {
        return None;
    }
    if sh.dependent_slice_segment && (rs0 == 0 || st.ctb_slice_addr[rs0 - 1] != sh.slice_addr_rs) {
        return None;
    }
    let sc0 = st.scalars();
    let pv = PicView::new(pic);
    let sv = StView::new(st);
    let sync = RowSync::new(n_rows);
    let results: Vec<Mutex<Option<Result<RowOut>>>> = (0..n_rows).map(|_| Mutex::new(None)).collect();
    pool.run(n_rows, &|i| {
        let r = catch_unwind(AssertUnwindSafe(|| decode_row(job, &sync, pv, sv, sc0, &starts, first_row, i)))
            .unwrap_or(Err(Error::Invalid("panic in WPP row")));
        if r.is_err() {
            sync.abort();
        }
        *results[i].lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
    });
    let mut outs = Vec::with_capacity(n_rows);
    for r in results {
        match r.into_inner().unwrap_or_else(|e| e.into_inner()) {
            Some(Ok(o)) => outs.push(o),
            other => {
                if std::env::var_os("STUDIO_HEVC_WPP_DEBUG").is_some() {
                    eprintln!("wpp fallback: poc {} rows {first_row}+{n_rows}: {:?}", pic.poc, other.map(|r| r.err()));
                }
                STATS_FALLBACK.fetch_add(1, Ordering::Relaxed);
                reset_rows(pic, st, sps, first_row, n_rows);
                st.set_scalars(&sc0);
                return None;
            }
        }
    }
    // Combine the scalar state as sequential decoding would leave it.
    let last = outs.last().expect("rows").sc;
    let mut sc = sc0;
    sc.qp_y_prev = last.qp_y_prev;
    sc.ds_ctx = last.ds_ctx;
    sc.any_nofilter = outs.iter().any(|o| o.sc.any_nofilter);
    sc.ctbs_decoded = sc0.ctbs_decoded + outs.iter().map(|o| o.sc.ctbs_decoded - sc0.ctbs_decoded).sum::<usize>();
    if let Some(o) = outs.iter().rev().find(|o| o.stored_ctx) {
        sc.wpp_ctx = o.sc.wpp_ctx;
    }
    st.set_scalars(&sc);
    STATS_PARALLEL.fetch_add(1, Ordering::Relaxed);
    Some(Ok(()))
}

/// Restores the "not decoded" state of CTB rows after a failed parallel
/// attempt (maps that sequential decoding reads before writing).
fn reset_rows(pic: &mut Picture, st: &mut PicState, sps: &Sps, first_row: usize, n_rows: usize) {
    let w = sps.pic_width_in_ctbs;
    let rs0 = first_row * w;
    let rs1 = ((first_row + n_rows) * w).min(st.ctb_hdr.len());
    pic.ctb_slice[rs0..rs1].fill(0);
    st.ctb_hdr[rs0..rs1].fill(u16::MAX);
    st.ctb_slice_addr[rs0..rs1].fill(usize::MAX);
    let w4 = st.w4;
    let y4a = (first_row << sps.log2_ctb) >> 2;
    let y4b = (((first_row + n_rows) << sps.log2_ctb) >> 2).min(st.h4);
    st.flags[y4a * w4..y4b * w4].fill(0);
    st.edge_v[y4a * w4..y4b * w4].fill(0);
    st.edge_h[y4a * w4..y4b * w4].fill(0);
}

#[allow(clippy::too_many_arguments)]
fn decode_row(
    job: &SegmentJob,
    sync: &RowSync,
    pv: PicView,
    sv: StView,
    sc0: StScalars,
    starts: &[usize],
    first_row: usize,
    i: usize,
) -> Result<RowOut> {
    let mut sd = SliceDecoder::from_views(
        job.sps,
        job.pps,
        job.sh,
        job.layout,
        job.scaling,
        job.rbsp,
        pv,
        sv,
        sc0,
        job.dpb,
        job.ref_dpb,
        job.hdr_idx,
    );
    let r = sd.decode_wpp_row(sync, starts, first_row, i);
    if r.is_err() {
        sync.abort();
    }
    // Let waiting rows observe the end of this row in any case.
    let stored = r?;
    Ok(RowOut { sc: sd.sc, stored_ctx: stored })
}

impl<'a> SliceDecoder<'a> {
    /// Decodes CTU row `i` of the segment (absolute row `first_row + i`).
    /// Returns whether WPP context variables were stored by this row.
    fn decode_wpp_row(&mut self, sync: &RowSync, starts: &[usize], first_row: usize, i: usize) -> Result<bool> {
        let w = self.sps.pic_width_in_ctbs;
        let n_rows = starts.len();
        let y = first_row + i;
        let rs0 = y * w;
        if i > 0 {
            self.cabac.e.reinit(starts[i]);
        }
        self.ctb_addr_rs = rs0;
        self.ctb_addr_ts = self.layout.rs_to_ts[rs0];
        self.tile_id = self.layout.tile_id[self.ctb_addr_ts];
        // Initial context selection (as in `decode`, 9.3.1).
        if i == 0 {
            if self.ctb_addr_ts != 0 {
                self.st.ctb_slice_addr.set(rs0, self.sh.slice_addr_rs);
                self.wpp_row_start_contexts(0, y);
            }
        } else {
            if !sync.wait(i - 1, 2) {
                return Err(Error::Invalid("WPP: aborted"));
            }
            self.sc.wpp_ctx = *sync.wpp_ctx[i - 1].lock().unwrap_or_else(|e| e.into_inner());
            self.st.ctb_slice_addr.set(rs0, self.sh.slice_addr_rs);
            self.wpp_row_start_contexts(0, y);
        }
        self.first_qg = true;
        self.sc.qp_y_prev = self.sh.slice_qp;
        self.qp_y = self.sh.slice_qp;
        let mut stored = false;
        for x in 0..w {
            if i > 0 && !sync.wait(i - 1, (x + 2).min(w)) {
                return Err(Error::Invalid("WPP: aborted"));
            }
            let rs = rs0 + x;
            self.ctb_addr_rs = rs;
            self.ctb_addr_ts = self.layout.rs_to_ts[rs];
            if self.st.ctb_hdr.get(rs) != u16::MAX {
                return Err(Error::Invalid("CTB decoded twice"));
            }
            self.st.ctb_hdr.set(rs, self.hdr_idx);
            self.st.ctb_slice_addr.set(rs, self.sh.slice_addr_rs);
            self.pic.ctb_slice.set(rs, self.hdr_idx);
            self.coding_tree_unit(x, y)?;
            self.sc.ctbs_decoded += 1;
            if self.cabac.overrun() {
                return Err(Error::UnexpectedEof);
            }
            if x == 1 {
                self.sc.wpp_ctx = Some(self.cabac.ctx);
                *sync.wpp_ctx[i].lock().unwrap_or_else(|e| e.into_inner()) = Some(self.cabac.ctx);
                stored = true;
            }
            let end_of_slice_segment = self.cabac.end_of_slice_segment_flag();
            self.sc.qp_y_prev = self.qp_y;
            sync.publish(i, x + 1);
            if end_of_slice_segment {
                if i + 1 != n_rows {
                    return Err(Error::Invalid("WPP: slice segment ends before its last substream"));
                }
                if self.pps.dependent_slice_segments_enabled {
                    self.sc.ds_ctx = Some(self.cabac.ctx);
                }
                return Ok(stored);
            }
            if x + 1 == w {
                if i + 1 == n_rows {
                    return Err(Error::Invalid("WPP: slice segment continues past its substreams"));
                }
                let pos = self.cabac.end_of_subset_pos()?;
                if pos != starts[i + 1] {
                    return Err(Error::Invalid("WPP: substream does not end at the next entry point"));
                }
            }
        }
        Ok(stored)
    }
}

