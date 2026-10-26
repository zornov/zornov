// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Compiled execution plans.
//!
//! `Session::new` turns a [`Model`] into a flat schedule:
//! 1. `Constant` nodes and initializers become constants;
//! 2. nodes are topologically sorted (Kahn) and pruned to what the outputs need;
//! 3. static shape inference runs interleaved with constant folding (ops whose
//!    inputs are all constant, plus `Shape`/`Size` of statically shaped values);
//! 4. Conv → BatchNormalization pairs are folded into the conv weights, and
//!    back-to-back Transposes that cancel are removed (TFLite NHWC wrappers);
//! 5. MatMul/Gemm/Conv weights are pre-packed for the GEMM kernel;
//! 6. each value gets a slot and a last-use step, so intermediate tensors are
//!    dropped as soon as nothing reads them and reshape-like ops reuse their
//!    input buffer when it dies at that op.
//!
//! `run` executes the schedule; every op first goes to the accelerator backend
//! (if any) and falls back to the CPU kernel when the backend declines.

use crate::backend::{self, Backend, BackendKind};
use crate::error::{Error, Result};
use crate::kernels::par::resolve_threads;
use crate::model::Model;
use crate::onnx::{AttrValue, Graph, Node};
use crate::ops::{self, OpCtx, OpFn, Prepacked};
use crate::shape::{Shape, infer_node, to_static};
use crate::tensor::{DType, Tensor};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::Instant;

#[derive(Clone, Debug)]
pub struct Options {
    pub backend: BackendKind,
    /// Worker threads for GEMM/conv and large element-wise ops (0 = all cores).
    pub threads: usize,
    pub constant_folding: bool,
    pub fold_batchnorm: bool,
    /// Smallest op (in FLOPs) sent to the accelerator backend.
    pub metal_min_flops: f64,
    /// Collect per-op-type timings (see [`Session::profile`]).
    pub profile: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { backend: BackendKind::Cpu, threads: 0, constant_folding: true, fold_batchnorm: true, metal_min_flops: 4.0e6, profile: false }
    }
}

impl Options {
    pub fn cpu(threads: usize) -> Self {
        Options { threads, ..Default::default() }
    }
    pub fn with_backend(mut self, b: BackendKind) -> Self {
        self.backend = b;
        self
    }
}

enum Exec {
    Kernel(OpFn),
    If { outer: Vec<String> },
}

struct Step {
    node: Node,
    exec: Exec,
    inputs: Vec<Option<usize>>,
    outputs: Vec<Option<usize>>,
    free_after: Vec<usize>,
    cache: Option<Prepacked>,
    opset: i64,
    alias: bool,
}

/// What the optimiser did.
#[derive(Clone, Debug, Default)]
pub struct CompileStats {
    pub nodes_in: usize,
    pub nodes_scheduled: usize,
    pub folded: usize,
    pub batchnorms_folded: usize,
    pub transposes_removed: usize,
    pub prepacked: usize,
}

pub struct Session {
    steps: Vec<Step>,
    consts: Vec<Option<Tensor>>,
    names: Vec<String>,
    inputs: Vec<(String, usize, Option<DType>, Option<Vec<Option<usize>>>)>,
    outputs: Vec<(String, usize, Option<DType>)>,
    threads: usize,
    accel: Option<Box<dyn Backend>>,
    profile_on: bool,
    profile: Mutex<HashMap<String, (f64, usize)>>,
    pub stats: CompileStats,
}

/// Names a subgraph reads from enclosing scopes.
pub fn free_vars(g: &Graph) -> Vec<String> {
    let mut defined: HashSet<&str> = g.inputs.iter().map(|v| v.name.as_str()).collect();
    defined.extend(g.initializers.iter().map(|(n, _)| n.as_str()));
    for n in &g.nodes {
        defined.extend(n.outputs.iter().map(|s| s.as_str()));
    }
    let mut out: Vec<String> = Vec::new();
    for n in &g.nodes {
        for i in &n.inputs {
            if !i.is_empty() && !defined.contains(i.as_str()) && !out.contains(i) {
                out.push(i.clone());
            }
        }
        for sg in subgraphs(n) {
            for v in free_vars(sg) {
                if !defined.contains(v.as_str()) && !out.contains(&v) {
                    out.push(v);
                }
            }
        }
    }
    out
}

fn subgraphs(n: &Node) -> Vec<&Graph> {
    let mut v = Vec::new();
    for a in &n.attrs {
        match &a.value {
            AttrValue::Graph(g) => v.push(g.as_ref()),
            AttrValue::Graphs(gs) => v.extend(gs.iter()),
            _ => {}
        }
    }
    v
}

/// All value names a node depends on (inputs plus subgraph free variables).
fn node_deps(n: &Node) -> Vec<String> {
    let mut d: Vec<String> = n.inputs.iter().filter(|s| !s.is_empty()).cloned().collect();
    for sg in subgraphs(n) {
        for v in free_vars(sg) {
            if !d.contains(&v) {
                d.push(v);
            }
        }
    }
    d
}

/// Kahn topological order of `nodes` (indices).
fn topo_order(nodes: &[Node]) -> Result<Vec<usize>> {
    let mut producer: HashMap<&str, usize> = HashMap::new();
    for (i, n) in nodes.iter().enumerate() {
        for o in &n.outputs {
            if !o.is_empty() {
                producer.insert(o.as_str(), i);
            }
        }
    }
    let deps: Vec<Vec<usize>> = nodes.iter().map(|n| node_deps(n).iter().filter_map(|d| producer.get(d.as_str()).copied()).collect()).collect();
    let mut indeg = vec![0usize; nodes.len()];
    let mut users: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    for (i, ds) in deps.iter().enumerate() {
        for &d in ds {
            indeg[i] += 1;
            users[d].push(i);
        }
    }
    // keep the file order among ready nodes
    let mut ready: std::collections::BTreeSet<usize> = (0..nodes.len()).filter(|&i| indeg[i] == 0).collect();
    let mut order = Vec::with_capacity(nodes.len());
    while let Some(&i) = ready.iter().next() {
        ready.remove(&i);
        order.push(i);
        for &u in &users[i] {
            indeg[u] -= 1;
            if indeg[u] == 0 {
                ready.insert(u);
            }
        }
    }
    if order.len() != nodes.len() {
        return Err(Error("graph has a cycle".into()));
    }
    Ok(order)
}

fn eval_node(node: &Node, ins: &[Option<&Tensor>], opset: i64, threads: usize) -> Result<Vec<Tensor>> {
    let f = ops::lookup(&node.domain, &node.op_type).ok_or_else(|| Error(format!("unsupported op {}", node.op_type)))?;
    f(&OpCtx { opset, threads, cache: None }, node, ins)
}

fn with_ctx<T>(r: Result<T>, node: &Node) -> Result<T> {
    r.map_err(|e| Error(format!("{} '{}': {}", node.op_type, node.name, e.0)))
}

/// Runs a subgraph (If branches) with a simple interpreter. `outer` resolves
/// names from enclosing scopes.
fn run_graph_scoped<'a>(g: &Graph, outer: &dyn Fn(&str) -> Option<&'a Tensor>, opset: i64, threads: usize) -> Result<Vec<Tensor>> {
    let mut locals: HashMap<String, Tensor> = g.initializers.iter().cloned().collect();
    for n in &g.nodes {
        let outs = {
            let ins: Vec<Option<&Tensor>> = n.inputs.iter().map(|i| if i.is_empty() { None } else { locals.get(i.as_str()).or_else(|| outer(i)) }).collect();
            if n.op_type == "If" && ops::normalize_domain(&n.domain).is_empty() {
                let cond = ins.first().copied().flatten().ok_or_else(|| Error("If: missing condition".into()))?;
                let branch = if cond.to_bool_vec().first().copied().unwrap_or(false) { "then_branch" } else { "else_branch" };
                let Some(AttrValue::Graph(sg)) = n.attr(branch) else { return Err(Error(format!("If: no {branch}"))) };
                let nested = |name: &str| locals.get(name).or_else(|| outer(name));
                run_graph_scoped(sg, &nested, opset, threads)?
            } else {
                with_ctx(eval_node(n, &ins, opset, threads), n)?
            }
        };
        for (name, t) in n.outputs.iter().zip(outs) {
            if !name.is_empty() {
                locals.insert(name.clone(), t);
            }
        }
    }
    g.outputs.iter().map(|o| locals.remove(&o.name).or_else(|| outer(&o.name).cloned()).ok_or_else(|| Error(format!("subgraph output {} not computed", o.name)))).collect()
}

fn perm_of(n: &Node, rank_hint: Option<usize>) -> Option<Vec<usize>> {
    match n.attr_ints("perm") {
        Some(p) => Some(p.iter().map(|&v| v as usize).collect()),
        None => rank_hint.map(|r| (0..r).rev().collect()),
    }
}

impl Session {
    pub fn new(model: &Model, opts: Options) -> Result<Session> {
        let threads = resolve_threads(opts.threads);
        let g = &model.graph;
        let opset = model.opset_version("");
        let opset_of = |n: &Node| model.opset_version(&n.domain);
        let mut stats = CompileStats { nodes_in: g.nodes.len(), ..Default::default() };

        // unsupported ops fail early with the full list
        let un = model.unsupported_ops();
        if !un.is_empty() {
            return Err(Error(format!("model uses unsupported ops: {}", un.join(", "))));
        }

        let mut consts: HashMap<String, Tensor> = g.initializers.iter().cloned().collect();
        let graph_inputs: Vec<_> = g.inputs.iter().filter(|v| !consts.contains_key(&v.name)).cloned().collect();
        let output_names: HashSet<String> = g.outputs.iter().map(|o| o.name.clone()).collect();
        let mut nodes: Vec<Node> = Vec::with_capacity(g.nodes.len());
        for n in &g.nodes {
            if n.op_type == "Constant" && ops::normalize_domain(&n.domain).is_empty() {
                let t = with_ctx(ops::shape_ops::constant(&OpCtx { opset, threads, cache: None }, n, &[]), n)?;
                consts.insert(n.outputs[0].clone(), t.into_iter().next().unwrap());
            } else {
                nodes.push(n.clone());
            }
        }

        // topological order + pruning
        let order = topo_order(&nodes)?;
        let mut needed: HashSet<String> = output_names.clone();
        let mut live = vec![false; nodes.len()];
        for &i in order.iter().rev() {
            if nodes[i].outputs.iter().any(|o| needed.contains(o)) {
                live[i] = true;
                needed.extend(node_deps(&nodes[i]));
            }
        }
        let mut order: Vec<usize> = order.into_iter().filter(|&i| live[i]).collect();

        // shape inference + constant folding
        let mut shapes: HashMap<String, Shape> = HashMap::new();
        for v in &graph_inputs {
            if let Some(d) = v.dims() {
                shapes.insert(v.name.clone(), d);
            }
        }
        let mut removed = vec![false; nodes.len()];
        for &i in &order {
            let n = &nodes[i];
            let deps = node_deps(n);
            let is_if = n.op_type == "If";
            if opts.constant_folding && !is_if && deps.iter().all(|d| consts.contains_key(d)) {
                let ins: Vec<Option<&Tensor>> = n.inputs.iter().map(|s| if s.is_empty() { None } else { consts.get(s) }).collect();
                let outs = with_ctx(eval_node(n, &ins, opset_of(n), threads), n)?;
                for (name, t) in n.outputs.iter().zip(outs) {
                    if !name.is_empty() {
                        shapes.insert(name.clone(), t.shape.iter().map(|&d| Some(d)).collect());
                        consts.insert(name.clone(), t);
                    }
                }
                removed[i] = true;
                stats.folded += 1;
                continue;
            }
            if opts.constant_folding && (n.op_type == "Shape" || n.op_type == "Size") && ops::normalize_domain(&n.domain).is_empty() {
                if let Some(st) = n.input(0).and_then(|x| shapes.get(x)).and_then(to_static) {
                    let fake = Tensor::zeros(DType::Bool, &st);
                    let outs = with_ctx(eval_node(n, &[Some(&fake)], opset_of(n), threads), n)?;
                    let t = outs.into_iter().next().unwrap();
                    shapes.insert(n.outputs[0].clone(), t.shape.iter().map(|&d| Some(d)).collect());
                    consts.insert(n.outputs[0].clone(), t);
                    removed[i] = true;
                    stats.folded += 1;
                    continue;
                }
            }
            let ins: Vec<Option<Shape>> = n
                .inputs
                .iter()
                .map(|s| if s.is_empty() { None } else { shapes.get(s).cloned().or_else(|| consts.get(s).map(|t| t.shape.iter().map(|&d| Some(d)).collect())) })
                .collect();
            let cs: Vec<Option<&Tensor>> = n.inputs.iter().map(|s| consts.get(s)).collect();
            for (name, s) in n.outputs.iter().zip(infer_node(n, &ins, &cs, opset_of(n))) {
                if let Some(s) = s
                    && !name.is_empty()
                {
                    shapes.insert(name.clone(), s);
                }
            }
        }
        order.retain(|&i| !removed[i]);

        // consumer counts
        let consumers = |nodes: &[Node], order: &[usize]| {
            let mut c: HashMap<String, usize> = HashMap::new();
            for &i in order {
                for d in node_deps(&nodes[i]) {
                    *c.entry(d).or_default() += 1;
                }
            }
            c
        };

        // Conv + BatchNormalization folding
        if opts.fold_batchnorm {
            let cons = consumers(&nodes, &order);
            let producer: HashMap<String, usize> = order.iter().flat_map(|&i| nodes[i].outputs.iter().map(move |o| (o.clone(), i))).collect();
            let mut drop_nodes = HashSet::new();
            for &bi in &order {
                let bn = &nodes[bi];
                if bn.op_type != "BatchNormalization" || !ops::normalize_domain(&bn.domain).is_empty() || bn.outputs.iter().skip(1).any(|o| !o.is_empty()) {
                    continue;
                }
                let Some(x) = bn.input(0) else { continue };
                let Some(&ci) = producer.get(x) else { continue };
                let conv = &nodes[ci];
                if conv.op_type != "Conv" || cons.get(x).copied().unwrap_or(0) != 1 || output_names.contains(x) {
                    continue;
                }
                let params: Option<Vec<Vec<f32>>> = (1..5).map(|k| bn.input(k).and_then(|n| consts.get(n)).map(|t| t.to_f32_vec())).collect();
                let Some(p) = params else { continue };
                let Some(w) = conv.input(1).and_then(|n| consts.get(n)) else { continue };
                if !w.dtype().is_float() {
                    continue;
                }
                let bias = match conv.input(2) {
                    Some(b) => match consts.get(b) {
                        Some(t) => Some(t.to_f32_vec()),
                        None => continue,
                    },
                    None => None,
                };
                let oc = w.shape[0];
                if p.iter().any(|v| v.len() != oc) {
                    continue;
                }
                let eps = bn.attr_f("epsilon", 1e-5);
                let per = w.len() / oc;
                let mut wv = w.to_f32_vec();
                let mut bv = bias.unwrap_or_else(|| vec![0.0; oc]);
                for c in 0..oc {
                    let s = p[0][c] / (p[3][c] + eps).sqrt();
                    wv[c * per..(c + 1) * per].iter_mut().for_each(|v| *v *= s);
                    bv[c] = (bv[c] - p[2][c]) * s + p[1][c];
                }
                let wname = format!("{}__bnfold_w{bi}", conv.name);
                let bname = format!("{}__bnfold_b{bi}", conv.name);
                consts.insert(wname.clone(), Tensor::f32(&w.shape, wv));
                consts.insert(bname.clone(), Tensor::f32(&[oc], bv));
                let out = bn.outputs[0].clone();
                let conv = &mut nodes[ci];
                conv.inputs.truncate(1);
                conv.inputs.push(wname);
                conv.inputs.push(bname);
                conv.outputs = vec![out];
                drop_nodes.insert(bi);
                stats.batchnorms_folded += 1;
            }
            order.retain(|i| !drop_nodes.contains(i));
        }

        // cancel Transpose pairs
        loop {
            let cons = consumers(&nodes, &order);
            let producer: HashMap<String, usize> = order.iter().flat_map(|&i| nodes[i].outputs.iter().map(move |o| (o.clone(), i))).collect();
            let mut found = None;
            for &t2 in &order {
                let n2 = &nodes[t2];
                if n2.op_type != "Transpose" || output_names.contains(&n2.outputs[0]) {
                    continue;
                }
                let Some(y) = n2.input(0) else { continue };
                let Some(&t1) = producer.get(y) else { continue };
                let n1 = &nodes[t1];
                if n1.op_type != "Transpose" || cons.get(y).copied().unwrap_or(0) != 1 || output_names.contains(y) {
                    continue;
                }
                let r = shapes.get(y).map(|s| s.len());
                let (Some(p1), Some(p2)) = (perm_of(n1, r), perm_of(n2, r)) else { continue };
                if p1.len() == p2.len() && (0..p2.len()).all(|j| p1[p2[j]] == j) {
                    found = Some((t1, t2, n1.inputs[0].clone(), n2.outputs[0].clone()));
                    break;
                }
            }
            let Some((t1, t2, x, z)) = found else { break };
            for &i in &order {
                for inp in nodes[i].inputs.iter_mut() {
                    if *inp == z {
                        *inp = x.clone();
                    }
                }
            }
            order.retain(|&i| i != t1 && i != t2);
            stats.transposes_removed += 2;
        }

        // value ids
        let mut ids: HashMap<String, usize> = HashMap::new();
        let mut names: Vec<String> = Vec::new();
        let id_of = |name: &str, ids: &mut HashMap<String, usize>, names: &mut Vec<String>| -> usize {
            if let Some(&i) = ids.get(name) {
                return i;
            }
            ids.insert(name.to_string(), names.len());
            names.push(name.to_string());
            names.len() - 1
        };
        for v in &graph_inputs {
            id_of(&v.name, &mut ids, &mut names);
        }
        let mut steps: Vec<Step> = Vec::with_capacity(order.len());
        for &i in &order {
            let n = &nodes[i];
            let (exec, deps): (Exec, Vec<Option<String>>) = if n.op_type == "If" && ops::normalize_domain(&n.domain).is_empty() {
                // every name the branches read from enclosing scopes (this may
                // include the If's own condition, e.g. for a nested If)
                let mut outer: Vec<String> = Vec::new();
                for v in subgraphs(n).into_iter().flat_map(free_vars) {
                    if !outer.contains(&v) {
                        outer.push(v);
                    }
                }
                let mut d = vec![n.input(0).map(|s| s.to_string())];
                d.extend(outer.iter().map(|s| Some(s.clone())));
                (Exec::If { outer }, d)
            } else {
                let f = ops::lookup(&n.domain, &n.op_type).ok_or_else(|| Error(format!("unsupported op {}", n.op_type)))?;
                (Exec::Kernel(f), n.inputs.iter().map(|s| if s.is_empty() { None } else { Some(s.clone()) }).collect())
            };
            let inputs: Vec<Option<usize>> = deps.iter().map(|d| d.as_ref().map(|s| id_of(s, &mut ids, &mut names))).collect();
            let outputs: Vec<Option<usize>> = n.outputs.iter().map(|o| if o.is_empty() { None } else { Some(id_of(o, &mut ids, &mut names)) }).collect();
            let cache = if matches!(exec, Exec::Kernel(_)) {
                let ins: Vec<Option<&Tensor>> = n.inputs.iter().map(|s| consts.get(s)).collect();
                ops::prepack(n, &ins, threads)
            } else {
                None
            };
            if cache.is_some() {
                stats.prepacked += 1;
            }
            steps.push(Step { alias: ops::is_alias_op(&n.domain, &n.op_type), node: n.clone(), exec, inputs, outputs, free_after: Vec::new(), cache, opset: opset_of(n) });
        }
        for o in &g.outputs {
            id_of(&o.name, &mut ids, &mut names);
        }
        let mut const_slots: Vec<Option<Tensor>> = vec![None; names.len()];
        for (name, &id) in &ids {
            if let Some(t) = consts.remove(name) {
                const_slots[id] = Some(t);
            }
        }
        // liveness
        let out_ids: HashSet<usize> = g.outputs.iter().map(|o| ids[&o.name]).collect();
        let mut last_use: HashMap<usize, usize> = HashMap::new();
        for (s, st) in steps.iter().enumerate() {
            for &i in st.inputs.iter().flatten() {
                last_use.insert(i, s);
            }
            for &o in st.outputs.iter().flatten() {
                last_use.entry(o).or_insert(s);
            }
        }
        for (id, s) in last_use {
            if const_slots[id].is_none() && !out_ids.contains(&id) {
                steps[s].free_after.push(id);
            }
        }
        stats.nodes_scheduled = steps.len();

        let inputs = graph_inputs.iter().map(|v| (v.name.clone(), ids[&v.name], v.dtype(), v.dims())).collect();
        let outputs = g.outputs.iter().map(|o| (o.name.clone(), ids[&o.name], o.dtype())).collect();
        let accel = backend::accelerator(opts.backend, opts.metal_min_flops)?;
        Ok(Session { steps, consts: const_slots, names, inputs, outputs, threads, accel, profile_on: opts.profile, profile: Mutex::new(HashMap::new()), stats })
    }

    pub fn input_names(&self) -> Vec<&str> {
        self.inputs.iter().map(|i| i.0.as_str()).collect()
    }
    pub fn output_names(&self) -> Vec<&str> {
        self.outputs.iter().map(|o| o.0.as_str()).collect()
    }
    /// Declared input shape (`None` = dynamic dim).
    pub fn input_shape(&self, name: &str) -> Option<Vec<Option<usize>>> {
        self.inputs.iter().find(|i| i.0 == name).and_then(|i| i.3.clone())
    }
    pub fn backend_name(&self) -> &'static str {
        self.accel.as_ref().map(|a| a.name()).unwrap_or("cpu")
    }

    /// Runs the model. Inputs are given by name.
    pub fn run(&self, inputs: &[(&str, Tensor)]) -> Result<Vec<(String, Tensor)>> {
        self.run_owned(inputs.iter().map(|(n, t)| (n.to_string(), t.clone())).collect())
    }

    /// Like [`Session::run`] but takes ownership (no input copy).
    pub fn run_owned(&self, inputs: Vec<(String, Tensor)>) -> Result<Vec<(String, Tensor)>> {
        let mut vals: Vec<Option<Tensor>> = vec![None; self.names.len()];
        for (name, t) in inputs {
            let Some((_, id, _, dims)) = self.inputs.iter().find(|i| i.0 == name) else {
                return Err(Error(format!("unknown input '{name}' (inputs: {})", self.input_names().join(", "))));
            };
            if let Some(d) = dims
                && (d.len() != t.rank() || d.iter().zip(&t.shape).any(|(a, b)| a.is_some_and(|a| a != *b)))
            {
                return Err(Error(format!("input '{name}': shape {:?} does not match declared {:?}", t.shape, d)));
            }
            vals[*id] = Some(t);
        }
        for st in &self.steps {
            let t0 = if self.profile_on { Some(Instant::now()) } else { None };
            let take = st.alias
                && st.inputs.first().copied().flatten().is_some_and(|id| self.consts[id].is_none() && st.free_after.contains(&id) && vals[id].is_some());
            let outs: Vec<Tensor> = if take {
                let shape = {
                    let ins = self.gather(st, &vals)?;
                    with_ctx(ops::shape_ops::alias_shape(&OpCtx { opset: st.opset, threads: self.threads, cache: None }, &st.node, &ins), &st.node)?
                };
                let id0 = st.inputs[0].unwrap();
                let mut t = vals[id0].take().unwrap();
                t.shape = shape;
                vec![t]
            } else {
                let ins = self.gather(st, &vals)?;
                with_ctx(self.exec(st, &ins), &st.node)?
            };
            let mut it = outs.into_iter();
            for o in &st.outputs {
                let t = it.next();
                if let Some(id) = o {
                    match t {
                        Some(t) => vals[*id] = Some(t),
                        None => return Err(Error(format!("{} '{}' produced too few outputs", st.node.op_type, st.node.name))),
                    }
                }
            }
            for &id in &st.free_after {
                vals[id] = None;
            }
            if let Some(t0) = t0
                && let Ok(mut p) = self.profile.lock()
            {
                let e = p.entry(st.node.op_type.clone()).or_default();
                e.0 += t0.elapsed().as_secs_f64();
                e.1 += 1;
            }
        }
        let mut res = Vec::with_capacity(self.outputs.len());
        for (name, id, dt) in &self.outputs {
            let t = match vals[*id].take() {
                Some(t) => t,
                None => self.consts[*id].clone().ok_or_else(|| Error(format!("output '{name}' was not computed")))?,
            };
            let t = match dt {
                Some(DType::F16) if t.dtype() == DType::F32 => t.cast(DType::F16),
                _ => t,
            };
            res.push((name.clone(), t));
        }
        Ok(res)
    }

    fn gather<'a>(&'a self, st: &Step, vals: &'a [Option<Tensor>]) -> Result<Vec<Option<&'a Tensor>>> {
        st.inputs
            .iter()
            .map(|o| match o {
                None => Ok(None),
                Some(id) => match self.consts[*id].as_ref().or(vals[*id].as_ref()) {
                    Some(t) => Ok(Some(t)),
                    None => Err(Error(format!("{} '{}': input '{}' has no value (missing graph input?)", st.node.op_type, st.node.name, self.names[*id]))),
                },
            })
            .collect()
    }

    fn exec(&self, st: &Step, ins: &[Option<&Tensor>]) -> Result<Vec<Tensor>> {
        match &st.exec {
            Exec::Kernel(f) => {
                let ctx = OpCtx { opset: st.opset, threads: self.threads, cache: st.cache.as_ref() };
                if let Some(a) = &self.accel
                    && let Some(r) = a.try_run(&st.node, ins, &ctx)
                {
                    return r;
                }
                f(&ctx, &st.node, ins)
            }
            Exec::If { outer } => {
                let cond = ins[0].ok_or_else(|| Error("If: missing condition".into()))?;
                let branch = if cond.to_bool_vec().first().copied().unwrap_or(false) { "then_branch" } else { "else_branch" };
                let Some(AttrValue::Graph(g)) = st.node.attr(branch) else { return Err(Error(format!("If: no {branch}"))) };
                let map: HashMap<&str, &Tensor> = outer.iter().zip(&ins[1..]).filter_map(|(n, t)| t.map(|t| (n.as_str(), t))).collect();
                let lookup = |n: &str| map.get(n).copied();
                run_graph_scoped(g, &lookup, st.opset, self.threads)
            }
        }
    }

    /// Accumulated `(op_type, seconds, calls)` when `Options::profile` is set, slowest first.
    pub fn profile(&self) -> Vec<(String, f64, usize)> {
        let mut v: Vec<(String, f64, usize)> = self.profile.lock().map(|p| p.iter().map(|(k, (t, n))| (k.clone(), *t, *n)).collect()).unwrap_or_default();
        v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        v
    }
}
