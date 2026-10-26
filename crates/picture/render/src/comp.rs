// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The node compositor in the renderer: a `comp` effect holds a graph
//! (`studio_fx::comp::Graph`, as JSON in its `graph` parameter) that
//! replaces the clip's picture, or everything below an adjustment layer,
//! with the graph's output.
//!
//! [`FileReader`] gives the graph's `read` nodes their pictures: files are
//! opened once, frames decoded on demand and kept within a byte budget.
//! The CPU path runs the whole graph here; the GPU compositor runs the
//! per-pixel nodes as WGSL (`gpu::comp`) and comes back here for the rest.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use studio_fx::comp::{self, Graph, Host, Reader};
use studio_fx::Image;

use crate::effects::EffectCtx;
use crate::frame::Frame;
use crate::scene::{EffectDesc, ParamsExt, SECOND};
use crate::source::{MediaSource, open_path};

/// The effect kind that holds a graph.
pub const KIND: &str = "comp";

/// The graph in a `comp` effect, if it parses.
pub fn graph_of(e: &EffectDesc) -> Option<Graph> {
    if e.kind != KIND {
        return None;
    }
    let text = e.params.text("graph", "");
    Graph::from_json(text).ok()
}

/// Whether a `comp` effect's picture changes over time by itself.
pub fn is_animated(e: &EffectDesc) -> bool {
    graph_of(e).is_some_and(|g| g.is_animated() || g.nodes.iter().any(|n| n.kind == "read"))
}

struct Open {
    source: Box<dyn MediaSource>,
    stamp: u64,
}

struct Frames {
    open: HashMap<String, Open>,
    frames: HashMap<(String, u64), (Arc<Image>, u64)>,
    order: Vec<(String, u64)>,
    bytes: usize,
}

fn store() -> &'static Mutex<Frames> {
    static S: OnceLock<Mutex<Frames>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Frames { open: HashMap::new(), frames: HashMap::new(), order: Vec::new(), bytes: 0 }))
}

/// Opens files the renderer cannot read itself (a cut-out's matte file,
/// say): `None` when the file is not one it knows.
pub type Opener = fn(&Path) -> Option<Box<dyn MediaSource>>;

fn openers() -> &'static Mutex<Vec<Opener>> {
    static O: OnceLock<Mutex<Vec<Opener>>> = OnceLock::new();
    O.get_or_init(|| Mutex::new(Vec::new()))
}

/// Adds a file opener for `read` nodes (tried before the built-in ones).
pub fn add_opener(f: Opener) {
    let mut o = openers().lock().unwrap_or_else(|e| e.into_inner());
    if !o.iter().any(|g| std::ptr::fn_addr_eq(*g, f)) {
        o.push(f);
    }
}

fn open(path: &str) -> Option<Box<dyn MediaSource>> {
    let p = Path::new(path);
    let list = openers().lock().unwrap_or_else(|e| e.into_inner()).clone();
    list.iter().find_map(|f| f(p)).or_else(|| open_path(Path::new(path.split('#').next().unwrap_or(path))).ok())
}

/// Bytes of decoded `read` frames kept between frames.
const BUDGET: usize = 768 << 20;

/// Pictures for `read` nodes, from files on disk.
pub struct FileReader;

fn file_stamp(path: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    // A `#` part carries options for an opener, not the file name.
    if let Ok(m) = std::fs::metadata(path.split('#').next().unwrap_or(path)) {
        m.len().hash(&mut h);
        if let Ok(t) = m.modified() {
            t.hash(&mut h);
        }
    }
    h.finish()
}

impl Reader for FileReader {
    fn read(&self, path: &str, seconds: f64) -> Option<(Arc<Image>, u64)> {
        let mut guard = store().lock().unwrap_or_else(|e| e.into_inner());
        let s = &mut *guard;
        let stamp = file_stamp(path);
        if s.open.get(path).is_none_or(|o| o.stamp != stamp) {
            // A changed file: forget what was decoded from it.
            s.frames.retain(|k, _| k.0 != path);
            s.order.retain(|k| k.0 != path);
            let source = open(path)?;
            s.open.insert(path.to_owned(), Open { source, stamp });
        }
        let index = s.open.get(path)?.source.frame_index((seconds * SECOND as f64).round() as i64);
        let key = (path.to_owned(), index);
        if let Some((img, k)) = s.frames.get(&key).cloned() {
            if let Some(p) = s.order.iter().position(|k2| *k2 == key) {
                let k2 = s.order.remove(p);
                s.order.push(k2);
            }
            return Some((img, k));
        }
        let mut got = None;
        let mut extra = Vec::new();
        s.open.get_mut(path)?.source.decode_to(index, &mut |i, pic| {
            if i == index {
                got = Some(pic);
            } else if index.saturating_sub(i) <= 2 {
                extra.push((i, pic));
            }
        });
        let pic = got?;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (stamp, index).hash(&mut h);
        let id = h.finish();
        let img = Arc::new(pic.to_frame(true).into_fx());
        let bytes = img.pixels.len() * 16;
        s.frames.insert(key.clone(), (img.clone(), id));
        s.order.push(key);
        s.bytes += bytes;
        // Keep the frames just before too: temporal nodes ask for them next.
        for (i, p) in extra {
            let k = (path.to_owned(), i);
            if s.frames.contains_key(&k) {
                continue;
            }
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (stamp, i).hash(&mut h);
            let im = Arc::new(p.to_frame(true).into_fx());
            s.bytes += im.pixels.len() * 16;
            s.frames.insert(k.clone(), (im, h.finish()));
            s.order.push(k);
        }
        while s.bytes > BUDGET && !s.order.is_empty() {
            let k = s.order.remove(0);
            if let Some((im, _)) = s.frames.remove(&k) {
                s.bytes -= im.pixels.len() * 16;
            }
        }
        Some((img, id))
    }
}

/// A host for evaluating a graph over `input` at the effect's time.
pub fn host<'a>(input: &'a Image, ctx: EffectCtx) -> Host<'a> {
    Host { input, input_key: comp::hash_image(input), reader: &FileReader, time: ctx.time as f64, frame_rate: ctx.frame_rate as f64, seed: ctx.seed }
}

/// Runs a `comp` effect on the CPU. A graph that does not parse or run
/// leaves the picture as it was.
pub fn apply_cpu(frame: Frame, e: &EffectDesc, ctx: EffectCtx) -> Frame {
    let Some(graph) = graph_of(e) else { return frame };
    let img = frame.into_fx();
    let out = match comp::evaluate(&graph, &host(&img, ctx)) {
        Ok(p) => p.rgba.clone(),
        Err(_) => img,
    };
    Frame::from_fx(out)
}
