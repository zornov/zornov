// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! HF generation (ISO/IEC 14496-3 4.6.18.6): low band extraction, the
//! covariance-method LPC inverse filtering (4.6.18.6.2), chirp factors and
//! patching of the high band.

use crate::fft::C32;
use crate::sbr::freq::FreqTables;

/// t_HFGen: slots of the previous frame kept in front of X_low.
pub(crate) const T_HFGEN: usize = 8;
/// t_HFAdj: offset of the envelope adjuster in QMF slots.
pub(crate) const T_HFADJ: usize = 2;

/// `X_low[k][0..40]`, 8 history slots then 32 current slots.
pub(crate) type XLow = [[C32; 40]; 32];
/// `X_high[k][0..40]`.
pub(crate) type XHigh = [[C32; 40]; 64];

/// Build X_low from the current (`w_cur`) and previous (`w_prev`) analysis
/// output. History bands use the previous frame's `kx_prev`.
pub(crate) fn lf_gen(x_low: &mut XLow, w_cur: &[[C32; 32]; 32], w_prev: &[[C32; 32]; 32], kx: usize, kx_prev: usize) {
    for row in x_low.iter_mut() {
        *row = [C32::default(); 40];
    }
    for k in 0..kx.min(32) {
        for i in 0..32 {
            x_low[k][i + T_HFGEN] = w_cur[i][k];
        }
    }
    for k in 0..kx_prev.min(32) {
        for i in 0..T_HFGEN {
            x_low[k][i] = w_prev[i + 32 - T_HFGEN][k];
        }
    }
}

/// Second-order complex LPC coefficients alpha0, alpha1 per low band
/// (covariance method). Accumulated in double precision: for strongly tonal
/// bands the covariance matrix is nearly singular.
pub(crate) fn inverse_filter(x_low: &XLow, k0: usize, alpha0: &mut [C32; 64], alpha1: &mut [C32; 64]) {
    #[derive(Clone, Copy, Default)]
    struct C64 {
        re: f64,
        im: f64,
    }
    // conj(a) * b
    let cmul = |a: C32, b: C32| C64 {
        re: a.re as f64 * b.re as f64 + a.im as f64 * b.im as f64,
        im: a.re as f64 * b.im as f64 - a.im as f64 * b.re as f64,
    };
    for k in 0..k0.min(32) {
        let x = &x_low[k];
        let nrg = |v: C32| v.re as f64 * v.re as f64 + v.im as f64 * v.im as f64;
        let mut sum0 = 0f64;
        for v in &x[1..38] {
            sum0 += nrg(*v);
        }
        let r22 = sum0 + nrg(x[0]); // phi(2,2)
        let r11 = sum0 + nrg(x[38]); // phi(1,1)
        let mut s1 = C64::default();
        let mut s2 = C64::default();
        for i in 1..38 {
            let a = cmul(x[i], x[i + 1]);
            s1.re += a.re;
            s1.im += a.im;
            let b = cmul(x[i], x[i + 2]);
            s2.re += b.re;
            s2.im += b.im;
        }
        let h1 = cmul(x[0], x[1]);
        let t1 = cmul(x[38], x[39]);
        let h2 = cmul(x[0], x[2]);
        let phi_1_2 = C64 { re: s1.re + h1.re, im: s1.im + h1.im };
        let phi_0_1 = C64 { re: s1.re + t1.re, im: s1.im + t1.im };
        let phi_0_2 = C64 { re: s2.re + h2.re, im: s2.im + h2.im };
        let dk = r22 * r11 - (phi_1_2.re * phi_1_2.re + phi_1_2.im * phi_1_2.im) / 1.000001;
        let mut a1 = C64::default();
        if dk != 0.0 {
            a1 = C64 {
                re: (phi_0_1.re * phi_1_2.re - phi_0_1.im * phi_1_2.im - phi_0_2.re * r11) / dk,
                im: (phi_0_1.re * phi_1_2.im + phi_0_1.im * phi_1_2.re - phi_0_2.im * r11) / dk,
            };
        }
        let mut a0 = C64::default();
        if r11 != 0.0 {
            a0 = C64 {
                re: -(phi_0_1.re + a1.re * phi_1_2.re + a1.im * phi_1_2.im) / r11,
                im: -(phi_0_1.im + a1.im * phi_1_2.re - a1.re * phi_1_2.im) / r11,
            };
        }
        let (mut a0, mut a1) = (C32::new(a0.re as f32, a0.im as f32), C32::new(a1.re as f32, a1.im as f32));
        if a1.norm_sqr() >= 16.0 || a0.norm_sqr() >= 16.0 {
            a0 = C32::default();
            a1 = C32::default();
        }
        alpha0[k] = a0;
        alpha1[k] = a1;
    }
}

/// Chirp factors per noise band from the inverse filtering modes.
pub(crate) fn chirp(bw: &mut [f32; 5], invf: &[[u8; 5]; 2], n_q: usize) {
    const BW_TAB: [f32; 4] = [0.0, 0.75, 0.9, 0.98];
    for i in 0..n_q {
        let mut new_bw = if invf[0][i] + invf[1][i] == 1 { 0.6 } else { BW_TAB[invf[0][i] as usize] };
        if new_bw < bw[i] {
            new_bw = 0.75 * new_bw + 0.25 * bw[i];
        } else {
            new_bw = 0.90625 * new_bw + 0.09375 * bw[i];
        }
        bw[i] = if new_bw < 0.015625 { 0.0 } else { new_bw };
    }
}

/// Patch the low band into the high band with the LPC whitening filter.
#[allow(clippy::too_many_arguments)]
pub(crate) fn hf_gen(
    x_high: &mut XHigh,
    x_low: &XLow,
    alpha0: &[C32; 64],
    alpha1: &[C32; 64],
    bw: &[f32; 5],
    ft: &FreqTables,
    start: usize,
    end: usize,
) {
    let mut k = ft.kx;
    for row in x_high[ft.kx..(ft.kx + ft.m).min(64)].iter_mut() {
        *row = [C32::default(); 40];
    }
    let mut g = 0usize;
    for j in 0..ft.num_patches {
        for x in 0..ft.patch_num_subbands[j] {
            let p = ft.patch_start_subband[j] + x;
            while g < ft.n_q && k >= ft.f_noise[g + 1] {
                g += 1;
            }
            let b = bw[g.min(ft.n_q - 1)];
            let a = alpha1[p].scale(b * b);
            let c = alpha0[p].scale(b);
            let src = &x_low[p];
            let dst = &mut x_high[k];
            for i in start + T_HFADJ..end + T_HFADJ {
                dst[i] = src[i] + c * src[i - 1] + a * src[i - 2];
            }
            k += 1;
        }
    }
}
