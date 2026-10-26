// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! 8x8 DCT (ST 2019-1 equations 8.3 and 8.4) and the zig-zag scan (Figure 48).
//!
//! The cosine table is computed with plain arithmetic (no libm) so the
//! encoder's output is bit-identical on every platform.

use std::sync::OnceLock;

/// Figure 48: bit-stream index r of each position, row = vertical frequency v,
/// column = horizontal frequency u.
const ZZ_OF: [[u8; 8]; 8] = [
    [0, 1, 5, 6, 14, 15, 27, 28],
    [2, 4, 7, 13, 16, 26, 29, 42],
    [3, 8, 12, 17, 25, 30, 41, 43],
    [9, 11, 18, 24, 31, 40, 44, 53],
    [10, 19, 23, 32, 39, 45, 52, 54],
    [20, 22, 33, 38, 46, 51, 55, 60],
    [21, 34, 37, 47, 50, 56, 59, 61],
    [35, 36, 48, 49, 57, 58, 62, 63],
];

/// Raster position (v * 8 + u) of bit-stream index r.
pub const ZIGZAG: [u8; 64] = {
    let mut t = [0u8; 64];
    let mut v = 0;
    while v < 8 {
        let mut u = 0;
        while u < 8 {
            t[ZZ_OF[v][u] as usize] = (v * 8 + u) as u8;
            u += 1;
        }
        v += 1;
    }
    t
};

/// cos(x) by Taylor series for x in [0, 2*pi].
fn cos_series(x: f64) -> f64 {
    let x2 = x * x;
    let mut term = 1.0f64;
    let mut sum = 1.0f64;
    let mut k = 1.0f64;
    for _ in 0..30 {
        term *= -x2 / ((2.0 * k - 1.0) * (2.0 * k));
        sum += term;
        k += 1.0;
    }
    sum
}

/// A[u][i] = C(u)/2 * cos((2i+1) u pi / 16).
fn basis() -> &'static [[f64; 8]; 8] {
    static B: OnceLock<[[f64; 8]; 8]> = OnceLock::new();
    B.get_or_init(|| {
        let pi = std::f64::consts::PI;
        let mut c32 = [0f64; 32];
        for (m, c) in c32.iter_mut().enumerate() {
            *c = cos_series(m as f64 * pi / 16.0);
        }
        let mut a = [[0f64; 8]; 8];
        for (u, row) in a.iter_mut().enumerate() {
            let cu = if u == 0 { std::f64::consts::FRAC_1_SQRT_2 } else { 1.0 };
            for (i, v) in row.iter_mut().enumerate() {
                *v = cu / 2.0 * c32[((2 * i + 1) * u) % 32];
            }
        }
        a
    })
}

/// Forward DCT of samples x[j*8+i] (already level-shifted) to X[v*8+u].
pub fn fdct(x: &[f64; 64]) -> [f64; 64] {
    let a = basis();
    let mut t = [0f64; 64];
    // t[v][i] = sum_j A[v][j] x[j][i]
    for v in 0..8 {
        for i in 0..8 {
            let mut s = 0.0;
            for j in 0..8 {
                s += a[v][j] * x[j * 8 + i];
            }
            t[v * 8 + i] = s;
        }
    }
    let mut out = [0f64; 64];
    for v in 0..8 {
        for u in 0..8 {
            let mut s = 0.0;
            for i in 0..8 {
                s += t[v * 8 + i] * a[u][i];
            }
            out[v * 8 + u] = s;
        }
    }
    out
}

/// Inverse DCT of X[v*8+u] to samples x[j*8+i].
pub fn idct(c: &[f64; 64]) -> [f64; 64] {
    let a = basis();
    // t[v][i] = sum_u X[v][u] A[u][i]
    let mut t = [0f64; 64];
    for v in 0..8 {
        for i in 0..8 {
            let mut s = 0.0;
            for u in 0..8 {
                s += c[v * 8 + u] * a[u][i];
            }
            t[v * 8 + i] = s;
        }
    }
    let mut out = [0f64; 64];
    for j in 0..8 {
        for i in 0..8 {
            let mut s = 0.0;
            for v in 0..8 {
                s += a[v][j] * t[v * 8 + i];
            }
            out[j * 8 + i] = s;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zigzag_is_a_permutation_starting_at_dc() {
        let mut seen = [false; 64];
        for &p in &ZIGZAG {
            seen[p as usize] = true;
        }
        assert!(seen.iter().all(|&s| s));
        assert_eq!(ZIGZAG[0], 0);
        assert_eq!(ZIGZAG[1], 1); // first AC is horizontal
        assert_eq!(ZIGZAG[2], 8);
    }

    #[test]
    fn dct_round_trips_and_dc_is_eight_times_the_mean() {
        let mut x = [0f64; 64];
        for (k, v) in x.iter_mut().enumerate() {
            *v = ((k * 37) % 255) as f64 - 128.0;
        }
        let c = fdct(&x);
        let mean: f64 = x.iter().sum::<f64>() / 64.0;
        assert!((c[0] - 8.0 * mean).abs() < 1e-9);
        let y = idct(&c);
        for k in 0..64 {
            assert!((x[k] - y[k]).abs() < 1e-9);
        }
    }
}
