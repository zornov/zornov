// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Inter prediction: merge candidates (spatial, temporal, combined bi-predictive,
//! zero), AMVP, temporal motion vector prediction (8.5.3.2), fractional sample
//! interpolation (8.5.3.3.3) and weighted sample prediction (8.5.3.3.4).

use crate::cabac::syntax::PartMode;
use crate::ctu::{SliceDecoder, F_INTRA};
use crate::picture::{Mv, MvField, Picture, Plane};

pub const LUMA_FILTER: [[i32; 8]; 4] = [
    [0, 0, 0, 64, 0, 0, 0, 0],
    [-1, 4, -10, 58, 17, -5, 1, 0],
    [-1, 4, -11, 40, 40, -11, 4, -1],
    [0, 1, -5, 17, 58, -10, 4, -1],
];

pub const CHROMA_FILTER: [[i32; 4]; 8] = [
    [0, 64, 0, 0],
    [-2, 58, 10, -2],
    [-4, 54, 16, -2],
    [-6, 46, 28, -4],
    [-4, 36, 36, -4],
    [-4, 28, 46, -6],
    [-2, 16, 54, -4],
    [-2, 10, 58, -2],
];

const COMB_IDX: [(usize, usize); 12] =
    [(0, 1), (1, 0), (0, 2), (2, 0), (1, 2), (2, 1), (0, 3), (3, 0), (1, 3), (3, 1), (2, 3), (3, 2)];

/// Motion vector scaling (8-179 .. 8-182).
pub fn scale_mv(mv: Mv, td: i32, tb: i32) -> Mv {
    let td = td.clamp(-128, 127);
    let tb = tb.clamp(-128, 127);
    if td == 0 {
        return mv;
    }
    let tx = (16384 + (td.abs() >> 1)) / td;
    let dsf = ((tb * tx + 32) >> 6).clamp(-4096, 4095);
    let sc = |c: i16| -> i16 {
        let p = dsf * c as i32;
        let v = p.signum() * ((p.abs() + 127) >> 8);
        v.clamp(-32768, 32767) as i16
    };
    Mv { x: sc(mv.x), y: sc(mv.y) }
}

impl<'a> SliceDecoder<'a> {
    /// Prediction block availability (6.4.2), including the intra check.
    #[allow(clippy::too_many_arguments)]
    fn pb_avail(
        &self,
        x_cb: usize,
        y_cb: usize,
        n_cb: usize,
        w: usize,
        h: usize,
        part_idx: usize,
        xn: isize,
        yn: isize,
    ) -> bool {
        if xn < 0 || yn < 0 || xn as usize >= self.sps.pic_width || yn as usize >= self.sps.pic_height {
            return false;
        }
        let (xu, yu) = (xn as usize, yn as usize);
        let same_cb = xu >= x_cb && yu >= y_cb && xu < x_cb + n_cb && yu < y_cb + n_cb;
        let a = if !same_cb {
            self.avail(xn, yn)
        } else {
            !((w << 1) == n_cb && (h << 1) == n_cb && part_idx == 1 && y_cb + h <= yu && x_cb + w > xu)
        };
        a && self.st.flags.get(self.idx4(xu, yu)) & F_INTRA == 0
    }

    #[inline]
    fn mvf_at(&self, x: isize, y: isize) -> MvField {
        self.pic.mvf_at(x as usize, y as usize)
    }

    // -----------------------------------------------------------------------
    // Temporal motion vector prediction (8.5.3.2.8 / 8.5.3.2.9)
    // -----------------------------------------------------------------------

    fn col_pic(&self) -> Option<&'a Picture> {
        let l = if self.sh.is_b() && !self.sh.collocated_from_l0 { 1 } else { 0 };
        if self.sh.collocated_ref_idx >= self.sh.num_ref_idx_active[l] {
            return None;
        }
        self.dpb.get(self.ref_dpb[l][self.sh.collocated_ref_idx])
    }

    fn col_mv(&self, col: &Picture, x: usize, y: usize, ref_idx: usize, lx: usize) -> Option<Mv> {
        if x >= col.width() || y >= col.height() {
            return None;
        }
        let f = col.mvf_at(x, y);
        if f.pred_flag == 0 {
            return None;
        }
        let list_col = if !f.uses(0) {
            1
        } else if !f.uses(1) {
            0
        } else if self.no_backward_pred {
            lx
        } else if self.sh.collocated_from_l0 {
            1
        } else {
            0
        };
        let ref_idx_col = f.ref_idx[list_col];
        if ref_idx_col < 0 || ref_idx_col >= 16 {
            return None;
        }
        let refs = col.slice_refs_at(x, y);
        let col_ref_poc = refs.poc[list_col][ref_idx_col as usize];
        let col_ref_lt = refs.is_lt[list_col][ref_idx_col as usize];
        let cur_lt = self.ref_lt[lx][ref_idx];
        if col_ref_lt != cur_lt {
            return None;
        }
        let mv_col = f.mv[list_col];
        let col_diff = col.poc - col_ref_poc;
        let cur_diff = self.pic.poc - self.ref_poc[lx][ref_idx];
        if cur_lt || col_diff == cur_diff {
            Some(mv_col)
        } else {
            Some(scale_mv(mv_col, col_diff, cur_diff))
        }
    }

    fn temporal_mv(&self, x_pb: usize, y_pb: usize, w: usize, h: usize, ref_idx: usize, lx: usize) -> Option<Mv> {
        if !self.sh.temporal_mvp_enabled {
            return None;
        }
        let col = self.col_pic()?;
        if col.width() != self.pic.width() || col.height() != self.pic.height() {
            return None;
        }
        let xbr = x_pb + w;
        let ybr = y_pb + h;
        let l2 = self.sps.log2_ctb;
        if (y_pb >> l2) == (ybr >> l2) && ybr < self.sps.pic_height && xbr < self.sps.pic_width {
            if let Some(mv) = self.col_mv(col, (xbr >> 4) << 4, (ybr >> 4) << 4, ref_idx, lx) {
                return Some(mv);
            }
        }
        let xc = x_pb + (w >> 1);
        let yc = y_pb + (h >> 1);
        self.col_mv(col, (xc >> 4) << 4, (yc >> 4) << 4, ref_idx, lx)
    }

    // -----------------------------------------------------------------------
    // Merge mode (8.5.3.2.2 - 8.5.3.2.5)
    // -----------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    pub fn derive_merge(
        &self,
        x_cb: usize,
        y_cb: usize,
        n_cb: usize,
        x_pb0: usize,
        y_pb0: usize,
        w0: usize,
        h0: usize,
        part_idx0: usize,
        merge_idx: usize,
    ) -> MvField {
        let par = self.pps.log2_parallel_merge_level;
        let (x_pb, y_pb, w, h, part_idx) = if par > 2 && n_cb == 8 {
            (x_cb, y_cb, n_cb, n_cb, 0)
        } else {
            (x_pb0, y_pb0, w0, h0, part_idx0)
        };
        let pm = self.part_mode;
        let diff_mer = |xn: isize, yn: isize| -> bool {
            (x_pb as isize >> par) == (xn >> par) && (y_pb as isize >> par) == (yn >> par)
        };
        let (xp, yp, wi, hi) = (x_pb as isize, y_pb as isize, w as isize, h as isize);
        let mut list: [MvField; 6] = [MvField::default(); 6];
        let mut cnt = 0usize;

        // A1
        let (xa1, ya1) = (xp - 1, yp + hi - 1);
        let avail_a1 = !diff_mer(xa1, ya1)
            && !(part_idx == 1 && matches!(pm, PartMode::PNx2N | PartMode::PnLx2N | PartMode::PnRx2N))
            && self.pb_avail(x_cb, y_cb, n_cb, w, h, part_idx, xa1, ya1);
        let a1 = if avail_a1 { self.mvf_at(xa1, ya1) } else { MvField::default() };
        if avail_a1 {
            list[cnt] = a1;
            cnt += 1;
        }
        // B1
        let (xb1, yb1) = (xp + wi - 1, yp - 1);
        let avail_b1 = !diff_mer(xb1, yb1)
            && !(part_idx == 1 && matches!(pm, PartMode::P2NxN | PartMode::P2NxnU | PartMode::P2NxnD))
            && self.pb_avail(x_cb, y_cb, n_cb, w, h, part_idx, xb1, yb1);
        let b1 = if avail_b1 { self.mvf_at(xb1, yb1) } else { MvField::default() };
        let flag_b1 = avail_b1 && !(avail_a1 && a1 == b1);
        if flag_b1 {
            list[cnt] = b1;
            cnt += 1;
        }
        // B0
        let (xb0, yb0) = (xp + wi, yp - 1);
        let avail_b0 = !diff_mer(xb0, yb0) && self.pb_avail(x_cb, y_cb, n_cb, w, h, part_idx, xb0, yb0);
        let b0 = if avail_b0 { self.mvf_at(xb0, yb0) } else { MvField::default() };
        let flag_b0 = avail_b0 && !(avail_b1 && b1 == b0);
        if flag_b0 {
            list[cnt] = b0;
            cnt += 1;
        }
        // A0
        let (xa0, ya0) = (xp - 1, yp + hi);
        let avail_a0 = !diff_mer(xa0, ya0) && self.pb_avail(x_cb, y_cb, n_cb, w, h, part_idx, xa0, ya0);
        let a0 = if avail_a0 { self.mvf_at(xa0, ya0) } else { MvField::default() };
        let flag_a0 = avail_a0 && !(avail_a1 && a1 == a0);
        if flag_a0 {
            list[cnt] = a0;
            cnt += 1;
        }
        // B2
        if cnt < 4 {
            let (xb2, yb2) = (xp - 1, yp - 1);
            let avail_b2 = !diff_mer(xb2, yb2) && self.pb_avail(x_cb, y_cb, n_cb, w, h, part_idx, xb2, yb2);
            if avail_b2 {
                let b2 = self.mvf_at(xb2, yb2);
                if !(avail_a1 && a1 == b2) && !(avail_b1 && b1 == b2) {
                    list[cnt] = b2;
                    cnt += 1;
                }
            }
        }
        let max = self.sh.max_num_merge_cand.max(1);
        let restrict = |mut f: MvField| -> MvField {
            if f.pred_flag == 3 && w0 + h0 == 12 {
                f.pred_flag = 1;
                f.ref_idx[1] = -1;
                f.mv[1] = Mv::ZERO;
            }
            f
        };
        if merge_idx < cnt {
            return restrict(list[merge_idx]);
        }
        // Temporal candidate
        if cnt < max {
            let mut col = MvField::default();
            if let Some(mv) = self.temporal_mv(x_pb, y_pb, w, h, 0, 0) {
                col.mv[0] = mv;
                col.ref_idx[0] = 0;
                col.pred_flag |= 1;
            }
            if self.sh.is_b() {
                if let Some(mv) = self.temporal_mv(x_pb, y_pb, w, h, 0, 1) {
                    col.mv[1] = mv;
                    col.ref_idx[1] = 0;
                    col.pred_flag |= 2;
                }
            }
            if col.pred_flag != 0 && cnt < 6 {
                list[cnt] = col;
                cnt += 1;
            }
        }
        let mut cands: Vec<MvField> = list[..cnt.min(max)].to_vec();
        // Combined bi-predictive candidates
        if self.sh.is_b() && cands.len() > 1 && cands.len() < max {
            let num_orig = cands.len();
            let mut comb_idx = 0;
            loop {
                let (i0, i1) = COMB_IDX[comb_idx];
                let l0 = cands[i0];
                let l1 = cands[i1];
                if l0.uses(0) && l1.uses(1) {
                    let p0 = self.ref_poc[0][l0.ref_idx[0] as usize & 15];
                    let p1 = self.ref_poc[1][l1.ref_idx[1] as usize & 15];
                    if p0 != p1 || l0.mv[0] != l1.mv[1] {
                        cands.push(MvField {
                            mv: [l0.mv[0], l1.mv[1]],
                            ref_idx: [l0.ref_idx[0], l1.ref_idx[1]],
                            pred_flag: 3,
                        });
                    }
                }
                comb_idx += 1;
                if comb_idx == num_orig * (num_orig - 1) || cands.len() == max {
                    break;
                }
            }
        }
        // Zero candidates
        let num_ref = if self.sh.is_b() {
            self.sh.num_ref_idx_active[0].min(self.sh.num_ref_idx_active[1])
        } else {
            self.sh.num_ref_idx_active[0]
        };
        let mut zero_idx = 0usize;
        while cands.len() < max {
            let r = if zero_idx < num_ref { zero_idx as i8 } else { 0 };
            let f = if self.sh.is_b() {
                MvField { mv: [Mv::ZERO; 2], ref_idx: [r, r], pred_flag: 3 }
            } else {
                MvField { mv: [Mv::ZERO; 2], ref_idx: [r, -1], pred_flag: 1 }
            };
            cands.push(f);
            zero_idx += 1;
        }
        restrict(cands[merge_idx.min(cands.len() - 1)])
    }

    // -----------------------------------------------------------------------
    // AMVP (8.5.3.2.6 / 8.5.3.2.7)
    // -----------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    pub fn derive_amvp(
        &self,
        x_cb: usize,
        y_cb: usize,
        n_cb: usize,
        x_pb: usize,
        y_pb: usize,
        w: usize,
        h: usize,
        part_idx: usize,
        ref_idx: usize,
        lx: usize,
        mvp_flag: usize,
    ) -> Mv {
        let ly = 1 - lx;
        let target_poc = self.ref_poc[lx][ref_idx];
        let target_lt = self.ref_lt[lx][ref_idx];
        let (xp, yp, wi, hi) = (x_pb as isize, y_pb as isize, w as isize, h as isize);
        let same_ref = |f: &MvField, l: usize| -> bool {
            let r = f.ref_idx[l] as usize & 15;
            self.ref_poc[l][r] == target_poc
        };
        // Non-scaled candidate from a neighbour.
        let unscaled = |f: &MvField| -> Option<Mv> {
            if f.uses(lx) && same_ref(f, lx) {
                Some(f.mv[lx])
            } else if f.uses(ly) && same_ref(f, ly) {
                Some(f.mv[ly])
            } else {
                None
            }
        };
        // Scaled candidate (long-term status must match).
        let scaled = |f: &MvField| -> Option<Mv> {
            for l in [lx, ly] {
                if f.uses(l) {
                    let r = f.ref_idx[l] as usize & 15;
                    let nb_lt = self.ref_lt[l][r];
                    if nb_lt == target_lt {
                        let mv = f.mv[l];
                        if !nb_lt && !target_lt {
                            let td = self.pic.poc - self.ref_poc[l][r];
                            let tb = self.pic.poc - target_poc;
                            return Some(scale_mv(mv, td, tb));
                        }
                        return Some(mv);
                    }
                }
            }
            None
        };

        // Spatial A
        let a_pos = [(xp - 1, yp + hi), (xp - 1, yp + hi - 1)];
        let mut a_av = [false; 2];
        let mut a_f = [MvField::default(); 2];
        for k in 0..2 {
            a_av[k] = self.pb_avail(x_cb, y_cb, n_cb, w, h, part_idx, a_pos[k].0, a_pos[k].1);
            if a_av[k] {
                a_f[k] = self.mvf_at(a_pos[k].0, a_pos[k].1);
            }
        }
        let is_scaled = a_av[0] || a_av[1];
        let mut mv_a: Option<Mv> = None;
        for k in 0..2 {
            if a_av[k] {
                if let Some(m) = unscaled(&a_f[k]) {
                    mv_a = Some(m);
                    break;
                }
            }
        }
        if mv_a.is_none() {
            for k in 0..2 {
                if a_av[k] {
                    if let Some(m) = scaled(&a_f[k]) {
                        mv_a = Some(m);
                        break;
                    }
                }
            }
        }
        // Spatial B
        let b_pos = [(xp + wi, yp - 1), (xp + wi - 1, yp - 1), (xp - 1, yp - 1)];
        let mut b_av = [false; 3];
        let mut b_f = [MvField::default(); 3];
        for k in 0..3 {
            b_av[k] = self.pb_avail(x_cb, y_cb, n_cb, w, h, part_idx, b_pos[k].0, b_pos[k].1);
            if b_av[k] {
                b_f[k] = self.mvf_at(b_pos[k].0, b_pos[k].1);
            }
        }
        let mut mv_b: Option<Mv> = None;
        for k in 0..3 {
            if b_av[k] {
                if let Some(m) = unscaled(&b_f[k]) {
                    mv_b = Some(m);
                    break;
                }
            }
        }
        if !is_scaled && mv_b.is_some() && mv_a.is_none() {
            mv_a = mv_b;
        }
        if !is_scaled {
            mv_b = None;
            for k in 0..3 {
                if b_av[k] {
                    if let Some(m) = scaled(&b_f[k]) {
                        mv_b = Some(m);
                        break;
                    }
                }
            }
        }
        let mut cands: [Mv; 3] = [Mv::ZERO; 3];
        let mut cnt = 0;
        if let Some(a) = mv_a {
            cands[cnt] = a;
            cnt += 1;
        }
        if let Some(b) = mv_b {
            if !(mv_a.is_some() && mv_a == Some(b)) {
                cands[cnt] = b;
                cnt += 1;
            }
        }
        if cnt < 2 {
            if let Some(t) = self.temporal_mv(x_pb, y_pb, w, h, ref_idx, lx) {
                cands[cnt] = t;
                cnt += 1;
            }
        }
        while cnt < 2 {
            cands[cnt] = Mv::ZERO;
            cnt += 1;
        }
        cands[mvp_flag.min(1)]
    }

    // -----------------------------------------------------------------------
    // Sample prediction
    // -----------------------------------------------------------------------

    /// Decodes the inter prediction samples of one PB into the current picture.
    pub fn predict_inter(&mut self, x_pb: usize, y_pb: usize, w: usize, h: usize, f: &MvField) -> crate::error::Result<()> {
        let mut refs: [Option<&'a Picture>; 2] = [None, None];
        for l in 0..2 {
            if f.uses(l) {
                let ri = f.ref_idx[l];
                if ri < 0 || ri as usize >= self.sh.num_ref_idx_active[l] {
                    return Err(crate::error::Error::Invalid("reference index out of range"));
                }
                let p = self
                    .dpb
                    .get(self.ref_dpb[l][ri as usize])
                    .ok_or(crate::error::Error::Invalid("missing reference picture"))?;
                if p.width() != self.pic.width() || p.height() != self.pic.height() {
                    return Err(crate::error::Error::Invalid("reference picture size mismatch"));
                }
                refs[l] = Some(p);
            }
        }
        let weighted = self.sh.pwt.is_some();
        let mut pred0 = std::mem::take(&mut self.pred_buf[0]);
        let mut pred1 = std::mem::take(&mut self.pred_buf[1]);
        for c in 0..3usize {
            let (xc, yc, wc, hc) = if c == 0 { (x_pb, y_pb, w, h) } else { (x_pb / 2, y_pb / 2, w / 2, h / 2) };
            let bd = if c == 0 { self.sps.bit_depth_luma } else { self.sps.bit_depth_chroma } as u32;
            for l in 0..2 {
                if let Some(rp) = refs[l] {
                    let buf = if l == 0 { &mut pred0 } else { &mut pred1 };
                    if c == 0 {
                        interp_luma(&rp.planes[0], xc, yc, wc, hc, f.mv[l], bd, buf, &mut self.src_buf, &mut self.tmp_buf);
                    } else {
                        interp_chroma(&rp.planes[c], xc, yc, wc, hc, f.mv[l], bd, buf, &mut self.src_buf, &mut self.tmp_buf);
                    }
                }
            }
            let plane = self.pic.planes[c];
            let maxv = (1i32 << bd) - 1;
            let bi = refs[0].is_some() && refs[1].is_some();
            // SAFETY: the prediction block lies in the CTU owned by this decoder.
            let rows = (0..hc).map(|y| unsafe { plane.row_mut(xc, yc + y, wc) });
            if !weighted {
                if bi {
                    let shift2 = 15 - bd as i32;
                    let off2 = 1 << (shift2 - 1);
                    for (y, r) in rows.enumerate() {
                        let (a, b) = (&pred0[y * wc..y * wc + wc], &pred1[y * wc..y * wc + wc]);
                        put_bi(r, a, b, off2, shift2, maxv);
                    }
                } else {
                    let p = if refs[0].is_some() { &pred0 } else { &pred1 };
                    let shift1 = 14 - bd as i32;
                    let off1 = if shift1 > 0 { 1 << (shift1 - 1) } else { 0 };
                    for (y, r) in rows.enumerate() {
                        put_uni(r, &p[y * wc..y * wc + wc], off1, shift1, maxv);
                    }
                }
            } else {
                let pwt = self.sh.pwt.as_ref().expect("weighted");
                let shift1 = 14 - bd as i32;
                let denom = if c == 0 { pwt.luma_log2_denom } else { pwt.chroma_log2_denom } as i32;
                let log2wd = denom + shift1;
                let wo = |l: usize| -> (i32, i32) {
                    let r = (f.ref_idx[l].max(0) as usize).min(15);
                    (pwt.weight[l][r][c], pwt.offset[l][r][c] << (bd - 8))
                };
                if bi {
                    let (w0, o0) = wo(0);
                    let (w1, o1) = wo(1);
                    let off = (o0 + o1 + 1) << log2wd;
                    for (y, r) in rows.enumerate() {
                        let (a, b) = (&pred0[y * wc..y * wc + wc], &pred1[y * wc..y * wc + wc]);
                        put_w_bi(r, a, b, w0, w1, off, log2wd + 1, maxv);
                    }
                } else {
                    let l = if refs[0].is_some() { 0 } else { 1 };
                    let p = if l == 0 { &pred0 } else { &pred1 };
                    let (w0, o0) = wo(l);
                    for (y, r) in rows.enumerate() {
                        put_w_uni(r, &p[y * wc..y * wc + wc], w0, o0, log2wd, maxv);
                    }
                }
            }
        }
        self.pred_buf[0] = pred0;
        self.pred_buf[1] = pred1;
        Ok(())
    }
}

/// Returns a sample window of `w` x `h` starting at (x, y): either a view
/// into the plane (when fully inside) or a clamped copy in `buf`.
/// Result: (data, stride, offset of (x, y)).
#[inline]
fn window<'b>(plane: &'b Plane, x: isize, y: isize, w: usize, h: usize, buf: &'b mut Vec<u16>) -> (&'b [u16], usize, usize) {
    let pw = plane.width as isize;
    let ph = plane.height as isize;
    if x >= 0 && y >= 0 && x + w as isize <= pw && y + h as isize <= ph {
        return (&plane.data, plane.stride, y as usize * plane.stride + x as usize);
    }
    buf.clear();
    buf.resize(w * h, 0);
    let inside_x = x >= 0 && x + w as isize <= pw;
    for (j, out) in buf.chunks_exact_mut(w).enumerate() {
        let yy = (y + j as isize).clamp(0, ph - 1) as usize;
        let row = &plane.data[yy * plane.stride..yy * plane.stride + plane.width];
        if inside_x {
            out.copy_from_slice(&row[x as usize..x as usize + w]);
        } else {
            for (i, o) in out.iter_mut().enumerate() {
                *o = row[(x + i as isize).clamp(0, pw - 1) as usize];
            }
        }
    }
    (&buf[..], w, 0)
}

/// `out[y * w + x] = src[y * stride + x] << shift` for a `w` x `h` block.
#[inline(never)]
fn copy_shift(src: &[u16], stride: usize, out: &mut [i32], w: usize, h: usize, shift: i32) {
    for (y, o) in out[..w * h].chunks_exact_mut(w).enumerate() {
        let r = &src[y * stride..y * stride + w];
        for (o, &v) in o.iter_mut().zip(r) {
            *o = (v as i32) << shift;
        }
    }
}

/// Horizontal N-tap filter: `out[y][x] = (sum_k c[k] * src[y][x + k]) >> shift`.
#[inline(never)]
fn filter_h<const N: usize>(src: &[u16], stride: usize, out: &mut [i32], w: usize, h: usize, c: &[i32; N], shift: i32) {
    for (y, o) in out[..w * h].chunks_exact_mut(w).enumerate() {
        let r = &src[y * stride..y * stride + w + N - 1];
        for (x, o) in o.iter_mut().enumerate() {
            let t = &r[x..x + N];
            let mut s = 0i32;
            for k in 0..N {
                s += c[k] * t[k] as i32;
            }
            *o = s >> shift;
        }
    }
}

/// 8-bit horizontal N-tap filter in 16-bit arithmetic. Exact: for 8-bit
/// samples every filter output lies in [-24 * 255, 88 * 255], so the
/// wrapping 16-bit sum equals the true sum (shift1 is 0 at 8 bits).
#[inline(never)]
fn filter_h_8bit<const N: usize>(src: &[u16], stride: usize, out: &mut [i32], w: usize, h: usize, c: &[i32; N]) {
    let c16: [i16; N] = std::array::from_fn(|k| c[k] as i16);
    for (y, o) in out[..w * h].chunks_exact_mut(w).enumerate() {
        let r = &src[y * stride..y * stride + w + N - 1];
        for (x, o) in o.iter_mut().enumerate() {
            let t = &r[x..x + N];
            let mut s = 0i16;
            for k in 0..N {
                s = s.wrapping_add(c16[k].wrapping_mul(t[k] as i16));
            }
            *o = s as i32;
        }
    }
}

/// 8-bit vertical N-tap filter on samples in 16-bit arithmetic (see `filter_h_8bit`).
#[inline(never)]
fn filter_v_8bit<const N: usize>(src: &[u16], stride: usize, out: &mut [i32], w: usize, h: usize, c: &[i32; N]) {
    let c16: [i16; N] = std::array::from_fn(|k| c[k] as i16);
    for (y, o) in out[..w * h].chunks_exact_mut(w).enumerate() {
        let rows: [&[u16]; N] = std::array::from_fn(|k| &src[(y + k) * stride..(y + k) * stride + w]);
        for (x, o) in o.iter_mut().enumerate() {
            let mut s = 0i16;
            for k in 0..N {
                s = s.wrapping_add(c16[k].wrapping_mul(rows[k][x] as i16));
            }
            *o = s as i32;
        }
    }
}

/// Vertical N-tap filter on samples: `out[y][x] = (sum_k c[k] * src[y + k][x]) >> shift`.
#[inline(never)]
fn filter_v<const N: usize>(src: &[u16], stride: usize, out: &mut [i32], w: usize, h: usize, c: &[i32; N], shift: i32) {
    for (y, o) in out[..w * h].chunks_exact_mut(w).enumerate() {
        let rows: [&[u16]; N] = std::array::from_fn(|k| &src[(y + k) * stride..(y + k) * stride + w]);
        for (x, o) in o.iter_mut().enumerate() {
            let mut s = 0i32;
            for k in 0..N {
                s += c[k] * rows[k][x] as i32;
            }
            *o = s >> shift;
        }
    }
}

/// Vertical N-tap filter on intermediate values (row stride `w`).
#[inline(never)]
fn filter_v_i32<const N: usize>(src: &[i32], out: &mut [i32], w: usize, h: usize, c: &[i32; N], shift: i32) {
    for (y, o) in out[..w * h].chunks_exact_mut(w).enumerate() {
        let rows: [&[i32]; N] = std::array::from_fn(|k| &src[(y + k) * w..(y + k) * w + w]);
        for (x, o) in o.iter_mut().enumerate() {
            let mut s = 0i32;
            for k in 0..N {
                s += c[k] * rows[k][x];
            }
            *o = s >> shift;
        }
    }
}

/// Fractional sample interpolation of a `w` x `h` block whose integer
/// position (minus the filter's left/top reach) is (xi, yi).
#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn interp<const N: usize>(
    plane: &Plane,
    xi: isize,
    yi: isize,
    w: usize,
    h: usize,
    cx: Option<&[i32; N]>,
    cy: Option<&[i32; N]>,
    bit_depth: u32,
    out: &mut [i32],
    buf: &mut Vec<u16>,
    tmp: &mut [i32],
) {
    let half = N / 2 - 1;
    let shift1 = (bit_depth as i32 - 8).min(4);
    let shift3 = (14 - bit_depth as i32).max(2);
    let sw = w + N - 1;
    let sh = h + N - 1;
    let (src, stride, base) = window(plane, xi - half as isize, yi - half as isize, sw, sh, buf);
    let eight = bit_depth == 8;
    match (cx, cy) {
        (None, None) => copy_shift(&src[base + half * stride + half..], stride, out, w, h, shift3),
        (Some(c), None) if eight => filter_h_8bit(&src[base + half * stride..], stride, out, w, h, c),
        (Some(c), None) => filter_h(&src[base + half * stride..], stride, out, w, h, c, shift1),
        (None, Some(c)) if eight => filter_v_8bit(&src[base + half..], stride, out, w, h, c),
        (None, Some(c)) => filter_v(&src[base + half..], stride, out, w, h, c, shift1),
        (Some(cx), Some(cy)) => {
            if eight {
                filter_h_8bit(&src[base..], stride, tmp, w, sh, cx);
            } else {
                filter_h(&src[base..], stride, tmp, w, sh, cx, shift1);
            }
            filter_v_i32(tmp, out, w, h, cy, 6);
        }
    }
}

/// Luma sample interpolation (8.5.3.3.3.1). Output in 14-bit intermediate precision.
#[allow(clippy::too_many_arguments)]
pub fn interp_luma(
    plane: &Plane,
    x_pb: usize,
    y_pb: usize,
    w: usize,
    h: usize,
    mv: Mv,
    bit_depth: u32,
    out: &mut [i32],
    buf: &mut Vec<u16>,
    tmp: &mut [i32],
) {
    let fx = (mv.x & 3) as usize;
    let fy = (mv.y & 3) as usize;
    let xi = x_pb as isize + (mv.x as isize >> 2);
    let yi = y_pb as isize + (mv.y as isize >> 2);
    let cx = if fx != 0 { Some(&LUMA_FILTER[fx]) } else { None };
    let cy = if fy != 0 { Some(&LUMA_FILTER[fy]) } else { None };
    interp::<8>(plane, xi, yi, w, h, cx, cy, bit_depth, out, buf, tmp);
}

/// Chroma sample interpolation (8.5.3.3.3.2) for 4:2:0 (1/8 sample units).
#[allow(clippy::too_many_arguments)]
pub fn interp_chroma(
    plane: &Plane,
    xc: usize,
    yc: usize,
    w: usize,
    h: usize,
    mv: Mv,
    bit_depth: u32,
    out: &mut [i32],
    buf: &mut Vec<u16>,
    tmp: &mut [i32],
) {
    let fx = (mv.x & 7) as usize;
    let fy = (mv.y & 7) as usize;
    let xi = xc as isize + (mv.x as isize >> 3);
    let yi = yc as isize + (mv.y as isize >> 3);
    let cx = if fx != 0 { Some(&CHROMA_FILTER[fx]) } else { None };
    let cy = if fy != 0 { Some(&CHROMA_FILTER[fy]) } else { None };
    interp::<4>(plane, xi, yi, w, h, cx, cy, bit_depth, out, buf, tmp);
}

/// Default (non-weighted) uni-prediction output of one row.
#[inline(never)]
fn put_uni(d: &mut [u16], p: &[i32], off: i32, shift: i32, maxv: i32) {
    for (d, &p) in d.iter_mut().zip(p) {
        *d = ((p + off) >> shift).max(0).min(maxv) as u16;
    }
}

/// Default (non-weighted) bi-prediction output of one row.
#[inline(never)]
fn put_bi(d: &mut [u16], p0: &[i32], p1: &[i32], off: i32, shift: i32, maxv: i32) {
    for ((d, &a), &b) in d.iter_mut().zip(p0).zip(p1) {
        *d = ((a + b + off) >> shift).max(0).min(maxv) as u16;
    }
}

/// Explicitly weighted uni-prediction output of one row.
#[inline(never)]
fn put_w_uni(d: &mut [u16], p: &[i32], w0: i32, o0: i32, log2wd: i32, maxv: i32) {
    if log2wd >= 1 {
        let rnd = 1 << (log2wd - 1);
        for (d, &p) in d.iter_mut().zip(p) {
            *d = (((p * w0 + rnd) >> log2wd) + o0).max(0).min(maxv) as u16;
        }
    } else {
        for (d, &p) in d.iter_mut().zip(p) {
            *d = (p * w0 + o0).max(0).min(maxv) as u16;
        }
    }
}

/// Explicitly weighted bi-prediction output of one row.
#[allow(clippy::too_many_arguments)]
#[inline(never)]
fn put_w_bi(d: &mut [u16], p0: &[i32], p1: &[i32], w0: i32, w1: i32, off: i32, shift: i32, maxv: i32) {
    for ((d, &a), &b) in d.iter_mut().zip(p0).zip(p1) {
        *d = ((a * w0 + b * w1 + off) >> shift).max(0).min(maxv) as u16;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mv_scaling() {
        let mv = Mv { x: 64, y: -32 };
        // same distance -> identity
        assert_eq!(scale_mv(mv, 4, 4), mv);
        // half distance
        let s = scale_mv(mv, 4, 2);
        assert_eq!(s, Mv { x: 32, y: -16 });
        // opposite direction
        let s = scale_mv(mv, 2, -2);
        assert_eq!(s, Mv { x: -64, y: 32 });
    }

    #[test]
    fn integer_interp_is_shift() {
        let mut p = Plane::new(16, 16, 0);
        for y in 0..16 {
            for x in 0..16 {
                p.set(x, y, (x * 10 + y) as u16);
            }
        }
        let mut out = vec![0; 64];
        let mut src: Vec<u16> = Vec::new();
        let mut tmp = vec![0; 16 * 16];
        interp_luma(&p, 4, 4, 4, 4, Mv { x: 4, y: 8 }, 8, &mut out, &mut src, &mut tmp);
        for y in 0..4 {
            for x in 0..4 {
                assert_eq!(out[y * 4 + x], (((5 + x) * 10 + (6 + y)) as i32) << 6);
            }
        }
        // constant plane: any fractional position yields the constant << 6
        let p = Plane::new(16, 16, 100);
        for (mx, my) in [(1, 0), (0, 3), (2, 2), (-5, 7)] {
            interp_luma(&p, 4, 4, 4, 4, Mv { x: mx, y: my }, 8, &mut out, &mut src, &mut tmp);
            assert!(out[..16].iter().all(|&v| v == 100 << 6), "{mx},{my}");
            interp_chroma(&p, 2, 2, 4, 4, Mv { x: mx, y: my }, 8, &mut out, &mut src, &mut tmp);
            assert!(out[..16].iter().all(|&v| v == 100 << 6), "c {mx},{my}");
        }
    }
}
