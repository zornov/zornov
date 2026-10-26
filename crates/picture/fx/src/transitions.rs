// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Transitions between an outgoing picture `a` and an incoming picture `b`.
//!
//! Every transition is a function of progress `p` in 0..1. At `p = 0` the
//! result is exactly `a` and at `p = 1` exactly `b`; the dispatcher
//! guarantees this so cuts in and out of a transition never jump.

use std::sync::OnceLock;

use crate::easing::{Easing, EASING_NAMES};
use crate::image::{add_light, lerp_px, linear_to_srgb, luma, premultiply, scale_px, unpremultiply, Edge, Image, Px};
use crate::math::{fbm, hash2, smoothstep, TAU};
use crate::ops::{gaussian_blur, pct_h};
use crate::params::{ParamDef, ParamSet, Unit};
use crate::shapes::Shape;

/// Extra inputs a transition may use.
#[derive(Clone, Copy, Debug, Default)]
pub struct TransitionContext {
    /// Per-transition seed for the random-looking ones (glitch cut).
    pub seed: u32,
}

/// One transition.
#[derive(Clone, Copy)]
pub struct TransitionDef {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub params: &'static [ParamDef],
    /// WGSL compute shader, if there is one. Uses the transition binding
    /// layout from `wgsl/transition_common.wgsl`; the renderer should skip
    /// the shader at progress 0 and 1 and copy the input, as the CPU path
    /// does.
    pub wgsl: Option<&'static str>,
    run: fn(&TransitionContext, &Image, &Image, f32, &ParamSet) -> Image,
    /// Set for transitions registered at run time (plugins).
    ext: Option<&'static dyn crate::external::TransitionRun>,
}

impl std::fmt::Debug for TransitionDef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransitionDef").field("id", &self.id).finish()
    }
}

impl TransitionDef {
    const fn new(
        id: &'static str,
        name: &'static str,
        description: &'static str,
        params: &'static [ParamDef],
        run: fn(&TransitionContext, &Image, &Image, f32, &ParamSet) -> Image,
    ) -> Self {
        TransitionDef { id, name, description, params, wgsl: None, run, ext: None }
    }

    /// A transition whose work is done by `run` (a plugin), for
    /// [`crate::external::register_transition`].
    pub const fn external(
        id: &'static str,
        name: &'static str,
        description: &'static str,
        params: &'static [ParamDef],
        wgsl: Option<&'static str>,
        run: &'static dyn crate::external::TransitionRun,
    ) -> Self {
        TransitionDef { id, name, description, params, wgsl, run: |_, a, _, _, _| a.clone(), ext: Some(run) }
    }

    const fn wgsl(mut self, src: &'static str) -> Self {
        self.wgsl = Some(src);
        self
    }

    pub fn defaults(&self) -> ParamSet {
        ParamSet::defaults(self.params)
    }

    /// Renders the transition at `progress`. Both pictures must have the
    /// same size.
    pub fn apply(&self, ctx: &TransitionContext, a: &Image, b: &Image, progress: f32, params: &ParamSet) -> Image {
        assert!(crate::effects::same_defs(params.defs(), self.params), "parameter set belongs to another transition");
        assert_eq!((a.width, a.height), (b.width, b.height), "transition inputs differ in size");
        if !(progress > 0.0) || a.width == 0 || a.height == 0 {
            return a.clone();
        }
        if progress >= 1.0 {
            return b.clone();
        }
        let mut out = match self.ext {
            Some(ext) => ext.run(ctx, a, b, progress, params),
            None => (self.run)(ctx, a, b, progress, params),
        };
        if (out.width, out.height) != (a.width, a.height) {
            return a.clone();
        }
        debug_assert!(out.is_finite(), "transition `{}` produced a non-finite value", self.id);
        out.sanitise();
        out
    }
}

/// Every transition, in browser order.
pub fn all() -> &'static [TransitionDef] {
    TRANSITIONS
}

/// Looks a transition up by id: built-ins, then ones registered at run
/// time ([`crate::external`]).
pub fn find(id: &str) -> Option<&'static TransitionDef> {
    TRANSITIONS.iter().find(|t| t.id == id).or_else(|| crate::external::find_transition(id))
}

const DIRECTIONS: &[&str] = &["Left", "Right", "Up", "Down"];
const EASING: ParamDef = ParamDef::choice("easing", "Easing", EASING_NAMES, 9);
const EASING_LINEAR: ParamDef = ParamDef::choice("easing", "Easing", EASING_NAMES, 0);
const EASING_SINE: ParamDef = ParamDef::choice("easing", "Easing", EASING_NAMES, 3);
const DIRECTION: ParamDef = ParamDef::choice("direction", "Direction", DIRECTIONS, 0);
const CENTER: ParamDef = ParamDef::point("center", "Centre", [0.5, 0.5]);
const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
/// Motion blur as a fraction of a 360-degree shutter; 0.5 is the film-like
/// 180-degree default.
const MOTION_BLUR: ParamDef = ParamDef::float("motion_blur", "Motion blur", 0.0, 1.0, 0.5).unit(Unit::Fraction);

static TRANSITIONS: &[TransitionDef] = &[
    TransitionDef::new(
        "dissolve",
        "Dissolve",
        "Cross-fade, mixed in linear light so it does not dip in brightness.",
        &[ParamDef::choice("curve", "Curve", &["Linear", "Smooth"], 1)],
        dissolve,
    )
    .wgsl(crate::shaders::T_DISSOLVE),
    TransitionDef::new(
        "dip",
        "Dip to colour",
        "Fades out to a colour, then in to the next clip.",
        &[
            ParamDef::colour("colour", "Colour", BLACK),
            ParamDef::float("hold", "Hold", 0.0, 0.8, 0.1).unit(Unit::Fraction),
        ],
        dip,
    )
    .wgsl(crate::shaders::T_DIP),
    TransitionDef::new(
        "wipe",
        "Wipe",
        "A straight edge sweeps across at any angle.",
        &[
            ParamDef::float("angle", "Angle", -180.0, 180.0, 0.0).unit(Unit::Degrees),
            ParamDef::float("softness", "Softness", 0.0, 0.5, 0.05).unit(Unit::Fraction),
            EASING_SINE,
        ],
        wipe,
    )
    .wgsl(crate::shaders::T_WIPE),
    TransitionDef::new(
        "clock_wipe",
        "Clock wipe",
        "A hand sweeps round like a clock.",
        &[
            ParamDef::float("start", "Start angle", -180.0, 180.0, 0.0).unit(Unit::Degrees),
            ParamDef::choice("direction", "Direction", &["Clockwise", "Anticlockwise"], 0),
            ParamDef::float("softness", "Softness", 0.0, 45.0, 3.0).unit(Unit::Degrees),
            CENTER,
            EASING_SINE,
        ],
        clock_wipe,
    )
    .wgsl(crate::shaders::T_CLOCK_WIPE),
    TransitionDef::new(
        "iris",
        "Iris",
        "A shape opens from (or closes to) a point.",
        &[
            ParamDef::choice("shape", "Shape", &["Circle", "Rectangle", "Diamond", "Star"], 0),
            ParamDef::choice("mode", "Mode", &["Open", "Close"], 0),
            ParamDef::float("softness", "Softness", 0.0, 0.3, 0.02).unit(Unit::Fraction),
            CENTER,
            EASING,
        ],
        iris,
    ),
    TransitionDef::new(
        "slide",
        "Slide",
        "The next clip slides in while the current one drifts back and dims.",
        &[DIRECTION, MOTION_BLUR, EASING],
        slide,
    )
    .wgsl(crate::shaders::T_SLIDE),
    TransitionDef::new("push", "Push", "The next clip pushes the current one out of frame.", &[DIRECTION, MOTION_BLUR, EASING], push)
        .wgsl(crate::shaders::T_PUSH),
    TransitionDef::new(
        "cover",
        "Cover",
        "The next clip slides in over the current one.",
        &[DIRECTION, ParamDef::float("shadow", "Shadow", 0.0, 1.0, 0.4).unit(Unit::Fraction), MOTION_BLUR, EASING],
        cover,
    )
    .wgsl(crate::shaders::T_COVER),
    TransitionDef::new(
        "reveal",
        "Reveal",
        "The current clip slides away to uncover the next.",
        &[DIRECTION, ParamDef::float("shadow", "Shadow", 0.0, 1.0, 0.4).unit(Unit::Fraction), MOTION_BLUR, EASING],
        reveal,
    )
    .wgsl(crate::shaders::T_REVEAL),
    TransitionDef::new(
        "zoom_through",
        "Zoom through",
        "Flies into the current clip and out into the next.",
        &[
            ParamDef::float("amount", "Zoom", 1.0, 8.0, 3.0).unit(Unit::Times),
            ParamDef::float("blur", "Motion blur", 0.0, 1.0, 0.6).unit(Unit::Fraction),
            EASING,
        ],
        zoom_through,
    ),
    TransitionDef::new(
        "spin",
        "Spin",
        "Whirls the current clip away and the next one in.",
        &[
            ParamDef::float("turns", "Turns", 0.25, 4.0, 1.0).unit(Unit::Times),
            ParamDef::choice("direction", "Direction", &["Clockwise", "Anticlockwise"], 0),
            ParamDef::float("zoom", "Zoom pulse", 0.0, 1.0, 0.4).unit(Unit::Fraction),
            ParamDef::float("blur", "Motion blur", 0.0, 1.0, 0.5).unit(Unit::Fraction),
            EASING,
        ],
        spin,
    ),
    TransitionDef::new(
        "flip",
        "Flip",
        "Turns over like a card to show the next clip on its back.",
        &[
            ParamDef::choice("axis", "Axis", &["Horizontal", "Vertical"], 0),
            ParamDef::float("perspective", "Perspective", 0.0, 1.0, 0.6).unit(Unit::Fraction),
            ParamDef::colour("background", "Background", BLACK),
            EASING,
        ],
        flip,
    ),
    TransitionDef::new(
        "cube",
        "Cube",
        "Rotates a cube to bring the next clip round on the next face.",
        &[DIRECTION, ParamDef::colour("background", "Background", BLACK), EASING],
        cube,
    ),
    TransitionDef::new(
        "page_curl",
        "Page curl",
        "Peels the current clip away like a page.",
        &[
            ParamDef::float("angle", "Direction", -180.0, 180.0, 30.0).unit(Unit::Degrees),
            ParamDef::float("radius", "Curl radius", 0.01, 0.3, 0.08).unit(Unit::Fraction),
            ParamDef::float("shadow", "Shadow", 0.0, 1.0, 0.5).unit(Unit::Fraction),
            ParamDef::float("back", "Paper back", 0.0, 1.0, 0.7).unit(Unit::Fraction),
            EASING_SINE,
        ],
        page_curl,
    ),
    TransitionDef::new(
        "blur_dissolve",
        "Blur dissolve",
        "Cross-fade through a soft blur.",
        &[ParamDef::float("blur", "Blur", 0.0, 20.0, 3.0).unit(Unit::PercentHeight)],
        blur_dissolve,
    ),
    TransitionDef::new(
        "whip_pan",
        "Whip pan",
        "A fast camera swish with heavy motion blur.",
        &[DIRECTION, ParamDef::float("blur", "Motion blur", 0.0, 30.0, 12.0).unit(Unit::PercentHeight)],
        whip_pan,
    ),
    TransitionDef::new(
        "glitch_cut",
        "Glitch cut",
        "Digital breakup that tears from one clip to the next.",
        &[ParamDef::float("intensity", "Intensity", 0.0, 1.0, 0.7).unit(Unit::Fraction)],
        glitch_cut,
    ),
    TransitionDef::new(
        "light_flash",
        "Light flash",
        "A burst of light hides the cut.",
        &[
            ParamDef::colour("colour", "Colour", [1.0, 0.94, 0.85, 1.0]),
            ParamDef::float("intensity", "Intensity", 0.0, 8.0, 1.5),
            ParamDef::float("width", "Flash length", 0.05, 1.0, 0.4).unit(Unit::Fraction),
        ],
        light_flash,
    ),
    TransitionDef::new(
        "luma_fade",
        "Luma fade",
        "Reveals the next clip in the order of a brightness map.",
        &[
            ParamDef::choice("map", "Map", &["Linear", "Radial", "Noise", "Incoming brightness", "Outgoing brightness"], 2),
            ParamDef::float("softness", "Softness", 0.0, 1.0, 0.2).unit(Unit::Fraction),
            ParamDef::float("angle", "Angle", -180.0, 180.0, 0.0).unit(Unit::Degrees),
            ParamDef::float("scale", "Noise scale", 1.0, 100.0, 20.0).unit(Unit::PercentHeight),
            ParamDef::bool("invert", "Invert", false),
            EASING_LINEAR,
        ],
        luma_fade,
    )
    .wgsl(crate::shaders::T_LUMA_FADE),
    TransitionDef::new(
        "shape_reveal",
        "Shape reveal",
        "The next clip grows out of a spinning shape with an outline.",
        &[
            ParamDef::choice("shape", "Shape", &["Heart", "Star", "Hexagon", "Circle", "Triangle", "Diamond"], 0),
            ParamDef::float("spin", "Spin", -720.0, 720.0, 90.0).unit(Unit::Degrees),
            ParamDef::float("softness", "Softness", 0.0, 0.3, 0.01).unit(Unit::Fraction),
            ParamDef::float("border", "Outline width", 0.0, 5.0, 0.6).unit(Unit::PercentHeight),
            ParamDef::colour("border_colour", "Outline colour", [1.0, 1.0, 1.0, 1.0]),
            CENTER,
            EASING,
        ],
        shape_reveal,
    ),
];

fn ease(p: &ParamSet, t: f32) -> f32 {
    Easing::from_index(p.choice("easing")).apply(t)
}

fn mix_images(a: &Image, b: &Image, f: impl Fn(usize, usize) -> f32 + Sync) -> Image {
    a.map_xy(|x, y, pa| lerp_px(pa, b.get(x, y), f(x, y).clamp(0.0, 1.0)))
}

fn dissolve(_: &TransitionContext, a: &Image, b: &Image, t: f32, p: &ParamSet) -> Image {
    let k = if p.choice("curve") == 1 { smoothstep(0.0, 1.0, t) } else { t };
    crate::ops::mix(a, b, k)
}

fn dip(_: &TransitionContext, a: &Image, b: &Image, t: f32, p: &ParamSet) -> Image {
    let c = p.colour("colour");
    let col = premultiply([c[0], c[1], c[2]], c[3]);
    let hold = p.f("hold");
    let half = (1.0 - hold) * 0.5;
    if t < half {
        let k = smoothstep(0.0, 1.0, t / half);
        a.map(|px| lerp_px(px, col, k))
    } else if t <= half + hold {
        Image::filled(a.width, a.height, col)
    } else {
        let k = smoothstep(0.0, 1.0, (t - half - hold) / half);
        b.map(|px| lerp_px(col, px, k))
    }
}

/// 0..1 position of every pixel along a direction, 0 at the first corner
/// the direction reaches.
fn ramp(img: &Image, angle_deg: f32) -> impl Fn(usize, usize) -> f32 {
    let (s, c) = angle_deg.to_radians().sin_cos();
    let (w, h) = (img.width as f32, img.height as f32);
    let corners = [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)].map(|(x, y)| x * c + y * s);
    let lo = corners.iter().cloned().fold(f32::INFINITY, f32::min);
    let hi = corners.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    move |x, y| ((x as f32 + 0.5) * c + (y as f32 + 0.5) * s - lo) / (hi - lo).max(1e-6)
}

/// Mask for revealing in the order of `m` (0 first): 1 where revealed.
#[inline]
fn reveal_mask(m: f32, e: f32, soft: f32) -> f32 {
    let edge = e * (1.0 + soft);
    smoothstep(edge, edge - soft, m)
}

fn wipe(_: &TransitionContext, a: &Image, b: &Image, t: f32, p: &ParamSet) -> Image {
    let e = ease(p, t);
    let soft = p.f("softness");
    let r = ramp(a, p.f("angle"));
    mix_images(a, b, |x, y| reveal_mask(r(x, y), e, soft))
}

fn clock_wipe(_: &TransitionContext, a: &Image, b: &Image, t: f32, p: &ParamSet) -> Image {
    let e = ease(p, t);
    let soft = p.f("softness") / 360.0;
    let c = p.point("center");
    let (cx, cy) = (c[0] * a.width as f32, c[1] * a.height as f32);
    let start = p.f("start").to_radians();
    let ccw = p.choice("direction") == 1;
    mix_images(a, b, |x, y| {
        let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
        // Angle from twelve o'clock, growing clockwise (y points down).
        let mut ang = dx.atan2(-dy) - start;
        if ccw {
            ang = -ang;
        }
        reveal_mask(ang.rem_euclid(TAU) / TAU, e, soft)
    })
}

/// Largest normalised shape distance of any point on the frame edge,
/// measured from `(cx, cy)` in frame-height units: the size at which the
/// shape covers the whole frame.
fn cover_size(w: f32, h: f32, cx: f32, cy: f32, d: impl Fn(f32, f32) -> f32) -> f32 {
    let mut m = 0.0f32;
    for i in 0..=64 {
        let f = i as f32 / 64.0;
        for (x, y) in [(f * w, 0.0), (f * w, h), (0.0, f * h), (w, f * h)] {
            m = m.max(d((x - cx) / h, (y - cy) / h));
        }
    }
    m
}

fn iris(_: &TransitionContext, a: &Image, b: &Image, t: f32, p: &ParamSet) -> Image {
    let e = ease(p, t);
    let (w, h) = (a.width as f32, a.height as f32);
    let aspect = w / h;
    let c = p.point("center");
    let (cx, cy) = (c[0] * w, c[1] * h);
    let shape_id = p.choice("shape");
    let d = move |x: f32, y: f32| match shape_id {
        // The rectangle keeps the frame's proportions.
        1 => Shape::Square.distance(x / aspect * 2.0f32.sqrt(), y * 2.0f32.sqrt()) * 0.5,
        2 => Shape::Diamond.distance(x, y),
        3 => Shape::Star.distance(x, y),
        _ => Shape::Circle.distance(x, y),
    };
    let soft = p.f("softness");
    let full = cover_size(w, h, cx, cy, d) + soft;
    let close = p.choice("mode") == 1;
    mix_images(a, b, |x, y| {
        let dist = d((x as f32 + 0.5 - cx) / h, (y as f32 + 0.5 - cy) / h);
        if close {
            let s = (1.0 - e) * full;
            1.0 - smoothstep(s, s - soft, dist)
        } else {
            let s = e * full;
            smoothstep(s, s - soft, dist)
        }
    })
}

fn dir_vec(p: &ParamSet, img: &Image) -> (f32, f32) {
    let (w, h) = (img.width as f32, img.height as f32);
    match p.choice("direction") {
        0 => (-w, 0.0),
        1 => (w, 0.0),
        2 => (0.0, -h),
        _ => (0.0, h),
    }
}

/// Sample `img` displaced by `(ox, oy)` pixels, or `None` outside it.
fn shifted(img: &Image, x: usize, y: usize, ox: f32, oy: f32) -> Option<Px> {
    let (sx, sy) = (x as f32 + 0.5 - ox, y as f32 + 0.5 - oy);
    if sx < 0.0 || sy < 0.0 || sx > img.width as f32 || sy > img.height as f32 {
        None
    } else {
        Some(img.sample(sx, sy, Edge::Clamp))
    }
}

/// Generic two-layer slide: `a` moved by `ka * e`, `b` moved in from one
/// frame away by `kb`, `b` drawn on top when `b_on_top`. `shade` darkens the
/// lower layer near the upper one's edge.
#[allow(clippy::too_many_arguments)]
fn slide_layers(a: &Image, b: &Image, e: f32, dir: (f32, f32), ka: f32, b_moves: bool, b_on_top: bool, shadow: f32, dim_a: f32) -> Image {
    let (ax, ay) = (dir.0 * ka * e, dir.1 * ka * e);
    let (bx, by) = if b_moves { (dir.0 * (e - 1.0), dir.1 * (e - 1.0)) } else { (0.0, 0.0) };
    let shade_len = 0.06 * a.height as f32;
    let len = (dir.0.abs() + dir.1.abs()).max(1.0);
    let (ux, uy) = (dir.0 / len, dir.1 / len);
    a.map_xy(|x, y, _| {
        let pa = shifted(a, x, y, ax, ay).map(|c| scale_px(c, 1.0 - dim_a * e));
        let pb = shifted(b, x, y, bx, by);
        // Distance (pixels) from the moving layer's edge, for the shadow.
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        let edge_dist = |ox: f32, oy: f32, leading: bool| {
            // Position along the motion direction relative to the layer.
            let along = (fx - ox) * ux + (fy - oy) * uy;
            let size = (a.width as f32 * ux.abs()) + (a.height as f32 * uy.abs());
            let pos = if ux + uy > 0.0 { along } else { along + size };
            if leading { pos - size } else { -pos }
        };
        let (top, bottom, d) = if b_on_top {
            (pb, pa, edge_dist(bx, by, true))
        } else {
            (pa, pb, edge_dist(ax, ay, false))
        };
        let s = if shadow > 0.0 && d > 0.0 { shadow * 0.6 * (-d / shade_len).exp() * smoothstep(0.0, 0.05, e) } else { 0.0 };
        match (top, bottom) {
            (Some(t), Some(u)) => crate::image::over(t, scale_px(u, 1.0 - s)),
            (Some(t), None) => t,
            (None, Some(u)) => scale_px(u, 1.0 - s),
            (None, None) => [0.0; 4],
        }
    })
}

/// Renders a moving transition with shutter motion blur: averages frames
/// spread over the part of the transition the shutter is open for. A
/// transition is assumed to last about 15 frames, so a 180-degree shutter
/// covers 1/30 of it. Samples outside 0..1 are clamped, which keeps the
/// ends exact and continuous.
fn with_motion_blur(t: f32, p: &ParamSet, render: impl Fn(f32) -> Image) -> Image {
    let span = p.f("motion_blur") / 15.0;
    if span <= 1e-4 {
        return render(t);
    }
    const SAMPLES: usize = 8;
    let mut acc = render((t - span * 0.5).clamp(0.0, 1.0));
    for i in 1..SAMPLES {
        let ti = (t + span * (i as f32 / (SAMPLES - 1) as f32 - 0.5)).clamp(0.0, 1.0);
        let f = render(ti);
        for (a, b) in acc.pixels.iter_mut().zip(&f.pixels) {
            for c in 0..4 {
                a[c] += b[c];
            }
        }
    }
    acc.pixels.iter_mut().for_each(|px| *px = scale_px(*px, 1.0 / SAMPLES as f32));
    acc
}

fn slide(_: &TransitionContext, a: &Image, b: &Image, t: f32, p: &ParamSet) -> Image {
    with_motion_blur(t, p, |t| slide_layers(a, b, ease(p, t), dir_vec(p, a), 0.3, true, true, 0.3, 0.35))
}

fn push(_: &TransitionContext, a: &Image, b: &Image, t: f32, p: &ParamSet) -> Image {
    with_motion_blur(t, p, |t| slide_layers(a, b, ease(p, t), dir_vec(p, a), 1.0, true, true, 0.0, 0.0))
}

fn cover(_: &TransitionContext, a: &Image, b: &Image, t: f32, p: &ParamSet) -> Image {
    with_motion_blur(t, p, |t| slide_layers(a, b, ease(p, t), dir_vec(p, a), 0.0, true, true, p.f("shadow"), 0.0))
}

fn reveal(_: &TransitionContext, a: &Image, b: &Image, t: f32, p: &ParamSet) -> Image {
    with_motion_blur(t, p, |t| slide_layers(a, b, ease(p, t), dir_vec(p, a), 1.0, false, false, p.f("shadow"), 0.0))
}

/// Samples `img` scaled by `s` and rotated by `rot` about the frame
/// centre, averaging `taps` samples spread over `blur_s` extra scale and
/// `blur_r` extra rotation for motion blur.
fn transform_sample(img: &Image, x: usize, y: usize, s: f32, rot: f32, blur_s: f32, blur_r: f32, taps: usize) -> Px {
    let (cx, cy) = (img.width as f32 * 0.5, img.height as f32 * 0.5);
    let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
    let mut acc = [0.0f32; 4];
    for i in 0..taps {
        let f = if taps > 1 { i as f32 / (taps - 1) as f32 - 0.5 } else { 0.0 };
        let ss = (s * (1.0 + blur_s * f)).max(1e-3);
        let (sn, cs) = (-(rot + blur_r * f)).sin_cos();
        let (rx, ry) = ((dx * cs - dy * sn) / ss, (dx * sn + dy * cs) / ss);
        let v = img.sample(cx + rx, cy + ry, Edge::Mirror);
        for c in 0..4 {
            acc[c] += v[c];
        }
    }
    scale_px(acc, 1.0 / taps as f32)
}

fn zoom_through(_: &TransitionContext, a: &Image, b: &Image, t: f32, p: &ParamSet) -> Image {
    let e = ease(p, t);
    let amount = p.f("amount");
    let blur = p.f("blur") * (std::f32::consts::PI * t).sin() * 0.25 * (amount - 1.0).min(3.0);
    let sa = 1.0 + (amount - 1.0) * e;
    // The next clip arrives from a little further away, so the motion
    // carries on through the cut.
    let sb = 0.6 + 0.4 * e;
    let k = smoothstep(0.35, 0.65, t);
    let taps = if blur > 0.01 { 8 } else { 1 };
    Image::from_fn(a.width, a.height, |x, y| {
        let pa = if k < 1.0 { transform_sample(a, x, y, sa, 0.0, blur, 0.0, taps) } else { [0.0; 4] };
        let pb = if k > 0.0 { transform_sample(b, x, y, sb, 0.0, blur * 0.5, 0.0, taps) } else { [0.0; 4] };
        lerp_px(pa, pb, k)
    })
}

fn spin(_: &TransitionContext, a: &Image, b: &Image, t: f32, p: &ParamSet) -> Image {
    let e = ease(p, t);
    let sign = if p.choice("direction") == 1 { -1.0 } else { 1.0 };
    let total = p.f("turns") * TAU * sign;
    let rot = total * e;
    let s = 1.0 + p.f("zoom") * (std::f32::consts::PI * e).sin();
    let blur_r = p.f("blur") * total.abs().min(TAU * 2.0) * 0.06 * (std::f32::consts::PI * t).sin() * sign;
    let taps = if blur_r.abs() > 1e-3 { 9 } else { 1 };
    let k = smoothstep(0.4, 0.6, t);
    Image::from_fn(a.width, a.height, |x, y| {
        let pa = if k < 1.0 { transform_sample(a, x, y, s, rot, 0.0, blur_r, taps) } else { [0.0; 4] };
        let pb = if k > 0.0 { transform_sample(b, x, y, s, rot - total, 0.0, blur_r, taps) } else { [0.0; 4] };
        lerp_px(pa, pb, k)
    })
}

fn flip(_: &TransitionContext, a: &Image, b: &Image, t: f32, p: &ParamSet) -> Image {
    let theta = std::f32::consts::PI * ease(p, t);
    let (w, h) = (a.width as f32, a.height as f32);
    let vertical = p.choice("axis") == 1;
    let bg = p.colour("background");
    let bg = premultiply([bg[0], bg[1], bg[2]], bg[3]);
    // Camera distance in frame heights: closer means stronger perspective.
    let dist = 6.0 + (1.2 - 6.0) * p.f("perspective");
    let (img, phi) = if theta < std::f32::consts::FRAC_PI_2 { (a, theta) } else { (b, theta - std::f32::consts::PI) };
    let (sn, cs) = phi.sin_cos();
    // Shrink a little mid-flip so the near edge stays inside the frame.
    let k = 1.0 - 0.2 * theta.sin();
    let shade = 0.6 + 0.4 * cs.abs();
    let (half_u, half_v) = if vertical { (0.5, w / h * 0.5) } else { (w / h * 0.5, 0.5) };
    Image::from_fn(a.width, a.height, |x, y| {
        let (mut u, mut v) = ((x as f32 + 0.5 - w * 0.5) / h / k, (y as f32 + 0.5 - h * 0.5) / h / k);
        if vertical {
            std::mem::swap(&mut u, &mut v);
        }
        // Inverse perspective projection of a plane rotated about the v axis.
        let den = dist * cs - u * sn;
        if den <= 1e-4 {
            return bg;
        }
        let s = u * dist / den;
        let tt = v * (dist + s * sn) / dist;
        if s.abs() > half_u || tt.abs() > half_v {
            return bg;
        }
        let (su, sv) = if vertical { (tt, s) } else { (s, tt) };
        let px = img.sample(su * h + w * 0.5, sv * h + h * 0.5, Edge::Clamp);
        let px = [px[0] * shade, px[1] * shade, px[2] * shade, px[3]];
        crate::image::over(px, bg)
    })
}

fn cube(_: &TransitionContext, a: &Image, b: &Image, t: f32, p: &ParamSet) -> Image {
    let beta = std::f32::consts::FRAC_PI_2 * ease(p, t);
    let (w, h) = (a.width as f32, a.height as f32);
    let dir = p.choice("direction");
    let vertical = dir >= 2;
    // "Left" brings the right-hand face round; flipping handles the rest.
    let flipped = dir == 1 || dir == 3;
    let bg = p.colour("background");
    let bg = premultiply([bg[0], bg[1], bg[2]], bg[3]);
    let (hw, hh) = if vertical { (0.5, w / h * 0.5) } else { (w / h * 0.5, 0.5) };
    let dist = 2.5;
    let (sb, cb) = beta.sin_cos();
    let shade_a = 0.55 + 0.45 * cb;
    let shade_b = 0.55 + 0.45 * sb;
    Image::from_fn(a.width, a.height, |x, y| {
        let (mut u, mut v) = ((x as f32 + 0.5 - w * 0.5) / h, (y as f32 + 0.5 - h * 0.5) / h);
        if vertical {
            std::mem::swap(&mut u, &mut v);
        }
        if flipped {
            u = -u;
        }
        // Ray from the camera at (0, 0, -dist) through (u, v, 0); the cube's
        // centre is at (0, 0, hw). Work in the cube's own rotated frame.
        let (ox, oz) = (0.0, -dist - hw);
        let (dx, dz) = (u, dist);
        let rot = |x: f32, z: f32| (x * cb - z * sb, x * sb + z * cb);
        let (lox, loz) = rot(ox, oz);
        let (ldx, ldz) = rot(dx, dz);
        let mut best: Option<(f32, Px)> = None;
        // Front face (outgoing picture): z = -hw.
        if ldz.abs() > 1e-6 {
            let tt = (-hw - loz) / ldz;
            let (fx, fy) = (lox + ldx * tt, v * tt);
            if tt > 0.0 && fx.abs() <= hw && fy.abs() <= hh {
                best = Some((tt, face_sample(a, fx, fy, vertical, flipped, shade_a)));
            }
        }
        // Side face (incoming picture): x = +hw.
        if ldx.abs() > 1e-6 {
            let tt = (hw - lox) / ldx;
            let (fz, fy) = (loz + ldz * tt, v * tt);
            if tt > 0.0 && fz.abs() <= hw && fy.abs() <= hh && best.is_none_or(|(bt, _)| tt < bt) {
                best = Some((tt, face_sample(b, fz, fy, vertical, flipped, shade_b)));
            }
        }
        match best {
            Some((_, px)) => crate::image::over(px, bg),
            None => bg,
        }
    })
}

fn face_sample(img: &Image, fu: f32, fv: f32, vertical: bool, flipped: bool, shade: f32) -> Px {
    let fu = if flipped { -fu } else { fu };
    let (x, y) = if vertical { (fv, fu) } else { (fu, fv) };
    let (w, h) = (img.width as f32, img.height as f32);
    let px = img.sample(x * h + w * 0.5, y * h + h * 0.5, Edge::Clamp);
    [px[0] * shade, px[1] * shade, px[2] * shade, px[3]]
}

fn page_curl(_: &TransitionContext, a: &Image, b: &Image, t: f32, p: &ParamSet) -> Image {
    let e = ease(p, t);
    let (w, h) = (a.width as f32, a.height as f32);
    let (ny, nx) = p.f("angle").to_radians().sin_cos();
    let r = p.f("radius") * h;
    let corners = [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)].map(|(x, y)| x * nx + y * ny);
    let lo = corners.iter().cloned().fold(f32::INFINITY, f32::min);
    let hi = corners.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    // The fold line sweeps from just past the far corner (nothing lifted)
    // to beyond the near one (everything lifted away).
    let fold = (hi + 1.0) + (lo - r - 1.0 - (hi + 1.0)) * e;
    // Shadows fade in as the page starts to lift and out as it leaves.
    let lifted = ((hi - fold) / r.max(1.0)).clamp(0.0, 1.0);
    let remaining = ((fold - (lo - r - 1.0)) / r.max(1.0)).clamp(0.0, 1.0);
    let shadow = p.f("shadow") * lifted * remaining;
    let back = p.f("back");
    let inside = |x: f32, y: f32| x >= 0.0 && y >= 0.0 && x <= w && y <= h;
    let back_side = |px: Px, light: f32| {
        // The back of the page: the picture showing faintly through paper.
        let paper = [px[3], px[3], px[3], px[3]];
        scale_px(lerp_px(px, paper, back), light)
    };
    Image::from_fn(a.width, a.height, |x, y| {
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        let u = fx * nx + fy * ny - fold;
        // Original position of a page point that has travelled `arc` along
        // the page from the fold line.
        let orig = |arc: f32| (fx + nx * (arc - u), fy + ny * (arc - u));
        let (top, base): (Option<Px>, Px) = if u > r {
            // Only the incoming picture here, shaded near the curl.
            let s = shadow * 0.7 * (-(u - r) / (r * 1.5 + 1.0)).exp();
            (None, scale_px(b.get(x, y), 1.0 - s))
        } else if u >= 0.0 {
            let asin = (u / r.max(1e-3)).clamp(-1.0, 1.0).asin();
            let (bx, by) = orig(r * (std::f32::consts::PI - asin));
            let (gx, gy) = orig(r * asin);
            let top = if inside(bx, by) {
                // Back of the page on the upper half of the cylinder.
                Some(back_side(a.sample(bx, by, Edge::Clamp), 0.8 + 0.2 * asin.cos()))
            } else if inside(gx, gy) {
                Some(scale_px(a.sample(gx, gy, Edge::Clamp), 0.65 + 0.35 * asin.cos()))
            } else {
                None
            };
            (top, scale_px(b.get(x, y), 1.0 - shadow * 0.7))
        } else {
            let (bx, by) = orig(std::f32::consts::PI * r - u);
            let top = inside(bx, by).then(|| back_side(a.sample(bx, by, Edge::Clamp), 0.9));
            // The flat page darkens slightly where the curl hangs over it.
            let s = shadow * 0.35 * smoothstep(-1.5 * r, 0.0, u);
            (top, scale_px(a.get(x, y), 1.0 - s))
        };
        match top {
            Some(t) => crate::image::over(t, base),
            None => base,
        }
    })
}

fn blur_dissolve(_: &TransitionContext, a: &Image, b: &Image, t: f32, p: &ParamSet) -> Image {
    let r = pct_h(a, p.f("blur")) * (std::f32::consts::PI * t).sin() * 0.5;
    let ba = gaussian_blur(a, r, r, Edge::Clamp);
    let bb = gaussian_blur(b, r, r, Edge::Clamp);
    crate::ops::mix(&ba, &bb, smoothstep(0.0, 1.0, t))
}

fn whip_pan(_: &TransitionContext, a: &Image, b: &Image, t: f32, p: &ParamSet) -> Image {
    let e = Easing::SineInOut.apply(t);
    let (dx, dy) = dir_vec(p, a);
    let len = (dx.abs() + dy.abs()).max(1.0);
    let (ux, uy) = (dx / len, dy / len);
    let (ox, oy) = (dx * e, dy * e);
    let blur = pct_h(a, p.f("blur")) * (std::f32::consts::PI * t).sin().powi(2);
    let taps = (blur.ceil() as usize).clamp(1, 48);
    let at = |fx: f32, fy: f32| -> Px {
        // A strip of the two pictures side by side, moved by (ox, oy).
        let (ax, ay) = (fx - ox, fy - oy);
        if ax >= 0.0 && ay >= 0.0 && ax <= a.width as f32 && ay <= a.height as f32 {
            a.sample(ax, ay, Edge::Clamp)
        } else {
            b.sample(ax + dx, ay + dy, Edge::Clamp)
        }
    };
    Image::from_fn(a.width, a.height, |x, y| {
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        let mut acc = [0.0f32; 4];
        for i in 0..taps {
            let f = if taps > 1 { i as f32 / (taps - 1) as f32 - 0.5 } else { 0.0 };
            let v = at(fx + ux * blur * f, fy + uy * blur * f);
            for c in 0..4 {
                acc[c] += v[c];
            }
        }
        scale_px(acc, 1.0 / taps as f32)
    })
}

fn glitch_cut(ctx: &TransitionContext, a: &Image, b: &Image, t: f32, p: &ParamSet) -> Image {
    let amp = p.f("intensity") * (std::f32::consts::PI * t).sin();
    let m = smoothstep(0.35, 0.65, t);
    let (w, h) = (a.width as f32, a.height as f32);
    let step = (t * 16.0).floor() as i32;
    let band_h = (0.03 * h).max(1.0);
    let seed = ctx.seed;
    Image::from_fn(a.width, a.height, |x, y| {
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        let band = (fy / band_h).floor() as i32;
        let src = if hash2(band, step, seed ^ 0xb1) < m { b } else { a };
        let r = hash2(band, step, seed ^ 0xb2);
        let shift = if r < amp * 0.6 { (hash2(band, step, seed ^ 0xb3) - 0.5) * 0.25 * w * amp } else { 0.0 };
        let split = if shift != 0.0 { 0.012 * w * amp } else { 0.0 };
        let sx = fx + shift;
        let r_ = src.sample(sx - split, fy, Edge::Wrap);
        let g_ = src.sample(sx, fy, Edge::Wrap);
        let b_ = src.sample(sx + split, fy, Edge::Wrap);
        [r_[0], g_[1], b_[2], r_[3].max(g_[3]).max(b_[3])]
    })
}

fn light_flash(_: &TransitionContext, a: &Image, b: &Image, t: f32, p: &ParamSet) -> Image {
    let width = p.f("width");
    // Bell-shaped burst centred on the cut, forced to zero at both ends.
    let bell = (-((t - 0.5) / (width * 0.5)).powi(2)).exp() * (std::f32::consts::PI * t).sin();
    let k = smoothstep(0.5 - width * 0.25, 0.5 + width * 0.25, t);
    let i = p.f("intensity") * bell;
    let c = p.colour("colour");
    let light = [c[0] * c[3] * i * 0.3, c[1] * c[3] * i * 0.3, c[2] * c[3] * i * 0.3];
    a.map_xy(|x, y, pa| {
        let px = lerp_px(pa, b.get(x, y), k);
        let exposed = [px[0] * (1.0 + i), px[1] * (1.0 + i), px[2] * (1.0 + i), px[3]];
        add_light(exposed, light)
    })
}

fn luma_fade(_: &TransitionContext, a: &Image, b: &Image, t: f32, p: &ParamSet) -> Image {
    let e = ease(p, t);
    let soft = p.f("softness");
    let invert = p.b("invert");
    let map = p.choice("map");
    let (w, h) = (a.width as f32, a.height as f32);
    let r = ramp(a, p.f("angle"));
    let scale = pct_h(a, p.f("scale")).max(1.0);
    let far = (0.25 * w * w + 0.25 * h * h).sqrt();
    let perceptual = |px: Px| linear_to_srgb(luma(unpremultiply_px(px)).clamp(0.0, 1.0));
    mix_images(a, b, |x, y| {
        let m = match map {
            0 => r(x, y),
            1 => {
                let (dx, dy) = (x as f32 + 0.5 - w * 0.5, y as f32 + 0.5 - h * 0.5);
                (dx * dx + dy * dy).sqrt() / far
            }
            2 => ((fbm((x as f32 + 0.5) / scale, (y as f32 + 0.5) / scale, 0.0, 4, 7) - 0.5) * 2.2 + 0.5).clamp(0.0, 1.0),
            // Bright parts of the incoming clip show first.
            3 => 1.0 - perceptual(b.get(x, y)),
            _ => 1.0 - perceptual(a.get(x, y)),
        };
        reveal_mask(if invert { 1.0 - m } else { m }, e, soft)
    })
}

fn unpremultiply_px(p: Px) -> Px {
    let c = unpremultiply(p);
    [c[0], c[1], c[2], 1.0]
}

fn shape_reveal(_: &TransitionContext, a: &Image, b: &Image, t: f32, p: &ParamSet) -> Image {
    let e = ease(p, t);
    let shape = [Shape::Heart, Shape::Star, Shape::Hexagon, Shape::Circle, Shape::Triangle, Shape::Diamond][p.choice("shape")];
    let (w, h) = (a.width as f32, a.height as f32);
    let c = p.point("center");
    let (cx, cy) = (c[0] * w, c[1] * h);
    let rot = p.f("spin").to_radians() * (1.0 - e);
    let soft = p.f("softness");
    let border = pct_h(a, p.f("border")) / h;
    // Rotation-proof cover size: farthest corner over the inner radius.
    let far = [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)]
        .map(|(x, y)| ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() / h)
        .into_iter()
        .fold(0.0f32, f32::max);
    let full = far / shape.inradius() + soft + border;
    let s = e * full;
    let bc = p.colour("border_colour");
    let outline = premultiply([bc[0], bc[1], bc[2]], bc[3]);
    let fade = smoothstep(0.0, 0.08, e);
    let (sn, cs) = (-rot).sin_cos();
    a.map_xy(|x, y, pa| {
        let (dx, dy) = ((x as f32 + 0.5 - cx) / h, (y as f32 + 0.5 - cy) / h);
        let (rx, ry) = (dx * cs - dy * sn, dx * sn + dy * cs);
        // Distance from the outline in frame heights (approximately).
        let r = (rx * rx + ry * ry).sqrt();
        let edge = shape.radius(ry.atan2(rx)) * s;
        let inside = smoothstep(edge, edge - soft, r);
        let mut px = lerp_px(pa, b.get(x, y), inside);
        if border > 0.0 {
            let aa = 1.0 / h;
            let ring = smoothstep(border * 0.5 + aa, border * 0.5 - aa, (r - edge - border * 0.5).abs()) * fade;
            px = lerp_px(px, outline, ring);
        }
        px
    })
}

/// Cached list of transition ids, handy for UIs.
pub fn ids() -> &'static [&'static str] {
    static IDS: OnceLock<Vec<&'static str>> = OnceLock::new();
    IDS.get_or_init(|| TRANSITIONS.iter().map(|t| t.id).collect())
}
