// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Per-op tests: one-node models built with `GraphBuilder`, run through
//! `Session`, compared against naive reference implementations written here.
//!
//! Every op is run three ways and the results must agree:
//! 1. all inputs are runtime graph inputs;
//! 2. all inputs are initializers with constant folding (the op runs at build time);
//! 3. all inputs are initializers without folding (exercises weight pre-packing).

#![allow(clippy::too_many_arguments, clippy::needless_range_loop, clippy::type_complexity)]

use studio_nn::onnx::{self, AttrValue, Attribute, Graph, GraphBuilder, ValueInfo, attr_f, attr_i, attr_ints, attr_s};
use studio_nn::{DType, Model, Options, Session, Tensor};

// ====================================================================== helpers

struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    fn next_u32(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 32) as u32
    }
    /// Uniform in [-1, 1).
    fn f(&mut self) -> f32 {
        (self.next_u32() as f64 / 4294967296.0 * 2.0 - 1.0) as f32
    }
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (self.f() + 1.0) * 0.5 * (hi - lo)
    }
    fn int(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.next_u32() as i64) % (hi - lo)
    }
    fn vec(&mut self, n: usize) -> Vec<f32> {
        (0..n).map(|_| self.f()).collect()
    }
    fn t(&mut self, shape: &[usize]) -> Tensor {
        let n = shape.iter().product();
        Tensor::f32(shape, self.vec(n))
    }
    fn t_range(&mut self, shape: &[usize], lo: f32, hi: f32) -> Tensor {
        let n = shape.iter().product();
        Tensor::f32(shape, (0..n).map(|_| self.range(lo, hi)).collect())
    }
}

fn numel(s: &[usize]) -> usize {
    s.iter().product()
}

fn strides(s: &[usize]) -> Vec<usize> {
    let mut st = vec![1; s.len()];
    for i in (0..s.len().saturating_sub(1)).rev() {
        st[i] = st[i + 1] * s[i + 1];
    }
    st
}

/// Calls `f` with every multi-index of `shape` in row-major order.
fn for_idx(shape: &[usize], mut f: impl FnMut(&[usize])) {
    let n = numel(shape);
    let mut idx = vec![0usize; shape.len()];
    for _ in 0..n {
        f(&idx);
        for d in (0..shape.len()).rev() {
            idx[d] += 1;
            if idx[d] < shape[d] {
                break;
            }
            idx[d] = 0;
        }
    }
}

fn flat(shape: &[usize], idx: &[usize]) -> usize {
    idx.iter().zip(strides(shape)).map(|(i, s)| i * s).sum()
}

fn bshape(a: &[usize], b: &[usize]) -> Vec<usize> {
    let r = a.len().max(b.len());
    (0..r)
        .map(|i| {
            let da = if i + a.len() >= r { a[i + a.len() - r] } else { 1 };
            let db = if i + b.len() >= r { b[i + b.len() - r] } else { 1 };
            assert!(da == db || da == 1 || db == 1, "test bug: {a:?} vs {b:?}");
            da.max(db)
        })
        .collect()
}

/// Offset into a tensor of shape `s` broadcast to an output multi-index.
fn boff(s: &[usize], out_idx: &[usize]) -> usize {
    let r = out_idx.len();
    let st = strides(s);
    let mut o = 0;
    for i in 0..s.len() {
        let oi = out_idx[r - s.len() + i];
        o += if s[i] == 1 { 0 } else { oi } * st[i];
    }
    o
}

/// Builds a one-node model: input `i` is named `x{i}` (None = omitted).
/// Modes: 0 = all runtime inputs, 1 / 2 = all initializers (with / without
/// folding), 3 = input 0 at runtime and the rest initializers. Modes 0 and 3
/// also output `Shape(y_j)` so wrong static shape inference (which drives
/// `Shape` folding) is caught.
fn one_node(opset: i64, domain: &str, op: &str, ins: &[Option<Tensor>], attrs: &[Attribute], nout: usize, mode: u8) -> Model {
    let mut b = GraphBuilder::new(opset);
    let mut names = Vec::new();
    for (i, t) in ins.iter().enumerate() {
        match t {
            None => names.push(String::new()),
            Some(t) => {
                let n = format!("x{i}");
                let runtime = mode == 0 || (mode == 3 && i == 0);
                if runtime {
                    let sh: Vec<i64> = t.shape.iter().map(|&d| d as i64).collect();
                    b.input(&n, t.dtype(), &sh);
                } else {
                    b.init(&n, t.clone());
                }
                names.push(n);
            }
        }
    }
    let outs: Vec<String> = (0..nout).map(|j| format!("y{j}")).collect();
    let in_refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
    let out_refs: Vec<&str> = outs.iter().map(|s| s.as_str()).collect();
    if domain.is_empty() {
        b.node(op, &in_refs, &out_refs, attrs.to_vec());
    } else {
        b.node_domain(domain, op, &in_refs, &out_refs, attrs.to_vec());
    }
    for o in &outs {
        b.output(o, DType::F32);
    }
    if mode == 0 || mode == 3 {
        for o in &outs {
            let so = format!("{o}_shape");
            b.node("Shape", &[o.as_str()], &[so.as_str()], vec![]);
            b.output(&so, DType::I64);
        }
    }
    b.build()
}

fn try_run_mode(opset: i64, domain: &str, op: &str, ins: &[Option<Tensor>], attrs: &[Attribute], nout: usize, mode: u8) -> studio_nn::Result<Vec<Tensor>> {
    let m = one_node(opset, domain, op, ins, attrs, nout, mode);
    let opts = Options { threads: 3, constant_folding: mode != 2, ..Default::default() };
    let s = Session::new(&m, opts)?;
    let names = ["x0", "x1", "x2", "x3", "x4", "x5", "x6", "x7", "x8", "x9"];
    let feeds: Vec<(&str, Tensor)> = match mode {
        0 => ins.iter().enumerate().filter_map(|(i, t)| t.as_ref().map(|t| (names[i], t.clone()))).collect(),
        3 => ins[0].iter().map(|t| ("x0", t.clone())).collect(),
        _ => Vec::new(),
    };
    let mut outs: Vec<Tensor> = s.run(&feeds)?.into_iter().map(|(_, t)| t).collect();
    if mode == 0 || mode == 3 {
        let shapes = outs.split_off(nout);
        for (j, (y, sh)) in outs.iter().zip(&shapes).enumerate() {
            let want: Vec<i64> = y.shape.iter().map(|&d| d as i64).collect();
            assert_eq!(sh.to_i64_vec(), want, "{op}: Shape(y{j}) disagrees with the computed output shape (static shape inference bug?), mode {mode}");
        }
    }
    Ok(outs)
}

fn same(a: &Tensor, b: &Tensor) -> bool {
    if a.shape != b.shape || a.dtype() != b.dtype() {
        return false;
    }
    let scale = a.f32s().iter().fold(1.0f32, |m, v| m.max(v.abs()));
    a.max_abs_diff(b) <= 2e-5 * scale
}

/// Runs a one-node model in all three modes and checks they agree.
fn run_d(opset: i64, domain: &str, op: &str, ins: &[Option<Tensor>], attrs: &[Attribute], nout: usize) -> Vec<Tensor> {
    let a = try_run_mode(opset, domain, op, ins, attrs, nout, 0).unwrap_or_else(|e| panic!("{op} (runtime inputs): {e}"));
    for mode in [1u8, 2, 3] {
        let b = try_run_mode(opset, domain, op, ins, attrs, nout, mode).unwrap_or_else(|e| panic!("{op} (mode {mode}): {e}"));
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(&b) {
            assert!(same(x, y), "{op}: mode {mode} differs from runtime mode: {:?} {:?} vs {:?} {:?} (diff {})", x.shape, x.dtype(), y.shape, y.dtype(), x.max_abs_diff(y));
        }
    }
    a
}

fn run(opset: i64, op: &str, ins: &[Option<Tensor>], attrs: &[Attribute], nout: usize) -> Vec<Tensor> {
    run_d(opset, "", op, ins, attrs, nout)
}

fn run1(opset: i64, op: &str, ins: &[Tensor], attrs: &[Attribute]) -> Tensor {
    let ins: Vec<Option<Tensor>> = ins.iter().cloned().map(Some).collect();
    run(opset, op, &ins, attrs, 1).remove(0)
}

#[track_caller]
fn check(got: &Tensor, shape: &[usize], want: &[f32], tol: f32, what: &str) {
    assert_eq!(got.shape, shape, "{what}: shape");
    let g = got.f32s();
    assert_eq!(g.len(), want.len(), "{what}: len");
    let mut worst = (0.0f32, 0usize);
    for (i, (&a, &b)) in g.iter().zip(want).enumerate() {
        let d = if a.is_nan() && b.is_nan() || a == b { 0.0 } else { (a - b).abs() / (1.0 + b.abs()) };
        if !(d <= worst.0) {
            worst = (d, i);
        }
    }
    if !(worst.0 <= tol) {
        let i = worst.1;
        panic!("{what}: element {i} got {} want {} (rel err {}, tol {tol})\n got: {:?}\nwant: {:?}", g[i], want[i], worst.0, &g[..g.len().min(24)], &want[..want.len().min(24)]);
    }
}

#[track_caller]
fn check_t(got: &Tensor, want: &Tensor, tol: f32, what: &str) {
    check(got, &want.shape, &want.f32s(), tol, what);
}

#[track_caller]
fn check_i(got: &Tensor, shape: &[usize], want: &[i64], what: &str) {
    assert_eq!(got.shape, shape, "{what}: shape");
    assert_eq!(&got.to_i64_vec()[..], want, "{what}");
}

fn ints(name: &str, v: &[i64]) -> Attribute {
    attr_ints(name, v)
}

fn i64t(v: &[i64]) -> Tensor {
    Tensor::i64(&[v.len()], v.to_vec())
}

// ====================================================================== elementwise

fn erf_ref(x: f64) -> f64 {
    // Maclaurin series in f64 (fine for |x| <= 3), 1 beyond.
    if x.abs() > 3.5 {
        return x.signum();
    }
    let mut sum = 0.0;
    let mut term = x; // x^(2n+1) (-1)^n / n!
    for n in 0..80 {
        sum += term / (2 * n + 1) as f64;
        term *= -x * x / (n + 1) as f64;
    }
    sum * 2.0 / std::f64::consts::PI.sqrt()
}

#[test]
fn unary_ops() {
    let mut r = Rng::new(1);
    let sig = |x: f64| 1.0 / (1.0 + (-x).exp());
    let sp = |x: f64| (1.0 + x.exp()).ln();
    type F = Box<dyn Fn(f64) -> f64>;
    // (op, attrs, input range, reference)
    let cases: Vec<(&str, Vec<Attribute>, (f32, f32), F)> = vec![
        ("Relu", vec![], (-3.0, 3.0), Box::new(|x| x.max(0.0))),
        ("LeakyRelu", vec![attr_f("alpha", 0.1)], (-3.0, 3.0), Box::new(|x| if x >= 0.0 { x } else { 0.1 * x })),
        ("LeakyRelu", vec![], (-3.0, 3.0), Box::new(|x| if x >= 0.0 { x } else { 0.01 * x })),
        ("Sigmoid", vec![], (-20.0, 20.0), Box::new(sig)),
        ("Tanh", vec![], (-5.0, 5.0), Box::new(|x| x.tanh())),
        ("HardSigmoid", vec![attr_f("alpha", 0.3), attr_f("beta", 0.4)], (-5.0, 5.0), Box::new(|x| (0.3 * x + 0.4).clamp(0.0, 1.0))),
        ("HardSwish", vec![], (-5.0, 5.0), Box::new(|x| x * (x / 6.0 + 0.5).clamp(0.0, 1.0))),
        ("Erf", vec![], (-3.0, 3.0), Box::new(erf_ref)),
        ("Softplus", vec![], (-30.0, 30.0), Box::new(sp)),
        ("Softsign", vec![], (-5.0, 5.0), Box::new(|x| x / (1.0 + x.abs()))),
        ("Mish", vec![], (-5.0, 5.0), Box::new(move |x| x * sp(x).tanh())),
        ("Elu", vec![attr_f("alpha", 0.7)], (-3.0, 3.0), Box::new(|x| if x >= 0.0 { x } else { 0.7 * (x.exp() - 1.0) })),
        ("Selu", vec![], (-3.0, 3.0), Box::new(|x| 1.0507009873554805 * if x > 0.0 { x } else { 1.6732632423543772 * (x.exp() - 1.0) })),
        ("Celu", vec![attr_f("alpha", 2.0)], (-3.0, 3.0), Box::new(|x| x.max(0.0) + (2.0 * ((x / 2.0).exp() - 1.0)).min(0.0))),
        ("ThresholdedRelu", vec![attr_f("alpha", 0.5)], (-3.0, 3.0), Box::new(|x| if x > 0.5 { x } else { 0.0 })),
        ("Sqrt", vec![], (0.0, 9.0), Box::new(|x| x.sqrt())),
        ("Exp", vec![], (-5.0, 5.0), Box::new(|x| x.exp())),
        ("Log", vec![], (0.01, 9.0), Box::new(|x| x.ln())),
        ("Abs", vec![], (-3.0, 3.0), Box::new(|x| x.abs())),
        ("Neg", vec![], (-3.0, 3.0), Box::new(|x| -x)),
        ("Reciprocal", vec![], (0.5, 3.0), Box::new(|x| 1.0 / x)),
        ("Floor", vec![], (-3.0, 3.0), Box::new(|x| x.floor())),
        ("Ceil", vec![], (-3.0, 3.0), Box::new(|x| x.ceil())),
        ("Sign", vec![], (-3.0, 3.0), Box::new(|x| if x > 0.0 { 1.0 } else if x < 0.0 { -1.0 } else { 0.0 })),
        ("Sin", vec![], (-5.0, 5.0), Box::new(|x| x.sin())),
        ("Cos", vec![], (-5.0, 5.0), Box::new(|x| x.cos())),
        ("Tan", vec![], (-1.2, 1.2), Box::new(|x| x.tan())),
        ("Asin", vec![], (-0.99, 0.99), Box::new(|x| x.asin())),
        ("Acos", vec![], (-0.99, 0.99), Box::new(|x| x.acos())),
        ("Atan", vec![], (-5.0, 5.0), Box::new(|x| x.atan())),
        ("Sinh", vec![], (-3.0, 3.0), Box::new(|x| x.sinh())),
        ("Cosh", vec![], (-3.0, 3.0), Box::new(|x| x.cosh())),
        ("Asinh", vec![], (-3.0, 3.0), Box::new(|x| x.asinh())),
        ("Acosh", vec![], (1.0, 5.0), Box::new(|x| x.acosh())),
        ("Atanh", vec![], (-0.95, 0.95), Box::new(|x| x.atanh())),
    ];
    for (op, attrs, (lo, hi), f) in cases {
        let x = r.t_range(&[2, 3, 5], lo, hi);
        let want: Vec<f32> = x.f32s().iter().map(|&v| f(v as f64) as f32).collect();
        let y = run1(13, op, &[x.clone()], &attrs);
        check(&y, &x.shape, &want, 2e-5, op);
    }
    // Gelu (opset 20), exact and tanh approximation
    let x = r.t_range(&[4, 7], -4.0, 4.0);
    let want: Vec<f32> = x.f32s().iter().map(|&v| (0.5 * v as f64 * (1.0 + erf_ref(v as f64 / 2f64.sqrt()))) as f32).collect();
    check(&run1(20, "Gelu", &[x.clone()], &[]), &x.shape, &want, 1e-5, "Gelu");
    let want: Vec<f32> = x
        .f32s()
        .iter()
        .map(|&v| {
            let v = v as f64;
            (0.5 * v * (1.0 + ((2.0 / std::f64::consts::PI).sqrt() * (v + 0.044715 * v * v * v)).tanh())) as f32
        })
        .collect();
    check(&run1(20, "Gelu", &[x.clone()], &[attr_s("approximate", "tanh")]), &x.shape, &want, 1e-5, "Gelu tanh");
    // Round: half to even
    let x = Tensor::f32(&[8], vec![0.5, 1.5, 2.5, -0.5, -1.5, 0.4, -2.6, 3.0]);
    check(&run1(13, "Round", &[x], &[]), &[8], &[0.0, 2.0, 2.0, -0.0, -2.0, 0.0, -3.0, 3.0], 0.0, "Round");
    // large input exercises the threaded path
    let x = r.t(&[300, 301]);
    let want: Vec<f32> = x.f32s().iter().map(|&v| sig(v as f64) as f32).collect();
    check(&run1(13, "Sigmoid", &[x.clone()], &[]), &x.shape, &want, 1e-6, "Sigmoid large");
    // integer unary ops keep their dtype
    let xi = Tensor::i64(&[4], vec![-3, 0, 5, -7]);
    let y = run1(13, "Abs", &[xi.clone()], &[]);
    assert_eq!(y.dtype(), DType::I64);
    check_i(&y, &[4], &[3, 0, 5, 7], "Abs i64");
    check_i(&run1(13, "Neg", &[xi.clone()], &[]), &[4], &[3, 0, -5, 7], "Neg i64");
    check_i(&run1(13, "Sign", &[xi], &[]), &[4], &[-1, 0, 1, -1], "Sign i64");
}

#[test]
fn isnan_isinf_not_clip() {
    let x = Tensor::f32(&[5], vec![1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -2.0]);
    let y = run1(13, "IsNaN", &[x.clone()], &[]);
    assert_eq!(y.dtype(), DType::Bool);
    assert_eq!(y.to_bool_vec(), vec![false, true, false, false, false]);
    assert_eq!(run1(13, "IsInf", &[x.clone()], &[]).to_bool_vec(), vec![false, false, true, true, false]);
    assert_eq!(run1(13, "IsInf", &[x.clone()], &[attr_i("detect_negative", 0)]).to_bool_vec(), vec![false, false, true, false, false]);
    assert_eq!(run1(13, "IsInf", &[x], &[attr_i("detect_positive", 0)]).to_bool_vec(), vec![false, false, false, true, false]);
    let b = Tensor::bool(&[3], vec![true, false, true]);
    assert_eq!(run1(13, "Not", &[b], &[]).to_bool_vec(), vec![false, true, false]);

    let mut r = Rng::new(2);
    let x = r.t_range(&[3, 4], -3.0, 3.0);
    // opset 11+: min/max inputs, either optional
    let want: Vec<f32> = x.f32s().iter().map(|v| v.clamp(-1.0, 0.5)).collect();
    let y = run(13, "Clip", &[Some(x.clone()), Some(Tensor::scalar_f32(-1.0)), Some(Tensor::scalar_f32(0.5))], &[], 1);
    check(&y[0], &x.shape, &want, 0.0, "Clip");
    let want: Vec<f32> = x.f32s().iter().map(|v| v.min(0.5)).collect();
    let y = run(13, "Clip", &[Some(x.clone()), None, Some(Tensor::scalar_f32(0.5))], &[], 1);
    check(&y[0], &x.shape, &want, 0.0, "Clip max only");
    let want: Vec<f32> = x.f32s().iter().map(|v| v.max(-1.0)).collect();
    let y = run(13, "Clip", &[Some(x.clone()), Some(Tensor::scalar_f32(-1.0))], &[], 1);
    check(&y[0], &x.shape, &want, 0.0, "Clip min only");
    // opset 6: attributes
    let want: Vec<f32> = x.f32s().iter().map(|v| v.clamp(-0.25, 0.75)).collect();
    check(&run1(6, "Clip", &[x.clone()], &[attr_f("min", -0.25), attr_f("max", 0.75)]), &x.shape, &want, 0.0, "Clip-6");
    // integer clip
    let xi = Tensor::i64(&[5], vec![-9, -1, 0, 4, 12]);
    let y = run(13, "Clip", &[Some(xi), Some(Tensor::scalar_i64(-2)), Some(Tensor::scalar_i64(5))], &[], 1);
    assert_eq!(y[0].dtype(), DType::I64);
    check_i(&y[0], &[5], &[-2, -1, 0, 4, 5], "Clip i64");
    // u8 clip keeps u8
    let xu = Tensor::u8(&[4], vec![0, 10, 200, 255]);
    let y = run(13, "Clip", &[Some(xu), Some(Tensor::u8(&[], vec![5])), Some(Tensor::u8(&[], vec![210]))], &[], 1);
    assert_eq!(y[0].dtype(), DType::U8, "Clip u8 dtype");
    check_i(&y[0], &[4], &[5, 10, 200, 210], "Clip u8");
}

const BCAST_CASES: &[(&[usize], &[usize])] = &[
    (&[2, 3, 4], &[2, 3, 4]),
    (&[2, 3, 4], &[]),
    (&[], &[2, 3, 4]),
    (&[2, 3, 4], &[4]),
    (&[2, 3, 4], &[3, 1]),
    (&[3, 1], &[1, 4]),
    (&[2, 1, 4], &[3, 1]),
    (&[1], &[2, 3]),
    (&[5, 1, 3], &[1, 4, 1]),
    (&[2, 3, 4], &[2, 1, 4]),
    (&[2, 3, 4], &[1, 3, 1]),
    (&[4, 1], &[4, 6]),
    (&[1, 3, 1, 5], &[2, 1, 4, 1]),
    (&[256, 300], &[256, 300]),
    (&[256, 300], &[300]),
];

fn bin_ref(a: &Tensor, b: &Tensor, f: impl Fn(f64, f64) -> f64) -> (Vec<usize>, Vec<f32>) {
    let out = bshape(&a.shape, &b.shape);
    let (av, bv) = (a.f32s(), b.f32s());
    let mut v = Vec::new();
    for_idx(&out, |i| v.push(f(av[boff(&a.shape, i)] as f64, bv[boff(&b.shape, i)] as f64) as f32));
    (out, v)
}

#[test]
fn binary_arith_broadcasting() {
    let mut r = Rng::new(3);
    type F = fn(f64, f64) -> f64;
    let ops: &[(&str, F, (f32, f32), (f32, f32))] = &[
        ("Add", |a, b| a + b, (-2.0, 2.0), (-2.0, 2.0)),
        ("Sub", |a, b| a - b, (-2.0, 2.0), (-2.0, 2.0)),
        ("Mul", |a, b| a * b, (-2.0, 2.0), (-2.0, 2.0)),
        ("Div", |a, b| a / b, (-2.0, 2.0), (0.5, 2.0)),
        ("Pow", |a, b| a.powf(b), (0.1, 3.0), (-2.0, 2.0)),
        ("PRelu", |a, b| if a < 0.0 { a * b } else { a }, (-2.0, 2.0), (0.0, 1.0)),
    ];
    for &(op, f, ra, rb) in ops {
        for &(sa, sb) in BCAST_CASES {
            if op == "PRelu" && numel(sb) > numel(sa) {
                continue; // PRelu broadcasts slope to X only
            }
            let a = r.t_range(sa, ra.0, ra.1);
            let b = r.t_range(sb, rb.0, rb.1);
            let (os, want) = bin_ref(&a, &b, f);
            let y = run1(13, op, &[a, b], &[]);
            check(&y, &os, &want, 2e-6, &format!("{op} {sa:?} {sb:?}"));
        }
    }
    // Pow with exponent 2 / 0.5 fast paths and integer exponent tensor
    let a = r.t_range(&[3, 4], 0.0, 3.0);
    for e in [2.0f32, 0.5, 3.0, -1.0] {
        let (os, want) = bin_ref(&a, &Tensor::scalar_f32(e), |x, y| x.powf(y));
        check(&run1(13, "Pow", &[a.clone(), Tensor::scalar_f32(e)], &[]), &os, &want, 1e-6, "Pow scalar");
    }
    let (os, want) = bin_ref(&a, &Tensor::scalar_f32(3.0), |x, y| x.powf(y));
    let y = run1(13, "Pow", &[a.clone(), Tensor::scalar_i64(3)], &[]);
    assert_eq!(y.dtype(), DType::F32);
    check(&y, &os, &want, 1e-6, "Pow int exponent");
    // Mod: fmod=1 on floats (C fmod), fmod=0 on ints (Python modulo)
    let a = Tensor::f32(&[6], vec![5.5, -5.5, 5.5, -5.5, 3.0, -0.0]);
    let b = Tensor::f32(&[6], vec![2.0, 2.0, -2.0, -2.0, 1.5, 2.0]);
    let y = run1(13, "Mod", &[a, b], &[attr_i("fmod", 1)]);
    check(&y, &[6], &[1.5, -1.5, 1.5, -1.5, 0.0, -0.0], 0.0, "Mod fmod");
    let a = Tensor::i64(&[6], vec![7, -7, 7, -7, 6, 0]);
    let b = Tensor::i64(&[6], vec![3, 3, -3, -3, 3, 5]);
    check_i(&run1(13, "Mod", &[a.clone(), b.clone()], &[]), &[6], &[1, 2, -2, -1, 0, 0], "Mod int");
    check_i(&run1(13, "Mod", &[a, b], &[attr_i("fmod", 1)]), &[6], &[1, -1, 1, -1, 0, 0], "Mod int fmod");
    // integer arithmetic keeps i64, Div truncates
    let a = Tensor::i64(&[2, 3], vec![7, -7, 9, 1, 2, 3]);
    let b = Tensor::i64(&[3], vec![2, 2, -4]);
    let y = run1(13, "Div", &[a.clone(), b.clone()], &[]);
    assert_eq!(y.dtype(), DType::I64);
    check_i(&y, &[2, 3], &[3, -3, -2, 0, 1, 0], "Div i64");
    check_i(&run1(13, "Mul", &[a.clone(), b.clone()], &[]), &[2, 3], &[14, -14, -36, 2, 4, -12], "Mul i64");
    check_i(&run1(13, "Sub", &[a.clone(), b.clone()], &[]), &[2, 3], &[5, -9, 13, -1, 0, 7], "Sub i64");
    // u8 add stays u8
    let y = run1(13, "Add", &[Tensor::u8(&[3], vec![1, 2, 3]), Tensor::u8(&[1], vec![10])], &[]);
    assert_eq!(y.dtype(), DType::U8);
    check_i(&y, &[3], &[11, 12, 13], "Add u8");
}

#[test]
fn variadic_ops() {
    let mut r = Rng::new(4);
    let a = r.t(&[2, 3, 4]);
    let b = r.t(&[3, 1]);
    let c = r.t(&[4]);
    let out = bshape(&bshape(&a.shape, &b.shape), &c.shape);
    let (av, bv, cv) = (a.f32s(), b.f32s(), c.f32s());
    let mut want: [Vec<f32>; 4] = Default::default();
    for_idx(&out, |i| {
        let (x, y, z) = (av[boff(&a.shape, i)], bv[boff(&b.shape, i)], cv[boff(&c.shape, i)]);
        want[0].push(x.max(y).max(z));
        want[1].push(x.min(y).min(z));
        want[2].push(x + y + z);
        want[3].push((x + y + z) / 3.0);
    });
    for (k, op) in ["Max", "Min", "Sum", "Mean"].iter().enumerate() {
        let y = run1(13, op, &[a.clone(), b.clone(), c.clone()], &[]);
        check(&y, &out, &want[k], 1e-6, op);
        // single input is identity
        let y = run1(13, op, &[a.clone()], &[]);
        check_t(&y, &a, 0.0, op);
    }
    // int Max
    let y = run1(13, "Max", &[Tensor::i64(&[3], vec![1, 5, -2]), Tensor::i64(&[1], vec![2])], &[]);
    assert_eq!(y.dtype(), DType::I64);
    check_i(&y, &[3], &[2, 5, 2], "Max i64");
}

#[test]
fn compare_logic_where() {
    let mut r = Rng::new(5);
    type F = fn(f64, f64) -> bool;
    let ops: &[(&str, F)] = &[("Equal", |a, b| a == b), ("Less", |a, b| a < b), ("Greater", |a, b| a > b), ("LessOrEqual", |a, b| a <= b), ("GreaterOrEqual", |a, b| a >= b)];
    for &(op, f) in ops {
        for &(sa, sb) in &BCAST_CASES[..12] {
            // small integer-valued floats so that Equal hits
            let a = Tensor::f32(sa, (0..numel(sa)).map(|_| r.int(-2, 3) as f32).collect());
            let b = Tensor::f32(sb, (0..numel(sb)).map(|_| r.int(-2, 3) as f32).collect());
            let (os, want) = bin_ref(&a, &b, |x, y| f(x, y) as u8 as f64);
            let y = run1(13, op, &[a.clone(), b.clone()], &[]);
            assert_eq!(y.dtype(), DType::Bool);
            check(&y, &os, &want, 0.0, &format!("{op} {sa:?} {sb:?}"));
            let y = run1(13, op, &[a.cast(DType::I64), b.cast(DType::I64)], &[]);
            check(&y, &os, &want, 0.0, &format!("{op} i64 {sa:?} {sb:?}"));
        }
    }
    type L = fn(bool, bool) -> bool;
    let lops: &[(&str, L)] = &[("And", |a, b| a && b), ("Or", |a, b| a || b), ("Xor", |a, b| a ^ b)];
    for &(op, f) in lops {
        for &(sa, sb) in &BCAST_CASES[..12] {
            let a = Tensor::bool(sa, (0..numel(sa)).map(|_| r.next_u32() & 1 == 1).collect());
            let b = Tensor::bool(sb, (0..numel(sb)).map(|_| r.next_u32() & 1 == 1).collect());
            let (os, want) = bin_ref(&a, &b, |x, y| f(x != 0.0, y != 0.0) as u8 as f64);
            check(&run1(13, op, &[a, b], &[]), &os, &want, 0.0, &format!("{op} {sa:?} {sb:?}"));
        }
    }
    // Where with three-way broadcasting
    let shapes: &[(&[usize], &[usize], &[usize])] = &[(&[2, 3, 4], &[2, 3, 4], &[2, 3, 4]), (&[3, 1], &[1, 4], &[]), (&[4], &[2, 3, 1], &[3, 4]), (&[1, 1], &[5], &[2, 1])];
    for &(sc, sx, sy) in shapes {
        let c = Tensor::bool(sc, (0..numel(sc)).map(|_| r.next_u32() & 1 == 1).collect());
        let x = r.t(sx);
        let y = r.t(sy);
        let out = bshape(&bshape(sc, sx), sy);
        let (cv, xv, yv) = (c.to_bool_vec(), x.f32s(), y.f32s());
        let mut want = Vec::new();
        for_idx(&out, |i| want.push(if cv[boff(sc, i)] { xv[boff(sx, i)] } else { yv[boff(sy, i)] }));
        check(&run1(13, "Where", &[c.clone(), x.clone(), y.clone()], &[]), &out, &want, 0.0, &format!("Where {sc:?} {sx:?} {sy:?}"));
        let yi = run1(13, "Where", &[c, x.cast(DType::I64), y.cast(DType::I64)], &[]);
        assert_eq!(yi.dtype(), DType::I64);
    }
}

#[test]
fn cast_ops() {
    let x = Tensor::f32(&[7], vec![-1.7, -0.2, 0.0, 0.6, 2.5, 300.0, -300.0]);
    let to = |t: i64| vec![attr_i("to", t)];
    let y = run1(13, "Cast", &[x.clone()], &to(7));
    assert_eq!(y.dtype(), DType::I64);
    check_i(&y, &[7], &[-1, 0, 0, 0, 2, 300, -300], "f32->i64");
    let y = run1(13, "Cast", &[x.clone()], &to(9));
    assert_eq!(y.to_bool_vec(), vec![true, true, false, true, true, true, true]);
    let y = run1(13, "Cast", &[x.clone()], &to(10));
    assert_eq!(y.dtype(), DType::F16);
    check(&y, &[7], &[-1.7001953, -0.19995117, 0.0, 0.60009766, 2.5, 300.0, -300.0], 0.0, "f32->f16");
    let y = run1(13, "Cast", &[y], &to(1));
    assert_eq!(y.dtype(), DType::F32);
    let y = run1(13, "Cast", &[Tensor::i64(&[3], vec![-5, 0, 7])], &to(1));
    check(&y, &[3], &[-5.0, 0.0, 7.0], 0.0, "i64->f32");
    let y = run1(13, "Cast", &[Tensor::bool(&[2], vec![true, false])], &to(1));
    check(&y, &[2], &[1.0, 0.0], 0.0, "bool->f32");
    let y = run1(13, "Cast", &[Tensor::i64(&[3], vec![-1, 5, 300])], &to(6));
    assert_eq!(y.dtype(), DType::I64, "int32 maps to i64 storage");
    let y = run1(13, "CastLike", &[Tensor::i64(&[2], vec![1, 2]), Tensor::f32(&[1], vec![0.0])], &[]);
    assert_eq!(y.dtype(), DType::F32);
}

// ====================================================================== shape / data movement

fn seq(shape: &[usize]) -> Tensor {
    Tensor::f32(shape, (0..numel(shape)).map(|i| i as f32).collect())
}

#[test]
fn reshape_flatten_squeeze_unsqueeze() {
    let x = seq(&[2, 3, 4]);
    let cases: &[(&[i64], &[usize])] = &[(&[0, -1], &[2, 12]), (&[-1, 4], &[6, 4]), (&[0, 0, 2, 2], &[2, 3, 2, 2]), (&[24], &[24]), (&[4, 0, -1], &[4, 3, 2])];
    for &(s, want) in cases {
        let y = run1(13, "Reshape", &[x.clone(), i64t(s)], &[]);
        check(&y, want, &x.f32s(), 0.0, &format!("Reshape {s:?}"));
    }
    // allowzero: 0 is a real zero-size dim
    let e = Tensor::f32(&[0, 3], vec![]);
    let y = run(14, "Reshape", &[Some(e), Some(i64t(&[3, 0]))], &[attr_i("allowzero", 1)], 1);
    assert_eq!(y[0].shape, vec![3, 0]);
    // opset 5 attribute form is gone, opset 1 used attr "shape"
    let y = run1(1, "Reshape", &[x.clone()], &[ints("shape", &[4, -1])]);
    assert_eq!(y.shape, vec![4, 6]);

    for (axis, want) in [(0i64, vec![1usize, 24]), (1, vec![2, 12]), (2, vec![6, 4]), (3, vec![24, 1]), (-1, vec![6, 4])] {
        let y = run1(13, "Flatten", &[x.clone()], &[attr_i("axis", axis)]);
        check(&y, &want, &x.f32s(), 0.0, "Flatten");
    }

    let x1 = seq(&[1, 3, 1, 2]);
    assert_eq!(run1(11, "Squeeze", &[x1.clone()], &[ints("axes", &[0, -2])]).shape, vec![3, 2]);
    assert_eq!(run1(13, "Squeeze", &[x1.clone(), i64t(&[2])], &[]).shape, vec![1, 3, 2]);
    assert_eq!(run1(13, "Squeeze", &[x1.clone()], &[]).shape, vec![3, 2]);
    assert_eq!(run1(11, "Unsqueeze", &[x.clone()], &[ints("axes", &[0, 4])]).shape, vec![1, 2, 3, 4, 1]);
    assert_eq!(run1(13, "Unsqueeze", &[x.clone(), i64t(&[-1, 1])], &[]).shape, vec![2, 1, 3, 4, 1]);
    assert_eq!(run1(13, "Unsqueeze", &[Tensor::scalar_f32(2.0), i64t(&[0])], &[]).shape, vec![1]);
    let y = run(13, "Identity", &[Some(x.clone())], &[], 1);
    check_t(&y[0], &x, 0.0, "Identity");
    let y = run(13, "Dropout", &[Some(x.clone())], &[], 2);
    check_t(&y[0], &x, 0.0, "Dropout");
    assert_eq!(y[1].dtype(), DType::Bool);
}

fn transpose_ref(x: &Tensor, perm: &[usize]) -> Tensor {
    let os: Vec<usize> = perm.iter().map(|&p| x.shape[p]).collect();
    let v = x.f32s();
    let mut out = Vec::new();
    for_idx(&os, |o| {
        let mut src = vec![0; perm.len()];
        for (k, &p) in perm.iter().enumerate() {
            src[p] = o[k];
        }
        out.push(v[flat(&x.shape, &src)]);
    });
    Tensor::f32(&os, out)
}

#[test]
fn transpose_concat_split() {
    let mut r = Rng::new(6);
    let x = r.t(&[2, 3, 4, 5]);
    for perm in [vec![0usize, 2, 3, 1], vec![3, 2, 1, 0], vec![1, 0, 2, 3], vec![0, 1, 3, 2], vec![2, 0, 3, 1]] {
        let pi: Vec<i64> = perm.iter().map(|&p| p as i64).collect();
        check_t(&run1(13, "Transpose", &[x.clone()], &[ints("perm", &pi)]), &transpose_ref(&x, &perm), 0.0, "Transpose");
    }
    check_t(&run1(13, "Transpose", &[x.clone()], &[]), &transpose_ref(&x, &[3, 2, 1, 0]), 0.0, "Transpose default");
    let xi = x.cast(DType::I64);
    let y = run1(13, "Transpose", &[xi], &[ints("perm", &[1, 0, 2, 3])]);
    assert_eq!(y.dtype(), DType::I64);

    // Concat
    let a = r.t(&[2, 1, 4]);
    let b = r.t(&[2, 3, 4]);
    let c = r.t(&[2, 2, 4]);
    let y = run1(13, "Concat", &[a.clone(), b.clone(), c.clone()], &[attr_i("axis", 1)]);
    let mut want = Vec::new();
    for i in 0..2 {
        for t in [&a, &b, &c] {
            let d = t.shape[1] * 4;
            want.extend_from_slice(&t.f32s()[i * d..(i + 1) * d]);
        }
    }
    check(&y, &[2, 6, 4], &want, 0.0, "Concat axis 1");
    let y = run1(13, "Concat", &[a.clone(), a.clone()], &[attr_i("axis", -1)]);
    assert_eq!(y.shape, vec![2, 1, 8]);
    let y = run1(13, "Concat", &[i64t(&[1, 2]), i64t(&[]), i64t(&[3])], &[attr_i("axis", 0)]);
    check_i(&y, &[3], &[1, 2, 3], "Concat i64 with empty");

    // Split
    let x = seq(&[2, 6, 3]);
    let narrow = |x: &Tensor, ax: usize, s: usize, l: usize| {
        let mut sh = x.shape.clone();
        sh[ax] = l;
        let mut v = Vec::new();
        let xv = x.f32s();
        for_idx(&sh, |i| {
            let mut j = i.to_vec();
            j[ax] += s;
            v.push(xv[flat(&x.shape, &j)]);
        });
        Tensor::f32(&sh, v)
    };
    let chk = |ys: &[Tensor], sizes: &[usize], ax: usize, what: &str| {
        assert_eq!(ys.len(), sizes.len(), "{what}");
        let mut s = 0;
        for (y, &l) in ys.iter().zip(sizes) {
            check_t(y, &narrow(&x, ax, s, l), 0.0, what);
            s += l;
        }
    };
    chk(&run(11, "Split", &[Some(x.clone())], &[attr_i("axis", 1), ints("split", &[1, 3, 2])], 3), &[1, 3, 2], 1, "Split-11 attr");
    chk(&run(13, "Split", &[Some(x.clone()), Some(i64t(&[4, 2]))], &[attr_i("axis", 1)], 2), &[4, 2], 1, "Split-13 input");
    chk(&run(13, "Split", &[Some(x.clone())], &[attr_i("axis", -2)], 3), &[2, 2, 2], 1, "Split-13 equal");
    chk(&run(18, "Split", &[Some(seq(&[2, 6, 3]))], &[attr_i("axis", 1), attr_i("num_outputs", 4)], 4), &[2, 2, 2, 0], 1, "Split-18 num_outputs");
    let x7 = seq(&[7, 2]);
    let ys = run(18, "Split", &[Some(x7.clone())], &[attr_i("num_outputs", 3)], 3);
    assert_eq!(ys.iter().map(|y| y.shape[0]).collect::<Vec<_>>(), vec![3, 3, 1], "Split-18 uneven");
}

fn slice_ref(x: &Tensor, starts: &[i64], ends: &[i64], axes: &[usize], steps: &[i64]) -> Tensor {
    let r = x.rank();
    let mut st = vec![0i64; r];
    let mut sp = vec![1i64; r];
    let mut cnt: Vec<usize> = x.shape.clone();
    for (i, &a) in axes.iter().enumerate() {
        let d = x.shape[a] as i64;
        let step = steps[i];
        // numpy-style slice semantics
        let idx: Vec<i64> = if step > 0 {
            let s = if starts[i] < 0 { (starts[i] + d).max(0) } else { starts[i].min(d) };
            let e = if ends[i] < 0 { (ends[i] + d).max(0) } else { ends[i].min(d) };
            (0..).map(|k| s + k * step).take_while(|&v| v < e).collect()
        } else {
            let s = if starts[i] < 0 { (starts[i] + d).max(-1) } else { starts[i].min(d - 1) };
            let e = if ends[i] < 0 { (ends[i] + d).max(-1) } else { ends[i].min(d - 1) };
            (0..).map(|k| s + k * step).take_while(|&v| v > e).collect()
        };
        cnt[a] = idx.len();
        st[a] = idx.first().copied().unwrap_or(0);
        sp[a] = step;
    }
    let v = x.f32s();
    let mut out = Vec::new();
    for_idx(&cnt, |o| {
        let src: Vec<usize> = (0..r).map(|d| (st[d] + o[d] as i64 * sp[d]) as usize).collect();
        out.push(v[flat(&x.shape, &src)]);
    });
    Tensor::f32(&cnt, out)
}

#[test]
fn slice_op() {
    let x = seq(&[4, 5, 6]);
    let cases: &[(&[i64], &[i64], Option<&[i64]>, Option<&[i64]>)] = &[
        (&[1, 0], &[3, 5], None, None),
        (&[-3], &[-1], Some(&[2]), None),
        (&[0], &[i64::MAX], Some(&[1]), Some(&[2])),
        (&[-1], &[i64::MIN], Some(&[2]), Some(&[-1])),
        (&[4, 3], &[0, -100], Some(&[-1, 0]), Some(&[-2, -1])),
        (&[1, 1, 1], &[4, 5, 6], Some(&[0, 1, 2]), Some(&[2, 3, 4])),
        (&[10], &[20], Some(&[1]), None),
        (&[5], &[-10], Some(&[2]), Some(&[-3])),
        (&[2], &[2], Some(&[0]), None),
        (&[-100], &[100], Some(&[-1]), Some(&[1])),
    ];
    for &(s, e, a, st) in cases {
        let axes: Vec<i64> = a.map(|a| a.to_vec()).unwrap_or_else(|| (0..s.len() as i64).collect());
        let steps: Vec<i64> = st.map(|s| s.to_vec()).unwrap_or_else(|| vec![1; s.len()]);
        let ax_u: Vec<usize> = axes.iter().map(|&v| if v < 0 { (v + 3) as usize } else { v as usize }).collect();
        let want = slice_ref(&x, s, e, &ax_u, &steps);
        let mut ins = vec![Some(x.clone()), Some(i64t(s)), Some(i64t(e))];
        if a.is_some() || st.is_some() {
            ins.push(Some(i64t(&axes)));
        }
        if st.is_some() {
            ins.push(Some(i64t(&steps)));
        }
        let y = run(13, "Slice", &ins, &[], 1);
        check_t(&y[0], &want, 0.0, &format!("Slice {s:?} {e:?} {a:?} {st:?}"));
    }
    // opset 1 attribute form
    let y = run1(1, "Slice", &[x.clone()], &[ints("starts", &[1, -2]), ints("ends", &[3, 1000]), ints("axes", &[0, 2])]);
    check_t(&y, &slice_ref(&x, &[1, -2], &[3, 1000], &[0, 2], &[1, 1]), 0.0, "Slice-1");
    let y = run(13, "Slice", &[Some(x.cast(DType::I64)), Some(i64t(&[1])), Some(i64t(&[3]))], &[], 1);
    assert_eq!(y[0].dtype(), DType::I64);
}

#[test]
fn gather_family() {
    let mut r = Rng::new(7);
    let x = r.t(&[4, 5, 3]);
    let xv = x.f32s();
    // Gather axis 1 with 2-D negative indices
    let idx = Tensor::i64(&[2, 2], vec![0, -1, 2, 4]);
    let y = run1(13, "Gather", &[x.clone(), idx.clone()], &[attr_i("axis", 1)]);
    let iv = [0usize, 4, 2, 4];
    let mut want = Vec::new();
    for a in 0..4 {
        for &i in &iv {
            for c in 0..3 {
                want.push(xv[(a * 5 + i) * 3 + c]);
            }
        }
    }
    check(&y, &[4, 2, 2, 3], &want, 0.0, "Gather axis 1");
    // scalar index axis 0
    let y = run1(13, "Gather", &[x.clone(), Tensor::scalar_i64(-2)], &[]);
    check(&y, &[5, 3], &xv[2 * 15..3 * 15], 0.0, "Gather scalar");
    // Gather on i64 data with i32-like indices
    let y = run1(13, "Gather", &[i64t(&[10, 20, 30]), Tensor::i64(&[2], vec![2, 0])], &[]);
    check_i(&y, &[2], &[30, 10], "Gather i64");
    // out of range is an error
    assert!(try_run_mode(13, "", "Gather", &[Some(x.clone()), Some(Tensor::scalar_i64(4))], &[], 1, 0).is_err());

    // GatherElements
    let x2 = r.t(&[3, 4]);
    let x2v = x2.f32s();
    let idx = Tensor::i64(&[3, 2], vec![0, 3, -1, 1, 2, 2]);
    let y = run1(13, "GatherElements", &[x2.clone(), idx], &[attr_i("axis", 1)]);
    let want = [x2v[0], x2v[3], x2v[4 + 3], x2v[4 + 1], x2v[8 + 2], x2v[8 + 2]];
    check(&y, &[3, 2], &want, 0.0, "GatherElements axis 1");
    let idx = Tensor::i64(&[2, 4], vec![2, 0, 1, 2, 0, 0, 0, 1]);
    let y = run1(13, "GatherElements", &[x2.clone(), idx], &[attr_i("axis", 0)]);
    let want = [x2v[8], x2v[1], x2v[4 + 2], x2v[8 + 3], x2v[0], x2v[1], x2v[2], x2v[4 + 3]];
    check(&y, &[2, 4], &want, 0.0, "GatherElements axis 0");

    // GatherND batch_dims 0
    let d = seq(&[2, 3, 4]);
    let idx = Tensor::i64(&[2, 2], vec![1, 2, 0, -1]);
    let y = run1(13, "GatherND", &[d.clone(), idx], &[]);
    let dv = d.f32s();
    let mut want = Vec::new();
    want.extend_from_slice(&dv[(3 + 2) * 4..(3 + 2) * 4 + 4]);
    want.extend_from_slice(&dv[2 * 4..2 * 4 + 4]);
    check(&y, &[2, 4], &want, 0.0, "GatherND");
    let idx = Tensor::i64(&[3, 3], vec![0, 0, 0, 1, 2, 3, 1, 1, 1]);
    let y = run1(13, "GatherND", &[d.clone(), idx], &[]);
    check(&y, &[3], &[0.0, 23.0, 17.0], 0.0, "GatherND full");
    // batch_dims 1: data [2,3,4], indices [2,2,1] -> [2,2,4]
    let idx = Tensor::i64(&[2, 2, 1], vec![2, 0, 1, 1]);
    let y = run1(13, "GatherND", &[d.clone(), idx], &[attr_i("batch_dims", 1)]);
    let mut want = Vec::new();
    for (b, j) in [(0usize, 2usize), (0, 0), (1, 1), (1, 1)] {
        want.extend_from_slice(&dv[(b * 3 + j) * 4..(b * 3 + j) * 4 + 4]);
    }
    check(&y, &[2, 2, 4], &want, 0.0, "GatherND batch_dims");
}

#[test]
fn scatter_family() {
    let d = seq(&[4, 3]);
    let idx = Tensor::i64(&[2, 1], vec![1, 3]);
    let upd = Tensor::f32(&[2, 3], vec![10.0, 11.0, 12.0, 20.0, 21.0, 22.0]);
    let y = run1(13, "ScatterND", &[d.clone(), idx.clone(), upd.clone()], &[]);
    check(&y, &[4, 3], &[0.0, 1.0, 2.0, 10.0, 11.0, 12.0, 6.0, 7.0, 8.0, 20.0, 21.0, 22.0], 0.0, "ScatterND");
    let y = run1(16, "ScatterND", &[d.clone(), idx.clone(), upd.clone()], &[attr_s("reduction", "add")]);
    check(&y, &[4, 3], &[0.0, 1.0, 2.0, 13.0, 15.0, 17.0, 6.0, 7.0, 8.0, 29.0, 31.0, 33.0], 0.0, "ScatterND add");
    let y = run1(16, "ScatterND", &[d.clone(), Tensor::i64(&[2, 1], vec![1, 1]), upd.clone()], &[attr_s("reduction", "mul")]);
    check(&y, &[4, 3], &[0.0, 1.0, 2.0, 600.0, 924.0, 1320.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0], 0.0, "ScatterND mul");
    let y = run1(13, "ScatterND", &[d.clone(), Tensor::i64(&[2, 2], vec![0, 1, -1, 2]), Tensor::f32(&[2], vec![-1.0, -2.0])], &[]);
    check(&y, &[4, 3], &[0.0, -1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, -2.0], 0.0, "ScatterND elements");
    let y = run1(13, "ScatterND", &[d.cast(DType::I64), idx.clone(), upd.cast(DType::I64)], &[]);
    assert_eq!(y.dtype(), DType::I64);
    // f16 data with f32 updates (f16 models compute in f32)
    let y = run1(13, "ScatterND", &[d.cast(DType::F16), idx.clone(), upd.clone()], &[]);
    assert_eq!(y.dtype(), DType::F16);
    check(&y, &[4, 3], &[0.0, 1.0, 2.0, 10.0, 11.0, 12.0, 6.0, 7.0, 8.0, 20.0, 21.0, 22.0], 0.0, "ScatterND f16");

    let d = Tensor::f32(&[3, 3], vec![0.0; 9]);
    let idx = Tensor::i64(&[2, 3], vec![1, 0, 2, 0, 2, 1]);
    let upd = Tensor::f32(&[2, 3], vec![1.0, 1.1, 1.2, 2.0, 2.1, 2.2]);
    let y = run1(13, "ScatterElements", &[d.clone(), idx.clone(), upd.clone()], &[attr_i("axis", 0)]);
    check(&y, &[3, 3], &[2.0, 1.1, 0.0, 1.0, 0.0, 2.2, 0.0, 2.1, 1.2], 0.0, "ScatterElements axis 0");
    let y = run1(13, "ScatterElements", &[d.cast(DType::F16), idx.clone(), upd.clone()], &[attr_i("axis", 0)]);
    assert_eq!(y.dtype(), DType::F16);
    let d = Tensor::f32(&[1, 5], vec![1.0, 2.0, 3.0, 4.0, 5.0]);
    let y = run1(13, "ScatterElements", &[d.clone(), Tensor::i64(&[1, 2], vec![1, -2]), Tensor::f32(&[1, 2], vec![1.1, 2.1])], &[attr_i("axis", 1)]);
    check(&y, &[1, 5], &[1.0, 1.1, 3.0, 2.1, 5.0], 0.0, "ScatterElements axis 1");
    let y = run1(16, "ScatterElements", &[d.clone(), Tensor::i64(&[1, 2], vec![1, 1]), Tensor::f32(&[1, 2], vec![1.1, 2.1])], &[attr_i("axis", 1), attr_s("reduction", "add")]);
    check(&y, &[1, 5], &[1.0, 5.2, 3.0, 4.0, 5.0], 1e-6, "ScatterElements add");
    let y = run1(18, "ScatterElements", &[d, Tensor::i64(&[1, 2], vec![1, 1]), Tensor::f32(&[1, 2], vec![1.1, 2.1])], &[attr_i("axis", 1), attr_s("reduction", "max")]);
    check(&y, &[1, 5], &[1.0, 2.1, 3.0, 4.0, 5.0], 1e-6, "ScatterElements max");
}

#[test]
fn expand_tile_pad_range() {
    let mut r = Rng::new(8);
    let x = r.t(&[3, 1]);
    for target in [vec![2i64, 3, 4], vec![1, 1], vec![3, 5], vec![2, 1, 1]] {
        let ts: Vec<usize> = target.iter().map(|&v| v as usize).collect();
        let out = bshape(&x.shape, &ts);
        let xv = x.f32s();
        let mut want = Vec::new();
        for_idx(&out, |i| want.push(xv[boff(&x.shape, i)]));
        check(&run1(13, "Expand", &[x.clone(), i64t(&target)], &[]), &out, &want, 0.0, &format!("Expand {target:?}"));
    }
    // Tile
    let x = seq(&[2, 3]);
    let y = run1(13, "Tile", &[x.clone(), i64t(&[2, 2])], &[]);
    let mut want = Vec::new();
    for_idx(&[4, 6], |i| want.push((i[0] % 2 * 3 + i[1] % 3) as f32));
    check(&y, &[4, 6], &want, 0.0, "Tile");
    assert_eq!(run1(13, "Tile", &[x.clone(), i64t(&[1, 0])], &[]).shape, vec![2, 0]);

    // Pad
    let x = seq(&[3, 4]);
    let xv = x.f32s();
    let pads = [1i64, 2, 2, 1]; // begin h, begin w, end h, end w
    let os = [3 + 3, 4 + 3];
    let modes: &[(&str, fn(i64, i64) -> Option<i64>)] = &[
        ("constant", |i, n| if i >= 0 && i < n { Some(i) } else { None }),
        ("edge", |i, n| Some(i.clamp(0, n - 1))),
        ("wrap", |i, n| Some(i.rem_euclid(n))),
        ("reflect", |i, n| {
            let mut i = i;
            while i < 0 || i >= n {
                if i < 0 {
                    i = -i;
                }
                if i >= n {
                    i = 2 * (n - 1) - i;
                }
            }
            Some(i)
        }),
    ];
    for &(mode, map) in modes {
        let mut want = Vec::new();
        for_idx(&os, |o| {
            let (a, b) = (map(o[0] as i64 - pads[0], 3), map(o[1] as i64 - pads[1], 4));
            want.push(match (a, b) {
                (Some(a), Some(b)) => xv[(a * 4 + b) as usize],
                _ => 9.5,
            });
        });
        let y = run(13, "Pad", &[Some(x.clone()), Some(i64t(&pads)), Some(Tensor::scalar_f32(9.5))], &[attr_s("mode", mode)], 1);
        check(&y[0], &os, &want, 0.0, &format!("Pad {mode}"));
        if mode == "wrap" {
            continue; // wrap is opset 19+, no attribute form
        }
        let y = run1(2, "Pad", &[x.clone()], &[attr_s("mode", mode), ints("pads", &pads), attr_f("value", 9.5)]);
        check(&y, &os, &want, 0.0, &format!("Pad-2 {mode}"));
    }
    // negative pads crop; default constant 0
    let y = run(13, "Pad", &[Some(x.clone()), Some(i64t(&[0, -1, 1, -2]))], &[], 1);
    check(&y[0], &[4, 1], &[1.0, 5.0, 9.0, 0.0], 0.0, "Pad negative");
    // opset 18 axes input
    let y = run(18, "Pad", &[Some(x.clone()), Some(i64t(&[1, 1])), None, Some(i64t(&[-1]))], &[], 1);
    check(&y[0], &[3, 6], &[0.0, 0.0, 1.0, 2.0, 3.0, 0.0, 0.0, 4.0, 5.0, 6.0, 7.0, 0.0, 0.0, 8.0, 9.0, 10.0, 11.0, 0.0], 0.0, "Pad axes");
    let y = run(13, "Pad", &[Some(x.cast(DType::I64)), Some(i64t(&[0, 1, 0, 0])), Some(Tensor::scalar_i64(-1))], &[], 1);
    assert_eq!(y[0].dtype(), DType::I64);
    check_i(&y[0], &[3, 5], &[-1, 0, 1, 2, 3, -1, 4, 5, 6, 7, -1, 8, 9, 10, 11], "Pad i64");

    // Range
    let y = run1(13, "Range", &[Tensor::scalar_f32(1.0), Tensor::scalar_f32(2.0), Tensor::scalar_f32(0.25)], &[]);
    check(&y, &[4], &[1.0, 1.25, 1.5, 1.75], 0.0, "Range f32");
    let y = run1(13, "Range", &[Tensor::scalar_i64(10), Tensor::scalar_i64(3), Tensor::scalar_i64(-3)], &[]);
    assert_eq!(y.dtype(), DType::I64);
    check_i(&y, &[3], &[10, 7, 4], "Range i64 negative");
    let y = run1(13, "Range", &[Tensor::scalar_i64(0), Tensor::scalar_i64(0), Tensor::scalar_i64(1)], &[]);
    assert_eq!(y.shape, vec![0]);
    let y = run1(13, "Range", &[Tensor::scalar_f32(0.0), Tensor::scalar_f32(-1.0), Tensor::scalar_f32(-0.3)], &[]);
    check(&y, &[4], &[0.0, -0.3, -0.6, -0.90000004], 1e-6, "Range f32 negative");
}

#[test]
fn pixel_shuffles_trilu_onehot() {
    let mut r = Rng::new(9);
    let (n, c, h, w, b) = (2usize, 8usize, 3usize, 2usize, 2usize);
    let x = r.t(&[n, c, h, w]);
    let xv = x.f32s();
    let cp = c / (b * b);
    let os = [n, cp, h * b, w * b];
    for mode in ["DCR", "CRD"] {
        let mut want = Vec::new();
        for_idx(&os, |o| {
            let (nn, cc, y, xx) = (o[0], o[1], o[2], o[3]);
            let (i, j) = (y % b, xx % b);
            let src_c = if mode == "DCR" { (i * b + j) * cp + cc } else { cc * b * b + i * b + j };
            want.push(xv[((nn * c + src_c) * h + y / b) * w + xx / b]);
        });
        let y = run1(13, "DepthToSpace", &[x.clone()], &[attr_i("blocksize", b as i64), attr_s("mode", mode)]);
        check(&y, &os, &want, 0.0, mode);
    }
    // SpaceToDepth is the inverse of DCR DepthToSpace
    let x = r.t(&[2, 3, 4, 6]);
    let xv = x.f32s();
    let os = [2, 12, 2, 3];
    let mut want = Vec::new();
    for_idx(&os, |o| {
        let (nn, oc, y, xx) = (o[0], o[1], o[2], o[3]);
        let (blk, cc) = (oc / 3, oc % 3);
        let (i, j) = (blk / 2, blk % 2);
        want.push(xv[((nn * 3 + cc) * 4 + y * 2 + i) * 6 + xx * 2 + j]);
    });
    check(&run1(13, "SpaceToDepth", &[x.clone()], &[attr_i("blocksize", 2)]), &os, &want, 0.0, "SpaceToDepth");

    // Trilu
    let x = r.t(&[2, 3, 5]);
    let xv = x.f32s();
    for upper in [0i64, 1] {
        for k in [-1i64, 0, 2, -5] {
            let mut want = Vec::new();
            for_idx(&x.shape, |i| {
                let d = i[2] as i64 - i[1] as i64;
                let keep = if upper == 1 { d >= k } else { d <= k };
                want.push(if keep { xv[flat(&x.shape, i)] } else { 0.0 });
            });
            let y = run(14, "Trilu", &[Some(x.clone()), Some(Tensor::scalar_i64(k))], &[attr_i("upper", upper)], 1);
            check(&y[0], &x.shape, &want, 0.0, &format!("Trilu upper={upper} k={k}"));
        }
    }
    let y = run(14, "Trilu", &[Some(x.clone())], &[attr_i("upper", 0)], 1);
    let mut want = Vec::new();
    for_idx(&x.shape, |i| want.push(if i[2] <= i[1] { xv[flat(&x.shape, i)] } else { 0.0 }));
    check(&y[0], &x.shape, &want, 0.0, "Trilu lower default k");

    // OneHot
    let idx = Tensor::i64(&[2, 2], vec![0, 3, -1, 5]);
    let vals = Tensor::f32(&[2], vec![-1.0, 2.0]);
    let y = run1(11, "OneHot", &[idx.clone(), Tensor::scalar_i64(4), vals.clone()], &[]);
    let iv = [0i64, 3, 3, 99];
    let mut want = Vec::new();
    for &i in &iv {
        for k in 0..4 {
            want.push(if i == k { 2.0 } else { -1.0 });
        }
    }
    check(&y, &[2, 2, 4], &want, 0.0, "OneHot axis -1");
    let y = run1(11, "OneHot", &[idx.clone(), Tensor::scalar_f32(4.0), vals.clone()], &[attr_i("axis", 0)]);
    let mut want = Vec::new();
    for k in 0..4 {
        for &i in &iv {
            want.push(if i == k { 2.0 } else { -1.0 });
        }
    }
    check(&y, &[4, 2, 2], &want, 0.0, "OneHot axis 0");
    let y = run1(11, "OneHot", &[Tensor::i64(&[3], vec![1, 0, 2]), Tensor::scalar_i64(3), Tensor::i64(&[2], vec![0, 1])], &[attr_i("axis", 1)]);
    assert_eq!(y.dtype(), DType::I64);
    check_i(&y, &[3, 3], &[0, 1, 0, 1, 0, 0, 0, 0, 1], "OneHot i64");
}

#[test]
fn nonzero_cumsum_topk() {
    let x = Tensor::f32(&[2, 3], vec![0.0, 1.0, 0.0, 2.0, 0.0, 3.0]);
    let y = run1(13, "NonZero", &[x], &[]);
    check_i(&y, &[2, 3], &[0, 1, 1, 1, 0, 2], "NonZero");
    let y = run1(13, "NonZero", &[Tensor::bool(&[4], vec![false, true, true, false])], &[]);
    check_i(&y, &[1, 2], &[1, 2], "NonZero bool");

    let x = seq(&[2, 3]).cast(DType::F32);
    let x = Tensor::f32(&[2, 3], x.f32s().iter().map(|v| v + 1.0).collect()); // 1..6
    let cases: &[(i64, i64, i64, &[f32])] = &[
        (0, 0, 1, &[1.0, 3.0, 6.0, 4.0, 9.0, 15.0]),
        (1, 0, 1, &[0.0, 1.0, 3.0, 0.0, 4.0, 9.0]),
        (0, 1, 1, &[6.0, 5.0, 3.0, 15.0, 11.0, 6.0]),
        (1, 1, 1, &[5.0, 3.0, 0.0, 11.0, 6.0, 0.0]),
        (0, 0, 0, &[1.0, 2.0, 3.0, 5.0, 7.0, 9.0]),
        (0, 1, -2, &[5.0, 7.0, 9.0, 4.0, 5.0, 6.0]),
    ];
    for &(ex, rev, axis, want) in cases {
        let y = run1(14, "CumSum", &[x.clone(), Tensor::scalar_i64(axis)], &[attr_i("exclusive", ex), attr_i("reverse", rev)]);
        check(&y, &[2, 3], want, 0.0, &format!("CumSum ex={ex} rev={rev} axis={axis}"));
    }
    let y = run1(14, "CumSum", &[Tensor::i64(&[4], vec![1, 2, 3, 4]), Tensor::i64(&[1], vec![0])], &[]);
    check_i(&y, &[4], &[1, 3, 6, 10], "CumSum i64");

    // TopK
    let x = Tensor::f32(&[2, 5], vec![3.0, 1.0, 4.0, 1.0, 5.0, 9.0, 2.0, 6.0, 5.0, 3.0]);
    let ys = run(11, "TopK", &[Some(x.clone()), Some(i64t(&[3]))], &[], 2);
    check(&ys[0], &[2, 3], &[5.0, 4.0, 3.0, 9.0, 6.0, 5.0], 0.0, "TopK values");
    check_i(&ys[1], &[2, 3], &[4, 2, 0, 0, 2, 3], "TopK indices");
    let ys = run(11, "TopK", &[Some(x.clone()), Some(i64t(&[2]))], &[attr_i("largest", 0)], 2);
    check(&ys[0], &[2, 2], &[1.0, 1.0, 2.0, 3.0], 0.0, "TopK smallest");
    check_i(&ys[1], &[2, 2], &[1, 3, 1, 4], "TopK smallest indices (ties: lower index first)");
    let ys = run(11, "TopK", &[Some(x.clone()), Some(i64t(&[1]))], &[attr_i("axis", 0)], 2);
    check(&ys[0], &[1, 5], &[9.0, 2.0, 6.0, 5.0, 5.0], 0.0, "TopK axis 0");
    check_i(&ys[1], &[1, 5], &[1, 1, 1, 1, 0], "TopK axis 0 indices");
    let ys = run(1, "TopK", &[Some(x)], &[attr_i("k", 2)], 2);
    check(&ys[0], &[2, 2], &[5.0, 4.0, 9.0, 6.0], 0.0, "TopK-1 attr");
}

// ====================================================================== conv / pool

struct ConvP {
    strides: Vec<usize>,
    /// begin pads then end pads
    pads: Vec<usize>,
    dil: Vec<usize>,
    group: usize,
}

fn conv_ref(x: &Tensor, w: &Tensor, b: Option<&Tensor>, p: &ConvP) -> Tensor {
    let sr = x.rank() - 2;
    let (n, oc, icg) = (x.shape[0], w.shape[0], w.shape[1]);
    let ocg = oc / p.group;
    let ks = &w.shape[2..];
    let isp = &x.shape[2..];
    let osp: Vec<usize> = (0..sr).map(|i| (isp[i] + p.pads[i] + p.pads[sr + i] - ((ks[i] - 1) * p.dil[i] + 1)) / p.strides[i] + 1).collect();
    let mut os = vec![n, oc];
    os.extend(&osp);
    let (xv, wv) = (x.f32s(), w.f32s());
    let bv = b.map(|b| b.to_f32_vec());
    let mut out = Vec::new();
    for_idx(&os, |o| {
        let (bn, co) = (o[0], o[1]);
        let g = co / ocg;
        let mut s = bv.as_ref().map(|b| b[co] as f64).unwrap_or(0.0);
        for ci in 0..icg {
            for_idx(ks, |k| {
                let mut ip = vec![bn, g * icg + ci];
                for i in 0..sr {
                    let v = (o[2 + i] * p.strides[i] + k[i] * p.dil[i]) as i64 - p.pads[i] as i64;
                    if v < 0 || v >= isp[i] as i64 {
                        return;
                    }
                    ip.push(v as usize);
                }
                let mut wi = vec![co, ci];
                wi.extend_from_slice(k);
                s += xv[flat(&x.shape, &ip)] as f64 * wv[flat(&w.shape, &wi)] as f64;
            });
        }
        out.push(s as f32);
    });
    Tensor::f32(&os, out)
}

fn conv_attrs(p: &ConvP) -> Vec<Attribute> {
    let c = |v: &[usize]| v.iter().map(|&x| x as i64).collect::<Vec<_>>();
    vec![ints("strides", &c(&p.strides)), ints("pads", &c(&p.pads)), ints("dilations", &c(&p.dil)), attr_i("group", p.group as i64)]
}

#[test]
fn conv_variants() {
    let mut r = Rng::new(10);
    // (x shape, w shape, strides, pads, dil, group)
    let cases: Vec<(Vec<usize>, Vec<usize>, Vec<usize>, Vec<usize>, Vec<usize>, usize)> = vec![
        // 1-D
        (vec![2, 3, 17], vec![4, 3, 3], vec![1], vec![1, 1], vec![1], 1),
        (vec![1, 4, 20], vec![6, 2, 5], vec![2], vec![2, 1], vec![2], 2),
        (vec![1, 4, 9], vec![4, 1, 3], vec![1], vec![1, 1], vec![1], 4),
        // 2-D
        (vec![1, 3, 8, 9], vec![5, 3, 3, 3], vec![1, 1], vec![1, 1, 1, 1], vec![1, 1], 1),
        (vec![2, 3, 11, 10], vec![4, 3, 3, 2], vec![2, 3], vec![1, 0, 2, 1], vec![1, 1], 1),
        (vec![1, 2, 12, 12], vec![3, 2, 3, 3], vec![1, 1], vec![2, 2, 2, 2], vec![2, 2], 1),
        (vec![1, 6, 7, 7], vec![9, 2, 3, 3], vec![1, 2], vec![1, 1, 1, 1], vec![1, 1], 3),
        (vec![2, 8, 9, 7], vec![8, 1, 3, 3], vec![1, 1], vec![1, 1, 1, 1], vec![1, 1], 8),   // depthwise
        (vec![1, 8, 10, 9], vec![8, 1, 5, 5], vec![2, 2], vec![2, 2, 2, 2], vec![1, 1], 8),  // depthwise strided
        (vec![1, 4, 9, 9], vec![4, 1, 3, 3], vec![1, 1], vec![2, 2, 2, 2], vec![2, 2], 4),   // depthwise dilated
        (vec![1, 4, 6, 6], vec![8, 1, 3, 3], vec![1, 1], vec![1, 1, 1, 1], vec![1, 1], 4),   // depthwise multiplier 2
        (vec![2, 16, 5, 7], vec![12, 16, 1, 1], vec![1, 1], vec![0, 0, 0, 0], vec![1, 1], 1), // pointwise
        (vec![1, 8, 6, 6], vec![4, 4, 1, 1], vec![1, 1], vec![0, 0, 0, 0], vec![1, 1], 2),   // grouped pointwise
        (vec![1, 3, 7, 7], vec![2, 3, 1, 1], vec![2, 2], vec![0, 0, 0, 0], vec![1, 1], 1),   // strided 1x1
        (vec![1, 3, 40, 37], vec![16, 3, 3, 3], vec![1, 1], vec![1, 1, 1, 1], vec![1, 1], 1), // many columns
        (vec![1, 64, 14, 14], vec![32, 64, 3, 3], vec![1, 1], vec![1, 1, 1, 1], vec![1, 1], 1), // big K
        // 3-D
        (vec![1, 2, 5, 6, 7], vec![3, 2, 3, 3, 3], vec![1, 1, 1], vec![1, 1, 1, 1, 1, 1], vec![1, 1, 1], 1),
        (vec![2, 4, 6, 5, 6], vec![4, 2, 2, 3, 2], vec![2, 1, 2], vec![0, 1, 1, 1, 0, 0], vec![1, 2, 1], 2),
    ];
    for (xs, ws, strides, pads, dil, group) in cases {
        let x = r.t(&xs);
        let w = r.t(&ws);
        let b = r.t(&[ws[0]]);
        let p = ConvP { strides, pads, dil, group };
        let want = conv_ref(&x, &w, Some(&b), &p);
        let y = run1(13, "Conv", &[x.clone(), w.clone(), b.clone()], &conv_attrs(&p));
        check_t(&y, &want, 1e-5, &format!("Conv x{xs:?} w{ws:?}"));
        let want = conv_ref(&x, &w, None, &p);
        let y = run1(13, "Conv", &[x, w], &conv_attrs(&p));
        check_t(&y, &want, 1e-5, &format!("Conv no bias x{xs:?} w{ws:?}"));
    }
    // auto_pad SAME_UPPER / SAME_LOWER / VALID
    for (xs, ws, s) in [(vec![1, 2, 7, 8], vec![3, 2, 3, 4], 2usize), (vec![1, 2, 6, 6], vec![2, 2, 2, 2], 1), (vec![1, 1, 5, 9], vec![1, 1, 4, 3], 3)] {
        let x = r.t(&xs);
        let w = r.t(&ws);
        for mode in ["SAME_UPPER", "SAME_LOWER", "VALID"] {
            let mut pb = vec![0; 2];
            let mut pe = vec![0; 2];
            if mode != "VALID" {
                for i in 0..2 {
                    let o = xs[2 + i].div_ceil(s);
                    let total = ((o - 1) * s + ws[2 + i]).saturating_sub(xs[2 + i]);
                    pb[i] = if mode == "SAME_UPPER" { total / 2 } else { total - total / 2 };
                    pe[i] = total - pb[i];
                }
            }
            let p = ConvP { strides: vec![s, s], pads: [pb, pe].concat(), dil: vec![1, 1], group: 1 };
            let want = conv_ref(&x, &w, None, &p);
            let y = run1(13, "Conv", &[x.clone(), w.clone()], &[attr_s("auto_pad", mode), ints("strides", &[s as i64, s as i64])]);
            check_t(&y, &want, 1e-5, &format!("Conv auto_pad {mode} {xs:?}"));
        }
    }
    // f16 weights
    let x = r.t(&[1, 3, 6, 6]);
    let w = r.t(&[4, 3, 3, 3]);
    let w16 = Tensor::f16_from_f32(&w.shape, &w.f32s());
    let p = ConvP { strides: vec![1, 1], pads: vec![1, 1, 1, 1], dil: vec![1, 1], group: 1 };
    let want = conv_ref(&x, &w16.cast(DType::F32), None, &p);
    check_t(&run1(13, "Conv", &[x, w16], &conv_attrs(&p)), &want, 1e-5, "Conv f16 weights");
}

/// ConvTranspose reference; `out_sp` is the output spatial size, `pb` the begin pads.
fn conv_t_ref(x: &Tensor, w: &Tensor, b: Option<&Tensor>, strides: &[usize], pb: &[usize], dil: &[usize], group: usize, out_sp: &[usize]) -> Tensor {
    let sr = x.rank() - 2;
    let (n, ci_all) = (x.shape[0], x.shape[1]);
    let ocg = w.shape[1];
    let icg = ci_all / group;
    let oc = ocg * group;
    let mut os = vec![n, oc];
    os.extend(out_sp);
    let mut out = vec![0.0f64; numel(&os)];
    let (xv, wv) = (x.f32s(), w.f32s());
    let ks = &w.shape[2..];
    for_idx(&x.shape, |xi| {
        let ci = xi[1];
        let g = ci / icg;
        let xval = xv[flat(&x.shape, xi)] as f64;
        for o in 0..ocg {
            for_idx(ks, |k| {
                let mut oi = vec![xi[0], g * ocg + o];
                for i in 0..sr {
                    let v = (xi[2 + i] * strides[i] + k[i] * dil[i]) as i64 - pb[i] as i64;
                    if v < 0 || v >= out_sp[i] as i64 {
                        return;
                    }
                    oi.push(v as usize);
                }
                let mut wi = vec![ci, o];
                wi.extend_from_slice(k);
                out[flat(&os, &oi)] += xval * wv[flat(&w.shape, &wi)] as f64;
            });
        }
    });
    if let Some(b) = b {
        let bv = b.f32s();
        let plane = numel(out_sp);
        for (i, v) in out.iter_mut().enumerate() {
            *v += bv[(i / plane) % oc] as f64;
        }
    }
    Tensor::f32(&os, out.into_iter().map(|v| v as f32).collect())
}

#[test]
fn conv_transpose_variants() {
    let mut r = Rng::new(11);
    // (x, w, strides, pads(b..e), dil, group, output_padding)
    let cases: Vec<(Vec<usize>, Vec<usize>, Vec<usize>, Vec<usize>, Vec<usize>, usize, Vec<usize>)> = vec![
        (vec![1, 3, 5], vec![3, 2, 3], vec![2], vec![1, 1], vec![1], 1, vec![1]),
        (vec![2, 2, 4, 5], vec![2, 3, 3, 3], vec![1, 1], vec![0, 0, 0, 0], vec![1, 1], 1, vec![0, 0]),
        (vec![1, 4, 5, 4], vec![4, 2, 4, 4], vec![2, 2], vec![1, 1, 1, 1], vec![1, 1], 1, vec![0, 0]),
        (vec![1, 4, 3, 4], vec![4, 1, 3, 3], vec![2, 3], vec![1, 0, 1, 2], vec![1, 1], 2, vec![1, 2]),
        (vec![1, 6, 4, 4], vec![6, 1, 3, 3], vec![2, 2], vec![1, 1, 1, 1], vec![2, 1], 6, vec![1, 0]),
        (vec![1, 2, 3, 3, 3], vec![2, 2, 2, 2, 2], vec![2, 2, 2], vec![0, 0, 0, 0, 0, 0], vec![1, 1, 1], 1, vec![0, 0, 0]),
    ];
    for (xs, ws, s, pads, dil, group, opad) in cases {
        let sr = xs.len() - 2;
        let x = r.t(&xs);
        let w = r.t(&ws);
        let b = r.t(&[ws[1] * group]);
        let out_sp: Vec<usize> = (0..sr).map(|i| s[i] * (xs[2 + i] - 1) + opad[i] + (ws[2 + i] - 1) * dil[i] + 1 - pads[i] - pads[sr + i]).collect();
        let want = conv_t_ref(&x, &w, Some(&b), &s, &pads[..sr], &dil, group, &out_sp);
        let c = |v: &[usize]| v.iter().map(|&x| x as i64).collect::<Vec<_>>();
        let attrs = vec![ints("strides", &c(&s)), ints("pads", &c(&pads)), ints("dilations", &c(&dil)), attr_i("group", group as i64), ints("output_padding", &c(&opad))];
        let y = run1(13, "ConvTranspose", &[x.clone(), w.clone(), b.clone()], &attrs);
        check_t(&y, &want, 1e-5, &format!("ConvTranspose x{xs:?} w{ws:?}"));
    }
    // output_shape: total padding split (begin = total - total/2 for NOTSET, total/2 for SAME_UPPER)
    let x = r.t(&[1, 2, 4, 4]);
    let w = r.t(&[2, 3, 3, 3]);
    for (os, auto) in [(vec![9usize, 8usize], "NOTSET"), (vec![8, 7], "SAME_UPPER"), (vec![9, 9], "NOTSET")] {
        let full = |_: usize| 2 * (4 - 1) + 3;
        let pb: Vec<usize> = (0..2)
            .map(|i| {
                let total = full(i) - os[i];
                if auto == "SAME_UPPER" { total / 2 } else { total - total / 2 }
            })
            .collect();
        let want = conv_t_ref(&x, &w, None, &[2, 2], &pb, &[1, 1], 1, &os);
        let mut attrs = vec![ints("strides", &[2, 2]), ints("output_shape", &[os[0] as i64, os[1] as i64])];
        if auto != "NOTSET" {
            attrs.push(attr_s("auto_pad", auto));
        }
        check_t(&run1(13, "ConvTranspose", &[x.clone(), w.clone()], &attrs), &want, 1e-5, &format!("ConvTranspose output_shape {os:?} {auto}"));
    }
    // auto_pad SAME_UPPER without output_shape: out = in * stride
    let want = conv_t_ref(&x, &w, None, &[2, 2], &[0, 0], &[1, 1], 1, &[8, 8]);
    check_t(&run1(13, "ConvTranspose", &[x.clone(), w.clone()], &[ints("strides", &[2, 2]), attr_s("auto_pad", "SAME_UPPER")]), &want, 1e-5, "ConvTranspose SAME_UPPER");
}

fn pool_ref(x: &Tensor, kind: &str, k: &[usize], s: &[usize], pads: &[usize], dil: &[usize], ceil: bool, include_pad: bool) -> (Tensor, Vec<i64>) {
    let sr = k.len();
    let isp = &x.shape[2..];
    let osp: Vec<usize> = (0..sr)
        .map(|i| {
            let eff = (k[i] - 1) * dil[i] + 1;
            let span = isp[i] + pads[i] + pads[sr + i] - eff;
            let mut o = if ceil { span.div_ceil(s[i]) + 1 } else { span / s[i] + 1 };
            if ceil && (o - 1) * s[i] >= isp[i] + pads[i] {
                o -= 1;
            }
            o
        })
        .collect();
    let mut os = x.shape[..2].to_vec();
    os.extend(&osp);
    let xv = x.f32s();
    let mut out = Vec::new();
    let mut idx = Vec::new();
    for_idx(&os, |o| {
        let mut best = f32::NEG_INFINITY;
        let mut bi = 0usize;
        let mut sum = 0.0f64;
        let mut cnt = 0usize;
        for_idx(k, |kk| {
            let mut ip = vec![o[0], o[1]];
            for i in 0..sr {
                let v = (o[2 + i] * s[i] + kk[i] * dil[i]) as i64 - pads[i] as i64;
                if v < 0 || v >= isp[i] as i64 {
                    return;
                }
                ip.push(v as usize);
            }
            let f = flat(&x.shape, &ip);
            let v = xv[f];
            if v > best {
                best = v;
                bi = f;
            }
            sum += v as f64;
            cnt += 1;
        });
        if kind == "MaxPool" {
            out.push(best);
            idx.push(bi as i64);
        } else {
            let c = if include_pad {
                (0..sr)
                    .map(|i| {
                        let start = (o[2 + i] * s[i]) as i64 - pads[i] as i64;
                        let end = (start + k[i] as i64).min((isp[i] + pads[sr + i]) as i64);
                        (end - start) as usize
                    })
                    .product()
            } else {
                cnt
            };
            out.push((sum / c as f64) as f32);
        }
    });
    (Tensor::f32(&os, out), idx)
}

#[test]
fn pooling() {
    let mut r = Rng::new(12);
    // (x, k, s, pads, dil, ceil)
    let cases: Vec<(Vec<usize>, Vec<usize>, Vec<usize>, Vec<usize>, Vec<usize>, bool)> = vec![
        (vec![1, 2, 7, 7], vec![2, 2], vec![2, 2], vec![0, 0, 0, 0], vec![1, 1], false),
        (vec![1, 2, 7, 7], vec![2, 2], vec![2, 2], vec![0, 0, 0, 0], vec![1, 1], true),
        (vec![2, 3, 9, 8], vec![3, 3], vec![2, 2], vec![1, 1, 1, 1], vec![1, 1], false),
        (vec![2, 3, 9, 8], vec![3, 3], vec![2, 2], vec![1, 1, 1, 1], vec![1, 1], true),
        (vec![1, 1, 6, 6], vec![3, 2], vec![2, 1], vec![1, 0, 0, 1], vec![1, 1], true),
        (vec![1, 2, 10], vec![3], vec![3], vec![1, 1], vec![1], true),
        (vec![1, 2, 4, 5, 6], vec![2, 2, 3], vec![2, 1, 2], vec![0, 1, 1, 1, 0, 1], vec![1, 1, 1], false),
    ];
    for (xs, k, s, pads, dil, ceil) in &cases {
        let x = r.t(xs);
        let c = |v: &[usize]| v.iter().map(|&x| x as i64).collect::<Vec<_>>();
        let base = vec![ints("kernel_shape", &c(k)), ints("strides", &c(s)), ints("pads", &c(pads)), attr_i("ceil_mode", *ceil as i64)];
        let (want, widx) = pool_ref(&x, "MaxPool", k, s, pads, dil, *ceil, false);
        let ys = run(12, "MaxPool", &[Some(x.clone())], &base, 2);
        check_t(&ys[0], &want, 0.0, &format!("MaxPool {xs:?} ceil={ceil}"));
        check_i(&ys[1], &want.shape, &widx, "MaxPool indices");
        for inc in [false, true] {
            let mut a = base.clone();
            a.push(attr_i("count_include_pad", inc as i64));
            let (want, _) = pool_ref(&x, "AveragePool", k, s, pads, dil, *ceil, inc);
            let y = run1(19, "AveragePool", &[x.clone()], &a);
            check_t(&y, &want, 1e-6, &format!("AveragePool {xs:?} ceil={ceil} include_pad={inc}"));
        }
    }
    // dilated max pool
    let x = r.t(&[1, 2, 9, 9]);
    let (want, _) = pool_ref(&x, "MaxPool", &[3, 2], &[1, 2], &[1, 1, 1, 1], &[2, 3], false, false);
    let y = run1(12, "MaxPool", &[x.clone()], &[ints("kernel_shape", &[3, 2]), ints("strides", &[1, 2]), ints("pads", &[1, 1, 1, 1]), ints("dilations", &[2, 3])]);
    check_t(&y, &want, 0.0, "MaxPool dilated");
    let (want, _) = pool_ref(&x, "MaxPool", &[3, 3], &[2, 2], &[0, 0, 0, 0], &[2, 2], true, false);
    let y = run1(12, "MaxPool", &[x.clone()], &[ints("kernel_shape", &[3, 3]), ints("strides", &[2, 2]), ints("dilations", &[2, 2]), attr_i("ceil_mode", 1)]);
    check_t(&y, &want, 0.0, "MaxPool dilated ceil");
    // SAME_UPPER pooling (pads computed like conv)
    let x = r.t(&[1, 1, 5, 5]);
    let (want, _) = pool_ref(&x, "MaxPool", &[2, 2], &[2, 2], &[0, 0, 1, 1], &[1, 1], false, false);
    let y = run1(12, "MaxPool", &[x.clone()], &[ints("kernel_shape", &[2, 2]), ints("strides", &[2, 2]), attr_s("auto_pad", "SAME_UPPER")]);
    check_t(&y, &want, 0.0, "MaxPool SAME_UPPER");
    // global pools
    let x = r.t(&[2, 3, 4, 5]);
    let xv = x.f32s();
    let avg: Vec<f32> = xv.chunks(20).map(|c| c.iter().sum::<f32>() / 20.0).collect();
    let mx: Vec<f32> = xv.chunks(20).map(|c| c.iter().copied().fold(f32::NEG_INFINITY, f32::max)).collect();
    check(&run1(13, "GlobalAveragePool", &[x.clone()], &[]), &[2, 3, 1, 1], &avg, 1e-6, "GlobalAveragePool");
    check(&run1(13, "GlobalMaxPool", &[x.clone()], &[]), &[2, 3, 1, 1], &mx, 0.0, "GlobalMaxPool");
    let x3 = r.t(&[2, 3, 7]);
    let avg: Vec<f32> = x3.f32s().chunks(7).map(|c| c.iter().sum::<f32>() / 7.0).collect();
    check(&run1(13, "GlobalAveragePool", &[x3], &[]), &[2, 3, 1], &avg, 1e-6, "GlobalAveragePool 1-D");
}

// ====================================================================== normalisation / softmax

#[test]
fn normalisations() {
    let mut r = Rng::new(13);
    let x = r.t(&[2, 3, 4, 5]);
    let xv = x.f32s();
    let (sc, bi, mean, var) = (r.t(&[3]), r.t(&[3]), r.t(&[3]), r.t_range(&[3], 0.1, 2.0));
    let (s, b, m, v) = (sc.f32s(), bi.f32s(), mean.f32s(), var.f32s());
    let mut want = Vec::new();
    for_idx(&x.shape, |i| {
        let c = i[1];
        want.push((xv[flat(&x.shape, i)] - m[c]) / (v[c] + 1e-3).sqrt() * s[c] + b[c]);
    });
    let y = run1(15, "BatchNormalization", &[x.clone(), sc.clone(), bi.clone(), mean.clone(), var.clone()], &[attr_f("epsilon", 1e-3)]);
    check(&y, &x.shape, &want, 1e-5, "BatchNormalization");
    let x2 = r.t(&[4, 3]);
    let x2v = x2.f32s();
    let want: Vec<f32> = (0..12).map(|i| (x2v[i] - m[i % 3]) / (v[i % 3] + 1e-5).sqrt() * s[i % 3] + b[i % 3]).collect();
    check(&run1(15, "BatchNormalization", &[x2, sc.clone(), bi.clone(), mean, var], &[]), &[4, 3], &want, 1e-5, "BatchNormalization 2-D");

    // InstanceNorm: per (n, c) plane
    let mut want = vec![0.0f32; xv.len()];
    for (p, ch) in xv.chunks(20).enumerate() {
        let mu = ch.iter().map(|&a| a as f64).sum::<f64>() / 20.0;
        let va = ch.iter().map(|&a| (a as f64 - mu).powi(2)).sum::<f64>() / 20.0;
        for (j, &a) in ch.iter().enumerate() {
            want[p * 20 + j] = (((a as f64 - mu) / (va + 1e-5).sqrt()) * s[p % 3] as f64 + b[p % 3] as f64) as f32;
        }
    }
    check(&run1(13, "InstanceNormalization", &[x.clone(), sc.clone(), bi.clone()], &[]), &x.shape, &want, 1e-5, "InstanceNormalization");

    // LayerNorm over trailing dims from `axis`
    for axis in [-1i64, 2, 1] {
        let ax = if axis < 0 { 4 + axis } else { axis } as usize;
        let inner: usize = x.shape[ax..].iter().product();
        let g = r.t(&x.shape[ax..]);
        let bb = r.t(&x.shape[ax..]);
        let (gv, bv) = (g.f32s(), bb.f32s());
        let mut want = vec![0.0f32; xv.len()];
        for (p, row) in xv.chunks(inner).enumerate() {
            let mu = row.iter().map(|&a| a as f64).sum::<f64>() / inner as f64;
            let va = row.iter().map(|&a| (a as f64 - mu).powi(2)).sum::<f64>() / inner as f64;
            for (j, &a) in row.iter().enumerate() {
                want[p * inner + j] = ((a as f64 - mu) / (va + 1e-5).sqrt() * gv[j] as f64 + bv[j] as f64) as f32;
            }
        }
        let y = run1(17, "LayerNormalization", &[x.clone(), g.clone(), bb.clone()], &[attr_i("axis", axis)]);
        check(&y, &x.shape, &want, 1e-5, &format!("LayerNormalization axis {axis}"));
        // without bias
        let want_nb: Vec<f32> = want.iter().enumerate().map(|(i, &w)| w - bv[i % inner]).collect();
        let y = run(17, "LayerNormalization", &[Some(x.clone()), Some(g.clone())], &[attr_i("axis", axis)], 1);
        check(&y[0], &x.shape, &want_nb, 1e-5, "LayerNormalization no bias");
    }

    // GroupNorm: opset 18 (scale per group) and 21 (per channel)
    let x = r.t(&[2, 6, 3, 2]);
    let xv = x.f32s();
    let groups = 3usize;
    let per = 2 * 6; // cpg * inner
    for opset in [18i64, 21] {
        let n_sc = if opset == 18 { groups } else { 6 };
        let g = r.t(&[n_sc]);
        let bb = r.t(&[n_sc]);
        let (gv, bv) = (g.f32s(), bb.f32s());
        let mut want = vec![0.0f32; xv.len()];
        for (p, blk) in xv.chunks(per).enumerate() {
            let mu = blk.iter().map(|&a| a as f64).sum::<f64>() / per as f64;
            let va = blk.iter().map(|&a| (a as f64 - mu).powi(2)).sum::<f64>() / per as f64;
            let grp = p % groups;
            for (j, &a) in blk.iter().enumerate() {
                let ch = grp * 2 + j / 6;
                let si = if opset == 18 { grp } else { ch };
                want[p * per + j] = ((a as f64 - mu) / (va + 1e-5).sqrt() * gv[si] as f64 + bv[si] as f64) as f32;
            }
        }
        let y = run1(opset, "GroupNormalization", &[x.clone(), g, bb], &[attr_i("num_groups", groups as i64)]);
        check(&y, &x.shape, &want, 1e-5, &format!("GroupNormalization opset {opset}"));
    }
    // LpNormalization
    let x = r.t(&[3, 4]);
    let xv = x.f32s();
    let want: Vec<f32> = (0..12).map(|i| xv[i] / xv[i / 4 * 4..i / 4 * 4 + 4].iter().map(|v| v * v).sum::<f32>().sqrt()).collect();
    check(&run1(13, "LpNormalization", &[x.clone()], &[attr_i("axis", 1)]), &[3, 4], &want, 1e-5, "LpNormalization");
}

fn softmax_rows(v: &[f32], outer: usize, d: usize, inner: usize, kind: &str) -> Vec<f32> {
    let mut out = vec![0.0f32; v.len()];
    for o in 0..outer {
        for i in 0..inner {
            let row: Vec<f64> = (0..d).map(|k| v[(o * d + k) * inner + i] as f64).collect();
            let mx = row.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let s: f64 = row.iter().map(|x| (x - mx).exp()).sum();
            let am = row.iter().position(|&x| x == mx).unwrap();
            for k in 0..d {
                out[(o * d + k) * inner + i] = match kind {
                    "Softmax" => ((row[k] - mx).exp() / s) as f32,
                    "LogSoftmax" => (row[k] - mx - s.ln()) as f32,
                    _ => (k == am) as u8 as f32,
                };
            }
        }
    }
    out
}

#[test]
fn softmax_family() {
    let mut r = Rng::new(14);
    let x = r.t_range(&[2, 3, 4], -5.0, 5.0);
    let xv = x.f32s();
    for kind in ["Softmax", "LogSoftmax", "Hardmax"] {
        // opset 13: per-axis
        for (axis, (o, d, i)) in [(-1i64, (6, 4, 1)), (1, (2, 3, 4)), (0, (1, 2, 12))] {
            let y = run1(13, kind, &[x.clone()], &[attr_i("axis", axis)]);
            check(&y, &x.shape, &softmax_rows(&xv, o, d, i, kind), 1e-5, &format!("{kind}-13 axis {axis}"));
        }
        // opset 11: coerced to 2-D at `axis` (default 1)
        for (axis, (o, d)) in [(1i64, (2, 12)), (2, (6, 4)), (0, (1, 24))] {
            let y = run1(11, kind, &[x.clone()], &[attr_i("axis", axis)]);
            check(&y, &x.shape, &softmax_rows(&xv, o, d, 1, kind), 1e-5, &format!("{kind}-11 axis {axis}"));
        }
        let y = run1(11, kind, &[x.clone()], &[]);
        check(&y, &x.shape, &softmax_rows(&xv, 2, 12, 1, kind), 1e-5, &format!("{kind}-11 default"));
        let y = run1(13, kind, &[x.clone()], &[]);
        check(&y, &x.shape, &softmax_rows(&xv, 6, 4, 1, kind), 1e-5, &format!("{kind}-13 default"));
    }
    // -inf entries (masked attention) stay finite where something is unmasked
    let x = Tensor::f32(&[2, 3], vec![0.0, f32::NEG_INFINITY, 1.0, f32::NEG_INFINITY, 2.0, f32::NEG_INFINITY]);
    let y = run1(13, "Softmax", &[x], &[]);
    let e = 1.0f32.exp();
    check(&y, &[2, 3], &[1.0 / (1.0 + e), 0.0, e / (1.0 + e), 0.0, 1.0, 0.0], 1e-6, "Softmax -inf");
    // large softmax row (threaded path)
    let x = r.t_range(&[64, 1000], -3.0, 3.0);
    check(&run1(13, "Softmax", &[x.clone()], &[]), &x.shape, &softmax_rows(&x.f32s(), 64, 1000, 1, "Softmax"), 1e-5, "Softmax big");
}

fn matmul_ref(a: &Tensor, b: &Tensor) -> Tensor {
    let (mut ash, mut bsh) = (a.shape.clone(), b.shape.clone());
    let (a1, b1) = (ash.len() == 1, bsh.len() == 1);
    if a1 {
        ash.insert(0, 1);
    }
    if b1 {
        bsh.push(1);
    }
    let (m, k, n) = (ash[ash.len() - 2], ash[ash.len() - 1], bsh[bsh.len() - 1]);
    let batch = bshape(&ash[..ash.len() - 2], &bsh[..bsh.len() - 2]);
    let (av, bv) = (a.f32s(), b.f32s());
    let mut out = Vec::new();
    let ab = &ash[..ash.len() - 2];
    let bb = &bsh[..bsh.len() - 2];
    for_idx(&batch, |bi| {
        let oa = if ab.is_empty() { 0 } else { boff(ab, bi) } * m * k;
        let ob = if bb.is_empty() { 0 } else { boff(bb, bi) } * k * n;
        for i in 0..m {
            for j in 0..n {
                let mut s = 0.0f64;
                for kk in 0..k {
                    s += av[oa + i * k + kk] as f64 * bv[ob + kk * n + j] as f64;
                }
                out.push(s as f32);
            }
        }
    });
    let mut os = batch.clone();
    if !a1 {
        os.push(m);
    }
    if !b1 {
        os.push(n);
    }
    Tensor::f32(&os, out)
}

#[test]
fn matmul_shapes() {
    let mut r = Rng::new(15);
    let cases: &[(&[usize], &[usize])] = &[
        (&[3, 4], &[4, 5]),
        (&[4], &[4, 5]),
        (&[3, 4], &[4]),
        (&[4], &[4]),
        (&[1, 7], &[7, 30]),
        (&[2, 3, 4], &[4, 5]),
        (&[2, 1, 3, 4], &[5, 4, 6]),
        (&[3, 4], &[2, 4, 5]),
        (&[2, 3, 4], &[2, 4, 5]),
        (&[2, 3, 1, 4], &[1, 3, 4, 2]),
        (&[5], &[2, 5, 3]),
        (&[2, 3, 5], &[5]),
        (&[33, 70], &[70, 29]),
        (&[1, 1, 300], &[300, 50]),
        (&[4, 1, 64, 64], &[1, 8, 64, 48]),
        (&[130, 513], &[513, 70]),
    ];
    for &(sa, sb) in cases {
        let a = r.t(sa);
        let b = r.t(sb);
        let want = matmul_ref(&a, &b);
        let y = run1(13, "MatMul", &[a, b], &[]);
        check_t(&y, &want, 2e-5, &format!("MatMul {sa:?} x {sb:?}"));
    }
    // f16 B
    let a = r.t(&[3, 8]);
    let b = r.t(&[8, 5]);
    let b16 = Tensor::f16_from_f32(&b.shape, &b.f32s());
    let want = matmul_ref(&a, &b16.cast(DType::F32));
    check_t(&run1(13, "MatMul", &[a, b16], &[]), &want, 1e-5, "MatMul f16");
}

#[test]
fn gemm_variants() {
    let mut r = Rng::new(16);
    let (m, k, n) = (5usize, 7usize, 6usize);
    for (ta, tb) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
        for c_shape in [None, Some(vec![n]), Some(vec![m, 1]), Some(vec![1]), Some(vec![m, n]), Some(vec![])] {
            for (alpha, beta) in [(1.0f32, 1.0f32), (0.5, 2.0), (1.5, 0.0)] {
                let a = r.t(&if ta == 1 { [k, m] } else { [m, k] });
                let b = r.t(&if tb == 1 { [n, k] } else { [k, n] });
                let (av, bv) = (a.f32s(), b.f32s());
                let c = c_shape.as_ref().map(|s| r.t(s));
                let mut want = Vec::new();
                for i in 0..m {
                    for j in 0..n {
                        let mut s = 0.0f64;
                        for kk in 0..k {
                            let x = if ta == 1 { av[kk * m + i] } else { av[i * k + kk] };
                            let y = if tb == 1 { bv[j * k + kk] } else { bv[kk * n + j] };
                            s += x as f64 * y as f64;
                        }
                        let mut v = alpha as f64 * s;
                        if let Some(c) = &c {
                            v += beta as f64 * c.f32s()[boff(&c.shape, &[i, j])] as f64;
                        }
                        want.push(v as f32);
                    }
                }
                let attrs = vec![attr_i("transA", ta), attr_i("transB", tb), attr_f("alpha", alpha), attr_f("beta", beta)];
                let mut ins = vec![Some(a), Some(b)];
                if let Some(c) = c {
                    ins.push(Some(c));
                }
                let y = run(13, "Gemm", &ins, &attrs, 1);
                check(&y[0], &[m, n], &want, 2e-5, &format!("Gemm ta={ta} tb={tb} C={c_shape:?} a={alpha} b={beta}"));
            }
        }
    }
    // m == 1 (GEMV path with packed B)
    let a = r.t(&[1, 40]);
    let b = r.t(&[30, 40]);
    let c = r.t(&[30]);
    let want = matmul_ref(&a, &transpose_ref(&b, &[1, 0]));
    let wv: Vec<f32> = want.f32s().iter().zip(c.f32s().iter()).map(|(x, y)| x + y).collect();
    check(&run1(13, "Gemm", &[a, b, c], &[attr_i("transB", 1)]), &[1, 30], &wv, 2e-5, "Gemm m=1");
}

// ====================================================================== reductions

fn reduce_ref(x: &Tensor, axes: &[usize], keep: bool, op: &str) -> Tensor {
    let r = x.rank();
    let rs: Vec<usize> = (0..r).map(|d| if axes.contains(&d) { 1 } else { x.shape[d] }).collect();
    let xv = x.f32s();
    let mut groups: Vec<Vec<f64>> = vec![Vec::new(); numel(&rs)];
    for_idx(&x.shape, |i| {
        let o: Vec<usize> = (0..r).map(|d| if axes.contains(&d) { 0 } else { i[d] }).collect();
        groups[flat(&rs, &o)].push(xv[flat(&x.shape, i)] as f64);
    });
    let v: Vec<f32> = groups
        .iter()
        .map(|g| {
            let n = g.len() as f64;
            (match op {
                "ReduceSum" => g.iter().sum(),
                "ReduceMean" => g.iter().sum::<f64>() / n,
                "ReduceMax" => g.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                "ReduceMin" => g.iter().copied().fold(f64::INFINITY, f64::min),
                "ReduceProd" => g.iter().product(),
                "ReduceL1" => g.iter().map(|v| v.abs()).sum(),
                "ReduceL2" => g.iter().map(|v| v * v).sum::<f64>().sqrt(),
                "ReduceSumSquare" => g.iter().map(|v| v * v).sum(),
                "ReduceLogSum" => g.iter().sum::<f64>().ln(),
                "ReduceLogSumExp" => g.iter().map(|v| v.exp()).sum::<f64>().ln(),
                _ => unreachable!(),
            }) as f32
        })
        .collect();
    let os: Vec<usize> = if keep { rs } else { (0..r).filter(|d| !axes.contains(d)).map(|d| x.shape[d]).collect() };
    Tensor::f32(&os, v)
}

#[test]
fn reductions() {
    let mut r = Rng::new(17);
    let x = r.t_range(&[3, 4, 5], 0.2, 1.5);
    let ops = ["ReduceSum", "ReduceMean", "ReduceMax", "ReduceMin", "ReduceProd", "ReduceL1", "ReduceL2", "ReduceSumSquare", "ReduceLogSum", "ReduceLogSumExp"];
    let axes_cases: &[&[i64]] = &[&[1], &[-1], &[0, 2], &[0, 1], &[2, 0], &[0, 1, 2]];
    for op in ops {
        for &ax in axes_cases {
            let au: Vec<usize> = ax.iter().map(|&a| if a < 0 { (a + 3) as usize } else { a as usize }).collect();
            for keep in [0i64, 1] {
                let want = reduce_ref(&x, &au, keep == 1, op);
                // opset 13: axes attribute (ReduceSum: input)
                let y = if op == "ReduceSum" {
                    run1(13, op, &[x.clone(), i64t(ax)], &[attr_i("keepdims", keep)])
                } else {
                    run1(13, op, &[x.clone()], &[ints("axes", ax), attr_i("keepdims", keep)])
                };
                check_t(&y, &want, 2e-5, &format!("{op}-13 axes {ax:?} keep {keep}"));
                // opset 18: axes input everywhere
                let y = run1(18, op, &[x.clone(), i64t(ax)], &[attr_i("keepdims", keep)]);
                check_t(&y, &want, 2e-5, &format!("{op}-18 axes {ax:?} keep {keep}"));
            }
        }
        // no axes: reduce everything
        let all = reduce_ref(&x, &[0, 1, 2], true, op);
        check_t(&run1(18, op, &[x.clone()], &[]), &all, 2e-5, &format!("{op} all"));
        // noop_with_empty_axes: identity
        check_t(&run1(18, op, &[x.clone()], &[attr_i("noop_with_empty_axes", 1)]), &x, 0.0, &format!("{op} noop"));
        check_t(&run(18, op, &[Some(x.clone()), Some(i64t(&[]))], &[attr_i("noop_with_empty_axes", 1)], 1)[0], &x, 0.0, &format!("{op} noop empty input"));
        check_t(&run(18, op, &[Some(x.clone()), Some(i64t(&[]))], &[], 1)[0], &all, 2e-5, &format!("{op} empty axes input"));
    }
    // integer reductions keep i64
    let xi = Tensor::i64(&[2, 3], vec![1, -2, 3, 4, 5, -6]);
    let y = run1(13, "ReduceSum", &[xi.clone(), i64t(&[1])], &[attr_i("keepdims", 0)]);
    assert_eq!(y.dtype(), DType::I64);
    check_i(&y, &[2], &[2, 3], "ReduceSum i64");
    check_i(&run1(13, "ReduceMax", &[xi.clone()], &[ints("axes", &[0])]), &[1, 3], &[4, 5, 3], "ReduceMax i64");

    // ArgMax / ArgMin with ties
    let x = Tensor::f32(&[2, 4], vec![1.0, 3.0, 3.0, 0.0, -1.0, -1.0, 2.0, -1.0]);
    check_i(&run1(13, "ArgMax", &[x.clone()], &[attr_i("axis", 1)]), &[2, 1], &[1, 2], "ArgMax");
    check_i(&run1(13, "ArgMax", &[x.clone()], &[attr_i("axis", 1), attr_i("select_last_index", 1)]), &[2, 1], &[2, 2], "ArgMax last");
    check_i(&run1(13, "ArgMin", &[x.clone()], &[attr_i("axis", 1), attr_i("keepdims", 0)]), &[2], &[3, 0], "ArgMin");
    check_i(&run1(13, "ArgMin", &[x.clone()], &[attr_i("axis", -1), attr_i("keepdims", 0), attr_i("select_last_index", 1)]), &[2], &[3, 3], "ArgMin last");
    check_i(&run1(13, "ArgMax", &[x.clone()], &[]), &[1, 4], &[0, 0, 0, 0], "ArgMax axis 0");
    check_i(&run1(13, "ArgMax", &[Tensor::i64(&[3], vec![5, 9, 9])], &[attr_i("select_last_index", 1)]), &[1], &[2], "ArgMax i64");
}

// ====================================================================== Resize

fn cubic_w(x: f64, a: f64) -> f64 {
    let x = x.abs();
    if x <= 1.0 {
        (a + 2.0) * x.powi(3) - (a + 3.0) * x * x + 1.0
    } else if x < 2.0 {
        a * x.powi(3) - 5.0 * a * x * x + 8.0 * a * x - 4.0 * a
    } else {
        0.0
    }
}

struct RS<'a> {
    mode: &'a str,
    coord: &'a str,
    nearest: &'a str,
    cubic_a: f64,
    exclude_outside: bool,
    extrapolation: f32,
}

/// Direct (non-separable) n-d Resize following the ONNX spec formulas.
fn resize_ref(x: &Tensor, os: &[usize], scales: &[f64], roi: &[(f64, f64)], p: &RS) -> Tensor {
    let r = x.rank();
    let xv = x.f32s();
    let mut out = Vec::new();
    for_idx(os, |o| {
        // per axis: list of (index, weight)
        let mut taps: Vec<Vec<(usize, f64)>> = Vec::new();
        let mut outside = false;
        for d in 0..r {
            // coordinates are computed in f32 like onnxruntime does
            let (inl, outl, s) = (x.shape[d] as f32, os[d] as f32, scales[d] as f32);
            let xo = o[d] as f32;
            if x.shape[d] == os[d] && s == 1.0 && p.coord != "tf_crop_and_resize" {
                taps.push(vec![(o[d], 1.0)]);
                continue;
            }
            let xi = match p.coord {
                "asymmetric" => xo / s,
                "align_corners" => {
                    if outl == 1.0 {
                        0.0
                    } else {
                        xo * (inl - 1.0) / (outl - 1.0)
                    }
                }
                "pytorch_half_pixel" => {
                    if outl > 1.0 {
                        (xo + 0.5) / s - 0.5
                    } else {
                        0.0
                    }
                }
                "tf_crop_and_resize" => {
                    let (a, b) = (roi[d].0 as f32, roi[d].1 as f32);
                    if outl > 1.0 { a * (inl - 1.0) + xo * (b - a) * (inl - 1.0) / (outl - 1.0) } else { 0.5 * (a + b) * (inl - 1.0) }
                }
                _ => (xo + 0.5) / s - 0.5,
            } as f64;
            let (inl, s) = (inl as f64, s as f64);
            if p.coord == "tf_crop_and_resize" && (xi < 0.0 || xi > inl - 1.0) {
                outside = true;
            }
            let last = x.shape[d] as i64 - 1;
            let cl = |i: i64| i.clamp(0, last) as usize;
            match p.mode {
                "nearest" => {
                    let fr = xi - xi.floor();
                    let i = match p.nearest {
                        "floor" => xi.floor(),
                        "ceil" => xi.ceil(),
                        "round_prefer_ceil" => {
                            if fr == 0.5 {
                                xi.ceil()
                            } else {
                                xi.round()
                            }
                        }
                        "simple" => {
                            if s < 1.0 {
                                xi.ceil()
                            } else {
                                xi.floor()
                            }
                        }
                        _ => {
                            if fr == 0.5 {
                                xi.floor()
                            } else {
                                xi.round()
                            }
                        }
                    } as i64;
                    taps.push(vec![(cl(i), 1.0)]);
                }
                "linear" => {
                    let xc = if p.coord == "tf_crop_and_resize" { xi } else { xi.clamp(0.0, inl - 1.0) };
                    let i0 = xc.floor() as i64;
                    let f = xc - i0 as f64;
                    taps.push(vec![(cl(i0), 1.0 - f), (cl(i0 + 1), f)]);
                }
                _ => {
                    let i0 = xi.floor() as i64;
                    let f = xi - i0 as f64;
                    let mut t: Vec<(i64, f64)> = (-1..3).map(|k| (i0 + k, cubic_w(k as f64 - f, p.cubic_a))).collect();
                    if p.exclude_outside {
                        for e in t.iter_mut() {
                            if e.0 < 0 || e.0 > last {
                                e.1 = 0.0;
                            }
                        }
                        let s: f64 = t.iter().map(|e| e.1).sum();
                        t.iter_mut().for_each(|e| e.1 /= s);
                    }
                    taps.push(t.into_iter().map(|(i, w)| (cl(i), w)).collect());
                }
            }
        }
        if outside {
            out.push(p.extrapolation);
            return;
        }
        let tshape: Vec<usize> = taps.iter().map(|t| t.len()).collect();
        let mut s = 0.0f64;
        for_idx(&tshape, |ti| {
            let mut w = 1.0;
            let mut src = vec![0; r];
            for d in 0..r {
                let (i, wd) = taps[d][ti[d]];
                w *= wd;
                src[d] = i;
            }
            s += w * xv[flat(&x.shape, &src)] as f64;
        });
        out.push(s as f32);
    });
    Tensor::f32(os, out)
}

fn rs_attrs(p: &RS) -> Vec<Attribute> {
    vec![
        attr_s("mode", p.mode),
        attr_s("coordinate_transformation_mode", p.coord),
        attr_s("nearest_mode", p.nearest),
        attr_f("cubic_coeff_a", p.cubic_a as f32),
        attr_i("exclude_outside", p.exclude_outside as i64),
        attr_f("extrapolation_value", p.extrapolation),
    ]
}

#[test]
fn resize_modes() {
    let mut r = Rng::new(18);
    let x = r.t(&[1, 2, 5, 6]);
    let empty = Tensor::f32(&[0], vec![]);
    let coords = ["half_pixel", "align_corners", "asymmetric", "pytorch_half_pixel"];
    // scales (up, down, mixed) and sizes
    let scale_cases: &[[f32; 2]] = &[[2.0, 2.0], [0.5, 0.5], [1.5, 0.7], [3.0, 1.0], [0.6, 2.5]];
    let mut n_cases = 0;
    for mode in ["nearest", "linear", "cubic"] {
        let nearest_modes: &[&str] = if mode == "nearest" { &["round_prefer_floor", "round_prefer_ceil", "floor", "ceil"] } else { &["round_prefer_floor"] };
        let cubic_as: &[f64] = if mode == "cubic" { &[-0.75, -0.5] } else { &[-0.75] };
        for &coord in &coords {
            for &nm in nearest_modes {
                for &ca in cubic_as {
                    for excl in [false, true] {
                        if excl && mode != "cubic" {
                            continue;
                        }
                        let p = RS { mode, coord, nearest: nm, cubic_a: ca, exclude_outside: excl, extrapolation: 0.0 };
                        for sc in scale_cases {
                            let scales = [1.0f64, 1.0, sc[0] as f64, sc[1] as f64];
                            let os = [1, 2, (5.0 * sc[0]).floor() as usize, (6.0 * sc[1]).floor() as usize];
                            let want = resize_ref(&x, &os, &scales, &[(0.0, 1.0); 4], &p);
                            let st = Tensor::f32(&[4], vec![1.0, 1.0, sc[0], sc[1]]);
                            let y = run1(13, "Resize", &[x.clone(), empty.clone(), st], &rs_attrs(&p));
                            check_t(&y, &want, 2e-5, &format!("Resize {mode} {coord} {nm} a={ca} excl={excl} scales {sc:?}"));
                            n_cases += 1;
                        }
                        for size in [[7usize, 4usize], [10, 12], [3, 9], [1, 1]] {
                            let os = [1, 2, size[0], size[1]];
                            let scales = [1.0, 1.0, size[0] as f64 / 5.0, size[1] as f64 / 6.0];
                            let want = resize_ref(&x, &os, &scales, &[(0.0, 1.0); 4], &p);
                            let y = run(13, "Resize", &[Some(x.clone()), None, None, Some(i64t(&[1, 2, size[0] as i64, size[1] as i64]))], &rs_attrs(&p), 1);
                            check_t(&y[0], &want, 2e-5, &format!("Resize {mode} {coord} {nm} a={ca} excl={excl} sizes {size:?}"));
                            n_cases += 1;
                        }
                    }
                }
            }
        }
    }
    assert!(n_cases > 100);
    // tf_crop_and_resize with roi and extrapolation
    for mode in ["nearest", "linear"] {
        let p = RS { mode, coord: "tf_crop_and_resize", nearest: "round_prefer_floor", cubic_a: -0.75, exclude_outside: false, extrapolation: -7.0 };
        let roi_v = [0.0f32, 0.0, 0.2, -0.1, 1.0, 1.0, 0.9, 1.3];
        let roi: Vec<(f64, f64)> = (0..4).map(|i| (roi_v[i] as f64, roi_v[i + 4] as f64)).collect();
        let os = [1, 2, 4, 5];
        let want = resize_ref(&x, &os, &[1.0, 1.0, 0.8, 5.0 / 6.0], &roi, &p);
        let y = run(13, "Resize", &[Some(x.clone()), Some(Tensor::f32(&[8], roi_v.to_vec())), None, Some(i64t(&[1, 2, 4, 5]))], &rs_attrs(&p), 1);
        check_t(&y[0], &want, 2e-5, &format!("Resize tf_crop_and_resize {mode}"));
    }
    // opset 18 `axes` attribute with 2-element sizes
    let p = RS { mode: "linear", coord: "half_pixel", nearest: "round_prefer_floor", cubic_a: -0.75, exclude_outside: false, extrapolation: 0.0 };
    let want = resize_ref(&x, &[1, 2, 8, 3], &[1.0, 1.0, 8.0 / 5.0, 0.5], &[(0.0, 1.0); 4], &p);
    let mut a = rs_attrs(&p);
    a.push(ints("axes", &[2, 3]));
    let y = run(18, "Resize", &[Some(x.clone()), None, None, Some(i64t(&[8, 3]))], &a, 1);
    check_t(&y[0], &want, 2e-5, "Resize-18 axes sizes");
    let y = run(18, "Resize", &[Some(x.clone()), None, Some(Tensor::f32(&[2], vec![1.6, 0.5]))], &a, 1);
    check_t(&y[0], &resize_ref(&x, &[1, 2, 8, 3], &[1.0, 1.0, 1.6, 0.5], &[(0.0, 1.0); 4], &p), 2e-5, "Resize-18 axes scales");
    // opset 11 (roi input present but ignored), 3-D and 5-D linear
    let x3 = r.t(&[2, 3, 7]);
    let y = run1(11, "Resize", &[x3.clone(), empty.clone(), Tensor::f32(&[3], vec![1.0, 1.0, 1.75])], &rs_attrs(&p));
    check_t(&y, &resize_ref(&x3, &[2, 3, 12], &[1.0, 1.0, 1.75], &[(0.0, 1.0); 3], &p), 2e-5, "Resize 1-D linear");
    let x5 = r.t(&[1, 1, 3, 4, 5]);
    let y = run1(13, "Resize", &[x5.clone(), empty.clone(), Tensor::f32(&[5], vec![1.0, 1.0, 2.0, 1.5, 0.5])], &rs_attrs(&p));
    check_t(&y, &resize_ref(&x5, &[1, 1, 6, 6, 2], &[1.0, 1.0, 2.0, 1.5, 0.5], &[(0.0, 1.0); 5], &p), 2e-5, "Resize 3-D trilinear");
    // legacy: Upsample-9 and Resize-10 are asymmetric; nearest floors on upsampling
    let pn = RS { mode: "nearest", coord: "asymmetric", nearest: "simple", cubic_a: -0.75, exclude_outside: false, extrapolation: 0.0 };
    let sc = Tensor::f32(&[4], vec![1.0, 1.0, 2.0, 3.0]);
    let want = resize_ref(&x, &[1, 2, 10, 18], &[1.0, 1.0, 2.0, 3.0], &[(0.0, 1.0); 4], &pn);
    check_t(&run1(9, "Upsample", &[x.clone(), sc.clone()], &[attr_s("mode", "nearest")]), &want, 0.0, "Upsample-9 nearest");
    check_t(&run1(10, "Resize", &[x.clone(), sc.clone()], &[attr_s("mode", "nearest")]), &want, 0.0, "Resize-10 nearest");
    let pl = RS { mode: "linear", coord: "asymmetric", ..pn };
    let want = resize_ref(&x, &[1, 2, 10, 18], &[1.0, 1.0, 2.0, 3.0], &[(0.0, 1.0); 4], &pl);
    check_t(&run1(10, "Resize", &[x.clone(), sc.clone()], &[attr_s("mode", "linear")]), &want, 1e-6, "Resize-10 linear");
    check_t(&run1(7, "Upsample", &[x.clone()], &[attr_s("mode", "linear"), onnx::attr_floats("scales", &[1.0, 1.0, 2.0, 3.0])]), &want, 1e-6, "Upsample-7 attr scales");
}

// ====================================================================== GridSample

fn grid_ref(x: &Tensor, grid: &Tensor, mode: &str, pad: &str, align: bool) -> Tensor {
    let (n, c, h, w) = (x.shape[0], x.shape[1], x.shape[2], x.shape[3]);
    let (oh, ow) = (grid.shape[1], grid.shape[2]);
    let (xv, gv) = (x.f32s(), grid.f32s());
    let unnorm = |g: f64, s: usize| if align { (g + 1.0) / 2.0 * (s as f64 - 1.0) } else { ((g + 1.0) * s as f64 - 1.0) / 2.0 };
    let reflect = |v: f64, lo2: f64, hi2: f64| -> f64 {
        if lo2 == hi2 {
            return 0.0;
        }
        let (mn, span) = (lo2 / 2.0, (hi2 - lo2) / 2.0);
        let v = (v - mn).abs();
        let extra = v % span;
        let flips = (v / span).floor() as i64;
        if flips % 2 == 0 { extra + mn } else { span - extra + mn }
    };
    let padc = |v: f64, s: usize| -> f64 {
        match pad {
            "border" => v.clamp(0.0, s as f64 - 1.0),
            "reflection" => {
                let r = if align { reflect(v, 0.0, 2.0 * (s as f64 - 1.0)) } else { reflect(v, -1.0, 2.0 * s as f64 - 1.0) };
                r.clamp(0.0, s as f64 - 1.0)
            }
            _ => v,
        }
    };
    let mut out = vec![0.0f32; n * c * oh * ow];
    for b in 0..n {
        for y in 0..oh {
            for xo in 0..ow {
                let gi = ((b * oh + y) * ow + xo) * 2;
                let (ix, iy) = (unnorm(gv[gi] as f64, w), unnorm(gv[gi + 1] as f64, h));
                for ch in 0..c {
                    let at = |yy: i64, xx: i64| -> f64 { if yy < 0 || xx < 0 || yy >= h as i64 || xx >= w as i64 { 0.0 } else { xv[((b * c + ch) * h + yy as usize) * w + xx as usize] as f64 } };
                    let v = match mode {
                        "bilinear" => {
                            let (px, py) = (padc(ix, w), padc(iy, h));
                            let (x0, y0) = (px.floor(), py.floor());
                            let (fx, fy) = (px - x0, py - y0);
                            let (x0, y0) = (x0 as i64, y0 as i64);
                            at(y0, x0) * (1.0 - fx) * (1.0 - fy) + at(y0, x0 + 1) * fx * (1.0 - fy) + at(y0 + 1, x0) * (1.0 - fx) * fy + at(y0 + 1, x0 + 1) * fx * fy
                        }
                        "nearest" => {
                            let (px, py) = (padc(ix, w), padc(iy, h));
                            at(py.round_ties_even() as i64, px.round_ties_even() as i64)
                        }
                        _ => {
                            let (x0, y0) = (ix.floor(), iy.floor());
                            let (fx, fy) = (ix - x0, iy - y0);
                            let mut s = 0.0;
                            for j in 0..4 {
                                for i in 0..4 {
                                    let (xx, yy) = (x0 + i as f64 - 1.0, y0 + j as f64 - 1.0);
                                    let v = if pad == "zeros" { at(yy as i64, xx as i64) } else { at(padc(yy, h) as i64, padc(xx, w) as i64) };
                                    s += cubic_w(i as f64 - 1.0 - fx, -0.75) * cubic_w(j as f64 - 1.0 - fy, -0.75) * v;
                                }
                            }
                            s
                        }
                    };
                    out[((b * c + ch) * oh + y) * ow + xo] = v as f32;
                }
            }
        }
    }
    Tensor::f32(&[n, c, oh, ow], out)
}

#[test]
fn grid_sample_modes() {
    let mut r = Rng::new(19);
    let x = r.t(&[2, 3, 5, 6]);
    // grid points spread well outside [-1, 1] to exercise padding
    let grid = r.t_range(&[2, 4, 7, 2], -1.6, 1.6);
    for mode in ["bilinear", "nearest", "bicubic"] {
        for pad in ["zeros", "border", "reflection"] {
            for align in [0i64, 1] {
                let want = grid_ref(&x, &grid, mode, pad, align == 1);
                let y = run1(16, "GridSample", &[x.clone(), grid.clone()], &[attr_s("mode", mode), attr_s("padding_mode", pad), attr_i("align_corners", align)]);
                check_t(&y, &want, 2e-5, &format!("GridSample {mode} {pad} align={align}"));
            }
        }
    }
    // opset 20 spells the modes linear / cubic
    let want = grid_ref(&x, &grid, "bilinear", "zeros", false);
    check_t(&run1(20, "GridSample", &[x.clone(), grid.clone()], &[attr_s("mode", "linear")]), &want, 2e-5, "GridSample-20 linear");
    // identity grid with align_corners reproduces the input
    let (h, w) = (5usize, 6usize);
    let mut g = Vec::new();
    for y in 0..h {
        for xx in 0..w {
            g.push(xx as f32 / (w - 1) as f32 * 2.0 - 1.0);
            g.push(y as f32 / (h - 1) as f32 * 2.0 - 1.0);
        }
    }
    let g = Tensor::f32(&[2, h, w, 2], [g.clone(), g].concat());
    let y = run1(16, "GridSample", &[x.clone(), g], &[attr_i("align_corners", 1)]);
    check_t(&y, &x, 1e-5, "GridSample identity");
}

// ====================================================================== recurrent

fn sigm(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}

/// `W·x` for a gate block: `w` is [gates*hs, in] row-major, returns row `row`.
fn dotrow(w: &[f32], row: usize, cols: usize, v: &[f64]) -> f64 {
    (0..cols).map(|c| w[row * cols + c] as f64 * v[c]).sum()
}

struct RnnRef {
    y: Vec<f32>,
    yh: Vec<f32>,
    yc: Vec<f32>,
}

/// Hand-written LSTM/GRU/RNN recurrences (layout 0). Peepholes and lens optional.
fn rnn_ref(kind: &str, x: &Tensor, w: &Tensor, rr: &Tensor, b: Option<&Tensor>, lens: Option<&[usize]>, h0: Option<&Tensor>, c0: Option<&Tensor>, p: Option<&Tensor>, dirs: usize, reverse_only: bool, lbr: bool) -> RnnRef {
    let (seq, batch, isz) = (x.shape[0], x.shape[1], x.shape[2]);
    let hs = rr.shape[2];
    let gates = match kind {
        "LSTM" => 4,
        "GRU" => 3,
        _ => 1,
    };
    let (xv, wv, rv) = (x.f32s(), w.f32s(), rr.f32s());
    let mut y = vec![0.0f32; seq * dirs * batch * hs];
    let mut yh = vec![0.0f32; dirs * batch * hs];
    let mut yc = vec![0.0f32; dirs * batch * hs];
    for d in 0..dirs {
        let rev = reverse_only || d == 1;
        let wd = &wv[d * gates * hs * isz..(d + 1) * gates * hs * isz];
        let rd = &rv[d * gates * hs * hs..(d + 1) * gates * hs * hs];
        let bias: Vec<f64> = match b {
            Some(b) => b.f32s()[d * 2 * gates * hs..(d + 1) * 2 * gates * hs].iter().map(|&v| v as f64).collect(),
            None => vec![0.0; 2 * gates * hs],
        };
        let (wb, rb) = bias.split_at(gates * hs);
        for bt in 0..batch {
            let len = lens.map(|l| l[bt]).unwrap_or(seq);
            let mut h: Vec<f64> = (0..hs).map(|j| h0.map(|t| t.f32s()[(d * batch + bt) * hs + j] as f64).unwrap_or(0.0)).collect();
            let mut c: Vec<f64> = (0..hs).map(|j| c0.map(|t| t.f32s()[(d * batch + bt) * hs + j] as f64).unwrap_or(0.0)).collect();
            for step in 0..len {
                let t = if rev { len - 1 - step } else { step };
                let xt: Vec<f64> = (0..isz).map(|i| xv[(t * batch + bt) * isz + i] as f64).collect();
                let pre = |g: usize, j: usize, h: &[f64]| dotrow(wd, g * hs + j, isz, &xt) + dotrow(rd, g * hs + j, hs, h) + wb[g * hs + j] + rb[g * hs + j];
                let hn: Vec<f64> = match kind {
                    "LSTM" => {
                        let pp = |k: usize, j: usize| p.map(|p| p.f32s()[d * 3 * hs + k * hs + j] as f64).unwrap_or(0.0);
                        let mut hn = vec![0.0; hs];
                        let mut cn = vec![0.0; hs];
                        for j in 0..hs {
                            let i_g = sigm(pre(0, j, &h) + pp(0, j) * c[j]);
                            let f_g = sigm(pre(2, j, &h) + pp(2, j) * c[j]);
                            let c_t = pre(3, j, &h).tanh();
                            cn[j] = f_g * c[j] + i_g * c_t;
                            let o_g = sigm(pre(1, j, &h) + pp(1, j) * cn[j]);
                            hn[j] = o_g * cn[j].tanh();
                        }
                        c = cn;
                        hn
                    }
                    "GRU" => {
                        let z: Vec<f64> = (0..hs).map(|j| sigm(pre(0, j, &h))).collect();
                        let rg: Vec<f64> = (0..hs).map(|j| sigm(pre(1, j, &h))).collect();
                        let rh: Vec<f64> = (0..hs).map(|j| rg[j] * h[j]).collect();
                        (0..hs)
                            .map(|j| {
                                let xw = dotrow(wd, 2 * hs + j, isz, &xt) + wb[2 * hs + j];
                                let ht = if lbr { (xw + rg[j] * (dotrow(rd, 2 * hs + j, hs, &h) + rb[2 * hs + j])).tanh() } else { (xw + dotrow(rd, 2 * hs + j, hs, &rh) + rb[2 * hs + j]).tanh() };
                                (1.0 - z[j]) * ht + z[j] * h[j]
                            })
                            .collect()
                    }
                    _ => (0..hs).map(|j| pre(0, j, &h).tanh()).collect(),
                };
                h = hn;
                for j in 0..hs {
                    y[((t * dirs + d) * batch + bt) * hs + j] = h[j] as f32;
                }
            }
            for j in 0..hs {
                yh[(d * batch + bt) * hs + j] = h[j] as f32;
                yc[(d * batch + bt) * hs + j] = c[j] as f32;
            }
        }
    }
    RnnRef { y, yh, yc }
}

#[test]
fn recurrent_ops() {
    let mut r = Rng::new(20);
    let (seq, batch, isz, hs) = (5usize, 3usize, 4usize, 6usize);
    for kind in ["LSTM", "GRU", "RNN"] {
        let gates = match kind {
            "LSTM" => 4,
            "GRU" => 3,
            _ => 1,
        };
        for (direction, dirs) in [("forward", 1usize), ("reverse", 1), ("bidirectional", 2)] {
            for lbr in [false, true] {
                if lbr && kind != "GRU" {
                    continue;
                }
                for with_state in [false, true] {
                    for with_lens in [false, true] {
                        let x = r.t(&[seq, batch, isz]);
                        let w = r.t(&[dirs, gates * hs, isz]);
                        let rr = r.t(&[dirs, gates * hs, hs]);
                        let b = r.t(&[dirs, 2 * gates * hs]);
                        let h0 = r.t(&[dirs, batch, hs]);
                        let c0 = r.t(&[dirs, batch, hs]);
                        let p = r.t(&[dirs, 3 * hs]);
                        let lens = [5usize, 3, 1];
                        let peep = kind == "LSTM" && with_state;
                        let want = rnn_ref(
                            kind,
                            &x,
                            &w,
                            &rr,
                            Some(&b),
                            if with_lens { Some(&lens) } else { None },
                            if with_state { Some(&h0) } else { None },
                            if with_state { Some(&c0) } else { None },
                            if peep { Some(&p) } else { None },
                            dirs,
                            direction == "reverse",
                            lbr,
                        );
                        let mut ins = vec![Some(x.clone()), Some(w.clone()), Some(rr.clone()), Some(b.clone())];
                        ins.push(if with_lens { Some(Tensor::i64(&[batch], lens.iter().map(|&v| v as i64).collect())) } else { None });
                        ins.push(if with_state { Some(h0.clone()) } else { None });
                        if kind == "LSTM" {
                            ins.push(if with_state { Some(c0.clone()) } else { None });
                            ins.push(if peep { Some(p.clone()) } else { None });
                        }
                        let mut attrs = vec![attr_i("hidden_size", hs as i64), attr_s("direction", direction)];
                        if kind == "GRU" {
                            attrs.push(attr_i("linear_before_reset", lbr as i64));
                        }
                        let nout = if kind == "LSTM" { 3 } else { 2 };
                        let ys = run(14, kind, &ins, &attrs, nout);
                        let what = format!("{kind} {direction} lbr={lbr} state={with_state} lens={with_lens}");
                        check(&ys[0], &[seq, dirs, batch, hs], &want.y, 2e-5, &format!("{what} Y"));
                        check(&ys[1], &[dirs, batch, hs], &want.yh, 2e-5, &format!("{what} Y_h"));
                        if kind == "LSTM" {
                            check(&ys[2], &[dirs, batch, hs], &want.yc, 2e-5, &format!("{what} Y_c"));
                        }
                    }
                }
            }
        }
    }
    // layout = 1 (batch-major) matches layout 0 transposed
    let x = r.t(&[seq, batch, isz]);
    let w = r.t(&[1, 4 * hs, isz]);
    let rr = r.t(&[1, 4 * hs, hs]);
    let want = rnn_ref("LSTM", &x, &w, &rr, None, None, None, None, None, 1, false, false);
    let xb = transpose_ref(&x, &[1, 0, 2]);
    let ys = run(14, "LSTM", &[Some(xb), Some(w), Some(rr)], &[attr_i("hidden_size", hs as i64), attr_i("layout", 1)], 3);
    let y0 = Tensor::f32(&[seq, 1, batch, hs], want.y);
    check_t(&ys[0], &transpose_ref(&y0, &[2, 0, 1, 3]), 2e-5, "LSTM layout 1 Y");
    check(&ys[1], &[batch, 1, hs], &want.yh, 2e-5, "LSTM layout 1 Y_h");
}

// ====================================================================== quantization

fn q_ref(x: f32, s: f32, z: f32, lo: f32, hi: f32) -> f32 {
    ((x / s).round_ties_even() + z).clamp(lo, hi)
}

#[test]
fn quant_ops() {
    let mut r = Rng::new(21);
    let x = r.t_range(&[2, 3, 4], -3.0, 3.0);
    let xv = x.f32s();
    // per-tensor u8 and i8
    let y = run1(13, "QuantizeLinear", &[x.clone(), Tensor::scalar_f32(0.02), Tensor::u8(&[], vec![128])], &[]);
    assert_eq!(y.dtype(), DType::U8);
    check(&y, &x.shape, &xv.iter().map(|&v| q_ref(v, 0.02, 128.0, 0.0, 255.0)).collect::<Vec<_>>(), 0.0, "QuantizeLinear u8");
    let y = run1(13, "QuantizeLinear", &[x.clone(), Tensor::scalar_f32(0.03), Tensor::i8(&[], vec![-3])], &[]);
    assert_eq!(y.dtype(), DType::I8);
    check(&y, &x.shape, &xv.iter().map(|&v| q_ref(v, 0.03, -3.0, -128.0, 127.0)).collect::<Vec<_>>(), 0.0, "QuantizeLinear i8");
    let y = run(13, "QuantizeLinear", &[Some(x.clone()), Some(Tensor::scalar_f32(0.05))], &[], 1);
    assert_eq!(y[0].dtype(), DType::U8, "default zero point is uint8");
    // per-axis (axis 1 and default axis)
    let sc = Tensor::f32(&[3], vec![0.01, 0.02, 0.04]);
    let zp = Tensor::i8(&[3], vec![0, 10, -10]);
    let want: Vec<f32> = (0..24).map(|i| q_ref(xv[i], sc.f32s()[(i / 4) % 3], zp.f32s()[(i / 4) % 3], -128.0, 127.0)).collect();
    check(&run1(13, "QuantizeLinear", &[x.clone(), sc.clone(), zp.clone()], &[attr_i("axis", 1)]), &x.shape, &want, 0.0, "QuantizeLinear per-axis");
    check(&run1(13, "QuantizeLinear", &[x.clone(), sc.clone(), zp.clone()], &[]), &x.shape, &want, 0.0, "QuantizeLinear per-axis default");
    let sc4 = Tensor::f32(&[4], vec![0.01, 0.02, 0.04, 0.08]);
    let zp4 = Tensor::u8(&[4], vec![0, 50, 100, 128]);
    let want: Vec<f32> = (0..24).map(|i| q_ref(xv[i], sc4.f32s()[i % 4], zp4.f32s()[i % 4], 0.0, 255.0)).collect();
    check(&run1(13, "QuantizeLinear", &[x.clone(), sc4.clone(), zp4.clone()], &[attr_i("axis", -1)]), &x.shape, &want, 0.0, "QuantizeLinear axis -1");
    // DequantizeLinear
    let q = Tensor::u8(&[2, 3, 4], (0..24).map(|i| (i * 11 % 256) as u8).collect());
    let qv = q.f32s();
    let want: Vec<f32> = (0..24).map(|i| (qv[i] - 128.0) * 0.5).collect();
    check(&run1(13, "DequantizeLinear", &[q.clone(), Tensor::scalar_f32(0.5), Tensor::u8(&[], vec![128])], &[]), &q.shape, &want, 0.0, "DequantizeLinear");
    let want: Vec<f32> = (0..24).map(|i| (qv[i] - zp4.f32s()[i % 4]) * sc4.f32s()[i % 4]).collect();
    check(&run1(13, "DequantizeLinear", &[q.clone(), sc4.clone(), zp4.clone()], &[attr_i("axis", 2)]), &q.shape, &want, 1e-6, "DequantizeLinear per-axis");
    let qi = Tensor::i8(&[3], vec![-128, 0, 127]);
    check(&run(13, "DequantizeLinear", &[Some(qi), Some(Tensor::scalar_f32(0.1))], &[], 1)[0], &[3], &[-12.8, 0.0, 12.7], 1e-6, "DequantizeLinear i8 no zp");

    // DynamicQuantizeLinear
    let x = Tensor::f32(&[6], vec![0.0, 2.0, -3.0, -2.5, 1.34, 0.5]);
    let ys = run(11, "DynamicQuantizeLinear", &[Some(x.clone())], &[], 3);
    let (mn, mx) = (-3.0f32, 2.0f32);
    let scale = (mx - mn) / 255.0;
    let zpv = (-mn / scale).round_ties_even().clamp(0.0, 255.0);
    check(&ys[1], &[], &[scale], 1e-7, "DynamicQuantizeLinear scale");
    check(&ys[2], &[], &[zpv], 0.0, "DynamicQuantizeLinear zp");
    check(&ys[0], &[6], &x.f32s().iter().map(|&v| q_ref(v, scale, zpv, 0.0, 255.0)).collect::<Vec<_>>(), 0.0, "DynamicQuantizeLinear y");
    let ys = run(11, "DynamicQuantizeLinear", &[Some(Tensor::f32(&[3], vec![1.0, 2.0, 4.0]))], &[], 3);
    check(&ys[2], &[], &[0.0], 0.0, "DynamicQuantizeLinear positive zp");

    // MatMulInteger: per-tensor and per-row/col zero points, batched A
    let a = Tensor::u8(&[2, 3, 5], (0..30).map(|i| (i * 37 % 256) as u8).collect());
    let b = Tensor::i8(&[5, 4], (0..20).map(|i| (i * 29 % 256) as u8 as i8).collect());
    let (av, bv) = (a.to_i64_vec(), b.to_i64_vec());
    let mm = |azp: &dyn Fn(usize) -> i64, bzp: &dyn Fn(usize) -> i64| -> Vec<i64> {
        let mut o = Vec::new();
        for bt in 0..2 {
            for i in 0..3 {
                for j in 0..4 {
                    o.push((0..5).map(|k| (av[(bt * 3 + i) * 5 + k] - azp(i)) * (bv[k * 4 + j] - bzp(j))).sum());
                }
            }
        }
        o
    };
    let y = run1(10, "MatMulInteger", &[a.clone(), b.clone()], &[]);
    check_i(&y, &[2, 3, 4], &mm(&|_| 0, &|_| 0), "MatMulInteger");
    let y = run1(10, "MatMulInteger", &[a.clone(), b.clone(), Tensor::u8(&[], vec![100]), Tensor::i8(&[], vec![-5])], &[]);
    check_i(&y, &[2, 3, 4], &mm(&|_| 100, &|_| -5), "MatMulInteger zp");
    let azp = [3i64, 100, 200];
    let bzp = [1i64, -2, 3, -4];
    let y = run1(10, "MatMulInteger", &[a.clone(), b.clone(), Tensor::u8(&[3], azp.iter().map(|&v| v as u8).collect()), Tensor::i8(&[4], bzp.iter().map(|&v| v as i8).collect())], &[]);
    check_i(&y, &[2, 3, 4], &mm(&|i| azp[i], &|j| bzp[j]), "MatMulInteger per-row/col zp");

    // QLinearMatMul
    let (sa, sb, sy) = (0.02f32, 0.01f32, 0.05f32);
    let (za, zb, zy) = (100i64, -5i64, 120f32);
    let acc = mm(&|_| za, &|_| zb);
    let want: Vec<f32> = acc.iter().map(|&v| q_ref(v as f32 * sa * sb, sy, zy, 0.0, 255.0)).collect();
    let y = run1(10, "QLinearMatMul", &[a.clone(), Tensor::scalar_f32(sa), Tensor::u8(&[], vec![za as u8]), b.clone(), Tensor::scalar_f32(sb), Tensor::i8(&[], vec![zb as i8]), Tensor::scalar_f32(sy), Tensor::u8(&[], vec![zy as u8])], &[]);
    assert_eq!(y.dtype(), DType::U8);
    check(&y, &[2, 3, 4], &want, 0.0, "QLinearMatMul");

    // ConvInteger / QLinearConv against the float conv reference on integer data
    let x = Tensor::u8(&[1, 2, 5, 5], (0..50).map(|i| (i * 13 % 256) as u8).collect());
    let w = Tensor::i8(&[3, 2, 3, 3], (0..54).map(|i| ((i * 7) % 21) as i8 - 10).collect());
    let p = ConvP { strides: vec![1, 1], pads: vec![1, 1, 1, 1], dil: vec![1, 1], group: 1 };
    let xz = Tensor::f32(&x.shape, x.f32s().iter().map(|v| v - 7.0).collect());
    let want = conv_ref(&xz, &w.cast(DType::F32), None, &p);
    let y = run1(10, "ConvInteger", &[x.clone(), w.clone(), Tensor::u8(&[], vec![7])], &conv_attrs(&p));
    check_i(&y, &want.shape, &want.f32s().iter().map(|&v| v as i64).collect::<Vec<_>>(), "ConvInteger");
    // per-channel weight zero points
    let wzp = [1i8, -2, 3];
    let wz = Tensor::f32(&w.shape, w.f32s().iter().enumerate().map(|(i, &v)| v - wzp[i / 18] as f32).collect());
    let want = conv_ref(&xz, &wz, None, &p);
    let y = run1(10, "ConvInteger", &[x.clone(), w.clone(), Tensor::u8(&[], vec![7]), Tensor::i8(&[3], wzp.to_vec())], &conv_attrs(&p));
    check_i(&y, &want.shape, &want.f32s().iter().map(|&v| v as i64).collect::<Vec<_>>(), "ConvInteger per-channel zp");
    // QLinearConv with per-channel weight scale and int32 bias
    let ws = [0.01f32, 0.02, 0.005];
    let bias = Tensor::i64(&[3], vec![100, -200, 50]);
    let (xs_, ys_, yz) = (0.1f32, 0.3f32, 10.0f32);
    let accf = conv_ref(&xz, &wz, None, &p);
    let osz = 25;
    let want: Vec<f32> = accf.f32s().iter().enumerate().map(|(i, &a)| q_ref((a + bias.f32s()[i / osz] ) * xs_ * ws[i / osz], ys_, yz, 0.0, 255.0)).collect();
    let y = run1(
        10,
        "QLinearConv",
        &[x.clone(), Tensor::scalar_f32(xs_), Tensor::u8(&[], vec![7]), w.clone(), Tensor::f32(&[3], ws.to_vec()), Tensor::i8(&[3], wzp.to_vec()), Tensor::scalar_f32(ys_), Tensor::u8(&[], vec![yz as u8]), bias],
        &conv_attrs(&p),
    );
    assert_eq!(y.dtype(), DType::U8);
    check(&y, &want_shape(&accf), &want, 0.0, "QLinearConv");

    // contrib: DynamicQuantizeMatMul / MatMulIntegerToFloat roughly equal the float matmul
    let a = r.t(&[3, 16]);
    let bf = r.t(&[16, 5]);
    let bsc = 0.01f32;
    let bq = Tensor::i8(&bf.shape, bf.f32s().iter().map(|&v| (v / bsc).round().clamp(-127.0, 127.0) as i8).collect());
    let want = matmul_ref(&a, &Tensor::f32(&bf.shape, bq.f32s().iter().map(|v| v * bsc).collect()));
    let y = run_d(13, "com.microsoft", "DynamicQuantizeMatMul", &[Some(a.clone()), Some(bq.clone()), Some(Tensor::scalar_f32(bsc))], &[], 1);
    check_t(&y[0], &want, 0.05, "DynamicQuantizeMatMul");
}

fn want_shape(t: &Tensor) -> Vec<usize> {
    t.shape.clone()
}

// ====================================================================== Einsum

fn einsum_ref(eq: &str, ops: &[&Tensor]) -> Tensor {
    let (lhs, rhs) = eq.split_once("->").unwrap();
    let terms: Vec<Vec<char>> = lhs.split(',').map(|t| t.chars().collect()).collect();
    let out: Vec<char> = rhs.chars().collect();
    let mut letters: Vec<char> = Vec::new();
    let mut size = std::collections::HashMap::new();
    for (t, op) in terms.iter().zip(ops) {
        for (c, &d) in t.iter().zip(&op.shape) {
            size.insert(*c, d);
            if !letters.contains(c) {
                letters.push(*c);
            }
        }
    }
    let os: Vec<usize> = out.iter().map(|c| size[c]).collect();
    let ls: Vec<usize> = letters.iter().map(|c| size[c]).collect();
    let mut res = vec![0.0f64; numel(&os)];
    let vals: Vec<Vec<f32>> = ops.iter().map(|t| t.f32s().into_owned()).collect();
    for_idx(&ls, |li| {
        let get = |c: &char| li[letters.iter().position(|x| x == c).unwrap()];
        let mut p = 1.0f64;
        for (k, t) in terms.iter().enumerate() {
            let idx: Vec<usize> = t.iter().map(get).collect();
            p *= vals[k][flat(&ops[k].shape, &idx)] as f64;
        }
        let oi: Vec<usize> = out.iter().map(get).collect();
        res[flat(&os, &oi)] += p;
    });
    Tensor::f32(&os, res.into_iter().map(|v| v as f32).collect())
}

#[test]
fn einsum_patterns() {
    let mut r = Rng::new(22);
    let cases: Vec<(&str, Vec<Vec<usize>>)> = vec![
        ("ij,jk->ik", vec![vec![3, 4], vec![4, 5]]),
        ("bij,bjk->bik", vec![vec![2, 3, 4], vec![2, 4, 5]]),
        ("bhqd,bhkd->bhqk", vec![vec![2, 3, 4, 5], vec![2, 3, 6, 5]]),
        ("bhqk,bhkd->bhqd", vec![vec![2, 3, 4, 6], vec![2, 3, 6, 5]]),
        ("ij->ji", vec![vec![3, 4]]),
        ("abcd->acbd", vec![vec![2, 3, 4, 5]]),
        ("ij->i", vec![vec![3, 4]]),
        ("ijk->", vec![vec![2, 3, 4]]),
        ("i,i->", vec![vec![5], vec![5]]),
        ("i,j->ij", vec![vec![3], vec![4]]),
        ("ii->i", vec![vec![4, 4]]),
        ("ii->", vec![vec![4, 4]]),
        ("bn,anm,bm->ba", vec![vec![2, 3], vec![4, 3, 5], vec![2, 5]]),
        ("nchw,kc->nkhw", vec![vec![2, 3, 4, 5], vec![6, 3]]),
        ("bid,jd->bij", vec![vec![2, 3, 4], vec![5, 4]]),
        ("ij,kj->ikj", vec![vec![3, 4], vec![2, 4]]),
    ];
    for (eq, shapes) in cases {
        let ts: Vec<Tensor> = shapes.iter().map(|s| r.t(s)).collect();
        let refs: Vec<&Tensor> = ts.iter().collect();
        let want = einsum_ref(eq, &refs);
        let y = run1(12, "Einsum", &ts, &[attr_s("equation", eq)]);
        check_t(&y, &want, 2e-5, &format!("Einsum {eq}"));
    }
    // implicit output (alphabetical free letters) and ellipsis
    let a = r.t(&[3, 4]);
    let b = r.t(&[4, 5]);
    check_t(&run1(12, "Einsum", &[a.clone(), b.clone()], &[attr_s("equation", "ij,jk")]), &einsum_ref("ij,jk->ik", &[&a, &b]), 2e-5, "Einsum implicit");
    check_t(&run1(12, "Einsum", &[b.clone(), a.clone()], &[attr_s("equation", "jk,ij")]), &einsum_ref("jk,ij->ik", &[&b, &a]), 2e-5, "Einsum implicit reordered");
    let x = r.t(&[2, 3, 4, 5]);
    let w = r.t(&[5, 6]);
    check_t(&run1(12, "Einsum", &[x.clone(), w.clone()], &[attr_s("equation", "...d,de->...e")]), &einsum_ref("abcd,de->abce", &[&x, &w]), 2e-5, "Einsum ellipsis");
    let q = r.t(&[2, 3, 5, 2]);
    check_t(&run1(12, "Einsum", &[x.clone(), q.clone()], &[attr_s("equation", "...ij,...jk->...ik")]), &einsum_ref("abij,abjk->abik", &[&x, &q]), 2e-5, "Einsum batched ellipsis");
    let v = r.t(&[4, 5]);
    check_t(&run1(12, "Einsum", &[x.clone(), v.clone()], &[attr_s("equation", "a...j,...j->a...")]), &einsum_ref("abcj,cj->abc", &[&x, &v]), 2e-5, "Einsum ellipsis broadcast");
}

// ====================================================================== DFT

fn dft_ref(x: &Tensor, axis: usize, n: usize, inverse: bool, onesided: bool) -> Tensor {
    let r = x.rank();
    let complex = x.shape[r - 1] == 2;
    let mut os = x.shape.clone();
    let nout = if onesided { n / 2 + 1 } else { n };
    os[axis] = nout;
    os[r - 1] = 2;
    let xv = x.f32s();
    let mut out = vec![0.0f32; numel(&os)];
    for_idx(&os[..r - 1], |o| {
        let k = o[axis];
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for t in 0..n.min(x.shape[axis]) {
            let mut src = o.to_vec();
            src[axis] = t;
            src.push(0);
            let p = flat(&x.shape, &src);
            let (a, b) = (xv[p] as f64, if complex { xv[p + 1] as f64 } else { 0.0 });
            let ang = 2.0 * std::f64::consts::PI * (k * t) as f64 / n as f64 * if inverse { 1.0 } else { -1.0 };
            re += a * ang.cos() - b * ang.sin();
            im += a * ang.sin() + b * ang.cos();
        }
        if inverse {
            re /= n as f64;
            im /= n as f64;
        }
        let mut dst = o.to_vec();
        dst.push(0);
        let q = flat(&os, &dst);
        out[q] = re as f32;
        out[q + 1] = im as f32;
    });
    Tensor::f32(&os, out)
}

#[test]
fn dft_op() {
    let mut r = Rng::new(23);
    let xr = r.t(&[2, 12, 1]);
    let xc = r.t(&[2, 10, 3, 2]);
    // opset 17: axis attribute (default 1)
    check_t(&run1(17, "DFT", &[xr.clone()], &[]), &dft_ref(&xr, 1, 12, false, false), 1e-4, "DFT real");
    check_t(&run1(17, "DFT", &[xr.clone()], &[attr_i("onesided", 1)]), &dft_ref(&xr, 1, 12, false, true), 1e-4, "DFT onesided");
    check_t(&run1(17, "DFT", &[xc.clone()], &[attr_i("inverse", 1)]), &dft_ref(&xc, 1, 10, true, false), 1e-4, "DFT inverse");
    check_t(&run1(17, "DFT", &[xc.clone()], &[attr_i("axis", 2)]), &dft_ref(&xc, 2, 3, false, false), 1e-4, "DFT axis 2");
    // dft_length pads / truncates
    check_t(&run(17, "DFT", &[Some(xc.clone()), Some(Tensor::scalar_i64(16))], &[], 1)[0], &dft_ref(&xc, 1, 16, false, false), 1e-4, "DFT length 16");
    check_t(&run(17, "DFT", &[Some(xc.clone()), Some(Tensor::scalar_i64(7))], &[], 1)[0], &dft_ref(&xc, 1, 7, false, false), 1e-4, "DFT length 7");
    // opset 20: axis input (default -2)
    check_t(&run(20, "DFT", &[Some(xc.clone())], &[], 1)[0], &dft_ref(&xc, 2, 3, false, false), 1e-4, "DFT-20 default axis");
    check_t(&run(20, "DFT", &[Some(xc.clone()), None, Some(Tensor::scalar_i64(1))], &[], 1)[0], &dft_ref(&xc, 1, 10, false, false), 1e-4, "DFT-20 axis input");
    // forward then inverse round trip
    let f = run1(17, "DFT", &[xc.clone()], &[]);
    check_t(&run1(17, "DFT", &[f], &[attr_i("inverse", 1)]), &xc, 1e-5, "DFT round trip");
}

// ====================================================================== DeformConv

#[test]
fn deform_conv_op() {
    let mut r = Rng::new(24);
    let (n, c, h, w, oc, kh, kw) = (2usize, 4usize, 6usize, 7usize, 6usize, 3usize, 3usize);
    for (group, og, with_mask, stride, pad, dil) in [(1usize, 1usize, false, 1usize, 1usize, 1usize), (2, 2, true, 1, 1, 1), (1, 2, true, 2, 0, 1), (2, 1, false, 1, 2, 2)] {
        let x = r.t(&[n, c, h, w]);
        let wt = r.t(&[oc, c / group, kh, kw]);
        let b = r.t(&[oc]);
        let oh = (h + 2 * pad - ((kh - 1) * dil + 1)) / stride + 1;
        let ow = (w + 2 * pad - ((kw - 1) * dil + 1)) / stride + 1;
        let off = r.t_range(&[n, og * 2 * kh * kw, oh, ow], -2.5, 2.5);
        let mask = r.t_range(&[n, og * kh * kw, oh, ow], 0.0, 1.0);
        let (xv, wv, ov, mv, bv) = (x.f32s(), wt.f32s(), off.f32s(), mask.f32s(), b.f32s());
        let bil = |plane: &[f32], y: f64, xx: f64| -> f64 {
            if y <= -1.0 || xx <= -1.0 || y >= h as f64 || xx >= w as f64 {
                return 0.0;
            }
            let (y0, x0) = (y.floor() as i64, xx.floor() as i64);
            let (fy, fx) = (y - y0 as f64, xx - x0 as f64);
            let at = |a: i64, bb: i64| if a < 0 || bb < 0 || a >= h as i64 || bb >= w as i64 { 0.0 } else { plane[a as usize * w + bb as usize] as f64 };
            at(y0, x0) * (1.0 - fy) * (1.0 - fx) + at(y0, x0 + 1) * (1.0 - fy) * fx + at(y0 + 1, x0) * fy * (1.0 - fx) + at(y0 + 1, x0 + 1) * fy * fx
        };
        let (icg, ocg, cpo) = (c / group, oc / group, c / og);
        let mut want = vec![0.0f32; n * oc * oh * ow];
        for bn in 0..n {
            for o in 0..oc {
                let g = o / ocg;
                for oy in 0..oh {
                    for ox in 0..ow {
                        let mut s = bv[o] as f64;
                        for ci in 0..icg {
                            let ch = g * icg + ci;
                            let ogi = ch / cpo;
                            let plane = &xv[(bn * c + ch) * h * w..][..h * w];
                            for ky in 0..kh {
                                for kx in 0..kw {
                                    let kk = ky * kw + kx;
                                    let dy = ov[(((bn * og + ogi) * kh * kw + kk) * 2 * oh + oy) * ow + ox];
                                    let dy = {
                                        let _ = dy;
                                        ov[((bn * og * 2 * kh * kw + (ogi * kh * kw + kk) * 2) * oh + oy) * ow + ox]
                                    };
                                    let dx = ov[((bn * og * 2 * kh * kw + (ogi * kh * kw + kk) * 2 + 1) * oh + oy) * ow + ox];
                                    let py = (oy * stride) as f64 - pad as f64 + (ky * dil) as f64 + dy as f64;
                                    let px = (ox * stride) as f64 - pad as f64 + (kx * dil) as f64 + dx as f64;
                                    let mut v = bil(plane, py, px);
                                    if with_mask {
                                        v *= mv[((bn * og * kh * kw + ogi * kh * kw + kk) * oh + oy) * ow + ox] as f64;
                                    }
                                    s += v * wv[((o * icg + ci) * kh + ky) * kw + kx] as f64;
                                }
                            }
                        }
                        want[((bn * oc + o) * oh + oy) * ow + ox] = s as f32;
                    }
                }
            }
        }
        let attrs = vec![
            ints("strides", &[stride as i64; 2]),
            ints("pads", &[pad as i64; 4]),
            ints("dilations", &[dil as i64; 2]),
            attr_i("group", group as i64),
            attr_i("offset_group", og as i64),
            ints("kernel_shape", &[kh as i64, kw as i64]),
        ];
        let mut ins = vec![Some(x), Some(wt), Some(off), Some(b)];
        if with_mask {
            ins.push(Some(mask));
        }
        let y = run(19, "DeformConv", &ins, &attrs, 1);
        check(&y[0], &[n, oc, oh, ow], &want, 2e-5, &format!("DeformConv group={group} og={og} mask={with_mask} s={stride} p={pad} d={dil}"));
    }
    // zero offsets and no mask equal a regular conv
    let x = r.t(&[1, 3, 5, 5]);
    let wt = r.t(&[2, 3, 3, 3]);
    let off = Tensor::f32(&[1, 18, 5, 5], vec![0.0; 450]);
    let p = ConvP { strides: vec![1, 1], pads: vec![1, 1, 1, 1], dil: vec![1, 1], group: 1 };
    let y = run1(19, "DeformConv", &[x.clone(), wt.clone(), off], &[ints("pads", &[1, 1, 1, 1])]);
    check_t(&y, &conv_ref(&x, &wt, None, &p), 2e-5, "DeformConv zero offsets");
}

// ====================================================================== If

#[test]
fn if_reads_outer_scope() {
    let vi = |n: &str| ValueInfo { name: n.into(), elem_type: 1, shape: None };
    let mk_branch = |op: &str, out: &str| Graph {
        name: format!("{op}_branch"),
        nodes: vec![onnx::Node { name: format!("{op}_n"), op_type: op.into(), domain: String::new(), inputs: vec!["x".into(), "k".into()], outputs: vec![out.into()], attrs: vec![] }],
        outputs: vec![vi(out)],
        ..Default::default()
    };
    let mut b = GraphBuilder::new(13);
    b.input("cond", DType::Bool, &[]).input("x", DType::F32, &[3]).init("k", Tensor::f32(&[3], vec![10.0, 20.0, 30.0]));
    b.node("Relu", &["x"], &["xr"], vec![]);
    b.node(
        "If",
        &["cond"],
        &["y"],
        vec![
            Attribute { name: "then_branch".into(), value: AttrValue::Graph(Box::new(mk_branch("Add", "t_out"))) },
            Attribute { name: "else_branch".into(), value: AttrValue::Graph(Box::new(mk_branch("Mul", "e_out"))) },
        ],
    );
    b.node("Add", &["y", "xr"], &["z"], vec![]);
    b.output("z", DType::F32);
    let m = b.build();
    assert!(m.unsupported_ops().is_empty());
    let s = Session::new(&m, Options::default()).unwrap();
    let x = Tensor::f32(&[3], vec![1.0, -2.0, 3.0]);
    let run_c = |c: bool| s.run(&[("cond", Tensor::bool(&[], vec![c])), ("x", x.clone())]).unwrap().remove(0).1;
    check(&run_c(true), &[3], &[12.0, 18.0, 36.0], 0.0, "If then");
    check(&run_c(false), &[3], &[11.0, -40.0, 93.0], 0.0, "If else");
    // nested If in a branch reads the outer-outer scope
    let inner = Graph {
        name: "inner".into(),
        nodes: vec![onnx::Node {
            name: "in_if".into(),
            op_type: "If".into(),
            domain: String::new(),
            inputs: vec!["cond".into()],
            outputs: vec!["nested".into()],
            attrs: vec![
                Attribute { name: "then_branch".into(), value: AttrValue::Graph(Box::new(mk_branch("Sub", "s_out"))) },
                Attribute { name: "else_branch".into(), value: AttrValue::Graph(Box::new(mk_branch("Div", "d_out"))) },
            ],
        }],
        outputs: vec![vi("nested")],
        ..Default::default()
    };
    let mut b = GraphBuilder::new(13);
    b.input("cond", DType::Bool, &[]).input("x", DType::F32, &[3]).init("k", Tensor::f32(&[3], vec![10.0, 20.0, 30.0]));
    b.node(
        "If",
        &["cond"],
        &["y"],
        vec![
            Attribute { name: "then_branch".into(), value: AttrValue::Graph(Box::new(inner.clone())) },
            Attribute { name: "else_branch".into(), value: AttrValue::Graph(Box::new(mk_branch("Mul", "e_out"))) },
        ],
    );
    b.output("y", DType::F32);
    let s = Session::new(&b.build(), Options::default()).unwrap();
    let y = s.run(&[("cond", Tensor::bool(&[], vec![true])), ("x", x.clone())]).unwrap().remove(0).1;
    check(&y, &[3], &[-9.0, -22.0, -27.0], 0.0, "nested If");
}

// ====================================================================== contrib ops

#[test]
fn contrib_ops() {
    let mut r = Rng::new(25);
    let x = r.t(&[2, 3, 8]);
    let skip = r.t(&[2, 3, 8]);
    let g = r.t(&[8]);
    let be = r.t(&[8]);
    let bias = r.t(&[8]);
    let s: Vec<f32> = (0..48).map(|i| x.f32s()[i] + skip.f32s()[i] + bias.f32s()[i % 8]).collect();
    let mut want = vec![0.0f32; 48];
    for (p, row) in s.chunks(8).enumerate() {
        let mu = row.iter().map(|&v| v as f64).sum::<f64>() / 8.0;
        let va = row.iter().map(|&v| (v as f64 - mu).powi(2)).sum::<f64>() / 8.0;
        for j in 0..8 {
            want[p * 8 + j] = ((row[j] as f64 - mu) / (va + 1e-12).sqrt() * g.f32s()[j] as f64 + be.f32s()[j] as f64) as f32;
        }
    }
    let ys = run_d(13, "com.microsoft", "SkipLayerNormalization", &[Some(x.clone()), Some(skip), Some(g), Some(be), Some(bias.clone())], &[], 1);
    check(&ys[0], &[2, 3, 8], &want, 1e-5, "SkipLayerNormalization");
    let want: Vec<f32> = (0..48).map(|i| {
        let v = (x.f32s()[i] + bias.f32s()[i % 8]) as f64;
        (0.5 * v * (1.0 + erf_ref(v / 2f64.sqrt()))) as f32
    }).collect();
    check(&run_d(13, "com.microsoft", "BiasGelu", &[Some(x.clone()), Some(bias.clone())], &[], 1)[0], &[2, 3, 8], &want, 1e-5, "BiasGelu");
    let want: Vec<f32> = x.f32s().iter().map(|&v| v * (1.0 / (1.0 + (-1.702 * v).exp()))).collect();
    check(&run_d(13, "com.microsoft", "QuickGelu", &[Some(x.clone())], &[], 1)[0], &[2, 3, 8], &want, 1e-5, "QuickGelu");
}
