// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Macroblock reconstruction: intra prediction + residual, inter prediction (motion
//! compensation with weighting) + residual. Field macroblocks of MBAFF frames work on a
//! view of every other line of the frame.
//!
//! The slice decoder captures everything reconstruction needs in an [`MbRec`] (sample
//! positions, prediction modes, precomputed intra neighbour availability, dequantised
//! coefficients, motion), so reconstruction never looks at the parsing state and can run on
//! a separate thread, in macroblock order, behind the entropy decoder.

use crate::dbpipe::DeblockPipe;
use crate::deblock::DbMb;
use crate::error::{Error, Result};
use crate::inter::{Weight, mc_chroma, mc_luma, put_bi, put_single};
use crate::intra::{filter8, gather, pred16x16, pred_chroma, pred_nxn};
use crate::mb::*;
use crate::picture::{MbMotion, PAR_FRAME, Planes};
use crate::slicedec::{PredWeights, RefPic, SliceDecoder};
use crate::tables::{BLK4_XY, RASTER_TO_BLK4};
use crate::transform::{idct4_add, idct4_dc_add, idct8_add};

/// For each luma4x4BlkIdx: is the top-right 4x4 block inside the MB and already decoded
/// (only meaningful for blocks not on the top row / right column).
const TOPRIGHT_INSIDE: [bool; 16] = {
    let mut t = [false; 16];
    let mut b = 0;
    while b < 16 {
        let (x, y) = BLK4_XY[b];
        if y > 0 && x < 3 {
            t[b] = RASTER_TO_BLK4[(y - 1) * 4 + x + 1] < b;
        }
        b += 1;
    }
    t
};

/// Intra availability bits (per block in [`MbRec::avail`], per MB in [`MbRec::avail_mb`]).
const AV_LEFT: u8 = 1;
const AV_TOP: u8 = 2;
const AV_TOPLEFT: u8 = 4;
const AV_TOPRIGHT: u8 = 8;

/// Everything needed to reconstruct one macroblock.
#[derive(Clone)]
pub(crate) struct MbRec {
    pub addr: usize,
    /// Sample view of the macroblock in the target planes: origin and row step.
    pub lx0: usize,
    pub ly0: usize,
    pub cx0: usize,
    pub cy0: usize,
    pub vstep: usize,
    /// Origin in reference coordinates (fields for MBAFF field macroblocks).
    pub rx0: usize,
    pub ry0: usize,
    pub rcx0: usize,
    pub rcy0: usize,
    /// Parity of the current macroblock (PAR_FRAME for frame macroblocks).
    pub cur_par: u8,
    /// MBAFF field macroblock (field reference lists, refIdx >> 1 weights) and its parity.
    pub mbaff_field: bool,
    pub is_bottom: bool,
    pub kind: MbKind,
    pub transform_8x8: bool,
    pub pred_modes: [u8; 16],
    pub chroma_pred: u8,
    /// Intra neighbour availability per 4x4 block (8x8 blocks: first four entries).
    pub avail: [u8; 16],
    /// Intra neighbour availability of the whole macroblock (16x16 and chroma prediction).
    pub avail_mb: u8,
    pub coef: [i32; 384],
    pub luma_nz: u16,
    pub chroma_nz: u8,
    /// I_PCM samples (luma then Cb, Cr).
    pub pcm: [u8; 384],
    pub mv: [[[i16; 2]; 16]; 2],
    pub ref_idx: [[i8; 4]; 2],
    /// Blocks of uniform motion: (x, y, w, h) in 4x4 units.
    pub blocks: [(u8, u8, u8, u8); 16],
    pub nblocks: u8,
    /// Deblocking and motion data of the macroblock (for the deblocking pipe).
    pub db: DbMb,
    pub motion: MbMotion,
}

impl MbRec {
    pub fn boxed() -> Box<MbRec> {
        Box::new(MbRec {
            addr: 0,
            lx0: 0,
            ly0: 0,
            cx0: 0,
            cy0: 0,
            vstep: 1,
            rx0: 0,
            ry0: 0,
            rcx0: 0,
            rcy0: 0,
            cur_par: PAR_FRAME,
            mbaff_field: false,
            is_bottom: false,
            kind: MbKind::PSkip,
            transform_8x8: false,
            pred_modes: [0; 16],
            chroma_pred: 0,
            avail: [0; 16],
            avail_mb: 0,
            coef: [0; 384],
            luma_nz: 0,
            chroma_nz: 0,
            pcm: [0; 384],
            mv: [[[0; 2]; 16]; 2],
            ref_idx: [[-1; 4]; 2],
            blocks: [(0, 0, 0, 0); 16],
            nblocks: 0,
            db: DbMb::default(),
            motion: MbMotion::default(),
        })
    }

    #[inline]
    fn lofs(&self, p: &Planes, x: usize, y: usize) -> usize {
        p.y.idx((self.lx0 + x) as isize, (self.ly0 + y * self.vstep) as isize)
    }

    #[inline]
    fn cofs(&self, p: &Planes, x: usize, y: usize) -> usize {
        p.cb.idx((self.cx0 + x) as isize, (self.cy0 + y * self.vstep) as isize)
    }
}

/// Slice-level data reconstruction reads (shared, immutable).
#[derive(Clone, Copy)]
pub(crate) struct ReconEnv<'a> {
    pub refs: &'a [Vec<RefPic>; 2],
    pub field_refs: Option<&'a [[Vec<RefPic>; 2]; 2]>,
    pub weights: &'a PredWeights,
    pub chroma: bool,
}

impl<'a> ReconEnv<'a> {
    fn ref_list(&self, rec: &MbRec, list: usize) -> &'a [RefPic] {
        if rec.mbaff_field {
            if let Some(fr) = self.field_refs {
                return &fr[rec.is_bottom as usize][list];
            }
        }
        &self.refs[list]
    }
}

/// Where reconstructed macroblocks go: the target planes, and the deblocking pipe.
pub(crate) struct ReconSink<'a> {
    pub planes: &'a mut Planes,
    pub pipe: Option<&'a mut DeblockPipe>,
}

impl ReconSink<'_> {
    pub fn run(&mut self, rec: &MbRec, env: &ReconEnv) {
        reconstruct(rec, env, self.planes);
        if let Some(p) = self.pipe.as_deref_mut() {
            p.mb_done(rec.addr, self.planes, &rec.db, &rec.motion);
        }
    }
}

/// Reconstruct one macroblock into the planes.
pub(crate) fn reconstruct(rec: &MbRec, env: &ReconEnv, planes: &mut Planes) {
    match rec.kind {
        MbKind::IPcm => put_pcm(rec, env, planes),
        k if k.is_intra() => intra_reconstruct(rec, env, planes),
        _ => {
            inter_prediction(rec, env, planes);
            add_inter_residual(rec, env, planes);
        }
    }
}

fn put_pcm(rec: &MbRec, env: &ReconEnv, planes: &mut Planes) {
    for y in 0..16 {
        let o = rec.lofs(planes, 0, y);
        planes.y.data[o..o + 16].copy_from_slice(&rec.pcm[y * 16..y * 16 + 16]);
    }
    if env.chroma {
        for c in 0..2 {
            for y in 0..8 {
                let o = rec.cofs(planes, 0, y);
                let plane = if c == 0 { &mut planes.cb } else { &mut planes.cr };
                let s = 256 + c * 64 + y * 8;
                plane.data[o..o + 8].copy_from_slice(&rec.pcm[s..s + 8]);
            }
        }
    }
}

fn add_luma4(rec: &MbRec, planes: &mut Planes, blk: usize) {
    if rec.luma_nz >> blk & 1 == 0 {
        return;
    }
    let (x, y) = BLK4_XY[blk];
    let off = rec.lofs(planes, x * 4, y * 4);
    let stride = planes.y.stride * rec.vstep;
    let d: &[i32; 16] = rec.coef[blk * 16..blk * 16 + 16].try_into().unwrap();
    if d[1..].iter().all(|&v| v == 0) {
        idct4_dc_add(d[0], &mut planes.y.data, off, stride);
    } else {
        idct4_add(d, &mut planes.y.data, off, stride);
    }
}

fn add_luma8(rec: &MbRec, planes: &mut Planes, b8: usize) {
    if rec.luma_nz >> b8 & 1 == 0 {
        return;
    }
    let off = rec.lofs(planes, (b8 & 1) * 8, (b8 >> 1) * 8);
    let stride = planes.y.stride * rec.vstep;
    let d: &[i32; 64] = rec.coef[b8 * 64..b8 * 64 + 64].try_into().unwrap();
    idct8_add(d, &mut planes.y.data, off, stride);
}

fn add_chroma(rec: &MbRec, env: &ReconEnv, planes: &mut Planes) {
    if !env.chroma || rec.chroma_nz == 0 {
        return;
    }
    let stride = planes.cb.stride * rec.vstep;
    for c in 0..2 {
        for b in 0..4 {
            if rec.chroma_nz >> (c * 4 + b) & 1 == 0 {
                continue;
            }
            let off = rec.cofs(planes, (b & 1) * 4, (b >> 1) * 4);
            let s = 256 + c * 64 + b * 16;
            let d: &[i32; 16] = rec.coef[s..s + 16].try_into().unwrap();
            let plane = if c == 0 { &mut planes.cb } else { &mut planes.cr };
            if d[1..].iter().all(|&v| v == 0) {
                idct4_dc_add(d[0], &mut plane.data, off, stride);
            } else {
                idct4_add(d, &mut plane.data, off, stride);
            }
        }
    }
}

fn intra_reconstruct(rec: &MbRec, env: &ReconEnv, planes: &mut Planes) {
    let stride = planes.y.stride * rec.vstep;
    let av = |a: u8, bit: u8| a & bit != 0;
    match rec.kind {
        MbKind::INxN if !rec.transform_8x8 => {
            for blk in 0..16 {
                let (x, y) = BLK4_XY[blk];
                let a = rec.avail[blk];
                let off = rec.lofs(planes, x * 4, y * 4);
                let plane = &mut planes.y;
                let e = gather(&plane.data, off, stride, 4, av(a, AV_TOP), av(a, AV_LEFT), av(a, AV_TOPLEFT), av(a, AV_TOPRIGHT));
                pred_nxn(rec.pred_modes[blk], &e, 4, &mut plane.data, off, stride);
                add_luma4(rec, planes, blk);
            }
        }
        MbKind::INxN => {
            for b8 in 0..4 {
                let (x8, y8) = (b8 & 1, b8 >> 1);
                let a = rec.avail[b8];
                let off = rec.lofs(planes, x8 * 8, y8 * 8);
                let plane = &mut planes.y;
                let e = gather(&plane.data, off, stride, 8, av(a, AV_TOP), av(a, AV_LEFT), av(a, AV_TOPLEFT), av(a, AV_TOPRIGHT));
                let f = filter8(&e);
                pred_nxn(rec.pred_modes[b8], &f, 8, &mut plane.data, off, stride);
                add_luma8(rec, planes, b8);
            }
        }
        MbKind::I16x16 { mode, .. } => {
            let off = rec.lofs(planes, 0, 0);
            pred16x16(mode, &mut planes.y.data, off, stride, av(rec.avail_mb, AV_TOP), av(rec.avail_mb, AV_LEFT));
            for blk in 0..16 {
                add_luma4(rec, planes, blk);
            }
        }
        _ => {}
    }
    if env.chroma {
        let off = rec.cofs(planes, 0, 0);
        let cs = planes.cb.stride * rec.vstep;
        for c in 0..2 {
            let plane = if c == 0 { &mut planes.cb } else { &mut planes.cr };
            pred_chroma(rec.chroma_pred, &mut plane.data, off, cs, av(rec.avail_mb, AV_TOP), av(rec.avail_mb, AV_LEFT));
        }
        add_chroma(rec, env, planes);
    }
}

fn add_inter_residual(rec: &MbRec, env: &ReconEnv, planes: &mut Planes) {
    if rec.transform_8x8 {
        for b8 in 0..4 {
            add_luma8(rec, planes, b8);
        }
    } else if rec.luma_nz != 0 {
        for blk in 0..16 {
            add_luma4(rec, planes, blk);
        }
    }
    add_chroma(rec, env, planes);
}

fn weights(env: &ReconEnv, rec: &MbRec, r0: i8, r1: i8) -> [Weight; 3] {
    let wp = env.weights;
    let mbaff_field = rec.mbaff_field;
    match wp.mode {
        1 => {
            let lw = wp.luma_log2 as i32;
            let cw = wp.chroma_log2 as i32;
            // refIdxLXWP = refIdxLX >> 1 for MBAFF field macroblocks (8.4.2.3).
            let sh = mbaff_field as u32;
            let get = |l: usize, r: i8| wp.explicit[l].get((r as usize) >> sh).copied().unwrap_or_default();
            if r0 >= 0 && r1 >= 0 {
                let (e0, e1) = (get(0, r0), get(1, r1));
                let mut out = [Weight::Weighted {
                    log_wd: lw,
                    w0: e0.luma_weight,
                    w1: e1.luma_weight,
                    o0: e0.luma_offset,
                    o1: e1.luma_offset,
                }; 3];
                for c in 0..2 {
                    out[1 + c] = Weight::Weighted {
                        log_wd: cw,
                        w0: e0.chroma_weight[c],
                        w1: e1.chroma_weight[c],
                        o0: e0.chroma_offset[c],
                        o1: e1.chroma_offset[c],
                    };
                }
                out
            } else {
                let (l, r) = if r0 >= 0 { (0, r0) } else { (1, r1) };
                let e = get(l, r);
                let mut out = [Weight::Weighted { log_wd: lw, w0: e.luma_weight, w1: 0, o0: e.luma_offset, o1: 0 }; 3];
                for c in 0..2 {
                    out[1 + c] =
                        Weight::Weighted { log_wd: cw, w0: e.chroma_weight[c], w1: 0, o0: e.chroma_offset[c], o1: 0 };
                }
                out
            }
        }
        2 if r0 >= 0 && r1 >= 0 => {
            let (w0, w1) = if mbaff_field {
                let t = &wp.implicit_field[rec.is_bottom as usize];
                t.get(r0 as usize * 2 * wp.n1 + r1 as usize).copied().unwrap_or((32, 32))
            } else {
                wp.implicit.get(r0 as usize * wp.n1 + r1 as usize).copied().unwrap_or((32, 32))
            };
            [Weight::Weighted { log_wd: 5, w0, w1, o0: 0, o1: 0 }; 3]
        }
        _ => [Weight::Default; 3],
    }
}

/// Scratch buffers for one block's predictions (per list: luma, Cb, Cr), kept off the
/// zero-initialised stack frame.
struct McTmp {
    luma: [[u8; 256]; 2],
    chroma: [[[u8; 256]; 2]; 2],
}

thread_local! {
    static MC_TMP: std::cell::RefCell<Box<McTmp>> =
        std::cell::RefCell::new(Box::new(McTmp { luma: [[0; 256]; 2], chroma: [[[0; 256]; 2]; 2] }));
}

/// Motion-compensated prediction of the macroblock into the target planes. Reference
/// indices were validated by the parser ([`SliceDecoder::check_refs`]).
fn inter_prediction(rec: &MbRec, env: &ReconEnv, planes: &mut Planes) {
    MC_TMP.with(|t| inter_prediction_with(rec, env, planes, &mut t.borrow_mut()));
}

fn inter_prediction_with(rec: &MbRec, env: &ReconEnv, planes: &mut Planes, t: &mut McTmp) {
    let (rx0, ry0, rcx0, rcy0) = (rec.rx0, rec.ry0, rec.rcx0, rec.rcy0);
    let cur_par = rec.cur_par;
    let lstride = planes.y.stride * rec.vstep;
    let cstride = planes.cb.stride * rec.vstep;
    for &(x, y, w, h) in &rec.blocks[..rec.nblocks as usize] {
        let (x, y, w, h) = (x as usize, y as usize, w as usize, h as usize);
        let b8 = (y >> 1) * 2 + (x >> 1);
        let r = [rec.ref_idx[0][b8], rec.ref_idx[1][b8]];
        let px = (rx0 + x * 4) as i32;
        let py = (ry0 + y * 4) as i32;
        for list in 0..2 {
            if r[list] < 0 {
                continue;
            }
            let rp = &env.ref_list(rec, list)[r[list] as usize];
            let mv = rec.mv[list][y * 4 + x];
            let (mx, my) = (mv[0] as i32, mv[1] as i32);
            let (ry, rcb, rcr) = rp.pic.planes(rp.parity);
            mc_luma(ry, px + (mx >> 2), py + (my >> 2), (mx & 3) as u32, (my & 3) as u32, w * 4, h * 4, &mut t.luma[list]);
            if env.chroma {
                // Chroma vector vertical offset between fields of different parity (8.4.1.4).
                let myc = if cur_par != PAR_FRAME && rp.parity != PAR_FRAME {
                    my + 2 * (cur_par as i32 - rp.parity as i32)
                } else {
                    my
                };
                let cx = (rcx0 + x * 2) as i32 + (mx >> 3);
                let cy = (rcy0 + y * 2) as i32 + (myc >> 3);
                let (fx, fy) = ((mx & 7) as u32, (myc & 7) as u32);
                mc_chroma(rcb, cx, cy, fx, fy, w * 2, h * 2, &mut t.chroma[list][0]);
                mc_chroma(rcr, cx, cy, fx, fy, w * 2, h * 2, &mut t.chroma[list][1]);
            }
        }
        let wts = weights(env, rec, r[0], r[1]);
        let off = rec.lofs(planes, x * 4, y * 4);
        let coff = rec.cofs(planes, x * 2, y * 2);
        if r[0] >= 0 && r[1] >= 0 {
            put_bi(&t.luma[0], &t.luma[1], w * 4, h * 4, wts[0], &mut planes.y.data, off, lstride);
        } else {
            let l = if r[0] >= 0 { 0 } else { 1 };
            put_single(&t.luma[l], w * 4, h * 4, wts[0], &mut planes.y.data, off, lstride);
        }
        if env.chroma {
            for c in 0..2 {
                let plane = if c == 0 { &mut planes.cb } else { &mut planes.cr };
                if r[0] >= 0 && r[1] >= 0 {
                    put_bi(&t.chroma[0][c], &t.chroma[1][c], w * 2, h * 2, wts[1 + c], &mut plane.data, coff, cstride);
                } else {
                    let l = if r[0] >= 0 { 0 } else { 1 };
                    put_single(&t.chroma[l][c], w * 2, h * 2, wts[1 + c], &mut plane.data, coff, cstride);
                }
            }
        }
    }
}

impl SliceDecoder<'_> {
    /// Is the luma sample at (x, y) relative to the current MB available for intra
    /// prediction (decoded, same slice, constrained intra prediction respected)?
    fn intra_avail_at(&self, x: i32, y: i32) -> bool {
        match self.locate(x, y, 16, 16) {
            None => false,
            Some((a, _, _)) => a == self.addr || !self.env.pps.constrained_intra_pred || self.info(a).intra(),
        }
    }

    /// Left column rows y0..y1 (exclusive) all available.
    fn left_avail(&self, y0: i32, y1: i32) -> bool {
        (y0..y1).all(|y| self.intra_avail_at(-1, y))
    }

    /// Intra neighbour availability of the current macroblock's blocks (per block, and for
    /// the whole macroblock).
    fn intra_availability(&self) -> ([u8; 16], u8) {
        let bits = |l: bool, t: bool, tl: bool, tr: bool| {
            (l as u8) * AV_LEFT | (t as u8) * AV_TOP | (tl as u8) * AV_TOPLEFT | (tr as u8) * AV_TOPRIGHT
        };
        let mut avail = [0u8; 16];
        match self.kind {
            MbKind::INxN if !self.transform_8x8 => {
                for blk in 0..16 {
                    let (x, y) = BLK4_XY[blk];
                    let (xi, yi) = (x as i32 * 4, y as i32 * 4);
                    let left = x > 0 || self.left_avail(yi, yi + 4);
                    let top = y > 0 || self.intra_avail_at(xi, -1);
                    let topleft = (x > 0 && y > 0) || self.intra_avail_at(xi - 1, yi - 1);
                    let topright = if y == 0 { self.intra_avail_at(xi + 4, -1) } else { TOPRIGHT_INSIDE[blk] };
                    avail[blk] = bits(left, top, topleft, topright);
                }
            }
            MbKind::INxN => {
                for b8 in 0..4 {
                    let (x8, y8) = (b8 & 1, b8 >> 1);
                    let (xi, yi) = (x8 as i32 * 8, y8 as i32 * 8);
                    let left = x8 > 0 || self.left_avail(yi, yi + 8);
                    let top = y8 > 0 || self.intra_avail_at(xi, -1);
                    let topleft = (x8 > 0 && y8 > 0) || self.intra_avail_at(xi - 1, yi - 1);
                    let topright = match b8 {
                        0 => self.intra_avail_at(8, -1),
                        1 => self.intra_avail_at(16, -1),
                        2 => true,
                        _ => false,
                    };
                    avail[b8] = bits(left, top, topleft, topright);
                }
            }
            _ => {}
        }
        let avail_mb = if matches!(self.kind, MbKind::I16x16 { .. }) || self.chroma {
            bits(self.left_avail(0, 16), self.intra_avail_at(0, -1), false, false)
        } else {
            0
        };
        (avail, avail_mb)
    }

    /// Every used reference index must name a picture of the list.
    pub(crate) fn check_refs(&self) -> Result<()> {
        for list in 0..2 {
            let n = self.ref_list(list).len();
            if self.mb_ref[list].iter().any(|&r| r >= 0 && r as usize >= n) {
                return Err(Error::Bitstream("reference index without picture"));
            }
        }
        Ok(())
    }

    /// Capture the reconstruction data of the finished macroblock (after `finish_mb`) in
    /// `self.rec` and reconstruct it, inline or on the reconstruction thread.
    pub(crate) fn submit_mb(&mut self) {
        let mbaff_field = self.mbaff && self.mb_field;
        let origin = if mbaff_field {
            (self.mbx * 16, self.mby * 16, self.mbx * 8, self.mby * 8)
        } else {
            (self.lx0, self.ly0, self.cx0, self.cy0)
        };
        let cur_par = self.cur_parity();
        let intra = self.kind.is_intra();
        let (avail, avail_mb) = if intra && self.kind != MbKind::IPcm { self.intra_availability() } else { ([0; 16], 0) };
        let mut blocks = [(0usize, 0usize, 0usize, 0usize); 16];
        let nblocks = if intra { 0 } else { self.motion_blocks(&mut blocks) };
        let rec = &mut *self.rec;
        rec.addr = self.addr;
        rec.lx0 = self.lx0;
        rec.ly0 = self.ly0;
        rec.cx0 = self.cx0;
        rec.cy0 = self.cy0;
        rec.vstep = self.vstep;
        (rec.rx0, rec.ry0, rec.rcx0, rec.rcy0) = origin;
        rec.cur_par = cur_par;
        rec.mbaff_field = mbaff_field;
        rec.is_bottom = self.is_bottom;
        rec.kind = self.kind;
        rec.transform_8x8 = self.transform_8x8;
        rec.luma_nz = self.luma_nz;
        rec.chroma_nz = self.chroma_nz;
        rec.avail = avail;
        rec.avail_mb = avail_mb;
        if intra {
            rec.pred_modes = self.pred_modes;
            rec.chroma_pred = self.cur.chroma_pred;
        } else {
            rec.mv = self.mb_mv;
            rec.ref_idx = self.mb_ref;
            for (d, &(x, y, w, h)) in rec.blocks.iter_mut().zip(&blocks[..nblocks]) {
                *d = (x as u8, y as u8, w as u8, h as u8);
            }
            rec.nblocks = nblocks as u8;
        }
        rec.db = self.st.db[self.addr];
        rec.motion = self.st.motion[self.addr];
        self.dispatch_mb();
    }

    /// Blocks of uniform motion in the current MB: (x, y, w, h) in 4x4 units.
    fn motion_blocks(&self, out: &mut [(usize, usize, usize, usize); 16]) -> usize {
        let inference = self.env.sps.direct_8x8_inference;
        let mut n = 0;
        let mut push = |o: &mut [(usize, usize, usize, usize); 16], b: (usize, usize, usize, usize)| {
            o[n] = b;
            n += 1;
        };
        match self.kind {
            MbKind::PSkip => push(out, (0, 0, 4, 4)),
            MbKind::BSkip | MbKind::BDirect16x16 => {
                if self.uniform_motion() {
                    push(out, (0, 0, 4, 4));
                } else if inference {
                    for b8 in 0..4 {
                        push(out, ((b8 & 1) * 2, (b8 >> 1) * 2, 2, 2));
                    }
                } else {
                    for y in 0..4 {
                        for x in 0..4 {
                            push(out, (x, y, 1, 1));
                        }
                    }
                }
            }
            MbKind::Inter { part, .. } => match part {
                Part::P16x16 => push(out, (0, 0, 4, 4)),
                Part::P16x8 => {
                    push(out, (0, 0, 4, 2));
                    push(out, (0, 2, 4, 2));
                }
                Part::P8x16 => {
                    push(out, (0, 0, 2, 4));
                    push(out, (2, 0, 2, 4));
                }
                Part::P8x8 => {
                    for i in 0..4 {
                        let st = self.sub_types[i];
                        let (x8, y8) = ((i & 1) * 2, (i >> 1) * 2);
                        if st.pred == 0 {
                            if inference {
                                push(out, (x8, y8, 2, 2));
                            } else {
                                for k in 0..4 {
                                    push(out, (x8 + (k & 1), y8 + (k >> 1), 1, 1));
                                }
                            }
                            continue;
                        }
                        let (w, h) = (st.w as usize, st.h as usize);
                        for j in 0..st.num as usize {
                            let (x, y) = match (w, h) {
                                (2, 2) => (x8, y8),
                                (2, 1) => (x8, y8 + j),
                                (1, 2) => (x8 + j, y8),
                                _ => (x8 + (j & 1), y8 + (j >> 1)),
                            };
                            push(out, (x, y, w, h));
                        }
                    }
                }
            },
            _ => {}
        }
        n
    }

    fn uniform_motion(&self) -> bool {
        let r = self.mb_ref;
        if r[0].iter().any(|&v| v != r[0][0]) || r[1].iter().any(|&v| v != r[1][0]) {
            return false;
        }
        for list in 0..2 {
            if r[list][0] >= 0 && self.mb_mv[list].iter().any(|&m| m != self.mb_mv[list][0]) {
                return false;
            }
        }
        true
    }

}
