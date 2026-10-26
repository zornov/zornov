// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Multi-op graphs: optimiser equivalence (BN folding, pre-packing, constant
//! folding, Transpose cancellation), a transformer block against a naive
//! reference, external data, safetensors binding, dynamic shapes and f16
//! weights.

#![allow(clippy::needless_range_loop)]

use studio_nn::onnx::{self, ExternalData, GraphBuilder, attr_f, attr_i, attr_ints, attr_s, attr_t};
use studio_nn::{DType, Model, Options, Session, Tensor};
use std::time::Instant;

struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    fn f(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 40) as f64 / (1u64 << 24) as f64 * 2.0 - 1.0) as f32
    }
    fn t(&mut self, shape: &[usize], scale: f32) -> Tensor {
        let n = shape.iter().product();
        Tensor::f32(shape, (0..n).map(|_| self.f() * scale).collect())
    }
}

fn i64t(v: &[i64]) -> Tensor {
    Tensor::i64(&[v.len()], v.to_vec())
}

fn run(m: &Model, opts: Options, feeds: &[(&str, Tensor)]) -> Vec<Tensor> {
    let s = Session::new(m, opts).unwrap();
    s.run(feeds).unwrap().into_iter().map(|(_, t)| t).collect()
}

fn unopt() -> Options {
    Options { constant_folding: false, fold_batchnorm: false, threads: 1, ..Default::default() }
}

#[track_caller]
fn close(a: &Tensor, b: &Tensor, tol: f32, what: &str) {
    assert_eq!(a.shape, b.shape, "{what}: shape");
    let scale = b.f32s().iter().fold(1.0f32, |m, v| m.max(v.abs()));
    let d = a.max_abs_diff(b);
    assert!(d <= tol * scale, "{what}: max diff {d} (scale {scale})");
}

// ---------------------------------------------------------------- small CNN

fn small_cnn(r: &mut Rng) -> Model {
    let mut b = GraphBuilder::new(13);
    b.input("img", DType::F32, &[2, 3, 16, 16]);
    b.init("w1", r.t(&[8, 3, 3, 3], 0.4)).init("b1", r.t(&[8], 0.1));
    b.init("bn_s", r.t(&[8], 1.0)).init("bn_b", r.t(&[8], 0.2)).init("bn_m", r.t(&[8], 0.2));
    b.init("bn_v", Tensor::f32(&[8], (0..8).map(|i| 0.5 + i as f32 * 0.1).collect()));
    b.init("w2", r.t(&[8, 1, 3, 3], 0.4)); // depthwise, no bias
    b.init("bn2_s", r.t(&[8], 1.0)).init("bn2_b", r.t(&[8], 0.2)).init("bn2_m", r.t(&[8], 0.2));
    b.init("bn2_v", Tensor::f32(&[8], vec![1.0; 8]));
    b.init("fc_w", r.t(&[10, 8 * 4 * 4], 0.1)).init("fc_b", r.t(&[10], 0.1));
    b.node("Conv", &["img", "w1", "b1"], &["c1"], vec![attr_ints("pads", &[1, 1, 1, 1])]);
    b.node("BatchNormalization", &["c1", "bn_s", "bn_b", "bn_m", "bn_v"], &["n1"], vec![attr_f("epsilon", 1e-3)]);
    b.node("Relu", &["n1"], &["r1"], vec![]);
    b.node("MaxPool", &["r1"], &["p1"], vec![attr_ints("kernel_shape", &[2, 2]), attr_ints("strides", &[2, 2])]);
    b.node("Conv", &["p1", "w2"], &["c2"], vec![attr_ints("pads", &[1, 1, 1, 1]), attr_i("group", 8)]);
    b.node("BatchNormalization", &["c2", "bn2_s", "bn2_b", "bn2_m", "bn2_v"], &["n2"], vec![]);
    b.node("Relu", &["n2"], &["r2"], vec![]);
    b.node("MaxPool", &["r2"], &["p2"], vec![attr_ints("kernel_shape", &[2, 2]), attr_ints("strides", &[2, 2])]);
    // flatten through Shape → Gather → Concat → Reshape (constant-folded when static)
    b.node("Shape", &["p2"], &["sh"], vec![]);
    b.node("Constant", &[], &["zero"], vec![attr_t("value", i64t(&[0]))]);
    b.node("Gather", &["sh", "zero"], &["n"], vec![attr_i("axis", 0)]);
    b.node("Constant", &[], &["minus1"], vec![attr_t("value", i64t(&[-1]))]);
    b.node("Concat", &["n", "minus1"], &["flat_shape"], vec![attr_i("axis", 0)]);
    b.node("Reshape", &["p2", "flat_shape"], &["flat"], vec![]);
    b.node("Gemm", &["flat", "fc_w", "fc_b"], &["logits"], vec![attr_i("transB", 1)]);
    b.node("Softmax", &["logits"], &["prob"], vec![attr_i("axis", 1)]);
    b.output("prob", DType::F32);
    b.output("p1", DType::F32);
    b.build()
}

#[test]
fn cnn_optimisations_preserve_results() {
    let mut r = Rng::new(1);
    let m = small_cnn(&mut r);
    let img = r.t(&[2, 3, 16, 16], 1.0);
    let feeds = [("img", img)];
    let base = run(&m, unopt(), &feeds);
    let s = Session::new(&m, Options::default()).unwrap();
    // c1 feeds BN but p1 is a graph output further down, so both BNs fold
    assert_eq!(s.stats.batchnorms_folded, 2, "{:?}", s.stats);
    assert!(s.stats.prepacked >= 2, "{:?}", s.stats);
    assert!(s.stats.folded >= 3, "Shape/Gather/Concat should fold: {:?}", s.stats);
    let opt: Vec<Tensor> = s.run(&feeds).unwrap().into_iter().map(|(_, t)| t).collect();
    for (a, b) in opt.iter().zip(&base) {
        close(a, b, 1e-5, "optimised vs unoptimised");
    }
    // each option on its own
    for opts in [
        Options { fold_batchnorm: false, ..Default::default() },
        Options { constant_folding: false, ..Default::default() },
        Options { threads: 1, ..Default::default() },
        Options { threads: 7, ..Default::default() },
    ] {
        for (a, b) in run(&m, opts.clone(), &feeds).iter().zip(&base) {
            close(a, b, 1e-5, &format!("{opts:?}"));
        }
    }
    // probabilities sum to 1
    let p = base[0].f32s();
    for row in p.chunks(10) {
        assert!((row.iter().sum::<f32>() - 1.0).abs() < 1e-5);
    }
    // serialisation round trip gives the same model and results
    let bytes = m.to_bytes().unwrap();
    let m2 = Model::from_bytes(&bytes).unwrap();
    assert_eq!(m2.graph.nodes.len(), m.graph.nodes.len());
    assert_eq!(m2.op_histogram(), m.op_histogram());
    for (a, b) in run(&m2, Options::default(), &feeds).iter().zip(&base) {
        close(a, b, 1e-5, "after encode/decode");
    }
    assert!(m.report().contains("unsupported: none"));
}

#[test]
fn batchnorm_not_folded_when_conv_output_is_shared() {
    let mut r = Rng::new(2);
    let mut b = GraphBuilder::new(13);
    b.input("x", DType::F32, &[1, 2, 6, 6]);
    b.init("w", r.t(&[3, 2, 3, 3], 0.5));
    for n in ["s", "bb", "m"] {
        b.init(n, r.t(&[3], 0.5));
    }
    b.init("v", Tensor::f32(&[3], vec![0.7, 1.1, 0.9]));
    b.node("Conv", &["x", "w"], &["c"], vec![]);
    b.node("BatchNormalization", &["c", "s", "bb", "m", "v"], &["n"], vec![]);
    b.node("Add", &["n", "c"], &["y"], vec![]);
    b.output("y", DType::F32);
    let m = b.build();
    let x = r.t(&[1, 2, 6, 6], 1.0);
    let s = Session::new(&m, Options::default()).unwrap();
    assert_eq!(s.stats.batchnorms_folded, 0);
    close(&s.run(&[("x", x.clone())]).unwrap()[0].1, &run(&m, unopt(), &[("x", x)])[0], 1e-5, "shared conv output");
}

// ---------------------------------------------------------------- transformer block

struct Block {
    d: usize,
    heads: usize,
    ff: usize,
    ln1: (Tensor, Tensor),
    wqkv: Tensor,
    bqkv: Tensor,
    wo: Tensor,
    ln2: (Tensor, Tensor),
    w1: Tensor,
    b1: Tensor,
    w2: Tensor,
}

fn layer_norm_ref(x: &[f64], g: &[f32], b: &[f32]) -> Vec<f64> {
    let n = x.len() as f64;
    let mu = x.iter().sum::<f64>() / n;
    let var = x.iter().map(|v| (v - mu).powi(2)).sum::<f64>() / n;
    x.iter().enumerate().map(|(i, v)| (v - mu) / (var + 1e-5).sqrt() * g[i] as f64 + b[i] as f64).collect()
}

fn matvec(x: &[f64], w: &[f32], n: usize) -> Vec<f64> {
    // x [k] · w [k, n]
    let k = x.len();
    (0..n).map(|j| (0..k).map(|i| x[i] * w[i * n + j] as f64).sum()).collect()
}

fn erf64(x: f64) -> f64 {
    if x.abs() > 4.0 {
        return x.signum();
    }
    let mut sum = 0.0;
    let mut term = x;
    for n in 0..100 {
        sum += term / (2 * n + 1) as f64;
        term *= -x * x / (n + 1) as f64;
    }
    sum * 2.0 / std::f64::consts::PI.sqrt()
}

fn block_ref(blk: &Block, x: &Tensor) -> Vec<f32> {
    let (bsz, t, d) = (x.shape[0], x.shape[1], x.shape[2]);
    let (h, dh) = (blk.heads, blk.d / blk.heads);
    let xv: Vec<f64> = x.f32s().iter().map(|&v| v as f64).collect();
    let mut out = vec![0.0f32; xv.len()];
    for b in 0..bsz {
        let rows: Vec<Vec<f64>> = (0..t).map(|i| xv[(b * t + i) * d..(b * t + i + 1) * d].to_vec()).collect();
        let qkv: Vec<Vec<f64>> = rows
            .iter()
            .map(|r| {
                let n = layer_norm_ref(r, &blk.ln1.0.f32s(), &blk.ln1.1.f32s());
                let mut v = matvec(&n, &blk.wqkv.f32s(), 3 * d);
                v.iter_mut().zip(blk.bqkv.f32s().iter()).for_each(|(a, b)| *a += *b as f64);
                v
            })
            .collect();
        let mut attn = vec![vec![0.0f64; d]; t];
        for hh in 0..h {
            for i in 0..t {
                let q = &qkv[i][hh * dh..(hh + 1) * dh];
                let scores: Vec<f64> = (0..=i).map(|j| q.iter().zip(&qkv[j][d + hh * dh..d + (hh + 1) * dh]).map(|(a, b)| a * b).sum::<f64>() / (dh as f64).sqrt()).collect();
                let mx = scores.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                let e: Vec<f64> = scores.iter().map(|s| (s - mx).exp()).collect();
                let z: f64 = e.iter().sum();
                for (j, ej) in e.iter().enumerate() {
                    for c in 0..dh {
                        attn[i][hh * dh + c] += ej / z * qkv[j][2 * d + hh * dh + c];
                    }
                }
            }
        }
        for i in 0..t {
            let o = matvec(&attn[i], &blk.wo.f32s(), d);
            let x1: Vec<f64> = rows[i].iter().zip(&o).map(|(a, b)| a + b).collect();
            let n2 = layer_norm_ref(&x1, &blk.ln2.0.f32s(), &blk.ln2.1.f32s());
            let mut hdn = matvec(&n2, &blk.w1.f32s(), blk.ff);
            for (j, v) in hdn.iter_mut().enumerate() {
                let a = *v + blk.b1.f32s()[j] as f64;
                *v = 0.5 * a * (1.0 + erf64(a / 2f64.sqrt()));
            }
            let f = matvec(&hdn, &blk.w2.f32s(), d);
            for c in 0..d {
                out[(b * t + i) * d + c] = (x1[c] + f[c]) as f32;
            }
        }
    }
    out
}

fn block_model(blk: &Block, dynamic: bool) -> Model {
    let (d, h) = (blk.d, blk.heads);
    let dh = d / h;
    let mut b = GraphBuilder::new(17);
    if dynamic {
        b.input("x", DType::F32, &[-1, -2, d as i64]);
    } else {
        b.input("x", DType::F32, &[2, 5, d as i64]);
    }
    b.init("ln1_g", blk.ln1.0.clone()).init("ln1_b", blk.ln1.1.clone()).init("wqkv", blk.wqkv.clone()).init("bqkv", blk.bqkv.clone());
    b.init("wo", blk.wo.clone()).init("ln2_g", blk.ln2.0.clone()).init("ln2_b", blk.ln2.1.clone());
    b.init("w1", blk.w1.clone()).init("b1", blk.b1.clone()).init("w2", blk.w2.clone());
    b.init("split", i64t(&[d as i64; 3]));
    b.init("heads_shape", i64t(&[0, 0, h as i64, dh as i64]));
    b.init("merge_shape", i64t(&[0, 0, d as i64]));
    b.init("scale", Tensor::scalar_f32(1.0 / (dh as f32).sqrt()));
    b.init("one", Tensor::scalar_f32(1.0));
    b.init("half", Tensor::scalar_f32(0.5));
    b.init("sqrt2", Tensor::scalar_f32(std::f32::consts::SQRT_2));
    b.init("neg_inf", Tensor::scalar_f32(f32::NEG_INFINITY));
    b.init("zero_f", Tensor::scalar_f32(0.0));
    b.init("idx1", i64t(&[1]));
    b.node("LayerNormalization", &["x", "ln1_g", "ln1_b"], &["n1"], vec![attr_i("axis", -1)]);
    b.node("MatMul", &["n1", "wqkv"], &["qkv0"], vec![]);
    b.node("Add", &["qkv0", "bqkv"], &["qkv"], vec![]);
    b.node("Split", &["qkv", "split"], &["q0", "k0", "v0"], vec![attr_i("axis", -1)]);
    for (n, perm) in [("q", [0i64, 2, 1, 3]), ("k", [0, 2, 3, 1]), ("v", [0, 2, 1, 3])] {
        b.node("Reshape", &[&format!("{n}0"), "heads_shape"], &[&format!("{n}r")], vec![]);
        b.node("Transpose", &[&format!("{n}r")], &[&format!("{n}t")], vec![attr_ints("perm", &perm)]);
    }
    b.node("MatMul", &["qt", "kt"], &["s0"], vec![]);
    b.node("Mul", &["s0", "scale"], &["s1"], vec![]);
    // causal mask from Shape(x)[1]: Trilu(ones[T, T]) == 0 → -inf
    b.node("Shape", &["x"], &["xs"], vec![]);
    b.node("Gather", &["xs", "idx1"], &["tlen"], vec![attr_i("axis", 0)]);
    b.node("Concat", &["tlen", "tlen"], &["tt"], vec![attr_i("axis", 0)]);
    b.node("ConstantOfShape", &["tt"], &["ones"], vec![attr_t("value", Tensor::f32(&[1], vec![1.0]))]);
    b.node("Trilu", &["ones"], &["tri"], vec![attr_i("upper", 0)]);
    b.node("Equal", &["tri", "zero_f"], &["masked"], vec![]);
    b.node("Where", &["masked", "neg_inf", "s1"], &["s2"], vec![]);
    b.node("Softmax", &["s2"], &["p"], vec![attr_i("axis", -1)]);
    b.node("MatMul", &["p", "vt"], &["a0"], vec![]);
    b.node("Transpose", &["a0"], &["a1"], vec![attr_ints("perm", &[0, 2, 1, 3])]);
    b.node("Reshape", &["a1", "merge_shape"], &["a2"], vec![]);
    b.node("MatMul", &["a2", "wo"], &["o"], vec![]);
    b.node("Add", &["x", "o"], &["x1"], vec![]);
    b.node("LayerNormalization", &["x1", "ln2_g", "ln2_b"], &["n2"], vec![]);
    b.node("MatMul", &["n2", "w1"], &["h0"], vec![]);
    b.node("Add", &["h0", "b1"], &["h1"], vec![]);
    b.node("Div", &["h1", "sqrt2"], &["g0"], vec![]);
    b.node("Erf", &["g0"], &["g1"], vec![]);
    b.node("Add", &["g1", "one"], &["g2"], vec![]);
    b.node("Mul", &["h1", "g2"], &["g3"], vec![]);
    b.node("Mul", &["g3", "half"], &["g4"], vec![]);
    b.node("MatMul", &["g4", "w2"], &["f"], vec![]);
    b.node("Add", &["x1", "f"], &["y"], vec![]);
    b.output("y", DType::F32);
    b.build()
}

fn make_block(r: &mut Rng) -> Block {
    let (d, heads, ff) = (16usize, 4usize, 32usize);
    Block {
        d,
        heads,
        ff,
        ln1: (r.t(&[d], 1.0), r.t(&[d], 0.1)),
        wqkv: r.t(&[d, 3 * d], 0.3),
        bqkv: r.t(&[3 * d], 0.1),
        wo: r.t(&[d, d], 0.3),
        ln2: (r.t(&[d], 1.0), r.t(&[d], 0.1)),
        w1: r.t(&[d, ff], 0.3),
        b1: r.t(&[ff], 0.1),
        w2: r.t(&[ff, d], 0.3),
    }
}

#[test]
fn transformer_block_matches_reference() {
    let mut r = Rng::new(3);
    let blk = make_block(&mut r);
    let x = r.t(&[2, 5, blk.d], 1.0);
    let want = Tensor::f32(&x.shape, block_ref(&blk, &x));
    for dynamic in [false, true] {
        let m = block_model(&blk, dynamic);
        assert!(m.unsupported_ops().is_empty());
        for opts in [Options::default(), unopt()] {
            let y = run(&m, opts, &[("x", x.clone())]);
            close(&y[0], &want, 1e-4, &format!("transformer dynamic={dynamic}"));
        }
    }
    // the dynamic model also handles other batch/sequence sizes in the same session
    let m = block_model(&blk, true);
    let s = Session::new(&m, Options::default()).unwrap();
    for (bsz, t) in [(1usize, 1usize), (3, 7), (2, 5)] {
        let x = r.t(&[bsz, t, blk.d], 1.0);
        let y = s.run(&[("x", x.clone())]).unwrap().remove(0).1;
        close(&y, &Tensor::f32(&x.shape, block_ref(&blk, &x)), 1e-4, &format!("dynamic {bsz}x{t}"));
    }
}

// ---------------------------------------------------------------- transposes

#[test]
fn transpose_pairs_cancel() {
    let mut r = Rng::new(4);
    let x = r.t(&[2, 3, 4, 5], 1.0);
    // NCHW → NHWC → NCHW around nothing: removed
    let mut b = GraphBuilder::new(13);
    b.input("x", DType::F32, &[2, 3, 4, 5]);
    b.node("Transpose", &["x"], &["a"], vec![attr_ints("perm", &[0, 2, 3, 1])]);
    b.node("Transpose", &["a"], &["b"], vec![attr_ints("perm", &[0, 3, 1, 2])]);
    b.node("Relu", &["b"], &["y"], vec![]);
    b.output("y", DType::F32);
    let m = b.build();
    let s = Session::new(&m, Options::default()).unwrap();
    assert_eq!(s.stats.transposes_removed, 2);
    let y = s.run(&[("x", x.clone())]).unwrap().remove(0).1;
    let want = Tensor::f32(&x.shape, x.f32s().iter().map(|v| v.max(0.0)).collect());
    close(&y, &want, 0.0, "cancelled pair");
    // chains of pairs, and default (reversing) perms
    let mut b = GraphBuilder::new(13);
    b.input("x", DType::F32, &[2, 3, 4, 5]);
    b.node("Transpose", &["x"], &["a"], vec![]);
    b.node("Transpose", &["a"], &["b"], vec![]);
    b.node("Transpose", &["b"], &["c"], vec![attr_ints("perm", &[1, 0, 2, 3])]);
    b.node("Transpose", &["c"], &["d"], vec![attr_ints("perm", &[1, 0, 2, 3])]);
    b.node("Neg", &["d"], &["y"], vec![]);
    b.output("y", DType::F32);
    let m = b.build();
    let s = Session::new(&m, Options::default()).unwrap();
    assert_eq!(s.stats.transposes_removed, 4);
    let y = s.run(&[("x", x.clone())]).unwrap().remove(0).1;
    close(&y, &Tensor::f32(&x.shape, x.f32s().iter().map(|v| -v).collect()), 0.0, "two pairs");
    // not inverse: kept; intermediate used elsewhere: kept
    let mut b = GraphBuilder::new(13);
    b.input("x", DType::F32, &[2, 3, 4, 5]);
    b.node("Transpose", &["x"], &["a"], vec![attr_ints("perm", &[0, 2, 3, 1])]);
    b.node("Transpose", &["a"], &["b"], vec![attr_ints("perm", &[0, 2, 3, 1])]);
    b.node("Transpose", &["x"], &["c"], vec![attr_ints("perm", &[1, 0, 2, 3])]);
    b.node("Transpose", &["c"], &["d"], vec![attr_ints("perm", &[1, 0, 2, 3])]);
    b.node("Sigmoid", &["c"], &["e"], vec![]);
    b.output("b", DType::F32).output("d", DType::F32).output("e", DType::F32);
    let m = b.build();
    let s = Session::new(&m, Options::default()).unwrap();
    assert_eq!(s.stats.transposes_removed, 0);
    let got = s.run(&[("x", x.clone())]).unwrap();
    let base = run(&m, unopt(), &[("x", x.clone())]);
    for ((_, a), b) in got.iter().zip(&base) {
        close(a, b, 0.0, "kept transposes");
    }
    assert_eq!(got[0].1.shape, vec![2, 5, 3, 4]);
}

// ---------------------------------------------------------------- external data / safetensors

fn scratch_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("studio-nn-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn external_data_round_trip() {
    let mut r = Rng::new(5);
    let m = small_cnn(&mut r);
    let dir = scratch_dir("extdata");
    let bytes = onnx::encode_model(&m, Some(ExternalData { dir: dir.clone(), file: "weights.bin".into(), min_bytes: 64 })).unwrap();
    let path = dir.join("model.onnx");
    std::fs::write(&path, &bytes).unwrap();
    // the .onnx itself no longer contains the big tensors
    assert!(bytes.len() < 4096, "{} bytes", bytes.len());
    assert!(std::fs::metadata(dir.join("weights.bin")).unwrap().len() > 4 * 10 * 128);
    // loading from bytes without a directory is a clear error
    assert!(Model::from_bytes(&bytes).is_err());
    let m2 = Model::load(&path).unwrap();
    assert_eq!(m2.graph.initializers, m.graph.initializers);
    let img = r.t(&[2, 3, 16, 16], 1.0);
    let a = run(&m, Options::default(), &[("img", img.clone())]);
    let b = run(&m2, Options::default(), &[("img", img)]);
    for (x, y) in a.iter().zip(&b) {
        close(x, y, 0.0, "external data");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn safetensors_bind_initializers() {
    let mut r = Rng::new(6);
    let trained = small_cnn(&mut r);
    // the same graph with zeroed weights, as if exported without weights
    let mut blank = trained.clone();
    for (_, t) in blank.graph.initializers.iter_mut() {
        if t.dtype() == DType::F32 {
            *t = Tensor::zeros(DType::F32, &t.shape);
        }
    }
    let named: Vec<(&str, &Tensor)> = trained.graph.initializers.iter().map(|(n, t)| (n.as_str(), t)).collect();
    let st_bytes = studio_nn::safetensors::to_bytes(&named);
    let dir = scratch_dir("st");
    let p = dir.join("w.safetensors");
    std::fs::write(&p, &st_bytes).unwrap();
    let st = studio_nn::safetensors::load(&p).unwrap();
    assert_eq!(st.tensors.len(), named.len());
    blank.bind_initializers(st.tensors.iter().map(|(n, t)| (n.as_str(), t.clone())));
    assert_eq!(blank.graph.initializers, trained.graph.initializers);
    // binding a new name adds it
    let mut m = blank.clone();
    m.bind_initializers([("extra", Tensor::scalar_f32(1.0))]);
    assert!(m.initializer("extra").is_some());
    let img = r.t(&[2, 3, 16, 16], 1.0);
    let a = run(&trained, Options::default(), &[("img", img.clone())]);
    let b = run(&blank, Options::default(), &[("img", img)]);
    close(&a[0], &b[0], 0.0, "bound weights");
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------- dynamic shapes

#[test]
fn dynamic_shapes_same_session() {
    let mut r = Rng::new(7);
    let mut b = GraphBuilder::new(13);
    b.input("x", DType::F32, &[1, 3, -1, -2]);
    b.init("w", r.t(&[4, 3, 3, 3], 0.4)).init("b", r.t(&[4], 0.1));
    b.init("empty", Tensor::f32(&[0], vec![]));
    b.init("two", i64t(&[2]));
    b.node("Conv", &["x", "w", "b"], &["c"], vec![attr_ints("pads", &[1, 1, 1, 1]), attr_ints("strides", &[2, 2])]);
    b.node("Relu", &["c"], &["r"], vec![]);
    // resize back to the input's spatial size: sizes = Shape(x)
    b.node("Shape", &["x"], &["xs"], vec![]);
    b.node("Shape", &["r"], &["rs"], vec![]);
    b.node("Slice", &["xs", "two", "four"], &["hw"], vec![]);
    b.init("four", i64t(&[4]));
    b.init("zero", i64t(&[0]));
    b.node("Slice", &["rs", "zero", "two"], &["nc"], vec![]);
    b.node("Concat", &["nc", "hw"], &["sizes"], vec![attr_i("axis", 0)]);
    b.node("Resize", &["r", "empty", "empty", "sizes"], &["up"], vec![attr_s("mode", "linear")]);
    b.node("ReduceMean", &["up"], &["m"], vec![attr_ints("axes", &[1]), attr_i("keepdims", 1)]);
    b.node("Flatten", &["m"], &["y"], vec![attr_i("axis", 1)]);
    b.output("y", DType::F32);
    let m = b.build();
    assert_eq!(Session::new(&m, Options::default()).unwrap().input_shape("x"), Some(vec![Some(1), Some(3), None, None]));
    let s = Session::new(&m, Options::default()).unwrap();
    for (h, w) in [(8usize, 10usize), (5, 7), (8, 10)] {
        let x = r.t(&[1, 3, h, w], 1.0);
        let y = s.run(&[("x", x.clone())]).unwrap().remove(0).1;
        assert_eq!(y.shape, vec![1, h * w]);
        // same graph with a static input declaration folds the shape logic
        let mut ms = m.clone();
        ms.graph.inputs[0].shape = Some(vec![onnx::Dim::Value(1), onnx::Dim::Value(3), onnx::Dim::Value(h as i64), onnx::Dim::Value(w as i64)]);
        let ss = Session::new(&ms, Options::default()).unwrap();
        assert!(ss.stats.folded > s.stats.folded);
        close(&ss.run(&[("x", x.clone())]).unwrap()[0].1, &y, 1e-6, "static vs dynamic");
        close(&run(&m, unopt(), &[("x", x)])[0], &y, 1e-6, "dynamic unoptimised");
    }
    // wrong static dims are rejected
    assert!(s.run(&[("x", r.t(&[1, 4, 5, 5], 1.0))]).is_err());
    assert!(s.run(&[("nope", r.t(&[1, 3, 5, 5], 1.0))]).is_err());
}

// ---------------------------------------------------------------- f16

#[test]
fn f16_initializers_and_io() {
    let mut r = Rng::new(8);
    let w = r.t(&[4, 3, 3, 3], 0.5);
    let fc = r.t(&[4 * 6 * 6, 5], 0.1);
    let bias = r.t(&[5], 0.1);
    let to16 = |t: &Tensor| Tensor::f16_from_f32(&t.shape, &t.f32s());
    let build = |half: bool| {
        let mut b = GraphBuilder::new(13);
        b.input("x", if half { DType::F16 } else { DType::F32 }, &[1, 3, 6, 6]);
        if half {
            b.init("w", to16(&w)).init("fc", to16(&fc)).init("bias", to16(&bias));
        } else {
            b.init("w", to16(&w).cast(DType::F32)).init("fc", to16(&fc).cast(DType::F32)).init("bias", to16(&bias).cast(DType::F32));
        }
        b.init("shape", i64t(&[1, -1]));
        b.node("Conv", &["x", "w"], &["c"], vec![attr_ints("pads", &[1, 1, 1, 1])]);
        b.node("Sigmoid", &["c"], &["s"], vec![]);
        b.node("Reshape", &["s", "shape"], &["f"], vec![]);
        b.node("MatMul", &["f", "fc"], &["mm"], vec![]);
        b.node("Add", &["mm", "bias"], &["y"], vec![]);
        b.output("y", if half { DType::F16 } else { DType::F32 });
        b.build()
    };
    let x = r.t(&[1, 3, 6, 6], 1.0);
    let x16 = to16(&x);
    let want = run(&build(false), Options::default(), &[("x", x16.cast(DType::F32))]);
    for opts in [Options::default(), unopt()] {
        let got = run(&build(true), opts, &[("x", x16.clone())]);
        assert_eq!(got[0].dtype(), DType::F16, "declared f16 output comes back as f16");
        close(&got[0].cast(DType::F32), &want[0], 2e-3, "f16 model");
    }
    // the f16 model survives serialisation
    let m = Model::from_bytes(&build(true).to_bytes().unwrap()).unwrap();
    assert_eq!(m.initializer("w").unwrap().dtype(), DType::F16);
}

// ---------------------------------------------------------------- performance (release only)

/// `cargo test --release -p studio-nn --test models -- --ignored --nocapture gemm_throughput`
#[test]
#[ignore]
fn gemm_throughput() {
    use studio_nn::kernels::gemm::{gemm_packed, pack_a, pack_b, sgemm};
    let mut r = Rng::new(9);
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let shapes: Vec<(usize, usize, usize)> = match std::env::var("GEMM_SHAPES") {
        Ok(v) => v.split(';').map(|s| { let d: Vec<usize> = s.split('x').map(|x| x.parse().unwrap()).collect(); (d[0], d[1], d[2]) }).collect(),
        Err(_) => vec![(256, 256, 256), (512, 512, 512), (1024, 1024, 1024), (64, 3136, 576), (2048, 512, 2048)],
    };
    for &(m, n, k) in &shapes {
        let a = r.t(&[m, k], 1.0).f32s().into_owned();
        let b = r.t(&[k, n], 1.0).f32s().into_owned();
        let pa = pack_a(m, k, &a, k, 1);
        let pb = pack_b(k, n, &b, n, 1, 1);
        let mut c = vec![0.0f32; m * n];
        for t in [1, threads] {
            gemm_packed(&pa, &pb, &mut c, n, t); // warm up
            let reps = (2.0e9 / (2.0 * (m * n * k) as f64)).clamp(2.0, 50.0) as usize;
            let t0 = Instant::now();
            for _ in 0..reps {
                gemm_packed(&pa, &pb, &mut c, n, t);
            }
            let dt = t0.elapsed().as_secs_f64() / reps as f64;
            println!("packed {m}x{n}x{k} threads {t:2}: {:7.1} GFLOP/s", 2.0 * (m * n * k) as f64 / dt / 1e9);
        }
        let t0 = Instant::now();
        sgemm(m, n, k, &a, k, 1, &b, n, 1, &mut c, n, false, threads);
        let dt = t0.elapsed().as_secs_f64();
        println!("sgemm  {m}x{n}x{k} threads {threads:2}: {:7.1} GFLOP/s (incl. packing)", 2.0 * (m * n * k) as f64 / dt / 1e9);
    }
}

/// Conv throughput on a ResNet-like layer.
#[test]
#[ignore]
fn conv_throughput() {
    let mut r = Rng::new(10);
    for &(c, h, oc, k, g) in &[(64usize, 56usize, 64usize, 3usize, 1usize), (128, 28, 128, 3, 1), (256, 14, 256, 1, 1), (96, 56, 96, 3, 96)] {
        let mut b = GraphBuilder::new(13);
        b.input("x", DType::F32, &[1, c as i64, h as i64, h as i64]);
        b.init("w", r.t(&[oc, c / g, k, k], 0.1));
        let p = (k / 2) as i64;
        b.node("Conv", &["x", "w"], &["y"], vec![attr_ints("pads", &[p, p, p, p]), attr_i("group", g as i64)]);
        b.output("y", DType::F32);
        let s = Session::new(&b.build(), Options::default()).unwrap();
        let x = r.t(&[1, c, h, h], 1.0);
        let _ = s.run(&[("x", x.clone())]).unwrap();
        let reps = 10;
        let t0 = Instant::now();
        for _ in 0..reps {
            let _ = s.run(&[("x", x.clone())]).unwrap();
        }
        let dt = t0.elapsed().as_secs_f64() / reps as f64;
        let flops = 2.0 * (oc * h * h * (c / g) * k * k) as f64;
        println!("conv c{c} {h}x{h} oc{oc} k{k} g{g}: {:.2} ms, {:.1} GFLOP/s", dt * 1e3, flops / dt / 1e9);
    }
}
