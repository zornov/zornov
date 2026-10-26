// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Inter prediction syntax (mb_pred / sub_mb_pred for P and B macroblocks) and motion
//! vector derivation (8.4.1): median / directional prediction, P_Skip, and B direct
//! prediction in spatial and temporal mode, including field / MBAFF adaptations.

use crate::error::{Error, Result, check};
use crate::mb::*;
use crate::picture::{MbMotion, PAR_FRAME, Structure};
use crate::slicedec::{REF_NA, SliceDecoder};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    Median,
    /// 16x8 upper partition: prefer B.
    Top16x8,
    /// 16x8 lower partition: prefer A.
    Bottom16x8,
    /// 8x16 left partition: prefer A.
    Left8x16,
    /// 8x16 right partition: prefer C.
    Right8x16,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum VertScale {
    One,
    FrmToFld,
    FldToFrm,
}

#[inline]
fn median(a: i32, b: i32, c: i32) -> i32 {
    a.max(b).min(a.min(b).max(c))
}

#[inline]
fn min_positive(a: i8, b: i8) -> i8 {
    if a >= 0 && b >= 0 { a.min(b) } else { a.max(b) }
}

/// Picture coding structure for Table 8-8.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pcs {
    Fld,
    Frm,
    Afrm,
}

impl SliceDecoder<'_> {
    /// Motion of the neighbouring partition covering luma location (x, y) relative to the
    /// current MB (8.4.1.3.2): (refIdx, mv) with REF_NA when unavailable and -1 when intra
    /// or not using the list; MBAFF field/frame scaling applied.
    fn nb_motion(&self, list: usize, x: i32, y: i32) -> (i8, [i32; 2]) {
        if (0..16).contains(&x) && (0..16).contains(&y) {
            let blk = ((y >> 2) * 4 + (x >> 2)) as usize;
            if self.decoded[list] >> blk & 1 == 0 {
                return (REF_NA, [0, 0]);
            }
            let r = self.mb_ref[list][((y >> 3) * 2 + (x >> 3)) as usize];
            let mv = self.mb_mv[list][blk];
            return if r >= 0 { (r, [mv[0] as i32, mv[1] as i32]) } else { (-1, [0, 0]) };
        }
        let Some((a, xw, yw)) = self.locate(x, y, 16, 16) else {
            return (REF_NA, [0, 0]);
        };
        let m: &MbMotion = &self.st.motion[a];
        if m.intra {
            return (-1, [0, 0]);
        }
        let mut r = m.ref_idx[list][((yw >> 3) * 2 + (xw >> 3)) as usize];
        if r < 0 {
            return (-1, [0, 0]);
        }
        let v = m.mv[list][((yw >> 2) * 4 + (xw >> 2)) as usize];
        let mut mv = [v[0] as i32, v[1] as i32];
        if self.mbaff {
            if self.mb_field && !m.field {
                mv[1] /= 2;
                r = r.saturating_mul(2);
            } else if !self.mb_field && m.field {
                mv[1] *= 2;
                r >>= 1;
            }
        }
        (r, mv)
    }

    /// Store motion for a block of the current MB (4x4 units) and mark it decoded.
    pub(crate) fn put_motion(&mut self, list: usize, x: usize, y: usize, w: usize, h: usize, r: i8, mv: [i16; 2]) {
        self.set_motion(list, x, y, w, h, r, mv);
        self.mark_decoded(list, x, y, w, h);
    }

    fn set_motion(&mut self, list: usize, x: usize, y: usize, w: usize, h: usize, r: i8, mv: [i16; 2]) {
        for yy in y..y + h {
            for xx in x..x + w {
                self.mb_mv[list][yy * 4 + xx] = mv;
            }
        }
        for yy in (y..y + h).step_by(2) {
            for xx in (x..x + w).step_by(2) {
                self.mb_ref[list][(yy >> 1) * 2 + (xx >> 1)] = r;
            }
        }
    }

    fn mark_decoded(&mut self, list: usize, x: usize, y: usize, w: usize, h: usize) {
        for yy in y..y + h {
            for xx in x..x + w {
                self.decoded[list] |= 1 << (yy * 4 + xx);
            }
        }
    }

    /// Motion vector predictor (8.4.1.3) for a partition at (x, y) of width w (4x4 units).
    fn mvp(&self, list: usize, x: usize, y: usize, w: usize, r: i8, shape: Shape) -> [i32; 2] {
        let (x, y, w) = (x as i32 * 4, y as i32 * 4, w as i32 * 4);
        let (ra, ma) = self.nb_motion(list, x - 1, y);
        let (rb, mb) = self.nb_motion(list, x, y - 1);
        let (mut rc, mut mc) = self.nb_motion(list, x + w, y - 1);
        if rc == REF_NA {
            (rc, mc) = self.nb_motion(list, x - 1, y - 1);
        }
        match shape {
            Shape::Top16x8 if rb == r => return mb,
            Shape::Bottom16x8 if ra == r => return ma,
            Shape::Left8x16 if ra == r => return ma,
            Shape::Right8x16 if rc == r => return mc,
            _ => {}
        }
        if rb == REF_NA && rc == REF_NA && ra != REF_NA {
            return ma;
        }
        let matches = (ra == r) as u32 + (rb == r) as u32 + (rc == r) as u32;
        if matches == 1 {
            if ra == r {
                return ma;
            }
            if rb == r {
                return mb;
            }
            return mc;
        }
        [median(ma[0], mb[0], mc[0]), median(ma[1], mb[1], mc[1])]
    }

    pub(crate) fn derive_p_skip(&mut self) {
        let (ra, ma) = self.nb_motion(0, -1, 0);
        let (rb, mb) = self.nb_motion(0, 0, -1);
        let zero = ra == REF_NA || rb == REF_NA || (ra == 0 && ma == [0, 0]) || (rb == 0 && mb == [0, 0]);
        let mv = if zero { [0, 0] } else { self.mvp(0, 0, 0, 4, 0, Shape::Median) };
        self.put_motion(0, 0, 0, 4, 4, 0, [mv[0] as i16, mv[1] as i16]);
    }

    // ---- syntax ----------------------------------------------------------------------

    pub(crate) fn read_mb_pred(&mut self, part: Part, pred: [u8; 2]) -> Result<()> {
        let b = self.slice_type.is_b();
        let parts: &[(usize, usize, usize, usize, Shape)] = match part {
            Part::P16x16 => &[(0, 0, 4, 4, Shape::Median)],
            Part::P16x8 => &[(0, 0, 4, 2, Shape::Top16x8), (0, 2, 4, 2, Shape::Bottom16x8)],
            Part::P8x16 => &[(0, 0, 2, 4, Shape::Left8x16), (2, 0, 2, 4, Shape::Right8x16)],
            Part::P8x8 => unreachable!(),
        };
        let lists = if b { 2 } else { 1 };
        let mut refs = [[-1i8; 2]; 2];
        for list in 0..lists {
            for (i, &(x, y, w, h, _)) in parts.iter().enumerate() {
                if pred[i] & (1 << list) != 0 {
                    let r = self.read_ref_idx(list, x, y)?;
                    refs[list][i] = r;
                    for yy in (y..y + h).step_by(2) {
                        for xx in (x..x + w).step_by(2) {
                            self.parsed_ref[list][(yy >> 1) * 2 + (xx >> 1)] = r;
                        }
                    }
                }
            }
        }
        for list in 0..lists {
            for (i, &(x, y, w, h, shape)) in parts.iter().enumerate() {
                if pred[i] & (1 << list) != 0 {
                    let mvd = self.read_mvd(list, x, y)?;
                    self.store_mvd(list, x, y, w, h, mvd);
                    let r = refs[list][i];
                    let p = self.mvp(list, x, y, w, r, shape);
                    let mv = [p[0].wrapping_add(mvd[0]) as i16, p[1].wrapping_add(mvd[1]) as i16];
                    self.put_motion(list, x, y, w, h, r, mv);
                } else {
                    self.put_motion(list, x, y, w, h, -1, [0, 0]);
                }
            }
        }
        Ok(())
    }

    /// sub_mb_pred(); returns noSubMbPartSizeLessThan8x8Flag.
    pub(crate) fn read_sub_mb_pred(&mut self, ref0: bool) -> Result<bool> {
        let b = self.slice_type.is_b();
        let mut no_lt8 = true;
        for i in 0..4 {
            let t = if let Some(c) = self.cabac.as_mut() {
                if b { c.sub_mb_type_b() } else { c.sub_mb_type_p() }
            } else {
                self.br.ue()
            };
            let st = if b { b_sub_type(t) } else { p_sub_type(t) };
            let st = st.ok_or(Error::Bitstream("invalid sub_mb_type"))?;
            self.sub_types[i] = st;
            if st.pred == 0 {
                self.cur.direct8 |= 1 << i;
                if !self.env.sps.direct_8x8_inference {
                    no_lt8 = false;
                }
            } else if st.num > 1 {
                no_lt8 = false;
            }
        }
        if self.cur.direct8 != 0 {
            // Direct motion does not depend on the other partitions of this MB; it becomes
            // visible to neighbour derivation only at its turn in partition order.
            self.derive_direct(self.cur.direct8, false)?;
        }
        let lists = if b { 2 } else { 1 };
        let mut refs = [[-1i8; 4]; 2];
        for list in 0..lists {
            for i in 0..4 {
                let st = self.sub_types[i];
                if st.pred & (1 << list) != 0 {
                    let (x, y) = ((i & 1) * 2, (i >> 1) * 2);
                    let r = if ref0 { 0 } else { self.read_ref_idx(list, x, y)? };
                    refs[list][i] = r;
                    self.parsed_ref[list][i] = r;
                }
            }
        }
        for list in 0..lists {
            for i in 0..4 {
                let st = self.sub_types[i];
                let (x8, y8) = ((i & 1) * 2, (i >> 1) * 2);
                if st.pred == 0 {
                    self.mark_decoded(list, x8, y8, 2, 2);
                    continue;
                }
                if st.pred & (1 << list) == 0 {
                    self.put_motion(list, x8, y8, 2, 2, -1, [0, 0]);
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
                    let mvd = self.read_mvd(list, x, y)?;
                    self.store_mvd(list, x, y, w, h, mvd);
                    let r = refs[list][i];
                    let p = self.mvp(list, x, y, w, r, Shape::Median);
                    let mv = [p[0].wrapping_add(mvd[0]) as i16, p[1].wrapping_add(mvd[1]) as i16];
                    self.put_motion(list, x, y, w, h, r, mv);
                }
            }
        }
        Ok(no_lt8)
    }

    // ---- direct prediction -------------------------------------------------------------

    pub(crate) fn derive_direct_all(&mut self) -> Result<()> {
        self.derive_direct(0xf, true)
    }

    /// Co-located motion (8.4.1.2.1) for the 4x4 block (x, y) of the current MB:
    /// (mvCol, refIdxCol, identity of the referenced picture, vertMvScale).
    fn colocated(&self, x: usize, y: usize) -> ([i32; 2], i8, i32, VertScale) {
        let l1 = &self.env.refs[1][0];
        let col = &*l1.pic;
        let (x, y) = if self.env.sps.direct_8x8_inference { ((x >> 1) * 3, (y >> 1) * 3) } else { (x, y) };
        let (xcol, ycol) = (x * 4, y * 4);
        let w = self.mb_w;
        let addr = self.addr;
        let cur_pcs = if self.field_pic {
            Pcs::Fld
        } else if self.mbaff {
            Pcs::Afrm
        } else {
            Pcs::Frm
        };
        // Table 8-6: the co-located picture.
        let (motion, col_pcs): (&[MbMotion], Pcs) = match (self.field_pic, col.structure) {
            (true, Structure::Fields) => (&col.field_motion[l1.parity.min(1) as usize], Pcs::Fld),
            (_, Structure::Frame) => (&col.motion, Pcs::Frm),
            (_, Structure::Mbaff) => (&col.motion, Pcs::Afrm),
            (false, Structure::Fields) => {
                let par = if self.mb_field {
                    self.is_bottom as usize
                } else {
                    let cur = self.env.cur_poc;
                    if (col.top_poc - cur).abs() < (col.bottom_poc - cur).abs() { 0 } else { 1 }
                };
                (&col.field_motion[par], Pcs::Fld)
            }
        };
        let top_abs = (col.top_poc - self.env.cur_poc).abs();
        let bottom_abs = (col.bottom_poc - self.env.cur_poc).abs();
        // Table 8-8: co-located macroblock, yM and vertMvScale.
        let (mb_col, ym, vs) = match (cur_pcs, col_pcs) {
            (Pcs::Fld, Pcs::Fld) => (addr, ycol, VertScale::One),
            (Pcs::Fld, Pcs::Frm) => (2 * w * (addr / w) + addr % w + w * (ycol / 8), (2 * ycol) % 16, VertScale::FrmToFld),
            (Pcs::Fld, Pcs::Afrm) => {
                let x = 2 * addr;
                if motion.get(x).map(|m| m.field).unwrap_or(false) {
                    (x + self.bottom_field as usize, ycol, VertScale::One)
                } else {
                    (x + ycol / 8, (2 * ycol) % 16, VertScale::FrmToFld)
                }
            }
            (Pcs::Frm, Pcs::Fld) => (w * (addr / (2 * w)) + addr % w, 8 * ((addr / w) % 2) + 4 * (ycol / 8), VertScale::FldToFrm),
            (Pcs::Afrm, Pcs::Fld) => {
                if self.mb_field {
                    (addr / 2, ycol, VertScale::One)
                } else {
                    (addr / 2, 8 * (addr % 2) + 4 * (ycol / 8), VertScale::FldToFrm)
                }
            }
            (Pcs::Afrm, Pcs::Afrm) => {
                let col_field = motion.get(addr).map(|m| m.field).unwrap_or(false);
                match (self.mb_field, col_field) {
                    (false, false) | (true, true) => (addr, ycol, VertScale::One),
                    (false, true) => {
                        (2 * (addr / 2) + if top_abs < bottom_abs { 0 } else { 1 }, 8 * (addr % 2) + 4 * (ycol / 8), VertScale::FldToFrm)
                    }
                    (true, false) => (2 * (addr / 2) + ycol / 8, (2 * ycol) % 16, VertScale::FrmToFld),
                }
            }
            _ => (addr, ycol, VertScale::One),
        };
        let Some(m) = motion.get(mb_col) else {
            return ([0, 0], -1, -1, vs);
        };
        if m.intra {
            return ([0, 0], -1, -1, vs);
        }
        let blk = (ym / 4) * 4 + xcol / 4;
        let b8 = (ym / 8) * 2 + xcol / 8;
        let l = if m.ref_idx[0][b8] >= 0 { 0 } else { 1 };
        let mv = m.mv[l][blk];
        ([mv[0] as i32, mv[1] as i32], m.ref_idx[l][b8], m.ref_pic[l][b8], vs)
    }

    /// Derive direct-mode motion for the 8x8 blocks in `mask`. `visible` marks the blocks
    /// as decoded for neighbour derivation (B_Skip / B_Direct_16x16).
    pub(crate) fn derive_direct(&mut self, mask: u8, visible: bool) -> Result<()> {
        check(!self.ref_list(1).is_empty() && !self.ref_list(0).is_empty(), "direct prediction without references")?;
        let step = if self.env.sps.direct_8x8_inference { 2 } else { 1 };
        let put = |s: &mut Self, list: usize, x: usize, y: usize, r: i8, mv: [i16; 2]| {
            if visible {
                s.put_motion(list, x, y, step, step, r, mv);
            } else {
                s.set_motion(list, x, y, step, step, r, mv);
            }
        };
        if self.env.hdr.direct_spatial_mv_pred {
            let mut r = [-1i8; 2];
            for list in 0..2 {
                let (a, _) = self.nb_motion(list, -1, 0);
                let (b, _) = self.nb_motion(list, 0, -1);
                let (mut c, _) = self.nb_motion(list, 16, -1);
                if c == REF_NA {
                    (c, _) = self.nb_motion(list, -1, -1);
                }
                r[list] = min_positive(a.max(-1), min_positive(b.max(-1), c.max(-1)));
            }
            let direct_zero = r[0] < 0 && r[1] < 0;
            if direct_zero {
                r = [0, 0];
            }
            let mut mvp = [[0i16; 2]; 2];
            for list in 0..2 {
                if r[list] >= 0 && !direct_zero {
                    let p = self.mvp(list, 0, 0, 4, r[list], Shape::Median);
                    mvp[list] = [p[0] as i16, p[1] as i16];
                }
            }
            let l1_short = !self.ref_list(1)[0].long_term;
            for b8 in 0..4 {
                if mask >> b8 & 1 == 0 {
                    continue;
                }
                let (x8, y8) = ((b8 & 1) * 2, (b8 >> 1) * 2);
                for y in (y8..y8 + 2).step_by(step) {
                    for x in (x8..x8 + 2).step_by(step) {
                        let (mvc, refc, _, _) = self.colocated(x, y);
                        let col_zero = l1_short && refc == 0 && (-1..=1).contains(&mvc[0]) && (-1..=1).contains(&mvc[1]);
                        for list in 0..2 {
                            if r[list] < 0 {
                                put(self, list, x, y, -1, [0, 0]);
                            } else {
                                let mv = if direct_zero || (r[list] == 0 && col_zero) { [0, 0] } else { mvp[list] };
                                put(self, list, x, y, r[list], mv);
                            }
                        }
                    }
                }
            }
        } else {
            let cur_poc = self.cur_poc();
            let list0 = self.ref_list(0);
            let poc1 = self.ref_list(1)[0].poc;
            let cur_par = self.cur_parity();
            for b8 in 0..4 {
                if mask >> b8 & 1 == 0 {
                    continue;
                }
                let (x8, y8) = ((b8 & 1) * 2, (b8 >> 1) * 2);
                for y in (y8..y8 + 2).step_by(step) {
                    for x in (x8..x8 + 2).step_by(step) {
                        let (mut mvc, refc, refpic, vs) = self.colocated(x, y);
                        match vs {
                            VertScale::FrmToFld => mvc[1] /= 2,
                            VertScale::FldToFrm => mvc[1] *= 2,
                            VertScale::One => {}
                        }
                        let ref0 = if refc < 0 || refpic < 0 {
                            0
                        } else {
                            // MapColToList0 (8.4.1.2.3): the picture referred to by the
                            // co-located block, as a field or frame of the current structure.
                            let id = refpic >> 2;
                            let target = match vs {
                                VertScale::One => refpic,
                                VertScale::FrmToFld => id * 4 + cur_par as i32,
                                VertScale::FldToFrm => id * 4 + PAR_FRAME as i32,
                            };
                            match list0.iter().position(|rp| rp.code() == target) {
                                Some(p) => p as i8,
                                None => list0.iter().position(|rp| (rp.code() >> 2) == id).unwrap_or(0) as i8,
                            }
                        };
                        let rp0 = &list0[ref0 as usize];
                        let poc0 = rp0.poc;
                        let td = (poc1 - poc0).clamp(-128, 127);
                        let (mv0, mv1) = if rp0.long_term || poc1 - poc0 == 0 {
                            ([mvc[0] as i16, mvc[1] as i16], [0i16, 0])
                        } else {
                            let tb = (cur_poc - poc0).clamp(-128, 127);
                            let tx = (16384 + (td / 2).abs()) / td;
                            let dsf = ((tb * tx + 32) >> 6).clamp(-1024, 1023);
                            let m0 = [(dsf * mvc[0] + 128) >> 8, (dsf * mvc[1] + 128) >> 8];
                            ([m0[0] as i16, m0[1] as i16], [(m0[0] - mvc[0]) as i16, (m0[1] - mvc[1]) as i16])
                        };
                        put(self, 0, x, y, ref0, mv0);
                        put(self, 1, x, y, 0, mv1);
                    }
                }
            }
        }
        Ok(())
    }
}
