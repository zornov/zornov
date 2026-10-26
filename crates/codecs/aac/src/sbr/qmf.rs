// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Complex QMF banks of the SBR tool (ISO/IEC 14496-3 4.6.18.4): 32-band
//! analysis at the core rate and 64-band (or 32-band, downsampled)
//! synthesis, both evaluated through FFTs.
//!
//! Analysis, per slot of 32 new samples (x[0] newest):
//! `z(n) = x(n) c(2n)`, `u(n) = sum_j z(n + 64 j)`,
//! `W(k) = 2 sum_{n<64} u(n) e^{i pi (k + 1/2)(2n - 1/2)/64}`, k < 32.
//!
//! Synthesis (M = 64 bands, or 32 for downsampled SBR): shift v by 2M,
//! `v(n) = 1/64 Re sum_{k<M} X(k) e^{i pi (k + 1/2)(2n - P)/(2M)}`,
//! P = 255 (M = 64) or 127.5 (M = 32),
//! `out(j) = sum_{i<5} v(4Mi + j) c'(2Mi + j) + v(4Mi + 3M + j) c'(2Mi + M + j)`
//! with `c'(n) = c(64 n / M)`.

use crate::fft::{Fft, C32};
use crate::sbr::tables::QMF_C;

/// ISO/IEC 14496-3 Table 4.A.89 coefficient c(n): the smooth prototype with
/// the sign pattern (-1)^floor(n/128) of the standard's polyphase layout.
#[inline]
pub(crate) fn qmf_c(n: usize) -> f32 {
    if (n / 128) % 2 == 1 { -QMF_C[n] } else { QMF_C[n] }
}
use std::f64::consts::PI;
use std::sync::OnceLock;

struct AnalysisConsts {
    fft: Fft,
    /// e^{-i pi n / 64}
    pre: Vec<C32>,
    /// 2 e^{-i pi (k + 1/2) / 128}
    post: Vec<C32>,
    /// c(2n), n < 320
    window: Vec<f32>,
}

fn analysis_consts() -> &'static AnalysisConsts {
    static C: OnceLock<AnalysisConsts> = OnceLock::new();
    C.get_or_init(|| AnalysisConsts {
        fft: Fft::new(64),
        pre: (0..64).map(|n| C32::expi(-PI * n as f64 / 64.0)).collect(),
        post: (0..32).map(|k| C32::expi(-PI * (k as f64 + 0.5) / 128.0).scale(2.0)).collect(),
        window: (0..320).map(|n| qmf_c(2 * n)).collect(),
    })
}

/// 32-band analysis filterbank state of one channel.
#[derive(Clone)]
pub(crate) struct Analysis {
    /// Last 288 input samples followed by the current 1024 (time order).
    buf: Vec<f32>,
}

impl Analysis {
    pub fn new() -> Analysis {
        Analysis { buf: vec![0.0; 288 + 1024] }
    }

    /// Analyse 1024 samples into 32 slots of 32 complex subband samples.
    pub fn run(&mut self, input: &[f32], out: &mut [[C32; 32]; 32]) {
        let c = analysis_consts();
        self.buf.copy_within(1024.., 0);
        self.buf[288..].copy_from_slice(&input[..1024]);
        let mut u = [0f32; 64];
        let mut z = [C32::default(); 64];
        for (l, slot) in out.iter_mut().enumerate() {
            // x(n) = buf[32 l + 319 - n]
            let x = &self.buf[32 * l..32 * l + 320];
            for n in 0..64 {
                let mut acc = 0f32;
                for j in 0..5 {
                    let m = n + 64 * j;
                    acc += x[319 - m] * c.window[m];
                }
                u[n] = acc;
            }
            for n in 0..64 {
                z[n] = c.pre[n].scale(u[n]);
            }
            c.fft.forward(&mut z);
            for k in 0..32 {
                slot[k] = c.post[k] * z[k].conj();
            }
        }
    }
}

struct SynthesisConsts {
    fft: Fft,
    /// X(k) twiddle e^{-i pi (k + 1/2) P/(2M)}
    pre: Vec<C32>,
    /// e^{i pi n/(2M)} / 64
    post: Vec<C32>,
    /// c'(n) = c(64 n / M), n < 10 M
    window: Vec<f32>,
}

fn synthesis_consts(m: usize) -> &'static SynthesisConsts {
    static C64: OnceLock<SynthesisConsts> = OnceLock::new();
    static C32_: OnceLock<SynthesisConsts> = OnceLock::new();
    let cell = if m == 64 { &C64 } else { &C32_ };
    cell.get_or_init(|| {
        let mm = m as f64;
        // 64 bands: (2n - 255)/128. Downsampled 32 bands: (2n - 127.5)/64,
        // the phase that makes the 32-band analysis/synthesis pair
        // reconstruct (verified by `downsampled_bank_reconstructs`).
        let phase = if m == 64 { 255.0 } else { 127.5 };
        SynthesisConsts {
            fft: Fft::new(2 * m),
            pre: (0..m).map(|k| C32::expi(-PI * (k as f64 + 0.5) * phase / (2.0 * mm))).collect(),
            post: (0..2 * m).map(|n| C32::expi(PI * n as f64 / (2.0 * mm)).scale(1.0 / 64.0)).collect(),
            window: (0..10 * m).map(|n| qmf_c(n * 64 / m)).collect(),
        }
    })
}

/// Synthesis filterbank state of one output channel.
#[derive(Clone)]
pub(crate) struct Synthesis {
    m: usize,
    v: Vec<f32>,
    buf: Vec<C32>,
}

impl Synthesis {
    /// `m` = 64 (normal) or 32 (downsampled SBR).
    pub fn new(m: usize) -> Synthesis {
        Synthesis { m, v: vec![0.0; 20 * m], buf: vec![C32::default(); 2 * m] }
    }

    /// Synthesise `slots` time slots of `X[slot][k]` into `out`
    /// (`slots * m` samples).
    pub fn run(&mut self, x: &[[C32; 64]], slots: usize, out: &mut [f32]) {
        let m = self.m;
        let c = synthesis_consts(m);
        for (l, xs) in x.iter().take(slots).enumerate() {
            self.v.copy_within(0..18 * m, 2 * m);
            // conj(FFT(conj(a))) evaluates sum a_k e^{+i 2 pi k n / 2M}
            for k in 0..m {
                self.buf[k] = (xs[k] * c.pre[k]).conj();
            }
            for b in &mut self.buf[m..] {
                *b = C32::default();
            }
            c.fft.forward(&mut self.buf);
            for n in 0..2 * m {
                let s = self.buf[n].conj();
                self.v[n] = (c.post[n] * s).re;
            }
            let o = &mut out[l * m..(l + 1) * m];
            for (j, oj) in o.iter_mut().enumerate() {
                let mut acc = 0f32;
                for i in 0..5 {
                    acc += self.v[4 * m * i + j] * c.window[2 * m * i + j];
                    acc += self.v[4 * m * i + 3 * m + j] * c.window[2 * m * i + m + j];
                }
                *oj = acc;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analysis_matches_direct_formula() {
        let mut a = Analysis::new();
        let input: Vec<f32> = (0..1024).map(|i| ((i * 37 % 101) as f32 - 50.0) / 7.0).collect();
        let mut out = [[C32::default(); 32]; 32];
        a.run(&input, &mut out);
        // Direct evaluation for slot 20 (all history is from `input`).
        let l = 20;
        let mut x = [0f64; 320];
        for n in 0..320 {
            let t = 32 * l as isize + 31 - n as isize; // newest sample of the slot is input[32l+31]
            x[n] = if t >= 0 { input[t as usize] as f64 } else { 0.0 };
        }
        for k in [0usize, 5, 31] {
            let (mut re, mut im) = (0f64, 0f64);
            for n in 0..64 {
                let mut u = 0f64;
                for j in 0..5 {
                    u += x[n + 64 * j] * qmf_c(2 * (n + 64 * j)) as f64;
                }
                let th = PI * (k as f64 + 0.5) * (2.0 * n as f64 - 0.5) / 64.0;
                re += 2.0 * u * th.cos();
                im += 2.0 * u * th.sin();
            }
            assert!((out[l][k].re as f64 - re).abs() < 1e-3, "k {k}: {} vs {re}", out[l][k].re);
            assert!((out[l][k].im as f64 - im).abs() < 1e-3, "k {k}: {} vs {im}", out[l][k].im);
        }
    }

    /// Analysis followed by synthesis with the subbands copied straight
    /// through reconstructs an upsampled, delayed copy of the input.
    #[test]
    fn analysis_synthesis_reconstructs() {
        let mut a = Analysis::new();
        let mut s = Synthesis::new(64);
        let freq = 0.01234f64; // cycles per input sample
        let mut outs = Vec::new();
        let mut w = [[C32::default(); 32]; 32];
        let mut x = vec![[C32::default(); 64]; 32];
        let mut out = vec![0f32; 2048];
        for f in 0..6 {
            let input: Vec<f32> =
                (0..1024).map(|i| (2.0 * PI * freq * (f * 1024 + i) as f64).sin() as f32 * 1000.0).collect();
            a.run(&input, &mut w);
            for l in 0..32 {
                x[l] = [C32::default(); 64];
                x[l][..32].copy_from_slice(&w[l]);
            }
            s.run(&x, 32, &mut out);
            outs.extend_from_slice(&out);
        }
        // Expected: 1000 sin(2 pi (freq/2) (n - d)) for some delay d.
        let tail = &outs[6144..12288];
        let mut best = (f64::MAX, 0usize);
        for d in 0..1500 {
            let mut e = 0f64;
            for (i, &v) in tail.iter().enumerate().step_by(7) {
                let n = (6144 + i) as f64 - d as f64;
                let want = 1000.0 * (2.0 * PI * freq / 2.0 * n).sin();
                e += (v as f64 - want).powi(2);
            }
            if e < best.0 {
                best = (e, d);
            }
        }
        let (mut a, mut b) = (0f64, 0f64);
        for (i, &v) in tail.iter().enumerate() {
            let n = (6144 + i) as f64;
            a += v as f64 * (2.0 * PI * freq / 2.0 * n).sin();
            b += v as f64 * (2.0 * PI * freq / 2.0 * n).cos();
        }
        let amp = 2.0 * (a * a + b * b).sqrt() / tail.len() as f64;
        let phase_delay = (-b).atan2(a) / (2.0 * PI * freq / 2.0);
        assert!((amp / 1000.0 - 1.0).abs() < 0.005, "gain {amp} (phase delay {phase_delay})");
        let n_pts = tail.len().div_ceil(7) as f64;
        let rms_err = (best.0 / n_pts).sqrt();
        assert!(rms_err < 1.0, "delay {} rms error {rms_err}", best.1);
        // 577 output samples of filter delay (ISO/IEC 14496-3 4.6.18.4) plus
        // one sample from the half-sample analysis phase offset.
        assert!((577..=578).contains(&best.1), "QMF analysis + synthesis delay {}", best.1);
    }
}

#[cfg(test)]
mod downsampled_tests {
    use super::*;

    /// 32-band analysis followed by the 32-band (downsampled) synthesis
    /// reconstructs a delayed copy of white noise.
    #[test]
    fn downsampled_bank_reconstructs() {
        let mut a = Analysis::new();
        let mut s = Synthesis::new(32);
        let mut seed = 12345u32;
        let input: Vec<f32> = (0..8192)
            .map(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                (seed >> 8) as f32 / (1u32 << 24) as f32 * 600.0 - 300.0
            })
            .collect();
        let mut w = [[C32::default(); 32]; 32];
        let mut x = vec![[C32::default(); 64]; 32];
        let mut out = vec![0f32; 8192];
        for f in 0..8 {
            a.run(&input[f * 1024..(f + 1) * 1024], &mut w);
            for l in 0..32 {
                x[l][..32].copy_from_slice(&w[l]);
            }
            s.run(&x, 32, &mut out[f * 1024..(f + 1) * 1024]);
        }
        let (mut best_e, mut best_d) = (f64::MAX, 0);
        for d in 250..330 {
            let e: f64 = (2000..8000).map(|i| (out[i] as f64 - input[i - d] as f64).powi(2)).sum();
            if e < best_e {
                best_e = e;
                best_d = d;
            }
        }
        let rms = (best_e / 6000.0).sqrt();
        assert!(rms < 1.0, "delay {best_d}: rms error {rms} (signal rms ~173)");
        assert_eq!(best_d, 289);
    }
}

#[cfg(test)]
mod synthesis_tests {
    use super::*;

    #[test]
    fn synthesis_matches_direct_formula() {
        let mut s = Synthesis::new(64);
        let mut x = vec![[C32::default(); 64]; 1];
        for k in 0..64 {
            x[0][k] = C32::new((k as f32 * 0.37).sin(), (k as f32 * 0.11).cos());
        }
        let mut out = vec![0f32; 64];
        s.run(&x, 1, &mut out);
        // v after one slot: v[0..128] new, rest zero.
        let mut v = vec![0f64; 1280];
        for n in 0..128 {
            let mut acc = 0f64;
            for k in 0..64 {
                let th = PI * (k as f64 + 0.5) * (2.0 * n as f64 - 255.0) / 128.0;
                acc += (x[0][k].re as f64 * th.cos() - x[0][k].im as f64 * th.sin()) / 64.0;
            }
            v[n] = acc;
        }
        for j in 0..64 {
            let mut acc = 0f64;
            for i in 0..5 {
                acc += v[256 * i + j] * qmf_c(128 * i + j) as f64;
                acc += v[256 * i + 192 + j] * qmf_c(128 * i + 64 + j) as f64;
            }
            assert!((out[j] as f64 - acc).abs() < 1e-5, "j {j}: {} vs {acc}", out[j]);
        }
    }
}
