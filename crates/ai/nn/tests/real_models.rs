// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Real-model smoke tests (`cargo test -p studio-nn --test real_models -- --ignored --nocapture`).
//!
//! They only do something when `~/Movies/Studio/Models/` holds the models
//! (one folder per model, e.g. `modnet/model.onnx`,
//! `sam2.1_tiny/vision_encoder.onnx` + `.onnx_data`). A `MANIFEST.tsv` in that
//! folder is optional: any tab-separated field ending in `.onnx` is taken as a
//! path relative to the folder. DeepFilterNet3 must be untarred first; its
//! `enc.onnx` / `erb_dec.onnx` / `df_dec.onnx` are found anywhere below
//! `deepfilternet3/`. Nothing is ever downloaded.

use studio_nn::{DType, Model, Options, Session, Tensor};
use std::path::{Path, PathBuf};
use std::time::Instant;

fn models_dir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("Movies/Studio/Models")
}

/// Models we expect (relative path or a file name searched below a folder).
const EXPECTED: &[(&str, &str)] = &[
    ("modnet", "modnet/model.onnx"),
    ("u2netp", "u2netp/model.onnx"),
    ("depth-anything-v2-small", "depth-anything-v2-small/model.onnx"),
    ("transnetv2", "transnetv2/transnetv2.onnx"),
    ("birefnet_lite", "birefnet_lite/model.onnx"),
    ("sam2.1_tiny encoder", "sam2.1_tiny/vision_encoder.onnx"),
    ("sam2.1_tiny decoder", "sam2.1_tiny/prompt_encoder_mask_decoder.onnx"),
    ("migan", "migan/migan_pipeline_v2.onnx"),
    ("htdemucs", "htdemucs/htdemucs.onnx"),
    ("deepfilternet3 enc", "deepfilternet3/**/enc.onnx"),
    ("deepfilternet3 erb_dec", "deepfilternet3/**/erb_dec.onnx"),
    ("deepfilternet3 df_dec", "deepfilternet3/**/df_dec.onnx"),
];

fn root() -> Option<PathBuf> {
    let p = models_dir();
    if p.is_dir() {
        Some(p)
    } else {
        eprintln!("{} has no models; skipping real-model tests", p.display());
        None
    }
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().and_then(|e| e.to_str()) == Some("onnx") {
            out.push(p);
        }
    }
}

/// Resolves an `EXPECTED` pattern; `dir/**/name` searches recursively.
fn resolve(root: &Path, pattern: &str, manifest: &[PathBuf]) -> Option<PathBuf> {
    if let Some((dir, name)) = pattern.split_once("/**/") {
        let mut found = Vec::new();
        walk(&root.join(dir), &mut found);
        return found.into_iter().find(|p| p.file_name().and_then(|n| n.to_str()) == Some(name));
    }
    let p = root.join(pattern);
    if p.is_file() {
        return Some(p);
    }
    // tolerate a different layout listed in MANIFEST.tsv: match by folder + file name
    let (dir, name) = pattern.split_once('/').unwrap_or(("", pattern));
    manifest.iter().find(|m| m.file_name().and_then(|n| n.to_str()) == Some(name) && m.to_string_lossy().contains(dir)).cloned()
}

fn manifest_paths(root: &Path) -> Vec<PathBuf> {
    let Ok(text) = std::fs::read_to_string(root.join("MANIFEST.tsv")) else { return Vec::new() };
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .flat_map(|l| l.split('\t'))
        .map(|f| f.trim())
        .filter(|f| f.ends_with(".onnx"))
        .map(|f| if Path::new(f).is_absolute() { PathBuf::from(f) } else { root.join(f) })
        .filter(|p| p.is_file())
        .collect()
}

fn find(root: &Path, label: &str) -> Option<PathBuf> {
    let manifest = manifest_paths(root);
    EXPECTED.iter().find(|(l, _)| *l == label).and_then(|(_, pat)| resolve(root, pat, &manifest))
}

#[test]
#[ignore]
fn real_models_load_and_cover_all_ops() {
    let Some(root) = root() else { return };
    let manifest = manifest_paths(&root);
    let mut paths: Vec<(String, PathBuf)> = Vec::new();
    for (label, pat) in EXPECTED {
        match resolve(&root, pat, &manifest) {
            Some(p) => paths.push((label.to_string(), p)),
            None => eprintln!("[{label}] not found ({pat})"),
        }
    }
    // anything else in the folder is reported too
    let mut all = Vec::new();
    walk(&root, &mut all);
    all.extend(manifest);
    for p in all {
        if !paths.iter().any(|(_, q)| *q == p) {
            paths.push((p.strip_prefix(&root).unwrap_or(&p).display().to_string(), p));
        }
    }
    let mut failures = Vec::new();
    for (label, p) in &paths {
        let t0 = Instant::now();
        match Model::load(p) {
            Ok(m) => {
                println!("==== {label} ({}) loaded in {:.2}s\n{}", p.display(), t0.elapsed().as_secs_f64(), m.report());
                let un = m.unsupported_ops();
                if !un.is_empty() {
                    failures.push(format!("{label}: unsupported ops {un:?}"));
                }
                let t1 = Instant::now();
                match Session::new(&m, Options::default()) {
                    Ok(s) => println!("session built in {:.2}s: {:?}", t1.elapsed().as_secs_f64(), s.stats),
                    Err(e) => failures.push(format!("{label}: session: {e}")),
                }
            }
            Err(e) => failures.push(format!("{label}: load: {e}")),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Fills dynamic dims with `defaults` (applied to the unknown dims in order).
fn synthetic_input(shape: &[Option<usize>], defaults: &[usize], dtype: Option<DType>) -> Tensor {
    let mut it = defaults.iter();
    let dims: Vec<usize> = shape.iter().map(|d| d.unwrap_or_else(|| *it.next().unwrap_or(&1))).collect();
    let n: usize = dims.iter().product();
    let v: Vec<f32> = (0..n).map(|i| ((i * 2654435761) % 1000) as f32 / 1000.0).collect();
    let t = Tensor::f32(&dims, v);
    match dtype {
        Some(d) if d != DType::F32 => t.cast(d),
        _ => t,
    }
}

fn run_model(label: &str, defaults: &[usize]) {
    let Some(root) = root() else { return };
    let Some(path) = find(&root, label) else {
        eprintln!("[{label}] not found; skipping");
        return;
    };
    let m = Model::load(&path).unwrap();
    let t0 = Instant::now();
    let s = Session::new(&m, Options::default()).unwrap();
    let build = t0.elapsed().as_secs_f64();
    let dtypes: Vec<Option<DType>> = m.inputs().iter().map(|v| v.dtype()).collect();
    let feeds: Vec<(String, Tensor)> = s
        .input_names()
        .iter()
        .zip(dtypes)
        .map(|(n, dt)| {
            let shape = s.input_shape(n).unwrap_or_else(|| vec![None; 4]);
            (n.to_string(), synthetic_input(&shape, defaults, dt))
        })
        .collect();
    for (n, t) in &feeds {
        println!("[{label}] input {n}: {:?} {:?}", t.shape, t.dtype());
    }
    let mut times = Vec::new();
    let mut outs = Vec::new();
    for _ in 0..2 {
        let t1 = Instant::now();
        outs = s.run_owned(feeds.clone()).unwrap();
        times.push(t1.elapsed().as_secs_f64());
    }
    println!("[{label}] session build {build:.2}s, run {:.3}s (first) {:.3}s (second)", times[0], times[1]);
    for (n, t) in &outs {
        let v = t.f32s();
        let (mn, mx) = v.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), &x| (a.min(x), b.max(x)));
        println!("[{label}] output {n}: {:?} min {mn} max {mx}", t.shape);
        assert!(v.iter().all(|x| x.is_finite()), "[{label}] output {n} has non-finite values");
    }
}

#[test]
#[ignore]
fn real_modnet() {
    run_model("modnet", &[1, 512, 512]);
}

#[test]
#[ignore]
fn real_u2netp() {
    run_model("u2netp", &[1, 320, 320]);
}

#[test]
#[ignore]
fn real_depth_anything_v2_small() {
    run_model("depth-anything-v2-small", &[1, 518, 518]);
}

#[test]
#[ignore]
fn real_transnetv2() {
    run_model("transnetv2", &[1, 100, 27, 48]);
}
