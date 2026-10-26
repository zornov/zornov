// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The node compositor: a shot built as a graph of operations,
//! for work the layer stack cannot express (keys with core and
//! edge mattes, clean plates, set extensions, depth-of-field from depth).
//!
//! A [`Graph`] is a list of [`Node`]s; each names a kind from the
//! catalogue ([`nodes::all`]), its inputs (other nodes), parameters (with
//! optional keyframes) and free-form data (roto shapes, paint strokes,
//! tracks, file paths). Nodes pass [`Plate`]s: an RGBA picture plus the
//! optional depth, motion-vector and object-id channels.
//!
//! [`evaluate`] runs the graph for one frame on the CPU. Results are cached
//! per node by content: a node is recomputed only when its kind,
//! parameters at this time, data or upstream results change, so a still
//! background plate behind an animated key is computed once. The renderer
//! runs the per-pixel nodes on the GPU with the WGSL in [`NodeDef::wgsl`]
//! and falls back to this code for the rest.
//!
//! Every node also has a `mix` (0 keeps its main input) and an optional
//! `mask` node whose alpha limits where it applies.

use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::image::{Image, lerp_px};
use crate::params::{ParamDef, ParamKind, ParamSet, ParamValue};

pub mod clean;
pub mod geo;
pub mod gpu;
pub mod key;
pub mod lens;
pub mod nodes;
pub mod roto;

pub use nodes::{NodeCategory, NodeDef};

/// A node's id inside its graph.
pub type NodeId = u32;

/// What flows along the graph's edges: a picture and its extra channels,
/// all the same size.
#[derive(Clone, Debug, PartialEq)]
pub struct Plate {
    /// Linear premultiplied RGBA.
    pub rgba: Image,
    /// Distance from the camera, scene units (larger is further).
    pub depth: Option<Vec<f32>>,
    /// Motion since the previous frame, pixels (where the pixel moved by).
    pub motion: Option<Vec<[f32; 2]>>,
    /// Object id per pixel (0 is none).
    pub id: Option<Vec<f32>>,
}

impl Plate {
    pub fn new(rgba: Image) -> Plate {
        Plate { rgba, depth: None, motion: None, id: None }
    }

    pub fn width(&self) -> usize {
        self.rgba.width
    }

    pub fn height(&self) -> usize {
        self.rgba.height
    }

    /// Same channels as `self` with a new picture.
    pub fn with_rgba(&self, rgba: Image) -> Plate {
        Plate { rgba, depth: self.depth.clone(), motion: self.motion.clone(), id: self.id.clone() }
    }

    /// Alpha as its own plane.
    pub fn alpha(&self) -> Vec<f32> {
        self.rgba.pixels.iter().map(|p| p[3]).collect()
    }

    fn bytes(&self) -> usize {
        self.rgba.pixels.len() * 16 + self.depth.as_ref().map_or(0, |d| d.len() * 4) + self.motion.as_ref().map_or(0, |d| d.len() * 8) + self.id.as_ref().map_or(0, |d| d.len() * 4)
    }

    /// Resampled to `w`×`h` (bilinear, edges clamped), every channel.
    pub fn resized(&self, w: usize, h: usize) -> Plate {
        if (w, h) == (self.width(), self.height()) {
            return self.clone();
        }
        let (sw, sh) = (self.width() as f32 / w.max(1) as f32, self.height() as f32 / h.max(1) as f32);
        let rgba = Image::from_fn(w, h, |x, y| self.rgba.sample((x as f32 + 0.5) * sw, (y as f32 + 0.5) * sh, crate::image::Edge::Clamp));
        let plane = |p: &Vec<f32>| geo::resample_plane(p, self.width(), self.height(), w, h);
        let motion = self.motion.as_ref().map(|m| {
            let mx = geo::resample_plane(&m.iter().map(|v| v[0] / sw).collect::<Vec<f32>>(), self.width(), self.height(), w, h);
            let my = geo::resample_plane(&m.iter().map(|v| v[1] / sh).collect::<Vec<f32>>(), self.width(), self.height(), w, h);
            mx.into_iter().zip(my).map(|(a, b)| [a, b]).collect()
        });
        Plate { rgba, depth: self.depth.as_ref().map(plane), motion, id: self.id.as_ref().map(|p| geo::nearest_plane(p, self.width(), self.height(), w, h)) }
    }
}

/// One keyframe of a node parameter.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Key {
    /// Seconds from the start of the clip.
    pub t: f64,
    /// The value, written as for [`Node::params`].
    pub v: Json,
    /// Holds this value until the next key instead of easing to it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hold: bool,
}

/// One operation in the graph.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    /// Catalogue id ([`nodes::find`]).
    pub kind: String,
    /// What the user called it; empty shows the kind's name.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// Upstream nodes by input slot (see [`NodeDef::inputs`]).
    #[serde(default)]
    pub inputs: Vec<Option<NodeId>>,
    /// Parameter values by id: numbers, `true`/`false`, option names,
    /// `[x, y]` points (0..1 across the frame), colours as `"#rrggbb"` or
    /// `[r, g, b, a]` in linear light.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, Json>,
    /// Keyframes by parameter id; they override `params`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub keys: BTreeMap<String, Vec<Key>>,
    /// Shapes, strokes, tracks, paths: whatever the kind documents.
    #[serde(default, skip_serializing_if = "Json::is_null")]
    pub data: Json,
    /// 0 passes the main input through, 1 is the full result.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub mix: f32,
    /// A node whose alpha limits where this one applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<NodeId>,
    /// Passes the main input through unchanged.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub disabled: bool,
    /// Where the node sits in the graph view.
    #[serde(default)]
    pub pos: [f32; 2],
}

fn one() -> f32 {
    1.0
}

fn is_one(v: &f32) -> bool {
    *v == 1.0
}

impl Node {
    pub fn new(id: NodeId, kind: &str) -> Node {
        Node { id, kind: kind.to_owned(), name: String::new(), inputs: Vec::new(), params: BTreeMap::new(), keys: BTreeMap::new(), data: Json::Null, mix: 1.0, mask: None, disabled: false, pos: [0.0, 0.0] }
    }

    pub fn def(&self) -> Option<&'static NodeDef> {
        nodes::find(&self.kind)
    }

    /// The name shown in the graph: the user's, or the kind's with its id.
    pub fn label(&self) -> String {
        if !self.name.is_empty() {
            return self.name.clone();
        }
        format!("{}{}", self.def().map_or(self.kind.as_str(), |d| d.name), self.id)
    }

    pub fn input(&self, slot: usize) -> Option<NodeId> {
        self.inputs.get(slot).copied().flatten()
    }

    /// The parameter list this node reads: the catalogue's, or for an
    /// `effect` node the wrapped effect's.
    pub fn param_defs(&self) -> &'static [ParamDef] {
        if self.kind == "effect" {
            if let Some(e) = self.effect_def() {
                return e.params;
            }
        }
        self.def().map_or(&[], |d| d.params)
    }

    /// The effect an `effect` node wraps.
    pub fn effect_def(&self) -> Option<&'static crate::EffectDef> {
        self.data.get("effect").and_then(Json::as_str).and_then(crate::effects::find)
    }

    /// The parameter values at `t` seconds, keyframes applied.
    pub fn params_at(&self, t: f64) -> ParamSet {
        let defs = self.param_defs();
        let mut set = ParamSet::defaults(defs);
        for d in defs {
            let v = match self.keys.get(d.id).filter(|k| !k.is_empty()) {
                Some(keys) => keyed(keys, t, d),
                None => self.params.get(d.id).and_then(|j| value_of(j, d)),
            };
            if let Some(v) = v {
                let _ = set.set(d.id, v);
            }
        }
        set
    }

    /// Whether anything about the node changes with time by itself.
    fn animated(&self) -> bool {
        self.keys.values().any(|k| k.len() > 1)
    }
}

/// Reads a parameter value written in JSON for parameter `d`.
pub fn value_of(j: &Json, d: &ParamDef) -> Option<ParamValue> {
    match (&d.kind, j) {
        (ParamKind::Float { .. }, Json::Number(n)) => Some(ParamValue::Float(n.as_f64()? as f32)),
        (ParamKind::Int { .. }, Json::Number(n)) => Some(ParamValue::Int(n.as_f64()?.round() as i32)),
        (ParamKind::Bool, Json::Bool(b)) => Some(ParamValue::Bool(*b)),
        (ParamKind::Bool, Json::Number(n)) => Some(ParamValue::Bool(n.as_f64()? != 0.0)),
        (ParamKind::Choice(names), Json::String(s)) => names.iter().position(|n| n.eq_ignore_ascii_case(s) || n.replace(' ', "_").eq_ignore_ascii_case(s)).map(|i| ParamValue::Choice(i as u32)),
        (ParamKind::Choice(_), Json::Number(n)) => Some(ParamValue::Choice(n.as_u64()? as u32)),
        (ParamKind::Point, Json::Array(a)) if a.len() == 2 => Some(ParamValue::Point([a[0].as_f64()? as f32, a[1].as_f64()? as f32])),
        (ParamKind::Colour, Json::String(s)) => hex_colour(s).map(ParamValue::Colour),
        (ParamKind::Colour, Json::Array(a)) if a.len() >= 3 => {
            let c = |i: usize| a.get(i).and_then(Json::as_f64).map(|v| v as f32);
            Some(ParamValue::Colour([c(0)?, c(1)?, c(2)?, c(3).unwrap_or(1.0)]))
        }
        _ => None,
    }
}

/// A parameter value as JSON, the inverse of [`value_of`].
pub fn json_of(v: ParamValue, d: &ParamDef) -> Json {
    match v {
        ParamValue::Float(f) => serde_json::json!((f as f64 * 1e5).round() / 1e5),
        ParamValue::Int(i) => serde_json::json!(i),
        ParamValue::Bool(b) => serde_json::json!(b),
        ParamValue::Choice(c) => match d.kind {
            ParamKind::Choice(names) => serde_json::json!(names.get(c as usize).copied().unwrap_or("")),
            _ => serde_json::json!(c),
        },
        ParamValue::Point(p) => serde_json::json!([p[0], p[1]]),
        ParamValue::Colour(c) => serde_json::json!([c[0], c[1], c[2], c[3]]),
    }
}

/// `"#rrggbb"` or `"#rrggbbaa"` (sRGB) as a linear colour.
pub fn hex_colour(s: &str) -> Option<[f32; 4]> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 && h.len() != 8 {
        return None;
    }
    let b = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok().map(|v| v as f32 / 255.0);
    let a = if h.len() == 8 { b(6)? } else { 1.0 };
    Some([crate::image::srgb_to_linear(b(0)?), crate::image::srgb_to_linear(b(2)?), crate::image::srgb_to_linear(b(4)?), a])
}

fn keyed(keys: &[Key], t: f64, d: &ParamDef) -> Option<ParamValue> {
    let after = keys.iter().position(|k| k.t > t);
    let (a, b) = match after {
        Some(0) => return value_of(&keys[0].v, d),
        None => return value_of(&keys[keys.len() - 1].v, d),
        Some(i) => (&keys[i - 1], &keys[i]),
    };
    let va = value_of(&a.v, d)?;
    if a.hold {
        return Some(va);
    }
    let vb = value_of(&b.v, d)?;
    let u = ((t - a.t) / (b.t - a.t).max(1e-9)).clamp(0.0, 1.0) as f32;
    // Eased in and out, as layer keyframes are by default.
    let s = u * u * (3.0 - 2.0 * u);
    let l = |x: f32, y: f32| x + (y - x) * s;
    Some(match (va, vb) {
        (ParamValue::Float(x), ParamValue::Float(y)) => ParamValue::Float(l(x, y)),
        (ParamValue::Int(x), ParamValue::Int(y)) => ParamValue::Int(l(x as f32, y as f32).round() as i32),
        (ParamValue::Point(x), ParamValue::Point(y)) => ParamValue::Point([l(x[0], y[0]), l(x[1], y[1])]),
        (ParamValue::Colour(x), ParamValue::Colour(y)) => ParamValue::Colour(std::array::from_fn(|i| l(x[i], y[i]))),
        (v, _) => v,
    })
}

/// Where the `input` node's picture comes from at other times, so nodes
/// that look at neighbouring frames (motion vectors, clean plates) can see
/// them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InputMedia {
    /// The clip's media file.
    pub path: String,
    /// Source time (seconds) shown at the start of the clip.
    #[serde(default)]
    pub start: f64,
    /// Source seconds per clip second.
    #[serde(default = "one64")]
    pub speed: f64,
}

fn one64() -> f64 {
    1.0
}

/// A compositing graph.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct Graph {
    #[serde(default)]
    pub nodes: Vec<Node>,
    /// The node whose result is the graph's picture; the last node when
    /// unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<NodeId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_media: Option<InputMedia>,
}

impl Graph {
    /// A graph that passes its input through: one `input` node.
    pub fn passthrough() -> Graph {
        let mut g = Graph::default();
        g.add("input", &[]);
        g
    }

    pub fn from_json(s: &str) -> Result<Graph, String> {
        serde_json::from_str(s).map_err(|e| format!("not a compositing graph: {e}"))
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    pub fn node_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }

    /// Finds a node by id number, name or label (case-insensitive).
    pub fn find(&self, name: &str) -> Option<&Node> {
        let n = name.trim();
        if let Ok(id) = n.parse::<NodeId>() {
            if let Some(node) = self.node(id) {
                return Some(node);
            }
        }
        if let Some(x) = self.nodes.iter().find(|x| x.name.eq_ignore_ascii_case(n) || x.label().eq_ignore_ascii_case(n)) {
            return Some(x);
        }
        // A kind that only one node has ("input").
        let mut of_kind = self.nodes.iter().filter(|x| x.kind.eq_ignore_ascii_case(n));
        match (of_kind.next(), of_kind.next()) {
            (Some(x), None) => Some(x),
            _ => None,
        }
    }

    pub fn next_id(&self) -> NodeId {
        self.nodes.iter().map(|n| n.id).max().map_or(1, |m| m + 1)
    }

    /// Adds a node of `kind` fed by `inputs`, below the lowest node, and
    /// makes it the output. Returns its id.
    pub fn add(&mut self, kind: &str, inputs: &[Option<NodeId>]) -> NodeId {
        let id = self.next_id();
        let mut n = Node::new(id, kind);
        n.inputs = inputs.to_vec();
        let below = self.nodes.iter().map(|n| n.pos[1]).fold(f32::NEG_INFINITY, f32::max);
        let x = inputs.first().copied().flatten().and_then(|i| self.node(i)).map_or(0.0, |n| n.pos[0]);
        n.pos = [x, if below.is_finite() { below + 70.0 } else { 0.0 }];
        self.nodes.push(n);
        self.output = Some(id);
        id
    }

    /// Removes a node, reconnecting whatever read it to its main input.
    pub fn remove(&mut self, id: NodeId) -> bool {
        let Some(i) = self.nodes.iter().position(|n| n.id == id) else { return false };
        let main = self.nodes[i].input(0);
        self.nodes.remove(i);
        for n in &mut self.nodes {
            for s in n.inputs.iter_mut() {
                if *s == Some(id) {
                    *s = main;
                }
            }
            if n.mask == Some(id) {
                n.mask = None;
            }
        }
        if self.output == Some(id) {
            self.output = main.or(self.nodes.last().map(|n| n.id));
        }
        true
    }

    /// The node that makes the picture.
    pub fn output_id(&self) -> Option<NodeId> {
        self.output.filter(|o| self.node(*o).is_some()).or(self.nodes.last().map(|n| n.id))
    }

    /// Problems that stop the graph from rendering: unknown kinds, inputs
    /// that point nowhere, loops.
    pub fn check(&self) -> Result<(), String> {
        let mut seen = std::collections::HashSet::new();
        for n in &self.nodes {
            if !seen.insert(n.id) {
                return Err(format!("two nodes have id {}", n.id));
            }
            let def = n.def().ok_or_else(|| format!("node {}: no node kind \"{}\"; kinds: {}", n.id, n.kind, nodes::names()))?;
            if n.kind == "effect" && n.effect_def().is_none() {
                return Err(format!("node {}: an effect node needs data.effect, an effect id", n.id));
            }
            if n.inputs.len() > def.inputs.len() {
                return Err(format!("node {} ({}) takes {} inputs: {}", n.id, n.kind, def.inputs.len(), def.inputs.join(", ")));
            }
            for i in n.inputs.iter().flatten().chain(n.mask.iter()) {
                if self.node(*i).is_none() {
                    return Err(format!("node {} reads node {i}, which does not exist", n.id));
                }
            }
        }
        for n in &self.nodes {
            let mut path = Vec::new();
            self.loop_from(n.id, &mut path)?;
        }
        Ok(())
    }

    fn loop_from(&self, id: NodeId, path: &mut Vec<NodeId>) -> Result<(), String> {
        if path.contains(&id) {
            return Err(format!("nodes {:?} feed each other in a loop", path));
        }
        if path.len() > 512 {
            return Err("the graph is too deep".into());
        }
        let Some(n) = self.node(id) else { return Ok(()) };
        path.push(id);
        for i in n.inputs.iter().flatten().chain(n.mask.iter()) {
            self.loop_from(*i, path)?;
        }
        path.pop();
        Ok(())
    }

    /// Whether the picture changes over time with nothing else changing:
    /// a video read, keyframes, noise, paint lifetimes, a track.
    pub fn is_animated(&self) -> bool {
        self.nodes.iter().any(|n| n.animated() || n.def().is_some_and(|d| (d.animated)(n)))
    }

    /// Nodes in an order where every node comes after its inputs.
    pub fn order(&self) -> Vec<NodeId> {
        let mut out = Vec::new();
        fn visit(g: &Graph, id: NodeId, out: &mut Vec<NodeId>, depth: usize) {
            if out.contains(&id) || depth > 512 {
                return;
            }
            if let Some(n) = g.node(id) {
                for i in n.inputs.iter().flatten().chain(n.mask.iter()) {
                    visit(g, *i, out, depth + 1);
                }
            }
            out.push(id);
        }
        for n in &self.nodes {
            visit(self, n.id, &mut out, 0);
        }
        out
    }
}

/// Picture files the graph reads (`read` nodes), resolved by the host.
pub trait Reader: Sync {
    /// The frame of `path` shown at `seconds`, and a key that changes when
    /// the picture does (frame index, file identity). `None` when the file
    /// cannot be read.
    fn read(&self, path: &str, seconds: f64) -> Option<(Arc<Image>, u64)>;
}

/// A reader that has nothing.
pub struct NoFiles;

impl Reader for NoFiles {
    fn read(&self, _: &str, _: f64) -> Option<(Arc<Image>, u64)> {
        None
    }
}

/// Everything a graph evaluation needs from its host.
pub struct Host<'a> {
    /// The incoming picture (the clip, or everything below an adjustment
    /// layer) at the current time.
    pub input: &'a Image,
    /// A key that changes when `input` does. [`hash_image`] computes one.
    pub input_key: u64,
    pub reader: &'a dyn Reader,
    /// Seconds since the clip start.
    pub time: f64,
    pub frame_rate: f64,
    pub seed: u32,
}

/// What a node sees while it runs.
pub struct NodeCtx<'a> {
    /// Seconds since the clip start.
    pub time: f64,
    pub frame_rate: f64,
    pub seed: u32,
    pub width: usize,
    pub height: usize,
    pub reader: &'a dyn Reader,
    /// The node's inputs at other frames, for kinds that ask for them
    /// ([`NodeDef::offsets`]): `(frame offset, plates by input slot)`.
    pub neighbours: Vec<(i32, Vec<Option<Arc<Plate>>>)>,
    pub node: &'a Node,
}

impl NodeCtx<'_> {
    /// The frame number (clip frames from 0).
    pub fn frame(&self) -> i64 {
        (self.time * self.frame_rate).round() as i64
    }

    /// A blank plate of the comp's size.
    pub fn blank(&self) -> Plate {
        Plate::new(Image::new(self.width, self.height))
    }
}

/// Hash of a picture's pixels, for [`Host::input_key`].
pub fn hash_image(img: &Image) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (img.width, img.height).hash(&mut h);
    // Bits of every pixel; fast enough next to any node's work.
    let words: &[f32] = img.pixels.as_flattened();
    let mut acc = 0xcbf2_9ce4_8422_2325u64;
    for chunk in words.chunks(4) {
        let mut v = 0u64;
        for (i, f) in chunk.iter().enumerate() {
            v ^= (f.to_bits() as u64).rotate_left(i as u32 * 16);
        }
        acc = (acc ^ v).wrapping_mul(0x0000_0100_0000_01b3);
    }
    acc.hash(&mut h);
    h.finish()
}

// ------------------------------------------------------------- the cache

struct CacheEntry {
    plate: Arc<Plate>,
    bytes: usize,
    used: u64,
}

struct Cache {
    map: HashMap<u64, CacheEntry>,
    bytes: usize,
    budget: usize,
    clock: u64,
    hits: u64,
    misses: u64,
}

fn cache() -> &'static Mutex<Cache> {
    static C: OnceLock<Mutex<Cache>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(Cache { map: HashMap::new(), bytes: 0, budget: 768 << 20, clock: 0, hits: 0, misses: 0 }))
}

/// Sets how many bytes of node results are kept between frames.
pub fn set_cache_budget(bytes: usize) {
    let mut c = cache().lock().unwrap_or_else(|e| e.into_inner());
    c.budget = bytes;
    trim(&mut c);
}

/// Cache hits and misses since the start.
pub fn cache_stats() -> (u64, u64) {
    let c = cache().lock().unwrap_or_else(|e| e.into_inner());
    (c.hits, c.misses)
}

/// Forgets every cached result.
pub fn clear_cache() {
    let mut c = cache().lock().unwrap_or_else(|e| e.into_inner());
    c.map.clear();
    c.bytes = 0;
}

fn trim(c: &mut Cache) {
    while c.bytes > c.budget && !c.map.is_empty() {
        let Some((&k, _)) = c.map.iter().min_by_key(|(_, e)| e.used) else { break };
        if let Some(e) = c.map.remove(&k) {
            c.bytes -= e.bytes;
        }
    }
}

fn cache_get(key: u64) -> Option<Arc<Plate>> {
    let mut c = cache().lock().unwrap_or_else(|e| e.into_inner());
    c.clock += 1;
    let clock = c.clock;
    match c.map.get_mut(&key) {
        Some(e) => {
            e.used = clock;
            let p = e.plate.clone();
            c.hits += 1;
            Some(p)
        }
        None => {
            c.misses += 1;
            None
        }
    }
}

fn cache_put(key: u64, plate: Arc<Plate>) {
    let bytes = plate.bytes();
    let mut c = cache().lock().unwrap_or_else(|e| e.into_inner());
    if bytes > c.budget / 4 {
        return;
    }
    c.clock += 1;
    let used = c.clock;
    if let Some(old) = c.map.insert(key, CacheEntry { plate, bytes, used }) {
        c.bytes -= old.bytes;
    }
    c.bytes += bytes;
    trim(&mut c);
}

// -------------------------------------------------------------- evaluation

/// The result of one node at one time, with the key that names it.
#[derive(Clone)]
pub struct Evaluated {
    pub key: u64,
    pub plate: Arc<Plate>,
}

/// Runs a graph for one frame and returns the output node's plate (the
/// input, unchanged, for an empty graph).
pub fn evaluate(graph: &Graph, host: &Host) -> Result<Arc<Plate>, String> {
    let Some(out) = graph.output_id() else { return Ok(Arc::new(Plate::new(host.input.clone()))) };
    evaluate_node(graph, host, out)
}

/// Runs a graph up to node `id` (for viewing any node).
pub fn evaluate_node(graph: &Graph, host: &Host, id: NodeId) -> Result<Arc<Plate>, String> {
    graph.check()?;
    let mut ev = Evaluator { graph, host, memo: HashMap::new(), keys: Default::default() };
    Ok(ev.eval(id, 0, 0)?.plate)
}

/// Runs node `id` on inputs the caller already has (by slot, and its mask
/// node's result), for hosts that run some nodes elsewhere (the GPU).
/// Nodes that look at other frames evaluate their upstream themselves.
/// Results are cached like [`evaluate`]'s.
pub fn run_node(graph: &Graph, host: &Host, id: NodeId, inputs: &[Option<Arc<Plate>>], mask: Option<Arc<Plate>>) -> Result<Arc<Plate>, String> {
    let n = graph.node(id).ok_or_else(|| format!("no node {id}"))?;
    let def = n.def().ok_or_else(|| format!("no node kind \"{}\"", n.kind))?;
    if !(def.offsets)(n).is_empty() {
        return evaluate_node(graph, host, id);
    }
    let ev = Evaluator { graph, host, memo: HashMap::new(), keys: Default::default() };
    let key = ev.key_of(id, 0, 0);
    if let Some(p) = cache_get(key) {
        return Ok(p);
    }
    let plate = ev.compute(n, def, 0, inputs, mask, Vec::new())?;
    cache_put(key, plate.clone());
    Ok(plate)
}

/// Each node's key at the current frame, without running anything:
/// whether a result would be recomputed. Used by tests and the graph view.
pub fn node_keys(graph: &Graph, host: &Host) -> Result<Vec<(NodeId, u64)>, String> {
    graph.check()?;
    let ev = Evaluator { graph, host, memo: HashMap::new(), keys: Default::default() };
    Ok(graph.order().into_iter().map(|id| (id, ev.key_of(id, 0, 0))).collect())
}

struct Evaluator<'a> {
    graph: &'a Graph,
    host: &'a Host<'a>,
    memo: HashMap<(NodeId, i32), Evaluated>,
    keys: std::cell::RefCell<HashMap<(NodeId, i32), u64>>,
}

impl Evaluator<'_> {
    fn time(&self, offset: i32) -> f64 {
        self.host.time + offset as f64 / self.host.frame_rate.max(1e-6)
    }

    /// The content key of node `id` at frame `offset`, computed from its
    /// definition and its inputs' keys (no pixels are produced).
    fn key_of(&self, id: NodeId, offset: i32, depth: usize) -> u64 {
        if let Some(k) = self.keys.borrow().get(&(id, offset)) {
            return *k;
        }
        let k = self.compute_key(id, offset, depth);
        self.keys.borrow_mut().insert((id, offset), k);
        k
    }

    fn compute_key(&self, id: NodeId, offset: i32, depth: usize) -> u64 {
        let Some(n) = self.graph.node(id) else { return 0 };
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let t = self.time(offset);
        n.kind.hash(&mut h);
        n.disabled.hash(&mut h);
        n.mix.to_bits().hash(&mut h);
        format!("{:?}", n.params_at(t).values()).hash(&mut h);
        n.data.to_string().hash(&mut h);
        if let Some(e) = n.data.get("effect") {
            e.to_string().hash(&mut h);
        }
        let def = n.def();
        if depth < 256 {
            for i in &n.inputs {
                match i {
                    Some(i) => self.key_of(*i, offset, depth + 1).hash(&mut h),
                    None => 0u64.hash(&mut h),
                }
            }
            if let Some(m) = n.mask {
                self.key_of(m, offset, depth + 1).hash(&mut h);
            }
            if let Some(d) = def {
                for o in (d.offsets)(n) {
                    for i in n.inputs.iter().flatten() {
                        (o, self.key_of(*i, offset + o, depth + 1)).hash(&mut h);
                    }
                }
            }
        }
        match n.kind.as_str() {
            "input" => self.input_key(offset).hash(&mut h),
            "read" => {
                let p = nodes::read_request(n, &n.params_at(t), t, self.host.frame_rate);
                if let Some((path, s)) = p {
                    path.hash(&mut h);
                    // The reader names the frame; asking for it is cheap.
                    self.host.reader.read(&path, s).map(|(_, k)| k).hash(&mut h);
                }
            }
            _ => {}
        }
        if def.is_some_and(|d| (d.animated)(n)) {
            (t.to_bits(), self.host.seed).hash(&mut h);
        }
        self.host.frame_rate.to_bits().hash(&mut h);
        (self.host.input.width, self.host.input.height).hash(&mut h);
        h.finish()
    }

    fn input_key(&self, offset: i32) -> u64 {
        if offset == 0 {
            return self.host.input_key;
        }
        match &self.graph.input_media {
            Some(m) => {
                let s = m.start + self.time(offset) * m.speed;
                self.host.reader.read(&m.path, s).map_or(self.host.input_key, |(_, k)| k)
            }
            None => self.host.input_key,
        }
    }

    fn input_at(&self, offset: i32) -> Image {
        if offset != 0 {
            if let Some(m) = &self.graph.input_media {
                let s = m.start + self.time(offset) * m.speed;
                if let Some((img, _)) = self.host.reader.read(&m.path, s) {
                    let (w, h) = (self.host.input.width, self.host.input.height);
                    if (img.width, img.height) == (w, h) {
                        return (*img).clone();
                    }
                    return Plate::new((*img).clone()).resized(w, h).rgba;
                }
            }
        }
        self.host.input.clone()
    }

    fn eval(&mut self, id: NodeId, offset: i32, depth: usize) -> Result<Evaluated, String> {
        if let Some(e) = self.memo.get(&(id, offset)) {
            return Ok(e.clone());
        }
        if depth > 256 {
            return Err("the graph is too deep".into());
        }
        let graph = self.graph;
        let n = graph.node(id).ok_or_else(|| format!("no node {id}"))?;
        let def = n.def().ok_or_else(|| format!("no node kind \"{}\"", n.kind))?;
        let key = self.key_of(id, offset, 0);
        if let Some(p) = cache_get(key) {
            let e = Evaluated { key, plate: p };
            self.memo.insert((id, offset), e.clone());
            return Ok(e);
        }
        let mut inputs = Vec::with_capacity(n.inputs.len());
        for i in &n.inputs {
            inputs.push(match i {
                Some(i) => Some(self.eval(*i, offset, depth + 1)?.plate),
                None => None,
            });
        }
        let mask = match n.mask {
            Some(m) => Some(self.eval(m, offset, depth + 1)?.plate),
            None => None,
        };
        let mut neighbours = Vec::new();
        if !n.disabled {
            for o in (def.offsets)(n) {
                let mut at = Vec::new();
                for i in &n.inputs {
                    at.push(match i {
                        Some(i) => Some(self.eval(*i, offset + o, depth + 1)?.plate),
                        None => None,
                    });
                }
                neighbours.push((o, at));
            }
        }
        let plate = self.compute(n, def, offset, &inputs, mask, neighbours)?;
        cache_put(key, plate.clone());
        let e = Evaluated { key, plate };
        self.memo.insert((id, offset), e.clone());
        Ok(e)
    }

    /// Runs one node on given inputs, then mixes and masks it.
    fn compute(&self, n: &Node, def: &NodeDef, offset: i32, inputs: &[Option<Arc<Plate>>], mask: Option<Arc<Plate>>, neighbours: Vec<(i32, Vec<Option<Arc<Plate>>>)>) -> Result<Arc<Plate>, String> {
        let (w, h) = (self.host.input.width, self.host.input.height);
        let t = self.time(offset);
        let plate = if n.disabled {
            inputs.first().cloned().flatten().unwrap_or_else(|| Arc::new(Plate::new(Image::new(w, h))))
        } else {
            let ctx = NodeCtx { time: t, frame_rate: self.host.frame_rate, seed: self.host.seed, width: w, height: h, reader: self.host.reader, neighbours, node: n };
            let params = n.params_at(t);
            let mut out = if n.kind == "input" {
                Plate::new(self.input_at(offset))
            } else {
                (def.run)(&ctx, inputs, &params)?
            };
            if (out.width(), out.height()) != (w, h) {
                out = out.resized(w, h);
            }
            out.rgba.sanitise();
            // Mix and mask against the main input.
            let main = inputs.first().cloned().flatten();
            if n.mix < 1.0 || mask.is_some() {
                let base = main.map(|m| m.rgba.clone()).unwrap_or_else(|| Image::new(w, h));
                let mix = n.mix.clamp(0.0, 1.0);
                let m = mask.as_ref().map(|m| m.alpha());
                out.rgba = Image::from_fn(w, h, |x, y| {
                    let k = mix * m.as_ref().map_or(1.0, |m| m[y * w + x].clamp(0.0, 1.0));
                    lerp_px(base.get(x, y), out.rgba.get(x, y), k)
                });
            }
            Arc::new(out)
        };
        Ok(plate)
    }
}

#[cfg(test)]
mod tests;
