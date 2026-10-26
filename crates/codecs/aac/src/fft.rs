// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Radix-2 complex FFT used by the IMDCT and the SBR QMF banks.

use std::ops::{Add, Mul, Sub};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct C32 {
    pub re: f32,
    pub im: f32,
}

impl C32 {
    #[inline]
    pub const fn new(re: f32, im: f32) -> C32 {
        C32 { re, im }
    }
    /// e^{i theta}
    pub fn expi(theta: f64) -> C32 {
        C32 { re: theta.cos() as f32, im: theta.sin() as f32 }
    }
    #[inline]
    pub fn conj(self) -> C32 {
        C32 { re: self.re, im: -self.im }
    }
    #[inline]
    pub fn scale(self, s: f32) -> C32 {
        C32 { re: self.re * s, im: self.im * s }
    }
    #[inline]
    pub fn norm_sqr(self) -> f32 {
        self.re * self.re + self.im * self.im
    }
}

impl Add for C32 {
    type Output = C32;
    #[inline]
    fn add(self, o: C32) -> C32 {
        C32 { re: self.re + o.re, im: self.im + o.im }
    }
}
impl Sub for C32 {
    type Output = C32;
    #[inline]
    fn sub(self, o: C32) -> C32 {
        C32 { re: self.re - o.re, im: self.im - o.im }
    }
}
impl Mul for C32 {
    type Output = C32;
    #[inline]
    fn mul(self, o: C32) -> C32 {
        C32 { re: self.re * o.re - self.im * o.im, im: self.re * o.im + self.im * o.re }
    }
}

/// In-place forward FFT `X[k] = sum x[n] e^{-2 pi i nk/N}` of a fixed size.
pub(crate) struct Fft {
    n: usize,
    rev: Vec<u32>,
    twiddle: Vec<C32>,
}

impl Fft {
    pub fn new(n: usize) -> Fft {
        assert!(n.is_power_of_two() && n >= 2);
        let bits = n.trailing_zeros();
        let rev = (0..n as u32).map(|i| i.reverse_bits() >> (32 - bits)).collect();
        let twiddle = (0..n / 2).map(|k| C32::expi(-2.0 * std::f64::consts::PI * k as f64 / n as f64)).collect();
        Fft { n, rev, twiddle }
    }

    pub fn forward(&self, x: &mut [C32]) {
        let n = self.n;
        debug_assert_eq!(x.len(), n);
        let x = &mut x[..n];
        for i in 0..n {
            let j = self.rev[i] as usize;
            if i < j {
                x.swap(i, j);
            }
        }
        // First stage (butterflies of size 2) without twiddles.
        for p in x.chunks_exact_mut(2) {
            let (a, b) = (p[0], p[1]);
            p[0] = a + b;
            p[1] = a - b;
        }
        let mut half = 2;
        while half < n {
            let stride = n / (2 * half);
            for block in x.chunks_exact_mut(2 * half) {
                let (lo, hi) = block.split_at_mut(half);
                for j in 0..half {
                    let w = self.twiddle[j * stride];
                    let t = hi[j] * w;
                    let a = lo[j];
                    lo[j] = a + t;
                    hi[j] = a - t;
                }
            }
            half *= 2;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_direct_dft() {
        for &n in &[2usize, 8, 64, 128, 512] {
            let fft = Fft::new(n);
            let x: Vec<C32> =
                (0..n).map(|i| C32::new(((i * 7 + 3) % 11) as f32 - 5.0, ((i * 5 + 1) % 13) as f32 - 6.0)).collect();
            let mut y = x.clone();
            fft.forward(&mut y);
            for k in 0..n {
                let mut acc = (0f64, 0f64);
                for (i, v) in x.iter().enumerate() {
                    let th = -2.0 * std::f64::consts::PI * (i * k % n) as f64 / n as f64;
                    acc.0 += v.re as f64 * th.cos() - v.im as f64 * th.sin();
                    acc.1 += v.re as f64 * th.sin() + v.im as f64 * th.cos();
                }
                assert!((y[k].re as f64 - acc.0).abs() < 1e-3 * n as f64, "n={n} k={k}");
                assert!((y[k].im as f64 - acc.1).abs() < 1e-3 * n as f64, "n={n} k={k}");
            }
        }
    }
}
