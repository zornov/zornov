// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Filterbank (ISO/IEC 14496-3 4.6.11): window shapes, window sequences and
//! overlap-add.

use crate::ics::{IcsInfo, EIGHT_SHORT_SEQUENCE, LONG_START_SEQUENCE, LONG_STOP_SEQUENCE};
use crate::mdct::Imdct;
use std::f64::consts::PI;
use std::sync::OnceLock;

/// Rising window halves: `[shape][long=0/short=1]`, shape 0 = sine, 1 = KBD.
pub(crate) struct Windows {
    pub long: [Vec<f32>; 2],
    pub short: [Vec<f32>; 2],
}

/// Sine window rising half: `w[n] = sin(pi/N (n + 1/2))`, `n < N/2`.
pub(crate) fn sine_window(n: usize) -> Vec<f32> {
    (0..n / 2).map(|i| (PI / n as f64 * (i as f64 + 0.5)).sin() as f32).collect()
}

/// Zeroth-order modified Bessel function of the first kind.
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let q = x * x / 4.0;
    for k in 1..200 {
        term *= q / (k * k) as f64;
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

/// Kaiser-Bessel-derived window rising half (4.6.11.3.2):
/// `w[n] = sqrt(sum_{j<=n} W'(j) / sum_{j<=N/2} W'(j))`, with the Kaiser
/// kernel `W'(n) = I0(pi a sqrt(1 - ((n - N/4)/(N/4))^2)) / I0(pi a)`.
pub(crate) fn kbd_window(n: usize, alpha: f64) -> Vec<f32> {
    let q = (n / 4) as f64;
    let kernel: Vec<f64> = (0..=n / 2)
        .map(|j| {
            let r = (j as f64 - q) / q;
            bessel_i0(PI * alpha * (1.0 - r * r).max(0.0).sqrt()) / bessel_i0(PI * alpha)
        })
        .collect();
    let total: f64 = kernel.iter().sum();
    let mut acc = 0.0;
    (0..n / 2)
        .map(|i| {
            acc += kernel[i];
            (acc / total).sqrt() as f32
        })
        .collect()
}

pub(crate) fn windows() -> &'static Windows {
    static W: OnceLock<Windows> = OnceLock::new();
    W.get_or_init(|| Windows {
        long: [sine_window(2048), kbd_window(2048, 4.0)],
        short: [sine_window(256), kbd_window(256, 6.0)],
    })
}

/// Transform state shared by all channels.
pub(crate) struct Filterbank {
    long: Imdct,
    short: Imdct,
    buf: Vec<f32>,
    tmp: Vec<f32>,
}

impl Filterbank {
    pub fn new() -> Filterbank {
        Filterbank { long: Imdct::new(2048), short: Imdct::new(256), buf: vec![0.0; 2048], tmp: vec![0.0; 256] }
    }

    /// IMDCT, windowing and overlap-add of one frame.
    ///
    /// `spec`: 1024 coefficients (8 x 128 for short windows); `overlap`: the
    /// windowed second half of the previous frame, replaced by this frame's;
    /// `prev_shape`: window_shape of the previous frame (used for the left
    /// half), updated to this frame's. Writes 1024 samples into `out`.
    pub fn synthesize(
        &mut self,
        ics: &IcsInfo,
        spec: &[f32],
        overlap: &mut [f32],
        prev_shape: &mut u8,
        out: &mut [f32],
    ) {
        let w = windows();
        let ps = *prev_shape as usize;
        let cs = ics.window_shape as usize;
        let (lw_prev, sw_prev) = (&w.long[ps], &w.short[ps]);
        let (lw, sw) = (&w.long[cs], &w.short[cs]);
        let buf = &mut self.buf;
        if ics.window_sequence == EIGHT_SHORT_SEQUENCE {
            buf.fill(0.0);
            for win in 0..8 {
                self.short.transform(&spec[win * 128..win * 128 + 128], &mut self.tmp);
                let rise = if win == 0 { sw_prev } else { sw };
                let base = 448 + 128 * win;
                for i in 0..128 {
                    buf[base + i] += self.tmp[i] * rise[i];
                    buf[base + 128 + i] += self.tmp[128 + i] * sw[127 - i];
                }
            }
        } else {
            self.long.transform(&spec[..1024], buf);
            if ics.window_sequence == LONG_STOP_SEQUENCE {
                buf[..448].fill(0.0);
                for i in 0..128 {
                    buf[448 + i] *= sw_prev[i];
                }
            } else {
                for i in 0..1024 {
                    buf[i] *= lw_prev[i];
                }
            }
            if ics.window_sequence == LONG_START_SEQUENCE {
                for i in 0..128 {
                    buf[1472 + i] *= sw[127 - i];
                }
                buf[1600..].fill(0.0);
            } else {
                for i in 0..1024 {
                    buf[1024 + i] *= lw[1023 - i];
                }
            }
        }
        for i in 0..1024 {
            out[i] = overlap[i] + buf[i];
        }
        overlap.copy_from_slice(&buf[1024..2048]);
        *prev_shape = ics.window_shape;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_satisfy_princen_bradley() {
        let w = windows();
        for shape in 0..2 {
            for (win, n) in [(&w.long[shape], 2048usize), (&w.short[shape], 256)] {
                assert_eq!(win.len(), n / 2);
                for i in 0..n / 2 {
                    // w[n]^2 + w[n + N/2]^2 = 1 where w[n + N/2] = w[N/2 - 1 - n]
                    let s = win[i] * win[i] + win[n / 2 - 1 - i] * win[n / 2 - 1 - i];
                    assert!((s - 1.0).abs() < 1e-6, "shape {shape} N {n} i {i}: {s}");
                }
                assert!(win.windows(2).all(|p| p[0] <= p[1]));
            }
        }
    }

    #[test]
    fn known_window_values() {
        let w = windows();
        // Sine: sin(pi/2048 * 0.5)
        assert!((w.long[0][0] - 0.000766990319).abs() < 1e-9);
        // KBD alpha 4 / 6 reference values (ISO/IEC 14496-3 KBD tables).
        assert!((w.long[1][0] - 0.00029256153896361).abs() < 1e-8, "{}", w.long[1][0]);
        assert!((w.long[1][499] - 0.682022506).abs() < 2e-6, "{}", w.long[1][499]);
        assert!((w.short[1][0] - 4.3795702929468881e-005).abs() < 1e-8, "{}", w.short[1][0]);
        assert!((w.short[1][64] - 0.716675813).abs() < 2e-6, "{}", w.short[1][64]);
    }
}
