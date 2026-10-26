// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Parametric stereo (HE-AAC v2, ISO/IEC 14496-3 8.6.4 / 8.A): bitstream
//! (IID, ICC, IPD/OPD and extensions), hybrid analysis (20- and 34-band
//! configurations), transient-aware all-pass decorrelation, stereo mixing
//! (procedures A and B, with IPD/OPD phase smoothing) and hybrid synthesis.
//! Turns the mono SBR QMF signal into a stereo pair.

mod huff_tables;
pub(crate) mod tables;

use crate::bits::BitReader;
use crate::error::Result;
use crate::fft::C32;
use crate::huffman::Vlc;
use std::sync::OnceLock;
use tables::*;

const NUM_QMF_SLOTS: usize = 32;
const MAX_ENV: usize = 5;
const MAX_PAR: usize = 34;

struct Books {
    iid_df: Vlc,
    iid_dt: Vlc,
    iid_fine_df: Vlc,
    iid_fine_dt: Vlc,
    icc_df: Vlc,
    icc_dt: Vlc,
    ipd_df: Vlc,
    ipd_dt: Vlc,
    opd_df: Vlc,
    opd_dt: Vlc,
}

/// IPD/OPD books (ISO/IEC 14496-3 Table 8.A.24..8.A.27), symbol = index 0..7.
const IPD_DF: ([u8; 8], [u32; 8]) = ([1, 3, 4, 4, 4, 4, 4, 4], [0x01, 0x00, 0x06, 0x04, 0x02, 0x03, 0x05, 0x07]);
const IPD_DT: ([u8; 8], [u32; 8]) = ([1, 3, 4, 5, 5, 4, 4, 3], [0x01, 0x02, 0x02, 0x03, 0x02, 0x00, 0x03, 0x03]);
const OPD_DF: ([u8; 8], [u32; 8]) = ([1, 3, 4, 4, 5, 5, 4, 3], [0x01, 0x01, 0x06, 0x04, 0x0f, 0x0e, 0x05, 0x00]);
const OPD_DT: ([u8; 8], [u32; 8]) = ([1, 3, 4, 5, 5, 4, 4, 3], [0x01, 0x02, 0x01, 0x07, 0x06, 0x00, 0x02, 0x03]);

fn books() -> &'static Books {
    use huff_tables as h;
    static B: OnceLock<Books> = OnceLock::new();
    B.get_or_init(|| Books {
        iid_df: Vlc::new(&h::IID_DF_LENS, &h::IID_DF_CODES, 9),
        iid_dt: Vlc::new(&h::IID_DT_LENS, &h::IID_DT_CODES, 9),
        iid_fine_df: Vlc::new(&h::IID_FINE_DF_LENS, &h::IID_FINE_DF_CODES, 9),
        iid_fine_dt: Vlc::new(&h::IID_FINE_DT_LENS, &h::IID_FINE_DT_CODES, 9),
        icc_df: Vlc::new(&h::ICC_DF_LENS, &h::ICC_DF_CODES, 9),
        icc_dt: Vlc::new(&h::ICC_DT_LENS, &h::ICC_DT_CODES, 9),
        ipd_df: Vlc::new(&IPD_DF.0, &IPD_DF.1, 5),
        ipd_dt: Vlc::new(&IPD_DT.0, &IPD_DT.1, 5),
        opd_df: Vlc::new(&OPD_DF.0, &OPD_DF.1, 5),
        opd_dt: Vlc::new(&OPD_DT.0, &OPD_DT.1, 5),
    })
}

const NR_IIDICC_PAR: [usize; 6] = [10, 20, 34, 10, 20, 34];
const NR_IPDOPD_PAR: [usize; 6] = [5, 11, 17, 5, 11, 17];
const NUM_ENV_TAB: [[usize; 4]; 2] = [[0, 1, 2, 4], [1, 2, 3, 4]];

type Par = [[i8; MAX_PAR]; MAX_ENV + 1];

/// Parametric stereo decoder state.
pub(crate) struct Ps {
    pub start: bool,
    enable_iid: bool,
    iid_quant: bool,
    nr_iid_par: usize,
    nr_ipdopd_par: usize,
    enable_icc: bool,
    icc_mode: usize,
    nr_icc_par: usize,
    enable_ext: bool,
    enable_ipdopd: bool,
    num_env_old: usize,
    num_env: usize,
    border: [i32; MAX_ENV + 2],
    iid_par: Par,
    icc_par: Par,
    ipd_par: Par,
    opd_par: Par,
    is34: bool,
    is34_old: bool,

    in_buf: [[C32; 44]; 5],
    delay: Vec<[C32; NUM_QMF_SLOTS + PS_MAX_DELAY]>,
    ap_delay: Vec<[[C32; NUM_QMF_SLOTS + PS_MAX_AP_DELAY]; PS_AP_LINKS]>,
    peak_decay_nrg: [f32; 34],
    power_smooth: [f32; 34],
    peak_decay_diff_smooth: [f32; 34],
    /// [re/im][env+1][band] of h11, h12, h21, h22.
    h: [[[[f32; MAX_PAR]; MAX_ENV + 1]; 2]; 4],
    opd_hist: [u8; 17],
    ipd_hist: [u8; 17],
    lbuf: Vec<[C32; 32]>,
    rbuf: Vec<[C32; 32]>,
}

impl Ps {
    pub fn new() -> Ps {
        Ps {
            start: false,
            enable_iid: false,
            iid_quant: false,
            nr_iid_par: 0,
            nr_ipdopd_par: 0,
            enable_icc: false,
            icc_mode: 0,
            nr_icc_par: 0,
            enable_ext: false,
            enable_ipdopd: false,
            num_env_old: 0,
            num_env: 0,
            border: [0; MAX_ENV + 2],
            iid_par: [[0; MAX_PAR]; MAX_ENV + 1],
            icc_par: [[0; MAX_PAR]; MAX_ENV + 1],
            ipd_par: [[0; MAX_PAR]; MAX_ENV + 1],
            opd_par: [[0; MAX_PAR]; MAX_ENV + 1],
            is34: false,
            is34_old: false,
            in_buf: [[C32::default(); 44]; 5],
            delay: vec![[C32::default(); NUM_QMF_SLOTS + PS_MAX_DELAY]; 91],
            ap_delay: vec![[[C32::default(); NUM_QMF_SLOTS + PS_MAX_AP_DELAY]; PS_AP_LINKS]; 50],
            peak_decay_nrg: [0.0; 34],
            power_smooth: [0.0; 34],
            peak_decay_diff_smooth: [0.0; 34],
            h: [[[[0.0; MAX_PAR]; MAX_ENV + 1]; 2]; 4],
            opd_hist: [0; 17],
            ipd_hist: [0; 17],
            lbuf: vec![[C32::default(); 32]; 91],
            rbuf: vec![[C32::default(); 32]; 91],
        }
    }

    pub fn reset(&mut self) {
        *self = Ps::new();
    }

    /// (34-band configuration, IPD/OPD enabled) of the last parsed frame.
    pub fn mode(&self) -> (bool, bool) {
        (self.is34, self.enable_ipdopd)
    }

    /// ps_data(); consumes at most `bits_left` bits of `br` and returns the
    /// number of bits used. Malformed data disables PS for the frame.
    pub fn parse(&mut self, br: &mut BitReader, bits_left: usize) -> usize {
        let mut sub = br.clone();
        let start_pos = sub.position();
        let ok = self.parse_inner(&mut sub).is_ok();
        let used = sub.position() - start_pos;
        if ok && used <= bits_left {
            let _ = br.skip(used);
            return used;
        }
        self.start = false;
        self.iid_par = [[0; MAX_PAR]; MAX_ENV + 1];
        self.icc_par = [[0; MAX_PAR]; MAX_ENV + 1];
        self.ipd_par = [[0; MAX_PAR]; MAX_ENV + 1];
        self.opd_par = [[0; MAX_PAR]; MAX_ENV + 1];
        let _ = br.skip(bits_left);
        bits_left
    }

    fn read_par(
        br: &mut BitReader,
        par: &mut Par,
        num: usize,
        e: usize,
        dt: bool,
        num_env_old: usize,
        vlc: &Vlc,
        offset: i32,
        mask: Option<i32>,
    ) -> Result<()> {
        if dt {
            let e_prev = if e > 0 { e - 1 } else { num_env_old.saturating_sub(1) };
            for b in 0..num {
                let mut v = par[e_prev][b] as i32 + vlc.decode(br)? as i32 + offset;
                if let Some(m) = mask {
                    v &= m;
                }
                par[e][b] = v as i8;
            }
        } else {
            let mut v = 0i32;
            for b in 0..num {
                v += vlc.decode(br)? as i32 + offset;
                if let Some(m) = mask {
                    v &= m;
                }
                par[e][b] = v as i8;
            }
        }
        Ok(())
    }

    fn parse_inner(&mut self, br: &mut BitReader) -> Result<()> {
        let bk = books();
        let header = br.read_bit()?;
        if header {
            self.enable_iid = br.read_bit()?;
            if self.enable_iid {
                let mode = br.read_usize(3)?;
                if mode > 5 {
                    return Err(crate::Error::InvalidData("PS iid_mode"));
                }
                self.nr_iid_par = NR_IIDICC_PAR[mode];
                self.iid_quant = mode > 2;
                self.nr_ipdopd_par = NR_IPDOPD_PAR[mode];
            }
            self.enable_icc = br.read_bit()?;
            if self.enable_icc {
                self.icc_mode = br.read_usize(3)?;
                if self.icc_mode > 5 {
                    return Err(crate::Error::InvalidData("PS icc_mode"));
                }
                self.nr_icc_par = NR_IIDICC_PAR[self.icc_mode];
            }
            self.enable_ext = br.read_bit()?;
        }
        let frame_class = br.read_bit()?;
        self.num_env_old = self.num_env;
        self.num_env = NUM_ENV_TAB[frame_class as usize][br.read_usize(2)?];
        self.border[0] = -1;
        if frame_class {
            for e in 1..=self.num_env {
                self.border[e] = br.read(5)? as i32;
                if self.border[e] < self.border[e - 1] {
                    return Err(crate::Error::InvalidData("PS border"));
                }
            }
        } else {
            let log2 = [0, 0, 1, 2, 2][self.num_env];
            for e in 1..=self.num_env {
                self.border[e] = ((e * NUM_QMF_SLOTS) >> log2) as i32 - 1;
            }
        }
        if self.enable_iid {
            let lim = if self.iid_quant { 15 } else { 7 };
            for e in 0..self.num_env {
                let dt = br.read_bit()?;
                let (vlc, off) = match (dt, self.iid_quant) {
                    (false, false) => (&bk.iid_df, huff_tables::IID_DF_MIN),
                    (true, false) => (&bk.iid_dt, huff_tables::IID_DT_MIN),
                    (false, true) => (&bk.iid_fine_df, huff_tables::IID_FINE_DF_MIN),
                    (true, true) => (&bk.iid_fine_dt, huff_tables::IID_FINE_DT_MIN),
                };
                Self::read_par(br, &mut self.iid_par, self.nr_iid_par, e, dt, self.num_env_old, vlc, off, None)?;
                if self.iid_par[e][..self.nr_iid_par].iter().any(|&v| (v as i32).abs() > lim) {
                    return Err(crate::Error::InvalidData("PS iid out of range"));
                }
            }
        } else {
            self.iid_par = [[0; MAX_PAR]; MAX_ENV + 1];
        }
        if self.enable_icc {
            for e in 0..self.num_env {
                let dt = br.read_bit()?;
                let (vlc, off) =
                    if dt { (&bk.icc_dt, huff_tables::ICC_DT_MIN) } else { (&bk.icc_df, huff_tables::ICC_DF_MIN) };
                Self::read_par(br, &mut self.icc_par, self.nr_icc_par, e, dt, self.num_env_old, vlc, off, None)?;
                if self.icc_par[e][..self.nr_icc_par].iter().any(|&v| !(0..=7).contains(&v)) {
                    return Err(crate::Error::InvalidData("PS icc out of range"));
                }
            }
        } else {
            self.icc_par = [[0; MAX_PAR]; MAX_ENV + 1];
        }
        if self.enable_ext {
            let mut cnt = br.read_usize(4)?;
            if cnt == 15 {
                cnt += br.read_usize(8)?;
            }
            let mut cnt = cnt as isize * 8;
            while cnt > 7 {
                let id = br.read(2)?;
                let p0 = br.position();
                if id == 0 {
                    self.enable_ipdopd = br.read_bit()?;
                    if self.enable_ipdopd {
                        for e in 0..self.num_env {
                            let dt = br.read_bit()?;
                            let vlc = if dt { &bk.ipd_dt } else { &bk.ipd_df };
                            Self::read_par(br, &mut self.ipd_par, self.nr_ipdopd_par, e, dt, self.num_env_old, vlc, 0, Some(7))?;
                            let dt = br.read_bit()?;
                            let vlc = if dt { &bk.opd_dt } else { &bk.opd_df };
                            Self::read_par(br, &mut self.opd_par, self.nr_ipdopd_par, e, dt, self.num_env_old, vlc, 0, Some(7))?;
                        }
                    }
                    br.skip(1)?; // reserved_ps
                }
                cnt -= 2 + (br.position() - p0) as isize;
            }
            if cnt < 0 {
                return Err(crate::Error::InvalidData("PS extension overread"));
            }
            br.skip(cnt as usize)?;
        }
        // Fix up envelopes: the last border must reach the frame end.
        if self.num_env == 0 || self.border[self.num_env] < NUM_QMF_SLOTS as i32 - 1 {
            let source = if self.num_env > 0 { self.num_env as isize - 1 } else { self.num_env_old as isize - 1 };
            if source >= 0 && source as usize != self.num_env {
                let (s, d) = (source as usize, self.num_env);
                if self.enable_iid {
                    self.iid_par[d] = self.iid_par[s];
                }
                if self.enable_icc {
                    self.icc_par[d] = self.icc_par[s];
                }
                if self.enable_ipdopd {
                    self.ipd_par[d] = self.ipd_par[s];
                    self.opd_par[d] = self.opd_par[s];
                }
            }
            self.num_env += 1;
            self.border[self.num_env] = NUM_QMF_SLOTS as i32 - 1;
        }
        self.is34_old = self.is34;
        if self.enable_iid || self.enable_icc {
            self.is34 = (self.enable_iid && self.nr_iid_par == 34) || (self.enable_icc && self.nr_icc_par == 34);
        }
        if !self.enable_ipdopd {
            self.ipd_par = [[0; MAX_PAR]; MAX_ENV + 1];
            self.opd_par = [[0; MAX_PAR]; MAX_ENV + 1];
        }
        if header {
            self.start = true;
        }
        Ok(())
    }

    /// Apply PS to the QMF signal: `l` (in: mono, out: left) and `r` (out:
    /// right), both `[slot][band]` with 38 slots; `top` = kx + M.
    pub fn apply(&mut self, l: &mut [[C32; 64]; 38], r: &mut [[C32; 64]; 38], top: usize) {
        let is34 = self.is34 as usize;
        let top = top + NR_BANDS[is34] - 64;
        for k in top.min(NR_BANDS[is34])..NR_BANDS[is34] {
            self.delay[k] = [C32::default(); NUM_QMF_SLOTS + PS_MAX_DELAY];
        }
        for k in top.min(NR_ALLPASS_BANDS[is34])..NR_ALLPASS_BANDS[is34] {
            self.ap_delay[k] = [[C32::default(); NUM_QMF_SLOTS + PS_MAX_AP_DELAY]; PS_AP_LINKS];
        }
        let mut lbuf = std::mem::take(&mut self.lbuf);
        let mut rbuf = std::mem::take(&mut self.rbuf);
        self.hybrid_analysis(&mut lbuf, l, is34 == 1);
        self.decorrelation(&mut rbuf, &lbuf, is34 == 1);
        self.stereo_processing(&mut lbuf, &mut rbuf, is34 == 1);
        hybrid_synthesis(l, &lbuf, is34 == 1);
        hybrid_synthesis(r, &rbuf, is34 == 1);
        self.lbuf = lbuf;
        self.rbuf = rbuf;
    }

    fn hybrid_analysis(&mut self, out: &mut [[C32; 32]], l: &[[C32; 64]; 38], is34: bool) {
        let t = tables();
        for i in 0..5 {
            for j in 0..38 {
                self.in_buf[i][j + 6] = l[j][i];
            }
        }
        let filt = |input: &[C32; 44], filters: &[[C32; 13]], out: &mut [[C32; 32]], base: usize| {
            for (q, f) in filters.iter().enumerate() {
                for i in 0..32 {
                    let mut s = C32::default();
                    for n in 0..13 {
                        s = s + f[n] * input[i + n];
                    }
                    out[base + q][i] = s;
                }
            }
        };
        if is34 {
            filt(&self.in_buf[0], &t.f34_0_12, out, 0);
            filt(&self.in_buf[1], &t.f34_1_8, out, 12);
            filt(&self.in_buf[2], &t.f34_2_4, out, 20);
            filt(&self.in_buf[3], &t.f34_2_4, out, 24);
            filt(&self.in_buf[4], &t.f34_2_4, out, 28);
            for b in 5..64 {
                for j in 0..32 {
                    out[27 + b][j] = l[j][b];
                }
            }
        } else {
            let mut tmp = [[C32::default(); 32]; 8];
            filt(&self.in_buf[0], &t.f20_0_8, &mut tmp, 0);
            for i in 0..32 {
                out[0][i] = tmp[6][i];
                out[1][i] = tmp[7][i];
                out[2][i] = tmp[0][i];
                out[3][i] = tmp[1][i];
                out[4][i] = tmp[2][i] + tmp[5][i];
                out[5][i] = tmp[3][i] + tmp[4][i];
            }
            hybrid2_re(&self.in_buf[1], out, 6, true);
            hybrid2_re(&self.in_buf[2], out, 8, false);
            for b in 3..64 {
                for j in 0..32 {
                    out[7 + b][j] = l[j][b];
                }
            }
        }
        for i in 0..5 {
            let (head, tail) = self.in_buf[i].split_at_mut(32);
            head[..6].copy_from_slice(&tail[..6]);
        }
    }

    fn decorrelation(&mut self, out: &mut [[C32; 32]], s: &[[C32; 32]], is34b: bool) {
        let t = tables();
        let is34 = is34b as usize;
        let k_to_i: &[u8] = if is34b { &t.k_to_i_34 } else { &t.k_to_i_20 };
        const PEAK_DECAY: f32 = 0.765_928_3;
        const TRANSIENT_IMPACT: f32 = 1.5;
        const A_SMOOTH: f32 = 0.25;
        if is34b != self.is34_old {
            self.peak_decay_nrg = [0.0; 34];
            self.power_smooth = [0.0; 34];
            self.peak_decay_diff_smooth = [0.0; 34];
            for d in &mut self.delay {
                *d = [C32::default(); NUM_QMF_SLOTS + PS_MAX_DELAY];
            }
            for d in &mut self.ap_delay {
                *d = [[C32::default(); NUM_QMF_SLOTS + PS_MAX_AP_DELAY]; PS_AP_LINKS];
            }
        }
        let mut power = [[0f32; 32]; 34];
        for k in 0..NR_BANDS[is34] {
            let i = k_to_i[k] as usize;
            for n in 0..32 {
                power[i][n] += s[k][n].norm_sqr();
            }
        }
        let mut gain = [[0f32; 32]; 34];
        for i in 0..NR_PAR_BANDS[is34] {
            for n in 0..32 {
                let decayed = PEAK_DECAY * self.peak_decay_nrg[i];
                self.peak_decay_nrg[i] = decayed.max(power[i][n]);
                self.power_smooth[i] += A_SMOOTH * (power[i][n] - self.power_smooth[i]);
                self.peak_decay_diff_smooth[i] +=
                    A_SMOOTH * (self.peak_decay_nrg[i] - power[i][n] - self.peak_decay_diff_smooth[i]);
                let denom = TRANSIENT_IMPACT * self.peak_decay_diff_smooth[i];
                gain[i][n] = if denom > self.power_smooth[i] { self.power_smooth[i] / denom } else { 1.0 };
            }
        }
        let mut k = 0;
        while k < NR_ALLPASS_BANDS[is34] {
            let b = k_to_i[k] as usize;
            let g_decay = (1.0 - 0.05 * (k as f32 - DECAY_CUTOFF[is34] as f32)).clamp(0.0, 1.0);
            let d = &mut self.delay[k];
            d.copy_within(32..32 + PS_MAX_DELAY, 0);
            d[PS_MAX_DELAY..].copy_from_slice(&s[k]);
            let ap = &mut self.ap_delay[k];
            for link in ap.iter_mut() {
                link.copy_within(32..32 + PS_MAX_AP_DELAY, 0);
            }
            let phi = t.phi_fract[is34][k];
            let qf = t.q_fract_allpass[is34][k];
            let ag = [AP_A[0] * g_decay, AP_A[1] * g_decay, AP_A[2] * g_decay];
            for n in 0..32 {
                let mut v = d[PS_MAX_DELAY - 2 + n] * phi;
                for m in 0..PS_AP_LINKS {
                    let a = v.scale(ag[m]);
                    let link = ap[m][n + 2 - m];
                    let apd = v;
                    v = link * qf[m] - a;
                    ap[m][n + 5] = apd + v.scale(ag[m]);
                }
                out[k][n] = v.scale(gain[b][n]);
            }
            k += 1;
        }
        while k < NR_BANDS[is34] {
            let i = k_to_i[k] as usize;
            let d = &mut self.delay[k];
            d.copy_within(32..32 + PS_MAX_DELAY, 0);
            d[PS_MAX_DELAY..].copy_from_slice(&s[k]);
            let lag = if k < SHORT_DELAY_BAND[is34] { 14 } else { 1 };
            for n in 0..32 {
                out[k][n] = d[PS_MAX_DELAY - lag + n].scale(gain[i][n]);
            }
            k += 1;
        }
    }

    fn stereo_processing(&mut self, l: &mut [[C32; 32]], r: &mut [[C32; 32]], is34b: bool) {
        let t = tables();
        let is34 = is34b as usize;
        let k_to_i: &[u8] = if is34b { &t.k_to_i_34 } else { &t.k_to_i_20 };
        let h_lut = if self.icc_mode < 3 { &t.ha } else { &t.hb };
        // Carry the last envelope of the previous frame to row 0.
        if self.num_env_old > 0 {
            for m in 0..4 {
                for p in 0..2 {
                    self.h[m][p][0] = self.h[m][p][self.num_env_old];
                }
            }
        }
        let mut iid_m = [[0i8; MAX_PAR]; MAX_ENV + 1];
        let mut icc_m = [[0i8; MAX_PAR]; MAX_ENV + 1];
        let mut ipd_m = [[0i8; MAX_PAR]; MAX_ENV + 1];
        let mut opd_m = [[0i8; MAX_PAR]; MAX_ENV + 1];
        let n_env = self.num_env;
        if is34b {
            remap34(&mut iid_m, &self.iid_par, self.nr_iid_par, n_env, true);
            remap34(&mut icc_m, &self.icc_par, self.nr_icc_par, n_env, true);
            if self.enable_ipdopd {
                remap34(&mut ipd_m, &self.ipd_par, self.nr_ipdopd_par, n_env, false);
                remap34(&mut opd_m, &self.opd_par, self.nr_ipdopd_par, n_env, false);
            }
            if !self.is34_old {
                for m in 0..4 {
                    for p in 0..2 {
                        map_val_20_to_34(&mut self.h[m][p][0]);
                    }
                }
                self.ipd_hist = [0; 17];
                self.opd_hist = [0; 17];
            }
        } else {
            remap20(&mut iid_m, &self.iid_par, self.nr_iid_par, n_env, true);
            remap20(&mut icc_m, &self.icc_par, self.nr_icc_par, n_env, true);
            if self.enable_ipdopd {
                remap20(&mut ipd_m, &self.ipd_par, self.nr_ipdopd_par, n_env, false);
                remap20(&mut opd_m, &self.opd_par, self.nr_ipdopd_par, n_env, false);
            }
            if self.is34_old {
                for m in 0..4 {
                    for p in 0..2 {
                        map_val_34_to_20(&mut self.h[m][p][0]);
                    }
                }
                self.ipd_hist = [0; 17];
                self.opd_hist = [0; 17];
            }
        }
        let quant_off = if self.iid_quant { 15 + 15 } else { 7 };
        for e in 0..n_env {
            for b in 0..NR_PAR_BANDS[is34] {
                let iid_idx = (iid_m[e][b] as i32 + quant_off).max(0) as usize;
                let hh = h_lut[iid_idx.min(45)][(icc_m[e][b] as usize).min(7)];
                let (mut h11, mut h12, mut h21, mut h22) = (hh[0], hh[1], hh[2], hh[3]);
                if self.enable_ipdopd && b < NR_IPDOPD_BANDS[is34] {
                    let opd_idx = self.opd_hist[b] as usize * 8 + opd_m[e][b] as usize;
                    let ipd_idx = self.ipd_hist[b] as usize * 8 + ipd_m[e][b] as usize;
                    let (opd_re, opd_im) = (t.pd_re_smooth[opd_idx], t.pd_im_smooth[opd_idx]);
                    let (ipd_re, ipd_im) = (t.pd_re_smooth[ipd_idx], t.pd_im_smooth[ipd_idx]);
                    self.opd_hist[b] = (opd_idx & 0x3f) as u8;
                    self.ipd_hist[b] = (ipd_idx & 0x3f) as u8;
                    let adj_re = opd_re * ipd_re + opd_im * ipd_im;
                    let adj_im = opd_im * ipd_re - opd_re * ipd_im;
                    self.h[0][1][e + 1][b] = h11 * opd_im;
                    self.h[1][1][e + 1][b] = h12 * adj_im;
                    self.h[2][1][e + 1][b] = h21 * opd_im;
                    self.h[3][1][e + 1][b] = h22 * adj_im;
                    h11 *= opd_re;
                    h12 *= adj_re;
                    h21 *= opd_re;
                    h22 *= adj_re;
                }
                self.h[0][0][e + 1][b] = h11;
                self.h[1][0][e + 1][b] = h12;
                self.h[2][0][e + 1][b] = h21;
                self.h[3][0][e + 1][b] = h22;
            }
            let start = (self.border[e] + 1) as usize;
            let stop = (self.border[e + 1] + 1) as usize;
            let width = 1.0 / (stop - start).max(1) as f32;
            for k in 0..NR_BANDS[is34] {
                let b = k_to_i[k] as usize;
                let mut h = [0f32; 4];
                let mut hs = [0f32; 4];
                let mut hi = [0f32; 4];
                let mut his = [0f32; 4];
                let negate = (is34b && (9..=13).contains(&k)) || (!is34b && k <= 1);
                for m in 0..4 {
                    h[m] = self.h[m][0][e][b];
                    hs[m] = (self.h[m][0][e + 1][b] - h[m]) * width;
                    if self.enable_ipdopd {
                        let sgn = if negate { -1.0 } else { 1.0 };
                        hi[m] = sgn * self.h[m][1][e][b];
                        his[m] = (sgn * self.h[m][1][e + 1][b] - hi[m]) * width;
                    }
                }
                for n in start..stop.min(32) {
                    for m in 0..4 {
                        h[m] += hs[m];
                        hi[m] += his[m];
                    }
                    let lv = l[k][n];
                    let rv = r[k][n];
                    if self.enable_ipdopd {
                        l[k][n] = C32::new(
                            h[0] * lv.re + h[2] * rv.re - hi[0] * lv.im - hi[2] * rv.im,
                            hi[0] * lv.re + hi[2] * rv.re + h[0] * lv.im + h[2] * rv.im,
                        );
                        r[k][n] = C32::new(
                            h[1] * lv.re + h[3] * rv.re - hi[1] * lv.im - hi[3] * rv.im,
                            hi[1] * lv.re + hi[3] * rv.re + h[1] * lv.im + h[3] * rv.im,
                        );
                    } else {
                        l[k][n] = lv.scale(h[0]) + rv.scale(h[2]);
                        r[k][n] = lv.scale(h[1]) + rv.scale(h[3]);
                    }
                }
            }
        }
    }
}

fn hybrid2_re(input: &[C32; 44], out: &mut [[C32; 32]], base: usize, reverse: bool) {
    for i in 0..32 {
        let x = &input[i..i + 13];
        let inph = x[6].scale(G1_Q2[6]);
        let mut op = C32::default();
        for j in (0..6).step_by(2) {
            op = op + (x[j + 1] + x[11 - j]).scale(G1_Q2[j + 1]);
        }
        let (a, b) = (inph + op, inph - op);
        if reverse {
            out[base + 1][i] = a;
            out[base][i] = b;
        } else {
            out[base][i] = a;
            out[base + 1][i] = b;
        }
    }
}

fn hybrid_synthesis(out: &mut [[C32; 64]; 38], input: &[[C32; 32]], is34: bool) {
    for n in 0..32 {
        if is34 {
            let mut s = [C32::default(); 5];
            for i in 0..12 {
                s[0] = s[0] + input[i][n];
            }
            for i in 0..8 {
                s[1] = s[1] + input[12 + i][n];
            }
            for i in 0..4 {
                s[2] = s[2] + input[20 + i][n];
                s[3] = s[3] + input[24 + i][n];
                s[4] = s[4] + input[28 + i][n];
            }
            out[n][..5].copy_from_slice(&s);
            for b in 5..64 {
                out[n][b] = input[27 + b][n];
            }
        } else {
            let mut s0 = C32::default();
            for i in 0..6 {
                s0 = s0 + input[i][n];
            }
            out[n][0] = s0;
            out[n][1] = input[6][n] + input[7][n];
            out[n][2] = input[8][n] + input[9][n];
            for b in 3..64 {
                out[n][b] = input[7 + b][n];
            }
        }
    }
}

type Mapped = [[i8; MAX_PAR]; MAX_ENV + 1];

fn remap20(dst: &mut Mapped, par: &Par, num_par: usize, num_env: usize, full: bool) {
    for e in 0..num_env {
        match num_par {
            34 | 17 => map_idx_34_to_20(&mut dst[e], &par[e], full),
            10 | 5 => map_idx_10_to_20(&mut dst[e], &par[e], full),
            _ => dst[e] = par[e],
        }
    }
}

fn remap34(dst: &mut Mapped, par: &Par, num_par: usize, num_env: usize, full: bool) {
    for e in 0..num_env {
        match num_par {
            20 | 11 => map_idx_20_to_34(&mut dst[e], &par[e], full),
            10 | 5 => map_idx_10_to_34(&mut dst[e], &par[e], full),
            _ => dst[e] = par[e],
        }
    }
}

fn map_idx_10_to_20(m: &mut [i8; MAX_PAR], p: &[i8; MAX_PAR], full: bool) {
    let top = if full { 9 } else {
        m[10] = 0;
        4
    };
    for b in (0..=top).rev() {
        m[2 * b + 1] = p[b];
        m[2 * b] = p[b];
    }
}

fn map_idx_34_to_20(m: &mut [i8; MAX_PAR], p: &[i8; MAX_PAR], full: bool) {
    let a = |i: usize| p[i] as i32;
    m[0] = ((2 * a(0) + a(1)) / 3) as i8;
    m[1] = ((a(1) + 2 * a(2)) / 3) as i8;
    m[2] = ((2 * a(3) + a(4)) / 3) as i8;
    m[3] = ((a(4) + 2 * a(5)) / 3) as i8;
    m[4] = ((a(6) + a(7)) / 2) as i8;
    m[5] = ((a(8) + a(9)) / 2) as i8;
    m[6] = p[10];
    m[7] = p[11];
    m[8] = ((a(12) + a(13)) / 2) as i8;
    m[9] = ((a(14) + a(15)) / 2) as i8;
    m[10] = p[16];
    if full {
        m[11] = p[17];
        m[12] = p[18];
        m[13] = p[19];
        m[14] = ((a(20) + a(21)) / 2) as i8;
        m[15] = ((a(22) + a(23)) / 2) as i8;
        m[16] = ((a(24) + a(25)) / 2) as i8;
        m[17] = ((a(26) + a(27)) / 2) as i8;
        m[18] = ((a(28) + a(29) + a(30) + a(31)) / 4) as i8;
        m[19] = ((a(32) + a(33)) / 2) as i8;
    }
}

fn map_idx_10_to_34(m: &mut [i8; MAX_PAR], p: &[i8; MAX_PAR], full: bool) {
    const SRC: [usize; 34] =
        [0, 0, 0, 1, 1, 1, 2, 2, 2, 2, 3, 3, 4, 4, 4, 4, 5, 5, 6, 6, 7, 7, 7, 7, 8, 8, 8, 8, 9, 9, 9, 9, 9, 9];
    let n = if full { 34 } else {
        m[16] = 0;
        16
    };
    for b in 0..n {
        m[b] = p[SRC[b]];
    }
}

fn map_idx_20_to_34(m: &mut [i8; MAX_PAR], p: &[i8; MAX_PAR], full: bool) {
    if full {
        const SRC: [usize; 17] = [11, 12, 13, 14, 14, 15, 15, 16, 16, 17, 17, 18, 18, 18, 18, 19, 19];
        for (i, &s) in SRC.iter().enumerate() {
            m[17 + i] = p[s];
        }
    }
    m[16] = p[10];
    m[15] = p[9];
    m[14] = p[9];
    m[13] = p[8];
    m[12] = p[8];
    m[11] = p[7];
    m[10] = p[6];
    m[9] = p[5];
    m[8] = p[5];
    m[7] = p[4];
    m[6] = p[4];
    m[5] = p[3];
    m[4] = ((p[2] as i32 + p[3] as i32) / 2) as i8;
    m[3] = p[2];
    m[2] = p[1];
    m[1] = ((p[0] as i32 + p[1] as i32) / 2) as i8;
    m[0] = p[0];
}

fn map_val_34_to_20(p: &mut [f32; MAX_PAR]) {
    p[0] = (2.0 * p[0] + p[1]) * (1.0 / 3.0);
    p[1] = (p[1] + 2.0 * p[2]) * (1.0 / 3.0);
    p[2] = (2.0 * p[3] + p[4]) * (1.0 / 3.0);
    p[3] = (p[4] + 2.0 * p[5]) * (1.0 / 3.0);
    p[4] = (p[6] + p[7]) * 0.5;
    p[5] = (p[8] + p[9]) * 0.5;
    p[6] = p[10];
    p[7] = p[11];
    p[8] = (p[12] + p[13]) * 0.5;
    p[9] = (p[14] + p[15]) * 0.5;
    p[10] = p[16];
    p[11] = p[17];
    p[12] = p[18];
    p[13] = p[19];
    p[14] = (p[20] + p[21]) * 0.5;
    p[15] = (p[22] + p[23]) * 0.5;
    p[16] = (p[24] + p[25]) * 0.5;
    p[17] = (p[26] + p[27]) * 0.5;
    p[18] = (p[28] + p[29] + p[30] + p[31]) * 0.25;
    p[19] = (p[32] + p[33]) * 0.5;
}

fn map_val_20_to_34(p: &mut [f32; MAX_PAR]) {
    p[33] = p[19];
    p[32] = p[19];
    p[31] = p[18];
    p[30] = p[18];
    p[29] = p[18];
    p[28] = p[18];
    p[27] = p[17];
    p[26] = p[17];
    p[25] = p[16];
    p[24] = p[16];
    p[23] = p[15];
    p[22] = p[15];
    p[21] = p[14];
    p[20] = p[14];
    p[19] = p[13];
    p[18] = p[12];
    p[17] = p[11];
    p[16] = p[10];
    p[15] = p[9];
    p[14] = p[9];
    p[13] = p[8];
    p[12] = p[8];
    p[11] = p[7];
    p[10] = p[6];
    p[9] = p[5];
    p[8] = p[5];
    p[7] = p[4];
    p[6] = p[4];
    p[5] = p[3];
    p[4] = (p[2] + p[3]) * 0.5;
    p[3] = p[2];
    p[2] = p[1];
    p[1] = (p[0] + p[1]) * 0.5;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::huffman::tests::{assert_complete_prefix_code, assert_round_trip};

    #[test]
    fn ps_books_are_complete() {
        use huff_tables as h;
        let all: [(&[u8], &[u32], usize); 10] = [
            (&h::IID_DF_LENS, &h::IID_DF_CODES, 29),
            (&h::IID_DT_LENS, &h::IID_DT_CODES, 29),
            (&h::IID_FINE_DF_LENS, &h::IID_FINE_DF_CODES, 61),
            (&h::IID_FINE_DT_LENS, &h::IID_FINE_DT_CODES, 61),
            (&h::ICC_DF_LENS, &h::ICC_DF_CODES, 15),
            (&h::ICC_DT_LENS, &h::ICC_DT_CODES, 15),
            (&IPD_DF.0, &IPD_DF.1, 8),
            (&IPD_DT.0, &IPD_DT.1, 8),
            (&OPD_DF.0, &OPD_DF.1, 8),
            (&OPD_DT.0, &OPD_DT.1, 8),
        ];
        for (i, (l, c, n)) in all.iter().enumerate() {
            assert_eq!(l.len(), *n);
            assert_complete_prefix_code(l, c, &format!("ps book {i}"));
            assert_round_trip(l, c, 9);
        }
    }

    /// Neutral parameters (IID 0 dB, ICC 1, phases 0) must reproduce the mono
    /// input in both channels in every band, through hybrid analysis,
    /// decorrelation, mixing and hybrid synthesis.
    fn neutral(is34: bool, ipd: bool) {
        let mut ps = Ps::new();
        ps.start = true;
        ps.enable_iid = true;
        ps.enable_icc = true;
        let n = if is34 { 34 } else { 20 };
        ps.nr_iid_par = n;
        ps.nr_icc_par = n;
        ps.icc_mode = if is34 { 2 } else { 1 };
        ps.is34 = is34;
        ps.is34_old = is34;
        ps.enable_ipdopd = ipd;
        ps.nr_ipdopd_par = if is34 { 17 } else { 11 };
        ps.num_env = 1;
        ps.border = [-1, 31, 0, 0, 0, 0, 0];
        let mut frames = Vec::new();
        for f in 0..3 {
            let mut l = [[C32::default(); 64]; 38];
            for (n, slot) in l.iter_mut().enumerate() {
                for (k, v) in slot.iter_mut().enumerate().take(40) {
                    let t = (f * 32 + n) as f32;
                    *v = C32::new((t * 0.37 + k as f32).sin(), (t * 0.11 * (k + 1) as f32).cos());
                }
            }
            frames.push(l);
        }
        // Slots 32..38 are the look-ahead of the next frame.
        for f in 0..2 {
            let next = frames[f + 1];
            frames[f][32..38].copy_from_slice(&next[..6]);
        }
        for (f, orig) in frames.iter().take(2).enumerate() {
            let mut l = *orig;
            let mut r = [[C32::default(); 64]; 38];
            ps.apply(&mut l, &mut r, 40);
            ps.num_env_old = ps.num_env;
            if f == 0 {
                continue; // parameter interpolation starts from zero history
            }
            for n in 0..32 {
                for k in 0..40 {
                    for (out, name) in [(&l, "L"), (&r, "R")] {
                        let d = out[n][k] - orig[n][k];
                        assert!(d.norm_sqr() < 1e-6, "is34 {is34} ipd {ipd} {name} slot {n} band {k}: {:?} vs {:?}", out[n][k], orig[n][k]);
                    }
                }
            }
        }
    }

    #[test]
    fn neutral_parameters_20_bands() {
        neutral(false, false);
    }

    #[test]
    fn neutral_parameters_34_bands() {
        neutral(true, false);
    }

    #[test]
    fn neutral_parameters_with_ipd_opd() {
        neutral(false, true);
        neutral(true, true);
    }

    struct BitWriter {
        bytes: Vec<u8>,
        n: usize,
    }
    impl BitWriter {
        fn put(&mut self, v: u32, bits: u32) {
            for i in (0..bits).rev() {
                if self.n % 8 == 0 {
                    self.bytes.push(0);
                }
                if (v >> i) & 1 != 0 {
                    *self.bytes.last_mut().unwrap() |= 0x80 >> (self.n % 8);
                }
                self.n += 1;
            }
        }
    }

    #[test]
    fn parse_ps_data_with_ipd_extension() {
        use huff_tables as h;
        let mut w = BitWriter { bytes: Vec::new(), n: 0 };
        w.put(1, 1); // header
        w.put(1, 1); // enable_iid
        w.put(4, 3); // iid_mode 4: 20 bands, fine quantisation
        w.put(1, 1); // enable_icc
        w.put(1, 3); // icc_mode 1
        w.put(1, 1); // enable_ext
        w.put(0, 1); // frame_class
        w.put(1, 2); // num_env = 1
        // IID: df, values +1 then 0 (fine book)
        w.put(0, 1);
        let fi = |v: i32| ((v - h::IID_FINE_DF_MIN) as usize, h::IID_FINE_DF_LENS, h::IID_FINE_DF_CODES);
        for b in 0..20 {
            let (i, l, c) = fi(if b == 0 { 3 } else { 0 });
            w.put(c[i], l[i] as u32);
        }
        // ICC: df, all zero
        w.put(0, 1);
        for _ in 0..20 {
            let i = (0 - h::ICC_DF_MIN) as usize;
            w.put(h::ICC_DF_CODES[i], h::ICC_DF_LENS[i] as u32);
        }
        // Extension: 8 bytes, id 0 (IPD/OPD)
        let ext_start = w.n;
        w.put(8, 4); // cnt bytes
        w.put(0, 2); // ps_extension_id
        w.put(1, 1); // enable_ipdopd
        w.put(0, 1); // ipd df
        for _ in 0..11 {
            w.put(IPD_DF.1[2], IPD_DF.0[2] as u32); // +2 each band
        }
        w.put(0, 1); // opd df
        for _ in 0..11 {
            w.put(OPD_DF.1[0], OPD_DF.0[0] as u32);
        }
        w.put(0, 1); // reserved_ps
        let used = w.n - ext_start - 4;
        assert!(used <= 8 * 8);
        w.put(0, (8 * 8 - used) as u32);
        let total = w.n;
        w.put(0, 7);
        let mut br = BitReader::new(&w.bytes);
        let mut ps = Ps::new();
        let consumed = ps.parse(&mut br, total + 7);
        assert_eq!(consumed, total);
        assert!(ps.start && ps.iid_quant && ps.enable_ipdopd);
        assert_eq!(ps.num_env, 1);
        assert_eq!(ps.iid_par[0][0], 3);
        assert_eq!(ps.iid_par[0][19], 3);
        // IPD accumulates modulo 8 across bands.
        assert_eq!(ps.ipd_par[0][0], 2);
        assert_eq!(ps.ipd_par[0][3], 0);
        assert_eq!(ps.ipd_par[0][4], 2);
        assert_eq!(ps.mode(), (false, true));
        // Truncated data is rejected without consuming more than given.
        let mut ps2 = Ps::new();
        let mut br = BitReader::new(&w.bytes);
        assert_eq!(ps2.parse(&mut br, 20), 20);
        assert!(!ps2.start);
    }

}
