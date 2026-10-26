// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Forward MDCT (2048 samples -> 1024 coefficients) via a DCT-IV computed with
//! a 512-point complex FFT, and the sine window.

use std::f64::consts::PI;

/// Radix-2 complex FFT of a fixed power-of-two size.
pub struct Fft {
    n: usize,
    tw: Vec<(f64, f64)>,
    rev: Vec<usize>,
}

impl Fft {
    pub fn new(n: usize) -> Fft {
        assert!(n.is_power_of_two());
        let tw = (0..n / 2).map(|k| {
            let a = -2.0 * PI * k as f64 / n as f64;
            (a.cos(), a.sin())
        }).collect();
        let bits = n.trailing_zeros();
        let rev = (0..n).map(|i| if bits == 0 { 0 } else { i.reverse_bits() >> (usize::BITS - bits) }).collect();
        Fft { n, tw, rev }
    }

    /// In-place forward transform: X[k] = sum x[n] e^{-2 pi i nk / N}.
    pub fn run(&self, re: &mut [f64], im: &mut [f64]) {
        let n = self.n;
        for i in 0..n {
            let j = self.rev[i];
            if j > i {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= n {
            let half = len / 2;
            let step = n / len;
            for start in (0..n).step_by(len) {
                for k in 0..half {
                    let (wr, wi) = self.tw[k * step];
                    let (a, b) = (start + k, start + k + half);
                    let tr = re[b] * wr - im[b] * wi;
                    let ti = re[b] * wi + im[b] * wr;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            len <<= 1;
        }
    }
}

/// MDCT of 2M windowed samples into M coefficients:
/// `X[k] = sum_n z[n] cos(pi / M (n + 1/2 + M/2)(k + 1/2))`.
pub struct Mdct {
    m: usize,
    fft: Fft,
    pre: Vec<(f64, f64)>,
    post: Vec<(f64, f64)>,
}

impl Mdct {
    pub fn new(m: usize) -> Mdct {
        let l = m / 2;
        let tw = |i: usize| {
            let a = -PI * (i as f64 + 0.125) / m as f64;
            (a.cos(), a.sin())
        };
        Mdct { m, fft: Fft::new(l), pre: (0..l).map(tw).collect(), post: (0..l).map(tw).collect() }
    }

    /// `z` has 2M samples (already windowed); `out` receives M coefficients.
    pub fn forward(&self, z: &[f64], out: &mut [f64]) {
        let m = self.m;
        let h = m / 2;
        // Fold to a DCT-IV input: (-c_r - d, a - b_r).
        let mut v = vec![0f64; m];
        for n in 0..h {
            v[n] = -z[3 * h - 1 - n] - z[3 * h + n];
            v[h + n] = z[n] - z[m - 1 - n];
        }
        // DCT-IV via an M/2-point complex FFT.
        let l = m / 2;
        let mut re = vec![0f64; l];
        let mut im = vec![0f64; l];
        for n in 0..l {
            let (a, b) = (v[2 * n], v[m - 1 - 2 * n]);
            let (c, s) = self.pre[n];
            re[n] = a * c - b * s;
            im[n] = a * s + b * c;
        }
        self.fft.run(&mut re, &mut im);
        for k in 0..l {
            let (c, s) = self.post[k];
            let dr = re[k] * c - im[k] * s;
            let di = re[k] * s + im[k] * c;
            out[2 * k] = dr;
            out[m - 1 - 2 * k] = -di;
        }
    }
}

/// Sine window of length `n` (both halves).
pub fn sine_window(n: usize) -> Vec<f64> {
    (0..n).map(|i| (PI / n as f64 * (i as f64 + 0.5)).sin()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_direct_formula() {
        let m = 64;
        let z: Vec<f64> = (0..2 * m).map(|i| ((i * 37 % 23) as f64 - 11.0) * 0.1 + (i as f64 * 0.3).sin()).collect();
        let mdct = Mdct::new(m);
        let mut fast = vec![0f64; m];
        mdct.forward(&z, &mut fast);
        for k in 0..m {
            let direct: f64 = (0..2 * m)
                .map(|n| z[n] * (PI / m as f64 * (n as f64 + 0.5 + m as f64 / 2.0) * (k as f64 + 0.5)).cos())
                .sum();
            assert!((direct - fast[k]).abs() < 1e-9, "k {k}: {direct} vs {}", fast[k]);
        }
    }

    #[test]
    fn fft_impulse() {
        let f = Fft::new(8);
        let mut re = vec![0.0; 8];
        let mut im = vec![0.0; 8];
        re[1] = 1.0;
        f.run(&mut re, &mut im);
        for k in 0..8 {
            let a = -2.0 * PI * k as f64 / 8.0;
            assert!((re[k] - a.cos()).abs() < 1e-12 && (im[k] - a.sin()).abs() < 1e-12);
        }
    }
}
