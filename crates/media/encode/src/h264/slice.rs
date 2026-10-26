// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Slice encoding: the macroblock loop and CAVLC macroblock syntax (7.3.5).

use super::cabac::CabacWriter;
use super::cavlc::{write_block, Nc};
use super::deblock::DbMb;
use super::mb::{encode_mb, FrameIn, Kind, MbCoded, MbCtx, MbState, Neighbours, Recon};
use super::params::SliceHeader;
use super::tables::{cbp_code_inter, cbp_code_intra, BLK4_XY};
use crate::bits::{nal_unit, BitWriter};

pub struct SliceOut {
    /// The coded slice NAL unit (header byte + escaped RBSP).
    pub nal: Vec<u8>,
    pub db: Vec<DbMb>,
    pub mvs: Vec<[i16; 2]>,
}

/// Encode macroblock rows `rows` of the frame as one slice.
pub fn encode_slice(f: &FrameIn, hdr: &SliceHeader, rec: &mut Recon, rows: std::ops::Range<usize>) -> SliceOut {
    let mb_w = f.mb_w;
    let first = rows.start * mb_w;
    let end = rows.end * mb_w;
    let n = end - first;
    let mut states: Vec<MbState> = Vec::with_capacity(n);
    let mut db = Vec::with_capacity(n);
    let mut mvs = Vec::with_capacity(n);
    let mut w = BitWriter::with_capacity(n * 16);
    hdr.write(&mut w);
    let mut cabac = if hdr.cabac {
        w.align_one(); // cabac_alignment_one_bit
        Some(CabacWriter::new(hdr.qp, hdr.p))
    } else {
        None
    };
    let mut skip_run = 0u32;
    for addr in first..end {
        let (mbx, mby) = (addr % mb_w, addr / mb_w);
        let local = addr - first;
        let nb = Neighbours {
            a: (mbx > 0).then(|| local - 1),
            b: (addr >= first + mb_w).then(|| local - mb_w),
            c: (addr >= first + mb_w && mbx + 1 < mb_w).then(|| local - mb_w + 1),
            d: (addr >= first + mb_w && mbx > 0).then(|| local - mb_w - 1),
        };
        let cx = MbCtx { f, mbx, mby, nb, states: &states };
        let out = encode_mb(&cx, rec, hdr.p);
        match &mut cabac {
            Some(cw) => cw.write_mb(&out.coded, &out.state, &nb, &states, hdr.p, addr + 1 == end),
            None => {
                if out.coded.kind == Kind::PSkip {
                    skip_run += 1;
                } else {
                    if hdr.p {
                        w.ue(skip_run);
                        skip_run = 0;
                    }
                    write_mb_cavlc(&mut w, &out.coded, &out.state, &nb, &states, hdr.p);
                }
            }
        }
        states.push(out.state);
        db.push(out.db);
        mvs.push(out.mv);
    }
    match cabac {
        Some(cw) => {
            let bytes = cw.finish();
            w.put_bytes(&bytes);
        }
        None => {
            if skip_run > 0 {
                w.ue(skip_run);
            }
            w.trailing();
        }
    }
    let rbsp = w.finish();
    let nal = nal_unit(if hdr.idr { 3 } else { 2 }, if hdr.idr { 5 } else { 1 }, &rbsp);
    SliceOut { nal, db, mvs }
}

/// nC from the availability and counts of the left / top blocks.
fn nc_of(a: Option<u8>, b: Option<u8>) -> i32 {
    match (a, b) {
        (Some(a), Some(b)) => (a as i32 + b as i32 + 1) >> 1,
        (Some(a), None) => a as i32,
        (None, Some(b)) => b as i32,
        (None, None) => 0,
    }
}

/// nC of the luma block at raster (bx, by) of the current macroblock.
fn luma_nc(cur: &MbState, nb: &Neighbours, states: &[MbState], bx: usize, by: usize) -> i32 {
    let a = if bx > 0 { Some(cur.tc_luma[by * 4 + bx - 1]) } else { nb.a.map(|i| states[i].tc_luma[by * 4 + 3]) };
    let b = if by > 0 { Some(cur.tc_luma[(by - 1) * 4 + bx]) } else { nb.b.map(|i| states[i].tc_luma[12 + bx]) };
    nc_of(a, b)
}

fn chroma_nc(cur: &MbState, nb: &Neighbours, states: &[MbState], comp: usize, bx: usize, by: usize) -> i32 {
    let a = if bx > 0 { Some(cur.tc_chroma[comp][by * 2 + bx - 1]) } else { nb.a.map(|i| states[i].tc_chroma[comp][by * 2 + 1]) };
    let b = if by > 0 { Some(cur.tc_chroma[comp][bx]) } else { nb.b.map(|i| states[i].tc_chroma[comp][2 + bx]) };
    nc_of(a, b)
}

/// macroblock_layer() with CAVLC residuals.
fn write_mb_cavlc(w: &mut BitWriter, c: &MbCoded, cur: &MbState, nb: &Neighbours, states: &[MbState], p_slice: bool) {
    let off = if p_slice { 5 } else { 0 };
    let cbp_l = c.cbp & 15;
    let cbp_c = c.cbp >> 4;
    match c.kind {
        Kind::I4 => w.ue(off),
        Kind::I16 => w.ue(off + 1 + c.i16_mode as u32 + 4 * cbp_c as u32 + if cbp_l != 0 { 12 } else { 0 }),
        Kind::P16 => w.ue(0),
        Kind::PSkip => unreachable!("skipped macroblocks are not written"),
    }
    match c.kind {
        Kind::I4 => {
            for &rem in &c.i4_rem {
                if rem == 8 {
                    w.put_bit(true);
                } else {
                    w.put_bit(false);
                    w.put(3, rem as u32);
                }
            }
            w.ue(c.chroma_mode as u32);
        }
        Kind::I16 => w.ue(c.chroma_mode as u32),
        _ => {
            w.se(c.mvd[0]);
            w.se(c.mvd[1]);
        }
    }
    match c.kind {
        Kind::I4 => w.ue(cbp_code_intra(c.cbp)),
        Kind::P16 => w.ue(cbp_code_inter(c.cbp)),
        _ => {}
    }
    if c.cbp == 0 && c.kind != Kind::I16 {
        return;
    }
    w.se(0); // mb_qp_delta
    if c.kind == Kind::I16 {
        write_block(w, Nc::Luma(luma_nc(cur, nb, states, 0, 0)), &c.luma_dc);
    }
    for blk in 0..16 {
        if cbp_l & (1 << (blk / 4)) == 0 {
            continue;
        }
        let (bx, by) = BLK4_XY[blk];
        let nc = Nc::Luma(luma_nc(cur, nb, states, bx, by));
        if c.kind == Kind::I16 {
            write_block(w, nc, &c.luma[blk][1..]);
        } else {
            write_block(w, nc, &c.luma[blk]);
        }
    }
    if cbp_c != 0 {
        for comp in 0..2 {
            write_block(w, Nc::ChromaDc, &c.chroma_dc[comp]);
        }
    }
    if cbp_c == 2 {
        for comp in 0..2 {
            for b in 0..4 {
                let nc = Nc::Luma(chroma_nc(cur, nb, states, comp, b % 2, b / 2));
                write_block(w, nc, &c.chroma_ac[comp][b][1..]);
            }
        }
    }
}
