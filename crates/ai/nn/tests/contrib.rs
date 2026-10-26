// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! onnxruntime contrib ops used by real exports.

use studio_nn::onnx::{attr_f, attr_i, GraphBuilder};
use studio_nn::{DType, Session, Tensor};

fn rnd(n: usize, seed: u32) -> Vec<f32> {
    let mut s = seed.wrapping_mul(2654435761).wrapping_add(1);
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            (s % 2000) as f32 / 1000.0 - 1.0
        })
        .collect()
}

/// Naive attention per head on [B, S, N*H] inputs.
fn reference(q: &[f32], k: &[f32], v: &[f32], bias: &[f32], b: usize, s: usize, l: usize, n: usize, h: usize, scale: f32) -> Vec<f32> {
    let d = n * h;
    let mut out = vec![0.0f32; b * s * d];
    for bi in 0..b {
        for hi in 0..n {
            for si in 0..s {
                let qv: Vec<f32> = (0..h).map(|j| q[(bi * s + si) * d + hi * h + j] + bias[hi * h + j]).collect();
                let mut w: Vec<f32> = (0..l)
                    .map(|li| (0..h).map(|j| qv[j] * (k[(bi * l + li) * d + hi * h + j] + bias[d + hi * h + j])).sum::<f32>() * scale)
                    .collect();
                let mx = w.iter().copied().fold(f32::MIN, f32::max);
                let sum: f32 = w.iter_mut().map(|x| {
                    *x = (*x - mx).exp();
                    *x
                }).sum();
                for j in 0..h {
                    out[(bi * s + si) * d + hi * h + j] = (0..l).map(|li| w[li] / sum * (v[(bi * l + li) * d + hi * h + j] + bias[2 * d + hi * h + j])).sum();
                }
            }
        }
    }
    out
}

#[test]
fn multi_head_attention_matches_reference() {
    let (b, s, l, n, h) = (2, 5, 7, 3, 4);
    let d = n * h;
    let mut g = GraphBuilder::new(17);
    g.input("q", DType::F32, &[b as i64, s as i64, d as i64]);
    g.input("k", DType::F32, &[b as i64, l as i64, d as i64]);
    g.input("v", DType::F32, &[b as i64, l as i64, d as i64]);
    g.input("bias", DType::F32, &[3 * d as i64]);
    g.node_domain("com.microsoft", "MultiHeadAttention", &["q", "k", "v", "bias"], &["o"], vec![attr_i("num_heads", n as i64), attr_f("scale", 0.3)]);
    g.output("o", DType::F32);
    let sess = Session::new(&g.build(), Default::default()).unwrap();
    let (q, k, v, bias) = (rnd(b * s * d, 1), rnd(b * l * d, 2), rnd(b * l * d, 3), rnd(3 * d, 4));
    let out = sess
        .run(&[("q", Tensor::f32(&[b, s, d], q.clone())), ("k", Tensor::f32(&[b, l, d], k.clone())), ("v", Tensor::f32(&[b, l, d], v.clone())), ("bias", Tensor::f32(&[3 * d], bias.clone()))])
        .unwrap();
    let want = reference(&q, &k, &v, &bias, b, s, l, n, h, 0.3);
    let got = out[0].1.f32s();
    assert_eq!(out[0].1.shape, vec![b, s, d]);
    for (a, e) in got.iter().zip(&want) {
        assert!((a - e).abs() < 1e-4, "{a} vs {e}");
    }
}
