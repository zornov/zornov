// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What the renderer draws: the [`Scene`] trait and the plain data types a
//! scene hands over for one instant in time.
//!
//! A scene is anything that can say what is visible at time `t`: the
//! project model (through the `model` feature's adapter), a test fixture,
//! or a hover preview built by hand. It gives the renderer clips with all
//! their animated values already evaluated at `t`; the renderer then
//! resolves stacking, transitions, motion presets and nested scenes, and
//! draws.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use crate::blend::BlendMode;
use crate::mask::MaskSpec;
use crate::output::OutputSpace;
use crate::puppet::WarpDesc;
use crate::space::{CameraDesc, LightDesc, Space};
use crate::transform::Placement;

/// Microseconds, as in the project model.
pub type Micros = i64;

/// One second in [`Micros`].
pub const SECOND: Micros = 1_000_000;

/// Identifies a media file for the source provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MediaKey(pub u64);

/// A cut-out matte: the matte source's picture for the clip picture's own
/// frame index multiplies its alpha (see [`crate::matte`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MatteDesc {
    /// The matte pictures (registered under [`matte_key`]).
    pub media: MediaKey,
    /// Keep what the matte leaves out instead (remove the subject).
    pub invert: bool,
}

/// The key a matte file is registered under: a hash of its path, in a
/// range no media id reaches.
pub fn matte_key(file: &str) -> MediaKey {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in file.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    MediaKey(h | 1 << 63)
}

/// A parameter value, mirroring the model's `Value` without depending on it.
#[derive(Clone, Debug, PartialEq)]
pub enum Param {
    Bool(bool),
    Float(f64),
    Point([f64; 2]),
    /// Linear RGBA, straight alpha.
    Colour([f64; 4]),
    Text(String),
    Choice(String),
    Curve(Vec<[f64; 2]>),
}

impl Param {
    pub fn as_f32(&self) -> Option<f32> {
        match self {
            Param::Float(v) => Some(*v as f32),
            Param::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        }
    }

    pub fn as_point(&self) -> Option<[f32; 2]> {
        match self {
            Param::Point(p) => Some([p[0] as f32, p[1] as f32]),
            _ => None,
        }
    }

    pub fn as_colour(&self) -> Option<[f32; 4]> {
        match self {
            Param::Colour(c) => Some([c[0] as f32, c[1] as f32, c[2] as f32, c[3] as f32]),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Param::Bool(b) => Some(*b),
            Param::Float(v) => Some(*v != 0.0),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Param::Text(s) | Param::Choice(s) => Some(s),
            _ => None,
        }
    }
}

/// Named parameters.
pub type Params = BTreeMap<String, Param>;

/// Helpers for reading parameters with defaults.
pub trait ParamsExt {
    fn f(&self, key: &str, default: f32) -> f32;
    fn point(&self, key: &str, default: [f32; 2]) -> [f32; 2];
    fn colour(&self, key: &str, default: [f32; 4]) -> [f32; 4];
    fn flag(&self, key: &str, default: bool) -> bool;
    fn text<'a>(&'a self, key: &str, default: &'a str) -> &'a str;
}

impl ParamsExt for Params {
    fn f(&self, key: &str, default: f32) -> f32 {
        self.get(key).and_then(Param::as_f32).unwrap_or(default)
    }
    fn point(&self, key: &str, default: [f32; 2]) -> [f32; 2] {
        self.get(key).and_then(Param::as_point).unwrap_or(default)
    }
    fn colour(&self, key: &str, default: [f32; 4]) -> [f32; 4] {
        self.get(key).and_then(Param::as_colour).unwrap_or(default)
    }
    fn flag(&self, key: &str, default: bool) -> bool {
        self.get(key).and_then(Param::as_bool).unwrap_or(default)
    }
    fn text<'a>(&'a self, key: &str, default: &'a str) -> &'a str {
        self.get(key).and_then(Param::as_str).unwrap_or(default)
    }
}

/// Canvas and timing of a scene.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Canvas {
    pub width: u32,
    pub height: u32,
    pub frame_rate: f64,
    /// Background shown where nothing is drawn: linear premultiplied RGBA.
    pub background: [f32; 4],
    /// Output space of the project (decides whether HDR sources are tone
    /// mapped).
    pub output: OutputSpace,
    /// View (display) transform applied as the last colour step.
    pub view: crate::view::ViewTransform,
}

impl Canvas {
    pub fn new(width: u32, height: u32, frame_rate: f64) -> Canvas {
        Canvas { width, height, frame_rate, background: [0.0, 0.0, 0.0, 1.0], output: OutputSpace::SrgbDisplay, view: crate::view::ViewTransform::None }
    }

    pub fn size(&self) -> [f32; 2] {
        [self.width as f32, self.height as f32]
    }

    /// Length of one frame.
    pub fn frame_duration(&self) -> Micros {
        (SECOND as f64 / self.frame_rate.max(1e-6)).round() as Micros
    }
}

/// One effect in a stack.
#[derive(Clone, Debug, PartialEq)]
pub struct EffectDesc {
    /// Effect id as registered by the `fx` crate.
    pub kind: String,
    pub params: Params,
}

/// A colour grade (parameter names as read by [`crate::effects::grade_from_desc`]).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct GradeDesc {
    pub params: Params,
    /// Path of a `.cube` LUT applied as part of the grade.
    pub lut: Option<String>,
}

/// Reference to a motion preset.
#[derive(Clone, Debug, PartialEq)]
pub struct MotionRef {
    pub kind: String,
    pub duration: Micros,
    pub params: Params,
}

/// Entry, exit and loop motion presets of a clip.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct MotionDesc {
    pub entry: Option<MotionRef>,
    pub exit: Option<MotionRef>,
    pub looping: Option<MotionRef>,
}

/// Generated pictures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeneratorKind {
    Solid,
    Gradient,
    Text,
    Shape,
}

/// A text run: characters and style overrides.
#[derive(Clone, Debug, PartialEq)]
pub struct TextRun {
    pub text: String,
    pub style: Params,
}

/// Generated content and its parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct GeneratorDesc {
    pub kind: GeneratorKind,
    pub params: Params,
    pub text: Vec<TextRun>,
}

/// Where a clip's picture comes from.
#[derive(Clone)]
pub enum SourceDesc {
    /// A frame of a media file at a source time.
    Media { media: MediaKey, time: Micros },
    /// Another scene rendered recursively at its own time (compound clip).
    Nested { scene: Arc<dyn Scene>, time: Micros },
    /// Generated picture (solid, gradient, text, shape).
    Generator(GeneratorDesc),
    /// No picture: the clip's effects and grade apply to everything below.
    Adjustment,
    /// A camera: the topmost one looks at the 3D layers.
    Camera(CameraDesc),
    /// A light on the 3D layers.
    Light(LightDesc),
    /// 3D models, drawn together through the scene camera and lights.
    Mesh(crate::mesh_render::MeshDesc),
    /// A walkable world, drawn through the scene camera.
    World(crate::world_scene::WorldDesc),
}

impl fmt::Debug for SourceDesc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SourceDesc::Media { media, time } => write!(f, "Media({media:?} @ {time})"),
            SourceDesc::Nested { time, .. } => write!(f, "Nested(@ {time})"),
            SourceDesc::Generator(g) => write!(f, "Generator({:?})", g.kind),
            SourceDesc::Adjustment => write!(f, "Adjustment"),
            SourceDesc::Camera(_) => write!(f, "Camera"),
            SourceDesc::Light(l) => write!(f, "Light({:?})", l.kind),
            SourceDesc::Mesh(m) => write!(f, "Mesh({} models)", m.instances.len()),
            SourceDesc::World(w) => write!(f, "{w:?}"),
        }
    }
}

/// A visible clip at one instant, with every animated value evaluated.
#[derive(Clone, Debug)]
pub struct ClipDesc {
    /// Stable id (the model's clip id), returned by hit-testing.
    pub id: u64,
    /// Stacking order: higher is drawn later (on top). The spine is 0.
    pub order: i32,
    /// Clip start on the timeline and its length, for motion phases.
    pub start: Micros,
    pub duration: Micros,
    pub source: SourceDesc,
    pub placement: Placement,
    pub blend: BlendMode,
    pub effects: Vec<EffectDesc>,
    pub grade: Option<GradeDesc>,
    /// Secondary correction nodes after the grade, in order.
    pub grade_nodes: Vec<crate::nodes::GradeNodeDesc>,
    pub masks: Vec<MaskSpec>,
    /// Puppet pins and corner pin, applied after the masks.
    pub warp: Option<WarpDesc>,
    /// A cut-out's per-frame matte over a media clip's picture.
    pub matte: Option<MatteDesc>,
    /// A shadow on the ground under the layer (2D layers).
    pub shadow: Option<crate::shadow::ShadowDesc>,
    /// Set when the clip is a 3D layer.
    pub space: Option<Space>,
    pub motion: MotionDesc,
    /// Per-clip seed for animated effects.
    pub seed: u32,
}

impl ClipDesc {
    /// A clip with default placement and no processing.
    pub fn new(id: u64, order: i32, start: Micros, duration: Micros, source: SourceDesc) -> ClipDesc {
        ClipDesc {
            id,
            order,
            start,
            duration,
            source,
            placement: Placement::default(),
            blend: BlendMode::Normal,
            effects: Vec::new(),
            grade: None,
            grade_nodes: Vec::new(),
            masks: Vec::new(),
            warp: None,
            matte: None,
            shadow: None,
            space: None,
            motion: MotionDesc::default(),
            seed: id as u32,
        }
    }

    pub fn end(&self) -> Micros {
        self.start + self.duration
    }
}

/// A spine transition in progress, with both sides evaluated at the current
/// time (the outgoing clip past its end, the incoming one before its
/// start).
#[derive(Clone, Debug)]
pub struct TransitionDesc {
    /// Transition id as registered by the `fx` crate.
    pub kind: String,
    /// 0 at the start of the transition, 1 at its end.
    pub progress: f32,
    pub params: Params,
    pub left: ClipDesc,
    pub right: ClipDesc,
    pub seed: u32,
}

/// Everything the renderer needs to know about a timeline.
pub trait Scene: Send + Sync {
    fn canvas(&self) -> Canvas;

    /// Total length.
    fn duration(&self) -> Micros;

    /// The clips visible at `t`, in any order, with animated values
    /// evaluated at `t`. Hidden lanes and disabled clips are left out. When
    /// a spine transition is in progress at `t`, its two clips are returned
    /// by [`Scene::transition_at`] instead of here.
    fn clips_at(&self, t: Micros) -> Vec<ClipDesc>;

    /// The spine transition in progress at `t`, if any.
    fn transition_at(&self, t: Micros) -> Option<TransitionDesc>;
}

/// A scene built in code: a fixed list of clips. Used by tests, benchmarks
/// and hover previews.
#[derive(Clone)]
pub struct StaticScene {
    pub canvas: Canvas,
    pub duration: Micros,
    pub clips: Vec<ClipDesc>,
    pub transitions: Vec<(Micros, Micros, String, Params, u64, u64)>,
}

impl StaticScene {
    pub fn new(canvas: Canvas, duration: Micros) -> StaticScene {
        StaticScene { canvas, duration, clips: Vec::new(), transitions: Vec::new() }
    }

    pub fn with_clip(mut self, clip: ClipDesc) -> StaticScene {
        self.clips.push(clip);
        self
    }

    /// Adds a transition `kind` between clips `left` and `right`, running
    /// from `start` for `duration`.
    pub fn with_transition(mut self, left: u64, right: u64, kind: &str, start: Micros, duration: Micros, params: Params) -> StaticScene {
        self.transitions.push((start, duration, kind.to_string(), params, left, right));
        self
    }
}

impl Scene for StaticScene {
    fn canvas(&self) -> Canvas {
        self.canvas
    }

    fn duration(&self) -> Micros {
        self.duration
    }

    fn clips_at(&self, t: Micros) -> Vec<ClipDesc> {
        let in_transition: Vec<u64> = self
            .transitions
            .iter()
            .filter(|(s, d, ..)| t >= *s && t < *s + *d)
            .flat_map(|(_, _, _, _, l, r)| [*l, *r])
            .collect();
        self.clips.iter().filter(|c| t >= c.start && t < c.end() && !in_transition.contains(&c.id)).cloned().collect()
    }

    fn transition_at(&self, t: Micros) -> Option<TransitionDesc> {
        let (s, d, kind, params, l, r) = self.transitions.iter().find(|(s, d, ..)| t >= *s && t < *s + *d)?;
        let left = self.clips.iter().find(|c| c.id == *l)?.clone();
        let right = self.clips.iter().find(|c| c.id == *r)?.clone();
        Some(TransitionDesc { kind: kind.clone(), progress: (t - s) as f32 / *d as f32, params: params.clone(), left, right, seed: *l as u32 ^ *r as u32 })
    }
}
