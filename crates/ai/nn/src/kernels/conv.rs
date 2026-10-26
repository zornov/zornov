// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Convolution kernels on NCDHW data (1-D and 2-D convs are mapped to 3-D
//! with unit leading spatial dims): im2col written straight into packed GEMM
//! panels, a direct depthwise kernel, and transposed convolution via
//! GEMM + col2im.

use super::gemm::{NR, PackedA, PackedB, gemm_packed, gemm_region, pack_a, pack_b, sgemm};
use super::par::{SendPtr, par_dynamic};

#[derive(Clone, Debug, PartialEq)]
pub struct ConvGeom {
    pub n: usize,
    pub c_in: usize,
    pub c_out: usize,
    pub group: usize,
    /// Input spatial size (d, h, w).
    pub inp: [usize; 3],
    /// Output spatial size (d, h, w).
    pub out: [usize; 3],
    pub k: [usize; 3],
    pub stride: [usize; 3],
    pub dil: [usize; 3],
    /// Leading padding (d, h, w).
    pub pad: [usize; 3],
}

impl ConvGeom {
    pub fn in_size(&self) -> usize {
        self.inp.iter().product()
    }
    pub fn out_size(&self) -> usize {
        self.out.iter().product()
    }
    pub fn kernel_size(&self) -> usize {
        self.k.iter().product()
    }
    pub fn icg(&self) -> usize {
        self.c_in / self.group
    }
    pub fn ocg(&self) -> usize {
        self.c_out / self.group
    }
    fn is_pointwise(&self) -> bool {
        self.k == [1, 1, 1] && self.stride == [1, 1, 1] && self.pad == [0, 0, 0] && self.inp == self.out
    }
    fn is_depthwise_2d(&self) -> bool {
        self.icg() == 1 && self.ocg() == 1 && self.k[0] == 1 && self.inp[0] == 1 && self.out[0] == 1
    }
}

/// Packs weights `[c_out, icg, kd, kh, kw]` into one [`PackedA`] per group.
pub fn pack_conv_weights(w: &[f32], g: &ConvGeom) -> Vec<PackedA> {
    let kk = g.icg() * g.kernel_size();
    let ocg = g.ocg();
    (0..g.group).map(|grp| pack_a(ocg, kk, &w[grp * ocg * kk..(grp + 1) * ocg * kk], kk, 1)).collect()
}

/// im2col of output columns `p0..p0+nc` written directly in packed-B layout.
fn im2col_packed(x: &[f32], g: &ConvGeom, p0: usize, nc: usize, out: &mut PackedB) {
    let icg = g.icg();
    let [id, ih, iw] = g.inp;
    let [_, oh, ow] = g.out;
    let [kd, kh, kw] = g.k;
    let isz = g.in_size();
    let kk = icg * kd * kh * kw;
    debug_assert_eq!(out.k, kk);
    out.n = nc;
    let need = nc.div_ceil(NR) * kk * NR;
    if out.data.len() < need {
        out.data.resize(need, 0.0);
    }
    let data = &mut out.data;
    let mut r = 0;
    for ci in 0..icg {
        let xc = &x[ci * isz..(ci + 1) * isz];
        for kz in 0..kd {
            for ky in 0..kh {
                for kx in 0..kw {
                    let mut p = p0;
                    let end = p0 + nc;
                    let mut jj = 0;
                    while p < end {
                        let oz = p / (oh * ow);
                        let oy = (p / ow) % oh;
                        let ox0 = p % ow;
                        let seg = (ow - ox0).min(end - p);
                        let iz = (oz * g.stride[0] + kz * g.dil[0]) as isize - g.pad[0] as isize;
                        let iy = (oy * g.stride[1] + ky * g.dil[1]) as isize - g.pad[1] as isize;
                        let row_ok = iz >= 0 && (iz as usize) < id && iy >= 0 && (iy as usize) < ih;
                        let base = if row_ok { (iz as usize * ih + iy as usize) * iw } else { 0 };
                        let xoff = (kx * g.dil[2]) as isize - g.pad[2] as isize;
                        for s in 0..seg {
                            let ox = ox0 + s;
                            let ix = (ox * g.stride[2]) as isize + xoff;
                            let v = if row_ok && ix >= 0 && (ix as usize) < iw { xc[base + ix as usize] } else { 0.0 };
                            let j = jj + s;
                            data[(j / NR) * kk * NR + r * NR + (j % NR)] = v;
                        }
                        jj += seg;
                        p += seg;
                    }
                    // zero the padding lanes of the last panel
                    let tail = nc % NR;
                    if tail != 0 {
                        let pnl = nc / NR;
                        for l in tail..NR {
                            data[pnl * kk * NR + r * NR + l] = 0.0;
                        }
                    }
                    r += 1;
                }
            }
        }
    }
}

/// Direct depthwise 2-D convolution of one channel plane.
fn depthwise_plane(x: &[f32], w: &[f32], out: &mut [f32], g: &ConvGeom) {
    let [_, ih, iw] = g.inp;
    let [_, oh, ow] = g.out;
    let [_, kh, kw] = g.k;
    let (sy, sx) = (g.stride[1], g.stride[2]);
    let (dy, dx) = (g.dil[1], g.dil[2]);
    let (py, px) = (g.pad[1] as isize, g.pad[2] as isize);
    for oy in 0..oh {
        let orow = &mut out[oy * ow..(oy + 1) * ow];
        for ky in 0..kh {
            let iy = (oy * sy + ky * dy) as isize - py;
            if iy < 0 || iy as usize >= ih {
                continue;
            }
            let irow = &x[iy as usize * iw..(iy as usize + 1) * iw];
            for kx in 0..kw {
                let wv = w[ky * kw + kx];
                let off = (kx * dx) as isize - px;
                // valid ox: 0 <= ox*sx + off < iw
                let lo = if off >= 0 { 0 } else { ((-off) as usize).div_ceil(sx) };
                let hi_excl = if (iw as isize - off) <= 0 { 0 } else { ((iw as isize - off - 1) as usize / sx + 1).min(ow) };
                if lo >= hi_excl {
                    continue;
                }
                if sx == 1 {
                    let s0 = (lo as isize + off) as usize;
                    let src = &irow[s0..s0 + (hi_excl - lo)];
                    for (o, &v) in orow[lo..hi_excl].iter_mut().zip(src) {
                        *o += wv * v;
                    }
                } else {
                    for ox in lo..hi_excl {
                        orow[ox] += wv * irow[(ox as isize * sx as isize + off) as usize];
                    }
                }
            }
        }
    }
}

/// Forward convolution. `x` is `[n, c_in, d, h, w]`, result `[n, c_out, od, oh, ow]`.
/// `w` is only used for the depthwise path; `packed` comes from [`pack_conv_weights`].
pub fn conv_forward(x: &[f32], g: &ConvGeom, w: &[f32], packed: &[PackedA], bias: Option<&[f32]>, threads: usize) -> Vec<f32> {
    let osz = g.out_size();
    let isz = g.in_size();
    let (icg, ocg) = (g.icg(), g.ocg());
    let mut out = vec![0.0f32; g.n * g.c_out * osz];
    if let Some(b) = bias {
        for (i, plane) in out.chunks_mut(osz.max(1)).enumerate() {
            plane.fill(b[i % g.c_out]);
        }
    }
    if osz == 0 {
        return out;
    }
    let kk = icg * g.kernel_size();
    let op = SendPtr(out.as_mut_ptr());

    if g.is_depthwise_2d() {
        let ks = g.kernel_size();
        par_dynamic(g.n * g.c_out, threads, |i| {
            let c = i % g.c_out;
            let xin = &x[i * isz..(i + 1) * isz];
            // SAFETY: plane i is written by this item only.
            let o = unsafe { std::slice::from_raw_parts_mut(op.add(i * osz), osz) };
            depthwise_plane(xin, &w[c * ks..(c + 1) * ks], o, g);
        });
        return out;
    }

    for b in 0..g.n {
        if g.is_pointwise() {
            for grp in 0..g.group {
                let xg = &x[(b * g.c_in + grp * icg) * isz..][..icg * isz];
                let pb = pack_b(icg, isz, xg, isz, 1, threads);
                let o = &mut out[(b * g.c_out + grp * ocg) * osz..][..ocg * osz];
                gemm_packed(&packed[grp], &pb, o, osz, threads);
            }
            continue;
        }
        let nc = {
            let c = (65536 / kk.max(1)).clamp(4 * NR, 128 * NR);
            (c / NR * NR).min(osz.div_ceil(NR) * NR)
        };
        let chunks = osz.div_ceil(nc);
        let items = g.group * chunks;
        let inner_threads = if items >= threads { 1 } else { threads };
        let outer_threads = if items >= threads { threads } else { 1 };
        // per-worker scratch would be nicer; allocation per item is cheap next to the GEMM
        par_dynamic(items, outer_threads, |it| {
            let grp = it / chunks;
            let ch = it % chunks;
            let p0 = ch * nc;
            let ncols = nc.min(osz - p0);
            let xg = &x[(b * g.c_in + grp * icg) * isz..][..icg * isz];
            let mut pb = PackedB { k: kk, n: ncols, data: Vec::new() };
            im2col_packed(xg, g, p0, ncols, &mut pb);
            let pa = &packed[grp];
            // SAFETY: (group, chunk) items write disjoint column ranges of disjoint rows.
            let cbase = unsafe { op.add((b * g.c_out + grp * ocg) * osz + p0) };
            if inner_threads > 1 {
                let o = unsafe { std::slice::from_raw_parts_mut(cbase, (ocg - 1) * osz + ncols) };
                gemm_packed(pa, &pb, o, osz, inner_threads);
            } else {
                unsafe { gemm_region(pa, &pb, cbase, osz, 0, ocg.div_ceil(super::gemm::MR), 0, ncols.div_ceil(NR)) };
            }
        });
    }
    out
}

/// Transposed convolution. `g.c_in`/`g.inp` describe the input, `g.c_out`/`g.out`
/// the output, `w` is `[c_in, ocg, kd, kh, kw]`.
pub fn conv_transpose(x: &[f32], g: &ConvGeom, w: &[f32], bias: Option<&[f32]>, threads: usize) -> Vec<f32> {
    let isz = g.in_size();
    let osz = g.out_size();
    let (icg, ocg) = (g.icg(), g.ocg());
    let ks = g.kernel_size();
    let rows = ocg * ks;
    let mut out = vec![0.0f32; g.n * g.c_out * osz];
    if let Some(b) = bias {
        for (i, plane) in out.chunks_mut(osz.max(1)).enumerate() {
            plane.fill(b[i % g.c_out]);
        }
    }
    let [id, ih, iw] = g.inp;
    let [od, oh, ow] = g.out;
    let [kd, kh, kw] = g.k;
    let mut col = vec![0.0f32; rows * isz];
    for b in 0..g.n {
        for grp in 0..g.group {
            let xg = &x[(b * g.c_in + grp * icg) * isz..][..icg * isz];
            let wg = &w[grp * icg * rows..(grp + 1) * icg * rows];
            // col[r, p] = Σ_ci W[ci, r] · X[ci, p]
            sgemm(rows, isz, icg, wg, 1, rows, xg, isz, 1, &mut col, isz, false, threads);
            let obase = (b * g.c_out + grp * ocg) * osz;
            let op = SendPtr(out.as_mut_ptr());
            let col = &col;
            par_dynamic(ocg, threads, |oc| {
                // SAFETY: each output channel plane is owned by one item.
                let o = unsafe { std::slice::from_raw_parts_mut(op.add(obase + oc * osz), osz) };
                for kz in 0..kd {
                    for ky in 0..kh {
                        for kx in 0..kw {
                            let r = ((oc * kd + kz) * kh + ky) * kw + kx;
                            let crow = &col[r * isz..(r + 1) * isz];
                            for z in 0..id {
                                let oz = (z * g.stride[0] + kz * g.dil[0]) as isize - g.pad[0] as isize;
                                if oz < 0 || oz as usize >= od {
                                    continue;
                                }
                                for y in 0..ih {
                                    let oy = (y * g.stride[1] + ky * g.dil[1]) as isize - g.pad[1] as isize;
                                    if oy < 0 || oy as usize >= oh {
                                        continue;
                                    }
                                    let src = &crow[(z * ih + y) * iw..(z * ih + y + 1) * iw];
                                    let dst = &mut o[(oz as usize * oh + oy as usize) * ow..][..ow];
                                    let off = (kx * g.dil[2]) as isize - g.pad[2] as isize;
                                    for (xx, &v) in src.iter().enumerate() {
                                        let ox = (xx * g.stride[2]) as isize + off;
                                        if ox >= 0 && (ox as usize) < ow {
                                            dst[ox as usize] += v;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            });
        }
    }
    out
}
