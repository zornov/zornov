// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Single-precision GEMM: packed panels, cache blocking, an 8×12 NEON
//! micro-kernel (`vfmaq_laneq_f32`) and multithreading over row or column
//! panels with scoped threads. Also a GEMV path for `m == 1` (batch-1
//! decoders) and an i32 GEMM for quantized ops.

use super::par::{SendPtr, par_range};

pub const MR: usize = 8;
pub const NR: usize = 12;
const KC: usize = 256;
const MC: usize = 128;

/// A (m×k) packed into MR-row panels; panel `p` holds `k` groups of MR values.
#[derive(Clone, Debug)]
pub struct PackedA {
    pub m: usize,
    pub k: usize,
    pub data: Vec<f32>,
}

/// B (k×n) packed into NR-column panels; panel `p` holds `k` groups of NR values.
#[derive(Clone, Debug)]
pub struct PackedB {
    pub k: usize,
    pub n: usize,
    pub data: Vec<f32>,
}

/// Packs A where element (i, kk) is `a[i*rs + kk*cs]`.
pub fn pack_a(m: usize, k: usize, a: &[f32], rs: usize, cs: usize) -> PackedA {
    let panels = m.div_ceil(MR);
    let mut data = vec![0.0f32; panels * k * MR];
    for p in 0..panels {
        let base = p * k * MR;
        for i in 0..MR.min(m - p * MR) {
            let r = p * MR + i;
            let row = &a[r * rs..];
            if cs == 1 {
                for kk in 0..k {
                    data[base + kk * MR + i] = row[kk];
                }
            } else {
                for kk in 0..k {
                    data[base + kk * MR + i] = row[kk * cs];
                }
            }
        }
    }
    PackedA { m, k, data }
}

/// Packs B where element (kk, j) is `b[kk*rs + j*cs]`, in parallel.
pub fn pack_b(k: usize, n: usize, b: &[f32], rs: usize, cs: usize, threads: usize) -> PackedB {
    let panels = n.div_ceil(NR);
    let mut data = vec![0.0f32; panels * k * NR];
    let t = if k * n > 1 << 16 { threads } else { 1 };
    let ptr = SendPtr(data.as_mut_ptr());
    par_range(panels, t, |p0, p1| {
        for p in p0..p1 {
            // SAFETY: each panel p writes only its own k*NR slice.
            let dst = unsafe { std::slice::from_raw_parts_mut(ptr.add(p * k * NR), k * NR) };
            let j0 = p * NR;
            let w = NR.min(n - j0);
            if cs == 1 {
                for kk in 0..k {
                    let src = &b[kk * rs + j0..kk * rs + j0 + w];
                    dst[kk * NR..kk * NR + w].copy_from_slice(src);
                }
            } else {
                for kk in 0..k {
                    for j in 0..w {
                        dst[kk * NR + j] = b[kk * rs + (j0 + j) * cs];
                    }
                }
            }
        }
    });
    PackedB { k, n, data }
}

impl PackedB {
    /// Packs a panel-format B from a closure producing rows (used by im2col).
    pub fn zeros(k: usize, n: usize) -> PackedB {
        PackedB { k, n, data: vec![0.0; n.div_ceil(NR) * k * NR] }
    }
}

// ------------------------------------------------------------- micro-kernels

#[cfg(target_arch = "aarch64")]
#[inline(always)]
unsafe fn micro_full(kc: usize, a: *const f32, b: *const f32, c: *mut f32, ldc: usize) {
    use std::arch::aarch64::*;
    // SAFETY: caller guarantees `a` has kc*MR, `b` kc*NR readable floats and
    // `c` an MR×NR writable tile with row stride ldc.
    unsafe {
        let z = vdupq_n_f32(0.0);
        let mut acc = [[z; 3]; 8];
        let mut ap = a;
        let mut bp = b;
        macro_rules! row {
            ($r:expr, $av:expr, $l:expr, $b0:expr, $b1:expr, $b2:expr) => {
                acc[$r][0] = vfmaq_laneq_f32::<$l>(acc[$r][0], $b0, $av);
                acc[$r][1] = vfmaq_laneq_f32::<$l>(acc[$r][1], $b1, $av);
                acc[$r][2] = vfmaq_laneq_f32::<$l>(acc[$r][2], $b2, $av);
            };
        }
        for _ in 0..kc {
            let a0 = vld1q_f32(ap);
            let a1 = vld1q_f32(ap.add(4));
            let b0 = vld1q_f32(bp);
            let b1 = vld1q_f32(bp.add(4));
            let b2 = vld1q_f32(bp.add(8));
            row!(0, a0, 0, b0, b1, b2);
            row!(1, a0, 1, b0, b1, b2);
            row!(2, a0, 2, b0, b1, b2);
            row!(3, a0, 3, b0, b1, b2);
            row!(4, a1, 0, b0, b1, b2);
            row!(5, a1, 1, b0, b1, b2);
            row!(6, a1, 2, b0, b1, b2);
            row!(7, a1, 3, b0, b1, b2);
            ap = ap.add(MR);
            bp = bp.add(NR);
        }
        for (r, accr) in acc.iter().enumerate() {
            let p = c.add(r * ldc);
            vst1q_f32(p, vaddq_f32(vld1q_f32(p), accr[0]));
            vst1q_f32(p.add(4), vaddq_f32(vld1q_f32(p.add(4)), accr[1]));
            vst1q_f32(p.add(8), vaddq_f32(vld1q_f32(p.add(8)), accr[2]));
        }
    }
}

#[cfg(not(target_arch = "aarch64"))]
#[inline(always)]
unsafe fn micro_full(kc: usize, a: *const f32, b: *const f32, c: *mut f32, ldc: usize) {
    // SAFETY: as above.
    unsafe {
        let mut acc = [[0.0f32; NR]; MR];
        for p in 0..kc {
            let av = std::slice::from_raw_parts(a.add(p * MR), MR);
            let bv = std::slice::from_raw_parts(b.add(p * NR), NR);
            for i in 0..MR {
                for j in 0..NR {
                    acc[i][j] += av[i] * bv[j];
                }
            }
        }
        for i in 0..MR {
            for j in 0..NR {
                *c.add(i * ldc + j) += acc[i][j];
            }
        }
    }
}

#[inline(always)]
unsafe fn micro(kc: usize, a: *const f32, b: *const f32, c: *mut f32, ldc: usize, mr: usize, nr: usize) {
    // SAFETY: forwarded from callers; partial tiles go through a local buffer.
    unsafe {
        if mr == MR && nr == NR {
            micro_full(kc, a, b, c, ldc);
        } else {
            let mut tmp = [0.0f32; MR * NR];
            micro_full(kc, a, b, tmp.as_mut_ptr(), NR);
            for i in 0..mr {
                for j in 0..nr {
                    *c.add(i * ldc + j) += tmp[i * NR + j];
                }
            }
        }
    }
}

/// C += A·B over row panels `ip0..ip1` and column panels `jp0..jp1`.
///
/// # Safety
/// `c` must address an `a.m × b.n` matrix with row stride `ldc`, and no other
/// thread may write the same tiles concurrently.
pub unsafe fn gemm_region(a: &PackedA, b: &PackedB, c: *mut f32, ldc: usize, ip0: usize, ip1: usize, jp0: usize, jp1: usize) {
    let (m, n, k) = (a.m, b.n, a.k);
    debug_assert_eq!(a.k, b.k);
    let mut kc0 = 0;
    while kc0 < k {
        let kc = KC.min(k - kc0);
        let mut ib = ip0;
        while ib < ip1 {
            let ie = (ib + MC / MR).min(ip1);
            for j in jp0..jp1 {
                // SAFETY: panel offsets are within the packed buffers.
                let bp = unsafe { b.data.as_ptr().add(j * k * NR + kc0 * NR) };
                let nr = NR.min(n - j * NR);
                for i in ib..ie {
                    let mr = MR.min(m - i * MR);
                    unsafe {
                        let ap = a.data.as_ptr().add(i * k * MR + kc0 * MR);
                        micro(kc, ap, bp, c.add(i * MR * ldc + j * NR), ldc, mr, nr);
                    }
                }
            }
            ib = ie;
        }
        kc0 += kc;
    }
}

/// C (m×n, row stride ldc) += A·B with both operands packed.
pub fn gemm_packed(a: &PackedA, b: &PackedB, c: &mut [f32], ldc: usize, threads: usize) {
    let (m, n, k) = (a.m, b.n, a.k);
    assert_eq!(a.k, b.k, "gemm: inner dimensions differ");
    if m == 0 || n == 0 {
        return;
    }
    assert!(c.len() >= (m - 1) * ldc + n, "gemm: output too small");
    let (mp, np) = (m.div_ceil(MR), n.div_ceil(NR));
    let flops = 2.0 * m as f64 * n as f64 * k as f64;
    // at least ~0.5 MFLOP per worker; pool workers are cheap to wake
    let t = threads.max(1).min((flops / 5.0e5) as usize).max(1);
    let cp = SendPtr(c.as_mut_ptr());
    if t <= 1 {
        // SAFETY: c is large enough (asserted) and exclusively borrowed.
        unsafe { gemm_region(a, b, cp.0, ldc, 0, mp, 0, np) };
    } else if mp >= np || mp >= 2 * t {
        par_range(mp, t, |p0, p1| unsafe { gemm_region(a, b, cp.get(), ldc, p0, p1, 0, np) });
    } else {
        par_range(np, t, |p0, p1| unsafe { gemm_region(a, b, cp.get(), ldc, 0, mp, p0, p1) });
    }
}

/// c (n) += a (k) · B (packed), parallel over column panels.
pub fn gemv_packed(a: &[f32], b: &PackedB, c: &mut [f32], threads: usize) {
    let (k, n) = (b.k, b.n);
    let np = n.div_ceil(NR);
    let t = if k * n < 1 << 16 { 1 } else { threads };
    let cp = SendPtr(c.as_mut_ptr());
    par_range(np, t, |p0, p1| {
        for p in p0..p1 {
            let panel = &b.data[p * k * NR..(p + 1) * k * NR];
            let mut acc = [0.0f32; NR];
            for kk in 0..k {
                let x = a[kk];
                let row = &panel[kk * NR..kk * NR + NR];
                for j in 0..NR {
                    acc[j] += x * row[j];
                }
            }
            let w = NR.min(n - p * NR);
            for (j, v) in acc.iter().enumerate().take(w) {
                // SAFETY: panel p owns columns p*NR..p*NR+w.
                unsafe { *cp.add(p * NR + j) += v };
            }
        }
    });
}

/// General strided SGEMM: `C = A·B (+ C if accumulate)`, where A(i,kk) =
/// `a[i*a_rs + kk*a_cs]` and B(kk,j) = `b[kk*b_rs + j*b_cs]`.
#[allow(clippy::too_many_arguments)]
pub fn sgemm(m: usize, n: usize, k: usize, a: &[f32], a_rs: usize, a_cs: usize, b: &[f32], b_rs: usize, b_cs: usize, c: &mut [f32], ldc: usize, accumulate: bool, threads: usize) {
    if !accumulate {
        for i in 0..m {
            c[i * ldc..i * ldc + n].fill(0.0);
        }
    }
    if m == 0 || n == 0 || k == 0 {
        return;
    }
    if m == 1 {
        gemv_strided(n, k, &a[..], a_cs, b, b_rs, b_cs, &mut c[..n], threads);
        return;
    }
    let pa = pack_a(m, k, a, a_rs, a_cs);
    let pb = pack_b(k, n, b, b_rs, b_cs, threads);
    gemm_packed(&pa, &pb, c, ldc, threads);
}

/// c (n) += x · B with x(kk) = `x[kk*xs]` and B strided.
#[allow(clippy::too_many_arguments)]
pub fn gemv_strided(n: usize, k: usize, x: &[f32], xs: usize, b: &[f32], b_rs: usize, b_cs: usize, c: &mut [f32], threads: usize) {
    let xv: Vec<f32> = (0..k).map(|i| x[i * xs]).collect();
    let t = if n * k < 1 << 15 { 1 } else { threads };
    let cp = SendPtr(c.as_mut_ptr());
    if b_cs == 1 {
        // axpy rows of B
        par_range(n.div_ceil(64), t, |c0, c1| {
            let j0 = c0 * 64;
            let j1 = (c1 * 64).min(n);
            // SAFETY: disjoint column ranges.
            let out = unsafe { std::slice::from_raw_parts_mut(cp.add(j0), j1 - j0) };
            for (kk, &xv) in xv.iter().enumerate() {
                let row = &b[kk * b_rs + j0..kk * b_rs + j1];
                for (o, &r) in out.iter_mut().zip(row) {
                    *o += xv * r;
                }
            }
        });
    } else if b_rs == 1 {
        // dot with columns of B (B transposed in memory)
        par_range(n, t, |j0, j1| {
            for j in j0..j1 {
                let col = &b[j * b_cs..j * b_cs + k];
                let s = dot(&xv, col);
                unsafe { *cp.add(j) += s };
            }
        });
    } else {
        for j in 0..n {
            let mut s = 0.0;
            for kk in 0..k {
                s += xv[kk] * b[kk * b_rs + j * b_cs];
            }
            c[j] += s;
        }
    }
}

/// Dot product with 4 independent accumulators (vectorises well).
#[inline]
pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len());
    let (a, b) = (&a[..n], &b[..n]);
    let mut acc = [0.0f32; 16];
    let chunks = n / 16;
    for c in 0..chunks {
        let (x, y) = (&a[c * 16..c * 16 + 16], &b[c * 16..c * 16 + 16]);
        for i in 0..16 {
            acc[i] += x[i] * y[i];
        }
    }
    let mut s: f32 = acc.iter().sum();
    for i in chunks * 16..n {
        s += a[i] * b[i];
    }
    s
}

/// Integer GEMM: c[i*n + j] = Σ a[i*k + kk] · bt[j*k + kk]  (bt is B transposed).
pub fn gemm_i32(m: usize, n: usize, k: usize, a: &[i32], bt: &[i32], c: &mut [i32], threads: usize) {
    let t = if m * n * k < 1 << 16 { 1 } else { threads };
    let cp = SendPtr(c.as_mut_ptr());
    let work = |i0: usize, i1: usize| {
        for i in i0..i1 {
            let ar = &a[i * k..(i + 1) * k];
            for j in 0..n {
                let br = &bt[j * k..(j + 1) * k];
                let mut acc = [0i32; 8];
                let ch = k / 8;
                for c in 0..ch {
                    for l in 0..8 {
                        acc[l] = acc[l].wrapping_add(ar[c * 8 + l].wrapping_mul(br[c * 8 + l]));
                    }
                }
                let mut s: i32 = acc.iter().fold(0i32, |x, y| x.wrapping_add(*y));
                for kk in ch * 8..k {
                    s = s.wrapping_add(ar[kk].wrapping_mul(br[kk]));
                }
                // SAFETY: rows are disjoint between workers.
                unsafe { *cp.add(i * n + j) = s };
            }
        }
    };
    if m >= t {
        par_range(m, t, work);
    } else {
        work(0, m);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn naive(m: usize, n: usize, k: usize, a: &[f32], b: &[f32]) -> Vec<f32> {
        let mut c = vec![0.0; m * n];
        for i in 0..m {
            for j in 0..n {
                let mut s = 0.0f64;
                for kk in 0..k {
                    s += a[i * k + kk] as f64 * b[kk * n + j] as f64;
                }
                c[i * n + j] = s as f32;
            }
        }
        c
    }
    #[test]
    fn sgemm_matches_naive() {
        let mut seed = 1u32;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed as f32 / u32::MAX as f32) * 2.0 - 1.0
        };
        for &(m, n, k) in &[(1, 1, 1), (7, 13, 5), (8, 12, 256), (33, 29, 300), (1, 50, 70), (130, 70, 513), (2, 300, 9)] {
            let a: Vec<f32> = (0..m * k).map(|_| rnd()).collect();
            let b: Vec<f32> = (0..k * n).map(|_| rnd()).collect();
            let want = naive(m, n, k, &a, &b);
            for threads in [1, 4] {
                let mut c = vec![0.0; m * n];
                sgemm(m, n, k, &a, k, 1, &b, n, 1, &mut c, n, false, threads);
                let err = c.iter().zip(&want).map(|(x, y)| (x - y).abs()).fold(0.0, f32::max);
                assert!(err < 1e-4 * (k as f32).sqrt(), "{m}x{n}x{k} t{threads}: {err}");
            }
            // transposed B
            let bt: Vec<f32> = (0..n * k).map(|i| b[(i % k) * n + i / k]).collect();
            let mut c = vec![0.0; m * n];
            sgemm(m, n, k, &a, k, 1, &bt, 1, k, &mut c, n, false, 3);
            let err = c.iter().zip(&want).map(|(x, y)| (x - y).abs()).fold(0.0, f32::max);
            assert!(err < 1e-4 * (k as f32).sqrt(), "T {m}x{n}x{k}: {err}");
        }
    }
}
