// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Slice segment data decoding: CTU / coding quadtree / coding unit /
//! prediction unit / transform tree parsing (7.3.8) interleaved with
//! reconstruction (clause 8.4 - 8.6).

use crate::cabac::engine::init_contexts;
use crate::cabac::syntax::{PartMode, PRED_BI, PRED_L0, PRED_L1};
use crate::cabac::{Cabac, Ctx, NUM_CTX};
use crate::error::{Error, Result};
use crate::params::{Pps, ScalingFactors, Sps, TileLayout};
use crate::picture::{Mv, MvField, PicView, Picture, SharedSlice};
use crate::slice::SliceHeader;
use crate::transform;

pub const F_DECODED: u8 = 1;
pub const F_INTRA: u8 = 2;
pub const F_SKIP: u8 = 4;
/// Samples must not be modified by in-loop filters (pcm + pcm_loop_filter_disabled, or bypass).
pub const F_NOFILTER: u8 = 8;
/// Luma transform block contains non-zero coefficients.
pub const F_NZ: u8 = 16;
pub const F_PCM: u8 = 32;

/// Edge types stored per 4x4 block for its left (vertical) / top (horizontal) boundary.
pub const EDGE_PU: u8 = 1;
pub const EDGE_TU: u8 = 2;

#[derive(Clone, Copy, Debug, Default)]
pub struct SaoParams {
    pub type_idx: [u8; 3],
    /// SaoOffsetVal[cIdx][1..=4] (index 0 unused, always 0)
    pub offset: [[i16; 5]; 3],
    pub band_position: [u8; 3],
    pub eo_class: [u8; 3],
}

/// Per-picture working state that is not needed once the picture is finished.
pub struct PicState {
    pub w4: usize,
    pub h4: usize,
    pub flags: Vec<u8>,
    pub ct_depth: Vec<u8>,
    pub intra_mode: Vec<u8>,
    pub qp_y: Vec<i8>,
    pub edge_v: Vec<u8>,
    pub edge_h: Vec<u8>,
    pub sao: Vec<SaoParams>,
    /// Index of the slice segment header (in the picture's header list) per CTB; u16::MAX = not decoded.
    pub ctb_hdr: Vec<u16>,
    pub ctb_slice_addr: Vec<usize>,
    pub qp_y_prev: i32,
    pub wpp_ctx: Option<[Ctx; NUM_CTX]>,
    pub ds_ctx: Option<[Ctx; NUM_CTX]>,
    pub any_nofilter: bool,
    pub ctbs_decoded: usize,
}

/// Per-picture decoding state that is not a per-block map (kept per slice
/// decoder while decoding and written back afterwards).
#[derive(Clone, Copy)]
pub struct StScalars {
    pub qp_y_prev: i32,
    pub wpp_ctx: Option<[Ctx; NUM_CTX]>,
    pub ds_ctx: Option<[Ctx; NUM_CTX]>,
    pub any_nofilter: bool,
    pub ctbs_decoded: usize,
}

/// Shared view of the per-block maps of a `PicState` (see `SharedSlice`).
#[derive(Clone, Copy)]
pub struct StView {
    pub w4: usize,
    pub flags: SharedSlice<u8>,
    pub ct_depth: SharedSlice<u8>,
    pub intra_mode: SharedSlice<u8>,
    pub qp_y: SharedSlice<i8>,
    pub edge_v: SharedSlice<u8>,
    pub edge_h: SharedSlice<u8>,
    pub sao: SharedSlice<SaoParams>,
    pub ctb_hdr: SharedSlice<u16>,
    pub ctb_slice_addr: SharedSlice<usize>,
}

impl StView {
    pub fn new(st: &mut PicState) -> StView {
        StView {
            w4: st.w4,
            flags: SharedSlice::new(&mut st.flags),
            ct_depth: SharedSlice::new(&mut st.ct_depth),
            intra_mode: SharedSlice::new(&mut st.intra_mode),
            qp_y: SharedSlice::new(&mut st.qp_y),
            edge_v: SharedSlice::new(&mut st.edge_v),
            edge_h: SharedSlice::new(&mut st.edge_h),
            sao: SharedSlice::new(&mut st.sao),
            ctb_hdr: SharedSlice::new(&mut st.ctb_hdr),
            ctb_slice_addr: SharedSlice::new(&mut st.ctb_slice_addr),
        }
    }
}

impl PicState {
    pub fn scalars(&self) -> StScalars {
        StScalars {
            qp_y_prev: self.qp_y_prev,
            wpp_ctx: self.wpp_ctx,
            ds_ctx: self.ds_ctx,
            any_nofilter: self.any_nofilter,
            ctbs_decoded: self.ctbs_decoded,
        }
    }

    pub fn set_scalars(&mut self, s: &StScalars) {
        self.qp_y_prev = s.qp_y_prev;
        self.wpp_ctx = s.wpp_ctx;
        self.ds_ctx = s.ds_ctx;
        self.any_nofilter = s.any_nofilter;
        self.ctbs_decoded = s.ctbs_decoded;
    }

    pub fn matches(&self, sps: &Sps) -> bool {
        self.w4 == sps.pic_width.div_ceil(4) && self.h4 == sps.pic_height.div_ceil(4) && self.sao.len() == sps.pic_size_in_ctbs()
    }

    /// Prepares a reused state for a new picture.
    pub fn reset(&mut self) {
        self.flags.fill(0);
        self.edge_v.fill(0);
        self.edge_h.fill(0);
        self.ctb_hdr.fill(u16::MAX);
        self.ctb_slice_addr.fill(usize::MAX);
        self.qp_y_prev = 26;
        self.wpp_ctx = None;
        self.ds_ctx = None;
        self.any_nofilter = false;
        self.ctbs_decoded = 0;
    }

    pub fn new(sps: &Sps) -> PicState {
        let w4 = sps.pic_width.div_ceil(4);
        let h4 = sps.pic_height.div_ceil(4);
        let nctb = sps.pic_size_in_ctbs();
        PicState {
            w4,
            h4,
            flags: vec![0; w4 * h4],
            ct_depth: vec![0; w4 * h4],
            intra_mode: vec![1; w4 * h4],
            qp_y: vec![0; w4 * h4],
            edge_v: vec![0; w4 * h4],
            edge_h: vec![0; w4 * h4],
            sao: vec![SaoParams::default(); nctb],
            ctb_hdr: vec![u16::MAX; nctb],
            ctb_slice_addr: vec![usize::MAX; nctb],
            qp_y_prev: 26,
            wpp_ctx: None,
            ds_ctx: None,
            any_nofilter: false,
            ctbs_decoded: 0,
        }
    }
}

/// Chroma QP mapping for ChromaArrayType == 1 (Table 8-10).
pub fn chroma_qp_table(qpi: i32) -> i32 {
    const T: [i32; 14] = [29, 30, 31, 32, 33, 33, 34, 34, 35, 35, 36, 36, 37, 37];
    if qpi < 30 {
        qpi
    } else if qpi > 43 {
        qpi - 6
    } else {
        T[(qpi - 30) as usize]
    }
}

pub struct SliceDecoder<'a> {
    pub sps: &'a Sps,
    pub pps: &'a Pps,
    pub sh: &'a SliceHeader,
    pub layout: &'a TileLayout,
    pub scaling: Option<&'a ScalingFactors>,
    pub cabac: Cabac<'a>,
    /// Shared views of the picture and state being decoded (several slice
    /// decoders may work on one picture concurrently, see `wpp`).
    pub pic: PicView,
    pub st: StView,
    /// Scalar decoding state (written back to the `PicState` by the caller).
    pub sc: StScalars,
    pub dpb: &'a [Picture],
    pub ref_dpb: [[usize; 16]; 2],
    pub ref_poc: [[i32; 16]; 2],
    pub ref_lt: [[bool; 16]; 2],
    pub hdr_idx: u16,
    pub no_backward_pred: bool,
    // current CTB
    pub ctb_addr_rs: usize,
    pub ctb_addr_ts: usize,
    pub ctb_x: usize,
    pub ctb_y: usize,
    /// Availability of the neighbouring CTBs: [dy + 1][dx + 1] for dy in -1..=0, dx in -1..=1.
    pub nb_ok: [[bool; 3]; 2],
    pub tile_id: usize,
    // quantisation state
    pub qp_y: i32,
    pub qp_y_pred: i32,
    pub is_cu_qp_delta_coded: bool,
    pub cu_qp_delta_val: i32,
    pub first_qg: bool,
    // current coding unit
    pub cu_transquant_bypass: bool,
    pub cu_intra: bool,
    pub part_mode: PartMode,
    pub intra_split: bool,
    pub max_trafo_depth: u8,
    pub chroma_mode: u8,
    pub merge_2nx2n: bool,
    pub log2_cb: u8,
    pub ct_depth: u8,
    // scratch
    pub coeffs: Vec<i32>,
    pub res: Vec<i32>,
    pub nz_pos: Vec<u16>,
    pub tr_tmp: Vec<i32>,
    pub pred_buf: [Vec<i32>; 2],
    pub tmp_buf: Vec<i32>,
    pub src_buf: Vec<u16>,
    pub intra_out: Vec<i32>,
    _borrow: std::marker::PhantomData<&'a mut Picture>,
}

impl<'a> SliceDecoder<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        sps: &'a Sps,
        pps: &'a Pps,
        sh: &'a SliceHeader,
        layout: &'a TileLayout,
        scaling: Option<&'a ScalingFactors>,
        data: &'a [u8],
        pic: &'a mut Picture,
        st: &'a mut PicState,
        dpb: &'a [Picture],
        ref_dpb: [[usize; 16]; 2],
        hdr_idx: u16,
    ) -> SliceDecoder<'a> {
        let sc = st.scalars();
        let pv = PicView::new(pic);
        let sv = StView::new(st);
        Self::from_views(sps, pps, sh, layout, scaling, data, pv, sv, sc, dpb, ref_dpb, hdr_idx)
    }

    /// Creates a slice decoder working on shared views of the picture and
    /// its state. The caller guarantees that the views stay valid for `'a`
    /// and that concurrent decoders only touch disjoint CTUs (see `wpp`).
    #[allow(clippy::too_many_arguments)]
    pub fn from_views(
        sps: &'a Sps,
        pps: &'a Pps,
        sh: &'a SliceHeader,
        layout: &'a TileLayout,
        scaling: Option<&'a ScalingFactors>,
        data: &'a [u8],
        pic: PicView,
        st: StView,
        sc: StScalars,
        dpb: &'a [Picture],
        ref_dpb: [[usize; 16]; 2],
        hdr_idx: u16,
    ) -> SliceDecoder<'a> {
        let mut ref_poc = [[0i32; 16]; 2];
        let mut ref_lt = [[false; 16]; 2];
        let mut no_backward_pred = true;
        for l in 0..2 {
            for i in 0..sh.num_ref_idx_active[l] {
                if let Some(p) = dpb.get(ref_dpb[l][i]) {
                    ref_poc[l][i] = p.poc;
                    ref_lt[l][i] = p.mark == crate::picture::RefMark::LongTerm;
                    if p.poc > pic.poc {
                        no_backward_pred = false;
                    }
                }
            }
        }
        let start = sh.data_offset;
        let cabac = Cabac::new(data, start, sh.slice_qp, sh.init_type());
        SliceDecoder {
            sps,
            pps,
            sh,
            layout,
            scaling,
            cabac,
            pic,
            st,
            sc,
            dpb,
            ref_dpb,
            ref_poc,
            ref_lt,
            hdr_idx,
            no_backward_pred,
            ctb_addr_rs: 0,
            ctb_addr_ts: 0,
            ctb_x: 0,
            ctb_y: 0,
            nb_ok: [[false; 3]; 2],
            tile_id: 0,
            qp_y: sh.slice_qp,
            qp_y_pred: sh.slice_qp,
            is_cu_qp_delta_coded: false,
            cu_qp_delta_val: 0,
            first_qg: true,
            cu_transquant_bypass: false,
            cu_intra: false,
            part_mode: PartMode::P2Nx2N,
            intra_split: false,
            max_trafo_depth: 0,
            chroma_mode: 0,
            merge_2nx2n: false,
            log2_cb: 3,
            ct_depth: 0,
            coeffs: vec![0; 32 * 32],
            res: vec![0; 32 * 32],
            nz_pos: Vec::with_capacity(32 * 32),
            tr_tmp: vec![0; 32 * 32],
            pred_buf: [vec![0; 64 * 64], vec![0; 64 * 64]],
            tmp_buf: vec![0; (64 + 7) * 64],
            src_buf: Vec::with_capacity((64 + 7) * (64 + 7)),
            intra_out: vec![0; 32 * 32],
            _borrow: std::marker::PhantomData,
        }
    }

    // -----------------------------------------------------------------------
    // Map helpers
    // -----------------------------------------------------------------------

    #[inline(always)]
    pub fn idx4(&self, x: usize, y: usize) -> usize {
        (y >> 2) * self.st.w4 + (x >> 2)
    }

    #[inline]
    fn ctb_of(&self, x: usize, y: usize) -> usize {
        (y >> self.sps.log2_ctb) * self.sps.pic_width_in_ctbs + (x >> self.sps.log2_ctb)
    }

    /// z-scan order block availability (6.4.1) using the decoded map.
    #[inline]
    pub fn avail(&self, xn: isize, yn: isize) -> bool {
        if xn < 0 || yn < 0 || xn as usize >= self.sps.pic_width || yn as usize >= self.sps.pic_height {
            return false;
        }
        let (xu, yu) = (xn as usize, yn as usize);
        if self.st.flags.get(self.idx4(xu, yu)) & F_DECODED == 0 {
            return false;
        }
        let l2 = self.sps.log2_ctb;
        let dx = (xu >> l2) as isize - self.ctb_x as isize;
        let dy = (yu >> l2) as isize - self.ctb_y as isize;
        if (-1..=0).contains(&dy) && (-1..=1).contains(&dx) {
            return self.nb_ok[(dy + 1) as usize][(dx + 1) as usize];
        }
        self.avail_ctb(self.ctb_of(xu, yu))
    }

    /// Whether CTB `ctb` (already decoded) is in the current slice and tile.
    fn avail_ctb(&self, ctb: usize) -> bool {
        if ctb == self.ctb_addr_rs {
            return true;
        }
        self.st.ctb_slice_addr.get(ctb) == self.sh.slice_addr_rs
            && self.layout.tile_id[self.layout.rs_to_ts[ctb]] == self.tile_id
    }

    /// Availability of a neighbouring location before the CTU cache is set up.
    fn avail_slow(&self, xn: isize, yn: isize) -> bool {
        if xn < 0 || yn < 0 || xn as usize >= self.sps.pic_width || yn as usize >= self.sps.pic_height {
            return false;
        }
        let (xu, yu) = (xn as usize, yn as usize);
        if self.st.flags.get(self.idx4(xu, yu)) & F_DECODED == 0 {
            return false;
        }
        self.avail_ctb(self.ctb_of(xu, yu))
    }

    /// Precomputes the slice/tile availability of the neighbouring CTBs.
    fn setup_ctb_neighbours(&mut self, ctb_x: usize, ctb_y: usize) {
        self.ctb_x = ctb_x;
        self.ctb_y = ctb_y;
        let w = self.sps.pic_width_in_ctbs as isize;
        let h = self.sps.pic_height_in_ctbs as isize;
        for dy in 0..2 {
            for dx in 0..3 {
                let x = ctb_x as isize + dx as isize - 1;
                let y = ctb_y as isize + dy as isize - 1;
                self.nb_ok[dy][dx] = if dy == 1 && dx == 1 {
                    true
                } else if x < 0 || y < 0 || x >= w || y >= h || (dy == 1 && dx == 2) {
                    false
                } else {
                    let rs = (y * w + x) as usize;
                    self.st.ctb_hdr.get(rs) != u16::MAX && self.avail_ctb(rs)
                };
            }
        }
    }

    fn fill_map(map: SharedSlice<u8>, w4: usize, x: usize, y: usize, w: usize, h: usize, v: u8) {
        for yy in (y >> 2)..((y + h) >> 2) {
            map.fill(yy * w4 + (x >> 2), yy * w4 + ((x + w) >> 2), v);
        }
    }

    fn or_flags(&mut self, x: usize, y: usize, w: usize, h: usize, v: u8) {
        let w4 = self.st.w4;
        let x1 = ((x + w).min(self.sps.pic_width) + 3) >> 2;
        let y1 = ((y + h).min(self.sps.pic_height) + 3) >> 2;
        for yy in (y >> 2)..y1 {
            // SAFETY: the block lies in the CTU owned by this decoder.
            for e in unsafe { self.st.flags.slice_mut(yy * w4 + (x >> 2), yy * w4 + x1) } {
                *e |= v;
            }
        }
    }

    fn mark_edges(&mut self, x: usize, y: usize, w: usize, h: usize, kind: u8) {
        let w4 = self.st.w4;
        let pw = self.sps.pic_width;
        let ph = self.sps.pic_height;
        if x % 8 == 0 && x < pw {
            for yy in (y >> 2)..((y + h).min(ph).div_ceil(4)) {
                let i = yy * w4 + (x >> 2);
                self.st.edge_v.set(i, self.st.edge_v.get(i).max(kind));
            }
        }
        if y % 8 == 0 && y < ph {
            let r = (y >> 2) * w4;
            // SAFETY: the block lies in the CTU owned by this decoder.
            for e in unsafe { self.st.edge_h.slice_mut(r + (x >> 2), r + (x + w).min(pw).div_ceil(4)) } {
                *e = (*e).max(kind);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Slice data loop (7.3.8.1) with tiles / WPP / dependent slices (9.3.1)
    // -----------------------------------------------------------------------

    fn tile_col_start(&self, ctb_x: usize) -> usize {
        let mut s = 0;
        for &b in &self.layout.col_bd {
            if b <= ctb_x {
                s = b;
            }
        }
        s
    }

    fn first_ctb_in_tile(&self, ts: usize) -> bool {
        ts == 0 || self.layout.tile_id[ts] != self.layout.tile_id[ts - 1]
    }

    fn reset_contexts(&mut self) {
        self.cabac.ctx = init_contexts(self.sh.slice_qp, self.sh.init_type());
    }

    /// Context selection at the start of a WPP row: sync from the CTB above-right if available.
    pub(crate) fn wpp_row_start_contexts(&mut self, ctb_x: usize, ctb_y: usize) {
        let ctb = self.sps.ctb_size as isize;
        let x0 = (ctb_x << self.sps.log2_ctb) as isize;
        let y0 = (ctb_y << self.sps.log2_ctb) as isize;
        let xn = x0 + ctb;
        let yn = y0 - ctb;
        if self.avail_slow(xn, yn) {
            if let Some(c) = self.sc.wpp_ctx {
                self.cabac.ctx = c;
                return;
            }
        }
        self.reset_contexts();
    }

    pub fn decode(&mut self) -> Result<()> {
        let pic_size = self.sps.pic_size_in_ctbs();
        let w_ctb = self.sps.pic_width_in_ctbs;
        self.ctb_addr_rs = self.sh.segment_address;
        self.ctb_addr_ts = self.layout.rs_to_ts[self.ctb_addr_rs];
        if self.sh.dependent_slice_segment {
            let prev = self.ctb_addr_ts.checked_sub(1).map(|t| self.layout.ts_to_rs[t]);
            match prev {
                Some(p) if self.st.ctb_slice_addr.get(p) == self.sh.slice_addr_rs => {}
                _ => return Err(Error::Invalid("dependent slice segment does not follow its slice")),
            }
        }
        // Initial context selection (9.3.1).
        let ctb_x = self.ctb_addr_rs % w_ctb;
        let ctb_y = self.ctb_addr_rs / w_ctb;
        self.tile_id = self.layout.tile_id[self.ctb_addr_ts];
        let first_in_tile = self.first_ctb_in_tile(self.ctb_addr_ts);
        let wpp = self.pps.entropy_coding_sync_enabled;
        if first_in_tile {
            // contexts already initialised
        } else if wpp && ctb_x == self.tile_col_start(ctb_x) {
            // availability needs ctb_slice_addr of the current CTB set
            self.st.ctb_slice_addr.set(self.ctb_addr_rs, self.sh.slice_addr_rs);
            self.wpp_row_start_contexts(ctb_x, ctb_y);
        } else if self.sh.dependent_slice_segment {
            if let Some(c) = self.sc.ds_ctx {
                self.cabac.ctx = c;
            }
        }
        // qPY_PREV handling (8.6.1)
        if !self.sh.dependent_slice_segment || first_in_tile || (wpp && ctb_x == self.tile_col_start(ctb_x)) {
            self.first_qg = true;
            self.sc.qp_y_prev = self.sh.slice_qp;
        } else {
            self.first_qg = false;
        }
        self.qp_y = self.sc.qp_y_prev;

        loop {
            let ctb_x = self.ctb_addr_rs % w_ctb;
            let ctb_y = self.ctb_addr_rs / w_ctb;
            if self.st.ctb_hdr.get(self.ctb_addr_rs) != u16::MAX {
                return Err(Error::Invalid("CTB decoded twice"));
            }
            self.st.ctb_hdr.set(self.ctb_addr_rs, self.hdr_idx);
            self.st.ctb_slice_addr.set(self.ctb_addr_rs, self.sh.slice_addr_rs);
            self.pic.ctb_slice.set(self.ctb_addr_rs, self.hdr_idx);
            self.coding_tree_unit(ctb_x, ctb_y)?;
            self.sc.ctbs_decoded += 1;
            if self.cabac.overrun() {
                return Err(Error::UnexpectedEof);
            }
            // WPP storage after the 2nd CTB of a row in a tile.
            let col_start = self.tile_col_start(ctb_x);
            if wpp && (ctb_x == col_start + 1 || (ctb_x == col_start && self.tile_width(ctb_x) == 1)) {
                self.sc.wpp_ctx = Some(self.cabac.ctx);
            }
            let end_of_slice_segment = self.cabac.end_of_slice_segment_flag();
            self.sc.qp_y_prev = self.qp_y;
            self.ctb_addr_ts += 1;
            if end_of_slice_segment {
                if self.pps.dependent_slice_segments_enabled {
                    self.sc.ds_ctx = Some(self.cabac.ctx);
                }
                break;
            }
            if self.ctb_addr_ts >= pic_size {
                return Err(Error::Invalid("slice extends past end of picture"));
            }
            self.ctb_addr_rs = self.layout.ts_to_rs[self.ctb_addr_ts];
            let nx = self.ctb_addr_rs % w_ctb;
            let ny = self.ctb_addr_rs / w_ctb;
            let new_tile = self.layout.tile_id[self.ctb_addr_ts] != self.layout.tile_id[self.ctb_addr_ts - 1];
            let new_row = wpp && nx == self.tile_col_start(nx);
            if new_tile || new_row {
                self.cabac.end_of_subset()?;
                self.tile_id = self.layout.tile_id[self.ctb_addr_ts];
                if new_tile {
                    self.reset_contexts();
                } else {
                    self.st.ctb_slice_addr.set(self.ctb_addr_rs, self.sh.slice_addr_rs);
                    self.wpp_row_start_contexts(nx, ny);
                }
                self.first_qg = true;
                self.sc.qp_y_prev = self.sh.slice_qp;
                self.qp_y = self.sh.slice_qp;
            }
        }
        Ok(())
    }

    fn tile_width(&self, ctb_x: usize) -> usize {
        for w in self.layout.col_bd.windows(2) {
            if ctb_x >= w[0] && ctb_x < w[1] {
                return w[1] - w[0];
            }
        }
        1
    }

    // -----------------------------------------------------------------------
    // CTU and SAO syntax
    // -----------------------------------------------------------------------

    pub(crate) fn coding_tree_unit(&mut self, ctb_x: usize, ctb_y: usize) -> Result<()> {
        self.setup_ctb_neighbours(ctb_x, ctb_y);
        let x0 = ctb_x << self.sps.log2_ctb;
        let y0 = ctb_y << self.sps.log2_ctb;
        if self.sh.sao_luma || self.sh.sao_chroma {
            self.parse_sao(ctb_x, ctb_y)?;
        } else {
            self.st.sao.set(self.ctb_addr_rs, SaoParams::default());
        }
        self.coding_quadtree(x0, y0, self.sps.log2_ctb, 0)
    }

    fn parse_sao(&mut self, rx: usize, ry: usize) -> Result<()> {
        let w_ctb = self.sps.pic_width_in_ctbs;
        let rs = self.ctb_addr_rs;
        let mut merge_left = false;
        let mut merge_up = false;
        if rx > 0 {
            let left_in_slice = rs - 1 >= self.sh.slice_addr_rs && self.st.ctb_slice_addr.get(rs - 1) == self.sh.slice_addr_rs;
            let left_in_tile = self.layout.tile_id[self.ctb_addr_ts] == self.layout.tile_id[self.layout.rs_to_ts[rs - 1]];
            if left_in_slice && left_in_tile {
                merge_left = self.cabac.sao_merge_flag();
            }
        }
        if ry > 0 && !merge_left {
            let up = rs - w_ctb;
            let up_in_slice = up >= self.sh.slice_addr_rs && self.st.ctb_slice_addr.get(up) == self.sh.slice_addr_rs;
            let up_in_tile = self.layout.tile_id[self.ctb_addr_ts] == self.layout.tile_id[self.layout.rs_to_ts[up]];
            if up_in_slice && up_in_tile {
                merge_up = self.cabac.sao_merge_flag();
            }
        }
        if merge_left {
            self.st.sao.set(rs, self.st.sao.get(rs - 1));
            return Ok(());
        }
        if merge_up {
            self.st.sao.set(rs, self.st.sao.get(rs - w_ctb));
            return Ok(());
        }
        let mut p = SaoParams::default();
        let ncomp = if self.sps.chroma_format_idc != 0 { 3 } else { 1 };
        for c in 0..ncomp {
            let enabled = if c == 0 { self.sh.sao_luma } else { self.sh.sao_chroma };
            if !enabled {
                continue;
            }
            if c == 2 {
                p.type_idx[2] = p.type_idx[1];
                p.eo_class[2] = p.eo_class[1];
            } else {
                p.type_idx[c] = self.cabac.sao_type_idx();
            }
            if p.type_idx[c] == 0 {
                continue;
            }
            let bd = if c == 0 { self.sps.bit_depth_luma } else { self.sps.bit_depth_chroma } as u32;
            let cmax = (1u32 << (bd.min(10) - 5)) - 1;
            let mut abs = [0i32; 4];
            for a in abs.iter_mut() {
                *a = self.cabac.sao_offset_abs(cmax);
            }
            if p.type_idx[c] == 1 {
                for (i, a) in abs.iter().enumerate() {
                    let mut v = *a;
                    if v != 0 && self.cabac.bypass() != 0 {
                        v = -v;
                    }
                    p.offset[c][i + 1] = v as i16;
                }
                p.band_position[c] = self.cabac.sao_band_position();
            } else {
                p.offset[c][1] = abs[0] as i16;
                p.offset[c][2] = abs[1] as i16;
                p.offset[c][3] = -abs[2] as i16;
                p.offset[c][4] = -abs[3] as i16;
                if c == 0 {
                    p.eo_class[0] = self.cabac.sao_eo_class();
                }
                if c == 1 {
                    p.eo_class[1] = self.cabac.sao_eo_class();
                    p.eo_class[2] = p.eo_class[1];
                }
            }
        }
        self.st.sao.set(rs, p);
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Coding quadtree / coding unit
    // -----------------------------------------------------------------------

    fn log2_min_cu_qp_delta_size(&self) -> u8 {
        self.sps.log2_ctb - self.pps.diff_cu_qp_delta_depth
    }

    /// Starts a quantisation group at (x, y): derives qPY_PRED (8.6.1).
    fn start_quant_group(&mut self, xq: usize, yq: usize) {
        let prev = if self.first_qg { self.sh.slice_qp } else { self.sc.qp_y_prev };
        let qp_a = if self.avail(xq as isize - 1, yq as isize) && self.ctb_of(xq - 1, yq) == self.ctb_addr_rs {
            self.st.qp_y.get(self.idx4(xq - 1, yq)) as i32
        } else {
            prev
        };
        let qp_b = if self.avail(xq as isize, yq as isize - 1) && self.ctb_of(xq, yq - 1) == self.ctb_addr_rs {
            self.st.qp_y.get(self.idx4(xq, yq - 1)) as i32
        } else {
            prev
        };
        self.qp_y_pred = (qp_a + qp_b + 1) >> 1;
        self.is_cu_qp_delta_coded = false;
        self.cu_qp_delta_val = 0;
        self.update_qp();
    }

    fn update_qp(&mut self) {
        let off = self.sps.qp_bd_offset_y();
        self.qp_y = ((self.qp_y_pred + self.cu_qp_delta_val + 52 + 2 * off) % (52 + off)) - off;
    }

    fn coding_quadtree(&mut self, x0: usize, y0: usize, log2: u8, depth: u8) -> Result<()> {
        let size = 1usize << log2;
        let sps = self.sps;
        let split = if x0 + size <= sps.pic_width && y0 + size <= sps.pic_height && log2 > sps.log2_min_cb {
            let mut inc = 0;
            if self.avail(x0 as isize - 1, y0 as isize) && self.st.ct_depth.get(self.idx4(x0 - 1, y0)) > depth {
                inc += 1;
            }
            if self.avail(x0 as isize, y0 as isize - 1) && self.st.ct_depth.get(self.idx4(x0, y0 - 1)) > depth {
                inc += 1;
            }
            self.cabac.split_cu_flag(inc)
        } else {
            log2 > sps.log2_min_cb
        };
        if log2 >= self.log2_min_cu_qp_delta_size() {
            self.start_quant_group(x0, y0);
        }
        if split {
            let half = size >> 1;
            let x1 = x0 + half;
            let y1 = y0 + half;
            self.coding_quadtree(x0, y0, log2 - 1, depth + 1)?;
            if x1 < sps.pic_width {
                self.coding_quadtree(x1, y0, log2 - 1, depth + 1)?;
            }
            if y1 < sps.pic_height {
                self.coding_quadtree(x0, y1, log2 - 1, depth + 1)?;
            }
            if x1 < sps.pic_width && y1 < sps.pic_height {
                self.coding_quadtree(x1, y1, log2 - 1, depth + 1)?;
            }
            Ok(())
        } else {
            self.coding_unit(x0, y0, log2, depth)
        }
    }

    fn coding_unit(&mut self, x0: usize, y0: usize, log2: u8, depth: u8) -> Result<()> {
        let n = 1usize << log2;
        let sps = self.sps;
        let w4 = self.st.w4;
        self.log2_cb = log2;
        self.ct_depth = depth;
        self.cu_transquant_bypass = false;
        if self.pps.transquant_bypass_enabled {
            self.cu_transquant_bypass = self.cabac.cu_transquant_bypass_flag();
        }
        let mut skip = false;
        if !self.sh.is_intra() {
            let mut inc = 0;
            if self.avail(x0 as isize - 1, y0 as isize) && self.st.flags.get(self.idx4(x0 - 1, y0)) & F_SKIP != 0 {
                inc += 1;
            }
            if self.avail(x0 as isize, y0 as isize - 1) && self.st.flags.get(self.idx4(x0, y0 - 1)) & F_SKIP != 0 {
                inc += 1;
            }
            skip = self.cabac.cu_skip_flag(inc);
        }
        // Clear per-CU map entries (the CU may extend past the picture only by
        // whole min-CBs, which cannot happen as the size is a multiple of MinCb).
        Self::fill_map(self.st.ct_depth, w4, x0, y0, n, n, depth);
        {
            let base = if skip { F_SKIP } else { 0 };
            Self::fill_map(self.st.flags, w4, x0, y0, n, n, base);
        }
        // CU boundary is a transform edge.
        self.mark_edges(x0, y0, n, n, EDGE_TU);
        self.update_qp();
        self.cu_intra = false;
        self.intra_split = false;
        self.merge_2nx2n = false;
        self.part_mode = PartMode::P2Nx2N;

        if skip {
            self.prediction_unit(x0, y0, n, x0, y0, n, n, 0, true)?;
        } else {
            self.cu_intra = if self.sh.is_intra() { true } else { self.cabac.pred_mode_flag() };
            if self.cu_intra {
                Self::fill_map(self.st.flags, w4, x0, y0, n, n, F_INTRA);
                let pw4 = self.pic.w4;
                for yy in (y0 >> 2)..((y0 + n) >> 2) {
                    self.pic.mvf.fill(yy * pw4 + (x0 >> 2), yy * pw4 + ((x0 + n) >> 2), MvField::default());
                }
            }
            if !self.cu_intra || log2 == sps.log2_min_cb {
                self.part_mode = self.cabac.part_mode(self.cu_intra, log2, sps.log2_min_cb, sps.amp_enabled);
                if self.cu_intra && self.part_mode == PartMode::PNxN && log2 <= sps.log2_min_tb {
                    // NxN would produce blocks smaller than the minimum TB.
                    return Err(Error::Invalid("intra NxN below minimum transform size"));
                }
            }
            if self.cu_intra {
                self.intra_split = self.part_mode == PartMode::PNxN;
                let mut pcm = false;
                if self.part_mode == PartMode::P2Nx2N
                    && sps.pcm_enabled
                    && log2 >= sps.log2_min_pcm_cb
                    && log2 <= sps.log2_max_pcm_cb
                {
                    pcm = self.cabac.pcm_flag();
                }
                if pcm {
                    self.pcm_sample(x0, y0, log2)?;
                    self.trace_cu(x0, y0, 1 << log2, "PCM");
                    self.finish_cu(x0, y0, n);
                    return Ok(());
                }
                self.parse_intra_modes(x0, y0, log2)?;
            } else {
                self.inter_prediction_units(x0, y0, log2)?;
            }
            let rqt_root_cbf = if !self.cu_intra && !(self.part_mode == PartMode::P2Nx2N && self.merge_2nx2n) {
                self.cabac.rqt_root_cbf()
            } else {
                true
            };
            if rqt_root_cbf {
                self.max_trafo_depth = if self.cu_intra {
                    sps.max_transform_hierarchy_depth_intra + self.intra_split as u8
                } else {
                    sps.max_transform_hierarchy_depth_inter
                };
                self.transform_tree(x0, y0, x0, y0, log2, 0, 0, true, true)?;
            }
        }
        self.finish_cu(x0, y0, n);
        Ok(())
    }

    fn trace_cu(&self, x0: usize, y0: usize, n: usize, what: &str) {
        if let Some(p) = crate::debug::opts().trace_cu {
            if p == self.pic.poc {
                eprintln!(
                    "cu ({x0},{y0}) {n} {what} intra={} part={:?} bypass={} qp={} mode={} cmode={}",
                    self.cu_intra,
                    self.part_mode,
                    self.cu_transquant_bypass,
                    self.qp_y,
                    self.st.intra_mode.get(self.idx4(x0, y0)),
                    self.chroma_mode
                );
            }
        }
    }

    fn finish_cu(&mut self, x0: usize, y0: usize, n: usize) {
        self.trace_cu(x0, y0, n, "");
        let w4 = self.st.w4;
        let qp = self.qp_y as i8;
        let x1 = (x0 + n).min(self.sps.pic_width);
        let y1 = (y0 + n).min(self.sps.pic_height);
        for yy in (y0 >> 2)..y1.div_ceil(4) {
            let (a, b) = (yy * w4 + (x0 >> 2), yy * w4 + x1.div_ceil(4));
            self.st.qp_y.fill(a, b, qp);
            // SAFETY: the CU lies in the CTU owned by this decoder.
            for f in unsafe { self.st.flags.slice_mut(a, b) } {
                *f |= F_DECODED;
            }
        }
        if self.cu_transquant_bypass {
            self.or_flags(x0, y0, n, n, F_NOFILTER);
            self.sc.any_nofilter = true;
        }
        self.sc.qp_y_prev = self.qp_y;
        self.first_qg = false;
    }

    fn pcm_sample(&mut self, x0: usize, y0: usize, log2: u8) -> Result<()> {
        let n = 1usize << log2;
        let sps = self.sps;
        self.cabac.pcm_begin()?;
        let bdy = sps.pcm_bit_depth_luma as u32;
        let bdc = sps.pcm_bit_depth_chroma as u32;
        let sy = sps.bit_depth_luma as u32 - bdy;
        let sc = sps.bit_depth_chroma as u32 - bdc;
        for y in 0..n {
            for x in 0..n {
                let v = self.cabac.pcm_bits(bdy)? << sy;
                self.pic.planes[0].set(x0 + x, y0 + y, v as u16);
            }
        }
        if sps.chroma_format_idc != 0 {
            let nc = n / 2;
            for c in 1..3 {
                for y in 0..nc {
                    for x in 0..nc {
                        let v = self.cabac.pcm_bits(bdc)? << sc;
                        self.pic.planes[c].set(x0 / 2 + x, y0 / 2 + y, v as u16);
                    }
                }
            }
        }
        self.cabac.pcm_end();
        let w4 = self.st.w4;
        let mut f = F_INTRA | F_PCM;
        if sps.pcm_loop_filter_disabled {
            f |= F_NOFILTER;
            self.sc.any_nofilter = true;
        }
        Self::fill_map(self.st.flags, w4, x0, y0, n, n, f);
        Self::fill_map(self.st.intra_mode, w4, x0, y0, n, n, 1);
        self.cu_intra = true;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Intra mode syntax and derivation (8.4.2, 8.4.3)
    // -----------------------------------------------------------------------

    fn cand_intra_mode(&self, x_pb: usize, y_pb: usize, xn: isize, yn: isize, x_cb: usize, y_cb: usize, n_cb: usize, is_b: bool) -> u8 {
        if xn < 0 || yn < 0 {
            return 1;
        }
        let (xu, yu) = (xn as usize, yn as usize);
        let inside_cu = xu >= x_cb && xu < x_cb + n_cb && yu >= y_cb && yu < y_cb + n_cb;
        if !inside_cu && !self.avail(xn, yn) {
            return 1;
        }
        let f = self.st.flags.get(self.idx4(xu, yu));
        if f & F_INTRA == 0 || f & F_PCM != 0 {
            return 1;
        }
        if is_b && yn < ((y_pb >> self.sps.log2_ctb) << self.sps.log2_ctb) as isize {
            return 1;
        }
        let _ = x_pb;
        self.st.intra_mode.get(self.idx4(xu, yu))
    }

    fn parse_intra_modes(&mut self, x0: usize, y0: usize, log2: u8) -> Result<()> {
        let n = 1usize << log2;
        let nparts = if self.intra_split { 4 } else { 1 };
        let pb = if self.intra_split { n / 2 } else { n };
        let mut prev_flag = [false; 4];
        for f in prev_flag.iter_mut().take(nparts) {
            *f = self.cabac.prev_intra_luma_pred_flag();
        }
        let w4 = self.st.w4;
        for (j, &pf) in prev_flag.iter().enumerate().take(nparts) {
            let xp = x0 + (j % 2) * pb;
            let yp = y0 + (j / 2) * pb;
            let (mpm_idx, rem) = if pf { (self.cabac.mpm_idx(), 0) } else { (0, self.cabac.rem_intra_luma_pred_mode()) };
            let a = self.cand_intra_mode(xp, yp, xp as isize - 1, yp as isize, x0, y0, n, false);
            let b = self.cand_intra_mode(xp, yp, xp as isize, yp as isize - 1, x0, y0, n, true);
            let mut cand = if a == b {
                if a < 2 {
                    [0u8, 1, 26]
                } else {
                    [a, 2 + ((a + 29) % 32), 2 + ((a - 2 + 1) % 32)]
                }
            } else {
                let c = if a != 0 && b != 0 {
                    0
                } else if a != 1 && b != 1 {
                    1
                } else {
                    26
                };
                [a, b, c]
            };
            let mode = if pf {
                cand[mpm_idx as usize]
            } else {
                cand.sort_unstable();
                let mut m = rem;
                for &c in cand.iter() {
                    if m >= c {
                        m += 1;
                    }
                }
                m
            };
            Self::fill_map(self.st.intra_mode, w4, xp, yp, pb, pb, mode);
        }
        let icpm = self.cabac.intra_chroma_pred_mode();
        let luma0 = self.st.intra_mode.get(self.idx4(x0, y0));
        self.chroma_mode = if icpm == 4 {
            luma0
        } else {
            let m = [0u8, 26, 10, 1][icpm as usize];
            if m == luma0 {
                34
            } else {
                m
            }
        };
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Inter prediction units (7.3.8.6)
    // -----------------------------------------------------------------------

    fn inter_prediction_units(&mut self, x0: usize, y0: usize, log2: u8) -> Result<()> {
        let n = 1usize << log2;
        let h = n / 2;
        let q = n / 4;
        match self.part_mode {
            PartMode::P2Nx2N => self.prediction_unit(x0, y0, n, x0, y0, n, n, 0, false)?,
            PartMode::P2NxN => {
                self.prediction_unit(x0, y0, n, x0, y0, n, h, 0, false)?;
                self.prediction_unit(x0, y0, n, x0, y0 + h, n, h, 1, false)?;
            }
            PartMode::PNx2N => {
                self.prediction_unit(x0, y0, n, x0, y0, h, n, 0, false)?;
                self.prediction_unit(x0, y0, n, x0 + h, y0, h, n, 1, false)?;
            }
            PartMode::P2NxnU => {
                self.prediction_unit(x0, y0, n, x0, y0, n, q, 0, false)?;
                self.prediction_unit(x0, y0, n, x0, y0 + q, n, n * 3 / 4, 1, false)?;
            }
            PartMode::P2NxnD => {
                self.prediction_unit(x0, y0, n, x0, y0, n, n * 3 / 4, 0, false)?;
                self.prediction_unit(x0, y0, n, x0, y0 + n * 3 / 4, n, q, 1, false)?;
            }
            PartMode::PnLx2N => {
                self.prediction_unit(x0, y0, n, x0, y0, q, n, 0, false)?;
                self.prediction_unit(x0, y0, n, x0 + q, y0, n * 3 / 4, n, 1, false)?;
            }
            PartMode::PnRx2N => {
                self.prediction_unit(x0, y0, n, x0, y0, n * 3 / 4, n, 0, false)?;
                self.prediction_unit(x0, y0, n, x0 + n * 3 / 4, y0, q, n, 1, false)?;
            }
            PartMode::PNxN => {
                self.prediction_unit(x0, y0, n, x0, y0, h, h, 0, false)?;
                self.prediction_unit(x0, y0, n, x0 + h, y0, h, h, 1, false)?;
                self.prediction_unit(x0, y0, n, x0, y0 + h, h, h, 2, false)?;
                self.prediction_unit(x0, y0, n, x0 + h, y0 + h, h, h, 3, false)?;
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn prediction_unit(
        &mut self,
        x_cb: usize,
        y_cb: usize,
        n_cb: usize,
        x_pb: usize,
        y_pb: usize,
        w: usize,
        h: usize,
        part_idx: usize,
        skip: bool,
    ) -> Result<()> {
        let merge = skip || self.cabac.merge_flag();
        let mvf = if merge {
            let merge_idx = self.cabac.merge_idx(self.sh.max_num_merge_cand);
            if part_idx == 0 && self.part_mode == PartMode::P2Nx2N {
                self.merge_2nx2n = true;
            }
            self.derive_merge(x_cb, y_cb, n_cb, x_pb, y_pb, w, h, part_idx, merge_idx)
        } else {
            let idc = if self.sh.is_b() {
                self.cabac.inter_pred_idc(w, h, self.ct_depth as usize)
            } else {
                PRED_L0
            };
            let mut ref_idx = [-1i32; 2];
            let mut mvd = [[0i32; 2]; 2];
            let mut mvp = [0usize; 2];
            for l in 0..2 {
                let used = if l == 0 { idc != PRED_L1 } else { idc != PRED_L0 };
                if !used {
                    continue;
                }
                let num = self.sh.num_ref_idx_active[l];
                if num == 0 {
                    return Err(Error::Invalid("prediction from empty reference list"));
                }
                ref_idx[l] = if num > 1 { self.cabac.ref_idx(num) as i32 } else { 0 };
                if l == 1 && self.sh.mvd_l1_zero && idc == PRED_BI {
                    mvd[1] = [0, 0];
                } else {
                    mvd[l] = self.cabac.mvd_coding()?;
                }
                mvp[l] = self.cabac.mvp_flag();
            }
            let mut f = MvField::default();
            for l in 0..2 {
                if ref_idx[l] < 0 {
                    continue;
                }
                let pred = self.derive_amvp(x_cb, y_cb, n_cb, x_pb, y_pb, w, h, part_idx, ref_idx[l] as usize, l, mvp[l]);
                let mx = ((pred.x as i32 + mvd[l][0] + 65536) & 0xffff) as u16 as i16;
                let my = ((pred.y as i32 + mvd[l][1] + 65536) & 0xffff) as u16 as i16;
                f.mv[l] = Mv { x: mx, y: my };
                f.ref_idx[l] = ref_idx[l] as i8;
                f.pred_flag |= 1 << l;
            }
            f
        };
        // store motion
        let w4 = self.pic.w4;
        for yy in (y_pb >> 2)..((y_pb + h) >> 2) {
            self.pic.mvf.fill(yy * w4 + (x_pb >> 2), yy * w4 + ((x_pb + w) >> 2), mvf);
        }
        self.mark_edges(x_pb, y_pb, w, h, EDGE_PU);
        self.predict_inter(x_pb, y_pb, w, h, &mvf)
    }

    // -----------------------------------------------------------------------
    // Transform tree (7.3.8.8 - 7.3.8.10)
    // -----------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    fn transform_tree(
        &mut self,
        x0: usize,
        y0: usize,
        x_base: usize,
        y_base: usize,
        log2: u8,
        depth: u8,
        blk_idx: u8,
        parent_cb: bool,
        parent_cr: bool,
    ) -> Result<()> {
        let sps = self.sps;
        let split = if log2 <= sps.log2_max_tb
            && log2 > sps.log2_min_tb
            && depth < self.max_trafo_depth
            && !(self.intra_split && depth == 0)
        {
            self.cabac.split_transform_flag(log2)
        } else {
            let inter_split = sps.max_transform_hierarchy_depth_inter == 0
                && !self.cu_intra
                && self.part_mode != PartMode::P2Nx2N
                && depth == 0;
            log2 > sps.log2_max_tb || (self.intra_split && depth == 0) || inter_split
        };
        let mut cbf_cb = false;
        let mut cbf_cr = false;
        if log2 > 2 {
            if depth == 0 || parent_cb {
                cbf_cb = self.cabac.cbf_chroma(depth as usize);
            }
            if depth == 0 || parent_cr {
                cbf_cr = self.cabac.cbf_chroma(depth as usize);
            }
        } else {
            // 4x4 luma blocks share the parent's chroma block
            cbf_cb = parent_cb;
            cbf_cr = parent_cr;
        }
        if split {
            let half = 1usize << (log2 - 1);
            self.transform_tree(x0, y0, x0, y0, log2 - 1, depth + 1, 0, cbf_cb, cbf_cr)?;
            self.transform_tree(x0 + half, y0, x0, y0, log2 - 1, depth + 1, 1, cbf_cb, cbf_cr)?;
            self.transform_tree(x0, y0 + half, x0, y0, log2 - 1, depth + 1, 2, cbf_cb, cbf_cr)?;
            self.transform_tree(x0 + half, y0 + half, x0, y0, log2 - 1, depth + 1, 3, cbf_cb, cbf_cr)?;
            Ok(())
        } else {
            let cbf_luma = if self.cu_intra || depth != 0 || cbf_cb || cbf_cr {
                self.cabac.cbf_luma(depth as usize)
            } else {
                true
            };
            self.transform_unit(x0, y0, x_base, y_base, log2, blk_idx, cbf_luma, cbf_cb, cbf_cr)
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn transform_unit(
        &mut self,
        x0: usize,
        y0: usize,
        x_base: usize,
        y_base: usize,
        log2: u8,
        blk_idx: u8,
        cbf_luma: bool,
        cbf_cb: bool,
        cbf_cr: bool,
    ) -> Result<()> {
        let n = 1usize << log2;
        if (cbf_luma || cbf_cb || cbf_cr) && self.pps.cu_qp_delta_enabled && !self.is_cu_qp_delta_coded {
            let d = self.cabac.cu_qp_delta()?;
            let off = self.sps.qp_bd_offset_y();
            if d < -(26 + off / 2) || d > 25 + off / 2 {
                return Err(Error::Invalid("CuQpDeltaVal out of range"));
            }
            self.cu_qp_delta_val = d;
            self.is_cu_qp_delta_coded = true;
            self.update_qp();
        }
        // Luma
        if self.cu_intra {
            let mode = self.st.intra_mode.get(self.idx4(x0, y0));
            self.predict_intra(x0, y0, log2 as u32, 0, mode);
        }
        if cbf_luma {
            self.residual(x0, y0, log2 as u32, 0)?;
            self.or_flags(x0, y0, n, n, F_NZ);
        }
        self.mark_edges(x0, y0, n, n, EDGE_TU);
        if self.cu_intra {
            // mark decoded so later TUs of this CU can use these samples
            self.or_flags(x0, y0, n, n, F_DECODED);
        }
        // Chroma (4:2:0)
        if log2 > 2 {
            let (xc, yc) = (x0 / 2, y0 / 2);
            let l2c = log2 as u32 - 1;
            for (c, cbf) in [(1usize, cbf_cb), (2, cbf_cr)] {
                if self.cu_intra {
                    self.predict_intra(xc, yc, l2c, c, self.chroma_mode);
                }
                if cbf {
                    self.residual(xc, yc, l2c, c)?;
                }
            }
        } else if blk_idx == 3 {
            let (xc, yc) = (x_base / 2, y_base / 2);
            for (c, cbf) in [(1usize, cbf_cb), (2, cbf_cr)] {
                if self.cu_intra {
                    self.predict_intra(xc, yc, 2, c, self.chroma_mode);
                }
                if cbf {
                    self.residual(xc, yc, 2, c)?;
                }
            }
        }
        Ok(())
    }

    /// Parses residual_coding() for one TB and adds the reconstructed residual
    /// to the prediction already present in the picture.
    fn residual(&mut self, x0: usize, y0: usize, log2: u32, c_idx: usize) -> Result<()> {
        let n = 1usize << log2;
        let scan_idx = if self.cu_intra && (log2 == 2 || (log2 == 3 && c_idx == 0)) {
            let mode = if c_idx == 0 {
                self.st.intra_mode.get(self.idx4(x0, y0))
            } else {
                self.chroma_mode
            };
            if (6..=14).contains(&mode) {
                2
            } else if (22..=30).contains(&mode) {
                1
            } else {
                0
            }
        } else {
            0
        };
        let bypass = self.cu_transquant_bypass;
        let ts_allowed = self.pps.transform_skip_enabled && !bypass && log2 == 2;
        let sign_hiding = self.pps.sign_data_hiding_enabled && !bypass;
        let mut coeffs = std::mem::take(&mut self.coeffs);
        let mut res = std::mem::take(&mut self.res);
        let mut nz = std::mem::take(&mut self.nz_pos);
        nz.clear();
        let r = (|| -> Result<()> {
            let info = self.cabac.residual_coding(&mut coeffs, &mut nz, log2, c_idx, scan_idx, ts_allowed, sign_hiding)?;
            let bit_depth = if c_idx == 0 { self.sps.bit_depth_luma } else { self.sps.bit_depth_chroma } as u32;
            if bypass {
                res[..n * n].copy_from_slice(&coeffs[..n * n]);
            } else {
                let qp = if c_idx == 0 {
                    self.qp_y + self.sps.qp_bd_offset_y()
                } else {
                    let off = if c_idx == 1 {
                        self.pps.cb_qp_offset + self.sh.cb_qp_offset
                    } else {
                        self.pps.cr_qp_offset + self.sh.cr_qp_offset
                    };
                    let qpi = (self.qp_y + off).clamp(-self.sps.qp_bd_offset_c(), 57);
                    chroma_qp_table(qpi) + self.sps.qp_bd_offset_c()
                };
                let scale = self.scaling.map(|f| {
                    let matrix_id = c_idx + if self.cu_intra { 0 } else { 3 };
                    &f.f[log2 as usize - 2][matrix_id][..]
                });
                transform::dequantize_positions(&mut coeffs, &nz, log2, qp, bit_depth, scale);
                if info.transform_skip {
                    transform::transform_skip(&coeffs, &mut res, log2, bit_depth);
                } else {
                    let dst = self.cu_intra && c_idx == 0 && log2 == 2;
                    transform::inverse_transform_bbox(
                        &coeffs,
                        &mut res,
                        &mut self.tr_tmp,
                        log2,
                        bit_depth,
                        dst,
                        info.max_x,
                        info.max_y,
                    );
                }
            }
            let plane = self.pic.planes[c_idx];
            let maxv = (1i32 << bit_depth) - 1;
            let pw = plane.width;
            let ph = plane.height;
            let wn = n.min(pw.saturating_sub(x0));
            for y in 0..n.min(ph.saturating_sub(y0)) {
                // SAFETY: the transform block lies in the CTU owned by this decoder.
                let dst = unsafe { plane.row_mut(x0, y0 + y, wn) };
                add_residual_row(dst, &res[y * n..y * n + wn], maxv);
            }
            Ok(())
        })();
        // restore the all-zero invariant of the coefficient buffer
        for &p in &nz {
            coeffs[p as usize] = 0;
        }
        self.coeffs = coeffs;
        self.res = res;
        self.nz_pos = nz;
        r
    }
}

/// `dst = clip(dst + res)` for one row (vectorisable leaf).
#[inline(never)]
fn add_residual_row(dst: &mut [u16], res: &[i32], maxv: i32) {
    for (p, &r) in dst.iter_mut().zip(res) {
        *p = (*p as i32 + r).max(0).min(maxv) as u16;
    }
}

// Silence unused warnings for constants used only in some configurations.
const _: u8 = PRED_BI;
