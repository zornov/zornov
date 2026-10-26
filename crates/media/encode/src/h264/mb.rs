// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Macroblock analysis and reconstruction: mode decision, motion search,
//! transform / quantisation and the decoder-exact reconstruction. The result
//! is a [`MbCoded`] (the syntax to write) and an [`MbState`] (what later
//! macroblocks and the entropy coders need to know about it).

use super::deblock::DbMb;
use super::pred::{self, RefPic, MV_OUTSIDE};
use super::tables::{BLK4_XY, CHROMA_QP, RASTER_TO_BLK4, ZIGZAG4};
use super::transform::*;
use crate::bits::se_len;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Kind {
    #[default]
    I4,
    I16,
    P16,
    PSkip,
}

/// Everything about a coded macroblock that its neighbours and the entropy
/// coders need.
#[derive(Clone, Copy, Debug, Default)]
pub struct MbState {
    pub kind: Kind,
    /// Intra_4x4 prediction modes by luma4x4BlkIdx (2 = DC for non-I4 MBs).
    pub i4_modes: [u8; 16],
    pub chroma_mode: u8,
    /// TotalCoeff per 4x4 luma block, raster (y * 4 + x); for I16 the AC count.
    pub tc_luma: [u8; 16],
    /// TotalCoeff of chroma AC blocks, [cb/cr][raster 2x2].
    pub tc_chroma: [[u8; 4]; 2],
    /// coded_block_flag of DC blocks: luma (I16), Cb, Cr.
    pub dc_coded: [bool; 3],
    pub cbp: u8,
    pub mv: [i16; 2],
    pub mvd: [i16; 2],
    pub qp: u8,
}

impl MbState {
    pub fn intra(&self) -> bool {
        matches!(self.kind, Kind::I4 | Kind::I16)
    }
    pub fn inter(&self) -> bool {
        matches!(self.kind, Kind::P16 | Kind::PSkip)
    }
}

/// The syntax of one macroblock. Coefficient arrays are in scan order.
#[derive(Clone)]
pub struct MbCoded {
    pub kind: Kind,
    /// I16 prediction mode.
    pub i16_mode: u8,
    /// prev_intra4x4_pred_mode_flag / rem_intra4x4_pred_mode per luma4x4BlkIdx
    /// (rem = 8 means "use the predicted mode").
    pub i4_rem: [u8; 16],
    pub chroma_mode: u8,
    pub mvd: [i32; 2],
    pub cbp: u8,
    pub luma_dc: [i32; 16],
    /// By luma4x4BlkIdx; for I16 index 0 is unused (AC = 1..15).
    pub luma: [[i32; 16]; 16],
    pub chroma_dc: [[i32; 4]; 2],
    /// [cb/cr][blkIdx]; index 0 unused (AC = 1..15).
    pub chroma_ac: [[[i32; 16]; 4]; 2],
}

impl Default for MbCoded {
    fn default() -> Self {
        MbCoded {
            kind: Kind::I4,
            i16_mode: 0,
            i4_rem: [8; 16],
            chroma_mode: 0,
            mvd: [0; 2],
            cbp: 0,
            luma_dc: [0; 16],
            luma: [[0; 16]; 16],
            chroma_dc: [[0; 4]; 2],
            chroma_ac: [[[0; 16]; 4]; 2],
        }
    }
}

/// Availability of neighbouring macroblocks (A left, B top, C top-right, D top-left)
/// as indices into the slice's state vector.
#[derive(Clone, Copy, Default)]
pub struct Neighbours {
    pub a: Option<usize>,
    pub b: Option<usize>,
    pub c: Option<usize>,
    pub d: Option<usize>,
}

/// Lagrange multiplier for SAD/SATD based decisions.
pub fn lambda(qp: i32) -> f32 {
    0.92 * 2f32.powf((qp - 12) as f32 / 6.0)
}

/// Read-only inputs shared by all macroblocks of a slice.
pub struct FrameIn<'a> {
    pub mb_w: usize,
    pub mb_h: usize,
    /// Source planes, coded size, tightly packed.
    pub src_y: &'a [u8],
    pub src_u: &'a [u8],
    pub src_v: &'a [u8],
    pub refp: Option<&'a RefPic>,
    /// Motion vectors of the previous frame per macroblock (search candidates).
    pub prev_mv: &'a [[i16; 2]],
    pub qp: i32,
    /// Allow Intra_4x4 (speed preset).
    pub use_i4: bool,
}

/// Mutable reconstruction of the slice's macroblock rows.
pub struct Recon<'a> {
    /// Global macroblock row of the first row in these buffers.
    pub row0: usize,
    pub y: &'a mut [u8],
    pub u: &'a mut [u8],
    pub v: &'a mut [u8],
}

pub struct MbOut {
    pub coded: MbCoded,
    pub state: MbState,
    pub db: DbMb,
    pub mv: [i16; 2],
}

/// Coefficient "cost" of a block of levels (scan order) in the style of the
/// reference encoder: isolated ±1 levels are cheap to drop.
fn coeff_cost(levels: &[i32]) -> u32 {
    const RUN_COST: [u32; 16] = [3, 2, 2, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    let mut cost = 0;
    let mut run = 0;
    for &l in levels {
        if l == 0 {
            run += 1;
            continue;
        }
        if l.abs() > 1 {
            return 999;
        }
        cost += RUN_COST[run.min(15)];
        run = 0;
    }
    cost
}

fn to_scan(raster: &[i32; 16]) -> [i32; 16] {
    let mut s = [0i32; 16];
    for (k, &p) in ZIGZAG4.iter().enumerate() {
        s[k] = raster[p];
    }
    s
}

/// SATD of a 16x16 source block (stride `ss`) against a 16-stride prediction.
fn satd16(src: &[u8], so: usize, ss: usize, pred: &[u8; 256]) -> u32 {
    let mut total = 0;
    for b in 0..16 {
        let (bx, by) = (b % 4, b / 4);
        let mut d = [0i32; 16];
        for i in 0..4 {
            for j in 0..4 {
                d[i * 4 + j] = src[so + (by * 4 + i) * ss + bx * 4 + j] as i32 - pred[(by * 4 + i) * 16 + bx * 4 + j] as i32;
            }
        }
        total += satd4(&d);
    }
    total
}

fn satd8(src: &[u8], so: usize, ss: usize, pred: &[u8; 256]) -> u32 {
    let mut total = 0;
    for b in 0..4 {
        let (bx, by) = (b % 2, b / 2);
        let mut d = [0i32; 16];
        for i in 0..4 {
            for j in 0..4 {
                d[i * 4 + j] = src[so + (by * 4 + i) * ss + bx * 4 + j] as i32 - pred[(by * 4 + i) * 16 + bx * 4 + j] as i32;
            }
        }
        total += satd4(&d);
    }
    total
}

#[inline]
fn sad16(src: &[u8], so: usize, ss: usize, r: &[u8], ro: usize, rs: usize) -> u32 {
    let mut s = 0u32;
    for i in 0..16 {
        let a = &src[so + i * ss..so + i * ss + 16];
        let b = &r[ro + i * rs..ro + i * rs + 16];
        for j in 0..16 {
            s += (a[j] as i32 - b[j] as i32).unsigned_abs();
        }
    }
    s
}

/// Median of three.
fn median(a: i32, b: i32, c: i32) -> i32 {
    a.max(b).min(a.min(b).max(c))
}

pub struct MbCtx<'a, 'b> {
    pub f: &'a FrameIn<'b>,
    pub mbx: usize,
    pub mby: usize,
    pub nb: Neighbours,
    pub states: &'a [MbState],
}

impl MbCtx<'_, '_> {
    fn st(&self, i: Option<usize>) -> Option<&MbState> {
        i.map(|i| &self.states[i])
    }

    /// Neighbour motion data: (available, refIdx (0 or -1), mv).
    fn motion(&self, i: Option<usize>) -> (bool, i32, [i32; 2]) {
        match self.st(i) {
            None => (false, -1, [0, 0]),
            Some(s) if s.inter() => (true, 0, [s.mv[0] as i32, s.mv[1] as i32]),
            Some(_) => (true, -1, [0, 0]),
        }
    }

    /// Motion vector predictor of a 16x16 partition with refIdx 0 (8.4.1.3).
    pub fn mvp(&self) -> [i32; 2] {
        let a = self.motion(self.nb.a);
        let mut b = self.motion(self.nb.b);
        let mut c = if self.nb.c.is_some() { self.motion(self.nb.c) } else { self.motion(self.nb.d) };
        if !b.0 && !c.0 && a.0 {
            b = a;
            c = a;
        }
        let matches = [a.1 == 0, b.1 == 0, c.1 == 0];
        let n = matches.iter().filter(|&&m| m).count();
        if n == 1 {
            if matches[0] {
                return a.2;
            }
            if matches[1] {
                return b.2;
            }
            return c.2;
        }
        [median(a.2[0], b.2[0], c.2[0]), median(a.2[1], b.2[1], c.2[1])]
    }

    /// P_Skip motion vector (8.4.1.1).
    pub fn skip_mv(&self) -> [i32; 2] {
        let a = self.motion(self.nb.a);
        let b = self.motion(self.nb.b);
        if !a.0 || !b.0 {
            return [0, 0];
        }
        if a.1 == 0 && a.2 == [0, 0] {
            return [0, 0];
        }
        if b.1 == 0 && b.2 == [0, 0] {
            return [0, 0];
        }
        self.mvp()
    }

    /// Whether a motion vector keeps every interpolation tap inside the padded reference.
    fn mv_ok(&self, mv: [i32; 2]) -> bool {
        let w = (self.f.mb_w * 16) as i32;
        let h = (self.f.mb_h * 16) as i32;
        let x = (self.mbx * 16) as i32 + (mv[0] >> 2);
        let y = (self.mby * 16) as i32 + (mv[1] >> 2);
        mv[0] & 1 == 0
            && mv[1] & 1 == 0
            && x >= -MV_OUTSIDE
            && x <= w - 16 + MV_OUTSIDE
            && y >= -MV_OUTSIDE
            && y <= h - 16 + MV_OUTSIDE
            && (-2048..2048).contains(&mv[0])
            && (-512..512).contains(&mv[1])
    }

    /// Predicted Intra_4x4 mode of block `blk` given the current MB's modes so far.
    fn pred_i4_mode(&self, blk: usize, cur: &[u8; 16]) -> u8 {
        let (bx, by) = BLK4_XY[blk];
        let a = if bx > 0 {
            Some(cur[RASTER_TO_BLK4[by * 4 + bx - 1]])
        } else {
            self.st(self.nb.a).map(|s| if s.kind == Kind::I4 { s.i4_modes[RASTER_TO_BLK4[by * 4 + 3]] } else { 2 })
        };
        let b = if by > 0 {
            Some(cur[RASTER_TO_BLK4[(by - 1) * 4 + bx]])
        } else {
            self.st(self.nb.b).map(|s| if s.kind == Kind::I4 { s.i4_modes[RASTER_TO_BLK4[12 + bx]] } else { 2 })
        };
        match (a, b) {
            (Some(a), Some(b)) => a.min(b),
            _ => 2,
        }
    }
}

/// Analyse, code and reconstruct one macroblock.
pub fn encode_mb(cx: &MbCtx, rec: &mut Recon, p_slice: bool) -> MbOut {
    let f = cx.f;
    let qp = f.qp;
    let lam = lambda(qp);
    let w = f.mb_w * 16;
    let cw = f.mb_w * 8;
    let (mbx, mby) = (cx.mbx, cx.mby);
    let so = mby * 16 * w + mbx * 16;
    let cso = mby * 8 * cw + mbx * 8;
    let lrow = mby - rec.row0;
    let ro = lrow * 16 * w + mbx * 16;
    let cro = lrow * 8 * cw + mbx * 8;

    let has_a = cx.nb.a.is_some();
    let has_b = cx.nb.b.is_some();
    let has_d = cx.nb.d.is_some();

    // Chroma intra prediction choice (used if the MB ends up intra).
    let eu = pred::gather_n(rec.u, cro, cw, 8, has_b, has_a, has_d);
    let ev = pred::gather_n(rec.v, cro, cw, 8, has_b, has_a, has_d);

    let mut inter: Option<([i32; 2], u32, bool)> = None; // (mv, cost, is_skip_candidate)
    let mut pu = [0u8; 256];
    let mut pv = [0u8; 256];
    let mut py = [0u8; 256];

    if p_slice {
        let refp = f.refp.expect("P slice needs a reference");
        let mvp = cx.mvp();
        let skip = cx.skip_mv();
        // Early skip: the skip prediction leaves nothing to code.
        if cx.mv_ok(skip) {
            refp.luma16((mbx * 16) as i32, (mby * 16) as i32, skip, &mut py);
            refp.chroma8((mbx * 8) as i32, (mby * 8) as i32, skip, &mut pu, &mut pv);
            let sad = sad16(f.src_y, so, w, &py, 0, 16);
            // Cheap pre-check before the full quantisation test.
            let qstep = 0.625 * 2f32.powf(qp as f32 / 6.0);
            if (sad as f32) < qstep * 16.0 * 6.0 {
                let mut out = MbOut { coded: MbCoded::default(), state: MbState::default(), db: DbMb::default(), mv: [0; 2] };
                code_inter(cx, rec, &py, &pu, &pv, skip, mvp, &mut out);
                if out.coded.cbp == 0 {
                    return finish_skip(cx, rec, &py, &pu, &pv, skip, qp);
                }
            }
        }
        let (mv, cost) = motion_search(cx, refp, mvp, skip, lam);
        inter = Some((mv, cost, mv == skip));
    }

    // Intra 16x16 analysis.
    let ey = pred::gather_n(rec.y, ro, w, 16, has_b, has_a, has_d);
    let mut best16 = (u32::MAX, 2u8);
    let mut tmp = [0u8; 256];
    for mode in 0..4u8 {
        if !pred::mode16_allowed(mode, &ey) {
            continue;
        }
        pred::pred16(mode, &ey, &mut tmp);
        let c = satd16(f.src_y, so, w, &tmp) + (lam * 4.0) as u32;
        if c < best16.0 {
            best16 = (c, mode);
        }
    }
    let intra_bias = if p_slice { (lam * 6.0) as u32 } else { 0 };

    let try_intra = match inter {
        None => true,
        Some((_, c, _)) => best16.0 + intra_bias < c,
    };

    let mut out = MbOut { coded: MbCoded::default(), state: MbState::default(), db: DbMb::default(), mv: [0; 2] };
    if try_intra {
        // Chroma mode for intra.
        let mut bestc = (u32::MAX, 0u8);
        let mut tu = [0u8; 256];
        let mut tv = [0u8; 256];
        for mode in 0..4u8 {
            if !pred::chroma_mode_allowed(mode, &eu) {
                continue;
            }
            pred::pred_chroma(mode, &eu, &mut tu);
            pred::pred_chroma(mode, &ev, &mut tv);
            let c = satd8(f.src_u, cso, cw, &tu) + satd8(f.src_v, cso, cw, &tv);
            if c < bestc.0 {
                bestc = (c, mode);
            }
        }
        let cmode = bestc.1;
        // Intra 4x4 (written into the reconstruction), compared with 16x16.
        let mut chose_i4 = false;
        if f.use_i4 {
            let (cost4, coded4, state4) = code_i4(cx, rec, lam);
            let limit = match inter {
                Some((_, c, _)) => best16.0.min(c),
                None => best16.0,
            };
            if cost4 < limit {
                out.coded = coded4;
                out.state = state4;
                chose_i4 = true;
            }
        }
        if chose_i4 {
            out.coded.chroma_mode = cmode;
            out.state.chroma_mode = cmode;
            pred::pred_chroma(cmode, &eu, &mut pu);
            pred::pred_chroma(cmode, &ev, &mut pv);
            code_chroma(cx, rec, &pu, &pv, true, &mut out);
            finalize(&mut out, qp, Kind::I4);
            return out;
        }
        let i16_wins = match inter {
            Some((_, c, _)) => best16.0 + intra_bias < c,
            None => true,
        };
        if i16_wins {
            pred::pred16(best16.1, &ey, &mut py);
            code_i16(cx, rec, &py, best16.1, &mut out);
            out.coded.chroma_mode = cmode;
            out.state.chroma_mode = cmode;
            pred::pred_chroma(cmode, &eu, &mut pu);
            pred::pred_chroma(cmode, &ev, &mut pv);
            code_chroma(cx, rec, &pu, &pv, true, &mut out);
            finalize(&mut out, qp, Kind::I16);
            return out;
        }
    }
    // Inter 16x16.
    let (mv, _, _) = inter.expect("inter candidate");
    let refp = f.refp.expect("reference");
    refp.luma16((mbx * 16) as i32, (mby * 16) as i32, mv, &mut py);
    refp.chroma8((mbx * 8) as i32, (mby * 8) as i32, mv, &mut pu, &mut pv);
    let mvp = cx.mvp();
    code_inter(cx, rec, &py, &pu, &pv, mv, mvp, &mut out);
    let skip = cx.skip_mv();
    if out.coded.cbp == 0 && mv == skip {
        return finish_skip(cx, rec, &py, &pu, &pv, skip, qp);
    }
    out
}

fn finish_skip(cx: &MbCtx, rec: &mut Recon, py: &[u8; 256], pu: &[u8; 256], pv: &[u8; 256], mv: [i32; 2], qp: i32) -> MbOut {
    let w = cx.f.mb_w * 16;
    let cw = cx.f.mb_w * 8;
    let lrow = cx.mby - rec.row0;
    let ro = lrow * 16 * w + cx.mbx * 16;
    let cro = lrow * 8 * cw + cx.mbx * 8;
    for r in 0..16 {
        rec.y[ro + r * w..ro + r * w + 16].copy_from_slice(&py[r * 16..r * 16 + 16]);
    }
    for r in 0..8 {
        rec.u[cro + r * cw..cro + r * cw + 8].copy_from_slice(&pu[r * 16..r * 16 + 8]);
        rec.v[cro + r * cw..cro + r * cw + 8].copy_from_slice(&pv[r * 16..r * 16 + 8]);
    }
    let mv16 = [mv[0] as i16, mv[1] as i16];
    let qpc = CHROMA_QP[qp as usize];
    MbOut {
        coded: MbCoded { kind: Kind::PSkip, ..MbCoded::default() },
        state: MbState { kind: Kind::PSkip, i4_modes: [2; 16], mv: mv16, qp: qp as u8, ..MbState::default() },
        db: DbMb { intra: false, qp: qp as u8, qpc, nz: 0, mv: mv16 },
        mv: mv16,
    }
}

/// Fill derived state (cbp, total coefficient counts, deblocking info).
fn finalize(out: &mut MbOut, qp: i32, kind: Kind) {
    let c = &mut out.coded;
    c.kind = kind;
    let s = &mut out.state;
    s.kind = kind;
    s.qp = qp as u8;
    s.cbp = c.cbp;
    if kind != Kind::I4 {
        s.i4_modes = [2; 16];
    }
    let mut nz = 0u16;
    for blk in 0..16 {
        let (bx, by) = BLK4_XY[blk];
        let start = if kind == Kind::I16 { 1 } else { 0 };
        let n = c.luma[blk][start..].iter().filter(|&&v| v != 0).count() as u8;
        s.tc_luma[by * 4 + bx] = n;
        if n > 0 {
            nz |= 1 << (by * 4 + bx);
        }
    }
    for comp in 0..2 {
        for b in 0..4 {
            s.tc_chroma[comp][b] = c.chroma_ac[comp][b][1..].iter().filter(|&&v| v != 0).count() as u8;
        }
        s.dc_coded[1 + comp] = c.chroma_dc[comp].iter().any(|&v| v != 0);
    }
    s.dc_coded[0] = kind == Kind::I16 && c.luma_dc.iter().any(|&v| v != 0);
    s.mvd = [c.mvd[0].clamp(-32768, 32767) as i16, c.mvd[1].clamp(-32768, 32767) as i16];
    let intra = matches!(kind, Kind::I4 | Kind::I16);
    if intra {
        s.mv = [0, 0];
        out.mv = [0, 0];
    }
    out.db = DbMb { intra, qp: qp as u8, qpc: CHROMA_QP[qp as usize], nz: if intra { 0 } else { nz }, mv: s.mv };
}

/// Intra 4x4: choose a mode per block, code and reconstruct it.
fn code_i4(cx: &MbCtx, rec: &mut Recon, lam: f32) -> (u32, MbCoded, MbState) {
    let f = cx.f;
    let qp = f.qp;
    let w = f.mb_w * 16;
    let so = cx.mby * 16 * w + cx.mbx * 16;
    let lrow = cx.mby - rec.row0;
    let ro = lrow * 16 * w + cx.mbx * 16;
    let mut coded = MbCoded::default();
    let mut state = MbState::default();
    let mut modes = [2u8; 16];
    let mut total = (lam * 24.0) as u32;
    let mut cbp = 0u8;
    let mut pbuf = [0u8; 256];
    for blk in 0..16 {
        let (bx, by) = BLK4_XY[blk];
        let off = ro + by * 4 * w + bx * 4;
        let soff = so + by * 4 * w + bx * 4;
        let has_left = bx > 0 || cx.nb.a.is_some();
        let has_top = by > 0 || cx.nb.b.is_some();
        let has_tl = match (bx > 0, by > 0) {
            (true, true) => true,
            (false, true) => cx.nb.a.is_some(),
            (true, false) => cx.nb.b.is_some(),
            (false, false) => cx.nb.d.is_some(),
        };
        let has_tr = if by == 0 {
            if bx < 3 { cx.nb.b.is_some() } else { cx.nb.c.is_some() }
        } else if bx == 3 {
            false
        } else {
            RASTER_TO_BLK4[(by - 1) * 4 + bx + 1] < blk
        };
        let e = pred::gather4(rec.y, off, w, has_top, has_left, has_tl, has_tr);
        let pm = cx.pred_i4_mode(blk, &modes);
        let mut best = (u32::MAX, 2u8);
        let mut d = [0i32; 16];
        for mode in 0..9u8 {
            if !pred::mode4_allowed(mode, &e) {
                continue;
            }
            pred::pred4(mode, &e, &mut pbuf, 0);
            for i in 0..4 {
                for j in 0..4 {
                    d[i * 4 + j] = f.src_y[soff + i * w + j] as i32 - pbuf[i * 16 + j] as i32;
                }
            }
            let bits = if mode == pm { 1.0 } else { 4.0 };
            let c = satd4(&d) + (lam * bits) as u32;
            if c < best.0 {
                best = (c, mode);
            }
        }
        let mode = best.1;
        modes[blk] = mode;
        coded.i4_rem[blk] = if mode == pm { 8 } else if mode < pm { mode } else { mode - 1 };
        total += best.0;
        pred::pred4(mode, &e, &mut pbuf, 0);
        for i in 0..4 {
            for j in 0..4 {
                d[i * 4 + j] = f.src_y[soff + i * w + j] as i32 - pbuf[i * 16 + j] as i32;
            }
        }
        let lv = quant4(&fdct4(&d), qp, true, false);
        let scan = to_scan(&lv);
        if scan.iter().any(|&v| v != 0) {
            cbp |= 1 << (blk / 4);
            let dq = dequant_block(&lv, qp, None);
            idct4_add(&dq, &pbuf, 0, rec.y, off, w);
        } else {
            copy4(&pbuf, 0, rec.y, off, w);
        }
        coded.luma[blk] = scan;
    }
    coded.cbp = cbp;
    state.i4_modes = modes;
    (total, coded, state)
}

/// Intra 16x16 residual coding and reconstruction with prediction `py`.
fn code_i16(cx: &MbCtx, rec: &mut Recon, py: &[u8; 256], mode: u8, out: &mut MbOut) {
    let f = cx.f;
    let qp = f.qp;
    let w = f.mb_w * 16;
    let so = cx.mby * 16 * w + cx.mbx * 16;
    let lrow = cx.mby - rec.row0;
    let ro = lrow * 16 * w + cx.mbx * 16;
    let mut dc = [0i32; 16];
    let mut ac = [[0i32; 16]; 16]; // raster levels by raster block position
    for b in 0..16 {
        let (bx, by) = (b % 4, b / 4);
        let mut d = [0i32; 16];
        for i in 0..4 {
            for j in 0..4 {
                d[i * 4 + j] = f.src_y[so + (by * 4 + i) * w + bx * 4 + j] as i32 - py[(by * 4 + i) * 16 + bx * 4 + j] as i32;
            }
        }
        let t = fdct4(&d);
        dc[b] = t[0];
        ac[b] = quant4(&t, qp, true, true);
    }
    let dcl = quant_luma_dc(&dc, qp);
    let dcq = dequant_luma_dc(&dcl, qp);
    let any_ac = ac.iter().any(|b| b.iter().any(|&v| v != 0));
    let c = &mut out.coded;
    c.i16_mode = mode;
    c.luma_dc = to_scan(&dcl);
    for b in 0..16 {
        let (bx, by) = (b % 4, b / 4);
        let blk = RASTER_TO_BLK4[b];
        if !any_ac {
            ac[b] = [0; 16];
        }
        c.luma[blk] = to_scan(&ac[b]);
        c.luma[blk][0] = 0;
        let d = dequant_block(&ac[b], qp, Some(dcq[b]));
        idct4_add(&d, py, by * 4 * 16 + bx * 4, rec.y, ro + by * 4 * w + bx * 4, w);
    }
    c.cbp = if any_ac { 15 } else { 0 };
}

/// Chroma residual for both planes with predictions `pu`, `pv`; sets cbp bits 4..5.
fn code_chroma(cx: &MbCtx, rec: &mut Recon, pu: &[u8; 256], pv: &[u8; 256], intra: bool, out: &mut MbOut) {
    let f = cx.f;
    let qpc = CHROMA_QP[f.qp as usize] as i32;
    let cw = f.mb_w * 8;
    let cso = cx.mby * 8 * cw + cx.mbx * 8;
    let lrow = cx.mby - rec.row0;
    let cro = lrow * 8 * cw + cx.mbx * 8;
    let mut dcs = [[0i32; 4]; 2];
    let mut acs = [[[0i32; 16]; 4]; 2];
    for comp in 0..2 {
        let (src, pred) = if comp == 0 { (f.src_u, pu) } else { (f.src_v, pv) };
        let mut dc = [0i32; 4];
        for b in 0..4 {
            let (bx, by) = (b % 2, b / 2);
            let mut d = [0i32; 16];
            for i in 0..4 {
                for j in 0..4 {
                    d[i * 4 + j] = src[cso + (by * 4 + i) * cw + bx * 4 + j] as i32 - pred[(by * 4 + i) * 16 + bx * 4 + j] as i32;
                }
            }
            let t = fdct4(&d);
            dc[b] = t[0];
            acs[comp][b] = quant4(&t, qpc, intra, true);
        }
        dcs[comp] = quant_chroma_dc(&dc, qpc, intra);
    }
    // Drop isolated small AC levels of inter blocks.
    if !intra {
        let cost: u32 = acs.iter().flatten().map(|b| coeff_cost(&to_scan(b)[1..])).sum();
        if cost < 7 {
            acs = [[[0; 16]; 4]; 2];
        }
    }
    let any_ac = acs.iter().flatten().any(|b| b.iter().any(|&v| v != 0));
    let any_dc = dcs.iter().any(|d| d.iter().any(|&v| v != 0));
    let cbpc = if any_ac { 2 } else if any_dc { 1 } else { 0 };
    for comp in 0..2 {
        let pred = if comp == 0 { pu } else { pv };
        let dcq = dequant_chroma_dc(&dcs[comp], qpc);
        let plane: &mut [u8] = if comp == 0 { rec.u } else { rec.v };
        for b in 0..4 {
            let (bx, by) = (b % 2, b / 2);
            let d = dequant_block(&acs[comp][b], qpc, Some(dcq[b]));
            idct4_add(&d, pred, by * 4 * 16 + bx * 4, plane, cro + by * 4 * cw + bx * 4, cw);
            out.coded.chroma_ac[comp][b] = to_scan(&acs[comp][b]);
            out.coded.chroma_ac[comp][b][0] = 0;
        }
        out.coded.chroma_dc[comp] = dcs[comp];
    }
    out.coded.cbp = (out.coded.cbp & 15) | (cbpc << 4);
}

/// P_L0_16x16 residual coding and reconstruction.
#[allow(clippy::too_many_arguments)]
fn code_inter(cx: &MbCtx, rec: &mut Recon, py: &[u8; 256], pu: &[u8; 256], pv: &[u8; 256], mv: [i32; 2], mvp: [i32; 2], out: &mut MbOut) {
    let f = cx.f;
    let qp = f.qp;
    let w = f.mb_w * 16;
    let so = cx.mby * 16 * w + cx.mbx * 16;
    let lrow = cx.mby - rec.row0;
    let ro = lrow * 16 * w + cx.mbx * 16;
    let mut lv = [[0i32; 16]; 16]; // raster levels by raster block position
    for b in 0..16 {
        let (bx, by) = (b % 4, b / 4);
        let mut d = [0i32; 16];
        for i in 0..4 {
            for j in 0..4 {
                d[i * 4 + j] = f.src_y[so + (by * 4 + i) * w + bx * 4 + j] as i32 - py[(by * 4 + i) * 16 + bx * 4 + j] as i32;
            }
        }
        lv[b] = quant4(&fdct4(&d), qp, false, false);
    }
    // Per 8x8 and per macroblock coefficient cost thresholds.
    let mut cbp = 0u8;
    let mut total_cost = 0;
    let mut costs = [0u32; 4];
    for (b8, cost) in costs.iter_mut().enumerate() {
        for k in 0..4 {
            let blk = b8 * 4 + k;
            let (bx, by) = BLK4_XY[blk];
            *cost += coeff_cost(&to_scan(&lv[by * 4 + bx]));
        }
        if *cost < 4 {
            for k in 0..4 {
                let (bx, by) = BLK4_XY[b8 * 4 + k];
                lv[by * 4 + bx] = [0; 16];
            }
            *cost = 0;
        }
        total_cost += *cost;
    }
    if total_cost < 6 {
        lv = [[0; 16]; 16];
    }
    let c = &mut out.coded;
    for b in 0..16 {
        let (bx, by) = (b % 4, b / 4);
        let blk = RASTER_TO_BLK4[b];
        c.luma[blk] = to_scan(&lv[b]);
        let off = ro + by * 4 * w + bx * 4;
        let po = by * 4 * 16 + bx * 4;
        if lv[b].iter().any(|&v| v != 0) {
            cbp |= 1 << (blk / 4);
            let d = dequant_block(&lv[b], qp, None);
            idct4_add(&d, py, po, rec.y, off, w);
        } else {
            copy4(py, po, rec.y, off, w);
        }
    }
    c.cbp = cbp;
    c.mvd = [mv[0] - mvp[0], mv[1] - mvp[1]];
    out.state.mv = [mv[0] as i16, mv[1] as i16];
    out.mv = out.state.mv;
    code_chroma(cx, rec, pu, pv, false, out);
    finalize(out, qp, Kind::P16);
}

/// Predictive diamond search at integer positions followed by half-sample
/// refinement. Returns the motion vector (quarter units) and its SATD cost.
fn motion_search(cx: &MbCtx, refp: &RefPic, mvp: [i32; 2], skip: [i32; 2], lam: f32) -> ([i32; 2], u32) {
    let f = cx.f;
    let w = f.mb_w * 16;
    let so = cx.mby * 16 * w + cx.mbx * 16;
    let bx = (cx.mbx * 16) as i32;
    let by = (cx.mby * 16) as i32;
    let mv_bits = |mv: [i32; 2]| (lam * (se_len(mv[0] - mvp[0]) + se_len(mv[1] - mvp[1])) as f32) as u32;
    let int_cost = |m: [i32; 2]| -> u32 {
        let mv = [m[0] * 4, m[1] * 4];
        if !cx.mv_ok(mv) {
            return u32::MAX;
        }
        sad16(f.src_y, so, w, &refp.y, refp.idx(bx + m[0], by + m[1]), refp.stride) + mv_bits(mv)
    };
    // Candidates (integer sample units).
    let mut cands: Vec<[i32; 2]> = vec![[0, 0], [mvp[0] >> 2, mvp[1] >> 2], [skip[0] >> 2, skip[1] >> 2]];
    for n in [cx.nb.a, cx.nb.b, cx.nb.c] {
        if let Some(s) = n.map(|i| &cx.states[i]) {
            if s.inter() {
                cands.push([s.mv[0] as i32 >> 2, s.mv[1] as i32 >> 2]);
            }
        }
    }
    let addr = cx.mby * f.mb_w + cx.mbx;
    if let Some(p) = f.prev_mv.get(addr) {
        cands.push([p[0] as i32 >> 2, p[1] as i32 >> 2]);
    }
    let mut best = ([0i32; 2], u32::MAX);
    for c in cands {
        let k = int_cost(c);
        if k < best.1 {
            best = (c, k);
        }
    }
    if best.1 == u32::MAX {
        best = ([0, 0], int_cost([0, 0]));
    }
    // Coarse-to-fine diamond.
    for step in [16, 8, 4, 2, 1] {
        for _ in 0..8 {
            let c0 = best.0;
            let mut moved = false;
            for d in [[step, 0], [-step, 0], [0, step], [0, -step]] {
                let c = [c0[0] + d[0], c0[1] + d[1]];
                let k = int_cost(c);
                if k < best.1 {
                    best = (c, k);
                    moved = true;
                }
            }
            if !moved {
                break;
            }
        }
    }
    // Final small square around the best integer position.
    let c0 = best.0;
    for d in [[1, 1], [1, -1], [-1, 1], [-1, -1]] {
        let c = [c0[0] + d[0], c0[1] + d[1]];
        let k = int_cost(c);
        if k < best.1 {
            best = (c, k);
        }
    }
    // Half-sample refinement with SATD.
    let mut pbuf = [0u8; 256];
    let full = [best.0[0] * 4, best.0[1] * 4];
    let satd_cost = |mv: [i32; 2], pbuf: &mut [u8; 256]| -> u32 {
        if !cx.mv_ok(mv) {
            return u32::MAX;
        }
        refp.luma16(bx, by, mv, pbuf);
        satd16(f.src_y, so, w, pbuf) + mv_bits(mv)
    };
    let mut bmv = full;
    let mut bcost = satd_cost(full, &mut pbuf);
    for d in [[-2, -2], [0, -2], [2, -2], [-2, 0], [2, 0], [-2, 2], [0, 2], [2, 2]] {
        let mv = [full[0] + d[0], full[1] + d[1]];
        let k = satd_cost(mv, &mut pbuf);
        if k < bcost {
            bcost = k;
            bmv = mv;
        }
    }
    // The skip vector itself is worth a look (it costs no bits when residual-free).
    if skip != bmv && cx.mv_ok(skip) {
        let k = satd_cost(skip, &mut pbuf);
        if k < bcost {
            bcost = k;
            bmv = skip;
        }
    }
    (bmv, bcost)
}
