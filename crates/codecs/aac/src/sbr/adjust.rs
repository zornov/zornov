// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! HF adjustment (ISO/IEC 14496-3 4.6.18.7): envelope and noise floor
//! dequantisation (4.6.18.3.5), mapping, current-envelope estimation, gain
//! calculation with limiter and boost, gain smoothing and the addition of
//! noise floor and sinusoids.

use crate::fft::C32;
use crate::sbr::bitstream::SbrChannelData;
use crate::sbr::freq::FreqTables;
use crate::sbr::hf::{XHigh, T_HFADJ};
use crate::sbr::tables::NOISE_TABLE;
use crate::sbr::SbrHeader;

/// Dequantised envelope and noise floor values of one channel.
#[derive(Clone)]
pub(crate) struct Dequant {
    pub env: [[f32; 48]; 6],
    pub noise: [[f32; 5]; 3],
}

impl Default for Dequant {
    fn default() -> Self {
        Dequant { env: [[0.0; 48]; 6], noise: [[0.0; 5]; 3] }
    }
}

const NOISE_FLOOR_OFFSET: f32 = 6.0;

pub(crate) fn dequant_single(d: &SbrChannelData, ft: &FreqTables, out: &mut Dequant) {
    let alpha = if d.amp_res == 1 { 1.0 } else { 0.5 };
    for e in 1..=d.num_env {
        for k in 0..ft.n[d.freq_res[e] as usize] {
            let v = (alpha * d.env_q[e][k] as f32 + 6.0).exp2();
            // Energies beyond 1e20 are not meaningful (as in the reference
            // decoders): neutralise instead of overflowing later sums.
            out.env[e][k] = if v > 1e20 { 1.0 } else { v };
        }
    }
    for e in 1..=d.num_noise {
        for k in 0..ft.n_q {
            out.noise[e][k] = (NOISE_FLOOR_OFFSET - d.noise_q[e][k] as f32).exp2();
        }
    }
}

pub(crate) fn dequant_coupled(l: &SbrChannelData, r: &SbrChannelData, ft: &FreqTables, ol: &mut Dequant, or: &mut Dequant) {
    let alpha = if l.amp_res == 1 { 1.0 } else { 0.5 };
    let pan = if l.amp_res == 1 { 12.0 } else { 24.0 };
    for e in 1..=l.num_env {
        for k in 0..ft.n[l.freq_res[e] as usize] {
            let t1 = (l.env_q[e][k] as f32 * alpha + 7.0).exp2();
            let t1 = if t1 > 1e20 { 1.0 } else { t1 };
            let t2 = ((pan - r.env_q[e][k] as f32) * alpha).exp2();
            let fac = t1 / (1.0 + t2);
            ol.env[e][k] = fac;
            or.env[e][k] = fac * t2;
        }
    }
    for e in 1..=l.num_noise {
        for k in 0..ft.n_q {
            let t1 = (NOISE_FLOOR_OFFSET - l.noise_q[e][k] as f32 + 1.0).exp2();
            let t2 = (12.0 - r.noise_q[e][k] as f32).exp2();
            let fac = t1 / (1.0 + t2);
            ol.noise[e][k] = fac;
            or.noise[e][k] = fac * t2;
        }
    }
}

/// Persistent state of the adjuster for one channel.
#[derive(Clone)]
pub(crate) struct AdjustState {
    /// Sinusoid flags of the previous frame's last envelope (row 0).
    pub s_indexmapped: [[u8; 48]; 7],
    pub g_temp: [[f32; 48]; 42],
    pub q_temp: [[f32; 48]; 42],
    pub f_indexnoise: usize,
    pub f_indexsine: usize,
}

impl Default for AdjustState {
    fn default() -> Self {
        AdjustState {
            s_indexmapped: [[0; 48]; 7],
            g_temp: [[0.0; 48]; 42],
            q_temp: [[0.0; 48]; 42],
            f_indexnoise: 0,
            f_indexsine: 0,
        }
    }
}

/// Scratch arrays of one adjustment pass.
pub(crate) struct Work {
    e_origmapped: [[f32; 48]; 5],
    q_mapped: [[f32; 48]; 5],
    s_mapped: [[bool; 48]; 5],
    e_curr: [[f32; 48]; 5],
    gain: [[f32; 48]; 5],
    q_m: [[f32; 48]; 5],
    s_m: [[f32; 48]; 5],
}

impl Work {
    pub fn new() -> Work {
        Work {
            e_origmapped: [[0.0; 48]; 5],
            q_mapped: [[0.0; 48]; 5],
            s_mapped: [[false; 48]; 5],
            e_curr: [[0.0; 48]; 5],
            gain: [[0.0; 48]; 5],
            q_m: [[0.0; 48]; 5],
            s_m: [[0.0; 48]; 5],
        }
    }
}

/// Run mapping, estimation, gain calculation and assembly; writes the
/// adjusted high band into `y[slot][kx..kx+m]` for slots
/// `2 t_env[0] .. 2 t_env[L]`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn adjust(
    h: &SbrHeader,
    ft: &FreqTables,
    d: &SbrChannelData,
    dq: &Dequant,
    st: &mut AdjustState,
    w: &mut Work,
    x_high: &XHigh,
    y: &mut [[C32; 64]; 38],
    reset: bool,
) {
    let kx = ft.kx;
    let m_max = ft.m;
    let e_a = d.e_a;

    // Mapping (4.6.18.7.2).
    st.s_indexmapped[1..].iter_mut().for_each(|r| *r = [0; 48]);
    for e in 0..d.num_env {
        let res = d.freq_res[e + 1] as usize;
        let table = if res == 1 { &ft.f_high } else { &ft.f_low };
        for i in 0..ft.n[res] {
            for m in table[i]..table[i + 1] {
                w.e_origmapped[e][m - kx] = dq.env[e + 1][i];
            }
        }
        let k = if d.num_noise > 1 && d.t_env[e] >= d.t_q[1] { 1 } else { 0 };
        for i in 0..ft.n_q {
            for m in ft.f_noise[i]..ft.f_noise[i + 1] {
                w.q_mapped[e][m - kx] = dq.noise[k + 1][i];
            }
        }
        if d.add_harmonic_flag {
            for i in 0..ft.n[1] {
                let mid = (ft.f_high[i] + ft.f_high[i + 1]) >> 1;
                let present = (e as i32 >= e_a[1] || st.s_indexmapped[0][mid - kx] == 1) as u8;
                st.s_indexmapped[e + 1][mid - kx] = d.add_harmonic[i] as u8 * present;
            }
        }
        for i in 0..ft.n[res] {
            let any = (table[i]..table[i + 1]).any(|m| st.s_indexmapped[e + 1][m - kx] != 0);
            for m in table[i]..table[i + 1] {
                w.s_mapped[e][m - kx] = any;
            }
        }
    }
    st.s_indexmapped[0] = st.s_indexmapped[d.num_env];

    // Estimation of the current envelope (4.6.18.7.3).
    for e in 0..d.num_env {
        let ilb = 2 * d.t_env[e] + T_HFADJ;
        let iub = 2 * d.t_env[e + 1] + T_HFADJ;
        if h.interpol_freq {
            let recip = 1.0 / (iub - ilb) as f32;
            for m in 0..m_max {
                let s: f32 = x_high[m + kx][ilb..iub].iter().map(|v| v.norm_sqr()).sum();
                w.e_curr[e][m] = s * recip;
            }
        } else {
            let res = d.freq_res[e + 1] as usize;
            let table = if res == 1 { &ft.f_high } else { &ft.f_low };
            for p in 0..ft.n[res] {
                let mut s = 0f32;
                for k in table[p]..table[p + 1] {
                    s += x_high[k][ilb..iub].iter().map(|v| v.norm_sqr()).sum::<f32>();
                }
                let den = ((iub - ilb) * (table[p + 1] - table[p])) as f32;
                for k in table[p]..table[p + 1] {
                    w.e_curr[e][k - kx] = s / den;
                }
            }
        }
    }

    // Gain calculation (4.6.18.7.4).
    const LIM_GAIN: [f32; 4] = [0.70795, 1.0, 1.41254, 10000000000.0];
    for e in 0..d.num_env {
        // Bands outside the limiter table (unpatched tail) stay silent.
        w.gain[e] = [0.0; 48];
        w.q_m[e] = [0.0; 48];
        w.s_m[e] = [0.0; 48];
        let delta = !(e as i32 == e_a[1] || e as i32 == e_a[0]);
        for k in 0..ft.n_lim {
            let lo = ft.f_lim[k] - kx;
            let hi = ft.f_lim[k + 1] - kx;
            for m in lo..hi {
                let eo = w.e_origmapped[e][m];
                let qm = w.q_mapped[e][m];
                let temp = eo / (1.0 + qm);
                w.q_m[e][m] = (temp * qm).sqrt();
                w.s_m[e][m] = (temp * st.s_indexmapped[e + 1][m] as f32).sqrt();
                let g = if !w.s_mapped[e][m] {
                    (eo / ((1.0 + w.e_curr[e][m]) * (1.0 + qm * delta as u8 as f32))).sqrt()
                } else {
                    (eo * qm / ((1.0 + w.e_curr[e][m]) * (1.0 + qm))).sqrt()
                };
                w.gain[e][m] = g + f32::MIN_POSITIVE;
            }
            let (mut s0, mut s1) = (0f32, 0f32);
            for m in lo..hi {
                s0 += w.e_origmapped[e][m];
                s1 += w.e_curr[e][m];
            }
            let gain_max = (LIM_GAIN[h.limiter_gains as usize]
                * ((f32::EPSILON + s0) / (f32::EPSILON + s1)).sqrt())
            .min(100000.0);
            for m in lo..hi {
                let q_m_max = w.q_m[e][m] * gain_max / w.gain[e][m];
                w.q_m[e][m] = w.q_m[e][m].min(q_m_max);
                w.gain[e][m] = w.gain[e][m].min(gain_max);
            }
            let (mut s0, mut s1) = (0f32, 0f32);
            for m in lo..hi {
                s0 += w.e_origmapped[e][m];
                s1 += w.e_curr[e][m] * w.gain[e][m] * w.gain[e][m]
                    + w.s_m[e][m] * w.s_m[e][m]
                    + if delta && w.s_m[e][m] == 0.0 { w.q_m[e][m] * w.q_m[e][m] } else { 0.0 };
            }
            let boost = ((f32::EPSILON + s0) / (f32::EPSILON + s1)).sqrt().min(1.584893192);
            for m in lo..hi {
                w.gain[e][m] *= boost;
                w.q_m[e][m] *= boost;
                w.s_m[e][m] *= boost;
            }
        }
    }

    // Assembly (4.6.18.7.5).
    const H_SMOOTH: [f32; 5] = [0.33333333333333, 0.30150283239582, 0.21816949906249, 0.11516383427084, 0.03183050093751];
    let h_sl = if h.smoothing_mode { 0 } else { 4 };
    let t0 = 2 * d.t_env[0];
    if reset {
        for i in 0..h_sl {
            st.g_temp[i + t0][..m_max].copy_from_slice(&w.gain[0][..m_max]);
            st.q_temp[i + t0][..m_max].copy_from_slice(&w.q_m[0][..m_max]);
        }
    } else if h_sl > 0 {
        let old = 2 * d.t_env_num_env_old;
        for i in 0..4 {
            st.g_temp[i + t0] = st.g_temp[i + old];
            st.q_temp[i + t0] = st.q_temp[i + old];
        }
    }
    for e in 0..d.num_env {
        for i in 2 * d.t_env[e]..2 * d.t_env[e + 1] {
            st.g_temp[h_sl + i][..m_max].copy_from_slice(&w.gain[e][..m_max]);
            st.q_temp[h_sl + i][..m_max].copy_from_slice(&w.q_m[e][..m_max]);
        }
    }
    let mut indexnoise = st.f_indexnoise;
    let mut indexsine = st.f_indexsine;
    let mut g_filt = [0f32; 48];
    let mut q_filt = [0f32; 48];
    for e in 0..d.num_env {
        let transient = e as i32 == e_a[0] || e as i32 == e_a[1];
        for i in 2 * d.t_env[e]..2 * d.t_env[e + 1] {
            if h_sl > 0 && !transient {
                for m in 0..m_max {
                    let idx = i + h_sl;
                    let (mut g, mut q) = (0f32, 0f32);
                    for (j, hs) in H_SMOOTH.iter().enumerate() {
                        g += st.g_temp[idx - j][m] * hs;
                        q += st.q_temp[idx - j][m] * hs;
                    }
                    g_filt[m] = g;
                    q_filt[m] = q;
                }
            } else {
                g_filt[..m_max].copy_from_slice(&st.g_temp[i + h_sl][..m_max]);
                q_filt[..m_max].copy_from_slice(&st.q_temp[i + h_sl][..m_max]);
            }
            let yi = &mut y[i];
            for m in 0..m_max {
                yi[m + kx] = x_high[m + kx][i + T_HFADJ].scale(g_filt[m]);
            }
            // Sinusoid phase: phi_re = {1, 0, -1, 0}, phi_im = {0, 1, 0, -1},
            // the imaginary part alternating with (-1)^(k).
            let (phi_re, phi_im) = [(1.0f32, 0.0f32), (0.0, 1.0), (-1.0, 0.0), (0.0, -1.0)][indexsine];
            let mut sign_im = if kx & 1 == 1 { -phi_im } else { phi_im };
            let mut noise = indexnoise;
            for m in 0..m_max {
                noise = (noise + 1) & 0x1ff;
                let sm = w.s_m[e][m];
                if sm != 0.0 {
                    yi[m + kx].re += sm * phi_re;
                    yi[m + kx].im += sm * sign_im;
                } else if !transient {
                    let nz = NOISE_TABLE[noise];
                    yi[m + kx].re += q_filt[m] * nz[0];
                    yi[m + kx].im += q_filt[m] * nz[1];
                }
                sign_im = -sign_im;
            }
            indexnoise = (indexnoise + m_max) & 0x1ff;
            indexsine = (indexsine + 1) & 3;
        }
    }
    st.f_indexnoise = indexnoise;
    st.f_indexsine = indexsine;
}
