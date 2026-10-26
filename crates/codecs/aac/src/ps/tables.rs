// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Parametric stereo tables (ISO/IEC 14496-3 8.6.4): hybrid filter
//! prototypes, band mappings, fractional-delay phase factors, dequantisation
//! and mixing matrices. Everything except the prototypes is computed from
//! the formulas of the standard.

use crate::fft::C32;
use std::f64::consts::PI;
use std::sync::OnceLock;

/// Prototype taps 0..=6 (symmetric around tap 6) of the 8-band filter.
pub(crate) const G0_Q8: [f64; 7] = [
    0.00746082949812,
    0.02270420949825,
    0.04546865930473,
    0.07266113929591,
    0.09885108575264,
    0.11793710567217,
    0.125,
];
/// 12-band prototype (34-band mode, QMF band 0).
pub(crate) const G0_Q12: [f64; 7] = [
    0.04081179924692,
    0.03812810994926,
    0.05144908135699,
    0.06399831151592,
    0.07428313801106,
    0.08100347892914,
    0.08333333333333,
];
/// 4-band prototype (34-band mode, QMF bands 2..=4).
pub(crate) const G0_Q4: [f64; 7] =
    [-0.05908211155639, -0.04871498374946, 0.0, 0.07778723915851, 0.16486303567403, 0.23279856662996, 0.25];
/// Real 2-band filter (20-band mode, QMF bands 1 and 2).
pub(crate) const G1_Q2: [f32; 7] = [0.0, 0.01899487526049, 0.0, -0.07293139167538, 0.0, 0.30596630545168, 0.5];

pub(crate) const NR_PAR_BANDS: [usize; 2] = [20, 34];
pub(crate) const NR_BANDS: [usize; 2] = [71, 91];
pub(crate) const NR_ALLPASS_BANDS: [usize; 2] = [30, 50];
pub(crate) const SHORT_DELAY_BAND: [usize; 2] = [42, 62];
pub(crate) const NR_IPDOPD_BANDS: [usize; 2] = [11, 17];
pub(crate) const DECAY_CUTOFF: [usize; 2] = [10, 32];
pub(crate) const PS_AP_LINKS: usize = 3;
pub(crate) const PS_MAX_DELAY: usize = 14;
pub(crate) const PS_MAX_AP_DELAY: usize = 5;
/// Decay coefficients of the three all-pass links.
pub(crate) const AP_A: [f32; 3] = [0.65143905753106, 0.56471812200776, 0.48954165955695];

/// Hybrid/QMF band -> parameter band, 20-band mode (3GPP groupBorders /
/// bins2groupMap): 10 hybrid bands then QMF bands 3..=63.
fn k_to_i_20() -> [u8; 71] {
    let mut t = [0u8; 71];
    t[..10].copy_from_slice(&[1, 0, 0, 1, 2, 3, 4, 5, 6, 7]);
    // QMF group borders 3,4,5,6,7,8,9,11,14,18,23,35,64 -> bins 8..=19
    let borders = [3usize, 4, 5, 6, 7, 8, 9, 11, 14, 18, 23, 35, 64];
    for g in 0..12 {
        for b in borders[g]..borders[g + 1] {
            t[b + 7] = (8 + g) as u8;
        }
    }
    t
}

/// Hybrid/QMF band -> parameter band, 34-band mode (Table 8.49).
fn k_to_i_34() -> [u8; 91] {
    let head: [u8; 68] = [
        0, 1, 2, 3, 4, 5, 6, 6, 7, 2, 1, 0, 10, 10, 4, 5, 6, 7, 8, 9, 10, 11, 12, 9, 14, 11, 12, 13, 14, 15, 16, 13, 16,
        17, 18, 19, 20, 21, 22, 22, 23, 23, 24, 24, 25, 25, 26, 26, 27, 27, 27, 28, 28, 28, 29, 29, 29, 30, 30, 30, 31, 31,
        31, 31, 32, 32, 32, 32,
    ];
    let mut t = [33u8; 91];
    t[..68].copy_from_slice(&head);
    t
}

/// Centre frequencies of the hybrid sub-bands (x8 for 20 bands, x24 for 34).
const F_CENTER_20: [i32; 10] = [-3, -1, 1, 3, 5, 7, 10, 14, 18, 22];
const F_CENTER_34: [i32; 32] = [
    2, 6, 10, 14, 18, 22, 26, 30, 34, -10, -6, -2, 51, 57, 15, 21, 27, 33, 39, 45, 54, 66, 78, 42, 102, 66, 78, 90, 102,
    114, 126, 90,
];

pub(crate) struct PsTables {
    pub k_to_i_20: [u8; 71],
    pub k_to_i_34: [u8; 91],
    /// Complex hybrid filters `[band][tap]`, 13 taps.
    pub f20_0_8: Vec<[C32; 13]>,
    pub f34_0_12: Vec<[C32; 13]>,
    pub f34_1_8: Vec<[C32; 13]>,
    pub f34_2_4: Vec<[C32; 13]>,
    /// [is34][band]
    pub phi_fract: [Vec<C32>; 2],
    /// [is34][band][link]
    pub q_fract_allpass: [Vec<[C32; 3]>; 2],
    /// [iid 0..46][icc 0..8] -> (h11, h12, h21, h22); mixing A and B.
    pub ha: Vec<[[f32; 4]; 8]>,
    pub hb: Vec<[[f32; 4]; 8]>,
    pub pd_re_smooth: Vec<f32>,
    pub pd_im_smooth: Vec<f32>,
}

fn make_filters(proto: &[f64; 7], bands: usize) -> Vec<[C32; 13]> {
    (0..bands)
        .map(|q| {
            let mut f = [C32::default(); 13];
            for (n, v) in f.iter_mut().enumerate() {
                let p = proto[n.min(12 - n)];
                let theta = 2.0 * PI * (q as f64 + 0.5) * (n as f64 - 6.0) / bands as f64;
                *v = C32::new((p * theta.cos()) as f32, (-p * theta.sin()) as f32);
            }
            f
        })
        .collect()
}

pub(crate) fn tables() -> &'static PsTables {
    static T: OnceLock<PsTables> = OnceLock::new();
    T.get_or_init(|| {
        const LINKS: [f64; 3] = [0.43, 0.75, 0.347];
        const GAIN: f64 = 0.39;
        let mut phi_fract: [Vec<C32>; 2] = [Vec::new(), Vec::new()];
        let mut q_fract: [Vec<[C32; 3]>; 2] = [Vec::new(), Vec::new()];
        for is34 in 0..2 {
            for k in 0..NR_ALLPASS_BANDS[is34] {
                let f_center = if is34 == 0 {
                    if k < 10 { F_CENTER_20[k] as f64 * 0.125 } else { k as f64 + 0.5 - 7.0 }
                } else if k < 32 {
                    F_CENTER_34[k] as f64 / 24.0
                } else {
                    k as f64 + 0.5 - 27.0
                };
                let mut q = [C32::default(); 3];
                for (m, v) in q.iter_mut().enumerate() {
                    *v = C32::expi(-PI * LINKS[m] * f_center);
                }
                q_fract[is34].push(q);
                phi_fract[is34].push(C32::expi(-PI * GAIN * f_center));
            }
        }
        // IID quantisation steps in dB (Table 8.25 default, 8.26 fine).
        const IID_DB: [f64; 15] = [-25., -18., -14., -10., -7., -4., -2., 0., 2., 4., 7., 10., 14., 18., 25.];
        const IID_FINE_DB: [f64; 31] = [
            -50., -45., -40., -35., -30., -25., -22., -19., -16., -13., -10., -8., -6., -4., -2., 0., 2., 4., 6., 8.,
            10., 13., 16., 19., 22., 25., 30., 35., 40., 45., 50.,
        ];
        const ICC_INVQ: [f64; 8] = [1., 0.937, 0.84118, 0.60092, 0.36764, 0., -0.589, -1.];
        let iid_c: Vec<f64> = IID_DB.iter().chain(IID_FINE_DB.iter()).map(|db| 10f64.powf(db / 20.0)).collect();
        let mut ha = Vec::new();
        let mut hb = Vec::new();
        for &c in &iid_c {
            let c1 = 2f64.sqrt() / (1.0 + c * c).sqrt();
            let c2 = c * c1;
            let mut a = [[0f32; 4]; 8];
            let mut b = [[0f32; 4]; 8];
            for icc in 0..8 {
                let alpha = 0.5 * ICC_INVQ[icc].clamp(-1.0, 1.0).acos();
                let beta = alpha * (c1 - c2) * std::f64::consts::FRAC_1_SQRT_2;
                a[icc] = [
                    (c2 * (beta + alpha).cos()) as f32,
                    (c1 * (beta - alpha).cos()) as f32,
                    (c2 * (beta + alpha).sin()) as f32,
                    (c1 * (beta - alpha).sin()) as f32,
                ];
                let rho = ICC_INVQ[icc].max(0.05);
                let mut alpha = 0.5 * (2.0 * c * rho).atan2(c * c - 1.0);
                let mu = c + 1.0 / c;
                let mu = (1.0 + (4.0 * rho * rho - 4.0) / (mu * mu)).sqrt();
                let gamma = ((1.0 - mu) / (1.0 + mu)).sqrt().atan();
                if alpha < 0.0 {
                    alpha += PI / 2.0;
                }
                let s2 = 2f64.sqrt();
                b[icc] = [
                    (s2 * alpha.cos() * gamma.cos()) as f32,
                    (s2 * alpha.sin() * gamma.cos()) as f32,
                    (-s2 * alpha.sin() * gamma.sin()) as f32,
                    (s2 * alpha.cos() * gamma.sin()) as f32,
                ];
            }
            ha.push(a);
            hb.push(b);
        }
        let ipd_cos: Vec<f64> = (0..8).map(|i| (PI / 4.0 * i as f64).cos()).collect();
        let ipd_sin: Vec<f64> = (0..8).map(|i| (PI / 4.0 * i as f64).sin()).collect();
        let mut pd_re = vec![0f32; 512];
        let mut pd_im = vec![0f32; 512];
        for p0 in 0..8 {
            for p1 in 0..8 {
                for p2 in 0..8 {
                    let re = 0.25 * ipd_cos[p0] + 0.5 * ipd_cos[p1] + ipd_cos[p2];
                    let im = 0.25 * ipd_sin[p0] + 0.5 * ipd_sin[p1] + ipd_sin[p2];
                    let mag = 1.0 / (re * re + im * im).sqrt();
                    pd_re[p0 * 64 + p1 * 8 + p2] = (re * mag) as f32;
                    pd_im[p0 * 64 + p1 * 8 + p2] = (im * mag) as f32;
                }
            }
        }
        PsTables {
            k_to_i_20: k_to_i_20(),
            k_to_i_34: k_to_i_34(),
            f20_0_8: make_filters(&G0_Q8, 8),
            f34_0_12: make_filters(&G0_Q12, 12),
            f34_1_8: make_filters(&G0_Q8, 8),
            f34_2_4: make_filters(&G0_Q4, 4),
            phi_fract,
            q_fract_allpass: q_fract,
            ha,
            hb,
            pd_re_smooth: pd_re,
            pd_im_smooth: pd_im,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_factors_match_3gpp_tables() {
        let t = tables();
        // aFractDelayPhaseFactorReQmf / ImQmf start at QMF band 3 (k = 10).
        let re = [-0.411514f32, 0.718126, 0.898028, -0.109734];
        let im = [0.911403f32, 0.695913, -0.439939, -0.993961];
        for i in 0..4 {
            let v = t.phi_fract[0][10 + i];
            assert!((v.re - re[i]).abs() < 1e-5 && (v.im - im[i]).abs() < 1e-5, "{i}: {v:?}");
        }
        // Serial link 1 (delay fraction 0.75) for QMF band 3.
        let v = t.q_fract_allpass[0][10][1];
        assert!((v.re + 0.382683).abs() < 1e-5 && (v.im + 0.92388).abs() < 1e-5, "{v:?}");
        let v = t.q_fract_allpass[0][10][0];
        assert!((v.re - 0.015707).abs() < 1e-5 && (v.im - 0.999877).abs() < 1e-5, "{v:?}");
        // Hybrid band with f_center 1/8: aFractDelayPhaseFactorReSubQmf[0].
        let v = t.phi_fract[0][2];
        assert!((v.re - 0.988295).abs() < 1e-5 && (v.im + 0.152555).abs() < 1e-5);
    }

    #[test]
    fn mixing_matrix_identity_cases() {
        let t = tables();
        // IID 0 dB (index 7), ICC 1 (index 0): L = R = s.
        let h = t.ha[7][0];
        assert!((h[0] - 1.0).abs() < 1e-6 && (h[1] - 1.0).abs() < 1e-6);
        assert!(h[2].abs() < 1e-6 && h[3].abs() < 1e-6);
        assert_eq!(t.k_to_i_20[70], 19);
        assert_eq!(t.k_to_i_20[41], 18);
        assert_eq!(t.k_to_i_20[42], 19);
    }

    #[test]
    fn hybrid_prototypes_are_unity_dc_sum() {
        // The 8 complex band filters together form a delay of 6 taps.
        let t = tables();
        for (filters, bands) in [(&t.f20_0_8, 8usize), (&t.f34_0_12, 12), (&t.f34_2_4, 4)] {
            for n in 0..13 {
                let mut s = C32::default();
                for f in filters.iter().take(bands) {
                    s = s + f[n];
                }
                let want = if n == 6 { 1.0 } else { 0.0 };
                assert!((s.re - want).abs() < 2e-3 && s.im.abs() < 2e-3, "bands {bands} tap {n}: {s:?}");
            }
        }
    }
}
