// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A small in-place radix-2 complex FFT with a reusable twiddle table, used
//! by the loudness true-peak check, the VAD, the pitch shifter and the beat
//! analyser. Not the fastest possible, but allocation-free after `new`.

/// A planner for one power-of-two size.
#[derive(Clone, Debug)]
pub struct Fft {
    n: usize,
    cos: Vec<f32>,
    sin: Vec<f32>,
    rev: Vec<u32>,
}

impl Fft {
    /// Builds tables for size `n` (rounded up to a power of two, at least 2).
    pub fn new(n: usize) -> Self {
        let n = n.next_power_of_two().max(2);
        let half = n / 2;
        let cos = (0..half).map(|k| (std::f32::consts::TAU * k as f32 / n as f32).cos()).collect();
        let sin = (0..half).map(|k| (std::f32::consts::TAU * k as f32 / n as f32).sin()).collect();
        let bits = n.trailing_zeros();
        let rev = (0..n as u32).map(|i| i.reverse_bits() >> (32 - bits)).collect();
        Self { n, cos, sin, rev }
    }

    /// The transform size.
    pub fn len(&self) -> usize {
        self.n
    }

    /// Forward transform in place (no scaling).
    pub fn forward(&self, re: &mut [f32], im: &mut [f32]) {
        self.transform(re, im, false)
    }

    /// Inverse transform in place, scaled by 1/n.
    pub fn inverse(&self, re: &mut [f32], im: &mut [f32]) {
        self.transform(re, im, true);
        let s = 1.0 / self.n as f32;
        for (a, b) in re.iter_mut().zip(im.iter_mut()) {
            *a *= s;
            *b *= s;
        }
    }

    fn transform(&self, re: &mut [f32], im: &mut [f32], inverse: bool) {
        let n = self.n;
        debug_assert!(re.len() >= n && im.len() >= n);
        for i in 0..n {
            let j = self.rev[i] as usize;
            if i < j {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= n {
            let step = n / len;
            for start in (0..n).step_by(len) {
                for k in 0..len / 2 {
                    let wr = self.cos[k * step];
                    let wi = if inverse { self.sin[k * step] } else { -self.sin[k * step] };
                    let (p, q) = (start + k, start + k + len / 2);
                    let tr = re[q] * wr - im[q] * wi;
                    let ti = re[q] * wi + im[q] * wr;
                    re[q] = re[p] - tr;
                    im[q] = im[p] - ti;
                    re[p] += tr;
                    im[p] += ti;
                }
            }
            len <<= 1;
        }
    }
}

/// Periodic Hann window of length `n` written into `out`.
pub fn hann(out: &mut [f32]) {
    let n = out.len();
    for (i, w) in out.iter_mut().enumerate() {
        *w = 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_lands_in_one_bin() {
        let f = Fft::new(256);
        let mut re: Vec<f32> = (0..256).map(|i| (std::f32::consts::TAU * 8.0 * i as f32 / 256.0).sin()).collect();
        let mut im = vec![0.0; 256];
        f.forward(&mut re, &mut im);
        let mag: Vec<f32> = re.iter().zip(&im).map(|(a, b)| (a * a + b * b).sqrt()).collect();
        assert!((mag[8] - 128.0).abs() < 1e-2, "{}", mag[8]);
        assert!(mag.iter().enumerate().filter(|(k, _)| *k != 8 && *k != 248).all(|(_, m)| *m < 1e-2));
        f.inverse(&mut re, &mut im);
        for (i, v) in re.iter().enumerate() {
            assert!((v - (std::f32::consts::TAU * 8.0 * i as f32 / 256.0).sin()).abs() < 1e-4);
        }
    }
}
