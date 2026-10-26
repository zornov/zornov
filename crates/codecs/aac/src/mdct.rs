// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Inverse MDCT (ISO/IEC 14496-3 4.6.11.3.1) computed through a DCT-IV and a
//! quarter-length complex FFT.
//!
//! `x[n] = 2/N * sum_{k<N/2} X[k] cos(2 pi/N (n + n0)(k + 1/2))`,
//! `n0 = (N/2 + 1) / 2`, for `N = 2048` (long) and `N = 256` (short).

use crate::fft::{Fft, C32};
use std::f64::consts::PI;

pub(crate) struct Imdct {
    /// Number of input coefficients (N/2).
    m: usize,
    fft: Fft,
    pre: Vec<C32>,
    post: Vec<C32>,
    buf: Vec<C32>,
    u: Vec<f32>,
}

impl Imdct {
    /// IMDCT producing `n` output samples from `n / 2` coefficients.
    pub fn new(n: usize) -> Imdct {
        let m = n / 2;
        let scale = 1.0 / m as f64; // 2/N
        let pre = (0..m / 2)
            .map(|k| {
                let c = C32::expi(-PI * (4 * k + 1) as f64 / (4 * m) as f64);
                c.scale(scale as f32)
            })
            .collect();
        let post = (0..m / 2).map(|k| C32::expi(-PI * k as f64 / m as f64)).collect();
        Imdct { m, fft: Fft::new(m / 2), pre, post, buf: vec![C32::default(); m / 2], u: vec![0.0; m] }
    }

    /// DCT-IV of `input` (length m) into `self.u`, scaled by 2/N.
    fn dct4(&mut self, input: &[f32]) {
        let m = self.m;
        for k in 0..m / 2 {
            let z = C32::new(input[2 * k], input[m - 1 - 2 * k]);
            self.buf[k] = z * self.pre[k];
        }
        self.fft.forward(&mut self.buf);
        for n in 0..m / 2 {
            let c = self.buf[n] * self.post[n];
            self.u[2 * n] = c.re;
            self.u[m - 1 - 2 * n] = -c.im;
        }
    }

    /// `output.len() == 2 * input.len()`.
    pub fn transform(&mut self, input: &[f32], output: &mut [f32]) {
        let m = self.m;
        debug_assert!(input.len() >= m && output.len() >= 2 * m);
        self.dct4(input);
        let h = m / 2;
        let u = &self.u;
        for n in 0..h {
            output[n] = u[n + h];
        }
        for n in h..3 * h {
            output[n] = -u[3 * h - 1 - n];
        }
        for n in 3 * h..2 * m {
            output[n] = -u[n - 3 * h];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn direct(input: &[f32]) -> Vec<f64> {
        let m = input.len();
        let n = 2 * m;
        let n0 = (m as f64 + 1.0) / 2.0;
        (0..n)
            .map(|i| {
                let mut acc = 0f64;
                for (k, &x) in input.iter().enumerate() {
                    acc += x as f64 * (2.0 * PI / n as f64 * (i as f64 + n0) * (k as f64 + 0.5)).cos();
                }
                acc * 2.0 / n as f64
            })
            .collect()
    }

    #[test]
    fn matches_direct_reference() {
        for &n in &[256usize, 2048] {
            let m = n / 2;
            let input: Vec<f32> =
                (0..m).map(|k| ((k * 37 + 11) % 101) as f32 / 50.0 - 1.0 + if k == 3 { 20.0 } else { 0.0 }).collect();
            let mut imdct = Imdct::new(n);
            let mut out = vec![0f32; n];
            imdct.transform(&input, &mut out);
            let want = direct(&input);
            let err = out.iter().zip(&want).map(|(a, b)| (*a as f64 - b).abs()).fold(0.0, f64::max);
            let peak = want.iter().map(|v| v.abs()).fold(0.0, f64::max);
            assert!(err < 1e-5 * peak.max(1.0), "N={n}: max error {err} (peak {peak})");
        }
    }

    #[test]
    fn impulse_is_a_cosine() {
        let mut imdct = Imdct::new(256);
        let mut input = vec![0f32; 128];
        input[5] = 1.0;
        let mut out = vec![0f32; 256];
        imdct.transform(&input, &mut out);
        for (i, v) in out.iter().enumerate() {
            let want = (2.0 * PI / 256.0 * (i as f64 + 64.5) * 5.5).cos() * 2.0 / 256.0;
            assert!((*v as f64 - want).abs() < 1e-6);
        }
    }
}
