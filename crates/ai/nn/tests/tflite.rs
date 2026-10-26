// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! TFLite importer tests: models are emitted by a small test-only FlatBuffers
//! builder and checked against naive NHWC reference implementations.

use studio_nn::tensor::{f16_to_f32, f32_to_f16};
use studio_nn::{Model, Options, Session, Tensor};

// ------------------------------------------------------------------ flatbuffers builder

#[derive(Clone)]
enum F {
    U8(u8),
    I32(i32),
    U32(u32),
    F32(f32),
    Table(T),
    Tables(Vec<T>),
    I32s(Vec<i32>),
    Bytes(Vec<u8>),
    Str(String),
}

#[derive(Clone, Default)]
struct T(Vec<(usize, F)>);

impl T {
    fn with(mut self, id: usize, f: F) -> T {
        self.0.push((id, f));
        self
    }
}

fn align(buf: &mut Vec<u8>, a: usize) {
    while buf.len() % a != 0 {
        buf.push(0);
    }
}

fn patch(buf: &mut [u8], at: usize, target: usize) {
    buf[at..at + 4].copy_from_slice(&((target - at) as u32).to_le_bytes());
}

/// Writes the vtable, then the table, then its children (forward offsets).
fn write_table(buf: &mut Vec<u8>, t: &T) -> usize {
    let nf = t.0.iter().map(|(id, _)| id + 1).max().unwrap_or(0);
    let mut slots = vec![0u16; nf];
    let mut off = 4usize;
    let wide = |f: &F| !matches!(f, F::U8(_));
    for pass in [true, false] {
        for (id, f) in &t.0 {
            if wide(f) == pass {
                if pass {
                    off = off.div_ceil(4) * 4;
                }
                slots[*id] = off as u16;
                off += if pass { 4 } else { 1 };
            }
        }
    }
    let tsize = off.div_ceil(4) * 4;
    align(buf, 2);
    let vt = buf.len();
    buf.extend_from_slice(&((4 + 2 * nf) as u16).to_le_bytes());
    buf.extend_from_slice(&(tsize as u16).to_le_bytes());
    for s in &slots {
        buf.extend_from_slice(&s.to_le_bytes());
    }
    align(buf, 4);
    let tp = buf.len();
    buf.resize(tp + tsize, 0);
    buf[tp..tp + 4].copy_from_slice(&((tp - vt) as i32).to_le_bytes());
    let mut children = Vec::new();
    for (id, f) in &t.0 {
        let p = tp + slots[*id] as usize;
        match f {
            F::U8(v) => buf[p] = *v,
            F::I32(v) => buf[p..p + 4].copy_from_slice(&v.to_le_bytes()),
            F::U32(v) => buf[p..p + 4].copy_from_slice(&v.to_le_bytes()),
            F::F32(v) => buf[p..p + 4].copy_from_slice(&v.to_le_bytes()),
            other => children.push((p, other.clone())),
        }
    }
    for (p, f) in children {
        let c = write_child(buf, &f);
        patch(buf, p, c);
    }
    tp
}

fn write_child(buf: &mut Vec<u8>, f: &F) -> usize {
    match f {
        F::Table(t) => write_table(buf, t),
        F::Tables(v) => {
            align(buf, 4);
            let p = buf.len();
            buf.extend_from_slice(&(v.len() as u32).to_le_bytes());
            buf.resize(p + 4 + 4 * v.len(), 0);
            for (i, t) in v.iter().enumerate() {
                let tp = write_table(buf, t);
                patch(buf, p + 4 + 4 * i, tp);
            }
            p
        }
        F::I32s(v) => {
            align(buf, 4);
            let p = buf.len();
            buf.extend_from_slice(&(v.len() as u32).to_le_bytes());
            for x in v {
                buf.extend_from_slice(&x.to_le_bytes());
            }
            p
        }
        F::Bytes(v) => {
            while (buf.len() + 4) % 16 != 0 {
                buf.push(0);
            }
            let p = buf.len();
            buf.extend_from_slice(&(v.len() as u32).to_le_bytes());
            buf.extend_from_slice(v);
            p
        }
        F::Str(s) => {
            align(buf, 4);
            let p = buf.len();
            buf.extend_from_slice(&(s.len() as u32).to_le_bytes());
            buf.extend_from_slice(s.as_bytes());
            buf.push(0);
            p
        }
        _ => unreachable!("scalar child"),
    }
}

fn finish(root: &T) -> Vec<u8> {
    let mut buf = vec![0u8; 8];
    buf[4..8].copy_from_slice(b"TFL3");
    let r = write_table(&mut buf, root);
    patch(&mut buf, 0, r);
    buf
}

// ------------------------------------------------------------------ TFLite model builder

const FLOAT32: u8 = 0;
const FLOAT16: u8 = 1;
const INT32: u8 = 2;

#[derive(Default)]
struct Tfl {
    tensors: Vec<T>,
    shapes: Vec<Vec<i32>>,
    buffers: Vec<T>,
    ops: Vec<T>,
    codes: Vec<(i32, String)>,
    inputs: Vec<i32>,
}

impl Tfl {
    fn new() -> Tfl {
        Tfl { buffers: vec![T::default()], ..Default::default() }
    }
    fn tensor(&mut self, name: &str, shape: &[i32], ty: u8, data: Option<Vec<u8>>) -> i32 {
        let buf = match data {
            Some(d) => {
                self.buffers.push(T::default().with(0, F::Bytes(d)));
                self.buffers.len() as u32 - 1
            }
            None => 0,
        };
        self.tensors.push(T::default().with(0, F::I32s(shape.to_vec())).with(1, F::U8(ty)).with(2, F::U32(buf)).with(3, F::Str(name.into())));
        self.shapes.push(shape.to_vec());
        self.tensors.len() as i32 - 1
    }
    fn act(&mut self, name: &str, shape: &[i32]) -> i32 {
        self.tensor(name, shape, FLOAT32, None)
    }
    fn input(&mut self, name: &str, shape: &[i32]) -> i32 {
        let i = self.act(name, shape);
        self.inputs.push(i);
        i
    }
    fn cf32(&mut self, name: &str, shape: &[i32], v: &[f32]) -> i32 {
        self.tensor(name, shape, FLOAT32, Some(v.iter().flat_map(|x| x.to_le_bytes()).collect()))
    }
    fn cf16(&mut self, name: &str, shape: &[i32], v: &[f32]) -> i32 {
        self.tensor(name, shape, FLOAT16, Some(v.iter().flat_map(|&x| f32_to_f16(x).to_le_bytes()).collect()))
    }
    fn ci32(&mut self, name: &str, shape: &[i32], v: &[i32]) -> i32 {
        self.tensor(name, shape, INT32, Some(v.iter().flat_map(|x| x.to_le_bytes()).collect()))
    }
    fn code(&mut self, code: i32, custom: &str) -> u32 {
        let k = (code, custom.to_string());
        if let Some(i) = self.codes.iter().position(|c| *c == k) {
            return i as u32;
        }
        self.codes.push(k);
        self.codes.len() as u32 - 1
    }
    fn op(&mut self, code: i32, ins: &[i32], outs: &[i32], opts: Option<(u8, T)>) {
        let oc = self.code(code, "");
        let mut t = T::default().with(0, F::U32(oc)).with(1, F::I32s(ins.to_vec())).with(2, F::I32s(outs.to_vec()));
        if let Some((ty, o)) = opts {
            t = t.with(3, F::U8(ty)).with(4, F::Table(o));
        }
        self.ops.push(t);
    }
    fn custom(&mut self, name: &str, ins: &[i32], outs: &[i32], words: &[i32]) {
        let oc = self.code(32, name);
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        self.ops.push(T::default().with(0, F::U32(oc)).with(1, F::I32s(ins.to_vec())).with(2, F::I32s(outs.to_vec())).with(5, F::Bytes(bytes)));
    }
    fn build(&self, outputs: &[i32]) -> Vec<u8> {
        let codes: Vec<T> = self
            .codes
            .iter()
            .map(|(c, s)| {
                let mut t = T::default().with(0, F::U8((*c).min(127) as u8)).with(2, F::I32(1)).with(3, F::I32(*c));
                if !s.is_empty() {
                    t = t.with(1, F::Str(s.clone()));
                }
                t
            })
            .collect();
        let sg = T::default()
            .with(0, F::Tables(self.tensors.clone()))
            .with(1, F::I32s(self.inputs.clone()))
            .with(2, F::I32s(outputs.to_vec()))
            .with(3, F::Tables(self.ops.clone()))
            .with(4, F::Str("main".into()));
        let model = T::default()
            .with(0, F::U32(3))
            .with(1, F::Tables(codes))
            .with(2, F::Tables(vec![sg]))
            .with(3, F::Str("studio test model".into()))
            .with(4, F::Tables(self.buffers.clone()));
        finish(&model)
    }
}

// builtin codes
const ADD: i32 = 0;
const AVERAGE_POOL_2D: i32 = 1;
const CONCATENATION: i32 = 2;
const CONV_2D: i32 = 3;
const DEPTHWISE_CONV_2D: i32 = 4;
const DEQUANTIZE: i32 = 6;
const FULLY_CONNECTED: i32 = 9;
const MAX_POOL_2D: i32 = 17;
const RESIZE_BILINEAR: i32 = 23;
const SOFTMAX: i32 = 25;
const PAD: i32 = 34;
const MEAN: i32 = 40;
const STRIDED_SLICE: i32 = 45;
const TRANSPOSE_CONV: i32 = 67;
const RESIZE_NEAREST_NEIGHBOR: i32 = 97;
const HARD_SWISH: i32 = 117;

// option tables
fn conv_opts(same: bool, sw: i32, sh: i32, act: u8) -> (u8, T) {
    (1, T::default().with(0, F::U8(if same { 0 } else { 1 })).with(1, F::I32(sw)).with(2, F::I32(sh)).with(3, F::U8(act)).with(4, F::I32(1)).with(5, F::I32(1)))
}
fn dw_opts(same: bool, s: i32, dm: i32, act: u8) -> (u8, T) {
    (2, T::default().with(0, F::U8(if same { 0 } else { 1 })).with(1, F::I32(s)).with(2, F::I32(s)).with(3, F::I32(dm)).with(4, F::U8(act)))
}
fn pool_opts(same: bool, s: i32, k: i32) -> (u8, T) {
    (5, T::default().with(0, F::U8(if same { 0 } else { 1 })).with(1, F::I32(s)).with(2, F::I32(s)).with(3, F::I32(k)).with(4, F::I32(k)))
}
fn act_opts(ty: u8, act: u8) -> (u8, T) {
    (ty, T::default().with(0, F::U8(act)))
}

// ------------------------------------------------------------------ references

struct Rng(u64);
impl Rng {
    fn f(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
    }
    fn v(&mut self, n: usize, scale: f32) -> Vec<f32> {
        (0..n).map(|_| self.f() * scale).collect()
    }
}

fn tf_pad(inp: usize, k: usize, s: usize, d: usize, same: bool) -> (usize, usize) {
    let eff = (k - 1) * d + 1;
    if same {
        let out = inp.div_ceil(s);
        let total = ((out - 1) * s + eff).saturating_sub(inp);
        (out, total / 2)
    } else {
        ((inp - eff) / s + 1, 0)
    }
}

/// NHWC conv; weights OHWI, or 1HWC with depth multiplier `dm`.
#[allow(clippy::too_many_arguments)]
fn conv_ref(x: &[f32], xs: [usize; 4], w: &[f32], ws: [usize; 4], b: Option<&[f32]>, s: usize, same: bool, dm: Option<usize>) -> (Vec<f32>, [usize; 4]) {
    let [n, h, wd, c] = xs;
    let (kh, kw) = (ws[1], ws[2]);
    let oc_n = if dm.is_some() { ws[3] } else { ws[0] };
    let (oh, pt) = tf_pad(h, kh, s, 1, same);
    let (ow, pl) = tf_pad(wd, kw, s, 1, same);
    let mut out = vec![0f32; n * oh * ow * oc_n];
    for bi in 0..n {
        for oy in 0..oh {
            for ox in 0..ow {
                for oc in 0..oc_n {
                    let mut acc = b.map(|b| b[oc]).unwrap_or(0.0);
                    for ky in 0..kh {
                        for kx in 0..kw {
                            let iy = (oy * s + ky) as isize - pt as isize;
                            let ix = (ox * s + kx) as isize - pl as isize;
                            if iy < 0 || ix < 0 || iy as usize >= h || ix as usize >= wd {
                                continue;
                            }
                            let xb = ((bi * h + iy as usize) * wd + ix as usize) * c;
                            match dm {
                                Some(m) => acc += x[xb + oc / m] * w[(ky * kw + kx) * ws[3] + oc],
                                None => {
                                    for ic in 0..ws[3] {
                                        acc += x[xb + ic] * w[((oc * kh + ky) * kw + kx) * ws[3] + ic];
                                    }
                                }
                            }
                        }
                    }
                    out[((bi * oh + oy) * ow + ox) * oc_n + oc] = acc;
                }
            }
        }
    }
    (out, [n, oh, ow, oc_n])
}

/// NHWC transposed conv with OHWI weights (TFLite semantics).
fn tconv_ref(x: &[f32], xs: [usize; 4], w: &[f32], ws: [usize; 4], b: &[f32], s: usize, out_hw: [usize; 2]) -> Vec<f32> {
    let [n, h, wd, c] = xs;
    let [oc_n, kh, kw, _] = ws;
    let [oh, ow] = out_hw;
    let pt = ((h - 1) * s + kh).saturating_sub(oh) / 2;
    let pl = ((wd - 1) * s + kw).saturating_sub(ow) / 2;
    let mut out = vec![0f32; n * oh * ow * oc_n];
    for bi in 0..n {
        for iy in 0..h {
            for ix in 0..wd {
                for ky in 0..kh {
                    for kx in 0..kw {
                        let oy = (iy * s + ky) as isize - pt as isize;
                        let ox = (ix * s + kx) as isize - pl as isize;
                        if oy < 0 || ox < 0 || oy as usize >= oh || ox as usize >= ow {
                            continue;
                        }
                        for oc in 0..oc_n {
                            for ic in 0..c {
                                out[((bi * oh + oy as usize) * ow + ox as usize) * oc_n + oc] += x[((bi * h + iy) * wd + ix) * c + ic] * w[((oc * kh + ky) * kw + kx) * c + ic];
                            }
                        }
                    }
                }
            }
        }
    }
    for (i, v) in out.iter_mut().enumerate() {
        *v += b[i % oc_n];
    }
    out
}

fn close(got: &Tensor, want: &[f32], shape: &[usize], tol: f32) {
    assert_eq!(got.shape, shape, "shape");
    let g = got.to_f32_vec();
    let worst = g.iter().zip(want).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    assert!(worst <= tol, "max abs diff {worst} > {tol}\n got {:?}\nwant {:?}", &g[..g.len().min(12)], &want[..want.len().min(12)]);
}

fn session(bytes: &[u8]) -> (Model, Session) {
    let m = Model::from_bytes(bytes).expect("import");
    assert!(m.unsupported_ops().is_empty(), "{}", m.report());
    let s = Session::new(&m, Options::default()).expect("session");
    (m, s)
}

fn run1(bytes: &[u8], input: Tensor) -> Tensor {
    let (_, s) = session(bytes);
    let name = s.input_names()[0].to_string();
    s.run(&[(name.as_str(), input)]).expect("run").remove(0).1
}

// ------------------------------------------------------------------ tests

fn conv_model(rng: &mut Rng) -> (Vec<u8>, Vec<f32>, Vec<f32>) {
    let mut m = Tfl::new();
    let x = m.input("input", &[1, 10, 9, 3]);
    let w1v = rng.v(4 * 3 * 3 * 3, 1.5);
    let b1v = rng.v(4, 0.5);
    let w2v = rng.v(3 * 3 * 8, 1.0);
    let b2v = rng.v(8, 0.5);
    let w3v = rng.v(8 * 4, 1.0);
    let w1 = m.cf32("conv1/w", &[4, 3, 3, 3], &w1v);
    let b1 = m.cf32("conv1/b", &[4], &b1v);
    let y1 = m.act("conv1/out", &[1, 5, 5, 4]);
    m.op(CONV_2D, &[x, w1, b1], &[y1], Some(conv_opts(true, 2, 2, 3)));
    let w2 = m.cf32("dw/w", &[1, 3, 3, 8], &w2v);
    let b2 = m.cf32("dw/b", &[8], &b2v);
    let y2 = m.act("dw/out", &[1, 5, 5, 8]);
    m.op(DEPTHWISE_CONV_2D, &[y1, w2, b2], &[y2], Some(dw_opts(true, 1, 2, 0)));
    let w3 = m.cf32("pw/w", &[8, 1, 1, 4], &w3v);
    let y3 = m.act("pw/out", &[1, 5, 5, 8]);
    m.op(CONV_2D, &[y1, w3, -1], &[y3], Some(conv_opts(false, 1, 1, 0)));
    let y4 = m.act("add/out", &[1, 5, 5, 8]);
    m.op(ADD, &[y2, y3], &[y4], Some(act_opts(11, 1)));
    let axes = m.ci32("mean/axes", &[2], &[1, 2]);
    let out = m.act("Identity", &[1, 8]);
    m.op(MEAN, &[y4, axes], &[out], Some((27, T::default().with(0, F::U8(0)))));
    let bytes = m.build(&[out]);

    let xv = rng.v(10 * 9 * 3, 2.0);
    let (r1, s1) = conv_ref(&xv, [1, 10, 9, 3], &w1v, [4, 3, 3, 3], Some(&b1v), 2, true, None);
    assert_eq!(s1, [1, 5, 5, 4]);
    let r1: Vec<f32> = r1.iter().map(|v| v.clamp(0.0, 6.0)).collect();
    let (r2, _) = conv_ref(&r1, s1, &w2v, [1, 3, 3, 8], Some(&b2v), 1, true, Some(2));
    let (r3, _) = conv_ref(&r1, s1, &w3v, [8, 1, 1, 4], None, 1, false, None);
    let r4: Vec<f32> = r2.iter().zip(&r3).map(|(a, b)| (a + b).max(0.0)).collect();
    let mut want = vec![0f32; 8];
    for (i, v) in r4.iter().enumerate() {
        want[i % 8] += v / 25.0;
    }
    (bytes, xv, want)
}

#[test]
fn conv_depthwise_add_mean() {
    let mut rng = Rng(1);
    let (bytes, xv, want) = conv_model(&mut rng);
    let (m, s) = session(&bytes);
    assert_eq!(s.input_names(), vec!["input"]);
    assert_eq!(m.opset_version(""), 17);
    let hist = m.op_histogram();
    assert!(hist.iter().any(|(k, n)| k == "Conv" && *n == 3), "{hist:?}");
    let out = s.run(&[("input", Tensor::f32(&[1, 10, 9, 3], xv))]).unwrap();
    assert_eq!(out[0].0, "Identity");
    close(&out[0].1, &want, &[1, 8], 1e-4);
}

fn fc_model(rng: &mut Rng) -> (Vec<u8>, Vec<f32>, Vec<f32>) {
    let mut m = Tfl::new();
    let x = m.input("features", &[2, 1, 6]);
    let wv = rng.v(5 * 6, 1.0);
    let bv = rng.v(5, 1.0);
    let w = m.cf32("fc/w", &[5, 6], &wv);
    let b = m.cf32("fc/b", &[5], &bv);
    let y = m.act("fc/out", &[2, 5]);
    m.op(FULLY_CONNECTED, &[x, w, b], &[y], Some(act_opts(8, 0)));
    let out = m.act("probs", &[2, 5]);
    m.op(SOFTMAX, &[y], &[out], Some((9, T::default().with(0, F::F32(0.5)))));
    let bytes = m.build(&[out]);
    let xv = rng.v(12, 2.0);
    let mut want = Vec::new();
    for r in 0..2 {
        let logits: Vec<f32> = (0..5).map(|o| 0.5 * (bv[o] + (0..6).map(|i| xv[r * 6 + i] * wv[o * 6 + i]).sum::<f32>())).collect();
        let mx = logits.iter().copied().fold(f32::MIN, f32::max);
        let e: Vec<f32> = logits.iter().map(|l| (l - mx).exp()).collect();
        let sum: f32 = e.iter().sum();
        want.extend(e.iter().map(|v| v / sum));
    }
    (bytes, xv, want)
}

#[test]
fn fully_connected_softmax() {
    let mut rng = Rng(2);
    let (bytes, xv, want) = fc_model(&mut rng);
    let got = run1(&bytes, Tensor::f32(&[2, 1, 6], xv));
    close(&got, &want, &[2, 5], 1e-5);
}

fn bilinear_ref(x: &[f32], [n, h, w, c]: [usize; 4], oh: usize, ow: usize, align: bool, half: bool) -> Vec<f32> {
    let scale = |i: usize, o: usize| if align && o > 1 { (i - 1) as f32 / (o - 1) as f32 } else { i as f32 / o as f32 };
    let (hs, ws) = (scale(h, oh), scale(w, ow));
    let coord = |v: usize, s: f32, len: usize| {
        let f = if half { (v as f32 + 0.5) * s - 0.5 } else { v as f32 * s };
        let lo = (f.floor() as i64).max(0) as usize;
        let hi = (f.ceil() as i64).min(len as i64 - 1) as usize;
        (f, lo, hi)
    };
    let mut out = Vec::new();
    for b in 0..n {
        for y in 0..oh {
            let (iy, y0, y1) = coord(y, hs, h);
            for xo in 0..ow {
                let (ix, x0, x1) = coord(xo, ws, w);
                for ch in 0..c {
                    let at = |yy: usize, xx: usize| x[((b * h + yy) * w + xx) * c + ch];
                    let (dy, dx) = (iy - y0 as f32, ix - x0 as f32);
                    out.push(at(y0, x0) * (1.0 - dy) * (1.0 - dx) + at(y1, x0) * dy * (1.0 - dx) + at(y0, x1) * (1.0 - dy) * dx + at(y1, x1) * dy * dx);
                }
            }
        }
    }
    out
}

fn nearest_ref(x: &[f32], [n, h, w, c]: [usize; 4], oh: usize, ow: usize, align: bool, half: bool) -> Vec<f32> {
    let pick = |v: usize, i: usize, o: usize| {
        let s = if align && o > 1 { (i - 1) as f32 / (o - 1) as f32 } else { i as f32 / o as f32 };
        let off = if half { 0.5 } else { 0.0 };
        let r = if align { ((v as f32 + off) * s).round() as i64 } else { ((v as f32 + off) * s).floor() as i64 };
        r.clamp(0, i as i64 - 1) as usize
    };
    let mut out = Vec::new();
    for b in 0..n {
        for y in 0..oh {
            for xo in 0..ow {
                for ch in 0..c {
                    out.push(x[((b * h + pick(y, h, oh)) * w + pick(xo, w, ow)) * c + ch]);
                }
            }
        }
    }
    out
}

#[test]
fn resize_bilinear_and_nearest_variants() {
    let mut rng = Rng(3);
    let xv = rng.v(3 * 4 * 2, 3.0);
    for (align, half) in [(false, false), (true, false), (false, true)] {
        for (code, oh, ow) in [(RESIZE_BILINEAR, 5, 7), (RESIZE_BILINEAR, 2, 3), (RESIZE_NEAREST_NEIGHBOR, 7, 5), (RESIZE_NEAREST_NEIGHBOR, 2, 9)] {
            let mut m = Tfl::new();
            let x = m.input("image", &[1, 3, 4, 2]);
            let size = m.ci32("size", &[2], &[oh as i32, ow as i32]);
            let out = m.act("resized", &[1, oh as i32, ow as i32, 2]);
            let opts = if code == RESIZE_BILINEAR {
                (15, T::default().with(2, F::U8(align as u8)).with(3, F::U8(half as u8)))
            } else {
                (74, T::default().with(0, F::U8(align as u8)).with(1, F::U8(half as u8)))
            };
            m.op(code, &[x, size], &[out], Some(opts));
            let got = run1(&m.build(&[out]), Tensor::f32(&[1, 3, 4, 2], xv.clone()));
            let want = if code == RESIZE_BILINEAR { bilinear_ref(&xv, [1, 3, 4, 2], oh, ow, align, half) } else { nearest_ref(&xv, [1, 3, 4, 2], oh, ow, align, half) };
            close(&got, &want, &[1, oh, ow, 2], 1e-5);
        }
    }
}

#[test]
fn transpose_conv_and_mediapipe_transpose_bias() {
    let mut rng = Rng(4);
    let xv = rng.v(3 * 3 * 2, 1.0);
    // builtin TRANSPOSE_CONV, 3x3 kernel, SAME stride 2: 3 -> 6 (pads 0/1)
    let wv = rng.v(3 * 3 * 3 * 2, 1.0);
    let bv = rng.v(3, 1.0);
    let mut m = Tfl::new();
    let x = m.input("x", &[1, 3, 3, 2]);
    let os = m.ci32("output_shape", &[4], &[1, 6, 6, 3]);
    let w = m.cf32("tconv/w", &[3, 3, 3, 2], &wv);
    let b = m.cf32("tconv/b", &[3], &bv);
    let out = m.act("y", &[1, 6, 6, 3]);
    m.op(TRANSPOSE_CONV, &[os, w, x, b], &[out], Some((49, T::default().with(0, F::U8(0)).with(1, F::I32(2)).with(2, F::I32(2)))));
    let got = run1(&m.build(&[out]), Tensor::f32(&[1, 3, 3, 2], xv.clone()));
    close(&got, &tconv_ref(&xv, [1, 3, 3, 2], &wv, [3, 3, 3, 2], &bv, 2, [6, 6]), &[1, 6, 6, 3], 1e-5);

    // MediaPipe Convolution2DTransposeBias, 4x4 kernel, stride 2: 3 -> 6 (pads 1/1)
    let wv = rng.v(3 * 4 * 4 * 2, 1.0);
    let mut m = Tfl::new();
    let x = m.input("x", &[1, 3, 3, 2]);
    let w = m.cf32("tcb/w", &[3, 4, 4, 2], &wv);
    let b = m.cf32("tcb/b", &[3], &bv);
    let out = m.act("y", &[1, 6, 6, 3]);
    m.custom("Convolution2DTransposeBias", &[x, w, b], &[out], &[1, 2, 2]);
    let got = run1(&m.build(&[out]), Tensor::f32(&[1, 3, 3, 2], xv.clone()));
    close(&got, &tconv_ref(&xv, [1, 3, 3, 2], &wv, [3, 4, 4, 2], &bv, 2, [6, 6]), &[1, 6, 6, 3], 1e-5);
}

fn fp16_model(rng: &mut Rng) -> (Vec<u8>, Vec<f32>, Vec<f32>) {
    let wv = rng.v(3 * 2 * 2 * 2, 1.0);
    let bv = rng.v(3, 1.0);
    let mut m = Tfl::new();
    let x = m.input("x", &[1, 4, 4, 2]);
    let wh = m.cf16("w_fp16", &[3, 2, 2, 2], &wv);
    let w = m.act("w_fp32", &[3, 2, 2, 2]);
    m.op(DEQUANTIZE, &[wh], &[w], None);
    let bh = m.cf16("b_fp16", &[3], &bv);
    let b = m.act("b_fp32", &[3]);
    m.op(DEQUANTIZE, &[bh], &[b], None);
    let out = m.act("y", &[1, 3, 3, 3]);
    m.op(CONV_2D, &[x, w, b], &[out], Some(conv_opts(false, 1, 1, 0)));
    let bytes = m.build(&[out]);
    let xv = rng.v(32, 1.0);
    let wr: Vec<f32> = wv.iter().map(|&v| f16_to_f32(f32_to_f16(v))).collect();
    let br: Vec<f32> = bv.iter().map(|&v| f16_to_f32(f32_to_f16(v))).collect();
    let (want, _) = conv_ref(&xv, [1, 4, 4, 2], &wr, [3, 2, 2, 2], Some(&br), 1, false, None);
    (bytes, xv, want)
}

#[test]
fn fp16_dequantize_weights_are_folded() {
    let mut rng = Rng(5);
    let (bytes, xv, want) = fp16_model(&mut rng);
    let (m, s) = session(&bytes);
    let hist = m.op_histogram();
    assert!(!hist.iter().any(|(k, _)| k == "Cast" || k.contains("DEQUANTIZE")), "{hist:?}");
    assert!(m.graph.initializers.iter().all(|(_, t)| t.dtype() != studio_nn::DType::F16));
    let got = s.run(&[("x", Tensor::f32(&[1, 4, 4, 2], xv))]).unwrap().remove(0).1;
    close(&got, &want, &[1, 3, 3, 3], 1e-5);
}

#[test]
fn pad_pool_slice_hardswish_concat() {
    let mut rng = Rng(6);
    let xv = rng.v(4 * 4 * 2, 3.0);
    let mut m = Tfl::new();
    let x = m.input("x", &[1, 4, 4, 2]);
    let pads = m.ci32("pads", &[4, 2], &[0, 0, 1, 1, 0, 2, 0, 0]);
    let p = m.act("padded", &[1, 6, 6, 2]);
    m.op(PAD, &[x, pads], &[p], None);
    let mp = m.act("maxpool", &[1, 3, 3, 2]);
    m.op(MAX_POOL_2D, &[p], &[mp], Some(pool_opts(true, 2, 2)));
    let ap = m.act("avgpool", &[1, 3, 3, 2]);
    m.op(AVERAGE_POOL_2D, &[mp], &[ap], Some(pool_opts(true, 1, 3)));
    let begin = m.ci32("begin", &[4], &[0, 0, 1, 0]);
    let end = m.ci32("end", &[4], &[1, 3, 3, 2]);
    let strides = m.ci32("strides", &[4], &[1, 1, 2, 1]);
    let sl = m.act("sliced", &[3, 1, 2]);
    m.op(STRIDED_SLICE, &[ap, begin, end, strides], &[sl], Some((32, T::default().with(4, F::I32(1)))));
    let hs = m.act("hswish", &[3, 1, 2]);
    m.op(HARD_SWISH, &[sl], &[hs], None);
    let out = m.act("out", &[3, 1, 4]);
    m.op(CONCATENATION, &[hs, sl], &[out], Some((10, T::default().with(0, F::I32(-1)))));
    let got = run1(&m.build(&[out]), Tensor::f32(&[1, 4, 4, 2], xv.clone()));

    // reference
    let (h, w) = (6usize, 6usize);
    let at = |y: usize, x: usize, c: usize| -> f32 {
        if (1..5).contains(&y) && x < 4 { xv[((y - 1) * 4 + x) * 2 + c] } else { 0.0 }
    };
    let mut mpv = vec![0f32; 3 * 3 * 2];
    for oy in 0..3 {
        for ox in 0..3 {
            for c in 0..2 {
                let mut v = f32::MIN;
                for ky in 0..2 {
                    for kx in 0..2 {
                        let (iy, ix) = (oy * 2 + ky, ox * 2 + kx);
                        if iy < h && ix < w {
                            v = v.max(at(iy, ix, c));
                        }
                    }
                }
                mpv[(oy * 3 + ox) * 2 + c] = v;
            }
        }
    }
    let mut apv = vec![0f32; 18];
    for oy in 0..3i32 {
        for ox in 0..3i32 {
            for c in 0..2 {
                let (mut s, mut n) = (0.0, 0.0);
                for iy in oy - 1..=oy + 1 {
                    for ix in ox - 1..=ox + 1 {
                        if (0..3).contains(&iy) && (0..3).contains(&ix) {
                            s += mpv[((iy * 3 + ix) * 2) as usize + c];
                            n += 1.0;
                        }
                    }
                }
                apv[((oy * 3 + ox) * 2) as usize + c] = s / n;
            }
        }
    }
    // slice: batch 0 (shrunk), rows 0..3, col 1, channels 0..2
    let slv: Vec<f32> = (0..3).flat_map(|y| (0..2).map(move |c| (y, c))).map(|(y, c)| apv[(y * 3 + 1) * 2 + c]).collect();
    let mut want = Vec::new();
    for y in 0..3 {
        let s = &slv[y * 2..y * 2 + 2];
        want.extend(s.iter().map(|&v| v * (v / 6.0 + 0.5).clamp(0.0, 1.0)));
        want.extend_from_slice(s);
    }
    close(&got, &want, &[3, 1, 4], 1e-5);
}

#[test]
fn mediapipe_argmax_pool_unpool_roundtrip() {
    let mut rng = Rng(7);
    let xv = rng.v(4 * 4 * 2, 1.0);
    let mut m = Tfl::new();
    let x = m.input("x", &[1, 4, 4, 2]);
    let v = m.act("pooled", &[1, 2, 2, 2]);
    let i = m.act("indices", &[1, 2, 2, 2]);
    // TfLitePoolParams: padding VALID(2), stride_w, stride_h, filter_w, filter_h, activation
    m.custom("MaxPoolingWithArgmax2D", &[x], &[v, i], &[2, 2, 2, 2, 2, 0]);
    let out = m.act("unpooled", &[1, 4, 4, 2]);
    m.custom("MaxUnpooling2D", &[v, i], &[out], &[2, 2, 2, 2, 2, 0]);
    let (model, s) = session(&m.build(&[out]));
    assert!(model.opset_import.iter().any(|o| o.domain == "mediapipe"));
    let got = s.run(&[("x", Tensor::f32(&[1, 4, 4, 2], xv.clone()))]).unwrap().remove(0).1;
    let mut want = vec![0f32; 32];
    for oy in 0..2 {
        for ox in 0..2 {
            for c in 0..2 {
                let mut best = (f32::MIN, 0);
                for ky in 0..2 {
                    for kx in 0..2 {
                        let k = ((oy * 2 + ky) * 4 + ox * 2 + kx) * 2 + c;
                        if xv[k] > best.0 {
                            best = (xv[k], k);
                        }
                    }
                }
                want[best.1] = best.0;
            }
        }
    }
    close(&got, &want, &[1, 4, 4, 2], 0.0);
}

#[test]
fn quantised_models_are_rejected() {
    let mut m = Tfl::new();
    let x = m.tensor("q", &[1, 4], 3, None);
    // attach quantization {scale: [0.1], zero_point: [0]} by hand
    let scale = T::default().with(2, F::I32s(vec![0.1f32.to_bits() as i32]));
    m.tensors[x as usize] = m.tensors[x as usize].clone().with(4, F::Table(scale));
    m.inputs.push(x);
    let y = m.act("y", &[1, 4]);
    m.op(DEQUANTIZE, &[x], &[y], None);
    let err = Model::from_bytes(&m.build(&[y])).unwrap_err();
    assert!(err.0.contains("quantised"), "{err}");
}

#[test]
fn unknown_ops_are_reported() {
    let mut m = Tfl::new();
    let x = m.input("x", &[1, 4]);
    let y = m.act("y", &[1, 4]);
    m.custom("SomeVendorOp", &[x], &[y], &[]);
    let model = Model::from_bytes(&m.build(&[y])).unwrap();
    assert_eq!(model.unsupported_ops(), vec!["tflite::SomeVendorOp".to_string()]);
    assert!(Session::new(&model, Options::default()).is_err());
}

// ------------------------------------------------------------------ .task bundles

fn stored_deflate(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let chunks: Vec<&[u8]> = if data.is_empty() { vec![&[][..]] } else { data.chunks(65535).collect() };
    for (i, c) in chunks.iter().enumerate() {
        out.push((i + 1 == chunks.len()) as u8);
        out.extend_from_slice(&(c.len() as u16).to_le_bytes());
        out.extend_from_slice(&(!(c.len() as u16)).to_le_bytes());
        out.extend_from_slice(c);
    }
    out
}

fn zip(members: &[(&str, &[u8], bool)]) -> Vec<u8> {
    let (mut z, mut cd) = (Vec::new(), Vec::new());
    for (name, data, deflate) in members {
        let comp = if *deflate { stored_deflate(data) } else { data.to_vec() };
        let crc = studio_nn::zip::crc32(data);
        let method: u16 = if *deflate { 8 } else { 0 };
        let mut common = Vec::new();
        common.extend_from_slice(&method.to_le_bytes());
        common.extend_from_slice(&[0; 4]);
        common.extend_from_slice(&crc.to_le_bytes());
        common.extend_from_slice(&(comp.len() as u32).to_le_bytes());
        common.extend_from_slice(&(data.len() as u32).to_le_bytes());
        common.extend_from_slice(&(name.len() as u16).to_le_bytes());
        let off = z.len() as u32;
        z.extend_from_slice(&[0x50, 0x4b, 3, 4, 20, 0, 0, 0]);
        z.extend_from_slice(&common);
        z.extend_from_slice(&[0, 0]);
        z.extend_from_slice(name.as_bytes());
        z.extend_from_slice(&comp);
        cd.extend_from_slice(&[0x50, 0x4b, 1, 2, 20, 0, 20, 0, 0, 0]);
        cd.extend_from_slice(&common);
        cd.extend_from_slice(&[0; 12]);
        cd.extend_from_slice(&off.to_le_bytes());
        cd.extend_from_slice(name.as_bytes());
    }
    let cd_off = z.len() as u32;
    z.extend_from_slice(&cd);
    z.extend_from_slice(&[0x50, 0x4b, 5, 6, 0, 0, 0, 0]);
    z.extend_from_slice(&(members.len() as u16).to_le_bytes());
    z.extend_from_slice(&(members.len() as u16).to_le_bytes());
    z.extend_from_slice(&(cd.len() as u32).to_le_bytes());
    z.extend_from_slice(&cd_off.to_le_bytes());
    z.extend_from_slice(&[0, 0]);
    z
}

#[test]
fn task_bundle_loads_every_tflite_member() {
    let mut rng = Rng(8);
    let (fc, fc_x, fc_want) = fc_model(&mut rng);
    let (conv, conv_x, conv_want) = conv_model(&mut rng);
    assert!(conv.len() > fc.len());
    let task = zip(&[("hand_detector.tflite", &fc, false), ("metadata.json", b"{}", false), ("hand_landmarks_detector.tflite", &conv, true)]);
    let models = studio_nn::tflite::task_from_bytes(task.clone()).unwrap();
    let names: Vec<&str> = models.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, vec!["hand_landmarks_detector.tflite", "hand_detector.tflite"]);
    let s = Session::new(&models[0].1, Options::default()).unwrap();
    close(&s.run(&[("input", Tensor::f32(&[1, 10, 9, 3], conv_x))]).unwrap()[0].1, &conv_want, &[1, 8], 1e-4);
    let s = Session::new(&models[1].1, Options::default()).unwrap();
    close(&s.run(&[("features", Tensor::f32(&[2, 1, 6], fc_x))]).unwrap()[0].1, &fc_want, &[2, 5], 1e-5);

    // via the filesystem: load_task and Model::load (largest member)
    let path = std::env::temp_dir().join(format!("studio-nn-tflite-test-{}.task", std::process::id()));
    std::fs::write(&path, &task).unwrap();
    let from_file = studio_nn::tflite::load_task(&path).unwrap();
    let first = Model::load(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(from_file.len(), 2);
    assert_eq!(first, models[0].1);
}

// ------------------------------------------------------------------ real models (optional)

#[test]
#[ignore = "needs ~/Movies/Studio/Models/mediapipe"]
fn real_mediapipe_models() {
    let dir = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("Movies/Studio/Models/mediapipe");
    if !dir.exists() {
        eprintln!("{} not present; skipping", dir.display());
        return;
    }
    let mut files: Vec<_> = std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).collect();
    files.sort();
    for p in files {
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        let models = match ext.as_str() {
            "tflite" => vec![(p.file_name().unwrap().to_string_lossy().into_owned(), studio_nn::tflite::load(&p))],
            "task" => match studio_nn::tflite::load_task(&p) {
                Ok(v) => v.into_iter().map(|(n, m)| (n, Ok(m))).collect(),
                Err(e) => vec![(p.display().to_string(), Err(e))],
            },
            _ => continue,
        };
        for (name, m) in models {
            println!("=== {} :: {name}", p.display());
            let m = match m {
                Ok(m) => m,
                Err(e) => {
                    println!("import error: {e}");
                    continue;
                }
            };
            print!("{}", m.report());
            let s = match Session::new(&m, Options::default()) {
                Ok(s) => s,
                Err(e) => {
                    println!("session error: {e}");
                    continue;
                }
            };
            let inputs: Vec<(String, Tensor)> = m
                .inputs()
                .iter()
                .map(|v| {
                    let shape: Vec<usize> = v.dims().unwrap_or_default().iter().map(|d| d.unwrap_or(1)).collect();
                    (v.name.clone(), Tensor::zeros(v.dtype().unwrap_or(studio_nn::DType::F32), &shape))
                })
                .collect();
            let refs: Vec<(&str, Tensor)> = inputs.iter().map(|(n, t)| (n.as_str(), t.clone())).collect();
            match s.run(&refs) {
                Ok(outs) => {
                    for (n, t) in outs {
                        let v = t.to_f32_vec();
                        let finite = v.iter().all(|x| x.is_finite());
                        println!("  out {n} {:?} finite={finite} first={:?}", t.shape, &v[..v.len().min(4)]);
                    }
                }
                Err(e) => println!("run error: {e}"),
            }
        }
    }
}
