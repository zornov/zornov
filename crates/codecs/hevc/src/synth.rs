// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Random *valid* HEVC bitstream synthesis (feature `synth`, development only).
//!
//! Parameter sets and slice headers are written with random field values;
//! slice data is produced by running the decoder in synthesis mode, where each
//! bin is drawn pseudo-randomly and encoded with the reference CABAC encoder.
//! The resulting Annex B streams exercise tools that common encoders never
//! emit (tiles, PCM, dependent slices, long-term pictures, list modification,
//! custom scaling lists, ...) and are cross-checked against other decoders.

use crate::bitreader::nal_to_rbsp;
use crate::decoder::Decoder;
use crate::error::{Error, Result};
use crate::frame::Frame;
use crate::nal::NalHeader;
use crate::params::{parse_pps, parse_sps, Pps, Sps, StRps, TileLayout};

// ---------------------------------------------------------------------------
// Bit writing
// ---------------------------------------------------------------------------

#[derive(Default, Clone)]
pub struct BitWriter {
    bits: Vec<u8>,
}

impl BitWriter {
    pub fn u(&mut self, v: u64, n: u32) {
        for i in (0..n).rev() {
            self.bits.push(((v >> i) & 1) as u8);
        }
    }
    pub fn flag(&mut self, b: bool) {
        self.bits.push(b as u8);
    }
    pub fn ue(&mut self, v: u32) {
        let v1 = v as u64 + 1;
        let len = 64 - v1.leading_zeros();
        self.u(0, len - 1);
        self.u(v1, len);
    }
    pub fn se(&mut self, v: i32) {
        let k = if v > 0 { 2 * v as i64 - 1 } else { -2 * v as i64 };
        self.ue(k as u32);
    }
    pub fn trailing(&mut self) {
        self.bits.push(1);
        while self.bits.len() % 8 != 0 {
            self.bits.push(0);
        }
    }
    pub fn len(&self) -> usize {
        self.bits.len()
    }
    pub fn is_empty(&self) -> bool {
        self.bits.is_empty()
    }
    pub fn bytes(&self) -> Vec<u8> {
        let mut out = vec![0u8; self.bits.len().div_ceil(8)];
        for (i, &b) in self.bits.iter().enumerate() {
            out[i / 8] |= b << (7 - (i % 8));
        }
        out
    }
}

/// Inserts emulation prevention bytes.
pub fn escape(rbsp: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rbsp.len() + 16);
    let mut zeros = 0;
    for &b in rbsp {
        if zeros >= 2 && b <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(b);
        if b == 0 {
            zeros += 1;
        } else {
            zeros = 0;
        }
    }
    out
}

fn nal_header(w: &mut BitWriter, nal_type: u8, tid: u8) {
    w.u(0, 1);
    w.u(nal_type as u64, 6);
    w.u(0, 6);
    w.u(tid as u64 + 1, 3);
}

fn annexb(out: &mut Vec<u8>, nal: &[u8]) {
    out.extend_from_slice(&[0, 0, 0, 1]);
    out.extend_from_slice(nal);
}

// ---------------------------------------------------------------------------
// Random numbers
// ---------------------------------------------------------------------------

pub struct Rng(u64, Vec<String>, bool);

impl Rng {
    /// Features can be disabled with `STUDIO_SYNTH_OFF=name,name` (`all` disables
    /// every optional feature; `STUDIO_SYNTH_ON=name,..` then re-enables some).
    pub fn new(seed: u64) -> Rng {
        let off: Vec<String> = std::env::var("STUDIO_SYNTH_OFF")
            .unwrap_or_default()
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();
        let all = off.iter().any(|s| s == "all");
        let mut list = off;
        if all {
            list = std::env::var("STUDIO_SYNTH_ON")
                .unwrap_or_default()
                .split(',')
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect();
        }
        Rng(seed.wrapping_mul(0x2545F4914F6CDD1D).wrapping_add(0x9E3779B97F4A7C15) | 1, list, all)
    }
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    /// Random decision for an optional feature (respecting the env switches).
    /// The random number is always drawn so that streams stay comparable.
    pub fn feat(&mut self, name: &str, p: f64) -> bool {
        let v = self.chance(p);
        let listed = self.1.iter().any(|s| s == name);
        let enabled = if self.2 { listed } else { !listed };
        v && enabled
    }
    /// Uniform in lo..=hi.
    pub fn range(&mut self, lo: i64, hi: i64) -> i64 {
        if hi <= lo {
            return lo;
        }
        lo + (self.next() % ((hi - lo + 1) as u64)) as i64
    }
    pub fn chance(&mut self, p: f64) -> bool {
        (self.next() % 1_000_000) as f64 / 1_000_000.0 < p
    }
}

// ---------------------------------------------------------------------------
// Parameter set writers
// ---------------------------------------------------------------------------

fn write_ptl(w: &mut BitWriter, profile: u8) {
    w.u(0, 2);
    w.flag(false);
    w.u(profile as u64, 5);
    let mut compat = 0u32;
    compat |= 1 << (31 - profile);
    if profile == 1 {
        compat |= 1 << (31 - 2);
    }
    w.u(compat as u64, 32);
    w.flag(true); // progressive
    w.flag(false);
    w.flag(false);
    w.flag(true); // frame only
    w.u(0, 43);
    w.u(0, 1);
    w.u(153, 8); // level 5.1
}

fn write_vps(profile: u8, max_dec: u32, reorder: u32) -> Vec<u8> {
    let mut w = BitWriter::default();
    nal_header(&mut w, 32, 0);
    w.u(0, 4);
    w.flag(true);
    w.flag(true);
    w.u(0, 6);
    w.u(0, 3);
    w.flag(true);
    w.u(0xffff, 16);
    write_ptl(&mut w, profile);
    w.flag(true);
    w.ue(max_dec - 1);
    w.ue(reorder);
    w.ue(0);
    w.u(0, 6);
    w.ue(0);
    w.flag(false);
    w.flag(false);
    w.trailing();
    escape(&w.bytes())
}

/// Syntax-level description of a short-term RPS.
#[derive(Clone, Debug)]
enum RpsSyntax {
    Explicit { neg: Vec<(i32, bool)>, pos: Vec<(i32, bool)> },
    Inter { delta_idx_minus1: u32, delta_rps: i32, flags: Vec<(bool, bool)> },
}

fn write_rps(w: &mut BitWriter, idx: usize, num_sets: usize, r: &RpsSyntax) {
    match r {
        RpsSyntax::Explicit { neg, pos } => {
            if idx != 0 {
                w.flag(false);
            }
            w.ue(neg.len() as u32);
            w.ue(pos.len() as u32);
            let mut prev = 0;
            for &(d, used) in neg {
                w.ue((prev - d - 1) as u32);
                w.flag(used);
                prev = d;
            }
            prev = 0;
            for &(d, used) in pos {
                w.ue((d - prev - 1) as u32);
                w.flag(used);
                prev = d;
            }
        }
        RpsSyntax::Inter { delta_idx_minus1, delta_rps, flags } => {
            w.flag(true);
            if idx == num_sets {
                w.ue(*delta_idx_minus1);
            }
            w.flag(*delta_rps < 0);
            w.ue(delta_rps.unsigned_abs() - 1);
            for &(used, use_delta) in flags {
                w.flag(used);
                if !used {
                    w.flag(use_delta);
                }
            }
        }
    }
}

fn write_scaling_list(w: &mut BitWriter, rng: &mut Rng) {
    for size_id in 0..4usize {
        let step = if size_id == 3 { 3 } else { 1 };
        let mut m = 0usize;
        while m < 6 {
            let mode = rng.range(0, 3);
            if mode == 0 && m >= step {
                // predict from an earlier matrix (or default when delta = 0)
                w.flag(false);
                w.ue(rng.range(0, (m / step) as i64) as u32);
            } else if mode == 1 {
                w.flag(false);
                w.ue(0); // default list
            } else {
                w.flag(true);
                let coef_num = 64.min(1 << (4 + (size_id << 1)));
                let mut next = 8i32;
                if size_id > 1 {
                    let dc = rng.range(-7, 60) as i32;
                    w.se(dc);
                    next = dc + 8;
                }
                for _ in 0..coef_num {
                    // keep coefficients in 4..=80
                    let target = rng.range(4, 80) as i32;
                    let mut delta = target - next;
                    if delta > 127 {
                        delta -= 256;
                    }
                    if delta < -128 {
                        delta += 256;
                    }
                    w.se(delta);
                    next = (next + delta + 256) % 256;
                }
            }
            m += step;
        }
    }
}

struct SeqPlan {
    sps_rbsp: Vec<u8>,
    sps: Sps,
    profile: u8,
}

fn random_sps(rng: &mut Rng, max_dec: u32, reorder: u32) -> Result<SeqPlan> {
    let mut w = BitWriter::default();
    nal_header(&mut w, 33, 0);
    let bit_depth: u32 = if rng.feat("bit10", 0.35) { 10 } else { 8 };
    let bit_depth_c: u32 = if std::env::var_os("STUDIO_SYNTH_MIXED_DEPTH").is_some() && rng.chance(0.8) { 18 - bit_depth } else { bit_depth };
    let profile = if bit_depth == 8 && bit_depth_c == 8 { 1 } else { 2 };
    w.u(0, 4);
    w.u(0, 3);
    w.flag(true);
    write_ptl(&mut w, profile);
    w.ue(0); // sps id
    w.ue(1); // 4:2:0
    let log2_ctb = rng.range(4, 6) as u32;
    let log2_min_cb = rng.range(3, log2_ctb as i64) as u32;
    let min_cb = 1u32 << log2_min_cb;
    let ctb = 1u32 << log2_ctb;
    // picture of 1..=4 CTB columns and 1..=4 CTB rows (partial CTBs allowed)
    let w_cbs = rng.range(1, (4 * ctb / min_cb).min(208 / min_cb) as i64) as u32;
    let h_cbs = rng.range(1, (4 * ctb / min_cb).min(160 / min_cb) as i64) as u32;
    let width = (w_cbs * min_cb).max(8);
    let height = (h_cbs * min_cb).max(8);
    let width = width - width % min_cb;
    let height = height - height % min_cb;
    w.ue(width);
    w.ue(height);
    if rng.feat("crop", 0.3) && width >= 16 && height >= 16 {
        w.flag(true);
        w.ue(rng.range(0, 2) as u32);
        w.ue(rng.range(0, 2) as u32);
        w.ue(rng.range(0, 2) as u32);
        w.ue(rng.range(0, 2) as u32);
    } else {
        w.flag(false);
    }
    w.ue(bit_depth - 8);
    w.ue(bit_depth_c - 8);
    let log2_poc = rng.range(4, 8) as u32;
    w.ue(log2_poc - 4);
    w.flag(true);
    w.ue(max_dec - 1);
    w.ue(reorder);
    w.ue(0);
    w.ue(log2_min_cb - 3);
    w.ue(log2_ctb - log2_min_cb);
    let log2_min_tb = rng.range(2, (log2_min_cb - 1).min(4) as i64) as u32;
    let log2_max_tb = rng.range(log2_min_tb as i64, log2_ctb.min(5) as i64) as u32;
    w.ue(log2_min_tb - 2);
    w.ue(log2_max_tb - log2_min_tb);
    let max_depth = (log2_ctb - log2_min_tb) as i64;
    w.ue(rng.range(0, max_depth.min(4)) as u32);
    w.ue(rng.range(0, max_depth.min(4)) as u32);
    let scaling = rng.feat("scaling", 0.4);
    w.flag(scaling);
    if scaling {
        let data = rng.feat("scaling_data", 0.6);
        w.flag(data);
        if data {
            write_scaling_list(&mut w, rng);
        }
    }
    w.flag(rng.feat("amp", 0.5)); // amp
    w.flag(rng.feat("sao", 0.7)); // sao
    let pcm = rng.feat("pcm", 0.5) && log2_min_cb <= 5;
    w.flag(pcm);
    if pcm {
        w.u(rng.range(1, bit_depth as i64) as u64 - 1, 4);
        w.u(rng.range(1, bit_depth_c as i64) as u64 - 1, 4);
        let max_pcm = log2_ctb.min(5);
        let lo = rng.range(log2_min_cb.max(3) as i64, max_pcm as i64) as u32;
        let hi = rng.range(lo as i64, max_pcm as i64) as u32;
        w.ue(lo - 3);
        w.ue(hi - lo);
        w.flag(rng.feat("pcm_lf", 0.5));
    }
    // short-term RPS sets
    let nsets = rng.range(0, 4) as usize;
    let mut syn: Vec<RpsSyntax> = Vec::new();
    let mut derived: Vec<StRps> = Vec::new();
    w.ue(nsets as u32);
    for i in 0..nsets {
        let r = if i > 0 && rng.feat("rps_inter", 0.5) {
            let refr = &derived[i - 1];
            let n = refr.num_delta_pocs();
            let d = rng.range(1, 3) as i32 * if rng.chance(0.7) { -1 } else { 1 };
            // Never select dPoc == 0 (the current picture itself): HM and
            // ffmpeg would count it as an entry, the specification does not.
            let flags = (0..=n)
                .map(|j| {
                    let rd = if j < refr.num_negative {
                        refr.delta_poc_s0[j]
                    } else if j < n {
                        refr.delta_poc_s1[j - refr.num_negative]
                    } else {
                        0
                    };
                    if rd + d == 0 { (false, false) } else { (rng.chance(0.6), rng.chance(0.5)) }
                })
                .collect();
            RpsSyntax::Inter { delta_idx_minus1: 0, delta_rps: d, flags }
        } else {
            let nn = rng.range(0, 3) as usize;
            let np = rng.range(0, 1) as usize;
            let mut neg = Vec::new();
            let mut d = 0;
            for _ in 0..nn {
                d -= rng.range(1, 3) as i32;
                neg.push((d, rng.chance(0.7)));
            }
            let mut pos = Vec::new();
            d = 0;
            for _ in 0..np {
                d += rng.range(1, 3) as i32;
                pos.push((d, rng.chance(0.7)));
            }
            RpsSyntax::Explicit { neg, pos }
        };
        // derive by parsing a temporary encoding
        let derive = |r: &RpsSyntax, derived: &[StRps]| -> Result<StRps> {
            let mut tw = BitWriter::default();
            write_rps(&mut tw, i, nsets, r);
            tw.trailing();
            let bytes = tw.bytes();
            let mut br = crate::bitreader::BitReader::new(&bytes);
            crate::params::parse_st_rps(&mut br, i, nsets, derived, max_dec as usize)
        };
        let mut r = r;
        let mut d = derive(&r, &derived);
        if d.as_ref().map(|d| d.num_delta_pocs() as u32 >= max_dec).unwrap_or(true) {
            r = RpsSyntax::Explicit { neg: vec![(-1, true)], pos: vec![] };
            d = derive(&r, &derived);
        }
        let d = d?;
        write_rps(&mut w, i, nsets, &r);
        derived.push(d);
        syn.push(r);
    }
    let lt = rng.feat("lt", 0.5);
    w.flag(lt);
    if lt {
        let n = rng.range(0, 3);
        w.ue(n as u32);
        for _ in 0..n {
            w.u(rng.range(0, (1 << log2_poc) - 1) as u64, log2_poc);
            w.flag(rng.chance(0.6));
        }
    }
    w.flag(rng.feat("tmvp", 0.7)); // temporal mvp
    w.flag(rng.feat("strong", 0.5)); // strong intra smoothing
    let vui = rng.feat("vui", 0.3);
    w.flag(vui);
    if vui {
        w.flag(false); // aspect
        w.flag(false); // overscan
        w.flag(true); // video signal type
        w.u(5, 3);
        w.flag(rng.chance(0.5));
        w.flag(true);
        w.u(1, 8);
        w.u(1, 8);
        w.u(1, 8);
        w.flag(false); // chroma loc
        w.flag(false);
        w.flag(false);
        w.flag(false);
        w.flag(false); // default display window
        w.flag(false); // timing
        w.flag(false); // bitstream restriction
    }
    w.flag(false); // extensions
    w.trailing();
    let nal = escape(&w.bytes());
    let rbsp = nal_to_rbsp(&nal);
    let sps = parse_sps(&rbsp)?;
    let _ = syn;
    Ok(SeqPlan { sps_rbsp: nal, sps, profile })
}

fn random_pps(rng: &mut Rng, id: u32, sps: &Sps) -> Result<(Vec<u8>, Pps)> {
    let mut w = BitWriter::default();
    nal_header(&mut w, 34, 0);
    w.ue(id);
    w.ue(0);
    w.flag(rng.feat("dep_slices", 0.5)); // dependent slices
    w.flag(rng.feat("out_flag", 0.3)); // output flag present
    w.u(if rng.feat("extra_bits", 0.6) { rng.range(1, 2) as u64 } else { 0 }, 3); // extra slice header bits
    w.flag(rng.feat("signhide", 0.5)); // sign data hiding
    w.flag(rng.feat("cabac_init", 0.5)); // cabac init present
    w.ue(rng.range(0, 3) as u32);
    w.ue(rng.range(0, 3) as u32);
    let off = 6 * (sps.bit_depth_luma as i64 - 8);
    w.se(rng.range(-6 - off.min(6), 10) as i32); // init_qp_minus26
    w.flag(rng.feat("cip", 0.3)); // constrained intra
    w.flag(rng.feat("tskip", 0.5)); // transform skip
    let dqp = rng.feat("dqp", 0.6);
    w.flag(dqp);
    if dqp {
        w.ue(rng.range(0, (sps.log2_ctb - sps.log2_min_cb) as i64) as u32);
    }
    w.se(rng.range(-6, 6) as i32);
    w.se(rng.range(-6, 6) as i32);
    w.flag(rng.feat("slice_chroma", 0.5)); // slice chroma qp offsets present
    w.flag(rng.feat("wp", 0.4)); // weighted pred
    w.flag(rng.feat("wbp", 0.4)); // weighted bipred
    w.flag(rng.feat("bypass", 0.3)); // transquant bypass
    let tiles = rng.feat("tiles", 0.5) && (sps.pic_width_in_ctbs > 1 || sps.pic_height_in_ctbs > 1);
    w.flag(tiles);
    // Tiles and WPP together are not supported by the reference decoders used
    // for cross-checking; only combine them on request.
    let wpp = rng.feat("wpp", 0.5) && (!tiles || std::env::var_os("STUDIO_SYNTH_TILES_WPP").is_some());
    w.flag(wpp); // entropy coding sync
    if tiles {
        let cols = rng.range(1, sps.pic_width_in_ctbs.min(4) as i64) as usize;
        let rows = rng.range(1, sps.pic_height_in_ctbs.min(4) as i64) as usize;
        w.ue(cols as u32 - 1);
        w.ue(rows as u32 - 1);
        let uniform = rng.chance(0.5);
        w.flag(uniform);
        if !uniform {
            let mut left = sps.pic_width_in_ctbs;
            for i in 0..cols - 1 {
                let remaining = cols - 1 - i;
                let wdt = rng.range(1, (left - remaining) as i64) as usize;
                w.ue(wdt as u32 - 1);
                left -= wdt;
            }
            let mut left = sps.pic_height_in_ctbs;
            for j in 0..rows - 1 {
                let remaining = rows - 1 - j;
                let h = rng.range(1, (left - remaining) as i64) as usize;
                w.ue(h as u32 - 1);
                left -= h;
            }
        }
        w.flag(!rng.feat("lf_tiles_off", 0.5)); // loop filter across tiles
    }
    w.flag(!rng.feat("lf_slices_off", 0.5)); // loop filter across slices
    let dbc = rng.feat("dbctrl", 0.6);
    w.flag(dbc);
    if dbc {
        w.flag(rng.feat("db_override", 0.6)); // override enabled
        let dis = rng.feat("db_disable", 0.2);
        w.flag(dis);
        if !dis {
            w.se(rng.range(-6, 6) as i32);
            w.se(rng.range(-6, 6) as i32);
        }
    }
    let sl = sps.scaling_list_enabled && rng.feat("pps_scaling", 0.5);
    w.flag(sl);
    if sl {
        write_scaling_list(&mut w, rng);
    }
    w.flag(rng.feat("list_mod", 0.5)); // lists modification present
    w.ue(if rng.feat("parmrg", 0.6) { rng.range(1, sps.log2_ctb as i64 - 2) as u32 } else { 0 }); // log2_parallel_merge_level_minus2
    w.flag(rng.feat("sh_ext", 0.3)); // slice header extension present
    w.flag(false);
    w.trailing();
    let nal = escape(&w.bytes());
    let pps = parse_pps(&nal_to_rbsp(&nal))?;
    Ok((nal, pps))
}

// ---------------------------------------------------------------------------
// Slice segmentation
// ---------------------------------------------------------------------------

/// Partitions CTBs (TS order) into units obeying the tile / WPP constraints.
/// `within` restricts boundaries to lie inside [lo, hi).
fn partition(rng: &mut Rng, layout: &TileLayout, sps: &Sps, wpp: bool, lo: usize, hi: usize, p_split: f64) -> Vec<(usize, usize)> {
    let w = sps.pic_width_in_ctbs;
    let col_start = |x: usize| -> usize {
        let mut s = 0;
        for &b in &layout.col_bd {
            if b <= x {
                s = b;
            }
        }
        s
    };
    let mut units: Vec<(usize, usize)> = Vec::new();
    let mut start = lo;
    let x_lo = layout.ts_to_rs[lo] % w;
    let mut start_mid_row = wpp && x_lo != col_start(x_lo);
    let mut ts = lo;
    while ts < hi {
        let next = ts + 1;
        if next >= hi {
            break;
        }
        let rs_n = layout.ts_to_rs[next];
        let new_tile = layout.tile_id[next] != layout.tile_id[ts];
        let xn = rs_n % w;
        let row_start = xn == col_start(xn);
        // Is the current unit confined to one tile so far?
        let unit_single_tile = layout.tile_id[start] == layout.tile_id[ts];
        let unit_started_at_tile = start == 0 || layout.tile_id[start] != layout.tile_id[start - 1];
        let mut must_split = false;
        let mut may_split = true;
        if wpp && start_mid_row && row_start {
            must_split = true;
        }
        if new_tile {
            // continuing across a tile boundary requires the unit to have begun at a tile start
            if !unit_started_at_tile {
                must_split = true;
            }
        } else if !unit_single_tile {
            // a multi-tile unit can only end at a tile boundary
            may_split = false;
        }
        let split = must_split || (may_split && rng.chance(p_split));
        if split {
            units.push((start, next));
            start = next;
            start_mid_row = wpp && !row_start;
        } else if new_tile && !unit_single_tile && !unit_started_at_tile {
            unreachable!();
        }
        ts = next;
    }
    units.push((start, hi));
    // Validate WPP constraint for units starting mid-row (end within the row).
    units
}

// ---------------------------------------------------------------------------
// Pictures
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct RefPic {
    poc: i32,
    lt: bool,
}

struct LtEntry {
    sps_idx: Option<usize>,
    lsb: i32,
    used: bool,
    msb_present: bool,
    cycle: i32,
}

/// Generates a random stream. Returns the Annex B bytes and the frames the
/// generating decoder produced (which a correct decoder must reproduce).
pub fn generate(seed: u64) -> Result<(Vec<u8>, Vec<Frame>)> {
    let mut rng = Rng::new(seed);
    let mut out = Vec::new();
    let mut frames = Vec::new();
    let max_group = if rng.feat("reorder", 0.7) { rng.range(2, 4) as u32 } else { 1 };
    let max_refs = rng.range(1, 5) as u32;
    let reorder = max_group - 1;
    let max_dec = (max_refs + reorder + 2).min(16);
    let max_refs = max_refs.min(max_dec - reorder - 2);
    let seq = random_sps(&mut rng, max_dec, reorder)?;
    let sps = seq.sps.clone();
    let vps = write_vps(seq.profile, max_dec, reorder);
    let mut dec = Decoder::new::<&[u8]>(&[], 4)?;
    annexb(&mut out, &vps);
    dec.decode_nal(&vps)?;
    annexb(&mut out, &seq.sps_rbsp);
    dec.decode_nal(&seq.sps_rbsp)?;
    let npps = rng.range(1, 3) as u32;
    let mut ppss = Vec::new();
    for id in 0..npps {
        let (nal, pps) = random_pps(&mut rng, id, &sps)?;
        annexb(&mut out, &nal);
        dec.decode_nal(&nal)?;
        ppss.push(pps);
    }
    let max_lsb = 1i32 << sps.log2_max_poc_lsb;
    let npics = rng.range(3, 9);
    let mut refs: Vec<RefPic> = Vec::new();
    let mut history: Vec<i32> = Vec::new();
    let mut next_poc_base = 0i32;
    let mut pending: Vec<i32> = Vec::new(); // POCs of the current group still to decode
    let mut poc_offset = 0i32; // POC of the last IRAP (for numbering)
    let mut pic_idx = 0;
    let mut after_eos = false;
    while pic_idx < npics {
        // Decide picture kind
        let irap_kind = if pic_idx == 0 {
            if rng.chance(0.7) { 19 } else { 21 }
        } else if pending.is_empty() && rng.feat("irap_mid", 0.12) {
            if rng.chance(0.5) { 19 } else { 21 }
        } else {
            0
        };
        let poc: i32;
        if irap_kind != 0 {
            if irap_kind != 21 || pic_idx == 0 {
                // IDR resets POC; CRA keeps counting
            }
            if irap_kind == 21 && pic_idx > 0 && rng.feat("eos", 0.4) {
                // end of sequence before the CRA
                let mut w = BitWriter::default();
                nal_header(&mut w, 36, 0);
                let nal = w.bytes();
                annexb(&mut out, &nal);
                dec.decode_nal(&nal)?;
                after_eos = true;
            }
            if irap_kind == 19 {
                poc = 0;
                poc_offset = 0;
            } else if after_eos || pic_idx == 0 {
                // NoRaslOutputFlag CRA: POC msb is reset to 0. Avoid reusing a
                // POC of a picture that may still be in the DPB.
                let mut p = rng.range(0, (max_lsb - 1) as i64) as i32;
                while history.contains(&p) || refs.iter().any(|r| r.poc == p) {
                    p = (p + 1) & (max_lsb - 1);
                }
                poc = p;
                poc_offset = poc;
            } else {
                poc = next_poc_base;
                poc_offset = poc;
            }
            next_poc_base = poc + 1;
            refs.clear();
            history.clear();
        } else {
            if pending.is_empty() {
                let g = rng.range(1, max_group as i64) as i32;
                // decode order: last first, the rest shuffled
                let mut rest: Vec<i32> = (0..g - 1).map(|i| next_poc_base + i).collect();
                for i in (1..rest.len()).rev() {
                    let j = rng.range(0, i as i64) as usize;
                    rest.swap(i, j);
                }
                pending.push(next_poc_base + g - 1);
                pending.extend(rest);
                pending.reverse(); // pop from the end
                next_poc_base += g;
            }
            poc = pending.pop().expect("pending");
        }
        let _ = poc_offset;
        // ---- reference picture set
        let cur = poc;
        let mut keep: Vec<RefPic> = Vec::new();
        if irap_kind == 0 {
            for r in &refs {
                if (keep.len() as u32) < max_refs && rng.chance(0.8) {
                    keep.push(r.clone());
                }
            }
            if sps.long_term_refs_present {
                for r in keep.iter_mut() {
                    if !r.lt && rng.feat("lt_convert", 0.2) {
                        r.lt = true;
                    }
                }
            }
        }
        let mut st: Vec<(i32, bool)> = Vec::new();
        let mut lt_list: Vec<(i32, bool)> = Vec::new();
        for r in &keep {
            let used = rng.chance(0.75);
            if r.lt {
                lt_list.push((r.poc, used));
            } else {
                st.push((r.poc - cur, used));
            }
        }
        let num_curr = st.iter().filter(|x| x.1).count() + lt_list.iter().filter(|x| x.1).count();
        // ST RPS coding
        let mut neg: Vec<(i32, bool)> = st.iter().cloned().filter(|x| x.0 < 0).collect();
        let mut pos: Vec<(i32, bool)> = st.iter().cloned().filter(|x| x.0 > 0).collect();
        neg.sort_by(|a, b| b.0.cmp(&a.0));
        pos.sort_by(|a, b| a.0.cmp(&b.0));
        let target = StRpsKey::from(&neg, &pos);
        // SPS sets that match exactly
        let matching: Vec<usize> =
            sps.st_rps.iter().enumerate().filter(|(_, s)| StRpsKey::of(s) == target).map(|(i, _)| i).collect();
        let rps_choice: Option<usize> = if !matching.is_empty() && rng.chance(0.7) {
            Some(matching[rng.range(0, matching.len() as i64 - 1) as usize])
        } else {
            None
        };
        let explicit_syntax = if rps_choice.is_none() {
            Some(find_inter_rps(&mut rng, &sps, &neg, &pos).unwrap_or(RpsSyntax::Explicit { neg: neg.clone(), pos: pos.clone() }))
        } else {
            None
        };
        // LT entries
        let mut lt_entries: Vec<LtEntry> = Vec::new();
        for &(lpoc, used) in &lt_list {
            let lsb = lpoc & (max_lsb - 1);
            let ambiguous = history.iter().filter(|&&p| p != lpoc && (p & (max_lsb - 1)) == lsb).count() > 0;
            let msb_present = ambiguous || rng.chance(0.3);
            let cur_msb = cur - (cur & (max_lsb - 1));
            let lt_msb = lpoc - lsb;
            let cycle = (cur_msb - lt_msb) / max_lsb;
            let sps_idx = sps
                .lt_ref_pic_poc_lsb_sps
                .iter()
                .enumerate()
                .position(|(i, &l)| l as i32 == lsb && sps.used_by_curr_pic_lt_sps[i] == used);
            let sps_idx = if rng.chance(0.7) { sps_idx } else { None };
            lt_entries.push(LtEntry { sps_idx, lsb, used, msb_present, cycle });
        }
        // order: SPS-coded first; within groups non-msb first then by cycle
        lt_entries.sort_by_key(|e| (e.sps_idx.is_none(), e.msb_present, e.cycle));
        let is_irap = irap_kind != 0;
        let nal_type = if is_irap {
            irap_kind
        } else if rng.feat("trailn", 0.2) && pending.is_empty() {
            0 // TRAIL_N: not referenced afterwards
        } else {
            1
        };
        // ---- choose PPS and slices
        let pps_id = rng.range(0, npps as i64 - 1) as usize;
        let pps = &ppss[pps_id];
        let layout = TileLayout::new(&sps, pps)?;
        let nctb = sps.pic_size_in_ctbs();
        let wpp = pps.entropy_coding_sync_enabled;
        let p_slices = if rng.feat("slices", 0.8) { 0.15 } else { 0.0 };
        let slices = partition(&mut rng, &layout, &sps, wpp, 0, nctb, p_slices);
        let temporal_mvp = sps.temporal_mvp_enabled && rng.chance(0.7);
        // collocated picture: first entry of RefPicList0 (StCurrBefore/After/LtCurr order)
        let pic_output = !pps.output_flag_present || !rng.feat("no_output", 0.15);
        let slice_types: Vec<u32> = slices
            .iter()
            .map(|_| {
                if is_irap || num_curr == 0 || !rng.feat("inter", 0.9) {
                    2
                } else if rng.feat("bslices", 0.5) {
                    0
                } else {
                    1
                }
            })
            .collect();
        let all_b = slice_types.iter().all(|&t| t == 0 || t == 2);
        // ffmpeg evaluates the SAO slice-boundary rule with the current slice's
        // flag only; keep the flag uniform per picture unless requested.
        let pic_lf_across = rng.chance(0.5);
        let lf_mixed = std::env::var_os("STUDIO_SYNTH_LF_MIXED").is_some();
        let col_from_l0 = !(all_b && rng.chance(0.5));
        let mut first_segment = true;
        for (si, &(s0, s1)) in slices.iter().enumerate() {
            let segs = if pps.dependent_slice_segments_enabled {
                partition(&mut rng, &layout, &sps, wpp, s0, s1, 0.25)
            } else {
                vec![(s0, s1)]
            };
            let slice_type = slice_types[si];
            let mut indep_bits: Option<SliceFields> = None;
            for (k, &(g0, g1)) in segs.iter().enumerate() {
                let dependent = k > 0;
                if !dependent {
                    let mut fields = SliceFields::random(&mut rng, &sps, pps, slice_type, num_curr, col_from_l0);
                    if !lf_mixed {
                        fields.lf_across = pic_lf_across;
                    }
                    indep_bits = Some(fields);
                }
                let f = indep_bits.as_ref().expect("fields");
                let address = layout.ts_to_rs[g0];
                let hdr_ctx = HeaderCtx {
                    nal_type,
                    first: first_segment,
                    pps_id: pps_id as u32,
                    dependent,
                    address,
                    poc_lsb: cur & (max_lsb - 1),
                    rps_idx: rps_choice,
                    rps_syntax: explicit_syntax.as_ref(),
                    lt: &lt_entries,
                    pic_output,
                    temporal_mvp,
                };
                let mut entry_points: Vec<u32> = Vec::new();
                let need_ep = pps.tiles_enabled || pps.entropy_coding_sync_enabled;
                // placeholder entry points: count unknown yet -> write none, then rewrite
                let hdr_bits = write_slice_header(&hdr_ctx, f, &sps, pps, &entry_points)?;
                let header_nal = escape(&hdr_bits.bytes());
                let seg_seed = rng.next();
                let (data, starts) = dec.synth_slice(&header_nal, seg_seed, g1 - g0)?;
                if need_ep && !starts.is_empty() {
                    // substream sizes in escaped bytes
                    let mut bounds = vec![0usize];
                    bounds.extend(starts.iter().cloned());
                    bounds.push(data.len());
                    for i in 0..bounds.len() - 2 {
                        let part = &data[bounds[i]..bounds[i + 1]];
                        let esc = escape_continuing(&data[..bounds[i]], part);
                        entry_points.push(esc as u32);
                    }
                }
                let hdr_bits = write_slice_header(&hdr_ctx, f, &sps, pps, &entry_points)?;
                let mut rbsp = hdr_bits.bytes();
                rbsp.extend_from_slice(&data);
                let nal = escape(&rbsp);
                annexb(&mut out, &nal);
                first_segment = false;
            }
        }
        frames.extend(dec.end_of_picture());
        // ---- update reference model
        refs = keep;
        if nal_type != 0 {
            refs.push(RefPic { poc, lt: false });
        }
        history.push(poc);
        after_eos = false;
        pic_idx += 1;
        let _ = &mut next_poc_base;
    }
    frames.extend(dec.flush());
    Ok((out, frames))
}

/// Number of escaped bytes of `part` when it follows `prefix` in the same NAL.
fn escape_continuing(prefix: &[u8], part: &[u8]) -> usize {
    let mut zeros = 0;
    for &b in prefix.iter().rev().take(2) {
        if b == 0 {
            zeros += 1;
        } else {
            break;
        }
    }
    // zeros counted from the end; only consecutive trailing zeros matter
    let mut n = 0;
    for &b in part {
        if zeros >= 2 && b <= 3 {
            n += 1;
            zeros = 0;
        }
        n += 1;
        if b == 0 {
            zeros += 1;
        } else {
            zeros = 0;
        }
    }
    n
}

#[derive(PartialEq, Eq, Debug)]
struct StRpsKey(Vec<(i32, bool)>, Vec<(i32, bool)>);

impl StRpsKey {
    fn from(neg: &[(i32, bool)], pos: &[(i32, bool)]) -> StRpsKey {
        StRpsKey(neg.to_vec(), pos.to_vec())
    }
    fn of(s: &StRps) -> StRpsKey {
        StRpsKey(
            (0..s.num_negative).map(|i| (s.delta_poc_s0[i], s.used_s0[i])).collect(),
            (0..s.num_positive).map(|i| (s.delta_poc_s1[i], s.used_s1[i])).collect(),
        )
    }
}

/// Tries to code the desired RPS by inter-RPS prediction from an SPS set.
fn find_inter_rps(rng: &mut Rng, sps: &Sps, neg: &[(i32, bool)], pos: &[(i32, bool)]) -> Option<RpsSyntax> {
    let n = sps.st_rps.len();
    if n == 0 || !rng.chance(0.6) {
        return None;
    }
    let desired: Vec<(i32, bool)> = neg.iter().chain(pos.iter()).cloned().collect();
    for _ in 0..20 {
        let delta_idx_minus1 = rng.range(0, n as i64 - 1) as usize;
        let r = &sps.st_rps[n - 1 - delta_idx_minus1];
        let d = rng.range(-6, 6) as i32;
        if d == 0 {
            continue;
        }
        let mut cand: Vec<i32> = Vec::new();
        for j in 0..r.num_negative {
            cand.push(r.delta_poc_s0[j] + d);
        }
        for j in 0..r.num_positive {
            cand.push(r.delta_poc_s1[j] + d);
        }
        cand.push(d);
        if !desired.iter().all(|(x, _)| cand.contains(x)) {
            continue;
        }
        let flags: Vec<(bool, bool)> = cand
            .iter()
            .map(|c| match desired.iter().find(|(x, _)| x == c) {
                Some(&(_, used)) => (used, true),
                None => (false, false),
            })
            .collect();
        // Verify by parsing.
        let syn = RpsSyntax::Inter { delta_idx_minus1: delta_idx_minus1 as u32, delta_rps: d, flags };
        let mut w = BitWriter::default();
        write_rps(&mut w, n, n, &syn);
        w.trailing();
        let bytes = w.bytes();
        let mut br = crate::bitreader::BitReader::new(&bytes);
        if let Ok(s) = crate::params::parse_st_rps(&mut br, n, n, &sps.st_rps, 16) {
            if StRpsKey::of(&s) == StRpsKey::from(neg, pos) {
                return Some(syn);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Slice header
// ---------------------------------------------------------------------------

struct HeaderCtx<'a> {
    nal_type: u8,
    first: bool,
    pps_id: u32,
    dependent: bool,
    address: usize,
    poc_lsb: i32,
    rps_idx: Option<usize>,
    rps_syntax: Option<&'a RpsSyntax>,
    lt: &'a [LtEntry],
    pic_output: bool,
    temporal_mvp: bool,
}

/// Random per-slice fields (shared by the dependent segments of a slice).
struct SliceFields {
    slice_type: u32,
    extra_bits: Vec<bool>,
    sao_luma: bool,
    sao_chroma: bool,
    num_ref_override: Option<[u32; 2]>,
    list_entries: [Option<Vec<u32>>; 2],
    mvd_l1_zero: bool,
    cabac_init: bool,
    collocated_from_l0: bool,
    collocated_ref_idx: u32,
    pwt: Option<BitWriter>,
    five_minus_merge: u32,
    qp_delta: i32,
    cb: i32,
    cr: i32,
    deblock_override: Option<(bool, i32, i32)>,
    lf_across: bool,
    ext: Vec<u8>,
}

impl SliceFields {
    fn random(rng: &mut Rng, sps: &Sps, pps: &Pps, slice_type: u32, num_curr: usize, col_from_l0: bool) -> SliceFields {
        let b = slice_type == 0;
        let inter = slice_type != 2;
        let mut num = [pps.num_ref_idx_l0_default_active as u32, if b { pps.num_ref_idx_l1_default_active as u32 } else { 0 }];
        let mut ovr = None;
        if inter && rng.chance(0.5) {
            num = [rng.range(1, 6) as u32, if b { rng.range(1, 6) as u32 } else { 0 }];
            ovr = Some(num);
        }
        let mut list_entries: [Option<Vec<u32>>; 2] = [None, None];
        if inter && pps.lists_modification_present && num_curr > 1 {
            for l in 0..if b { 2 } else { 1 } {
                if rng.chance(0.6) {
                    list_entries[l] = Some((0..num[l]).map(|_| rng.range(0, num_curr as i64 - 1) as u32).collect());
                }
            }
        }
        // Collocated reference: index 0 of L0 (or L1) when not modified must be consistent per picture.
        // We always point at the picture at temp-list position 0 of the respective list by forcing
        // list entry 0 to 0 when the list is modified.
        let collocated_from_l0 = if b { col_from_l0 } else { true };
        let l = if collocated_from_l0 { 0 } else { 1 };
        if let Some(e) = list_entries[l].as_mut() {
            e[0] = 0;
        }
        let pwt = if (pps.weighted_pred && slice_type == 1) || (pps.weighted_bipred && b) {
            let mut w = BitWriter::default();
            let denom = rng.range(0, 7) as i32;
            w.ue(denom as u32);
            let cdenom = rng.range(0, 7) as i32;
            w.se(cdenom - denom);
            for li in 0..if b { 2 } else { 1 } {
                let n = num[li] as usize;
                let lf: Vec<bool> = (0..n).map(|_| rng.chance(0.6)).collect();
                let cf: Vec<bool> = (0..n).map(|_| rng.chance(0.6)).collect();
                for &f in &lf {
                    w.flag(f);
                }
                for &f in &cf {
                    w.flag(f);
                }
                for i in 0..n {
                    if lf[i] {
                        w.se(rng.range(-20, 20) as i32);
                        w.se(rng.range(-40, 40) as i32);
                    }
                    if cf[i] {
                        for _ in 0..2 {
                            w.se(rng.range(-20, 20) as i32);
                            w.se(rng.range(-100, 100) as i32);
                        }
                    }
                }
            }
            Some(w)
        } else {
            None
        };
        let qp_bd = 6 * (sps.bit_depth_luma as i32 - 8);
        let qp = rng.range((-qp_bd).max(-pps.init_qp + 0) as i64 - 0, 51) as i32;
        let qp = qp.clamp(-qp_bd, 51);
        let deblock_override = if pps.deblocking_filter_override_enabled && rng.chance(0.6) {
            let dis = rng.chance(0.3);
            Some((dis, rng.range(-6, 6) as i32, rng.range(-6, 6) as i32))
        } else {
            None
        };
        let max_cb_off = 12 - pps.cb_qp_offset.abs();
        let max_cr_off = 12 - pps.cr_qp_offset.abs();
        SliceFields {
            slice_type,
            extra_bits: (0..pps.num_extra_slice_header_bits).map(|_| rng.chance(0.5)).collect(),
            sao_luma: sps.sao_enabled && rng.chance(0.7),
            sao_chroma: sps.sao_enabled && rng.chance(0.7),
            num_ref_override: ovr,
            list_entries,
            mvd_l1_zero: rng.chance(0.3),
            cabac_init: rng.chance(0.5),
            collocated_from_l0,
            collocated_ref_idx: 0,
            pwt,
            five_minus_merge: rng.range(0, 4) as u32,
            qp_delta: qp - pps.init_qp,
            cb: rng.range(-(max_cb_off.min(6)) as i64, max_cb_off.min(6) as i64) as i32,
            cr: rng.range(-(max_cr_off.min(6)) as i64, max_cr_off.min(6) as i64) as i32,
            deblock_override,
            lf_across: rng.chance(0.5),
            ext: (0..rng.range(0, 3)).map(|_| rng.next() as u8).collect(),
        }
    }
}

fn ceil_log2(n: usize) -> u32 {
    if n <= 1 {
        0
    } else {
        usize::BITS - (n - 1).leading_zeros()
    }
}

fn write_slice_header(h: &HeaderCtx, f: &SliceFields, sps: &Sps, pps: &Pps, entry_points: &[u32]) -> Result<BitWriter> {
    let mut w = BitWriter::default();
    nal_header(&mut w, h.nal_type, 0);
    w.flag(h.first);
    let irap = (16..=23).contains(&h.nal_type);
    if irap {
        w.flag(false); // no_output_of_prior_pics
    }
    w.ue(h.pps_id);
    if !h.first {
        if pps.dependent_slice_segments_enabled {
            w.flag(h.dependent);
        }
        w.u(h.address as u64, ceil_log2(sps.pic_size_in_ctbs()));
    }
    if !h.dependent {
        for &b in &f.extra_bits {
            w.flag(b);
        }
        w.ue(f.slice_type);
        if pps.output_flag_present {
            w.flag(h.pic_output);
        }
        let idr = h.nal_type == 19 || h.nal_type == 20;
        if !idr {
            w.u(h.poc_lsb as u64, sps.log2_max_poc_lsb as u32);
            match h.rps_idx {
                Some(i) => {
                    w.flag(true);
                    if sps.st_rps.len() > 1 {
                        w.u(i as u64, ceil_log2(sps.st_rps.len()));
                    }
                }
                None => {
                    w.flag(false);
                    let syn = h.rps_syntax.ok_or(Error::Invalid("synth: no rps"))?;
                    write_rps(&mut w, sps.st_rps.len(), sps.st_rps.len(), syn);
                }
            }
            if sps.long_term_refs_present {
                let nsps = h.lt.iter().filter(|e| e.sps_idx.is_some()).count();
                if !sps.lt_ref_pic_poc_lsb_sps.is_empty() {
                    w.ue(nsps as u32);
                }
                w.ue((h.lt.len() - nsps) as u32);
                let mut prev_cycle = 0;
                for (i, e) in h.lt.iter().enumerate() {
                    if i < nsps {
                        if sps.lt_ref_pic_poc_lsb_sps.len() > 1 {
                            w.u(e.sps_idx.unwrap_or(0) as u64, ceil_log2(sps.lt_ref_pic_poc_lsb_sps.len()));
                        }
                    } else {
                        w.u(e.lsb as u64, sps.log2_max_poc_lsb as u32);
                        w.flag(e.used);
                    }
                    w.flag(e.msb_present);
                    if i == 0 || i == nsps {
                        prev_cycle = 0;
                    }
                    if e.msb_present {
                        let d = e.cycle - prev_cycle;
                        if d < 0 {
                            return Err(Error::Invalid("synth: lt cycle order"));
                        }
                        w.ue(d as u32);
                        prev_cycle = e.cycle;
                    }
                }
            } else if !h.lt.is_empty() {
                return Err(Error::Invalid("synth: lt without sps support"));
            }
            if sps.temporal_mvp_enabled {
                w.flag(h.temporal_mvp);
            }
        }
        if sps.sao_enabled {
            w.flag(f.sao_luma);
            w.flag(f.sao_chroma);
        }
        if f.slice_type != 2 {
            let b = f.slice_type == 0;
            w.flag(f.num_ref_override.is_some());
            let num = match f.num_ref_override {
                Some(n) => {
                    w.ue(n[0] - 1);
                    if b {
                        w.ue(n[1] - 1);
                    }
                    n
                }
                None => [pps.num_ref_idx_l0_default_active as u32, pps.num_ref_idx_l1_default_active as u32],
            };
            let num_curr = num_pic_total_curr(sps, h);
            if pps.lists_modification_present && num_curr > 1 {
                for l in 0..if b { 2 } else { 1 } {
                    w.flag(f.list_entries[l].is_some());
                    if let Some(e) = &f.list_entries[l] {
                        for i in 0..num[l] as usize {
                            w.u(e[i] as u64, ceil_log2(num_curr));
                        }
                    }
                }
            }
            if b {
                w.flag(f.mvd_l1_zero);
            }
            if pps.cabac_init_present {
                w.flag(f.cabac_init);
            }
            if h.temporal_mvp {
                if b {
                    w.flag(f.collocated_from_l0);
                }
                let l = if f.collocated_from_l0 || !b { 0 } else { 1 };
                if num[l] > 1 {
                    w.ue(f.collocated_ref_idx);
                }
            }
            if let Some(p) = &f.pwt {
                w.bits.extend_from_slice(&p.bits);
            }
            w.ue(f.five_minus_merge);
        }
        w.se(f.qp_delta);
        if pps.slice_chroma_qp_offsets_present {
            w.se(f.cb);
            w.se(f.cr);
        }
        let mut disabled = pps.pps_deblocking_filter_disabled;
        if pps.deblocking_filter_override_enabled {
            w.flag(f.deblock_override.is_some());
            if let Some((dis, beta, tc)) = f.deblock_override {
                w.flag(dis);
                disabled = dis;
                if !dis {
                    w.se(beta);
                    w.se(tc);
                }
            }
        }
        if pps.loop_filter_across_slices_enabled && (f.sao_luma || f.sao_chroma || !disabled) {
            w.flag(f.lf_across);
        }
    }
    if pps.tiles_enabled || pps.entropy_coding_sync_enabled {
        w.ue(entry_points.len() as u32);
        if !entry_points.is_empty() {
            let maxv = entry_points.iter().map(|&e| e - 1).max().unwrap_or(0);
            let len = (32 - maxv.leading_zeros()).max(1);
            w.ue(len - 1);
            for &e in entry_points {
                w.u((e - 1) as u64, len);
            }
        }
    }
    if pps.slice_segment_header_extension_present {
        w.ue(f.ext.len() as u32);
        for &b in &f.ext {
            w.u(b as u64, 8);
        }
    }
    // byte_alignment()
    w.flag(true);
    while w.len() % 8 != 0 {
        w.flag(false);
    }
    Ok(w)
}

fn num_pic_total_curr(sps: &Sps, h: &HeaderCtx) -> usize {
    let st = match h.rps_idx {
        Some(i) => sps.st_rps[i].clone(),
        None => {
            let mut w = BitWriter::default();
            if let Some(s) = h.rps_syntax {
                write_rps(&mut w, sps.st_rps.len(), sps.st_rps.len(), s);
            }
            w.trailing();
            let bytes = w.bytes();
            let mut br = crate::bitreader::BitReader::new(&bytes);
            crate::params::parse_st_rps(&mut br, sps.st_rps.len(), sps.st_rps.len(), &sps.st_rps, 16).unwrap_or_default()
        }
    };
    let mut n = 0;
    for i in 0..st.num_negative {
        n += st.used_s0[i] as usize;
    }
    for i in 0..st.num_positive {
        n += st.used_s1[i] as usize;
    }
    n + h.lt.iter().filter(|e| e.used).count()
}

#[allow(dead_code)]
fn _unused(_: NalHeader) {}
