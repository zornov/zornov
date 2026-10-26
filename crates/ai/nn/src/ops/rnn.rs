// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Recurrent operators: LSTM, GRU and RNN (forward / reverse / bidirectional,
//! sequence_lens, initial states, peepholes, clip, layout, custom activations).

use super::elementwise::sigmoid;
use super::{OpCtx, inp, opt};
use crate::error::{Error, Result};
use crate::kernels::gemm::sgemm;
use crate::onnx::Node;
use crate::tensor::Tensor;

type Act = Box<dyn Fn(f32) -> f32>;

fn activation(name: &str, alpha: Option<f32>, beta: Option<f32>) -> Result<Act> {
    Ok(match name.to_ascii_lowercase().as_str() {
        "sigmoid" => Box::new(sigmoid),
        "tanh" => Box::new(|x: f32| x.tanh()),
        "relu" => Box::new(|x: f32| x.max(0.0)),
        "hardsigmoid" => {
            let (a, b) = (alpha.unwrap_or(0.2), beta.unwrap_or(0.5));
            Box::new(move |x| (a * x + b).clamp(0.0, 1.0))
        }
        "leakyrelu" => {
            let a = alpha.unwrap_or(0.01);
            Box::new(move |x| if x >= 0.0 { x } else { a * x })
        }
        "thresholdedrelu" => {
            let a = alpha.unwrap_or(1.0);
            Box::new(move |x| if x > a { x } else { 0.0 })
        }
        "scaledtanh" => {
            let (a, b) = (alpha.unwrap_or(1.0), beta.unwrap_or(1.0));
            Box::new(move |x| a * (b * x).tanh())
        }
        "affine" => {
            let (a, b) = (alpha.unwrap_or(1.0), beta.unwrap_or(0.0));
            Box::new(move |x| a * x + b)
        }
        "elu" => {
            let a = alpha.unwrap_or(1.0);
            Box::new(move |x| if x >= 0.0 { x } else { a * (x.exp() - 1.0) })
        }
        "softsign" => Box::new(|x: f32| x / (1.0 + x.abs())),
        "softplus" => Box::new(super::elementwise::softplus),
        n => return Err(Error(format!("RNN activation {n} not supported"))),
    })
}

fn needs_params(name: &str) -> (bool, bool) {
    match name.to_ascii_lowercase().as_str() {
        "hardsigmoid" | "scaledtanh" | "affine" => (true, true),
        "leakyrelu" | "thresholdedrelu" | "elu" => (true, false),
        _ => (false, false),
    }
}

fn activations(node: &Node, per_dir: usize, dirs: usize, defaults: &[&str]) -> Result<Vec<Act>> {
    let names = node.attr_strings("activations").unwrap_or_default();
    let alphas = node.attr_floats("activation_alpha").unwrap_or_default();
    let betas = node.attr_floats("activation_beta").unwrap_or_default();
    let (mut ai, mut bi) = (0, 0);
    let mut out = Vec::new();
    for i in 0..per_dir * dirs {
        let name = names.get(i).map(|s| s.as_str()).unwrap_or(defaults[i % per_dir]);
        let (na, nb) = needs_params(name);
        let a = if na && ai < alphas.len() {
            ai += 1;
            Some(alphas[ai - 1])
        } else {
            None
        };
        let b = if nb && bi < betas.len() {
            bi += 1;
            Some(betas[bi - 1])
        } else {
            None
        };
        out.push(activation(name, a, b)?);
    }
    Ok(out)
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Lstm,
    Gru,
    Rnn,
}

pub fn rnn_op(ctx: &OpCtx, node: &Node, inputs: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
    let kind = match node.op_type.as_str() {
        "LSTM" => Kind::Lstm,
        "GRU" => Kind::Gru,
        _ => Kind::Rnn,
    };
    let gates = match kind {
        Kind::Lstm => 4,
        Kind::Gru => 3,
        Kind::Rnn => 1,
    };
    let x = inp(inputs, 0)?;
    let w = inp(inputs, 1)?.to_f32_vec();
    let r = inp(inputs, 2)?.to_f32_vec();
    let bias = opt(inputs, 3).map(|t| t.to_f32_vec());
    let seq_lens = opt(inputs, 4).map(|t| t.to_i64_vec());
    let init_h = opt(inputs, 5).map(|t| t.to_f32_vec());
    let init_c = if kind == Kind::Lstm { opt(inputs, 6).map(|t| t.to_f32_vec()) } else { None };
    let peep = if kind == Kind::Lstm { opt(inputs, 7).map(|t| t.to_f32_vec()) } else { None };
    let layout = node.attr_i("layout", 0);
    let dir = node.attr_s_or("direction", "forward");
    let dirs = if dir == "bidirectional" { 2 } else { 1 };
    let hs = node.attr_i("hidden_size", 0) as usize;
    let hs = if hs == 0 { inp(inputs, 2)?.shape[2] } else { hs };
    let clip = node.attr_f("clip", 0.0);
    let input_forget = node.attr_i("input_forget", 0) != 0;
    let lbr = node.attr_i("linear_before_reset", 0) != 0;
    if x.rank() != 3 {
        return Err(Error(format!("{}: X must be 3-D", node.op_type)));
    }
    let (seq, batch, isz) = if layout == 0 { (x.shape[0], x.shape[1], x.shape[2]) } else { (x.shape[1], x.shape[0], x.shape[2]) };
    // time-major copy of X: [seq, batch, isz]
    let xv = x.f32s();
    let xt: Vec<f32> = if layout == 0 {
        xv.into_owned()
    } else {
        let mut v = vec![0.0; seq * batch * isz];
        for b in 0..batch {
            for t in 0..seq {
                v[(t * batch + b) * isz..][..isz].copy_from_slice(&xv[(b * seq + t) * isz..][..isz]);
            }
        }
        v
    };
    let per_dir = match kind {
        Kind::Lstm => 3,
        Kind::Gru => 2,
        Kind::Rnn => 1,
    };
    let defaults: &[&str] = match kind {
        Kind::Lstm => &["Sigmoid", "Tanh", "Tanh"],
        Kind::Gru => &["Sigmoid", "Tanh"],
        Kind::Rnn => &["Tanh"],
    };
    let acts = activations(node, per_dir, dirs, defaults)?;
    let gh = gates * hs;
    let mut y = vec![0.0f32; seq * dirs * batch * hs];
    let mut yh = vec![0.0f32; dirs * batch * hs];
    let mut yc = vec![0.0f32; dirs * batch * hs];
    let lens: Vec<usize> = (0..batch).map(|b| seq_lens.as_ref().map(|l| l[b].clamp(0, seq as i64) as usize).unwrap_or(seq)).collect();
    let cl = |v: f32| if clip > 0.0 { v.clamp(-clip, clip) } else { v };
    for d in 0..dirs {
        let reverse = dir == "reverse" || d == 1;
        let wd = &w[d * gh * isz..(d + 1) * gh * isz];
        let rd = &r[d * gh * hs..(d + 1) * gh * hs];
        let zero = vec![0.0f32; 2 * gh];
        let bd = bias.as_ref().map(|b| &b[d * 2 * gh..(d + 1) * 2 * gh]).unwrap_or(&zero);
        let (wb, rb) = (&bd[..gh], &bd[gh..]);
        // XW: [seq*batch, gh]
        let mut xw = vec![0.0f32; seq * batch * gh];
        sgemm(seq * batch, gh, isz, &xt, isz, 1, wd, 1, isz, &mut xw, gh, false, ctx.threads);
        let fa = &acts[d * per_dir];
        let ga = acts.get(d * per_dir + 1);
        let ha = acts.get(d * per_dir + 2);
        let mut h: Vec<f32> = init_h.as_ref().map(|v| v[d * batch * hs..(d + 1) * batch * hs].to_vec()).unwrap_or_else(|| vec![0.0; batch * hs]);
        let mut c: Vec<f32> = init_c.as_ref().map(|v| v[d * batch * hs..(d + 1) * batch * hs].to_vec()).unwrap_or_else(|| vec![0.0; batch * hs]);
        let pd = peep.as_ref().map(|p| &p[d * 3 * hs..(d + 1) * 3 * hs]);
        let mut hr = vec![0.0f32; batch * gh];
        for s in 0..seq {
            // H_{t-1} · R^T for all gates
            sgemm(batch, gh, hs, &h, hs, 1, rd, 1, hs, &mut hr, gh, false, 1);
            for b in 0..batch {
                if s >= lens[b] {
                    continue;
                }
                let t = if reverse { lens[b] - 1 - s } else { s };
                let xg = &xw[(t * batch + b) * gh..(t * batch + b + 1) * gh];
                let hg = &hr[b * gh..(b + 1) * gh];
                let hb = &mut h[b * hs..(b + 1) * hs];
                match kind {
                    Kind::Lstm => {
                        let cb = &mut c[b * hs..(b + 1) * hs];
                        let (ga, ha) = (ga.unwrap(), ha.unwrap());
                        for j in 0..hs {
                            let pre = |g: usize| xg[g * hs + j] + hg[g * hs + j] + wb[g * hs + j] + rb[g * hs + j];
                            let (pi, po, pf) = pd.map(|p| (p[j], p[hs + j], p[2 * hs + j])).unwrap_or((0.0, 0.0, 0.0));
                            let cprev = cb[j];
                            let it = fa(cl(pre(0) + pi * cprev));
                            let ft = if input_forget { 1.0 - it } else { fa(cl(pre(2) + pf * cprev)) };
                            let ct = ga(cl(pre(3)));
                            let cn = ft * cprev + it * ct;
                            let ot = fa(cl(pre(1) + po * cn));
                            cb[j] = cn;
                            hb[j] = ot * ha(cn);
                        }
                    }
                    Kind::Gru => {
                        let ga = ga.unwrap();
                        let mut zs = vec![0.0f32; hs];
                        let mut rs = vec![0.0f32; hs];
                        for j in 0..hs {
                            zs[j] = fa(cl(xg[j] + hg[j] + wb[j] + rb[j]));
                            rs[j] = fa(cl(xg[hs + j] + hg[hs + j] + wb[hs + j] + rb[hs + j]));
                        }
                        let hnew: Vec<f32> = if lbr {
                            (0..hs).map(|j| ga(cl(xg[2 * hs + j] + wb[2 * hs + j] + rs[j] * (hg[2 * hs + j] + rb[2 * hs + j])))).collect()
                        } else {
                            let rh: Vec<f32> = (0..hs).map(|j| rs[j] * hb[j]).collect();
                            let rh_w = &rd[2 * hs * hs..3 * hs * hs];
                            (0..hs)
                                .map(|j| {
                                    let dotv = crate::kernels::gemm::dot(&rh, &rh_w[j * hs..(j + 1) * hs]);
                                    ga(cl(xg[2 * hs + j] + dotv + rb[2 * hs + j] + wb[2 * hs + j]))
                                })
                                .collect()
                        };
                        for j in 0..hs {
                            hb[j] = (1.0 - zs[j]) * hnew[j] + zs[j] * hb[j];
                        }
                    }
                    Kind::Rnn => {
                        for j in 0..hs {
                            hb[j] = fa(cl(xg[j] + hg[j] + wb[j] + rb[j]));
                        }
                    }
                }
                y[((t * dirs + d) * batch + b) * hs..][..hs].copy_from_slice(hb);
            }
        }
        yh[d * batch * hs..(d + 1) * batch * hs].copy_from_slice(&h);
        yc[d * batch * hs..(d + 1) * batch * hs].copy_from_slice(&c);
    }
    if layout == 0 {
        let mut out = vec![Tensor::f32(&[seq, dirs, batch, hs], y), Tensor::f32(&[dirs, batch, hs], yh)];
        if kind == Kind::Lstm {
            out.push(Tensor::f32(&[dirs, batch, hs], yc));
        }
        return Ok(out);
    }
    // layout 1: Y [batch, seq, dirs, hs], Y_h [batch, dirs, hs]
    let mut y1 = vec![0.0f32; y.len()];
    for t in 0..seq {
        for d in 0..dirs {
            for b in 0..batch {
                y1[((b * seq + t) * dirs + d) * hs..][..hs].copy_from_slice(&y[((t * dirs + d) * batch + b) * hs..][..hs]);
            }
        }
    }
    let tr = |v: &[f32]| {
        let mut o = vec![0.0f32; v.len()];
        for d in 0..dirs {
            for b in 0..batch {
                o[(b * dirs + d) * hs..][..hs].copy_from_slice(&v[(d * batch + b) * hs..][..hs]);
            }
        }
        o
    };
    let mut out = vec![Tensor::f32(&[batch, seq, dirs, hs], y1), Tensor::f32(&[batch, dirs, hs], tr(&yh))];
    if kind == Kind::Lstm {
        out.push(Tensor::f32(&[batch, dirs, hs], tr(&yc)));
    }
    Ok(out)
}
