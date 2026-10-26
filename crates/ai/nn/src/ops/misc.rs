// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Einsum, DFT, DeformConv, onnxruntime contrib fusions and the MediaPipe
//! custom pooling ops used by imported TFLite models.

use super::elementwise::{binary_tensors, gelu, gelu_tanh};
use super::nn::conv_geom;
use super::shape_ops::{expand_to, norm_axis, transpose_tensor};
use super::{OpCtx, inp, opt};
use crate::error::{Error, Result};
use crate::kernels::gemm::sgemm;
use crate::onnx::Node;
use crate::tensor::{Tensor, numel};

// ------------------------------------------------------------ Einsum

type Letters = Vec<u32>;

fn parse_term(s: &str, rank: usize, ell: &mut Option<usize>) -> Result<Letters> {
    let mut out = Vec::new();
    let named = s.chars().filter(|c| c.is_ascii_alphabetic()).count();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '.' {
            if chars.next() != Some('.') || chars.next() != Some('.') {
                return Err(Error(format!("Einsum: bad ellipsis in '{s}'")));
            }
            let n = rank.checked_sub(named).ok_or_else(|| Error("Einsum: term longer than operand rank".into()))?;
            *ell = Some(ell.unwrap_or(0).max(n));
            // ellipsis letters are right-aligned: 1000 + (maxlen - n) .. are filled later
            for i in 0..n {
                out.push(1_000_000 + (n - i) as u32); // distance from the right of the ellipsis block
            }
        } else if c.is_ascii_alphabetic() {
            out.push(c as u32);
        } else if !c.is_whitespace() {
            return Err(Error(format!("Einsum: bad character '{c}'")));
        }
    }
    Ok(out)
}

/// Takes the diagonal for repeated letters within one operand.
fn dedup_letters(t: &Tensor, l: &Letters) -> Result<(Tensor, Letters)> {
    let mut uniq: Letters = Vec::new();
    for &c in l {
        if !uniq.contains(&c) {
            uniq.push(c);
        }
    }
    if uniq.len() == l.len() {
        return Ok((t.clone(), l.clone()));
    }
    let sizes: Vec<usize> = uniq.iter().map(|c| t.shape[l.iter().position(|x| x == c).unwrap()]).collect();
    let st = crate::tensor::strides(&t.shape);
    let ustr: Vec<usize> = uniq.iter().map(|c| l.iter().zip(&st).filter(|(x, _)| *x == c).map(|(_, s)| s).sum()).collect();
    let v = t.f32s();
    let n = numel(&sizes);
    let mut out = Vec::with_capacity(n);
    let mut idx = vec![0usize; sizes.len()];
    for _ in 0..n {
        out.push(v[idx.iter().zip(&ustr).map(|(i, s)| i * s).sum::<usize>()]);
        for d in (0..sizes.len()).rev() {
            idx[d] += 1;
            if idx[d] < sizes[d] {
                break;
            }
            idx[d] = 0;
        }
    }
    Ok((Tensor::f32(&sizes, out), uniq))
}

fn sum_axes(t: &Tensor, l: &Letters, drop: &[u32]) -> (Tensor, Letters) {
    let mut t = t.clone();
    let mut l = l.clone();
    for &c in drop {
        if let Some(ax) = l.iter().position(|x| *x == c) {
            let (outer, d, inner) = super::nn::split3(&t.shape, ax);
            let v = t.f32s();
            let mut o = vec![0.0f32; outer * inner];
            for a in 0..outer {
                for k in 0..d {
                    for i in 0..inner {
                        o[a * inner + i] += v[(a * d + k) * inner + i];
                    }
                }
            }
            let mut sh = t.shape.clone();
            sh.remove(ax);
            t = Tensor::f32(&sh, o);
            l.remove(ax);
        }
    }
    (t, l)
}

fn permute_to(t: &Tensor, l: &Letters, order: &Letters) -> Tensor {
    let perm: Vec<usize> = order.iter().map(|c| l.iter().position(|x| x == c).unwrap()).collect();
    transpose_tensor(t, &perm)
}

pub fn einsum(ctx: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let eq: String = node.attr_s("equation").ok_or_else(|| Error("Einsum: no equation".into()))?.chars().filter(|c| !c.is_whitespace()).collect();
    let ops: Vec<&Tensor> = inputs.iter().flatten().copied().collect();
    let (lhs, rhs) = match eq.split_once("->") {
        Some((a, b)) => (a.to_string(), Some(b.to_string())),
        None => (eq.clone(), None),
    };
    let terms: Vec<&str> = lhs.split(',').collect();
    if terms.len() != ops.len() {
        return Err(Error(format!("Einsum: {} terms for {} inputs", terms.len(), ops.len())));
    }
    let mut ell = None;
    let mut letters: Vec<Letters> = Vec::new();
    for (t, op) in terms.iter().zip(&ops) {
        letters.push(parse_term(t, op.rank(), &mut ell)?);
    }
    let ell_n = ell.unwrap_or(0);
    let out_letters: Letters = match &rhs {
        Some(r) => {
            let mut e2 = Some(ell_n);
            let mut l = parse_term(r, r.chars().filter(|c| c.is_ascii_alphabetic()).count() + if r.contains("...") { ell_n } else { 0 }, &mut e2)?;
            l.retain(|_| true);
            l
        }
        None => {
            let mut l: Letters = (0..ell_n).map(|i| 1_000_000 + (ell_n - i) as u32).collect();
            let mut named: Vec<u32> = letters.iter().flatten().copied().filter(|&c| c < 1_000_000).collect();
            named.sort();
            let once: Vec<u32> = named.iter().copied().filter(|c| named.iter().filter(|x| *x == c).count() == 1).collect();
            l.extend(once);
            l
        }
    };
    // letter sizes (broadcasting 1s)
    let mut size_of = std::collections::HashMap::new();
    for (l, op) in letters.iter().zip(&ops) {
        for (c, &d) in l.iter().zip(&op.shape) {
            let e = size_of.entry(*c).or_insert(d);
            if *e == 1 {
                *e = d;
            } else if d != 1 && d != *e {
                return Err(Error(format!("Einsum: size mismatch for a subscript ({d} vs {})", *e)));
            }
        }
    }
    // normalise operands: diagonals, broadcast 1-dims, float
    let mut cur: Vec<(Tensor, Letters)> = Vec::new();
    for (l, op) in letters.iter().zip(&ops) {
        let (t, l2) = dedup_letters(&op.cast(crate::DType::F32), l)?;
        let full: Vec<usize> = l2.iter().map(|c| size_of[c]).collect();
        let t = if full != t.shape { expand_to(&t, &full) } else { t };
        cur.push((t, l2));
    }
    let needed_later = |from: usize, cur: &[(Tensor, Letters)]| -> Vec<u32> {
        let mut v: Vec<u32> = out_letters.clone();
        for (_, l) in &cur[from..] {
            v.extend(l.iter().copied());
        }
        v
    };
    // sum out letters only used in one operand and not needed
    let (mut acc, mut al) = cur[0].clone();
    {
        let keep = needed_later(1, &cur);
        let drop: Vec<u32> = al.iter().copied().filter(|c| !keep.contains(c)).collect();
        let r = sum_axes(&acc, &al, &drop);
        acc = r.0;
        al = r.1;
    }
    for i in 1..cur.len() {
        let (bt, bl) = cur[i].clone();
        let keep = needed_later(i + 1, &cur);
        let drop_b: Vec<u32> = bl.iter().copied().filter(|c| !keep.contains(c) && !al.contains(c)).collect();
        let (bt, bl) = sum_axes(&bt, &bl, &drop_b);
        let drop_a: Vec<u32> = al.iter().copied().filter(|c| !keep.contains(c) && !bl.contains(c)).collect();
        let (a2, l2) = sum_axes(&acc, &al, &drop_a);
        acc = a2;
        al = l2;
        let batch: Letters = al.iter().copied().filter(|c| bl.contains(c) && keep.contains(c)).collect();
        let contr: Letters = al.iter().copied().filter(|c| bl.contains(c) && !keep.contains(c)).collect();
        let m_l: Letters = al.iter().copied().filter(|c| !bl.contains(c)).collect();
        let n_l: Letters = bl.iter().copied().filter(|c| !al.contains(c)).collect();
        let a_order: Letters = batch.iter().chain(&m_l).chain(&contr).copied().collect();
        let b_order: Letters = batch.iter().chain(&contr).chain(&n_l).copied().collect();
        let pa = permute_to(&acc, &al, &a_order);
        let pb = permute_to(&bt, &bl, &b_order);
        let sz = |l: &Letters| l.iter().map(|c| size_of[c]).product::<usize>();
        let (nb, m, k, n) = (sz(&batch), sz(&m_l), sz(&contr), sz(&n_l));
        let av = pa.f32s();
        let bv = pb.f32s();
        let mut out = vec![0.0f32; nb * m * n];
        for b in 0..nb {
            sgemm(m, n, k, &av[b * m * k..], k, 1, &bv[b * k * n..], n, 1, &mut out[b * m * n..(b + 1) * m * n], n, false, ctx.threads);
        }
        al = batch.iter().chain(&m_l).chain(&n_l).copied().collect();
        let shape: Vec<usize> = al.iter().map(|c| size_of[c]).collect();
        acc = Tensor::f32(&shape, out);
    }
    let drop: Vec<u32> = al.iter().copied().filter(|c| !out_letters.contains(c)).collect();
    let (acc, al) = sum_axes(&acc, &al, &drop);
    Ok(vec![permute_to(&acc, &al, &out_letters)])
}

// ------------------------------------------------------------ DFT

pub fn dft(ctx: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let r = x.rank();
    if r < 2 {
        return Err(Error("DFT: rank < 2".into()));
    }
    let complex = x.shape[r - 1] == 2;
    let axis_raw = if ctx.opset >= 20 { opt(inputs, 2).map(|t| t.i64s()[0]).unwrap_or(-2) } else { node.attr_i("axis", 1) };
    let ax = norm_axis(axis_raw, r)?;
    if ax == r - 1 {
        return Err(Error("DFT: axis cannot be the complex dimension".into()));
    }
    let inverse = node.attr_i("inverse", 0) != 0;
    let onesided = node.attr_i("onesided", 0) != 0;
    let n_in = x.shape[ax];
    let n = opt(inputs, 1).filter(|t| !t.is_empty()).map(|t| t.i64s()[0] as usize).unwrap_or(n_in);
    let n_out = if onesided { n / 2 + 1 } else { n };
    let outer: usize = x.shape[..ax].iter().product();
    let inner: usize = x.shape[ax + 1..r - 1].iter().product();
    let c = x.shape[r - 1];
    let v = x.f32s();
    let plan = crate::audio::Fft::new(n);
    let mut out = vec![0.0f32; outer * n_out * inner * 2];
    let mut re = vec![0.0f32; n];
    let mut im = vec![0.0f32; n];
    for o in 0..outer {
        for i in 0..inner {
            for k in 0..n {
                if k < n_in {
                    let p = ((o * n_in + k) * inner + i) * c;
                    re[k] = v[p];
                    im[k] = if complex { v[p + 1] } else { 0.0 };
                } else {
                    re[k] = 0.0;
                    im[k] = 0.0;
                }
            }
            plan.run(&mut re, &mut im, inverse);
            let s = if inverse { 1.0 / n as f32 } else { 1.0 };
            for k in 0..n_out {
                let p = ((o * n_out + k) * inner + i) * 2;
                out[p] = re[k] * s;
                out[p + 1] = im[k] * s;
            }
        }
    }
    let mut shape = x.shape.clone();
    shape[ax] = n_out;
    shape[r - 1] = 2;
    Ok(vec![Tensor::f32(&shape, out)])
}

// ------------------------------------------------------------ DeformConv

pub fn deform_conv(ctx: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let w = inp(inputs, 1)?;
    let off = inp(inputs, 2)?;
    let bias = opt(inputs, 3).map(|t| t.to_f32_vec());
    let mask = opt(inputs, 4).map(|t| t.to_f32_vec());
    if x.rank() != 4 {
        return Err(Error("DeformConv: only 2-D supported".into()));
    }
    let (g, oshape) = conv_geom(node, &x.shape, &w.shape)?;
    let og = node.attr_i("offset_group", 1).max(1) as usize;
    let (n, c) = (g.n, g.c_in);
    let [_, h, wd] = g.inp;
    let [_, oh, ow] = g.out;
    let [_, kh, kw] = g.k;
    let ks = kh * kw;
    let osz = oh * ow;
    let (icg, ocg) = (g.icg(), g.ocg());
    let cpo = c / og; // channels per offset group
    let xv = x.f32s();
    let ov = off.f32s();
    let wv = w.f32s();
    let mut out = vec![0.0f32; n * g.c_out * osz];
    let mut col = vec![0.0f32; icg * ks * osz];
    for b in 0..n {
        for grp in 0..g.group {
            for ci in 0..icg {
                let ch = grp * icg + ci;
                let ogi = ch / cpo.max(1);
                let plane = &xv[(b * c + ch) * h * wd..][..h * wd];
                for ky in 0..kh {
                    for kx in 0..kw {
                        let kidx = ky * kw + kx;
                        let r = ci * ks + kidx;
                        let oy_ch = ((b * og + ogi) * ks + kidx) * 2;
                        for oy in 0..oh {
                            for ox in 0..ow {
                                let p = oy * ow + ox;
                                let dy = ov[(oy_ch * oh + oy) * ow + ox];
                                let dx = ov[((oy_ch + 1) * oh + oy) * ow + ox];
                                let py = (oy * g.stride[1]) as f32 - g.pad[1] as f32 + (ky * g.dil[1]) as f32 + dy;
                                let px = (ox * g.stride[2]) as f32 - g.pad[2] as f32 + (kx * g.dil[2]) as f32 + dx;
                                let mut val = bilinear_zero(plane, h, wd, py, px);
                                if let Some(m) = &mask {
                                    val *= m[(((b * og + ogi) * ks + kidx) * oh + oy) * ow + ox];
                                }
                                col[r * osz + p] = val;
                            }
                        }
                    }
                }
            }
            let wg = &wv[grp * ocg * icg * ks..(grp + 1) * ocg * icg * ks];
            let o = &mut out[(b * g.c_out + grp * ocg) * osz..][..ocg * osz];
            sgemm(ocg, osz, icg * ks, wg, icg * ks, 1, &col, osz, 1, o, osz, false, ctx.threads);
        }
    }
    if let Some(bv) = bias {
        for (i, plane) in out.chunks_mut(osz).enumerate() {
            let bb = bv[i % g.c_out];
            plane.iter_mut().for_each(|v| *v += bb);
        }
    }
    Ok(vec![Tensor::f32(&oshape, out)])
}

/// Bilinear sample with zero outside (torchvision deform_conv semantics).
fn bilinear_zero(p: &[f32], h: usize, w: usize, y: f32, x: f32) -> f32 {
    if y <= -1.0 || x <= -1.0 || y >= h as f32 || x >= w as f32 {
        return 0.0;
    }
    let (y0, x0) = (y.floor() as i64, x.floor() as i64);
    let (fy, fx) = (y - y0 as f32, x - x0 as f32);
    let at = |yy: i64, xx: i64| if yy < 0 || xx < 0 || yy >= h as i64 || xx >= w as i64 { 0.0 } else { p[yy as usize * w + xx as usize] };
    at(y0, x0) * (1.0 - fy) * (1.0 - fx) + at(y0, x0 + 1) * (1.0 - fy) * fx + at(y0 + 1, x0) * fy * (1.0 - fx) + at(y0 + 1, x0 + 1) * fy * fx
}

// ------------------------------------------------------------ contrib fusions

pub fn skip_layer_norm(ctx: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let skip = inp(inputs, 1)?;
    let mut s = binary_tensors("Add", x, skip, false)?;
    if let Some(b) = opt(inputs, 4) {
        s = binary_tensors("Add", &s, b, false)?;
    }
    let gamma = inp(inputs, 2)?;
    let beta = opt(inputs, 3);
    let mut ln = Node { op_type: "LayerNormalization".into(), ..Default::default() };
    ln.attrs.push(crate::onnx::attr_f("epsilon", node.attr_f("epsilon", 1e-12)));
    let mut r = super::nn::layer_norm(ctx, &ln, &[Some(&s), Some(gamma), beta])?;
    r.push(s);
    Ok(r)
}

pub fn bias_gelu(_: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let s = match opt(inputs, 1) {
        Some(b) => binary_tensors("Add", x, b, false)?,
        None => x.cast(crate::DType::F32),
    };
    let f: fn(f32) -> f32 = if node.op_type == "FastGelu" { gelu_tanh } else { gelu };
    let v: Vec<f32> = s.f32s().iter().map(|&e| f(e)).collect();
    Ok(vec![Tensor::f32(&s.shape, v)])
}

// ------------------------------------------------------------ MediaPipe custom pooling (NHWC)

fn nhwc_pool_geom(node: &Node, h: usize, w: usize) -> (usize, usize, usize, usize, usize, usize, usize, usize) {
    let k = node.attr_ints("kernel_shape").unwrap_or_else(|| vec![2, 2]);
    let s = node.attr_ints("strides").unwrap_or_else(|| vec![2, 2]);
    let (kh, kw, sh, sw) = (k[0] as usize, k[1] as usize, s[0] as usize, s[1] as usize);
    let same = node.attr_s_or("padding", "VALID") == "SAME";
    let (oh, ow) = if same { (h.div_ceil(sh), w.div_ceil(sw)) } else { ((h - kh) / sh + 1, (w - kw) / sw + 1) };
    let ph = if same { (((oh - 1) * sh + kh).saturating_sub(h)) / 2 } else { 0 };
    let pw = if same { (((ow - 1) * sw + kw).saturating_sub(w)) / 2 } else { 0 };
    (kh, kw, sh, sw, oh, ow, ph, pw)
}

/// MediaPipe `MaxPoolingWithArgmax2D`: NHWC values plus flat (y*W + x)*C + c indices.
pub fn max_pool_argmax_nhwc(_: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let (n, h, w, c) = (x.shape[0], x.shape[1], x.shape[2], x.shape[3]);
    let (kh, kw, sh, sw, oh, ow, ph, pw) = nhwc_pool_geom(node, h, w);
    let v = x.f32s();
    let mut out = vec![f32::NEG_INFINITY; n * oh * ow * c];
    let mut idx = vec![0.0f32; n * oh * ow * c];
    for b in 0..n {
        for oy in 0..oh {
            for ox in 0..ow {
                for ch in 0..c {
                    let o = ((b * oh + oy) * ow + ox) * c + ch;
                    for ky in 0..kh {
                        for kx in 0..kw {
                            let (iy, ix) = ((oy * sh + ky) as isize - ph as isize, (ox * sw + kx) as isize - pw as isize);
                            if iy < 0 || ix < 0 || iy as usize >= h || ix as usize >= w {
                                continue;
                            }
                            let flat = (iy as usize * w + ix as usize) * c + ch;
                            let val = v[b * h * w * c + flat];
                            if val > out[o] {
                                out[o] = val;
                                idx[o] = flat as f32;
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(vec![Tensor::f32(&[n, oh, ow, c], out), Tensor::f32(&[n, oh, ow, c], idx)])
}

/// MediaPipe `MaxUnpooling2D`: scatters values to the argmax positions.
pub fn max_unpool_nhwc(_: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let ids = inp(inputs, 1)?.f32s();
    let (n, h, w, c) = (x.shape[0], x.shape[1], x.shape[2], x.shape[3]);
    let k = node.attr_ints("kernel_shape").unwrap_or_else(|| vec![2, 2]);
    let s = node.attr_ints("strides").unwrap_or_else(|| vec![2, 2]);
    let same = node.attr_s_or("padding", "VALID") == "SAME";
    let (oh, ow) = if same { (h * s[0] as usize, w * s[1] as usize) } else { ((h - 1) * s[0] as usize + k[0] as usize, (w - 1) * s[1] as usize + k[1] as usize) };
    let v = x.f32s();
    let mut out = vec![0.0f32; n * oh * ow * c];
    let per = h * w * c;
    for b in 0..n {
        for i in 0..per {
            let t = ids[b * per + i] as usize;
            if t < oh * ow * c {
                out[b * oh * ow * c + t] = v[b * per + i];
            }
        }
    }
    Ok(vec![Tensor::f32(&[n, oh, ow, c], out)])
}

// ------------------------------------------------------------ MultiHeadAttention

/// `[B, L, N*H]` view of a key/value input given as `[B, L, D]` or `[B, N, L, H]`.
fn to_bld(t: &Tensor, heads: usize) -> Result<(Vec<f32>, usize, usize)> {
    match t.rank() {
        3 => Ok((t.to_f32_vec(), t.shape[1], t.shape[2])),
        4 => {
            let (b, n, l, h) = (t.shape[0], t.shape[1], t.shape[2], t.shape[3]);
            if n != heads {
                return Err(Error(format!("MultiHeadAttention: BNSH input has {n} heads, expected {heads}")));
            }
            let v = t.f32s();
            let mut out = vec![0.0f32; b * l * n * h];
            for bi in 0..b {
                for ni in 0..n {
                    for li in 0..l {
                        let src = ((bi * n + ni) * l + li) * h;
                        let dst = (bi * l + li) * n * h + ni * h;
                        out[dst..dst + h].copy_from_slice(&v[src..src + h]);
                    }
                }
            }
            Ok((out, l, n * h))
        }
        r => Err(Error(format!("MultiHeadAttention: unsupported key/value rank {r}"))),
    }
}

/// onnxruntime `com.microsoft::MultiHeadAttention`: separate (or packed
/// QKV / KV) inputs, optional QKV bias, key padding mask and attention bias.
/// Outputs `[B, S, N*Hv]` (+ present key / value in BNSH when requested).
pub fn multi_head_attention(ctx: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let n = node.attr_i("num_heads", 0).max(1) as usize;
    let q_in = inp(inputs, 0)?;
    let b = q_in.shape[0];
    // Unpack the query / key / value as [B, S|L, D].
    let (mut q, s, d, mut k, l, mut v, dv) = match (opt(inputs, 1), opt(inputs, 2)) {
        (None, None) if q_in.rank() == 5 => {
            // packed QKV [B, S, N, 3, H]
            let (s, h) = (q_in.shape[1], q_in.shape[4]);
            let src = q_in.f32s();
            let mut parts = [vec![0.0f32; b * s * n * h], vec![0.0f32; b * s * n * h], vec![0.0f32; b * s * n * h]];
            for bi in 0..b {
                for si in 0..s {
                    for ni in 0..n {
                        for (j, part) in parts.iter_mut().enumerate() {
                            let from = (((bi * s + si) * n + ni) * 3 + j) * h;
                            let to = (bi * s + si) * n * h + ni * h;
                            part[to..to + h].copy_from_slice(&src[from..from + h]);
                        }
                    }
                }
            }
            let [pq, pk, pv] = parts;
            (pq, s, n * h, pk, s, pv, n * h)
        }
        (Some(kt), None) if kt.rank() == 5 => {
            // packed KV [B, L, N, 2, H]
            let (l, h) = (kt.shape[1], kt.shape[4]);
            let src = kt.f32s();
            let mut pk = vec![0.0f32; b * l * n * h];
            let mut pv = vec![0.0f32; b * l * n * h];
            for bi in 0..b {
                for li in 0..l {
                    for ni in 0..n {
                        let from = ((bi * l + li) * n + ni) * 2 * h;
                        let to = (bi * l + li) * n * h + ni * h;
                        pk[to..to + h].copy_from_slice(&src[from..from + h]);
                        pv[to..to + h].copy_from_slice(&src[from + h..from + 2 * h]);
                    }
                }
            }
            (q_in.to_f32_vec(), q_in.shape[1], q_in.shape[2], pk, l, pv, n * h)
        }
        (Some(kt), Some(vt)) => {
            let (kk, l, dk) = to_bld(kt, n)?;
            let (vv, lv, dv) = to_bld(vt, n)?;
            if lv != l || dk != q_in.shape[2] {
                return Err(Error(format!("MultiHeadAttention: shapes q {:?} k {:?} v {:?}", q_in.shape, kt.shape, vt.shape)));
            }
            (q_in.to_f32_vec(), q_in.shape[1], q_in.shape[2], kk, l, vv, dv)
        }
        _ => return Err(Error("MultiHeadAttention: unsupported input combination".into())),
    };
    let (h, hv) = (d / n, dv / n);
    // QKV bias [D + D + Dv].
    if let Some(bias) = opt(inputs, 3).filter(|t| !t.is_empty()) {
        let bv = bias.f32s();
        if bv.len() == d + d + dv {
            for (x, bo, w) in [(&mut q, 0, d), (&mut k, d, d), (&mut v, 2 * d, dv)] {
                for (i, val) in x.iter_mut().enumerate() {
                    *val += bv[bo + i % w];
                }
            }
        }
    }
    let scale = node.attr_f("scale", 0.0);
    let scale = if scale == 0.0 { 1.0 / (h as f32).sqrt() } else { scale };
    let pad_mask = opt(inputs, 4).filter(|t| !t.is_empty()).map(|t| (t.shape.clone(), t.to_i64_vec()));
    let attn_bias = opt(inputs, 5).filter(|t| !t.is_empty()).map(|t| (t.shape.clone(), t.to_f32_vec()));
    let unidirectional = node.attr_i("unidirectional", 0) != 0;
    let mut out = vec![0.0f32; b * s * dv];
    let mut scores = vec![0.0f32; s * l];
    for bi in 0..b {
        for hi in 0..n {
            let qa = &q[bi * s * d + hi * h..];
            let kb = &k[bi * l * d + hi * h..];
            sgemm(s, l, h, qa, d, 1, kb, 1, d, &mut scores, l, false, ctx.threads);
            for si in 0..s {
                let row = &mut scores[si * l..(si + 1) * l];
                for (li, x) in row.iter_mut().enumerate() {
                    *x *= scale;
                    if let Some((shape, ab)) = &attn_bias {
                        // [B|1, N|1, S, L]
                        let (bb, nb) = (if shape[0] == 1 { 0 } else { bi }, if shape[1] == 1 { 0 } else { hi });
                        *x += ab[((bb * shape[1] + nb) * s + si) * l + li];
                    }
                    if let Some((shape, m)) = &pad_mask {
                        let keep = if shape.len() == 2 { m[bi * l + li] != 0 } else { li < m[bi] as usize };
                        if !keep {
                            *x = f32::NEG_INFINITY;
                        }
                    }
                    if unidirectional && li > si + l.saturating_sub(s) {
                        *x = f32::NEG_INFINITY;
                    }
                }
                let mx = row.iter().copied().fold(f32::NEG_INFINITY, f32::max);
                let mut sum = 0.0;
                for x in row.iter_mut() {
                    *x = if mx.is_finite() { (*x - mx).exp() } else { 0.0 };
                    sum += *x;
                }
                if sum > 0.0 {
                    row.iter_mut().for_each(|x| *x /= sum);
                }
            }
            let vb = &v[bi * l * dv + hi * hv..];
            let oc = &mut out[bi * s * dv + hi * hv..];
            sgemm(s, hv, l, &scores, l, 1, vb, dv, 1, oc, dv, false, ctx.threads);
        }
    }
    let mut res = vec![Tensor::f32(&[b, s, dv], out)];
    if node.outputs.len() > 1 {
        let to_bnsh = |x: &[f32], len: usize, w: usize| {
            let hh = w / n;
            let mut o = vec![0.0f32; b * n * len * hh];
            for bi in 0..b {
                for ni in 0..n {
                    for li in 0..len {
                        let src = (bi * len + li) * w + ni * hh;
                        let dst = ((bi * n + ni) * len + li) * hh;
                        o[dst..dst + hh].copy_from_slice(&x[src..src + hh]);
                    }
                }
            }
            Tensor::f32(&[b, n, len, hh], o)
        };
        res.push(to_bnsh(&k, l, d));
        res.push(to_bnsh(&v, l, dv));
    }
    Ok(res)
}
