// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! residual() syntax (7.3.5.3) for CAVLC and CABAC, with inverse scanning and scaling.

use crate::cavlc::{NcKind, residual_block};
use crate::error::Result;
use crate::mb::*;
use crate::slicedec::SliceDecoder;
use crate::tables::{BLK4_XY, FIELD4, FIELD8, ZIGZAG4, ZIGZAG8, chroma_qp};
use crate::transform::{chroma_dc_dequant, dequant4, dequant8, luma_dc_dequant};

/// Non-zero levels of one block: `lvl[k]` at scan position `pos[k]`, for k < n.
struct Coeffs {
    n: usize,
    pos: [u8; 64],
    lvl: [i32; 64],
}

impl SliceDecoder<'_> {
    /// With data partitioning and constrained intra prediction, inter neighbours of an intra
    /// macroblock count as unavailable for nC (9.2.1).
    fn nc_unavailable(&self, a: usize) -> bool {
        self.dp && self.env.pps.constrained_intra_pred && self.cur.intra() && !self.info(a).intra()
    }

    /// CAVLC TotalCoeff of the luma 4x4 block at (x, y) relative to the current MB.
    fn nnz_luma(&self, x: isize, y: isize) -> Option<u8> {
        match self.locate4(x, y) {
            None => None,
            Some((None, bx, by)) => Some(self.cur.nnz[by * 4 + bx]),
            Some((Some(a), _, _)) if self.nc_unavailable(a) => None,
            Some((Some(a), bx, by)) => Some(self.info(a).nnz[by * 4 + bx]),
        }
    }

    fn nc_from(a: Option<u8>, b: Option<u8>) -> i32 {
        match (a, b) {
            (Some(a), Some(b)) => (a as i32 + b as i32 + 1) >> 1,
            (Some(a), None) => a as i32,
            (None, Some(b)) => b as i32,
            (None, None) => 0,
        }
    }

    fn nc_luma(&self, x: usize, y: usize) -> i32 {
        Self::nc_from(self.nnz_luma(x as isize - 1, y as isize), self.nnz_luma(x as isize, y as isize - 1))
    }

    /// Chroma 4x4 block neighbour (6.4.11.5): (addr or None for the current MB, block index).
    fn chroma_nb(&self, bx: usize, by: usize, dx: i32, dy: i32) -> Option<(Option<usize>, usize)> {
        let (a, xw, yw) = self.locate(bx as i32 * 4 + dx, by as i32 * 4 + dy, 8, 8)?;
        let blk = ((yw / 4) * 2 + xw / 4) as usize;
        Some((if a == self.addr { None } else { Some(a) }, blk))
    }

    fn nc_chroma(&self, c: usize, bx: usize, by: usize) -> i32 {
        let base = 16 + c * 4;
        let get = |n: Option<(Option<usize>, usize)>| -> Option<u8> {
            match n? {
                (None, blk) => Some(self.cur.nnz[base + blk]),
                (Some(a), _) if self.nc_unavailable(a) => None,
                (Some(a), blk) => Some(self.info(a).nnz[base + blk]),
            }
        };
        Self::nc_from(get(self.chroma_nb(bx, by, -1, 0)), get(self.chroma_nb(bx, by, 0, -1)))
    }

    /// coded_block_flag bit `bit` of the MB containing the neighbour, for CABAC contexts.
    fn cbf_neighbour(&self, n: Option<usize>, bit: u32) -> usize {
        match n {
            None => self.cur.intra() as usize,
            Some(a) => ((self.info(a).cbf >> bit) & 1) as usize,
        }
    }

    fn cbf_luma_ctx(&self, x: usize, y: usize) -> usize {
        let f = |s: &Self, nx: isize, ny: isize| -> usize {
            match s.locate4(nx, ny) {
                None => s.cur.intra() as usize,
                Some((None, bx, by)) => ((s.cur.cbf >> (by * 4 + bx)) & 1) as usize,
                Some((Some(a), bx, by)) => ((s.info(a).cbf >> (by * 4 + bx)) & 1) as usize,
            }
        };
        f(self, x as isize - 1, y as isize) + 2 * f(self, x as isize, y as isize - 1)
    }

    fn cbf_chroma_ac_ctx(&self, c: usize, bx: usize, by: usize) -> usize {
        let base = 16 + c as u32 * 4;
        let get = |n: Option<(Option<usize>, usize)>| -> usize {
            match n {
                None => self.cur.intra() as usize,
                Some((None, blk)) => ((self.cur.cbf >> (base + blk as u32)) & 1) as usize,
                Some((Some(a), blk)) => ((self.info(a).cbf >> (base + blk as u32)) & 1) as usize,
            }
        };
        get(self.chroma_nb(bx, by, -1, 0)) + 2 * get(self.chroma_nb(bx, by, 0, -1))
    }

    /// Decode one block's non-zero levels as (scan position, level) pairs into `out`;
    /// returns the number of non-zero levels (CAVLC TotalCoeff). `cat` is the CABAC
    /// ctxBlockCat; `cbf_inc` its coded_block_flag ctxIdxInc (None for 8x8 blocks, which
    /// have no flag).
    fn read_block(&mut self, nc: NcKind, cat: usize, cbf_inc: Option<usize>, max: usize, out: &mut Coeffs) -> Result<u32> {
        out.n = 0;
        if let Some(c) = self.cabac.as_mut() {
            if let Some(inc) = cbf_inc {
                if !c.coded_block_flag(cat, inc) {
                    return Ok(0);
                }
            }
            let n = c.residual_block_sparse(cat, max, self.mb_field, &mut out.pos, &mut out.lvl);
            for v in &mut out.lvl[..n] {
                *v = (*v).clamp(-(1 << 15), (1 << 15) - 1);
            }
            out.n = n;
            return Ok(n as u32);
        }
        let mut dense = [0i32; 64];
        let total = if self.dp {
            // Category 3 (intra) / 4 (inter) residual data live in partitions B / C. A
            // missing partition leaves the residual at zero.
            let r = if self.cur.intra() { self.br_b.as_mut() } else { self.br_c.as_mut() };
            match r {
                Some(r) => residual_block(r, nc, max, &mut dense)?,
                None => 0,
            }
        } else {
            residual_block(&mut self.br, nc, max, &mut dense)?
        };
        for (k, &v) in dense[..max].iter().enumerate() {
            if v != 0 {
                out.pos[out.n] = k as u8;
                out.lvl[out.n] = v;
                out.n += 1;
            }
        }
        Ok(total)
    }

    pub(crate) fn read_residual(&mut self, cbp_luma: u8, cbp_chroma: u8) -> Result<()> {
        self.rec.coef = [0; 384];
        let intra = self.kind.is_intra();
        let i16 = matches!(self.kind, MbKind::I16x16 { .. });
        let qp = self.qp;
        let dq = self.env.dequant;
        let ly = if intra { 0 } else { 3 };
        // Field macroblocks use the field scan (8.5.6, 8.5.7).
        let (scan4, scan8) = if self.mb_field { (&FIELD4, &FIELD8) } else { (&ZIGZAG4, &ZIGZAG8) };
        let cabac = self.cabac.is_some();
        let mut levels = Coeffs { n: 0, pos: [0; 64], lvl: [0; 64] };

        // Intra16x16 DC.
        if i16 {
            let inc = if cabac {
                Some(self.cbf_neighbour(self.mb_a, CBF_LUMA_DC) + 2 * self.cbf_neighbour(self.mb_b, CBF_LUMA_DC))
            } else {
                None
            };
            let nc = NcKind::Nc(self.nc_luma(0, 0));
            let n = self.read_block(nc, 0, inc, 16, &mut levels)?;
            if n > 0 {
                self.cur.cbf |= 1 << CBF_LUMA_DC;
                let mut c = [0i32; 16];
                for k in 0..levels.n {
                    c[scan4[levels.pos[k] as usize & 15] as usize] = levels.lvl[k];
                }
                let dc = luma_dc_dequant(&c, dq.ls4[0][(qp % 6) as usize][0], qp);
                for r in 0..16 {
                    let blk = crate::tables::RASTER_TO_BLK4[r];
                    self.rec.coef[blk * 16] = dc[r];
                    if dc[r] != 0 {
                        self.luma_nz |= 1 << blk;
                    }
                }
            }
        }

        // Luma blocks.
        let ls4 = &dq.ls4[ly][(qp % 6) as usize];
        for b8 in 0..4 {
            let coded = cbp_luma >> b8 & 1 != 0;
            if self.transform_8x8 {
                if !coded {
                    continue;
                }
                let ls8 = &dq.ls8[if intra { 0 } else { 1 }][(qp % 6) as usize];
                if cabac {
                    self.read_block(NcKind::Nc(0), 5, None, 64, &mut levels)?;
                    let any = levels.n > 0;
                    let coef = &mut self.rec.coef[b8 * 64..b8 * 64 + 64];
                    for k in 0..levels.n {
                        let r = scan8[levels.pos[k] as usize & 63] as usize & 63;
                        coef[r] = dequant8(levels.lvl[k], ls8[r], qp);
                    }
                    // coded_block_flag of an 8x8 block is inferred to be 1 (CABAC context use).
                    for k in 0..4 {
                        let (x, y) = BLK4_XY[b8 * 4 + k];
                        self.cur.cbf |= 1 << (y * 4 + x);
                    }
                    if any {
                        self.luma_nz |= 1 << b8;
                    }
                } else {
                    let mut any = false;
                    for i4 in 0..4 {
                        let blk = b8 * 4 + i4;
                        let (x, y) = BLK4_XY[blk];
                        let nc = NcKind::Nc(self.nc_luma(x, y));
                        let n = self.read_block(nc, 2, None, 16, &mut levels)?;
                        self.cur.nnz[y * 4 + x] = n as u8;
                        for k in 0..levels.n {
                            let r = scan8[4 * (levels.pos[k] as usize & 15) + i4] as usize;
                            self.rec.coef[b8 * 64 + r] = dequant8(levels.lvl[k], ls8[r], qp);
                            any = true;
                        }
                    }
                    if any {
                        self.luma_nz |= 1 << b8;
                    }
                }
                continue;
            }
            for i4 in 0..4 {
                let blk = b8 * 4 + i4;
                let (x, y) = BLK4_XY[blk];
                if !coded {
                    continue;
                }
                let inc = if cabac { Some(self.cbf_luma_ctx(x, y)) } else { None };
                let nc = NcKind::Nc(if cabac { 0 } else { self.nc_luma(x, y) });
                let (cat, max, start) = if i16 { (1, 15, 1) } else { (2, 16, 0) };
                let n = self.read_block(nc, cat, inc, max, &mut levels)?;
                self.cur.nnz[y * 4 + x] = n as u8;
                if n > 0 {
                    self.cur.cbf |= 1 << (y * 4 + x);
                    self.luma_nz |= 1 << blk;
                    let coef = &mut self.rec.coef[blk * 16..blk * 16 + 16];
                    for k in 0..levels.n {
                        let r = scan4[(start + levels.pos[k] as usize) & 15] as usize & 15;
                        coef[r] = dequant4(levels.lvl[k], ls4[r], qp);
                    }
                }
            }
        }

        if !self.chroma {
            return Ok(());
        }
        let pps = self.env.pps;
        let qpc = [chroma_qp(qp, pps.chroma_qp_index_offset), chroma_qp(qp, pps.second_chroma_qp_index_offset)];
        // Chroma DC.
        if cbp_chroma & 3 != 0 {
            for c in 0..2 {
                let inc = if cabac {
                    let bit = CBF_CB_DC + c as u32;
                    Some(self.cbf_neighbour(self.mb_a, bit) + 2 * self.cbf_neighbour(self.mb_b, bit))
                } else {
                    None
                };
                let n = self.read_block(NcKind::ChromaDc420, 3, inc, 4, &mut levels)?;
                if n > 0 {
                    self.cur.cbf |= 1 << (CBF_CB_DC + c as u32);
                    let list = if intra { 1 + c } else { 4 + c };
                    let q = qpc[c];
                    let ls00 = dq.ls4[list][(q % 6) as usize][0];
                    let mut cdc = [0i32; 4];
                    for k in 0..levels.n {
                        cdc[levels.pos[k] as usize & 3] = levels.lvl[k];
                    }
                    let dc = chroma_dc_dequant(&cdc, ls00, q);
                    for b in 0..4 {
                        self.rec.coef[256 + c * 64 + b * 16] = dc[b];
                        if dc[b] != 0 {
                            self.chroma_nz |= 1 << (c * 4 + b);
                        }
                    }
                }
            }
        }
        // Chroma AC.
        if cbp_chroma & 2 != 0 {
            for c in 0..2 {
                let list = if intra { 1 + c } else { 4 + c };
                let q = qpc[c];
                let lsc = &dq.ls4[list][(q % 6) as usize];
                for b in 0..4 {
                    let (bx, by) = (b & 1, b >> 1);
                    let inc = if cabac { Some(self.cbf_chroma_ac_ctx(c, bx, by)) } else { None };
                    let nc = NcKind::Nc(if cabac { 0 } else { self.nc_chroma(c, bx, by) });
                    let n = self.read_block(nc, 4, inc, 15, &mut levels)?;
                    self.cur.nnz[16 + c * 4 + b] = n as u8;
                    if n > 0 {
                        self.cur.cbf |= 1 << (16 + c * 4 + b);
                        self.chroma_nz |= 1 << (c * 4 + b);
                        let coef = &mut self.rec.coef[256 + c * 64 + b * 16..256 + c * 64 + b * 16 + 16];
                        for k in 0..levels.n {
                            let r = scan4[(1 + levels.pos[k] as usize) & 15] as usize & 15;
                            coef[r] = dequant4(levels.lvl[k], lsc[r], q);
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
