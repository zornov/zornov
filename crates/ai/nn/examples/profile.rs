// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Per-op-type profile of one model run:
//! `cargo run --release -p studio-nn --example profile -- <model> <input.bin> <d0,d1,..> [cpu|metal|auto]`
//! (f32 little-endian input as dumped by studio-ai's model_check).

use studio_nn::{BackendKind, Model, Options, Session, Tensor};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let model = Model::load(&a[1]).expect("model");
    let bytes = std::fs::read(&a[2]).expect("input");
    let shape: Vec<usize> = a[3].split(',').map(|d| d.parse().unwrap()).collect();
    let v: Vec<f32> = bytes.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    let backend = match a.get(4).map(String::as_str) {
        Some("metal") => BackendKind::Metal,
        Some("auto") => BackendKind::Auto,
        _ => BackendKind::Cpu,
    };
    let sess = Session::new(&model, Options { profile: true, ..Options::default() }.with_backend(backend)).expect("session");
    let name = sess.input_names()[0].to_string();
    for i in 0..2 {
        let t0 = std::time::Instant::now();
        let out = sess.run(&[(&name, Tensor::f32(&shape, v.clone()))]).expect("run");
        println!("run {i}: {:.0} ms ({} outputs, backend {})", t0.elapsed().as_secs_f64() * 1e3, out.len(), sess.backend_name());
    }
    let mut p = sess.profile();
    p.sort_by(|x, y| y.1.partial_cmp(&x.1).unwrap());
    let total: f64 = p.iter().map(|x| x.1).sum();
    for (op, secs, n) in p.iter().take(25) {
        println!("{op:<28} {:>9.1} ms {:>5.1}%  ×{n}", secs * 1e3, secs / total * 100.0);
    }
}
