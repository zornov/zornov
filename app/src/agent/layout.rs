// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Layout knowledge: where every layer, picture, text block, puppet (and its
//! parts), shape, 3D object and world item is on the canvas at any moment,
//! worked out from the project itself, not guessed and not by a model.
//!
//! How it is known:
//!
//! * The renderer resolves the frame (`studio_render::resolve`), so the
//!   draw order, the 3D camera and each layer's placed box are exactly what
//!   the picture is made from. A 3D layer or model is projected with the
//!   renderer's own `layer_geometry` / `screen_rects`, with depth.
//! * Each item is then drawn alone on a see-through canvas by the CPU
//!   compositor, and its alpha read. That alpha is the source picture's own
//!   transparency after transform, crop, mask, effects and puppet bending,
//!   so the bounds are of what shows, not of the transform box.
//! * From the alpha grids: what covers what, how much of each item is
//!   visible, free space, balance, and the line boxes of a text block.
//! * Rigs report each named part from the posed skeleton and pins through
//!   the layer's own placement; part layers report where they were drawn.
//!
//! Positions are canvas pixels from the centre, y down, like `x` and `y`
//! everywhere else. Each [`Item`] carries `labels` so what an object
//! detector finds in footage (a person, a dog) can be attached to the item
//! it was found in: nothing here looks inside footage.

use super::{Session, secs};
use crate::view::{self, SEC};
use serde_json::{Value, json};
use std::collections::HashMap;
use studio_model as m;

#[path = "layout_answer.rs"]
mod answer;
#[path = "layout_check.rs"]
mod check;
#[path = "layout_grid.rs"]
pub(super) mod grid;

use grid::{Grid, Rect};

/// Width of the coverage grids the reads use.
pub(super) const GRID_W: usize = 256;
/// A coarser grid for the short summaries inside other replies.
pub(super) const SUMMARY_GRID_W: usize = 160;

/// What an item is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Picture,
    Footage,
    Text,
    Shape,
    Solid,
    Puppet,
    Part,
    Model,
    World,
    WorldObject,
    Compound,
}

impl Kind {
    pub(super) fn word(self) -> &'static str {
        match self {
            Kind::Picture => "picture",
            Kind::Footage => "footage",
            Kind::Text => "text",
            Kind::Shape => "shape",
            Kind::Solid => "solid",
            Kind::Puppet => "puppet",
            Kind::Part => "part",
            Kind::Model => "3d model",
            Kind::World => "world",
            Kind::WorldObject => "world object",
            Kind::Compound => "compound",
        }
    }
}

/// What an item does in the picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Role {
    /// Fills the frame: a place, not a thing in it.
    Backdrop,
    Text,
    /// A character: a rig or a puppet.
    Figure,
    Prop,
    /// A layer that belongs to a rig (a mouth, a hand).
    Part,
}

/// Something found in an item's picture (by object detection), with its
/// box in the picture's own 0..1 coordinates and on the canvas.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Label {
    pub name: String,
    pub score: f32,
    pub source: String,
    pub uv: Rect,
    pub canvas: Rect,
}

/// A named part of a rig at one moment.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct PartAt {
    pub name: String,
    pub at: (f32, f32),
    /// Where it was read from: `bone`, `pin` or `layer`.
    pub via: &'static str,
}

/// One thing on the canvas.
#[derive(Clone, Debug)]
pub(super) struct Item {
    pub id: u64,
    /// The index of a world object, for `WorldObject` items (their `id` is
    /// the world's clip).
    pub object: Option<usize>,
    pub name: String,
    pub kind: Kind,
    pub role: Role,
    /// Its text, or the media it shows.
    pub what: String,
    /// The rig a part belongs to.
    pub of: Option<u64>,
    /// Draw order, 0 is the bottom.
    pub z: usize,
    /// Distance in front of the lens, for 3D layers, models and world objects.
    pub depth: Option<f32>,
    /// The transform box, canvas px from the centre.
    pub quad: Vec<(f32, f32)>,
    /// The box of what shows (alpha at least half), canvas px from the centre.
    pub bounds: Option<Rect>,
    /// The box including faint edges (glow, shadow, blur), when it is wider.
    pub reach: Option<Rect>,
    /// Whether `bounds` come from the drawn alpha (else the transform box).
    pub from_alpha: bool,
    pub grid: Option<Grid>,
    pub rotation: f32,
    pub tilt: Option<(f32, f32)>,
    pub opacity: f32,
    /// Share of the transform box inside the frame.
    pub on_frame: f32,
    /// Pixels of the transform box beyond the frame: left, top, right, bottom.
    pub spill: [f32; 4],
    /// Whether the drawn alpha reaches the frame's left, top, right, bottom edge.
    pub touches: [bool; 4],
    /// Share of the item still showing after what is above it.
    pub visible: f32,
    /// Items (by position in the layout) above it that hide part of it.
    pub covered_by: Vec<(usize, f32)>,
    /// Items below it that it hides part of.
    pub covers: Vec<(usize, f32)>,
    pub lines: Vec<Rect>,
    pub parts: Vec<PartAt>,
    pub facing: Option<&'static str>,
    pub labels: Vec<Label>,
    pub transition: bool,
}

impl Item {
    /// The middle of what shows (else of the box).
    pub(super) fn centre(&self) -> (f32, f32) {
        match self.bounds {
            Some(b) => ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0),
            None if !self.quad.is_empty() => {
                let n = self.quad.len() as f32;
                (self.quad.iter().map(|p| p.0).sum::<f32>() / n, self.quad.iter().map(|p| p.1).sum::<f32>() / n)
            }
            None => (0.0, 0.0),
        }
    }

    /// The box of the transform quad.
    pub(super) fn quad_box(&self) -> Rect {
        let xs = self.quad.iter().map(|p| p.0);
        let ys = self.quad.iter().map(|p| p.1);
        [xs.clone().fold(f32::MAX, f32::min), ys.clone().fold(f32::MAX, f32::min), xs.fold(f32::MIN, f32::max), ys.fold(f32::MIN, f32::max)]
    }

    /// Whether it has anything showing in the frame.
    pub(super) fn shows(&self) -> bool {
        self.bounds.is_some()
    }
}

/// The frame at one moment.
pub(super) struct Layout {
    pub t: view::Time,
    pub w: f32,
    pub h: f32,
    /// Canvas pixels per grid pixel.
    pub cell: f32,
    pub gw: usize,
    pub gh: usize,
    /// Bottom to top.
    pub items: Vec<Item>,
    pub camera: Option<Value>,
}

/// What to work out.
#[derive(Clone, Debug, Default)]
pub(super) struct Opts {
    /// Width of the grids (default [`GRID_W`]).
    pub grid: usize,
    /// Draw only these items (and what is beside them) for their alpha; the
    /// others keep their transform box.
    pub focus: Vec<u64>,
    /// Place everything from the renderer's geometry and draw nothing: no
    /// alpha, so no occlusion, but fast (and enough for world objects).
    pub geometry_only: bool,
}

/// Motion over a stretch of time.
#[derive(Clone, Debug)]
pub(super) struct Motion {
    /// (seconds, x, y) of the middle of the transform box.
    pub path: Vec<(f64, f32, f32)>,
    pub travel: f32,
    pub avg: f32,
    pub top: f32,
    pub enters: Option<f64>,
    pub leaves: Option<f64>,
}

/// What the model knows of a clip beside what the renderer is given.
#[derive(Clone, Debug, Default)]
struct Info {
    name: String,
    rig_of: Option<u64>,
    rigged: bool,
}

#[cfg(feature = "render")]
mod engine {
    pub(super) use studio_render::compositor::layer_geometry;
    pub(super) use studio_render::scene::{GeneratorKind as RGen, SourceDesc};
    pub(super) use studio_render::source::{DecodeCache, SourceRegistry, Sources};
    pub(super) use studio_render::{CpuCompositor, FrameRenderer};
}

#[cfg(feature = "render")]
use engine::*;

/// What a render of the project needs, built once per read.
#[cfg(feature = "render")]
pub(super) struct Ctx {
    registry: std::sync::Arc<SourceRegistry>,
    cache: std::sync::Arc<DecodeCache>,
    sources: Sources,
}

/// One drawn thing before its alpha is read.
#[cfg(feature = "render")]
struct Raw {
    id: u64,
    object: Option<usize>,
    kind: Kind,
    what: String,
    name: Option<String>,
    quad: Vec<(f32, f32)>,
    depth: Option<f32>,
    opacity: f32,
    rotation: f32,
    tilt: Option<(f32, f32)>,
    full: bool,
    transition: bool,
}

#[cfg(feature = "render")]
fn r1(v: f32) -> f32 {
    (v * 10.0).round() / 10.0
}

#[cfg(feature = "render")]
impl Session {
    pub(super) fn layout_ctx(&self) -> Ctx {
        let registry = SourceRegistry::new();
        crate::editor::render_view::register_sources(&self.doc, &registry);
        crate::editor::render_view::install_text_renderer(self.doc.project.height);
        let cache = DecodeCache::new(256 << 20);
        let sources = Sources::new(registry.clone(), cache.clone());
        Ctx { registry, cache, sources }
    }

    fn clip_infos(&self) -> HashMap<u64, Info> {
        self.doc
            .model
            .active()
            .clips
            .iter()
            .map(|c| {
                let rig_of = c.meta.get("part").and_then(|p| p.get("rig")).and_then(Value::as_u64);
                let rigged = c.skeleton.as_ref().is_some_and(|s| s.enabled && !s.bones.is_empty()) || c.puppet.as_ref().is_some_and(|p| p.enabled && !p.pins.is_empty());
                (c.id.0, Info { name: c.name.clone(), rig_of, rigged })
            })
            .collect()
    }

    /// The frame at `t` as the renderer resolves it: every drawn thing with
    /// its place, bottom to top (3D layers in their depth order).
    fn layout_raw(&self, ctx: &mut Ctx, scene: &studio_render::model_scene::ModelScene, t: view::Time) -> (Vec<Raw>, [f32; 2], studio_render::space::CameraDesc) {
        use studio_render::resolve::{Node, resolve};
        let list = resolve(scene, t);
        let canvas = list.canvas;
        let cs = canvas.size();
        let camera = list.camera();
        let order = studio_render::space::draw_order(&list, |clip, pose| layer_geometry(&mut ctx.sources, clip, pose, &canvas, &camera, 1.0).and_then(|g| g.depth));
        let mut out: Vec<Raw> = Vec::new();
        let mut push_layer = |ctx: &mut Ctx, clip: &studio_render::ClipDesc, pose: &studio_render::Pose, transition: bool, out: &mut Vec<Raw>| {
            if clip.blend.is_matte() {
                return;
            }
            let opacity = clip.placement.opacity * pose.opacity;
            let (kind, what) = match &clip.source {
                SourceDesc::Media { media, .. } => {
                    let item = self.doc.project.media(media.0 as u32);
                    (if item.is_some_and(|i| i.kind == view::MediaKind::Image) { Kind::Picture } else { Kind::Footage }, item.map(|i| i.name.clone()).unwrap_or_default())
                }
                SourceDesc::Generator(g) => match g.kind {
                    RGen::Text => (Kind::Text, g.text.iter().map(|r| r.text.as_str()).collect::<String>()),
                    RGen::Shape => (Kind::Shape, String::new()),
                    RGen::Solid | RGen::Gradient => (Kind::Solid, String::new()),
                },
                SourceDesc::Nested { .. } => (Kind::Compound, String::new()),
                SourceDesc::Mesh(desc) => {
                    // Each model is its own item: its box seen through the camera.
                    let cam = desc.camera.unwrap_or(camera);
                    for (id, r) in studio_render::mesh_render::screen_rects(desc, &canvas) {
                        let one = studio_render::mesh_render::MeshDesc { instances: desc.instances.iter().filter(|i| i.id == id).cloned().collect(), camera: desc.camera, lights: Vec::new(), ..Default::default() };
                        let depth = studio_render::mesh_render::world_bounds(&one).map(|(a, b)| cam.view([(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0, (a[2] + b[2]) / 2.0])[2]);
                        let quad = vec![(r[0] - cs[0] / 2.0, r[1] - cs[1] / 2.0), (r[2] - cs[0] / 2.0, r[1] - cs[1] / 2.0), (r[2] - cs[0] / 2.0, r[3] - cs[1] / 2.0), (r[0] - cs[0] / 2.0, r[3] - cs[1] / 2.0)];
                        out.push(Raw { id, object: None, kind: Kind::Model, what: String::new(), name: None, quad, depth, opacity, rotation: 0.0, tilt: None, full: false, transition });
                    }
                    return;
                }
                SourceDesc::World(w) => {
                    let quad = vec![(-cs[0] / 2.0, -cs[1] / 2.0), (cs[0] / 2.0, -cs[1] / 2.0), (cs[0] / 2.0, cs[1] / 2.0), (-cs[0] / 2.0, cs[1] / 2.0)];
                    out.push(Raw { id: clip.id, object: None, kind: Kind::World, what: w.world.name.clone(), name: None, quad, depth: None, opacity, rotation: 0.0, tilt: None, full: true, transition });
                    world_objects(w, clip.id, &camera, &canvas, opacity, out);
                    return;
                }
                SourceDesc::Adjustment | SourceDesc::Camera(_) | SourceDesc::Light(_) => return,
            };
            let Some(g) = layer_geometry(&mut ctx.sources, clip, pose, &canvas, &camera, 1.0) else { return };
            let quad: Vec<(f32, f32)> = g.quad().iter().map(|p| (p[0] - cs[0] / 2.0, p[1] - cs[1] / 2.0)).collect();
            let tilt = clip.space.map(|s| (s.rotation_x + pose.turn[0], s.rotation_y + pose.turn[1])).or(pose.is_3d().then_some((pose.turn[0], pose.turn[1]))).filter(|t| t.0.abs() > 0.01 || t.1.abs() > 0.01);
            out.push(Raw { id: clip.id, object: None, kind, what, name: None, quad, depth: g.depth, opacity, rotation: clip.placement.rotation + pose.rotation, tilt, full: false, transition });
        };
        for ni in order {
            match &list.nodes[ni] {
                Node::Layer { clip, pose } => push_layer(ctx, clip, pose, false, &mut out),
                Node::Transition { desc, left_pose, right_pose } => {
                    push_layer(ctx, &desc.left, left_pose, true, &mut out);
                    push_layer(ctx, &desc.right, right_pose, true, &mut out);
                }
                Node::Adjustment { .. } => {}
            }
        }
        (out, cs, camera)
    }

    /// Each job `(clip, width)` drawn alone at `t`: its alpha grid.
    fn layout_alphas(&self, ctx: &Ctx, t: view::Time, jobs: &[(u64, usize)]) -> Vec<Option<Grid>> {
        if jobs.is_empty() {
            return Vec::new();
        }
        let (pw, ph) = (self.doc.project.width.max(1) as f64, self.doc.project.height.max(1) as f64);
        let fps = self.doc.project.fps.max(1.0) as f64;
        let model = &self.doc.model;
        let draw = |renderer: &mut CpuCompositor, (id, width): (u64, usize)| -> Option<Grid> {
            let w = (width as u32).min(pw as u32).max(16);
            let h = ((w as f64 * ph / pw).round() as u32).max(16);
            let scene = alone(model, id);
            let e = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                renderer.render_encoded(&scene, t.max(0), Some((w, h)), &studio_render::OutputSettings { premultiplied: false, dither: false, ..Default::default() }, studio_render::OutputDepth::Eight, (t.max(0) as f64 / 1e6 * fps) as u32)
            }))
            .ok()?;
            let (ew, eh) = (e.width as usize, e.height as usize);
            let mut g = Grid::new(ew, eh);
            match e.pixels {
                studio_render::EncodedPixels::Rgba8(v) => g.a.iter_mut().enumerate().for_each(|(i, a)| *a = v[i * 4 + 3]),
                studio_render::EncodedPixels::Rgba16(v) => g.a.iter_mut().enumerate().for_each(|(i, a)| *a = (v[i * 4 + 3] >> 8) as u8),
            }
            // Rounding noise of the renderer is not a picture.
            g.a.iter_mut().filter(|a| **a < grid::FAINT / 2).for_each(|a| *a = 0);
            Some(g)
        };
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 8).min(jobs.len());
        let per = jobs.len().div_ceil(threads).max(1);
        std::thread::scope(|sc| {
            let hs: Vec<_> = jobs
                .chunks(per)
                .map(|chunk| {
                    let (registry, cache, draw) = (ctx.registry.clone(), ctx.cache.clone(), &draw);
                    (chunk.len(), sc.spawn(move || {
                        let mut r = CpuCompositor::new(Sources::new(registry, cache));
                        chunk.iter().map(|&j| draw(&mut r, j)).collect::<Vec<_>>()
                    }))
                })
                .collect();
            hs.into_iter().flat_map(|(n, h)| h.join().unwrap_or_else(|_| vec![None; n])).collect()
        })
    }

    /// Everything on the canvas at `t`.
    pub(super) fn layout_at(&mut self, t: view::Time, o: &Opts) -> Result<Layout, String> {
        let mut ctx = self.layout_ctx();
        let scene = studio_render::model_scene::ModelScene::new(std::sync::Arc::new(self.doc.model.clone()));
        let (raws, cs, camera) = self.layout_raw(&mut ctx, &scene, t);
        let infos = self.clip_infos();
        let width = if o.grid == 0 { GRID_W } else { o.grid };
        // Which items get their alpha read: all, or the focus and whatever
        // is beside it.
        let bbox = |r: &Raw| {
            let xs = r.quad.iter().map(|p| p.0);
            let ys = r.quad.iter().map(|p| p.1);
            [xs.clone().fold(f32::MAX, f32::min), ys.clone().fold(f32::MAX, f32::min), xs.fold(f32::MIN, f32::max), ys.fold(f32::MIN, f32::max)]
        };
        let want: Vec<bool> = raws
            .iter()
            .map(|r| {
                r.object.is_none()
                    && !o.geometry_only
                    && (o.focus.is_empty()
                        || o.focus.contains(&r.id)
                        || raws.iter().any(|f| o.focus.contains(&f.id) && f.object.is_none() && grid::rect_area(&grid::rect_intersect(&bbox(f), &bbox(r))) > 0.0))
            })
            .collect();
        let mut jobs: Vec<(u64, usize)> = raws.iter().zip(&want).filter(|(_, w)| **w).map(|(r, _)| (r.id, width)).collect();
        // Text lines are read from a bigger drawing of the text alone.
        // The same drawing gives the lettering's own bounds, which a small
        // grid loses at thin strokes.
        let line_jobs: Vec<usize> = raws.iter().enumerate().filter(|(i, r)| want[*i] && r.kind == Kind::Text).map(|(i, _)| i).collect();
        let line_width = (cs[0] as usize).min(1280);
        jobs.extend(line_jobs.iter().map(|&i| (raws[i].id, line_width)));
        let drawn = self.layout_alphas(&ctx, t, &jobs);
        let mut drawn = drawn.into_iter();
        let mut grids: Vec<Option<Grid>> = Vec::new();
        for w in &want {
            grids.push(if *w { drawn.next().flatten() } else { None });
        }
        let mut big: HashMap<usize, Grid> = HashMap::new();
        for &i in &line_jobs {
            if let Some(g) = drawn.next().flatten() {
                big.insert(i, g);
            }
        }
        let (gw, gh) = grids.iter().flatten().next().map_or((width, ((width as f32 * cs[1] / cs[0]).round() as usize).max(1)), |g| (g.w, g.h));
        let cell = cs[0] / gw as f32;
        let frame: Rect = [-cs[0] / 2.0, -cs[1] / 2.0, cs[0] / 2.0, cs[1] / 2.0];
        let to_canvas = |b: Rect, c: f32| [b[0] * c - cs[0] / 2.0, b[1] * c - cs[1] / 2.0, b[2] * c - cs[0] / 2.0, b[3] * c - cs[1] / 2.0];
        let mut items: Vec<Item> = Vec::new();
        for (i, r) in raws.iter().enumerate() {
            let info = infos.get(&r.id).cloned().unwrap_or_default();
            let g = grids[i].as_ref().filter(|g| g.w == gw && g.h == gh).cloned();
            let bounds = g.as_ref().and_then(|g| g.bounds(grid::SOLID)).map(|b| to_canvas(b, cell));
            let reach = g.as_ref().and_then(|g| g.bounds(grid::FAINT)).map(|b| to_canvas(b, cell)).filter(|rc| bounds.is_some_and(|b| (b[0] - rc[0]).max(b[1] - rc[1]).max(rc[2] - b[2]).max(rc[3] - b[3]) > 1.5 * cell));
            let box_ = {
                let xs = r.quad.iter().map(|p| p.0);
                let ys = r.quad.iter().map(|p| p.1);
                [xs.clone().fold(f32::MAX, f32::min), ys.clone().fold(f32::MAX, f32::min), xs.fold(f32::MIN, f32::max), ys.fold(f32::MIN, f32::max)]
            };
            let from_alpha = g.is_some();
            let lines: Vec<Rect> = big.get(&i).map(|bg| grid::line_boxes(bg).into_iter().map(|b| to_canvas(b, cs[0] / bg.w as f32)).collect()).unwrap_or_default();
            // Lettering: the union of its lines is the box that shows.
            let bounds = if from_alpha && !lines.is_empty() { Some(lines.iter().fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |a, l| [a[0].min(l[0]), a[1].min(l[1]), a[2].max(l[2]), a[3].max(l[3])])) } else { bounds };
            // Without a drawing, the transform box stands in.
            let bounds = if from_alpha { bounds } else if r.quad.len() >= 3 && grid::rect_area(&grid::rect_intersect(&box_, &frame)) > 0.0 { Some(grid::rect_intersect(&box_, &frame)) } else { None };
            let name = if r.object.is_some() { r.name.clone().unwrap_or_default() } else if info.name.is_empty() { format!("clip {}", r.id) } else { info.name.clone() };
            let role = if info.rig_of.is_some() {
                Role::Part
            } else if r.kind == Kind::Text {
                Role::Text
            } else if r.kind == Kind::World || r.full || g.as_ref().is_some_and(|g| g.frame_share() >= 0.8) || (g.is_none() && grid::rect_area(&grid::rect_intersect(&box_, &frame)) >= 0.8 * (cs[0] * cs[1]) && r.kind != Kind::WorldObject) {
                Role::Backdrop
            } else if info.rigged {
                Role::Figure
            } else {
                Role::Prop
            };
            let kind = if info.rig_of.is_some() {
                Kind::Part
            } else if info.rigged && matches!(r.kind, Kind::Picture | Kind::Footage) {
                Kind::Puppet
            } else {
                r.kind
            };
            let touches = g.as_ref().map_or([false; 4], |g| g.touches(grid::SOLID));
            items.push(Item {
                id: r.id,
                object: r.object,
                name,
                kind,
                role,
                what: r.what.clone(),
                of: info.rig_of,
                z: i,
                depth: r.depth,
                bounds,
                reach,
                from_alpha,
                grid: g,
                rotation: r.rotation,
                tilt: r.tilt,
                opacity: r.opacity,
                on_frame: grid::share_inside(&r.quad, &frame),
                spill: grid::spill(&box_, &frame),
                touches,
                visible: if from_alpha { 1.0 } else { 0.0 },
                covered_by: Vec::new(),
                covers: Vec::new(),
                lines,
                parts: Vec::new(),
                facing: None,
                labels: Vec::new(),
                quad: r.quad.clone(),
                transition: r.transition,
            });
        }
        // What covers what, among the items that were drawn.
        let drawn_ix: Vec<usize> = (0..items.len()).filter(|&i| items[i].grid.is_some()).collect();
        let covers = grid::occlusion(&drawn_ix.iter().map(|&i| items[i].grid.as_ref().expect("drawn")).collect::<Vec<_>>(), 0.02);
        for (k, c) in covers.iter().enumerate() {
            let i = drawn_ix[k];
            items[i].visible = c.visible;
            items[i].covered_by = c.covered_by.iter().map(|&(j, s)| (drawn_ix[j], s)).collect();
            items[i].covers = c.covers.iter().map(|&(j, s)| (drawn_ix[j], s)).collect();
        }
        let (w, h) = (cs[0], cs[1]);
        let mut lay = Layout { t, w, h, cell, gw, gh, items, camera: None };
        self.layout_rigs(&mut lay);
        self.layout_labels(&mut lay);
        lay.camera = self.layout_camera(t, &camera, cs);
        if lay.items.iter().any(|i| i.depth.is_some()) {
            // The camera only matters when something is in 3D.
        } else {
            lay.camera = None;
        }
        Ok(lay)
    }

    /// The camera 3D things are seen through, in words and numbers.
    fn layout_camera(&self, t: view::Time, cam: &studio_render::space::CameraDesc, cs: [f32; 2]) -> Option<Value> {
        let id = m::space::active_camera(self.doc.model.active(), t).map(|c| c.id.0);
        let fov = 2.0 * (cs[1] * 0.5 / cam.zoom.max(1e-3)).atan().to_degrees();
        let f = cam.axes[2];
        Some(json!({
            "clip": id,
            "position": cam.position.map(|v| (v * 10.0).round() / 10.0),
            "looks": f.map(|v| (v * 100.0).round() / 100.0),
            "vertical_fov_deg": (fov * 10.0).round() / 10.0,
            "zoom": (cam.zoom * 10.0).round() / 10.0,
        }))
    }

    /// Each rig's named parts and the way it faces, at the layout's moment.
    fn layout_rigs(&self, lay: &mut Layout) {
        let t = lay.t;
        for ix in 0..lay.items.len() {
            let (id, kind) = (lay.items[ix].id, lay.items[ix].kind);
            if kind != Kind::Puppet || lay.items[ix].object.is_some() {
                continue;
            }
            let (Some(mc), Some(vc)) = (self.model_clip(id as u32), self.doc.project.clip(id as u32)) else { continue };
            let Some(map) = crate::editor::pins::LayerMap::at(&self.doc.project, &self.doc.model, id as u32, t) else { continue };
            let local = (t - vc.start).clamp(0, vc.dur);
            let at = |p: m::Vec2| {
                let c = map.canvas(p);
                (c.x, c.y)
            };
            let mut parts: Vec<PartAt> = Vec::new();
            let mut add = |parts: &mut Vec<PartAt>, name: String, p: (f32, f32), via: &'static str| {
                if !parts.iter().any(|q| q.name == name) {
                    parts.push(PartAt { name, at: (r1(p.0), r1(p.1)), via });
                }
            };
            let flip = mc.transform_at(local).flip_x;
            let mut facing: Option<bool> = None;
            if let Some(sk) = mc.skeleton_at(local).filter(|s| s.enabled && !s.bones.is_empty()) {
                let posed = sk.forward();
                for b in &sk.bones {
                    let Some(pb) = posed.bone(b.id) else { continue };
                    let n = b.name.to_lowercase();
                    let side = n.split_once('.').map(|(_, s)| format!(".{s}")).unwrap_or_default();
                    let mid = m::Vec2::new((pb.head.x + pb.tail.x) / 2.0, (pb.head.y + pb.tail.y) / 2.0);
                    let (name, p) = if n == "head" || n.starts_with("head.") {
                        ("head".to_owned(), mid)
                    } else if n.contains("forearm") || n.contains("hand") {
                        (format!("hand{side}"), pb.tail)
                    } else if n.contains("shin") || n.contains("foot") || n.contains("feet") || (n.contains("leg") && n.ends_with(".lower")) {
                        (format!("foot{side}"), pb.tail)
                    } else if n == "hips" || n.starts_with("hips") || n.contains("pelvis") {
                        ("hips".to_owned(), mid)
                    } else if n == "torso" || n == "chest" || n == "body" || n.starts_with("spine") {
                        ("torso".to_owned(), mid)
                    } else if n.starts_with("tail") || n.contains("beak") || n.contains("snout") {
                        (n.clone(), pb.tail)
                    } else {
                        continue;
                    };
                    add(&mut parts, name, at(p), "bone");
                }
                facing = sk.faces_right.map(|r| r != flip);
            }
            if let Some(pp) = mc.puppet_at(local).filter(|p| p.enabled) {
                for pin in pp.pins.iter().filter(|p| p.kind == m::PinKind::Position && !p.name.is_empty()) {
                    let n = pin.name.to_lowercase();
                    let n = n.strip_prefix("face ").unwrap_or(&n).trim().to_owned();
                    let name = if n.contains("mouth") && !n.contains("left") && !n.contains("right") { "mouth".to_owned() } else { n.replace(' ', ".") };
                    add(&mut parts, name, at(pin.position), "pin");
                }
                if facing.is_none() {
                    let named = |want: &[&str]| pp.pins.iter().find(|p| want.iter().any(|w| p.name.to_lowercase().contains(w))).map(|p| p.rest.x);
                    if let (Some(head), Some(body)) = (named(&["head", "beak", "face"]), named(&["body", "chest", "back", "hips", "torso"])) {
                        if (head - body).abs() > 0.03 {
                            facing = Some((head > body) != flip);
                        }
                    }
                }
            }
            // Part layers drawn for this rig: where each shows.
            let kids: Vec<(String, (f32, f32))> = lay.items.iter().filter(|p| p.of == Some(id)).filter(|p| p.shows()).map(|p| (p.name.clone(), p.centre())).collect();
            for (n, c) in kids {
                add(&mut parts, n.to_lowercase().replace(' ', "."), (c.0, c.1), "layer");
            }
            lay.items[ix].facing = facing.map(|r| if r { "right" } else { "left" });
            lay.items[ix].parts = parts;
        }
    }

    /// What an object detector found in the picture of each item. Detections
    /// are stored on the clip (`meta.labels`: a list of `{name, box:
    /// [left, top, right, bottom] in 0..1 of the picture, score, source}`);
    /// the `objects` tool writes them there.
    fn layout_labels(&self, lay: &mut Layout) {
        let t = lay.t;
        for it in lay.items.iter_mut().filter(|i| i.object.is_none()) {
            let Some(mc) = self.model_clip(it.id as u32) else { continue };
            let Some(list) = mc.meta.get("labels").and_then(Value::as_array) else { continue };
            let Some(map) = crate::editor::pins::LayerMap::at(&self.doc.project, &self.doc.model, it.id as u32, t) else { continue };
            for l in list.iter().take(40) {
                let Some(b) = l.get("box").and_then(Value::as_array).filter(|b| b.len() == 4) else { continue };
                let n: Vec<f32> = b.iter().filter_map(|v| v.as_f64()).map(|v| v as f32).collect();
                if n.len() != 4 {
                    continue;
                }
                let corners = [(n[0], n[1]), (n[2], n[1]), (n[2], n[3]), (n[0], n[3])].map(|(u, v)| {
                    let c = map.canvas(m::Vec2::new(u as f64, v as f64));
                    (c.x, c.y)
                });
                let xs = corners.iter().map(|p| p.0);
                let ys = corners.iter().map(|p| p.1);
                let canvas = [xs.clone().fold(f32::MAX, f32::min), ys.clone().fold(f32::MAX, f32::min), xs.fold(f32::MIN, f32::max), ys.fold(f32::MIN, f32::max)];
                it.labels.push(Label {
                    name: l.get("name").and_then(Value::as_str).unwrap_or("object").to_owned(),
                    score: l.get("score").and_then(Value::as_f64).unwrap_or(1.0) as f32,
                    source: l.get("source").and_then(Value::as_str).unwrap_or("objects").to_owned(),
                    uv: [n[0], n[1], n[2], n[3]],
                    canvas,
                });
            }
        }
    }

    /// The middle of every drawn thing at `n` moments across `from..to`,
    /// from the placed boxes (no drawing): paths, speeds, and when each
    /// enters or leaves the frame.
    pub(super) fn layout_motion(&self, from: view::Time, to: view::Time, n: usize) -> HashMap<(u64, Option<usize>), Motion> {
        let mut ctx = self.layout_ctx();
        let scene = studio_render::model_scene::ModelScene::new(std::sync::Arc::new(self.doc.model.clone()));
        let n = n.clamp(2, 24);
        let mut tracks: HashMap<(u64, Option<usize>), Vec<(f64, f32, f32, bool)>> = HashMap::new();
        for k in 0..n {
            let t = from + ((to - from) as f64 * k as f64 / (n - 1) as f64) as view::Time;
            let (raws, cs, _) = self.layout_raw(&mut ctx, &scene, t);
            let frame: Rect = [-cs[0] / 2.0, -cs[1] / 2.0, cs[0] / 2.0, cs[1] / 2.0];
            for r in raws {
                let q = &r.quad;
                if q.is_empty() {
                    continue;
                }
                let c = (q.iter().map(|p| p.0).sum::<f32>() / q.len() as f32, q.iter().map(|p| p.1).sum::<f32>() / q.len() as f32);
                let inside = grid::share_inside(q, &frame) > 0.0;
                tracks.entry((r.id, r.object)).or_default().push((secs(t), c.0, c.1, inside));
            }
        }
        tracks
            .into_iter()
            .map(|(key, pts)| {
                let mut travel = 0f32;
                let mut top = 0f32;
                for w in pts.windows(2) {
                    let d = ((w[1].1 - w[0].1).powi(2) + (w[1].2 - w[0].2).powi(2)).sqrt();
                    travel += d;
                    top = top.max(d / (w[1].0 - w[0].0).max(1e-3) as f32);
                }
                let span = (pts.last().map_or(0.0, |p| p.0) - pts.first().map_or(0.0, |p| p.0)).max(1e-3) as f32;
                let enters = pts.windows(2).find(|w| !w[0].3 && w[1].3).map(|w| w[1].0);
                let leaves = pts.windows(2).find(|w| w[0].3 && !w[1].3).map(|w| w[1].0);
                (key, Motion { path: pts.iter().map(|p| (p.0, r1(p.1), r1(p.2))).collect(), travel: r1(travel), avg: r1(travel / span), top: r1(top), enters, leaves })
            })
            .collect()
    }
}

/// A scene with only clip `keep` (and the cameras and lights that place it)
/// drawn, on a see-through canvas.
#[cfg(feature = "render")]
fn alone(model: &m::Project, keep: u64) -> studio_render::model_scene::ModelScene {
    let mut p = model.clone();
    p.settings.background.a = 0.0;
    let active = p.active_sequence;
    if let Some(seq) = p.sequences.iter_mut().find(|s| s.id == active) {
        for c in seq.clips.iter_mut() {
            if c.id.0 != keep && !c.is_camera_or_light() {
                c.enabled = false;
            }
        }
    }
    studio_render::model_scene::ModelScene::new(std::sync::Arc::new(p))
}

/// The objects of a world as items: each box projected through the camera.
#[cfg(feature = "render")]
fn world_objects(w: &studio_render::world_scene::WorldDesc, world_clip: u64, scene_camera: &studio_render::space::CameraDesc, canvas: &studio_render::Canvas, opacity: f32, out: &mut Vec<Raw>) {
    let cam = w.camera.unwrap_or(*scene_camera);
    let k = w.metres_per_px.max(1e-6);
    let cs = canvas.size();
    let mut found: Vec<(f32, Raw)> = Vec::new();
    for (index, o) in w.world.objects.iter().enumerate() {
        let Some(asset) = w.world.assets.get(o.asset) else { continue };
        let (lo, hi) = (asset.bounds.min, asset.bounds.max);
        let (s, c) = o.inst.yaw.to_radians().sin_cos();
        let mut pts: Vec<([f32; 2], f32)> = Vec::new();
        for corner in 0..8 {
            let p = [if corner & 1 == 0 { lo[0] } else { hi[0] }, if corner & 2 == 0 { lo[1] } else { hi[1] }, if corner & 4 == 0 { lo[2] } else { hi[2] }];
            let (x, z) = (p[0] * o.inst.scale, p[2] * o.inst.scale);
            let m = [o.inst.pos[0] + c * x + s * z, o.inst.pos[1] + p[1] * o.inst.scale, o.inst.pos[2] - s * x + c * z];
            // Metres, y up, to canvas pixels, y down.
            pts.push(cam.project(cs, [m[0] / k, -m[1] / k, m[2] / k]));
        }
        // Behind the lens, or partly: not placed.
        if pts.iter().any(|p| p.1 < 1.0) {
            continue;
        }
        let xs = pts.iter().map(|p| p.0[0]);
        let ys = pts.iter().map(|p| p.0[1]);
        let b = [xs.clone().fold(f32::MAX, f32::min), ys.clone().fold(f32::MAX, f32::min), xs.fold(f32::MIN, f32::max), ys.fold(f32::MIN, f32::max)];
        if b[2] < 0.0 || b[0] > cs[0] || b[3] < 0.0 || b[1] > cs[1] {
            continue;
        }
        let centre = cam.view([o.inst.pos[0] / k, -(o.inst.pos[1] + (lo[1] + hi[1]) * 0.5 * o.inst.scale) / k, o.inst.pos[2] / k]);
        let quad = vec![(b[0] - cs[0] / 2.0, b[1] - cs[1] / 2.0), (b[2] - cs[0] / 2.0, b[1] - cs[1] / 2.0), (b[2] - cs[0] / 2.0, b[3] - cs[1] / 2.0), (b[0] - cs[0] / 2.0, b[3] - cs[1] / 2.0)];
        found.push((centre[2], Raw { id: world_clip, object: Some(index), kind: Kind::WorldObject, what: format!("{:?}", o.kind).to_lowercase(), name: Some(o.name.clone()), quad, depth: Some(centre[2]), opacity, rotation: 0.0, tilt: None, full: false, transition: false }));
    }
    // The nearest first, and not a forest.
    found.sort_by(|a, b| a.0.total_cmp(&b.0));
    found.truncate(60);
    // Drawn far to near, like the picture.
    found.reverse();
    out.extend(found.into_iter().map(|f| f.1));
}

#[cfg(not(feature = "render"))]
impl Session {
    pub(super) fn layout_tool(&mut self, _a: &Value) -> Result<String, String> {
        Err("layout needs the renderer, which this build does not have".into())
    }
}

/// Time as seconds in the replies.
pub(super) fn sec(t: view::Time) -> f64 {
    (t as f64 / SEC as f64 * 100.0).round() / 100.0
}

#[cfg(test)]
#[path = "layout_tests.rs"]
mod tests;
