// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Deblocking overlapped with decoding (progressive frames, macroblocks in raster order).
//!
//! The reconstruction hands every finished macroblock row (its samples plus the per-MB
//! deblocking and motion data) to a helper thread as an owned copy. The helper assembles
//! those rows in its own planes and filters each row as soon as it arrives. Intra
//! prediction of later rows keeps reading the decoder's unfiltered planes, and the helper's
//! planes are only touched by the helper, so no memory is shared. Filtering rows in arrival
//! (raster) order is exactly the sequential filter order, so the result is bit-identical to
//! deblocking the finished picture.
//!
//! Anything unusual (macroblocks not decoded in raster order, slice errors, concealment, FMO,
//! data partitioning) breaks the pipe and the caller deblocks its own planes as before.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;

use crate::deblock::{DbMb, DbSlice, DeblockCtx, deblock_rows, deblock_rows_from};
use crate::picture::{MbMotion, Planes};

/// One finished macroblock row.
struct RowMsg {
    row: usize,
    /// Whole padded lines of the row (16 luma, 8 per chroma plane).
    y: Vec<u8>,
    cb: Vec<u8>,
    cr: Vec<u8>,
    db: Vec<DbMb>,
    motion: Vec<MbMotion>,
    /// Slice parameters, when they changed since the previous row.
    slices: Option<Vec<DbSlice>>,
}

pub struct DeblockPipe {
    tx: Option<Sender<RowMsg>>,
    handle: Option<JoinHandle<Option<Planes>>>,
    mb_w: usize,
    mb_h: usize,
    /// Next macroblock address expected (raster order).
    next_addr: usize,
    rows_sent: usize,
    broken: bool,
    slices_sent: usize,
    pending_slices: Option<Vec<DbSlice>>,
    /// Deblocking / motion data of the row being completed.
    row_db: Vec<DbMb>,
    row_motion: Vec<MbMotion>,
}

/// Rows still to filter when the last row arrives from which the helper switches to the
/// parallel (wavefront) filter for the rest.
const WAVEFRONT_BACKLOG: usize = 2;

impl DeblockPipe {
    /// Start the helper for a progressive 4:2:0 frame of `mb_w` x `mb_h` macroblocks.
    pub fn start(mb_w: usize, mb_h: usize) -> Option<DeblockPipe> {
        let (tx, rx) = channel::<RowMsg>();
        let handle = std::thread::Builder::new()
            .name("h264-deblock".into())
            .spawn(move || helper(rx, mb_w, mb_h))
            .ok()?;
        Some(DeblockPipe {
            tx: Some(tx),
            handle: Some(handle),
            mb_w,
            mb_h,
            next_addr: 0,
            rows_sent: 0,
            broken: false,
            slices_sent: 0,
            pending_slices: None,
            row_db: Vec::with_capacity(mb_w),
            row_motion: Vec::with_capacity(mb_w),
        })
    }

    /// Give up: the caller will deblock its own planes.
    pub fn abandon(&mut self) {
        self.broken = true;
        self.tx = None;
    }

    /// Current slice parameter list (call before decoding each slice).
    pub fn set_slices(&mut self, slices: &[DbSlice]) {
        if slices.len() != self.slices_sent {
            self.slices_sent = slices.len();
            self.pending_slices = Some(slices.to_vec());
        }
    }

    /// Macroblock `addr` is completely reconstructed; `db` / `motion` are its final
    /// deblocking and motion data.
    pub fn mb_done(&mut self, addr: usize, planes: &Planes, db: &DbMb, motion: &MbMotion) {
        if self.broken {
            return;
        }
        if addr != self.next_addr {
            self.abandon();
            return;
        }
        self.next_addr += 1;
        self.row_db.push(*db);
        self.row_motion.push(*motion);
        if self.next_addr % self.mb_w != 0 {
            return;
        }
        let row = addr / self.mb_w;
        let band = |p: &crate::picture::Plane, lines: usize| {
            let start = (row * lines + p.pad) * p.stride;
            p.data[start..start + lines * p.stride].to_vec()
        };
        let msg = RowMsg {
            row,
            y: band(&planes.y, 16),
            cb: band(&planes.cb, 8),
            cr: band(&planes.cr, 8),
            db: std::mem::replace(&mut self.row_db, Vec::with_capacity(self.mb_w)),
            motion: std::mem::replace(&mut self.row_motion, Vec::with_capacity(self.mb_w)),
            slices: self.pending_slices.take(),
        };
        let sent = self.tx.as_ref().is_some_and(|tx| tx.send(msg).is_ok());
        if sent {
            self.rows_sent += 1;
        } else {
            self.abandon();
        }
    }

    /// The deblocked planes, if every row went through the pipe.
    pub fn finish(mut self) -> Option<Planes> {
        let complete = !self.broken && self.rows_sent == self.mb_h;
        self.tx = None;
        let handle = self.handle.take()?;
        if !complete {
            // The helper stops at the closed channel; nothing waits for it.
            return None;
        }
        handle.join().ok().flatten()
    }
}

impl Drop for DeblockPipe {
    fn drop(&mut self) {
        // Closing the channel ends the helper; it is not joined (it owns all its data).
        self.tx = None;
    }
}

fn helper(rx: Receiver<RowMsg>, mb_w: usize, mb_h: usize) -> Option<Planes> {
    let n = mb_w * mb_h;
    let mut planes = Planes::new(mb_w * 16, mb_h * 16, true);
    let mut db = vec![DbMb { slice: u16::MAX, ..DbMb::default() }; n];
    let mut motion = vec![MbMotion::default(); n];
    let mut slices: Vec<DbSlice> = Vec::new();
    let mut received = 0usize;
    let mut filtered = 0usize;
    while filtered < mb_h {
        // Take everything that has arrived, then filter the next row.
        if received == filtered {
            match rx.recv() {
                Ok(m) if m.row == received => {
                    store(m, mb_w, &mut planes, &mut db, &mut motion, &mut slices);
                    received += 1;
                }
                Ok(_) => return None,
                Err(_) => return None,
            }
        }
        while let Ok(m) = rx.try_recv() {
            if m.row != received {
                return None;
            }
            store(m, mb_w, &mut planes, &mut db, &mut motion, &mut slices);
            received += 1;
        }
        let cx = DeblockCtx {
            mbs: &db,
            motion: &motion,
            slices: &slices,
            mb_w,
            mb_h,
            mbaff: false,
            field_pic: false,
            chroma: true,
        };
        if received == mb_h && mb_h - filtered >= WAVEFRONT_BACKLOG {
            // The decoder is done and waits for us: finish in parallel.
            deblock_rows_from(&mut planes, &cx, filtered, true);
            filtered = mb_h;
        } else {
            deblock_rows(&mut planes, &cx, filtered..filtered + 1);
            filtered += 1;
        }
    }
    Some(planes)
}

/// Copy a received row into the helper's picture.
fn store(m: RowMsg, mb_w: usize, planes: &mut Planes, db: &mut [DbMb], motion: &mut [MbMotion], slices: &mut Vec<DbSlice>) {
    let put = |p: &mut crate::picture::Plane, lines: usize, src: &[u8]| {
        let start = (m.row * lines + p.pad) * p.stride;
        p.data[start..start + src.len()].copy_from_slice(src);
    };
    put(&mut planes.y, 16, &m.y);
    put(&mut planes.cb, 8, &m.cb);
    put(&mut planes.cr, 8, &m.cr);
    let span = m.row * mb_w..(m.row + 1) * mb_w;
    db[span.clone()].copy_from_slice(&m.db);
    motion[span].copy_from_slice(&m.motion);
    if let Some(s) = m.slices {
        *slices = s;
    }
}
