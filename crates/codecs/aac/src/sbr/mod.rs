// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Spectral band replication (HE-AAC v1, ISO/IEC 14496-3 4.6.18): one
//! [`SbrElement`] per SCE/CPE/LFE. The core output is analysed by a 32-band
//! QMF bank, the high band is regenerated (patching + inverse filtering),
//! its envelope adjusted to the transmitted energies with noise and
//! sinusoids, and the 64-band QMF synthesis produces the output at twice the
//! core rate. Mono elements may carry parametric stereo (HE-AAC v2).

mod adjust;
mod bitstream;
mod freq;
mod hf;
mod qmf;
pub(crate) mod tables;

use crate::bits::BitReader;
use crate::config::ElementKind;
use crate::error::Result;
use crate::fft::C32;
use crate::ps::Ps;
use adjust::{AdjustState, Dequant, Work};
use bitstream::SbrChannelData;
use freq::FreqTables;
use hf::{XHigh, XLow, T_HFADJ};
use qmf::{Analysis, Synthesis};

const EXTENSION_ID_PS: u32 = 2;

/// sbr_header() fields.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SbrHeader {
    pub amp_res: u8,
    pub start_freq: u8,
    pub stop_freq: u8,
    pub xover_band: u8,
    pub freq_scale: u8,
    pub alter_scale: u8,
    pub noise_bands: u8,
    pub limiter_bands: u8,
    pub limiter_gains: u8,
    pub interpol_freq: bool,
    pub smoothing_mode: bool,
}

type QmfFrame = [[C32; 64]; 38];

struct SbrChannel {
    data: SbrChannelData,
    dq: Dequant,
    adj: AdjustState,
    bw: [f32; 5],
    analysis: Analysis,
    synthesis: Synthesis,
    /// Analysis output of the current and previous frame.
    w: Box<[[[C32; 32]; 32]; 2]>,
    w_pos: usize,
    /// Adjusted high band of the current and previous frame.
    y: Box<[QmfFrame; 2]>,
    y_pos: usize,
    /// Slots of the previous frame's high band reaching into this frame.
    overhang: usize,
}

impl SbrChannel {
    fn new(bands: usize) -> SbrChannel {
        SbrChannel {
            data: SbrChannelData::default(),
            dq: Dequant::default(),
            adj: AdjustState::default(),
            bw: [0.0; 5],
            analysis: Analysis::new(),
            synthesis: Synthesis::new(bands),
            w: Box::new([[[C32::default(); 32]; 32]; 2]),
            w_pos: 0,
            y: Box::new([[[C32::default(); 64]; 38]; 2]),
            y_pos: 0,
            overhang: 0,
        }
    }
}

pub(crate) struct SbrElement {
    fs: u32,
    downsampled: bool,
    header: SbrHeader,
    have_header: bool,
    tables: Option<FreqTables>,
    /// Frequency tables changed since the last processed frame.
    reset: bool,
    coupling: bool,
    kx_prev: usize,
    m_prev: usize,
    ch: Vec<SbrChannel>,
    ps: Option<Box<Ps>>,
    ps_seen: bool,
    x_low: Box<XLow>,
    x_high: Box<XHigh>,
    alpha0: Box<[C32; 64]>,
    alpha1: Box<[C32; 64]>,
    work: Box<Work>,
    x: Vec<Box<QmfFrame>>,
}

impl SbrElement {
    /// `fs`: SBR (output) sampling rate; `nch`: channels of the element.
    pub fn new(fs: u32, nch: usize, downsampled: bool) -> Result<SbrElement> {
        let bands = if downsampled { 32 } else { 64 };
        Ok(SbrElement {
            fs,
            downsampled,
            header: SbrHeader::default(),
            have_header: false,
            tables: None,
            reset: true,
            coupling: false,
            kx_prev: 32,
            m_prev: 0,
            // A mono element keeps a second synthesis bank for PS output.
            ch: (0..2).map(|_| SbrChannel::new(bands)).collect(),
            ps: if nch == 1 { Some(Box::new(Ps::new())) } else { None },
            ps_seen: false,
            x_low: Box::new([[C32::default(); 40]; 32]),
            x_high: Box::new([[C32::default(); 40]; 64]),
            alpha0: Box::new([C32::default(); 64]),
            alpha1: Box::new([C32::default(); 64]),
            work: Box::new(Work::new()),
            x: (0..2).map(|_| Box::new([[C32::default(); 64]; 38])).collect(),
        })
    }

    pub fn reset(&mut self) {
        let bands = if self.downsampled { 32 } else { 64 };
        self.ch = (0..2).map(|_| SbrChannel::new(bands)).collect();
        self.header = SbrHeader::default();
        self.have_header = false;
        self.tables = None;
        self.reset = true;
        self.kx_prev = 32;
        self.m_prev = 0;
        if let Some(ps) = &mut self.ps {
            ps.reset();
        }
    }

    pub fn ps_seen(&self) -> bool {
        self.ps_seen
    }

    /// Mode of the parametric stereo state when it is running.
    pub fn ps_mode(&self) -> Option<(bool, bool)> {
        self.ps.as_ref().filter(|p| p.start).map(|p| p.mode())
    }

    /// Whether the last channel pair payload used SBR coupling.
    pub fn coupled(&self) -> bool {
        self.coupling
    }

    /// Whether frequency tables (and so SBR data) are available.
    pub fn has_header(&self) -> bool {
        self.have_header && self.tables.is_some()
    }

    fn turn_off(&mut self) {
        self.have_header = false;
        self.tables = None;
        for c in &mut self.ch {
            c.data.e_a[1] = -1;
        }
    }

    /// sbr_extension_data() after the CRC bits.
    pub fn parse(&mut self, br: &mut BitReader, kind: ElementKind, allow_ps: bool) -> Result<()> {
        let r = self.parse_inner(br, kind, allow_ps);
        if r.is_err() {
            self.turn_off();
        }
        r
    }

    fn parse_inner(&mut self, br: &mut BitReader, kind: ElementKind, allow_ps: bool) -> Result<()> {
        if br.read_bit()? {
            let changed = self.header.parse(br)?;
            if changed || self.tables.is_none() {
                self.tables = Some(FreqTables::new(&self.header, self.fs)?);
                self.reset = true;
            }
            self.have_header = true;
        }
        if !self.have_header {
            return Ok(());
        }
        let ft = self.tables.as_ref().expect("tables with header");
        let amp = self.header.amp_res;
        match kind {
            ElementKind::Cpe => {
                if br.read_bit()? {
                    br.skip(8)?; // bs_reserved
                }
                self.coupling = br.read_bit()?;
                let (c0, c1) = self.ch.split_at_mut(1);
                let (d0, d1) = (&mut c0[0].data, &mut c1[0].data);
                if self.coupling {
                    d0.parse_grid(br, amp)?;
                    d1.copy_grid(d0);
                    d0.parse_dtdf(br)?;
                    d1.parse_dtdf(br)?;
                    d0.parse_invf(br, ft.n_q)?;
                    d1.invf_mode[1] = d1.invf_mode[0];
                    d1.invf_mode[0] = d0.invf_mode[0];
                    d0.parse_envelope(br, ft, false)?;
                    d0.parse_noise(br, ft, false)?;
                    d1.parse_envelope(br, ft, true)?;
                    d1.parse_noise(br, ft, true)?;
                } else {
                    d0.parse_grid(br, amp)?;
                    d1.parse_grid(br, amp)?;
                    d0.parse_dtdf(br)?;
                    d1.parse_dtdf(br)?;
                    d0.parse_invf(br, ft.n_q)?;
                    d1.parse_invf(br, ft.n_q)?;
                    d0.parse_envelope(br, ft, false)?;
                    d1.parse_envelope(br, ft, false)?;
                    d0.parse_noise(br, ft, false)?;
                    d1.parse_noise(br, ft, false)?;
                }
                d0.parse_harmonics(br, ft)?;
                d1.parse_harmonics(br, ft)?;
            }
            _ => {
                self.coupling = false;
                if br.read_bit()? {
                    br.skip(4)?; // bs_reserved
                }
                let d = &mut self.ch[0].data;
                d.parse_grid(br, amp)?;
                d.parse_dtdf(br)?;
                d.parse_invf(br, ft.n_q)?;
                d.parse_envelope(br, ft, false)?;
                d.parse_noise(br, ft, false)?;
                d.parse_harmonics(br, ft)?;
            }
        }
        if br.read_bit()? {
            // bs_extended_data
            let mut cnt = br.read_usize(4)?;
            if cnt == 15 {
                cnt += br.read_usize(8)?;
            }
            let mut bits = 8 * cnt;
            while bits > 7 {
                let id = br.read(2)?;
                bits -= 2;
                if id == EXTENSION_ID_PS && allow_ps && self.ps.is_some() {
                    let used = self.ps.as_mut().unwrap().parse(br, bits);
                    bits -= used;
                    self.ps_seen = true;
                } else {
                    br.skip(bits)?;
                    bits = 0;
                }
            }
            br.skip(bits)?;
        }
        Ok(())
    }

    /// Process one frame: `l`/`r` are the 1024-sample core outputs (16-bit
    /// scale); `have_data` whether an SBR payload was parsed this frame.
    /// Writes 2048 (1024 when downsampled) samples into `out_l` / `out_r`.
    /// With `ps`, a mono element renders stereo into `out_l` and `out_r`.
    pub fn process(
        &mut self,
        l: &[f32],
        r: Option<&Vec<f32>>,
        have_data: bool,
        out_l: &mut [f32],
        out_r: Option<&mut Vec<f32>>,
        ps: bool,
    ) -> Result<()> {
        let active = have_data && self.have_header && self.tables.is_some();
        let nch = if r.is_some() { 2 } else { 1 };
        if active {
            let ft = self.tables.as_ref().unwrap();
            if nch == 2 && self.coupling {
                let (c0, c1) = self.ch.split_at_mut(1);
                adjust::dequant_coupled(&c0[0].data, &c1[0].data, ft, &mut c0[0].dq, &mut c1[0].dq);
            } else {
                for c in self.ch.iter_mut().take(nch) {
                    adjust::dequant_single(&c.data, ft, &mut c.dq);
                }
            }
        }
        let (kx, m) = match (&self.tables, active) {
            (Some(ft), true) => (ft.kx, ft.m),
            _ => (32, 0),
        };
        for c in 0..nch {
            let input = if c == 0 { l } else { r.unwrap() };
            let ch = &mut self.ch[c];
            let (wa, wb) = ch.w.split_at_mut(1);
            let (cur, prev) = if ch.w_pos == 0 { (&mut wa[0], &wb[0]) } else { (&mut wb[0], &wa[0]) };
            ch.analysis.run(input, cur);
            hf::lf_gen(&mut self.x_low, cur, prev, kx, self.kx_prev);
            ch.w_pos ^= 1;
            ch.y_pos ^= 1;
            let mut new_overhang = 0;
            if active {
                let ft = self.tables.as_ref().unwrap();
                let d = &ch.data;
                hf::inverse_filter(&self.x_low, ft.k0, &mut self.alpha0, &mut self.alpha1);
                hf::chirp(&mut ch.bw, &d.invf_mode, ft.n_q);
                let (start, end) = (2 * d.t_env[0], 2 * d.t_env[d.num_env]);
                hf::hf_gen(&mut self.x_high, &self.x_low, &self.alpha0, &self.alpha1, &ch.bw, ft, start, end);
                let y = &mut ch.y[ch.y_pos];
                adjust::adjust(&self.header, ft, d, &ch.dq, &mut ch.adj, &mut self.work, &self.x_high, y, self.reset);
                new_overhang = end.saturating_sub(32);
            }
            // Assemble the QMF frame X (x_gen).
            let x = &mut self.x[c];
            let (y_prev, y_cur) = (&ch.y[ch.y_pos ^ 1], &ch.y[ch.y_pos]);
            let i_temp = ch.overhang;
            for (i, slot) in x.iter_mut().enumerate() {
                *slot = [C32::default(); 64];
                if i < i_temp {
                    for k in 0..self.kx_prev {
                        slot[k] = self.x_low[k][i + T_HFADJ];
                    }
                    for k in self.kx_prev..(self.kx_prev + self.m_prev).min(64) {
                        slot[k] = y_prev[i + 32][k];
                    }
                } else {
                    for k in 0..kx {
                        slot[k] = self.x_low[k][i + T_HFADJ];
                    }
                    if i < 32 {
                        for k in kx..(kx + m).min(64) {
                            slot[k] = y_cur[i][k];
                        }
                    }
                }
            }
            ch.overhang = new_overhang;
        }
        let (xa, xb) = self.x.split_at_mut(1);
        if ps && nch == 1 {
            let p = self.ps.as_mut().expect("PS state for mono element");
            if p.start {
                p.apply(&mut xa[0], &mut xb[0], kx + m);
            } else {
                *xb[0] = *xa[0];
            }
        }
        let n = if self.downsampled { 32 } else { 64 };
        self.ch[0].synthesis.run(&xa[0][..], 32, &mut out_l[..32 * n]);
        if let Some(out_r) = out_r {
            let src = if nch == 2 || ps { &xb[0] } else { &xa[0] };
            self.ch[1].synthesis.run(&src[..], 32, &mut out_r[..32 * n]);
        }
        self.kx_prev = kx;
        self.m_prev = m;
        if active {
            self.reset = false;
        }
        Ok(())
    }
}
