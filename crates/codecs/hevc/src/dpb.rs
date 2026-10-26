// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Decoded picture buffer: picture order count (8.3.1), reference picture set
//! decoding and marking (8.3.2), generation of unavailable pictures (8.3.3),
//! reference picture list construction (8.3.4) and output / bumping (C.5.2).

use crate::error::{Error, Result};
use crate::frame::Frame;
use crate::picture::{Picture, RefMark};
use crate::slice::SliceHeader;

/// Reference picture set of the current picture as DPB indices.
#[derive(Debug, Default, Clone)]
pub struct RpsSets {
    pub st_curr_before: Vec<usize>,
    pub st_curr_after: Vec<usize>,
    pub lt_curr: Vec<usize>,
}

/// PicOrderCntVal derivation (8.3.1).
pub fn compute_poc(poc_lsb: i32, log2_max_lsb: u8, prev_tid0_poc: i32, irap_no_rasl: bool) -> i32 {
    let max = 1i32 << log2_max_lsb;
    let msb = if irap_no_rasl {
        0
    } else {
        let prev_lsb = prev_tid0_poc & (max - 1);
        let prev_msb = prev_tid0_poc - prev_lsb;
        if poc_lsb < prev_lsb && prev_lsb - poc_lsb >= max / 2 {
            prev_msb + max
        } else if poc_lsb > prev_lsb && poc_lsb - prev_lsb > max / 2 {
            prev_msb - max
        } else {
            prev_msb
        }
    };
    msb + poc_lsb
}

/// Parameters for generating missing reference pictures.
pub struct GenParams {
    pub width: usize,
    pub height: usize,
    pub bit_depth_luma: u8,
    pub bit_depth_chroma: u8,
    pub log2_ctb: u8,
    pub crop: [usize; 4],
}

#[derive(Default)]
pub struct Dpb {
    pub pics: Vec<Picture>,
    /// Released pictures kept for buffer reuse.
    pub pool: Vec<Picture>,
}

const POOL_MAX: usize = 4;

impl Dpb {
    fn release(&mut self, p: Picture) {
        if self.pool.len() < POOL_MAX && !p.corrupt {
            self.pool.push(p);
        }
    }

    /// Removes all pictures (keeping their buffers for reuse).
    pub fn clear(&mut self) {
        while let Some(p) = self.pics.pop() {
            self.release(p);
        }
    }

    /// Returns a picture buffer with the given geometry (reused when possible).
    /// Sample planes of reused pictures are not cleared.
    pub fn take_picture(&mut self, g: &GenParams) -> Picture {
        if let Some(i) = self.pool.iter().position(|p| {
            p.width() == g.width
                && p.height() == g.height
                && p.bit_depth_luma == g.bit_depth_luma
                && p.bit_depth_chroma == g.bit_depth_chroma
                && p.log2_ctb == g.log2_ctb
        }) {
            let mut p = self.pool.swap_remove(i);
            p.reset(g.crop);
            return p;
        }
        Picture::new(g.width, g.height, g.bit_depth_luma, g.bit_depth_chroma, g.log2_ctb, g.crop)
    }

    pub fn num_needed_for_output(&self) -> usize {
        self.pics.iter().filter(|p| p.needed_for_output).count()
    }

    /// Removes pictures that are neither needed for output nor used for reference.
    pub fn remove_unused(&mut self) {
        let mut i = 0;
        while i < self.pics.len() {
            if !self.pics[i].needed_for_output && self.pics[i].mark == RefMark::Unused {
                let p = self.pics.swap_remove(i);
                self.release(p);
            } else {
                i += 1;
            }
        }
    }

    /// Outputs the picture with the smallest POC that is needed for output ("bumping").
    pub fn bump(&mut self) -> Option<Frame> {
        let mut best: Option<usize> = None;
        for (i, p) in self.pics.iter().enumerate() {
            if p.needed_for_output && best.is_none_or(|b| p.poc < self.pics[b].poc) {
                best = Some(i);
            }
        }
        let i = best?;
        let frame = Frame::from_picture(&self.pics[i]);
        self.pics[i].needed_for_output = false;
        if self.pics[i].mark == RefMark::Unused {
            let p = self.pics.swap_remove(i);
            self.release(p);
        }
        Some(frame)
    }

    /// Outputs all pictures in POC order and empties the DPB.
    pub fn flush(&mut self, out: &mut Vec<Frame>) {
        while let Some(f) = self.bump() {
            out.push(f);
        }
        self.clear();
    }

    /// Applies the RPS of the current picture (8.3.2) and returns the current
    /// reference sets. Missing reference pictures are generated (8.3.3).
    pub fn apply_rps(
        &mut self,
        sh: &SliceHeader,
        cur_poc: i32,
        log2_max_lsb: u8,
        irap_no_rasl: bool,
        is_idr: bool,
        genp: &GenParams,
    ) -> RpsSets {
        if irap_no_rasl {
            for p in self.pics.iter_mut() {
                p.mark = RefMark::Unused;
            }
        }
        let mut sets = RpsSets::default();
        if is_idr {
            for p in self.pics.iter_mut() {
                p.mark = RefMark::Unused;
            }
            return sets;
        }
        let max_lsb = 1i32 << log2_max_lsb;
        let rps = &sh.st_rps;
        let mut keep = vec![false; self.pics.len()];
        // Long-term pictures first
        let mut lt_curr_pocs: Vec<(i32, bool)> = Vec::new();
        let mut lt_foll_pocs: Vec<(i32, bool)> = Vec::new();
        for i in 0..sh.num_long_term {
            let mut poc = sh.lt_poc_lsb[i];
            let msb = sh.lt_delta_poc_msb_present[i];
            if msb {
                poc += cur_poc - sh.lt_delta_poc_msb_cycle[i] * max_lsb - (cur_poc & (max_lsb - 1));
            }
            if sh.lt_used_by_curr[i] {
                lt_curr_pocs.push((poc, msb));
            } else {
                lt_foll_pocs.push((poc, msb));
            }
        }
        let find_lt = |pics: &[Picture], poc: i32, msb: bool| -> Option<usize> {
            pics.iter().position(|p| {
                p.mark != RefMark::Unused
                    && if msb { p.poc == poc } else { (p.poc & (max_lsb - 1)) == poc }
            })
        };
        let mut lt_curr_idx: Vec<Option<usize>> = Vec::new();
        for &(poc, msb) in &lt_curr_pocs {
            let f = find_lt(&self.pics, poc, msb);
            if let Some(i) = f {
                keep[i] = true;
            }
            lt_curr_idx.push(f);
        }
        for &(poc, msb) in &lt_foll_pocs {
            if let Some(i) = find_lt(&self.pics, poc, msb) {
                keep[i] = true;
            }
        }
        for (i, k) in keep.iter().enumerate() {
            if *k {
                self.pics[i].mark = RefMark::LongTerm;
            }
        }
        // Short-term
        let find_st = |pics: &[Picture], keep: &[bool], poc: i32| -> Option<usize> {
            pics.iter()
                .enumerate()
                .position(|(i, p)| p.mark == RefMark::ShortTerm && !keep[i] && p.poc == poc)
        };
        let mut before: Vec<Option<usize>> = Vec::new();
        let mut after: Vec<Option<usize>> = Vec::new();
        let mut st_keep = vec![false; self.pics.len()];
        for i in 0..rps.num_negative {
            let poc = cur_poc + rps.delta_poc_s0[i];
            let f = find_st(&self.pics, &keep, poc);
            if let Some(j) = f {
                st_keep[j] = true;
            }
            if rps.used_s0[i] {
                before.push(f);
            }
        }
        let before_pocs: Vec<i32> =
            (0..rps.num_negative).filter(|&i| rps.used_s0[i]).map(|i| cur_poc + rps.delta_poc_s0[i]).collect();
        for i in 0..rps.num_positive {
            let poc = cur_poc + rps.delta_poc_s1[i];
            let f = find_st(&self.pics, &keep, poc);
            if let Some(j) = f {
                st_keep[j] = true;
            }
            if rps.used_s1[i] {
                after.push(f);
            }
        }
        let after_pocs: Vec<i32> =
            (0..rps.num_positive).filter(|&i| rps.used_s1[i]).map(|i| cur_poc + rps.delta_poc_s1[i]).collect();
        // Mark everything else unused
        for (i, p) in self.pics.iter_mut().enumerate() {
            if !keep[i] && !st_keep[i] {
                p.mark = RefMark::Unused;
            }
        }
        // Generate missing pictures for the "Curr" sets.
        let gen_pic = |pics: &mut Vec<Picture>, poc: i32, lt: bool| -> usize {
            let mut p = Picture::new(genp.width, genp.height, genp.bit_depth_luma, genp.bit_depth_chroma, genp.log2_ctb, genp.crop);
            p.poc = poc;
            p.mark = if lt { RefMark::LongTerm } else { RefMark::ShortTerm };
            p.needed_for_output = false;
            p.corrupt = true;
            pics.push(p);
            pics.len() - 1
        };
        for (k, f) in before.into_iter().enumerate() {
            let idx = match f {
                Some(i) => i,
                None => gen_pic(&mut self.pics, before_pocs[k], false),
            };
            sets.st_curr_before.push(idx);
        }
        for (k, f) in after.into_iter().enumerate() {
            let idx = match f {
                Some(i) => i,
                None => gen_pic(&mut self.pics, after_pocs[k], false),
            };
            sets.st_curr_after.push(idx);
        }
        for (k, f) in lt_curr_idx.into_iter().enumerate() {
            let idx = match f {
                Some(i) => i,
                None => {
                    let (poc, _) = lt_curr_pocs[k];
                    gen_pic(&mut self.pics, poc, true)
                }
            };
            sets.lt_curr.push(idx);
        }
        sets
    }

    /// Reference picture list construction (8.3.4). Returns DPB indices.
    pub fn ref_lists(&self, sets: &RpsSets, sh: &SliceHeader) -> Result<[[usize; 16]; 2]> {
        let mut lists = [[usize::MAX; 16]; 2];
        if sh.is_intra() {
            return Ok(lists);
        }
        let total = sets.st_curr_before.len() + sets.st_curr_after.len() + sets.lt_curr.len();
        if total == 0 {
            return Err(Error::Invalid("no reference pictures for inter slice"));
        }
        let nlists = if sh.is_b() { 2 } else { 1 };
        for l in 0..nlists {
            let n_active = sh.num_ref_idx_active[l];
            let n_temp = n_active.max(total);
            let mut temp: Vec<usize> = Vec::with_capacity(n_temp);
            let (first, second) = if l == 0 {
                (&sets.st_curr_before, &sets.st_curr_after)
            } else {
                (&sets.st_curr_after, &sets.st_curr_before)
            };
            while temp.len() < n_temp {
                for &i in first.iter().chain(second.iter()).chain(sets.lt_curr.iter()) {
                    if temp.len() < n_temp {
                        temp.push(i);
                    }
                }
            }
            for i in 0..n_active {
                let e = if sh.rpl_modification[l] { sh.list_entry[l][i] as usize } else { i };
                lists[l][i] = *temp.get(e).ok_or(Error::Invalid("list_entry out of range"))?;
            }
        }
        Ok(lists)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poc_wrap() {
        // log2 max lsb 4 => max 16
        assert_eq!(compute_poc(2, 4, 14, false), 18);
        assert_eq!(compute_poc(14, 4, 18, false), 14);
        assert_eq!(compute_poc(5, 4, 3, false), 5);
        assert_eq!(compute_poc(7, 4, 100, true), 7);
    }
}
