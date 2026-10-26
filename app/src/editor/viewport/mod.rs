// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The 3D workspace: the Player as a space to move through, edit and
//! shoot in, for any project with 3D or 2.5D content.
//!
//! - **The editor's eye** ([`nav::Nav`]) is separate from every scene
//!   camera: orbit, pan, dolly, zoom to the pointer, fly and walk, frame
//!   the selection, the axis views, perspective and orthographic, smooth
//!   moves and saved views (bookmarks). The picture can also be seen
//!   through any camera; a camera can be set to the view, locked to it
//!   while travelling, or given a recorded fly-through as keyframes.
//! - **The world** ([`objects`]) is every model, 3D layer, 2D layer (shown
//!   as a card in space, spread out by stacking order on request), camera
//!   and light at the playhead, from the same geometry the renderer uses,
//!   so a click picks what is drawn there and an outline sits on it.
//! - **Scene editing**: click to select, move/rotate/scale gizmos with
//!   snapping, pivots, world/local/view axes, a floor grid, an outliner
//!   by lane (collections) with visibility and locks, shading modes and a
//!   quad view; everything is a normal undoable edit.
//!
//! The workspace's settings are one per project ([`Workspace`]), saved
//! beside the project as `workspace-3d.json`, and shared by the app and
//! the AI (whose `viewport` tool reads and drives the same state, so the
//! person sees what the AI does).

mod colliders;
pub mod nav;
#[cfg(feature = "render")]
pub mod ui;
#[cfg(feature = "render")]
pub mod world;

use crate::doc::Doc;
use crate::view::{SEC, Time};
use nav::{Axis, Glide, Nav};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::sync::{LazyLock, Mutex};
use studio_model as m;
use studio_model::space as sp;

/// File beside the project holding the workspace.
pub const FILE: &str = "workspace-3d.json";

/// How the viewport draws the world (mirrors the renderer's shading).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shade {
    Solid,
    #[default]
    Material,
    Rendered,
    Wireframe,
    Xray,
}

impl Shade {
    pub const ALL: [Shade; 5] = [Shade::Solid, Shade::Material, Shade::Rendered, Shade::Wireframe, Shade::Xray];

    pub fn name(self) -> &'static str {
        match self {
            Shade::Solid => "solid",
            Shade::Material => "material",
            Shade::Rendered => "rendered",
            Shade::Wireframe => "wireframe",
            Shade::Xray => "xray",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Shade::Solid => "Solid",
            Shade::Material => "Material preview",
            Shade::Rendered => "Rendered",
            Shade::Wireframe => "Wireframe",
            Shade::Xray => "X-ray",
        }
    }

    pub fn parse(s: &str) -> Option<Shade> {
        Some(match s.trim().to_lowercase().replace(['-', ' ', '_'], "").as_str() {
            "solid" | "clay" => Shade::Solid,
            "material" | "materialpreview" | "lookdev" => Shade::Material,
            "rendered" | "render" | "final" => Shade::Rendered,
            "wireframe" | "wire" => Shade::Wireframe,
            "xray" | "seethrough" => Shade::Xray,
            _ => return None,
        })
    }

    #[cfg(feature = "render")]
    pub fn render(self) -> studio_render::viewport::Shading {
        use studio_render::viewport::Shading;
        match self {
            Shade::Solid => Shading::Solid,
            Shade::Material => Shading::Material,
            Shade::Rendered => Shading::Rendered,
            Shade::Wireframe => Shading::Wireframe,
            Shade::Xray => Shading::Xray,
        }
    }
}

/// What the viewport looks through.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "through", content = "camera")]
pub enum Look {
    /// The editor's own eye.
    #[default]
    Editor,
    /// A scene camera: a clip id, or the active camera at the playhead.
    Camera(Option<u64>),
}

/// What a drag on a selected object does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tool {
    Select,
    #[default]
    Move,
    Rotate,
    Scale,
}

impl Tool {
    pub fn parse(s: &str) -> Option<Tool> {
        Some(match s.trim().to_lowercase().as_str() {
            "select" => Tool::Select,
            "move" | "grab" | "translate" => Tool::Move,
            "rotate" | "turn" => Tool::Rotate,
            "scale" | "size" => Tool::Scale,
            _ => return None,
        })
    }
}

/// The axes a gizmo moves along.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Orient {
    #[default]
    World,
    Local,
    View,
    /// The picked faces' own axes (edit mode; objects use their own).
    Normal,
}

impl Orient {
    pub fn parse(s: &str) -> Option<Orient> {
        Some(match s.trim().to_lowercase().as_str() {
            "world" | "global" => Orient::World,
            "local" | "own" => Orient::Local,
            "view" | "screen" => Orient::View,
            "normal" | "surface" => Orient::Normal,
            _ => return None,
        })
    }
}

/// What several selected objects turn and scale about.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pivot {
    /// The middle of the selection.
    #[default]
    Median,
    /// Each about its own anchor.
    Individual,
    /// The 3D cursor.
    Cursor,
    /// The middle of the selection's box.
    Bounds,
    /// The element (or object) picked last.
    Active,
}

impl Pivot {
    pub fn parse(s: &str) -> Option<Pivot> {
        Some(match s.trim().to_lowercase().as_str() {
            "median" | "centre" | "center" | "middle" => Pivot::Median,
            "individual" | "each" | "own" => Pivot::Individual,
            "cursor" | "3d_cursor" => Pivot::Cursor,
            "bounds" | "box" | "bounding_box" => Pivot::Bounds,
            "active" | "last" => Pivot::Active,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Snap {
    pub on: bool,
    /// World pixels.
    pub step: f64,
    /// Degrees.
    pub angle: f64,
    /// Scale fraction (0.1 = 10 %).
    pub scale: f64,
}

impl Default for Snap {
    fn default() -> Snap {
        Snap { on: false, step: 10.0, angle: 15.0, scale: 0.1 }
    }
}

impl Snap {
    pub fn value(v: f64, step: f64) -> f64 {
        if step > 0.0 { (v / step).round() * step } else { v }
    }
}

/// A saved view.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bookmark {
    pub name: String,
    pub nav: Nav,
}

/// A fly-through being recorded: the view at each moment of playback.
#[derive(Clone, Debug, Default)]
pub struct Recording {
    /// The camera that gets the path.
    pub camera: Option<u64>,
    pub samples: Vec<(Time, Nav)>,
}

/// Things that live only while the app runs.
#[derive(Clone, Debug, Default)]
pub struct Runtime {
    /// Whose workspace this is (the project folder), when saved.
    pub owner: Option<std::path::PathBuf>,
    pub loaded: bool,
    pub dirty: bool,
    pub glide: Option<Glide>,
    /// Navigation ended at this time (seconds): the picture refines to
    /// full quality a moment after.
    pub moved_at: f64,
    /// Fly or walk mode (look with the pointer, travel with WASD/QE).
    pub fly: Option<Fly>,
    pub recording: Option<Recording>,
    /// Clips the AI selected for the app to pick up.
    pub select: Option<Vec<u32>>,
    /// Walking in a world with physics (first or third person).
    #[cfg(feature = "render")]
    pub walk: Option<Box<world::WalkState>>,
    /// "Record walk" asked for: start recording once walking.
    pub want_record: bool,
    /// The terrain brush, while sculpting a world.
    #[cfg(feature = "render")]
    pub brush: Option<world::Brush>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fly {
    Fly,
    Walk,
}

/// The 3D workspace of one project.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Workspace {
    /// The Player shows the 3D workspace (otherwise the flat final frame).
    pub on: bool,
    pub look: Look,
    /// The editor's eye (the big view; the perspective pane in quad view).
    pub nav: Nav,
    /// Four views: top, front, right and the eye.
    pub quad: bool,
    /// Top, front and right panes of the quad view.
    pub panes: [Nav; 3],
    /// The quad pane the pointer last used (0..3 the fixed panes, 3 the eye).
    pub active: usize,
    pub shade: Shade,
    /// 2D layers as cards in the world.
    pub cards: bool,
    /// World pixels between neighbouring 2D layers (0: all on the canvas
    /// plane).
    pub spread: f64,
    pub bookmarks: Vec<Bookmark>,
    /// Clips hidden in the viewport only (the final picture keeps them).
    pub hidden: BTreeSet<u64>,
    /// Clips that cannot be picked or moved in the viewport.
    pub locked: BTreeSet<u64>,
    pub grid: bool,
    /// The physics shapes of models (what they collide as) drawn over them.
    pub colliders: bool,
    /// Height of the floor (world y, down positive); default the canvas's
    /// bottom edge.
    pub floor: Option<f64>,
    /// Camera frustums, light shapes and outlines.
    pub overlays: bool,
    /// The canvas frame drawn in the world.
    pub frame: bool,
    /// While looking through a camera, navigating moves the camera.
    pub lock_camera: bool,
    pub tool: Tool,
    pub orient: Orient,
    pub pivot: Pivot,
    pub snap: Snap,
    /// The 3D cursor: where new things go, and a pivot.
    pub cursor: [f64; 3],
    /// Fly and walk speed, canvas pixels per second.
    pub speed: f64,
    #[serde(skip)]
    pub rt: Runtime,
}

impl Default for Workspace {
    fn default() -> Workspace {
        let w = 1920.0;
        let h = 1080.0;
        Workspace::for_canvas(w, h)
    }
}

impl Workspace {
    /// A fresh workspace for a canvas.
    pub fn for_canvas(w: f64, h: f64) -> Workspace {
        let mut top = Nav { ortho: true, ..Nav::front(w) };
        top.set_axis(Axis::Top);
        let mut front = Nav::front(w);
        front.ortho = true;
        let mut right = Nav::front(w);
        right.set_axis(Axis::Right);
        let r = 0.5 * (w * w + h * h).sqrt();
        for n in [&mut top, &mut front, &mut right] {
            n.frame([0.0; 3], r, w, w / h.max(1.0));
        }
        Workspace {
            on: false,
            look: Look::Editor,
            nav: Nav::three_quarter(w, h),
            quad: false,
            panes: [top, front, right],
            active: 3,
            shade: Shade::Material,
            cards: true,
            spread: 0.0,
            bookmarks: Vec::new(),
            hidden: BTreeSet::new(),
            locked: BTreeSet::new(),
            grid: true,
            colliders: false,
            floor: None,
            overlays: true,
            frame: true,
            lock_camera: false,
            tool: Tool::Move,
            orient: Orient::World,
            pivot: Pivot::Median,
            snap: Snap::default(),
            cursor: [0.0; 3],
            speed: w * 0.6,
            rt: Runtime::default(),
        }
    }

    pub fn changed(&mut self) {
        self.rt.dirty = true;
    }

    /// The view being driven: the pane in use in quad view, else the eye.
    pub fn nav_mut(&mut self) -> &mut Nav {
        if self.quad && self.active < 3 { &mut self.panes[self.active] } else { &mut self.nav }
    }

    /// Glides the eye to `to` over a quarter of a second.
    pub fn glide_to(&mut self, to: Nav, now: f64) {
        let from = *self.nav_mut();
        self.rt.glide = Some(Glide { from, to, start: now, seconds: 0.25 });
        self.changed();
    }

    /// Advances a glide; returns whether one is running.
    pub fn step_glide(&mut self, now: f64) -> bool {
        let Some(g) = self.rt.glide else { return false };
        let (n, done) = g.at(now);
        *self.nav_mut() = n;
        self.rt.moved_at = now;
        if done {
            self.rt.glide = None;
        }
        true
    }

    pub fn floor_y(&self, height: f64) -> f64 {
        self.floor.unwrap_or(height * 0.5)
    }
}

static WS: LazyLock<Mutex<Workspace>> = LazyLock::new(|| Mutex::new(Workspace::default()));

/// Works on the workspace of the project open now.
pub fn with<R>(f: impl FnOnce(&mut Workspace) -> R) -> R {
    let mut g = WS.lock().unwrap_or_else(|e| e.into_inner());
    f(&mut g)
}

/// A copy of the workspace.
pub fn snapshot() -> Workspace {
    with(|w| w.clone())
}

fn owner_of(doc: &Doc) -> Option<std::path::PathBuf> {
    doc.folder.as_ref().map(|f| f.root().to_path_buf())
}

/// Makes the workspace the one of `doc`'s project: read from beside the
/// project when it changed (a new project starts fresh).
pub fn follow(doc: &Doc) {
    let owner = owner_of(doc);
    with(|w| {
        if w.rt.loaded && w.rt.owner == owner {
            return;
        }
        let (cw, ch) = (doc.project.width.max(1) as f64, doc.project.height.max(1) as f64);
        let read = owner.as_ref().and_then(|o| std::fs::read_to_string(o.join(FILE)).ok()).and_then(|s| serde_json::from_str::<Workspace>(&s).ok());
        *w = read.unwrap_or_else(|| Workspace::for_canvas(cw, ch));
        w.rt.owner = owner;
        w.rt.loaded = true;
    });
}

/// Writes the workspace beside the project when it changed.
pub fn save(doc: &Doc) {
    let Some(owner) = owner_of(doc) else { return };
    let text = with(|w| {
        if !w.rt.dirty || w.rt.owner.as_ref() != Some(&owner) {
            return None;
        }
        w.rt.dirty = false;
        serde_json::to_string_pretty(w).ok()
    });
    if let Some(t) = text {
        let _ = std::fs::write(owner.join(FILE), t);
    }
}

/// Whether the project has anything that lives in space (a 3D layer, a
/// model, a camera or a light): the Player then offers the workspace.
pub fn has_space(doc: &Doc) -> bool {
    doc.model.active().clips.iter().any(|c| c.transform.three_d || matches!(&c.source, m::ClipSource::Generator(g) if matches!(g.kind, m::GeneratorKind::Camera | m::GeneratorKind::Light | m::GeneratorKind::Model)))
}

/// The editor view as a scene view, for tools that draw or aim from it
/// (sculpting, modeling, placing): `None` while the workspace is off.
#[allow(dead_code)] // the hook for the modeling tools
pub fn current_view(width: f64) -> Option<sp::View> {
    with(|w| (w.on && w.look == Look::Editor).then(|| w.nav.view(width)))
}

// ------------------------------------------------------------------ world

/// What an object in the world is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A layer in 3D.
    Layer,
    /// A 2D layer shown as a card in space.
    Card,
    Model,
    Camera,
    Light,
}

/// One thing in the world at a moment.
#[derive(Clone, Debug)]
pub struct Object {
    pub id: u64,
    pub name: String,
    pub kind: Kind,
    /// Where its anchor is.
    pub anchor: [f64; 3],
    /// A layer's or card's picture corners (top left, top right, bottom
    /// right, bottom left).
    pub corners: Option<[[f64; 3]; 4]>,
    /// World bounds.
    pub min: [f64; 3],
    pub max: [f64; 3],
    /// Its own axes (columns of its rotation): right, down, back.
    pub axes: sp::Mat3,
    /// A camera's view, a light's aim point.
    pub view: Option<sp::View>,
    pub aim: Option<[f64; 3]>,
    /// A model's file and model-to-world matrix (rows of 3x4).
    pub model: Option<(String, [[f64; 4]; 3])>,
}

fn bounds_of(pts: &[[f64; 3]]) -> ([f64; 3], [f64; 3]) {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for p in pts {
        for i in 0..3 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    (lo, hi)
}

/// A model's model-to-world matrix, as the renderer places it.
pub fn model_matrix(tr: &m::Transform) -> [[f64; 4]; 3] {
    let r = sp::rotation(tr.orientation, tr.rotation_x, tr.rotation_y, tr.rotation);
    let k = [tr.scale_x * if tr.flip_x { -1.0 } else { 1.0 }, tr.scale_y * if tr.flip_y { -1.0 } else { 1.0 }, tr.scale_z];
    let pos = [tr.position.x, tr.position.y, tr.position_z];
    let anchor = [tr.anchor.x, tr.anchor.y, tr.anchor_z];
    std::array::from_fn(|row| {
        let mm = [r[row][0] * k[0], r[row][1] * k[1], r[row][2] * k[2]];
        [mm[0], mm[1], mm[2], pos[row] - (mm[0] * anchor[0] + mm[1] * anchor[1] + mm[2] * anchor[2])]
    })
}

pub fn xform(mx: &[[f64; 4]; 3], p: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|r| mx[r][0] * p[0] + mx[r][1] * p[1] + mx[r][2] * p[2] + mx[r][3])
}

/// The bounds of a model file in its own pixels.
#[cfg(feature = "render")]
pub fn mesh_of(file: &str) -> Option<std::sync::Arc<studio_mesh::Mesh>> {
    studio_render::mesh_render::load(file)
}

/// Everything in the world at `t` (hidden clips left out), with each 2D
/// layer's card depth from `depths` (`None`: 2D layers are not in the
/// world).
pub fn objects(doc: &Doc, t: Time, depths: Option<&HashMap<u64, f32>>, hidden: &BTreeSet<u64>) -> Vec<Object> {
    let seq = doc.model.active();
    let w = doc.project.width.max(1) as f64;
    let h = doc.project.height.max(1) as f64;
    let hidden_lanes: BTreeSet<u64> = seq.lanes.iter().filter(|l| l.hidden).map(|l| l.id.0).collect();
    let mut out = Vec::new();
    for mc in &seq.clips {
        if t < mc.start || t >= mc.start + mc.duration.max(1) || hidden.contains(&mc.id.0) || hidden_lanes.contains(&mc.lane.0) {
            continue;
        }
        let gen_kind = match &mc.source {
            m::ClipSource::Generator(g) => Some(g.kind),
            _ => None,
        };
        let base = |kind: Kind, anchor: [f64; 3]| Object { id: mc.id.0, name: mc.name.clone(), kind, anchor, corners: None, min: anchor, max: anchor, axes: sp::rotation([0.0; 3], 0.0, 0.0, 0.0), view: None, aim: None, model: None };
        match gen_kind {
            Some(m::GeneratorKind::Camera) => {
                let v = sp::camera_view(seq, mc, t, w);
                let mut o = base(Kind::Camera, v.position);
                let s = sp::CameraSettings::read(&mc.generator_params_at((t - mc.start).clamp(0, mc.duration)), w);
                o.aim = s.two_node.then_some(s.poi);
                o.axes = sp::transpose(&v.axes);
                // The frustum out to a fifth of a lens is its size.
                let k = 0.2;
                let pts: Vec<[f64; 3]> = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]]
                    .iter()
                    .map(|c| {
                        let local = [c[0] * w * 0.5 * k, c[1] * h * 0.5 * k, v.zoom * k];
                        let wv = sp::apply(&sp::transpose(&v.axes), local);
                        [v.position[0] + wv[0], v.position[1] + wv[1], v.position[2] + wv[2]]
                    })
                    .chain(std::iter::once(v.position))
                    .collect();
                (o.min, o.max) = bounds_of(&pts);
                o.view = Some(v);
                out.push(o);
            }
            Some(m::GeneratorKind::Light) => {
                let tr = seq.placed_transform(mc, t);
                let s = sp::LightSettings::read(&mc.generator_params_at((t - mc.start).clamp(0, mc.duration)));
                let mut o = base(Kind::Light, [tr.position.x, tr.position.y, tr.position_z]);
                o.aim = matches!(s.kind, sp::LightKind::Spot | sp::LightKind::Parallel).then_some(s.poi);
                let r = w * 0.02;
                o.min = [o.anchor[0] - r, o.anchor[1] - r, o.anchor[2] - r];
                o.max = [o.anchor[0] + r, o.anchor[1] + r, o.anchor[2] + r];
                out.push(o);
            }
            Some(m::GeneratorKind::Adjustment) => {}
            Some(m::GeneratorKind::Model) => {
                let tr = sp::oriented_transform(seq, mc, t, w);
                let mx = model_matrix(&tr);
                let mut o = base(Kind::Model, [tr.position.x, tr.position.y, tr.position_z]);
                o.axes = sp::rotation(tr.orientation, tr.rotation_x, tr.rotation_y, tr.rotation);
                let file = crate::mesh_store::file_of(mc).unwrap_or_default();
                #[cfg(feature = "render")]
                let local = mesh_of(&file).map(|mesh| mesh.bounds());
                #[cfg(not(feature = "render"))]
                let local: Option<([f32; 3], [f32; 3])> = None;
                let (lo, hi) = local.map(|(a, b)| (a.map(|v| v as f64), b.map(|v| v as f64))).unwrap_or(([-50.0; 3], [50.0; 3]));
                let corners: Vec<[f64; 3]> = (0..8).map(|i| xform(&mx, [if i & 1 == 0 { lo[0] } else { hi[0] }, if i & 2 == 0 { lo[1] } else { hi[1] }, if i & 4 == 0 { lo[2] } else { hi[2] }])).collect();
                (o.min, o.max) = bounds_of(&corners);
                o.model = Some((file, mx));
                out.push(o);
            }
            _ => {
                let three_d = mc.transform.three_d;
                let depth = depths.and_then(|d| d.get(&mc.id.0).copied());
                if !three_d && depths.is_none() {
                    continue;
                }
                let Some(g) = super::xform::geo(&doc.project, &doc.model, mc.id.0 as u32, t) else { continue };
                let plane = match g.space {
                    Some(s) => s.plane,
                    None => {
                        let driven = seq.driven(mc, t);
                        let mut tr = seq.placed_transform(&driven, t);
                        tr.position_z = depth.unwrap_or(0.0) as f64;
                        tr.rotation_x = 0.0;
                        tr.rotation_y = 0.0;
                        tr.orientation = [0.0; 3];
                        sp::Plane::of(&tr, m::Vec2::new(g.source().x as f64, g.source().y as f64), g.fit() as f64)
                    }
                };
                let corners: [[f64; 3]; 4] = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)].map(|(u, v)| {
                    let p = g.content_point(u, v);
                    plane.at(p.x as f64, p.y as f64)
                });
                let anchor = match g.space {
                    Some(s) => s.anchor,
                    None => [g.xf.pos.x as f64, g.xf.pos.y as f64, depth.unwrap_or(0.0) as f64],
                };
                let mut o = base(if three_d { Kind::Layer } else { Kind::Card }, anchor);
                let n = |a: [f64; 3]| {
                    let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt().max(1e-12);
                    a.map(|v| v / l)
                };
                let ex = n([corners[1][0] - corners[0][0], corners[1][1] - corners[0][1], corners[1][2] - corners[0][2]]);
                let ey = n([corners[3][0] - corners[0][0], corners[3][1] - corners[0][1], corners[3][2] - corners[0][2]]);
                let ez = [ex[1] * ey[2] - ex[2] * ey[1], ex[2] * ey[0] - ex[0] * ey[2], ex[0] * ey[1] - ex[1] * ey[0]];
                o.axes = [[ex[0], ey[0], ez[0]], [ex[1], ey[1], ez[1]], [ex[2], ey[2], ez[2]]];
                (o.min, o.max) = bounds_of(&corners);
                o.corners = Some(corners);
                out.push(o);
            }
        }
    }
    out
}

/// Where a ray first meets a triangle (Möller–Trumbore): distance along it.
fn ray_tri(o: [f64; 3], d: [f64; 3], a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> Option<f64> {
    let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let p = [d[1] * e2[2] - d[2] * e2[1], d[2] * e2[0] - d[0] * e2[2], d[0] * e2[1] - d[1] * e2[0]];
    let det = e1[0] * p[0] + e1[1] * p[1] + e1[2] * p[2];
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let s = [o[0] - a[0], o[1] - a[1], o[2] - a[2]];
    let u = (s[0] * p[0] + s[1] * p[1] + s[2] * p[2]) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = [s[1] * e1[2] - s[2] * e1[1], s[2] * e1[0] - s[0] * e1[2], s[0] * e1[1] - s[1] * e1[0]];
    let v = (d[0] * q[0] + d[1] * q[1] + d[2] * q[2]) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = (e2[0] * q[0] + e2[1] * q[1] + e2[2] * q[2]) * inv;
    (t > 0.0).then_some(t)
}

fn ray_box(o: [f64; 3], d: [f64; 3], lo: [f64; 3], hi: [f64; 3]) -> bool {
    let (mut t0, mut t1) = (0.0f64, f64::INFINITY);
    for i in 0..3 {
        if d[i].abs() < 1e-12 {
            if o[i] < lo[i] || o[i] > hi[i] {
                return false;
            }
            continue;
        }
        let (a, b) = ((lo[i] - o[i]) / d[i], (hi[i] - o[i]) / d[i]);
        t0 = t0.max(a.min(b));
        t1 = t1.min(a.max(b));
    }
    t0 <= t1
}

/// The object under a screen point (canvas pixels from the centre of the
/// view) and how far along the ray it was hit; cameras and lights within
/// `slop` screen pixels of their icon count. Locked objects are skipped.
pub fn pick(objs: &[Object], view: &sp::View, at: [f64; 2], slop: f64, locked: &BTreeSet<u64>) -> Option<(u64, f64)> {
    let o = view.position;
    let d0 = view.ray(m::Vec2::new(at[0], at[1]));
    let n = (d0[0] * d0[0] + d0[1] * d0[1] + d0[2] * d0[2]).sqrt().max(1e-12);
    let d = d0.map(|v| v / n);
    let mut best: Option<(u64, f64)> = None;
    let mut keep = |id: u64, t: f64| {
        if best.is_none_or(|b| t < b.1) {
            best = Some((id, t));
        }
    };
    for ob in objs.iter().filter(|ob| !locked.contains(&ob.id)) {
        match ob.kind {
            Kind::Camera | Kind::Light => {
                if let Some((c, depth)) = view.project(ob.anchor) {
                    let dist = ((c.x - at[0]).powi(2) + (c.y - at[1]).powi(2)).sqrt();
                    if dist <= slop {
                        keep(ob.id, depth);
                    }
                }
            }
            Kind::Layer | Kind::Card => {
                if let Some(c) = ob.corners {
                    if let Some(t) = ray_tri(o, d, c[0], c[1], c[2]).or_else(|| ray_tri(o, d, c[0], c[2], c[3])) {
                        keep(ob.id, t);
                    }
                }
            }
            Kind::Model => {
                if !ray_box(o, d, ob.min, ob.max) {
                    continue;
                }
                let mut hit: Option<f64> = None;
                #[cfg(feature = "render")]
                if let Some((file, mx)) = &ob.model
                    && let Some(mesh) = mesh_of(file)
                {
                    let p = |i: usize| xform(mx, mesh.positions[i].map(|v| v as f64));
                    for t in 0..mesh.triangles() {
                        let [a, b, c] = mesh.tri(t);
                        if let Some(x) = ray_tri(o, d, p(a), p(b), p(c)) {
                            hit = Some(hit.map_or(x, |h: f64| h.min(x)));
                        }
                    }
                }
                #[cfg(not(feature = "render"))]
                {
                    hit = Some(0.0);
                }
                if let Some(t) = hit {
                    keep(ob.id, t);
                }
            }
        }
    }
    best
}

/// The middle and size of some objects (all of them when `ids` is empty).
pub fn extent(objs: &[Object], ids: &[u64]) -> Option<([f64; 3], f64)> {
    let pts: Vec<[f64; 3]> = objs.iter().filter(|o| ids.is_empty() || ids.contains(&o.id)).flat_map(|o| [o.min, o.max]).collect();
    if pts.is_empty() {
        return None;
    }
    let (lo, hi) = bounds_of(&pts);
    let c = std::array::from_fn(|i| 0.5 * (lo[i] + hi[i]));
    let d: [f64; 3] = std::array::from_fn(|i| hi[i] - lo[i]);
    Some((c, (0.5 * (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()).max(20.0)))
}

// ------------------------------------------------------------ the camera

/// A new two-node camera on a lane of its own at the top, over the whole
/// sequence, where the default camera stands (one undo step).
pub fn add_camera(doc: &mut Doc) -> Option<u64> {
    let w = doc.project.width as f64;
    let seq = doc.seq();
    let lane = doc.alloc_id();
    let id = doc.alloc_id();
    let mut p = sp::CameraSettings::defaults(w);
    p.insert("two_node".into(), m::Value::Bool(true));
    let mut g = m::Generator::new(m::GeneratorKind::Camera);
    g.params = p;
    let dur = doc.project.duration().max(SEC);
    let mut c = m::Clip::new(m::ClipId(id), m::LaneId(lane), 0, dur, m::ClipSource::Generator(g));
    c.name = "Camera".into();
    c.transform.position_z = -sp::default_zoom(w);
    let edits = vec![m::Edit::AddLane { seq, lane: m::Lane::new(m::LaneId(lane), m::LaneKind::Visual, "Camera"), index: Some(0) }, m::Edit::InsertClip { seq, clip: Box::new(c) }];
    doc.apply("Add camera", m::Edit::Batch { edits }).then_some(id)
}

/// Tests that use the shared workspace take turns, and leave it off.
#[cfg(test)]
pub struct TestTurn(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);

#[cfg(test)]
impl Drop for TestTurn {
    fn drop(&mut self) {
        with(|w| *w = Workspace::default());
    }
}

#[cfg(test)]
pub fn test_lock() -> TestTurn {
    static L: Mutex<()> = Mutex::new(());
    TestTurn(L.lock().unwrap_or_else(|e| e.into_inner()))
}

fn frame_of(doc: &Doc) -> Time {
    (SEC as f64 / (doc.project.fps as f64).max(1.0)) as Time
}

/// The camera clip seen through (the active one when none is named).
pub fn look_camera(doc: &Doc, look: Look, t: Time) -> Option<u64> {
    match look {
        Look::Camera(Some(id)) => doc.model.active().clip(m::ClipId(id)).filter(|c| matches!(&c.source, m::ClipSource::Generator(g) if g.kind == m::GeneratorKind::Camera)).map(|c| c.id.0),
        _ => sp::active_camera(doc.model.active(), t).map(|c| c.id.0),
    }
}

/// A camera's eye at `t` (its point of interest is the target when it
/// has one).
pub fn camera_nav(doc: &Doc, id: u64, t: Time) -> Option<Nav> {
    let seq = doc.model.active();
    let mc = seq.clip(m::ClipId(id))?;
    let w = doc.project.width as f64;
    let v = sp::camera_view(seq, mc, t, w);
    let s = sp::CameraSettings::read(&mc.generator_params_at((t - mc.start).clamp(0, mc.duration)), w);
    Some(Nav::from_view(&v, w, s.two_node.then_some(s.poi)))
}

/// The camera clip changed so it sees what `nav` sees at `t`: aimed (two
/// node) at the view's target, the same lens; keyed when animated.
pub fn camera_to(mc: &m::Clip, nav: &Nav, t: Time, width: f64, frame: Time) -> m::Clip {
    let mut mc = mc.clone();
    let local = (t - mc.start).clamp(0, mc.duration);
    let pos = nav.position();
    if let m::ClipSource::Generator(g) = &mut mc.source {
        g.params.insert("two_node".into(), m::Value::Bool(true));
    }
    let put = super::xform::put;
    put(&mut mc, "transform.position", m::Value::Point(m::Vec2::new(pos[0], pos[1])), local, frame);
    put(&mut mc, "transform.position_z", m::Value::Scalar(pos[2]), local, frame);
    put(&mut mc, "generator.poi", m::Value::Point(m::Vec2::new(nav.target[0], nav.target[1])), local, frame);
    put(&mut mc, "generator.poi_z", m::Value::Scalar(nav.target[2]), local, frame);
    put(&mut mc, "generator.zoom", m::Value::Scalar(nav.zoom(width)), local, frame);
    // A camera aimed at a point has no own tilt or pan.
    for p in ["transform.rotation_x", "transform.rotation_y", "transform.orientation_x", "transform.orientation_y"] {
        if mc.keyframes.get(p).is_none_or(|k| k.keys.is_empty()) {
            put(&mut mc, p, m::Value::Angle(0.0), local, frame);
        }
    }
    mc
}

/// Sets a camera to the view (one undo step, `gesture` merges a drag).
pub fn set_camera_to_view(doc: &mut Doc, id: u64, nav: &Nav, t: Time, gesture: Option<&str>) -> Result<(), String> {
    let mc = doc.model.active().clip(m::ClipId(id)).cloned().ok_or("no such camera")?;
    if !matches!(&mc.source, m::ClipSource::Generator(g) if g.kind == m::GeneratorKind::Camera) {
        return Err(format!("clip {id} is not a camera"));
    }
    let next = camera_to(&mc, nav, t, doc.project.width as f64, frame_of(doc));
    let seq = doc.seq();
    let edit = m::Edit::SetClip { seq, clip: Box::new(next) };
    let ok = match gesture {
        Some(g) => doc.apply_gesture(g, "Camera to view", edit),
        None => doc.apply("Camera to view", edit),
    };
    if ok { Ok(()) } else { Err(doc.error.clone().unwrap_or_else(|| "the camera could not be changed".into())) }
}

/// Keys a recorded fly-through into a camera: position and aim at each
/// kept moment (linear between them, about every other frame), one undo
/// step. Returns the number of keys.
pub fn record_path(doc: &mut Doc, id: u64, samples: &[(Time, Nav)]) -> Result<usize, String> {
    let mut mc = doc.model.active().clip(m::ClipId(id)).cloned().ok_or("no such camera")?;
    if samples.is_empty() {
        return Err("nothing was recorded".into());
    }
    let every = frame_of(doc) * 2;
    let mut kept: Vec<(Time, Nav)> = Vec::new();
    for s in samples {
        if kept.last().is_none_or(|k| s.0 - k.0 >= every) {
            kept.push(*s);
        }
    }
    if let Some(last) = samples.last()
        && kept.last().is_some_and(|k| k.0 != last.0)
    {
        kept.push(*last);
    }
    if let m::ClipSource::Generator(g) = &mut mc.source {
        g.params.insert("two_node".into(), m::Value::Bool(true));
    }
    let (from, to) = (kept[0].0, kept[kept.len() - 1].0);
    // A path longer than the camera's clip lengthens it.
    if to >= mc.start + mc.duration {
        mc.duration = to - mc.start + frame_of(doc);
    }
    for p in ["transform.position", "transform.position_z", "generator.poi", "generator.poi_z"] {
        // The recorded stretch replaces what was keyed there.
        if let Some(tr) = mc.keyframes.get_mut(p) {
            tr.keys.retain(|k| k.time + mc.start < from || k.time + mc.start > to);
        }
    }
    for (t, nav) in &kept {
        let local = (t - mc.start).clamp(0, mc.duration);
        let pos = nav.position();
        let vals = [
            ("transform.position", m::Value::Point(m::Vec2::new(pos[0], pos[1]))),
            ("transform.position_z", m::Value::Scalar(pos[2])),
            ("generator.poi", m::Value::Point(m::Vec2::new(nav.target[0], nav.target[1]))),
            ("generator.poi_z", m::Value::Scalar(nav.target[2])),
        ];
        for (p, v) in vals {
            let track = mc.keyframes.entry(p.to_owned()).or_default();
            track.keys.retain(|k| k.time != local);
            track.keys.push(m::Keyframe::new(local, v, m::Interp::Linear));
            track.keys.sort_by_key(|k| k.time);
        }
    }
    let seq = doc.seq();
    if !doc.apply("Record camera path", m::Edit::SetClip { seq, clip: Box::new(mc) }) {
        return Err(doc.error.clone().unwrap_or_else(|| "the path could not be keyed".into()));
    }
    Ok(kept.len())
}

// ----------------------------------------------------------- moving things

/// A change to objects in world terms.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Change {
    /// World pixels.
    pub moved: [f64; 3],
    /// Degrees about each world axis (applied X, then Y, then Z) round
    /// the pivot.
    pub turned: [f64; 3],
    /// Then degrees about any axis (a unit world direction).
    pub turn_about: Option<([f64; 3], f64)>,
    /// Scale factors along x, y, z (1 = unchanged).
    pub scaled: Option<[f64; 3]>,
}

/// Turning by `deg` about a world axis (Rodrigues, right-handed: the
/// sense of `rot_z`; the X and Y rotation properties turn the other way,
/// so `about(x, d)` is `rot_x(-d)`).
pub fn about(axis: [f64; 3], deg: f64) -> sp::Mat3 {
    let l = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt().max(1e-12);
    let k = axis.map(|v| v / l);
    let (s, c) = deg.to_radians().sin_cos();
    let t = 1.0 - c;
    [
        [t * k[0] * k[0] + c, t * k[0] * k[1] - s * k[2], t * k[0] * k[2] + s * k[1]],
        [t * k[0] * k[1] + s * k[2], t * k[1] * k[1] + c, t * k[1] * k[2] - s * k[0]],
        [t * k[0] * k[2] - s * k[1], t * k[1] * k[2] + s * k[0], t * k[2] * k[2] + c],
    ]
}

/// Turning by `deg` about world axis `i`.
fn axis_rot(i: usize, deg: f64) -> sp::Mat3 {
    match i {
        0 => sp::rot_x(deg),
        1 => sp::rot_y(deg),
        _ => sp::rot_z(deg),
    }
}

/// Moves, turns or scales objects (each keyed at `t` where animated) as
/// one step. With a `pivot`, turning and scaling also carry each object
/// round it; without, each turns and scales about its own anchor. A 2D
/// layer turned or moved in depth becomes a 3D layer. Locked objects are
/// left alone.
pub fn change(doc: &mut Doc, ids: &[u64], t: Time, c: &Change, pivot: Option<[f64; 3]>, label: &str, gesture: Option<&str>) -> Result<usize, String> {
    let seq_id = doc.seq();
    let frame = frame_of(doc);
    let locked = with(|w| w.locked.clone());
    let seq = doc.model.active().clone();
    let xyz = sp::mul(&sp::mul(&axis_rot(2, c.turned[2]), &axis_rot(1, c.turned[1])), &axis_rot(0, c.turned[0]));
    let turn = match c.turn_about {
        Some((ax, deg)) => sp::mul(&about(ax, deg), &xyz),
        None => xyz,
    };
    let turning = c.turned.iter().any(|d| d.abs() > 1e-9) || c.turn_about.is_some_and(|a| a.1.abs() > 1e-9);
    let mut edits = Vec::new();
    for id in ids {
        if locked.contains(id) {
            continue;
        }
        let Some(mut mc) = seq.clip(m::ClipId(*id)).cloned() else { continue };
        let local = (t - mc.start).clamp(0, mc.duration);
        let own = mc.transform_at(local);
        let placed = seq.placed_transform(&mc, t);
        // World offset applied to the clip's own position (a parent's
        // carry is the same before and after).
        let at = [placed.position.x, placed.position.y, placed.position_z];
        let mut delta = c.moved;
        if let Some(p) = pivot {
            let rel = [at[0] - p[0], at[1] - p[1], at[2] - p[2]];
            if turning {
                let r = sp::apply(&turn, rel);
                for i in 0..3 {
                    delta[i] += r[i] - rel[i];
                }
            }
            if let Some(k) = c.scaled {
                for i in 0..3 {
                    delta[i] += rel[i] * (k[i] - 1.0);
                }
            }
        }
        let is_cam_or_light = mc.is_camera_or_light();
        let put = super::xform::put;
        if delta.iter().any(|d| d.abs() > 1e-9) {
            put(&mut mc, "transform.position", m::Value::Point(m::Vec2::new(own.position.x + delta[0], own.position.y + delta[1])), local, frame);
            put(&mut mc, "transform.position_z", m::Value::Scalar(own.position_z + delta[2]), local, frame);
            if delta[2].abs() > 1e-9 || is_cam_or_light {
                mc.transform.three_d = mc.transform.three_d || !is_cam_or_light;
            }
            // An aimed camera or light keeps looking the same way.
            if is_cam_or_light && mc.get_property("generator.poi").is_some() && !turning {
                let p = match mc.property_at("generator.poi", local) {
                    Some(m::Value::Point(q)) => q,
                    _ => m::Vec2::new(0.0, 0.0),
                };
                let pz = mc.property_at("generator.poi_z", local).and_then(|v| v.as_f64()).unwrap_or(0.0);
                put(&mut mc, "generator.poi", m::Value::Point(m::Vec2::new(p.x + delta[0], p.y + delta[1])), local, frame);
                put(&mut mc, "generator.poi_z", m::Value::Scalar(pz + delta[2]), local, frame);
            }
        }
        if turning && !is_cam_or_light {
            // The new rotation: the turn on top of the current one, back
            // into the clip's X, Y, Z angles (orientation kept).
            let o = sp::mul(&sp::mul(&sp::rot_x(own.orientation[0]), &sp::rot_y(own.orientation[1])), &sp::rot_z(own.orientation[2]));
            let cur = sp::rotation(own.orientation, own.rotation_x, own.rotation_y, own.rotation);
            let next = sp::mul(&turn, &cur);
            let rest = sp::mul(&sp::transpose(&o), &next);
            let e = sp::euler(&rest);
            put(&mut mc, "transform.rotation_x", m::Value::Angle(e[0]), local, frame);
            put(&mut mc, "transform.rotation_y", m::Value::Angle(e[1]), local, frame);
            put(&mut mc, "transform.rotation", m::Value::Angle(e[2]), local, frame);
            if e[0].abs() > 1e-6 || e[1].abs() > 1e-6 {
                mc.transform.three_d = true;
            }
        }
        if let Some(k) = c.scaled
            && !is_cam_or_light
        {
            put(&mut mc, "transform.scale_x", m::Value::Scalar(own.scale_x * k[0]), local, frame);
            put(&mut mc, "transform.scale_y", m::Value::Scalar(own.scale_y * k[1]), local, frame);
            put(&mut mc, "transform.scale_z", m::Value::Scalar(own.scale_z * k[2]), local, frame);
        }
        edits.push(m::Edit::SetClip { seq: seq_id, clip: Box::new(mc) });
    }
    let count = edits.len();
    if count == 0 {
        return Err("nothing to change (locked or not found)".into());
    }
    let edit = m::Edit::Batch { edits };
    let ok = match gesture {
        Some(g) => doc.apply_gesture(g, label, edit),
        None => doc.apply(label, edit),
    };
    if ok { Ok(count) } else { Err(doc.error.clone().unwrap_or_else(|| "the change was refused".into())) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_workspace_round_trips_through_its_file() {
        let mut w = Workspace::for_canvas(1920.0, 1080.0);
        w.on = true;
        w.shade = Shade::Xray;
        w.look = Look::Camera(Some(42));
        w.bookmarks.push(Bookmark { name: "wide".into(), nav: Nav::front(1920.0) });
        w.hidden.insert(7);
        let s = serde_json::to_string(&w).unwrap();
        let back: Workspace = serde_json::from_str(&s).unwrap();
        assert!(back.on);
        assert_eq!(back.shade, Shade::Xray);
        assert_eq!(back.look, Look::Camera(Some(42)));
        assert_eq!(back.bookmarks, w.bookmarks);
        assert!(back.hidden.contains(&7));
        // Old files missing fields still load.
        let thin: Workspace = serde_json::from_str("{\"on\": true}").unwrap();
        assert!(thin.on && thin.grid);
    }

    #[test]
    fn turning_about_an_axis_agrees_with_the_basic_turns() {
        for (i, ax) in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]].into_iter().enumerate() {
            let a = about(ax, 33.0);
            let b = axis_rot(i, if i < 2 { -33.0 } else { 33.0 });
            for r in 0..3 {
                for c in 0..3 {
                    assert!((a[r][c] - b[r][c]).abs() < 1e-9, "axis {i}");
                }
            }
        }
    }

    #[test]
    fn rays_hit_triangles_and_boxes() {
        let o = [0.0, 0.0, -10.0];
        let d = [0.0, 0.0, 1.0];
        assert_eq!(ray_tri(o, d, [-1.0, -1.0, 0.0], [1.0, -1.0, 0.0], [0.0, 1.0, 0.0]), Some(10.0));
        assert_eq!(ray_tri(o, d, [2.0, 2.0, 0.0], [3.0, 2.0, 0.0], [2.0, 3.0, 0.0]), None);
        assert!(ray_box(o, d, [-1.0; 3], [1.0; 3]));
        assert!(!ray_box(o, [1.0, 0.0, 0.0], [-1.0; 3], [1.0; 3]));
    }
}
