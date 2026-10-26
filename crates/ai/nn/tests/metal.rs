// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! CPU vs Metal agreement. Every test returns early (with a note) when no
//! Metal device is available.
#![cfg(target_os = "macos")]

use studio_nn::backend::metal::{Metal, MetalBackend};
use studio_nn::backend::{Backend, BackendKind, Cpu, metal_available};
use studio_nn::onnx::{Attribute, GraphBuilder, Node, attr_f, attr_i, attr_ints, attr_s};
use studio_nn::ops::{OpCtx, prepack};
use studio_nn::{DType, Options, Session, Tensor};

fn skip() -> bool {
    if !metal_available() {
        println!("Metal not available; skipping");
        return true;
    }
    false
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
    }
    fn tensor(&mut self, shape: &[usize]) -> Tensor {
        let n: usize = shape.iter().product();
        Tensor::f32(shape, (0..n).map(|_| self.next()).collect())
    }
}

fn node(op: &str, n_in: usize, attrs: Vec<Attribute>) -> Node {
    Node { name: op.into(), op_type: op.into(), inputs: (0..n_in).map(|i| format!("i{i}")).collect(), outputs: vec!["y".into()], attrs, ..Default::default() }
}

fn check(got: &Tensor, want: &Tensor, rel: f32, what: &str) {
    assert_eq!(got.shape, want.shape, "{what}: shape");
    let scale = want.f32s().iter().fold(1.0f32, |m, v| m.max(v.abs()));
    let d = got.max_abs_diff(want);
    assert!(d <= rel * scale, "{what}: max diff {d} (scale {scale})");
}

/// Runs `n` on Metal (must be claimed) and on the CPU, with and without
/// pre-packed weights, and compares.
fn compare(n: &Node, ins: &[&Tensor], rel: f32, what: &str) {
    let gpu = MetalBackend::new(0.0).expect("metal backend");
    let opt: Vec<Option<&Tensor>> = ins.iter().map(|t| Some(*t)).collect();
    let ctx = OpCtx { opset: 17, threads: 0, cache: None };
    let want = Cpu.try_run(n, &opt, &ctx).unwrap().unwrap();
    let got = gpu.try_run(n, &opt, &ctx).unwrap_or_else(|| panic!("{what}: Metal declined")).unwrap();
    check(&got[0], &want[0], rel, what);
    // constant-weight path (weight cache), run twice to hit the cache
    if let Some(pp) = prepack(n, &opt, 0) {
        let ctx = OpCtx { opset: 17, threads: 0, cache: Some(&pp) };
        for pass in 0..2 {
            let got = gpu.try_run(n, &opt, &ctx).unwrap_or_else(|| panic!("{what}: Metal declined (cached)")).unwrap();
            check(&got[0], &want[0], rel, &format!("{what} cached pass {pass}"));
        }
    }
}

#[test]
fn device_info() {
    if skip() {
        return;
    }
    let m = Metal::shared().unwrap();
    println!("device: {} (simdgroup GEMM: {})", m.device_name(), m.has_simdgroup_gemm());
    let a: Vec<f32> = (0..6).map(|v| v as f32).collect();
    let b: Vec<f32> = (0..12).map(|v| v as f32 * 0.5).collect();
    let c = m.sgemm(2, 4, 3, &a, &b).unwrap();
    let mut want = vec![0.0f32; 8];
    for i in 0..2 {
        for j in 0..4 {
            for k in 0..3 {
                want[i * 4 + j] += a[i * 3 + k] * b[k * 4 + j];
            }
        }
    }
    assert_eq!(c, want);
}

#[test]
fn matmul_shapes() {
    if skip() {
        return;
    }
    let mut r = Rng(1);
    let cases: &[(&[usize], &[usize])] = &[
        (&[64, 64], &[64, 64]),
        (&[1, 1], &[1, 1]),
        (&[7, 13], &[13, 5]),
        (&[33, 17], &[17, 65]),
        (&[100, 257], &[257, 129]),
        (&[128, 300], &[300, 96]),
        (&[3, 31, 40], &[40, 70]),     // batch folded into M
        (&[2, 3, 45, 20], &[2, 3, 20, 37]), // batched
        (&[2, 1, 40, 9], &[3, 9, 50]),  // broadcast batch dims
        (&[5, 33], &[4, 33, 41]),       // A broadcast over B's batch
        (&[19], &[19, 70]),             // 1-D A
        (&[40, 19], &[19]),             // 1-D B
    ];
    for (ash, bsh) in cases {
        let a = r.tensor(ash);
        let b = r.tensor(bsh);
        compare(&node("MatMul", 2, vec![]), &[&a, &b], 1e-5, &format!("MatMul {ash:?}x{bsh:?}"));
    }
}

#[test]
fn matmul_f16_weights() {
    if skip() {
        return;
    }
    let mut r = Rng(9);
    let a = r.tensor(&[40, 70]);
    let b32 = r.tensor(&[70, 50]);
    let b = Tensor::f16_from_f32(&[70, 50], &b32.f32s());
    compare(&node("MatMul", 2, vec![]), &[&a, &b], 1e-5, "MatMul f16 B");
}

#[test]
fn gemm_variants() {
    if skip() {
        return;
    }
    let mut r = Rng(2);
    for &(m, n, k) in &[(64usize, 64usize, 64usize), (37, 29, 53), (130, 70, 300), (1, 100, 64), (5, 3, 2)] {
        for ta in [0, 1] {
            for tb in [0, 1] {
                let a = r.tensor(&if ta == 1 { [k, m] } else { [m, k] });
                let b = r.tensor(&if tb == 1 { [n, k] } else { [k, n] });
                let attrs = || vec![attr_i("transA", ta), attr_i("transB", tb), attr_f("alpha", 0.75), attr_f("beta", 1.5)];
                compare(&node("Gemm", 2, attrs()), &[&a, &b], 1e-5, &format!("Gemm {m}x{n}x{k} ta={ta} tb={tb}"));
                for csh in [vec![n], vec![1, n], vec![m, 1], vec![m, n], vec![1]] {
                    let c = r.tensor(&csh);
                    compare(&node("Gemm", 3, attrs()), &[&a, &b, &c], 1e-5, &format!("Gemm {m}x{n}x{k} ta={ta} tb={tb} C{csh:?}"));
                }
            }
        }
    }
}

#[test]
fn conv_variants() {
    if skip() {
        return;
    }
    let mut r = Rng(3);
    struct Case {
        x: [usize; 4],
        w: [usize; 4],
        group: i64,
        strides: [i64; 2],
        pads: [i64; 4],
        dil: [i64; 2],
        bias: bool,
        auto: Option<&'static str>,
    }
    let cases = vec![
        base_like(),
        Case { x: [2, 3, 33, 29], w: [40, 3, 3, 3], strides: [2, 2], pads: [1, 0, 2, 1], ..base_like() },
        Case { x: [1, 16, 31, 31], w: [64, 16, 1, 1], pads: [0; 4], bias: false, ..base_like() },
        Case { x: [2, 16, 32, 32], w: [64, 16, 1, 1], pads: [0; 4], ..base_like() }, // pointwise -> GEMM fast path
        Case { x: [1, 8, 25, 25], w: [32, 8, 3, 3], dil: [2, 2], pads: [2, 2, 2, 2], ..base_like() },
        Case { x: [1, 16, 18, 18], w: [32, 4, 3, 3], group: 4, ..base_like() },  // grouped
        Case { x: [1, 8, 17, 17], w: [16, 1, 3, 3], group: 8, ..base_like() },   // depthwise multiplier 2
        Case { x: [1, 6, 15, 23], w: [70, 6, 5, 3], strides: [1, 2], pads: [2, 1, 2, 1], ..base_like() },
        Case { x: [1, 4, 30, 30], w: [36, 4, 3, 3], strides: [2, 2], pads: [0; 4], auto: Some("SAME_UPPER"), ..base_like() },
        Case { x: [2, 32, 40, 40], w: [64, 32, 3, 3], ..base_like() },
    ];
    fn base_like() -> Case {
        Case { x: [1, 8, 20, 20], w: [16, 8, 3, 3], group: 1, strides: [1, 1], pads: [1, 1, 1, 1], dil: [1, 1], bias: true, auto: None }
    }
    for c in cases {
        let x = r.tensor(&c.x);
        let w = r.tensor(&c.w);
        let b = r.tensor(&[c.w[0]]);
        let mut attrs = vec![attr_i("group", c.group), attr_ints("strides", &c.strides), attr_ints("dilations", &c.dil)];
        match c.auto {
            Some(a) => attrs.push(attr_s("auto_pad", a)),
            None => attrs.push(attr_ints("pads", &c.pads)),
        }
        let what = format!("Conv x{:?} w{:?} g{} s{:?} p{:?} d{:?}", c.x, c.w, c.group, c.strides, c.pads, c.dil);
        if c.bias {
            compare(&node("Conv", 3, attrs), &[&x, &w, &b], 1e-5, &what);
        } else {
            compare(&node("Conv", 2, attrs), &[&x, &w], 1e-5, &what);
        }
    }
    // 1-D conv
    let x = r.tensor(&[2, 6, 50]);
    let w = r.tensor(&[12, 6, 5]);
    compare(&node("Conv", 2, vec![attr_ints("pads", &[2, 2]), attr_ints("strides", &[2])]), &[&x, &w], 1e-5, "Conv1d");
}

#[test]
fn conv_depthwise_declines() {
    if skip() {
        return;
    }
    let mut r = Rng(4);
    let x = r.tensor(&[1, 8, 16, 16]);
    let w = r.tensor(&[8, 1, 3, 3]);
    let gpu = MetalBackend::new(0.0).unwrap();
    let ctx = OpCtx { opset: 17, threads: 0, cache: None };
    assert!(gpu.try_run(&node("Conv", 2, vec![attr_i("group", 8)]), &[Some(&x), Some(&w)], &ctx).is_none());
}

#[test]
fn min_flops_declines_small_ops() {
    if skip() {
        return;
    }
    let mut r = Rng(5);
    let a = r.tensor(&[8, 8]);
    let gpu = MetalBackend::new(1e9).unwrap();
    let ctx = OpCtx { opset: 17, threads: 0, cache: None };
    assert!(gpu.try_run(&node("MatMul", 2, vec![]), &[Some(&a), Some(&a)], &ctx).is_none());
    assert!(gpu.try_run(&node("Relu", 1, vec![]), &[Some(&a)], &ctx).is_none());
}

#[test]
fn elementwise_ops() {
    if skip() {
        return;
    }
    let mut r = Rng(6);
    let x = {
        let t = r.tensor(&[2, 3, 17, 19]);
        Tensor::f32(&t.shape, t.f32s().iter().map(|v| v * 4.0).collect())
    };
    let unary: Vec<(&str, Vec<Attribute>)> = vec![
        ("Relu", vec![]),
        ("Sigmoid", vec![]),
        ("Tanh", vec![]),
        ("Gelu", vec![]),
        ("Gelu", vec![attr_s("approximate", "tanh")]),
        ("Erf", vec![]),
        ("LeakyRelu", vec![attr_f("alpha", 0.2)]),
    ];
    for (op, attrs) in unary {
        compare(&node(op, 1, attrs), &[&x], 1e-5, op);
    }
    // Clip via inputs (opset 11+) and via attributes
    let lo = Tensor::scalar_f32(-0.5);
    let hi = Tensor::scalar_f32(1.25);
    compare(&node("Clip", 3, vec![]), &[&x, &lo, &hi], 0.0, "Clip inputs");
    compare(&node("Clip", 1, vec![attr_f("min", -1.0), attr_f("max", 2.0)]), &[&x], 0.0, "Clip attrs");
    let shapes: &[&[usize]] = &[&[2, 3, 17, 19], &[], &[1], &[19], &[17, 19], &[1, 3, 1, 1], &[3, 1, 1], &[2, 1, 1, 1]];
    for op in ["Add", "Sub", "Mul", "Div"] {
        for s in shapes {
            let y = r.tensor(s);
            compare(&node(op, 2, vec![]), &[&x, &y], 1e-6, &format!("{op} x{s:?}"));
            compare(&node(op, 2, vec![]), &[&y, &x], 1e-6, &format!("{op} {s:?}x"));
        }
    }
    // outer-product broadcast [N,1] (+) [1,M]
    let a = r.tensor(&[40, 1]);
    let b = r.tensor(&[1, 30]);
    compare(&node("Mul", 2, vec![]), &[&a, &b], 1e-6, "Mul outer");
    // unsupported pattern declines
    let gpu = MetalBackend::new(0.0).unwrap();
    let ctx = OpCtx { opset: 17, threads: 0, cache: None };
    let c = r.tensor(&[2, 1, 17, 1]);
    assert!(gpu.try_run(&node("Add", 2, vec![]), &[Some(&x), Some(&c)], &ctx).is_none());
}

fn small_cnn() -> studio_nn::Model {
    let mut r = Rng(7);
    let mut g = GraphBuilder::new(17);
    g.input("x", DType::F32, &[1, 3, 64, 64]);
    g.init("w1", r.tensor(&[16, 3, 3, 3])).init("b1", r.tensor(&[16]));
    g.init("w2", r.tensor(&[32, 16, 3, 3])).init("b2", r.tensor(&[32]));
    g.init("w3", r.tensor(&[32, 8, 3, 3])).init("b3", r.tensor(&[32]));
    g.init("w4", r.tensor(&[64, 32, 1, 1]));
    g.init("bn_s", r.tensor(&[64])).init("bn_b", r.tensor(&[64])).init("bn_m", r.tensor(&[64]));
    g.init("bn_v", Tensor::f32(&[64], (0..64).map(|i| 0.5 + i as f32 / 64.0).collect()));
    g.init("fc", r.tensor(&[10, 64])).init("fcb", r.tensor(&[10]));
    g.init("scale", Tensor::f32(&[1, 64, 1, 1], (0..64).map(|i| 1.0 + i as f32 / 100.0).collect()));
    let p1 = vec![attr_ints("pads", &[1, 1, 1, 1])];
    g.node("Conv", &["x", "w1", "b1"], &["c1"], p1.clone());
    g.node("Relu", &["c1"], &["r1"], vec![]);
    g.node("Conv", &["r1", "w2", "b2"], &["c2"], vec![attr_ints("pads", &[1, 1, 1, 1]), attr_ints("strides", &[2, 2])]);
    g.node("Sigmoid", &["c2"], &["s2"], vec![]);
    g.node("Mul", &["c2", "s2"], &["sw2"], vec![]); // SiLU
    g.node("Conv", &["sw2", "w3", "b3"], &["c3"], vec![attr_ints("pads", &[1, 1, 1, 1]), attr_i("group", 4)]);
    g.node("Add", &["c3", "sw2"], &["res"], vec![]);
    g.node("Conv", &["res", "w4"], &["c4"], vec![]);
    g.node("BatchNormalization", &["c4", "bn_s", "bn_b", "bn_m", "bn_v"], &["bn"], vec![]);
    g.node("Mul", &["bn", "scale"], &["sc"], vec![]);
    g.node("Gelu", &["sc"], &["ge"], vec![]);
    g.node("Clip", &["ge"], &["cl"], vec![attr_f("min", -1.0), attr_f("max", 3.0)]);
    g.node("GlobalAveragePool", &["cl"], &["gp"], vec![]);
    g.node("Flatten", &["gp"], &["fl"], vec![]);
    g.node("Gemm", &["fl", "fc", "fcb"], &["logits"], vec![attr_i("transB", 1)]);
    g.node("MatMul", &["cl", "cl"], &["mm"], vec![]);
    g.output("logits", DType::F32).output("mm", DType::F32);
    g.build()
}

#[test]
fn session_cnn_matches_cpu() {
    if skip() {
        return;
    }
    let model = small_cnn();
    let cpu = Session::new(&model, Options::default()).unwrap();
    let gpu = Session::new(&model, Options { backend: BackendKind::Metal, metal_min_flops: 0.0, ..Default::default() }).unwrap();
    assert_eq!(gpu.backend_name(), "metal");
    let x = Rng(8).tensor(&[1, 3, 64, 64]);
    let want = cpu.run(&[("x", x.clone())]).unwrap();
    for _ in 0..2 {
        let got = gpu.run(&[("x", x.clone())]).unwrap();
        for (g, w) in got.iter().zip(&want) {
            check(&g.1, &w.1, 1e-4, &g.0);
        }
    }
}

#[test]
fn session_auto_backend() {
    if skip() {
        return;
    }
    let model = small_cnn();
    let s = Session::new(&model, Options { backend: BackendKind::Auto, ..Default::default() }).unwrap();
    assert_eq!(s.backend_name(), "metal");
    let x = Rng(8).tensor(&[1, 3, 64, 64]);
    let out = s.run(&[("x", x)]).unwrap();
    assert_eq!(out[0].1.shape, vec![1, 10]);
}

#[test]
fn concurrent_sessions() {
    if skip() {
        return;
    }
    let model = small_cnn();
    let gpu = Session::new(&model, Options { backend: BackendKind::Metal, metal_min_flops: 0.0, ..Default::default() }).unwrap();
    let cpu = Session::new(&model, Options::default()).unwrap();
    let x = Rng(8).tensor(&[1, 3, 64, 64]);
    let want = cpu.run(&[("x", x.clone())]).unwrap();
    std::thread::scope(|s| {
        for _ in 0..4 {
            s.spawn(|| {
                let got = gpu.run(&[("x", x.clone())]).unwrap();
                check(&got[0].1, &want[0].1, 1e-4, "threaded logits");
            });
        }
    });
}
