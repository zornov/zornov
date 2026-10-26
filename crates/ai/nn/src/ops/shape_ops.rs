// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Data-movement and shape operators (dtype-generic).

use super::{OpCtx, inp, opt};
use crate::dispatch_dtype;
use crate::error::{Error, Result};
use crate::onnx::Node;
use crate::tensor::{DType, Data, Elem, Tensor, numel, strides};

pub fn norm_axis(axis: i64, rank: usize) -> Result<usize> {
    let r = rank as i64;
    let a = if axis < 0 { axis + r } else { axis };
    if a < 0 || a >= r.max(1) {
        return Err(Error(format!("axis {axis} out of range for rank {rank}")));
    }
    Ok(a as usize)
}

/// Integer list from an optional input, else from an attribute.
pub fn ints_from(inputs: &[Option<&Tensor>], i: usize, node: &Node, attr: &str) -> Option<Vec<i64>> {
    if let Some(t) = opt(inputs, i) {
        return Some(t.to_i64_vec());
    }
    node.attr_ints(attr)
}

// ------------------------------------------------------------ alias ops (shape only)

/// Output shape of Reshape / Flatten / Squeeze / Unsqueeze / Identity / Dropout.
pub fn alias_shape(ctx: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<usize>> {
    let x = inp(inputs, 0)?;
    let sh = &x.shape;
    match node.op_type.as_str() {
        "Reshape" => {
            let target = ints_from(inputs, 1, node, "shape").ok_or_else(|| Error("Reshape: no shape".into()))?;
            let allowzero = node.attr_i("allowzero", 0) != 0;
            reshape_target(sh, &target, allowzero)
        }
        "Flatten" => {
            let axis = node.attr_i("axis", 1);
            let a = if axis < 0 { axis + sh.len() as i64 } else { axis } as usize;
            if a > sh.len() {
                return Err(Error("Flatten: axis out of range".into()));
            }
            Ok(vec![sh[..a].iter().product(), sh[a..].iter().product()])
        }
        "Squeeze" => {
            let axes = ints_from(inputs, 1, node, "axes");
            match axes {
                None => Ok(sh.iter().copied().filter(|&d| d != 1).collect()),
                Some(ax) => {
                    let ax: Vec<usize> = ax.iter().map(|&a| norm_axis(a, sh.len())).collect::<Result<_>>()?;
                    Ok(sh.iter().enumerate().filter(|(i, _)| !ax.contains(i)).map(|(_, &d)| d).collect())
                }
            }
        }
        "Unsqueeze" => {
            let ax = ints_from(inputs, 1, node, "axes").ok_or_else(|| Error("Unsqueeze: no axes".into()))?;
            let r = sh.len() + ax.len();
            let mut ax: Vec<usize> = ax.iter().map(|&a| norm_axis(a, r)).collect::<Result<_>>()?;
            ax.sort();
            let mut out = Vec::with_capacity(r);
            let mut src = sh.iter();
            for i in 0..r {
                if ax.contains(&i) {
                    out.push(1);
                } else {
                    out.push(*src.next().ok_or_else(|| Error("Unsqueeze: bad axes".into()))?);
                }
            }
            Ok(out)
        }
        "Identity" | "Dropout" => Ok(sh.clone()),
        o => {
            let _ = ctx;
            Err(Error(format!("{o} is not an alias op")))
        }
    }
}

pub fn reshape_target(sh: &[usize], target: &[i64], allowzero: bool) -> Result<Vec<usize>> {
    let total = numel(sh);
    let mut out = Vec::with_capacity(target.len());
    let mut infer = None;
    for (i, &d) in target.iter().enumerate() {
        if d == -1 {
            if infer.is_some() {
                return Err(Error("Reshape: more than one -1".into()));
            }
            infer = Some(i);
            out.push(1);
        } else if d == 0 && !allowzero {
            out.push(*sh.get(i).ok_or_else(|| Error("Reshape: 0 dim beyond input rank".into()))?);
        } else if d < 0 {
            return Err(Error(format!("Reshape: bad dim {d}")));
        } else {
            out.push(d as usize);
        }
    }
    if let Some(i) = infer {
        let known: usize = out.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, &d)| d).product();
        if known == 0 || total % known != 0 {
            return Err(Error(format!("Reshape: cannot infer -1 reshaping {sh:?} to {target:?}")));
        }
        out[i] = total / known;
    }
    if numel(&out) != total {
        return Err(Error(format!("Reshape: {sh:?} to {target:?} changes the element count")));
    }
    Ok(out)
}

pub fn alias_op(ctx: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let shape = alias_shape(ctx, node, inputs)?;
    let x = inp(inputs, 0)?;
    let t = Tensor { shape, data: x.data.clone() };
    if node.op_type == "Dropout" && node.outputs.len() > 1 {
        let n = t.len();
        let s = t.shape.clone();
        return Ok(vec![t, Tensor::bool(&s, vec![true; n])]);
    }
    Ok(vec![t])
}

// ------------------------------------------------------------ transpose

pub fn transpose_data<T: Copy + Send + Sync>(v: &[T], shape: &[usize], perm: &[usize]) -> Vec<T> {
    let n = numel(shape);
    if shape.len() <= 1 || perm.iter().enumerate().all(|(i, &p)| i == p) || n == 0 {
        return v.to_vec();
    }
    // Drop size-1 axes, then merge input axes that stay adjacent and in order.
    let ist = strides(shape);
    let kept: Vec<usize> = perm.iter().copied().filter(|&p| shape[p] != 1).collect();
    let mut dims: Vec<usize> = Vec::new(); // output dims
    let mut st: Vec<usize> = Vec::new(); // input stride per output dim
    let mut last_in: Option<usize> = None;
    for &p in &kept {
        match last_in {
            // p directly follows the previous input axis (ignoring size-1 axes in between)
            Some(q) if p > q && (q + 1..p).all(|k| shape[k] == 1) => {
                let l = dims.len() - 1;
                dims[l] *= shape[p];
                st[l] = ist[p];
            }
            _ => {
                dims.push(shape[p]);
                st.push(ist[p]);
            }
        }
        last_in = Some(p);
    }
    if dims.len() <= 1 {
        return v.to_vec();
    }
    let r = dims.len();
    let ost = strides(&dims);
    let mut out: Vec<T> = Vec::with_capacity(n);
    // SAFETY: every element of `out` is written exactly once below.
    #[allow(clippy::uninit_vec)]
    unsafe {
        out.set_len(n)
    };
    let dst = crate::kernels::par::SendPtr(out.as_mut_ptr());
    let threads = crate::kernels::par::resolve_threads(0);
    let par = if n >= 1 << 15 { threads } else { 1 };
    // Output position of the input-innermost (stride 1) axis.
    let k = st.iter().position(|&s| s == 1).unwrap_or(r - 1);
    if k == r - 1 {
        // Contiguous runs of the last output dim.
        let inner = dims[r - 1];
        let outer = n / inner;
        crate::kernels::par::par_range(outer, par, |lo, hi| {
            for o in lo..hi {
                let (mut rem, mut base) = (o, 0usize);
                for d in (0..r - 1).rev() {
                    base += (rem % dims[d]) * st[d];
                    rem /= dims[d];
                }
                // SAFETY: rows o are disjoint between workers.
                unsafe { std::ptr::copy_nonoverlapping(v.as_ptr().add(base), dst.add(o * inner), inner) };
            }
        });
        return out;
    }
    // Blocked 2-D transpose of (output dim k, output dim r-1) for each outer index.
    let (rows, cols) = (dims[k], dims[r - 1]);
    let (in_cs, out_rs) = (st[r - 1], ost[k]);
    let others: Vec<usize> = (0..r - 1).filter(|&d| d != k).collect();
    let outer: usize = others.iter().map(|&d| dims[d]).product();
    const B: usize = 32;
    // Work unit: (outer index, band of B columns), so one huge 2-D transpose
    // still spreads over every thread.
    let bands = cols.div_ceil(B);
    crate::kernels::par::par_range(outer * bands, par, |lo, hi| {
        for u in lo..hi {
            let (o, band) = (u / bands, u % bands);
            let (mut rem, mut ib, mut ob) = (o, 0usize, 0usize);
            for &d in others.iter().rev() {
                let i = rem % dims[d];
                rem /= dims[d];
                ib += i * st[d];
                ob += i * ost[d];
            }
            let c0 = band * B;
            for r0 in (0..rows).step_by(B) {
                for c in c0..(c0 + B).min(cols) {
                    for rr in r0..(r0 + B).min(rows) {
                        // SAFETY: (o, rr, c) map to distinct output elements.
                        unsafe { *dst.add(ob + rr * out_rs + c) = v[ib + rr + c * in_cs] };
                    }
                }
            }
        }
    });
    out
}

pub fn transpose_tensor(x: &Tensor, perm: &[usize]) -> Tensor {
    fn go<T: Elem>(x: &Tensor, perm: &[usize]) -> Data {
        T::wrap(transpose_data(x.elems::<T>().unwrap(), &x.shape, perm))
    }
    let data = dispatch_dtype!(x.dtype(), go(x, perm));
    Tensor { shape: perm.iter().map(|&p| x.shape[p]).collect(), data }
}

pub fn transpose(_: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let r = x.rank();
    let perm: Vec<usize> = match node.attr_ints("perm") {
        Some(p) => p.iter().map(|&a| norm_axis(a, r)).collect::<Result<_>>()?,
        None => (0..r).rev().collect(),
    };
    if perm.len() != r {
        return Err(Error(format!("Transpose: perm {perm:?} for rank {r}")));
    }
    let t0 = std::time::Instant::now();
    let y = transpose_tensor(x, &perm);
    if std::env::var_os("STUDIO_TRACE_TRANSPOSE").is_some() {
        eprintln!("transpose {:?} {:?} {:?} {:.2} ms", x.dtype(), x.shape, perm, t0.elapsed().as_secs_f64() * 1e3);
    }
    Ok(vec![y])
}

// ------------------------------------------------------------ concat / split / slice

pub fn concat_tensors(ins: &[&Tensor], axis: i64) -> Result<Tensor> {
    let first = ins.iter().find(|t| t.len() > 0 || t.rank() > 0).unwrap_or(&ins[0]);
    let r = first.rank();
    let ax = norm_axis(axis, r)?;
    let dt = first.dtype();
    let ins: Vec<Tensor> = ins.iter().filter(|t| t.rank() == r).map(|t| if t.dtype() == dt { (*t).clone() } else { t.cast(dt) }).collect();
    let mut oshape = first.shape.clone();
    oshape[ax] = ins.iter().map(|t| t.shape[ax]).sum();
    for t in &ins {
        for d in 0..r {
            if d != ax && t.shape[d] != oshape[d] {
                return Err(Error(format!("Concat: shapes {:?} and {:?} differ off axis {ax}", first.shape, t.shape)));
            }
        }
    }
    let outer: usize = oshape[..ax].iter().product();
    let inner: usize = oshape[ax + 1..].iter().product();
    fn go<T: Elem>(ins: &[Tensor], outer: usize, inner: usize, ax: usize, n: usize) -> Data {
        let mut out = Vec::with_capacity(n);
        for o in 0..outer {
            for t in ins {
                let blk = t.shape[ax] * inner;
                out.extend_from_slice(&t.elems::<T>().unwrap()[o * blk..(o + 1) * blk]);
            }
        }
        T::wrap(out)
    }
    let n = numel(&oshape);
    let data = dispatch_dtype!(dt, go(&ins, outer, inner, ax, n));
    Ok(Tensor { shape: oshape, data })
}

pub fn concat(_: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let ins: Vec<&Tensor> = inputs.iter().flatten().copied().collect();
    if ins.is_empty() {
        return Err(Error("Concat: no inputs".into()));
    }
    Ok(vec![concat_tensors(&ins, node.attr_i("axis", 0))?])
}

/// Copies `x[.., start..start+len, ..]` along `ax`.
pub fn narrow(x: &Tensor, ax: usize, start: usize, len: usize) -> Tensor {
    let outer: usize = x.shape[..ax].iter().product();
    let inner: usize = x.shape[ax + 1..].iter().product();
    let d = x.shape[ax];
    fn go<T: Elem>(x: &Tensor, outer: usize, inner: usize, d: usize, start: usize, len: usize) -> Data {
        let v = x.elems::<T>().unwrap();
        let mut out = Vec::with_capacity(outer * len * inner);
        for o in 0..outer {
            out.extend_from_slice(&v[(o * d + start) * inner..(o * d + start + len) * inner]);
        }
        T::wrap(out)
    }
    let data = dispatch_dtype!(x.dtype(), go(x, outer, inner, d, start, len));
    let mut shape = x.shape.clone();
    shape[ax] = len;
    Tensor { shape, data }
}

pub fn split(ctx: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let ax = norm_axis(node.attr_i("axis", 0), x.rank())?;
    let d = x.shape[ax];
    let nout = node.outputs.len().max(1);
    let sizes: Vec<usize> = match ints_from(inputs, 1, node, "split") {
        Some(s) if !s.is_empty() => s.iter().map(|&v| v.max(0) as usize).collect(),
        _ => {
            let n = if ctx.opset >= 18 { node.attr_i("num_outputs", nout as i64) as usize } else { nout };
            let chunk = d.div_ceil(n.max(1));
            (0..n).map(|i| chunk.min(d.saturating_sub(i * chunk))).collect()
        }
    };
    if sizes.iter().sum::<usize>() != d {
        return Err(Error(format!("Split: sizes {sizes:?} do not sum to {d}")));
    }
    let mut start = 0;
    let mut out = Vec::new();
    for &s in &sizes {
        out.push(narrow(x, ax, start, s));
        start += s;
    }
    Ok(out)
}

pub fn slice(_: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let r = x.rank();
    let starts = ints_from(inputs, 1, node, "starts").ok_or_else(|| Error("Slice: no starts".into()))?;
    let ends = ints_from(inputs, 2, node, "ends").ok_or_else(|| Error("Slice: no ends".into()))?;
    let axes: Vec<usize> = match ints_from(inputs, 3, node, "axes") {
        Some(a) => a.iter().map(|&v| norm_axis(v, r)).collect::<Result<_>>()?,
        None => (0..starts.len()).collect(),
    };
    let steps = opt(inputs, 4).map(|t| t.to_i64_vec()).unwrap_or_else(|| vec![1; starts.len()]);
    let mut st = vec![0i64; r];
    let mut sp = vec![1i64; r];
    let mut cnt: Vec<usize> = x.shape.clone();
    for (i, &ax) in axes.iter().enumerate() {
        let dim = x.shape[ax] as i64;
        let step = steps[i];
        if step == 0 {
            return Err(Error("Slice: step 0".into()));
        }
        let mut s = starts[i];
        let mut e = ends[i];
        if s < 0 {
            s += dim;
        }
        if e < 0 {
            e += dim;
        }
        if step > 0 {
            s = s.clamp(0, dim);
            e = e.clamp(0, dim);
            cnt[ax] = if e > s { ((e - s + step - 1) / step) as usize } else { 0 };
        } else {
            s = s.clamp(0, dim - 1);
            e = e.clamp(-1, dim - 1);
            cnt[ax] = if s > e { ((s - e + (-step) - 1) / (-step)) as usize } else { 0 };
        }
        st[ax] = s;
        sp[ax] = step;
    }
    let ist = strides(&x.shape);
    fn go<T: Elem>(x: &Tensor, cnt: &[usize], st: &[i64], sp: &[i64], ist: &[usize]) -> Data {
        let v = x.elems::<T>().unwrap();
        let n = numel(cnt);
        let mut out = Vec::with_capacity(n);
        if n == 0 {
            return T::wrap(out);
        }
        let r = cnt.len();
        if r == 0 {
            return T::wrap(v.to_vec());
        }
        let mut idx = vec![0usize; r];
        let inner = cnt[r - 1];
        let outer = n / inner;
        for _ in 0..outer {
            let mut base: i64 = 0;
            for d in 0..r - 1 {
                base += (st[d] + idx[d] as i64 * sp[d]) * ist[d] as i64;
            }
            base += st[r - 1];
            if sp[r - 1] == 1 {
                out.extend_from_slice(&v[base as usize..base as usize + inner]);
            } else {
                out.extend((0..inner).map(|i| v[(base + i as i64 * sp[r - 1]) as usize]));
            }
            for d in (0..r - 1).rev() {
                idx[d] += 1;
                if idx[d] < cnt[d] {
                    break;
                }
                idx[d] = 0;
            }
        }
        T::wrap(out)
    }
    let data = dispatch_dtype!(x.dtype(), go(x, &cnt, &st, &sp, &ist));
    Ok(vec![Tensor { shape: cnt, data }])
}

// ------------------------------------------------------------ gather / scatter

fn fix_index(i: i64, dim: usize) -> Result<usize> {
    let j = if i < 0 { i + dim as i64 } else { i };
    if j < 0 || j as usize >= dim {
        return Err(Error(format!("index {i} out of range for dimension {dim}")));
    }
    Ok(j as usize)
}

pub fn gather(_: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let idx = inp(inputs, 1)?;
    let ax = norm_axis(node.attr_i("axis", 0), x.rank())?;
    let d = x.shape[ax];
    let ids: Vec<usize> = idx.i64s().iter().map(|&i| fix_index(i, d)).collect::<Result<_>>()?;
    let outer: usize = x.shape[..ax].iter().product();
    let inner: usize = x.shape[ax + 1..].iter().product();
    let mut shape = x.shape[..ax].to_vec();
    shape.extend_from_slice(&idx.shape);
    shape.extend_from_slice(&x.shape[ax + 1..]);
    fn go<T: Elem>(x: &Tensor, ids: &[usize], outer: usize, inner: usize, d: usize) -> Data {
        let v = x.elems::<T>().unwrap();
        let mut out = Vec::with_capacity(outer * ids.len() * inner);
        for o in 0..outer {
            for &i in ids {
                let s = (o * d + i) * inner;
                out.extend_from_slice(&v[s..s + inner]);
            }
        }
        T::wrap(out)
    }
    let data = dispatch_dtype!(x.dtype(), go(x, &ids, outer, inner, d));
    Ok(vec![Tensor { shape, data }])
}

pub fn gather_elements(_: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let idx = inp(inputs, 1)?;
    let ax = norm_axis(node.attr_i("axis", 0), x.rank())?;
    let xs = strides(&x.shape);
    let ish = idx.shape.clone();
    let iv = idx.i64s();
    let mut offs = Vec::with_capacity(iv.len());
    let r = ish.len();
    let mut pos = vec![0usize; r];
    for &i in iv.iter() {
        let j = fix_index(i, x.shape[ax])?;
        let mut o = 0;
        for d in 0..r {
            o += if d == ax { j } else { pos[d] } * xs[d];
        }
        offs.push(o);
        for d in (0..r).rev() {
            pos[d] += 1;
            if pos[d] < ish[d] {
                break;
            }
            pos[d] = 0;
        }
    }
    fn go<T: Elem>(x: &Tensor, offs: &[usize]) -> Data {
        let v = x.elems::<T>().unwrap();
        T::wrap(offs.iter().map(|&o| v[o]).collect())
    }
    let data = dispatch_dtype!(x.dtype(), go(x, &offs));
    Ok(vec![Tensor { shape: ish, data }])
}

pub fn gather_nd(_: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let idx = inp(inputs, 1)?;
    let b = node.attr_i("batch_dims", 0) as usize;
    let k = *idx.shape.last().ok_or_else(|| Error("GatherND: scalar indices".into()))?;
    let batch: usize = x.shape[..b].iter().product();
    let slice_shape = &x.shape[b + k..];
    let slice: usize = slice_shape.iter().product();
    let xs = strides(&x.shape[b..]);
    let per_batch_x: usize = x.shape[b..].iter().product();
    let n_idx: usize = idx.shape[b..idx.rank() - 1].iter().product();
    let iv = idx.i64s();
    let mut offs = Vec::new();
    for bi in 0..batch {
        for t in 0..n_idx {
            let tup = &iv[(bi * n_idx + t) * k..(bi * n_idx + t + 1) * k];
            let mut o = bi * per_batch_x;
            for (d, &i) in tup.iter().enumerate() {
                o += fix_index(i, x.shape[b + d])? * xs[d];
            }
            offs.push(o);
        }
    }
    fn go<T: Elem>(x: &Tensor, offs: &[usize], slice: usize) -> Data {
        let v = x.elems::<T>().unwrap();
        let mut out = Vec::with_capacity(offs.len() * slice);
        for &o in offs {
            out.extend_from_slice(&v[o..o + slice]);
        }
        T::wrap(out)
    }
    let data = dispatch_dtype!(x.dtype(), go(x, &offs, slice));
    let mut shape = idx.shape[..idx.rank() - 1].to_vec();
    shape.extend_from_slice(slice_shape);
    Ok(vec![Tensor { shape, data }])
}

fn reduce_into(dst: &mut f32, v: f32, red: &str) {
    match red {
        "add" => *dst += v,
        "mul" => *dst *= v,
        "max" => *dst = dst.max(v),
        "min" => *dst = dst.min(v),
        _ => *dst = v,
    }
}

/// Updates in the data's dtype (f16 models compute f32 updates internally).
fn same_dtype<'a>(upd: &'a Tensor, like: &Tensor) -> std::borrow::Cow<'a, Tensor> {
    if upd.dtype() == like.dtype() { std::borrow::Cow::Borrowed(upd) } else { std::borrow::Cow::Owned(upd.cast(like.dtype())) }
}

pub fn scatter_nd(_: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let idx = inp(inputs, 1)?;
    let upd_c = same_dtype(inp(inputs, 2)?, x);
    let upd: &Tensor = &upd_c;
    let red = node.attr_s_or("reduction", "none");
    let k = *idx.shape.last().ok_or_else(|| Error("ScatterND: scalar indices".into()))?;
    let xs = strides(&x.shape);
    let slice: usize = x.shape[k..].iter().product();
    let iv = idx.i64s();
    let n = iv.len() / k.max(1);
    if x.dtype() == DType::I64 || x.dtype() == DType::Bool || red == "none" {
        // generic copy path (reductions on ints via f32 below)
        if red == "none" {
            fn go<T: Elem>(x: &Tensor, upd: &Tensor, iv: &[i64], k: usize, n: usize, slice: usize, xs: &[usize]) -> Result<Data> {
                let mut v = x.elems::<T>().unwrap().to_vec();
                let u = upd.elems::<T>().ok_or_else(|| Error("ScatterND: updates dtype differs".into()))?;
                for t in 0..n {
                    let mut o = 0;
                    for d in 0..k {
                        o += fix_index(iv[t * k + d], x.shape[d])? * xs[d];
                    }
                    v[o..o + slice].copy_from_slice(&u[t * slice..(t + 1) * slice]);
                }
                Ok(T::wrap(v))
            }
            let data = dispatch_dtype!(x.dtype(), go(x, upd, &iv, k, n, slice, &xs))?;
            return Ok(vec![Tensor { shape: x.shape.clone(), data }]);
        }
    }
    let mut v = x.to_f32_vec();
    let u = upd.f32s();
    for t in 0..n {
        let mut o = 0;
        for d in 0..k {
            o += fix_index(iv[t * k + d], x.shape[d])? * xs[d];
        }
        for s in 0..slice {
            reduce_into(&mut v[o + s], u[t * slice + s], &red);
        }
    }
    Ok(vec![Tensor::f32(&x.shape, v).cast(x.dtype())])
}

pub fn scatter_elements(_: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let idx = inp(inputs, 1)?;
    let upd_c = same_dtype(inp(inputs, 2)?, x);
    let upd: &Tensor = &upd_c;
    let ax = norm_axis(node.attr_i("axis", 0), x.rank())?;
    let red = node.attr_s_or("reduction", "none");
    let xs = strides(&x.shape);
    let ish = idx.shape.clone();
    let r = ish.len();
    let iv = idx.i64s();
    let mut offs = Vec::with_capacity(iv.len());
    let mut pos = vec![0usize; r];
    for &i in iv.iter() {
        let j = fix_index(i, x.shape[ax])?;
        let mut o = 0;
        for d in 0..r {
            o += if d == ax { j } else { pos[d] } * xs[d];
        }
        offs.push(o);
        for d in (0..r).rev() {
            pos[d] += 1;
            if pos[d] < ish[d] {
                break;
            }
            pos[d] = 0;
        }
    }
    if red == "none" {
        fn go<T: Elem>(x: &Tensor, upd: &Tensor, offs: &[usize]) -> Result<Data> {
            let mut v = x.elems::<T>().unwrap().to_vec();
            let u = upd.elems::<T>().ok_or_else(|| Error("ScatterElements: updates dtype differs".into()))?;
            for (i, &o) in offs.iter().enumerate() {
                v[o] = u[i];
            }
            Ok(T::wrap(v))
        }
        let data = dispatch_dtype!(x.dtype(), go(x, upd, &offs))?;
        return Ok(vec![Tensor { shape: x.shape.clone(), data }]);
    }
    let mut v = x.to_f32_vec();
    let u = upd.f32s();
    for (i, &o) in offs.iter().enumerate() {
        reduce_into(&mut v[o], u[i], &red);
    }
    Ok(vec![Tensor::f32(&x.shape, v).cast(x.dtype())])
}

// ------------------------------------------------------------ expand / tile / pad

pub fn expand_to(x: &Tensor, out: &[usize]) -> Tensor {
    fn go<T: Elem>(x: &Tensor, out: &[usize]) -> Data {
        let v = x.elems::<T>().unwrap();
        let mut r = Vec::with_capacity(numel(out));
        super::elementwise::for_each_bcast(out, &[&x.shape], |_, o| r.push(v[o[0]]));
        T::wrap(r)
    }
    let data = dispatch_dtype!(x.dtype(), go(x, out));
    Tensor { shape: out.to_vec(), data }
}

pub fn expand(_: &OpCtx, _: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let s: Vec<usize> = inp(inputs, 1)?.i64s().iter().map(|&d| d.max(0) as usize).collect();
    let out = super::elementwise::broadcast_shape(&x.shape, &s)?;
    Ok(vec![expand_to(x, &out)])
}

pub fn tile(_: &OpCtx, _: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let reps = inp(inputs, 1)?.to_i64_vec();
    if reps.len() != x.rank() {
        return Err(Error("Tile: repeats length must equal rank".into()));
    }
    let out: Vec<usize> = x.shape.iter().zip(&reps).map(|(&d, &r)| d * r.max(0) as usize).collect();
    let xs = strides(&x.shape);
    fn go<T: Elem>(x: &Tensor, out: &[usize], xs: &[usize]) -> Data {
        let v = x.elems::<T>().unwrap();
        let n = numel(out);
        let r = out.len();
        let mut res = Vec::with_capacity(n);
        let mut idx = vec![0usize; r];
        for _ in 0..n {
            let mut o = 0;
            for d in 0..r {
                o += (idx[d] % x.shape[d]) * xs[d];
            }
            res.push(v[o]);
            for d in (0..r).rev() {
                idx[d] += 1;
                if idx[d] < out[d] {
                    break;
                }
                idx[d] = 0;
            }
        }
        T::wrap(res)
    }
    let data = dispatch_dtype!(x.dtype(), go(x, &out, &xs));
    Ok(vec![Tensor { shape: out, data }])
}

pub fn pad(_: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let r = x.rank();
    let raw = ints_from(inputs, 1, node, "pads").ok_or_else(|| Error("Pad: no pads".into()))?;
    let axes: Vec<usize> = match opt(inputs, 3) {
        Some(a) => a.i64s().iter().map(|&v| norm_axis(v, r)).collect::<Result<_>>()?,
        None => (0..r).collect(),
    };
    if raw.len() != 2 * axes.len() {
        return Err(Error(format!("Pad: {} pads for {} axes", raw.len(), axes.len())));
    }
    let mut before = vec![0i64; r];
    let mut after = vec![0i64; r];
    for (i, &a) in axes.iter().enumerate() {
        before[a] = raw[i];
        after[a] = raw[i + axes.len()];
    }
    let mode = node.attr_s_or("mode", "constant");
    let cval = opt(inputs, 2).map(|t| t.f32s()[0]).unwrap_or(node.attr_f("value", 0.0));
    let out: Vec<usize> = (0..r).map(|d| (x.shape[d] as i64 + before[d] + after[d]).max(0) as usize).collect();
    let xs = strides(&x.shape);
    // per dim: map output coordinate -> source coordinate (or None = constant)
    let maps: Vec<Vec<Option<usize>>> = (0..r)
        .map(|d| {
            let n = x.shape[d] as i64;
            (0..out[d] as i64)
                .map(|o| {
                    let mut i = o - before[d];
                    if i >= 0 && i < n {
                        return Some(i as usize);
                    }
                    if n == 0 {
                        return None;
                    }
                    match mode.as_str() {
                        "edge" => Some(i.clamp(0, n - 1) as usize),
                        "wrap" => Some(i.rem_euclid(n) as usize),
                        "reflect" => {
                            if n == 1 {
                                return Some(0);
                            }
                            let period = 2 * (n - 1);
                            i = i.rem_euclid(period);
                            if i >= n {
                                i = period - i;
                            }
                            Some(i as usize)
                        }
                        _ => None,
                    }
                })
                .collect()
        })
        .collect();
    fn go<T: Elem>(x: &Tensor, out: &[usize], maps: &[Vec<Option<usize>>], xs: &[usize], c: T) -> Data {
        let v = x.elems::<T>().unwrap();
        let n = numel(out);
        let r = out.len();
        let mut res = Vec::with_capacity(n);
        if r == 0 {
            return T::wrap(v.to_vec());
        }
        let mut idx = vec![0usize; r];
        for _ in 0..n {
            let mut o = 0;
            let mut ok = true;
            for d in 0..r {
                match maps[d][idx[d]] {
                    Some(s) => o += s * xs[d],
                    None => {
                        ok = false;
                        break;
                    }
                }
            }
            res.push(if ok { v[o] } else { c });
            for d in (0..r).rev() {
                idx[d] += 1;
                if idx[d] < out[d] {
                    break;
                }
                idx[d] = 0;
            }
        }
        T::wrap(res)
    }
    let data = match x.dtype() {
        DType::F32 => go::<f32>(x, &out, &maps, &xs, cval),
        DType::F16 => go::<u16>(x, &out, &maps, &xs, crate::tensor::f32_to_f16(cval)),
        DType::I64 => go::<i64>(x, &out, &maps, &xs, cval as i64),
        DType::I8 => go::<i8>(x, &out, &maps, &xs, cval as i8),
        DType::U8 => go::<u8>(x, &out, &maps, &xs, cval as u8),
        DType::Bool => go::<bool>(x, &out, &maps, &xs, cval != 0.0),
    };
    Ok(vec![Tensor { shape: out, data }])
}

// ------------------------------------------------------------ shape / constants

pub fn shape(_: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let r = x.rank() as i64;
    let clampi = |v: i64| (if v < 0 { v + r } else { v }).clamp(0, r) as usize;
    let s = clampi(node.attr_i("start", 0));
    let e = clampi(node.attr_i("end", r));
    let dims: Vec<i64> = if s < e { x.shape[s..e].iter().map(|&d| d as i64).collect() } else { vec![] };
    Ok(vec![Tensor::i64(&[dims.len()], dims)])
}

pub fn size(_: &OpCtx, _: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    Ok(vec![Tensor::scalar_i64(inp(inputs, 0)?.len() as i64)])
}

pub fn constant_of_shape(_: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let s: Vec<usize> = inp(inputs, 0)?.i64s().iter().map(|&d| d.max(0) as usize).collect();
    let v = node.attr_t("value").cloned().unwrap_or_else(|| Tensor::f32(&[1], vec![0.0]));
    let n = numel(&s);
    let data = match &v.data {
        Data::F32(x) => Data::F32(vec![x[0]; n]),
        Data::F16(x) => Data::F16(vec![x[0]; n]),
        Data::I64(x) => Data::I64(vec![x[0]; n]),
        Data::I8(x) => Data::I8(vec![x[0]; n]),
        Data::U8(x) => Data::U8(vec![x[0]; n]),
        Data::Bool(x) => Data::Bool(vec![x[0]; n]),
    };
    Ok(vec![Tensor { shape: s, data }])
}

pub fn constant(_: &OpCtx, node: &Node, _: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    if let Some(t) = node.attr_t("value") {
        return Ok(vec![t.clone()]);
    }
    if node.has("value_float") {
        return Ok(vec![Tensor::scalar_f32(node.attr_f("value_float", 0.0))]);
    }
    if let Some(v) = node.attr_floats("value_floats") {
        return Ok(vec![Tensor::f32(&[v.len()], v)]);
    }
    if node.has("value_int") {
        return Ok(vec![Tensor::scalar_i64(node.attr_i("value_int", 0))]);
    }
    if let Some(v) = node.attr_ints("value_ints") {
        return Ok(vec![Tensor::i64(&[v.len()], v)]);
    }
    Err(Error("Constant: unsupported value attribute".into()))
}

pub fn range(_: &OpCtx, _: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let s = inp(inputs, 0)?;
    let l = inp(inputs, 1)?;
    let d = inp(inputs, 2)?;
    if s.dtype().is_float() {
        let (a, b, c) = (s.f32s()[0], l.f32s()[0], d.f32s()[0]);
        let n = (((b - a) / c).ceil()).max(0.0) as usize;
        let v: Vec<f32> = (0..n).map(|i| a + i as f32 * c).collect();
        return Ok(vec![Tensor::f32(&[n], v)]);
    }
    let (a, b, c) = (s.i64s()[0], l.i64s()[0], d.i64s()[0]);
    if c == 0 {
        return Err(Error("Range: delta 0".into()));
    }
    let n = ((b - a) as f64 / c as f64).ceil().max(0.0) as usize;
    let v: Vec<i64> = (0..n).map(|i| a + i as i64 * c).collect();
    Ok(vec![Tensor::i64(&[n], v).cast(s.dtype())])
}

// ------------------------------------------------------------ pixel shuffles

fn reshape_transpose(x: &Tensor, shape6: &[usize], perm: &[usize], out: &[usize]) -> Tensor {
    let t = Tensor { shape: shape6.to_vec(), data: x.data.clone() };
    let mut r = transpose_tensor(&t, perm);
    r.shape = out.to_vec();
    r
}

pub fn depth_to_space(_: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let b = node.attr_i("blocksize", 1) as usize;
    if x.rank() != 4 || x.shape[1] % (b * b) != 0 {
        return Err(Error("DepthToSpace: needs NCHW with C divisible by blocksize²".into()));
    }
    let [n, c, h, w] = [x.shape[0], x.shape[1], x.shape[2], x.shape[3]];
    let out = [n, c / (b * b), h * b, w * b];
    Ok(vec![if node.attr_s_or("mode", "DCR") == "CRD" {
        reshape_transpose(x, &[n, c / (b * b), b, b, h, w], &[0, 1, 4, 2, 5, 3], &out)
    } else {
        reshape_transpose(x, &[n, b, b, c / (b * b), h, w], &[0, 3, 4, 1, 5, 2], &out)
    }])
}

pub fn space_to_depth(_: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let b = node.attr_i("blocksize", 1) as usize;
    if x.rank() != 4 || x.shape[2] % b != 0 || x.shape[3] % b != 0 {
        return Err(Error("SpaceToDepth: needs NCHW with H, W divisible by blocksize".into()));
    }
    let [n, c, h, w] = [x.shape[0], x.shape[1], x.shape[2], x.shape[3]];
    Ok(vec![reshape_transpose(x, &[n, c, h / b, b, w / b, b], &[0, 3, 5, 1, 2, 4], &[n, c * b * b, h / b, w / b])])
}

// ------------------------------------------------------------ misc

pub fn trilu(_: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let k = opt(inputs, 1).map(|t| t.i64s()[0]).unwrap_or(0);
    let upper = node.attr_i("upper", 1) != 0;
    let r = x.rank();
    if r < 2 {
        return Err(Error("Trilu: rank < 2".into()));
    }
    let (h, w) = (x.shape[r - 2], x.shape[r - 1]);
    fn go<T: Elem>(x: &Tensor, h: usize, w: usize, k: i64, upper: bool) -> Data {
        let mut v = x.elems::<T>().unwrap().to_vec();
        for (p, e) in v.iter_mut().enumerate() {
            let i = ((p / w) % h) as i64;
            let j = (p % w) as i64;
            let keep = if upper { j - i >= k } else { j - i <= k };
            if !keep {
                *e = T::default();
            }
        }
        T::wrap(v)
    }
    let data = dispatch_dtype!(x.dtype(), go(x, h, w, k, upper));
    Ok(vec![Tensor { shape: x.shape.clone(), data }])
}

pub fn one_hot(_: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let idx = inp(inputs, 0)?;
    let depth = inp(inputs, 1)?.f32s()[0] as i64;
    let vals = inp(inputs, 2)?;
    let r = idx.rank() + 1;
    let ax = norm_axis(node.attr_i("axis", -1), r)?;
    let mut shape = idx.shape.clone();
    shape.insert(ax, depth.max(0) as usize);
    let outer: usize = idx.shape[..ax].iter().product();
    let inner: usize = idx.shape[ax..].iter().product();
    let iv = idx.i64s();
    let d = depth.max(0) as usize;
    fn go<T: Elem>(vals: &Tensor, iv: &[i64], outer: usize, inner: usize, d: usize, depth: i64) -> Data {
        let v = vals.elems::<T>().unwrap();
        let (off, on) = (v[0], v[1]);
        let mut out = vec![off; outer * d * inner];
        for o in 0..outer {
            for i in 0..inner {
                let mut c = iv[o * inner + i];
                if c < 0 {
                    c += depth;
                }
                if c >= 0 && c < depth {
                    out[(o * d + c as usize) * inner + i] = on;
                }
            }
        }
        T::wrap(out)
    }
    let data = dispatch_dtype!(vals.dtype(), go(vals, &iv, outer, inner, d, depth));
    Ok(vec![Tensor { shape, data }])
}

pub fn non_zero(_: &OpCtx, _: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let r = x.rank();
    let v = x.f32s();
    let st = strides(&x.shape);
    let hits: Vec<usize> = v.iter().enumerate().filter(|(_, x)| **x != 0.0).map(|(i, _)| i).collect();
    let n = hits.len();
    let mut out = vec![0i64; r.max(1) * n];
    for (j, &h) in hits.iter().enumerate() {
        for d in 0..r {
            out[d * n + j] = ((h / st[d]) % x.shape[d]) as i64;
        }
    }
    Ok(vec![Tensor::i64(&[r.max(1), n], out)])
}

pub fn cum_sum(_: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let ax = norm_axis(inp(inputs, 1)?.i64s()[0], x.rank())?;
    let excl = node.attr_i("exclusive", 0) != 0;
    let rev = node.attr_i("reverse", 0) != 0;
    let outer: usize = x.shape[..ax].iter().product();
    let d = x.shape[ax];
    let inner: usize = x.shape[ax + 1..].iter().product();
    macro_rules! scan {
        ($v:expr, $zero:expr) => {{
            let v = $v;
            let mut out = vec![$zero; v.len()];
            for o in 0..outer {
                for i in 0..inner {
                    let mut acc = $zero;
                    for s in 0..d {
                        let k = if rev { d - 1 - s } else { s };
                        let p = (o * d + k) * inner + i;
                        if excl {
                            out[p] = acc;
                            acc += v[p];
                        } else {
                            acc += v[p];
                            out[p] = acc;
                        }
                    }
                }
            }
            out
        }};
    }
    if x.dtype() == DType::I64 {
        let o = scan!(x.as_i64().unwrap(), 0i64);
        return Ok(vec![Tensor::i64(&x.shape, o)]);
    }
    let o = scan!(&x.f32s(), 0.0f32);
    Ok(vec![Tensor::f32(&x.shape, o)])
}

pub fn top_k(_: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let x = inp(inputs, 0)?;
    let k = match opt(inputs, 1) {
        Some(t) => t.i64s()[0],
        None => node.attr_i("k", 1),
    } as usize;
    let ax = norm_axis(node.attr_i("axis", -1), x.rank())?;
    let largest = node.attr_i("largest", 1) != 0;
    let outer: usize = x.shape[..ax].iter().product();
    let d = x.shape[ax];
    if k > d {
        return Err(Error(format!("TopK: k={k} > axis size {d}")));
    }
    let inner: usize = x.shape[ax + 1..].iter().product();
    let v = x.f32s();
    let mut vals = vec![0.0f32; outer * k * inner];
    let mut idxs = vec![0i64; outer * k * inner];
    let mut buf: Vec<(f32, usize)> = Vec::with_capacity(d);
    for o in 0..outer {
        for i in 0..inner {
            buf.clear();
            buf.extend((0..d).map(|s| (v[(o * d + s) * inner + i], s)));
            buf.sort_by(|a, b| {
                let ord = if largest { b.0.partial_cmp(&a.0) } else { a.0.partial_cmp(&b.0) };
                ord.unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1))
            });
            for (j, &(val, s)) in buf.iter().take(k).enumerate() {
                vals[(o * k + j) * inner + i] = val;
                idxs[(o * k + j) * inner + i] = s as i64;
            }
        }
    }
    let mut shape = x.shape.clone();
    shape[ax] = k;
    let vt = Tensor::f32(&shape, vals);
    let vt = if x.dtype() == DType::F32 { vt } else { vt.cast(x.dtype()) };
    Ok(vec![vt, Tensor::i64(&shape, idxs)])
}
