// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Motion presets: clip entry, exit and loop animations expressed as a
//! [`Pose`] computed from progress. Text can run any preset per character,
//! word or line with a stagger.
//!
//! An entry preset maps eased progress `e` (0 = start, 1 = at rest) to a
//! pose; the exit version plays the same motion backwards in time, so every
//! entry is also an exit. Loop presets map a phase that repeats every
//! `period` seconds.

use std::ops::Range;

use crate::easing::{Easing, EASING_NAMES};
use crate::image::{scale_px, Edge, Image};
use crate::math::{hash1, noise1, TAU};
use crate::ops::gaussian_blur;
use crate::params::{ParamDef, ParamSet, Unit};

/// Where a layer is drawn relative to its resting place.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    /// Offset as a fraction of the frame width (x) and height (y).
    pub offset: [f32; 2],
    /// Scale about the layer centre.
    pub scale: [f32; 2],
    /// Clockwise rotation in degrees.
    pub rotation: f32,
    /// 0..1, multiplies the layer's opacity.
    pub opacity: f32,
    /// Blur radius in percent of frame height.
    pub blur: f32,
    /// Visible part of the layer as insets from its left, top, right and
    /// bottom edges, each a fraction of the layer size (0 = no clipping).
    pub clip: [f32; 4],
    /// Turn in 3D about the layer's horizontal and vertical axes, degrees:
    /// positive tips the top edge away and turns the right edge away. The
    /// renderer shows it in perspective.
    pub turn: [f32; 2],
    /// Pushed away from the viewer, as a fraction of the frame width
    /// (negative comes closer).
    pub depth: f32,
}

impl Pose {
    /// The layer at rest.
    pub const REST: Pose = Pose { offset: [0.0; 2], scale: [1.0; 2], rotation: 0.0, opacity: 1.0, blur: 0.0, clip: [0.0; 4], turn: [0.0; 2], depth: 0.0 };

    /// True when within `eps` of [`Pose::REST`] in every field.
    pub fn is_rest(&self, eps: f32) -> bool {
        let r = Pose::REST;
        let close = |a: f32, b: f32| (a - b).abs() <= eps;
        (0..2).all(|i| close(self.offset[i], r.offset[i]) && close(self.scale[i], r.scale[i]))
            && close(self.rotation, 0.0)
            && close(self.opacity, 1.0)
            && close(self.blur, 0.0)
            && self.clip.iter().all(|&c| close(c, 0.0))
            && self.turn.iter().all(|&c| close(c, 0.0))
            && close(self.depth, 0.0)
    }

    pub fn is_finite(&self) -> bool {
        self.offset.iter().chain(&self.scale).chain(&self.clip).chain(&self.turn).chain([self.rotation, self.opacity, self.blur, self.depth].iter()).all(|v| v.is_finite())
    }

    /// Combines two poses: offsets and rotations add, scales and opacity
    /// multiply, blur and clip take the larger. Used to stack a loop on top
    /// of an entry.
    pub fn then(&self, o: &Pose) -> Pose {
        Pose {
            offset: [self.offset[0] + o.offset[0], self.offset[1] + o.offset[1]],
            scale: [self.scale[0] * o.scale[0], self.scale[1] * o.scale[1]],
            rotation: self.rotation + o.rotation,
            opacity: self.opacity * o.opacity,
            blur: self.blur.max(o.blur),
            clip: std::array::from_fn(|i| self.clip[i].max(o.clip[i])),
            turn: [self.turn[0] + o.turn[0], self.turn[1] + o.turn[1]],
            depth: self.depth + o.depth,
        }
    }
}

/// Whether a preset is an entry/exit motion or a loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MotionKind {
    /// Usable as an entry (played forwards) or an exit (played backwards).
    InOut,
    Loop,
}

/// Which end of a clip an in/out motion plays at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Entry,
    Exit,
}

/// One motion preset.
#[derive(Clone, Copy)]
pub struct MotionDef {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub kind: MotionKind,
    pub params: &'static [ParamDef],
    eval: fn(f32, &ParamSet) -> Pose,
}

impl std::fmt::Debug for MotionDef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MotionDef").field("id", &self.id).finish()
    }
}

impl MotionDef {
    pub fn defaults(&self) -> ParamSet {
        ParamSet::defaults(self.params)
    }

    fn check(&self, params: &ParamSet) {
        assert!(crate::effects::same_defs(params.defs(), self.params), "parameter set belongs to another motion");
    }

    /// Pose of an in/out motion. `progress` runs 0..1 over the motion's
    /// duration: an entry ends at rest, an exit starts at rest.
    pub fn pose(&self, phase: Phase, progress: f32, params: &ParamSet) -> Pose {
        self.check(params);
        debug_assert_eq!(self.kind, MotionKind::InOut, "`{}` is a loop; use `loop_pose`", self.id);
        let t = if progress.is_nan() { 1.0 } else { progress.clamp(0.0, 1.0) };
        let t = match phase {
            Phase::Entry => t,
            Phase::Exit => 1.0 - t,
        };
        let e = Easing::from_index(params.choice("easing")).apply(t);
        // The ends are exact so clips start and finish precisely at rest.
        if t >= 1.0 {
            return Pose::REST;
        }
        (self.eval)(e, params)
    }

    /// Pose of a loop motion `time` seconds into the clip.
    pub fn loop_pose(&self, time: f32, params: &ParamSet) -> Pose {
        self.check(params);
        debug_assert_eq!(self.kind, MotionKind::Loop, "`{}` is not a loop", self.id);
        let period = params.f("period").max(0.05);
        let phase = if time.is_finite() { time / period } else { 0.0 };
        (self.eval)(phase, params)
    }
}

/// Film-like default shutter for motion blur: open for half of each frame.
pub const DEFAULT_SHUTTER: f32 = 180.0;

/// Times (seconds, relative to the frame time) at which the shutter samples
/// a frame: `samples` instants spread evenly over the open shutter.
fn shutter_offsets(frame_rate: f32, shutter_deg: f32, samples: usize) -> Vec<f32> {
    let open = shutter_deg.clamp(0.0, 360.0) / 360.0 / frame_rate.max(1.0);
    let n = samples.max(1);
    if n == 1 || open <= 0.0 {
        return vec![0.0];
    }
    (0..n).map(|i| open * (i as f32 / (n - 1) as f32 - 0.5)).collect()
}

impl MotionDef {
    /// Poses for motion blur: the in/out motion sampled across the open
    /// shutter of the frame at `progress`, for a motion lasting `duration`
    /// seconds. Draw each with [`apply_pose`] and average them (or use
    /// [`apply_poses`]). Fast moves smear along their path the way a camera
    /// records them; at rest every sample is identical.
    #[allow(clippy::too_many_arguments)]
    pub fn shutter_poses(
        &self,
        phase: Phase,
        progress: f32,
        duration: f32,
        frame_rate: f32,
        shutter_deg: f32,
        samples: usize,
        params: &ParamSet,
    ) -> Vec<Pose> {
        let d = if duration.is_finite() { duration.max(1e-3) } else { 1.0 };
        shutter_offsets(frame_rate, shutter_deg, samples)
            .into_iter()
            .map(|dt| self.pose(phase, (progress + dt / d).clamp(0.0, 1.0), params))
            .collect()
    }

    /// Loop counterpart of [`MotionDef::shutter_poses`].
    pub fn loop_shutter_poses(&self, time: f32, frame_rate: f32, shutter_deg: f32, samples: usize, params: &ParamSet) -> Vec<Pose> {
        shutter_offsets(frame_rate, shutter_deg, samples).into_iter().map(|dt| self.loop_pose(time + dt, params)).collect()
    }
}

/// Every motion preset: in/out ones first, then loops.
pub fn all() -> &'static [MotionDef] {
    MOTIONS
}

pub fn find(id: &str) -> Option<&'static MotionDef> {
    MOTIONS.iter().find(|m| m.id == id)
}

const fn easing(default: u32) -> ParamDef {
    ParamDef::choice("easing", "Easing", EASING_NAMES, default)
}

const CUBIC_OUT: u32 = 8;
const QUINT_OUT: u32 = 11;
const BACK_OUT: u32 = 17;
const ELASTIC_OUT: u32 = 19;
const BOUNCE_OUT: u32 = 20;
const LINEAR: u32 = 0;

const DISTANCE: ParamDef = ParamDef::float("distance", "Distance", 0.0, 1.0, 0.15).unit(Unit::Fraction);
const FADE: ParamDef = ParamDef::bool("fade", "Fade", true);
const PERIOD: ParamDef = ParamDef::float("period", "Period", 0.1, 10.0, 2.0).unit(Unit::Seconds);

macro_rules! in_out {
    ($id:literal, $name:literal, $desc:literal, [$($p:expr),* $(,)?], $f:expr) => {
        MotionDef { id: $id, name: $name, description: $desc, kind: MotionKind::InOut, params: &[$($p),*], eval: $f }
    };
}

macro_rules! looped {
    ($id:literal, $name:literal, $desc:literal, [$($p:expr),* $(,)?], $f:expr) => {
        MotionDef { id: $id, name: $name, description: $desc, kind: MotionKind::Loop, params: &[$($p),*], eval: $f }
    };
}

/// Opacity ramp for presets with a fade switch: reaches full a little
/// before the motion ends so the arrival reads clearly.
fn fade(e: f32, p: &ParamSet) -> f32 {
    if p.b("fade") { (e * 1.6).clamp(0.0, 1.0) } else { 1.0 }
}

static MOTIONS: &[MotionDef] = &[
    in_out!("fade", "Fade", "Fades in from transparent.", [easing(LINEAR)], |e, _| Pose { opacity: e.clamp(0.0, 1.0), ..Pose::REST }),
    in_out!("rise", "Rise", "Floats up into place.", [DISTANCE, FADE, easing(QUINT_OUT)], |e, p| Pose {
        offset: [0.0, (1.0 - e) * p.f("distance")],
        opacity: fade(e, p),
        ..Pose::REST
    }),
    in_out!("fall", "Fall", "Drops down into place.", [DISTANCE, FADE, easing(QUINT_OUT)], |e, p| Pose {
        offset: [0.0, -(1.0 - e) * p.f("distance")],
        opacity: fade(e, p),
        ..Pose::REST
    }),
    in_out!("drift_left", "Drift left", "Glides in from the right.", [DISTANCE, FADE, easing(QUINT_OUT)], |e, p| Pose {
        offset: [(1.0 - e) * p.f("distance"), 0.0],
        opacity: fade(e, p),
        ..Pose::REST
    }),
    in_out!("drift_right", "Drift right", "Glides in from the left.", [DISTANCE, FADE, easing(QUINT_OUT)], |e, p| Pose {
        offset: [-(1.0 - e) * p.f("distance"), 0.0],
        opacity: fade(e, p),
        ..Pose::REST
    }),
    in_out!(
        "zoom_in",
        "Zoom in",
        "Grows from small.",
        [ParamDef::float("start_scale", "Start size", 0.0, 1.0, 0.5).unit(Unit::Times), FADE, easing(CUBIC_OUT)],
        |e, p| {
            let s = p.f("start_scale") + (1.0 - p.f("start_scale")) * e;
            Pose { scale: [s, s], opacity: fade(e, p), ..Pose::REST }
        }
    ),
    in_out!(
        "zoom_out",
        "Zoom out",
        "Shrinks into place from large.",
        [ParamDef::float("start_scale", "Start size", 1.0, 4.0, 1.6).unit(Unit::Times), FADE, easing(CUBIC_OUT)],
        |e, p| {
            let s = p.f("start_scale") + (1.0 - p.f("start_scale")) * e;
            Pose { scale: [s, s], opacity: fade(e, p), ..Pose::REST }
        }
    ),
    in_out!("pop", "Pop", "Springs out from nothing with a little overshoot.", [easing(BACK_OUT)], |e, _| Pose {
        scale: [e.max(0.0), e.max(0.0)],
        opacity: (e * 3.0).clamp(0.0, 1.0),
        ..Pose::REST
    }),
    in_out!(
        "spin_in",
        "Spin in",
        "Twirls in while growing.",
        [ParamDef::float("angle", "Angle", -720.0, 720.0, -180.0).unit(Unit::Degrees), FADE, easing(CUBIC_OUT)],
        |e, p| {
            let s = 0.3 + 0.7 * e;
            Pose { rotation: (1.0 - e) * p.f("angle"), scale: [s, s], opacity: fade(e, p), ..Pose::REST }
        }
    ),
    in_out!("tumble", "Tumble", "Tumbles in from above, rolling as it lands.", [DISTANCE, easing(BOUNCE_OUT)], |e, p| Pose {
        offset: [-(1.0 - e) * p.f("distance") * 0.5, -(1.0 - e) * p.f("distance") * 2.0],
        rotation: -(1.0 - e) * 120.0,
        opacity: (e * 4.0).clamp(0.0, 1.0),
        ..Pose::REST
    }),
    in_out!(
        "swing",
        "Swing",
        "Swings into place and settles like a sign on a hook.",
        [ParamDef::float("angle", "Angle", -90.0, 90.0, 35.0).unit(Unit::Degrees), easing(ELASTIC_OUT)],
        |e, p| Pose { rotation: (1.0 - e) * p.f("angle"), opacity: (e * 4.0).clamp(0.0, 1.0), ..Pose::REST }
    ),
    in_out!("flip_x", "Flip X", "Turns round a vertical axis to face the viewer.", [FADE, easing(CUBIC_OUT)], |e, p| Pose {
        scale: [(e.clamp(-1.0, 2.0) * std::f32::consts::FRAC_PI_2).sin(), 1.0],
        opacity: fade(e, p),
        ..Pose::REST
    }),
    in_out!("flip_y", "Flip Y", "Turns round a horizontal axis to face the viewer.", [FADE, easing(CUBIC_OUT)], |e, p| Pose {
        scale: [1.0, (e.clamp(-1.0, 2.0) * std::f32::consts::FRAC_PI_2).sin()],
        opacity: fade(e, p),
        ..Pose::REST
    }),
    in_out!("squash", "Squash", "Drops in stretched and lands with a squash.", [DISTANCE, easing(BACK_OUT)], |e, p| {
        let k = 1.0 - e;
        let bump = (std::f32::consts::PI * e.clamp(0.0, 1.0)).sin();
        Pose {
            offset: [0.0, -k * p.f("distance")],
            // Tall and thin while falling, briefly wide and short on landing.
            scale: [1.0 - 0.3 * k + 0.15 * bump, 1.0 + 0.4 * k - 0.15 * bump],
            opacity: (e * 3.0).clamp(0.0, 1.0),
            ..Pose::REST
        }
    }),
    in_out!(
        "typewriter",
        "Typewriter",
        "Reveals left to right in hard steps, like typing.",
        [ParamDef::int("steps", "Steps", 2, 64, 16), easing(LINEAR)],
        |e, p| {
            let n = p.i("steps") as f32;
            Pose { clip: [0.0, 0.0, 1.0 - (e.clamp(0.0, 1.0) * n).floor() / n, 0.0], ..Pose::REST }
        }
    ),
    in_out!(
        "wipe_reveal",
        "Wipe reveal",
        "Uncovered by a straight edge.",
        [ParamDef::choice("direction", "Direction", &["Left to right", "Right to left", "Top down", "Bottom up"], 0), easing(CUBIC_OUT)],
        |e, p| {
            let k = 1.0 - e.clamp(0.0, 1.0);
            let mut clip = [0.0; 4];
            clip[[2, 0, 3, 1][p.choice("direction")]] = k;
            Pose { clip, ..Pose::REST }
        }
    ),
    in_out!(
        "blur_in",
        "Blur in",
        "Comes into focus as it fades in.",
        [ParamDef::float("blur", "Blur", 0.0, 10.0, 3.0).unit(Unit::PercentHeight), easing(CUBIC_OUT)],
        |e, p| Pose { blur: (1.0 - e).max(0.0) * p.f("blur"), opacity: e.clamp(0.0, 1.0), ..Pose::REST }
    ),
    in_out!(
        "turn_in",
        "Turn in (3D)",
        "Turns round its vertical axis to face the viewer, in perspective.",
        [ParamDef::float("angle", "Angle", -180.0, 180.0, 90.0).unit(Unit::Degrees), FADE, easing(CUBIC_OUT)],
        |e, p| Pose { turn: [0.0, (1.0 - e) * p.f("angle")], opacity: fade(e, p), ..Pose::REST }
    ),
    in_out!(
        "tilt_in",
        "Tilt up (3D)",
        "Stands up from lying back, round its bottom-to-top axis, in perspective.",
        [ParamDef::float("angle", "Angle", -180.0, 180.0, 75.0).unit(Unit::Degrees), FADE, easing(QUINT_OUT)],
        |e, p| Pose { turn: [(1.0 - e) * p.f("angle"), 0.0], opacity: fade(e, p), ..Pose::REST }
    ),
    in_out!(
        "card_flip",
        "Card flip (3D)",
        "Flips over like a playing card to show its face.",
        [ParamDef::choice("axis", "Axis", &["Vertical", "Horizontal"], 0), easing(BACK_OUT)],
        |e, p| {
            let a = (1.0 - e) * 180.0;
            // The back of the card is blank: hidden until the face turns
            // towards the viewer.
            let opacity = if a.abs() > 90.0 { 0.0 } else { 1.0 };
            let turn = if p.choice("axis") == 0 { [0.0, a] } else { [a, 0.0] };
            Pose { turn, opacity, ..Pose::REST }
        }
    ),
    in_out!(
        "swivel",
        "Swivel (3D)",
        "Swings round its vertical axis and settles with a little overshoot.",
        [ParamDef::float("angle", "Angle", -180.0, 180.0, -70.0).unit(Unit::Degrees), easing(BACK_OUT)],
        |e, p| Pose { turn: [0.0, (1.0 - e) * p.f("angle")], opacity: (e * 3.0).clamp(0.0, 1.0), ..Pose::REST }
    ),
    in_out!(
        "fly_in",
        "Fly in (3D)",
        "Flies in from deep in the distance (or from behind the viewer).",
        [
            ParamDef::float("distance", "Distance", -0.9, 10.0, 3.0).unit(Unit::Times),
            ParamDef::float("angle", "Turn", -90.0, 90.0, 20.0).unit(Unit::Degrees),
            FADE,
            easing(QUINT_OUT),
        ],
        |e, p| {
            let k = 1.0 - e;
            Pose { depth: k * p.f("distance"), turn: [0.0, k * p.f("angle")], opacity: fade(e, p), ..Pose::REST }
        }
    ),
    in_out!(
        "fall_back",
        "Fall back (3D)",
        "Falls forward out of the distance onto its feet.",
        [ParamDef::float("distance", "Distance", 0.0, 4.0, 0.6).unit(Unit::Times), FADE, easing(BOUNCE_OUT)],
        |e, p| {
            let k = 1.0 - e;
            Pose { depth: k * p.f("distance"), turn: [k * 80.0, 0.0], opacity: fade(e, p), ..Pose::REST }
        }
    ),
    looped!(
        "float",
        "Float",
        "Bobs gently up and down.",
        [ParamDef::float("amount", "Height", 0.0, 0.2, 0.015).unit(Unit::Fraction), PERIOD],
        |t, p| {
            let a = p.f("amount");
            Pose { offset: [0.0, (TAU * t).sin() * a], rotation: (TAU * t + 1.0).sin() * a * 40.0, ..Pose::REST }
        }
    ),
    looped!(
        "pulse",
        "Pulse",
        "Beats like a heart.",
        [ParamDef::float("amount", "Amount", 0.0, 0.5, 0.06).unit(Unit::Fraction), ParamDef::float("period", "Period", 0.1, 10.0, 0.8).unit(Unit::Seconds)],
        |t, p| {
            // A quick double beat then rest.
            let f = t.rem_euclid(1.0);
            let beat = |c: f32| (-((f - c) / 0.06).powi(2)).exp();
            let s = 1.0 + p.f("amount") * (beat(0.12) + 0.6 * beat(0.32));
            Pose { scale: [s, s], ..Pose::REST }
        }
    ),
    looped!(
        "sway",
        "Sway",
        "Rocks side to side.",
        [ParamDef::float("amount", "Angle", 0.0, 45.0, 5.0).unit(Unit::Degrees), PERIOD],
        |t, p| Pose { rotation: (TAU * t).sin() * p.f("amount"), ..Pose::REST }
    ),
    looped!(
        "orbit",
        "Orbit",
        "Circles round its resting place.",
        [ParamDef::float("amount", "Radius", 0.0, 0.2, 0.02).unit(Unit::Fraction), PERIOD],
        |t, p| {
            let a = p.f("amount");
            Pose { offset: [(TAU * t).cos() * a - a, (TAU * t).sin() * a], ..Pose::REST }
        }
    ),
    looped!(
        "jitter",
        "Jitter",
        "Nervous, stepped shaking.",
        [
            ParamDef::float("amount", "Amount", 0.0, 0.1, 0.006).unit(Unit::Fraction),
            ParamDef::float("period", "Step time", 0.02, 1.0, 0.08).unit(Unit::Seconds),
            ParamDef::int("seed", "Variation", 0, 999, 0),
        ],
        |t, p| {
            let step = t.floor() as i32;
            let s = p.i("seed") as u32;
            let a = p.f("amount");
            Pose {
                offset: [(hash1(step, s) - 0.5) * 2.0 * a, (hash1(step, s ^ 0xabc) - 0.5) * 2.0 * a],
                rotation: (hash1(step, s ^ 0x123) - 0.5) * a * 200.0,
                ..Pose::REST
            }
        }
    ),
    looped!(
        "breathe",
        "Breathe",
        "Slowly swells and relaxes.",
        [ParamDef::float("amount", "Amount", 0.0, 0.3, 0.03).unit(Unit::Fraction), ParamDef::float("period", "Period", 0.1, 10.0, 4.0).unit(Unit::Seconds)],
        |t, p| {
            let s = 1.0 + p.f("amount") * (0.5 - 0.5 * (TAU * t).cos());
            Pose { scale: [s, s], ..Pose::REST }
        }
    ),
    looped!(
        "wobble",
        "Wobble",
        "Jelly-like squash and stretch.",
        [ParamDef::float("amount", "Amount", 0.0, 0.3, 0.05).unit(Unit::Fraction), ParamDef::float("period", "Period", 0.1, 10.0, 0.6).unit(Unit::Seconds)],
        |t, p| {
            let w = (TAU * t).sin() * p.f("amount");
            Pose { scale: [1.0 + w, 1.0 - w], ..Pose::REST }
        }
    ),
    looped!(
        "bounce",
        "Bounce",
        "Hops up and down, squashing on landing.",
        [ParamDef::float("amount", "Height", 0.0, 0.3, 0.04).unit(Unit::Fraction), ParamDef::float("period", "Period", 0.1, 10.0, 0.7).unit(Unit::Seconds)],
        |t, p| {
            let f = t.rem_euclid(1.0);
            let a = p.f("amount");
            let h = (std::f32::consts::PI * f).sin();
            // Squash for the moments around the floor contact.
            let contact = (1.0 - h).powi(6) * a * 3.0;
            Pose { offset: [0.0, -h * a], scale: [1.0 + contact, 1.0 - contact], ..Pose::REST }
        }
    ),
    looped!(
        "spin",
        "Spin",
        "Keeps turning.",
        [ParamDef::float("turns", "Turns per period", -4.0, 4.0, 1.0).unit(Unit::Times), PERIOD],
        |t, p| Pose { rotation: 360.0 * t * p.f("turns"), ..Pose::REST }
    ),
    looped!(
        "drift",
        "Wander",
        "Meanders slowly around its place.",
        [ParamDef::float("amount", "Amount", 0.0, 0.2, 0.02).unit(Unit::Fraction), PERIOD, ParamDef::int("seed", "Variation", 0, 999, 0)],
        |t, p| {
            let s = p.i("seed") as u32;
            let a = p.f("amount");
            Pose { offset: [noise1(t, s) * a, noise1(t + 50.0, s ^ 7) * a], rotation: noise1(t * 0.7, s ^ 9) * a * 60.0, ..Pose::REST }
        }
    ),
    looped!(
        "spin_3d",
        "Spin (3D)",
        "Keeps turning round its vertical axis, like a sign on a pole.",
        [ParamDef::float("turns", "Turns per period", -4.0, 4.0, 1.0).unit(Unit::Times), ParamDef::choice("axis", "Axis", &["Vertical", "Horizontal"], 0), PERIOD],
        |t, p| {
            let a = 360.0 * t * p.f("turns");
            Pose { turn: if p.choice("axis") == 0 { [0.0, a] } else { [a, 0.0] }, ..Pose::REST }
        }
    ),
    looped!(
        "rock_3d",
        "Rock (3D)",
        "Rocks gently in depth, turning a little left and right.",
        [ParamDef::float("amount", "Angle", 0.0, 60.0, 12.0).unit(Unit::Degrees), PERIOD],
        |t, p| Pose { turn: [0.0, (TAU * t).sin() * p.f("amount")], ..Pose::REST }
    ),
    looped!(
        "hover_3d",
        "Hover (3D)",
        "Floats in space, tipping slowly on both axes like a card on air.",
        [ParamDef::float("amount", "Angle", 0.0, 45.0, 8.0).unit(Unit::Degrees), ParamDef::float("period", "Period", 0.1, 20.0, 5.0).unit(Unit::Seconds)],
        |t, p| {
            let a = p.f("amount");
            // A little nearer and further with the tip (none at rest).
            Pose { turn: [(TAU * t).sin() * a * 0.6, (TAU * t + 1.3).cos() * a], depth: (TAU * t).sin() * a * 0.0025, ..Pose::REST }
        }
    ),
];

/// How text is split for staggered motion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextUnit {
    Character,
    Word,
    Line,
}

/// The order units start in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StaggerOrder {
    Forward,
    Backward,
    /// From the middle out.
    Centre,
    /// Shuffled by a seed.
    Random(u32),
}

/// Byte ranges of the units in `text`. Whitespace is never a unit, so
/// spaces do not take up a slot in the stagger.
pub fn split_text(text: &str, unit: TextUnit) -> Vec<Range<usize>> {
    match unit {
        TextUnit::Character => text.char_indices().filter(|(_, c)| !c.is_whitespace()).map(|(i, c)| i..i + c.len_utf8()).collect(),
        TextUnit::Word => {
            let mut out = Vec::new();
            let mut start = None;
            for (i, c) in text.char_indices() {
                match (c.is_whitespace(), start) {
                    (false, None) => start = Some(i),
                    (true, Some(s)) => {
                        out.push(s..i);
                        start = None;
                    }
                    _ => {}
                }
            }
            if let Some(s) = start {
                out.push(s..text.len());
            }
            out
        }
        TextUnit::Line => {
            let mut out = Vec::new();
            let mut pos = 0;
            for line in text.split('\n') {
                let trimmed = line.trim_end_matches('\r');
                if !trimmed.trim().is_empty() {
                    out.push(pos..pos + trimmed.len());
                }
                pos += line.len() + 1;
            }
            out
        }
    }
}

/// Local progress of unit `index` of `count` when the whole text is at
/// `progress`. `overlap` 0 runs units strictly one after another, 1 runs
/// them all together; in between each unit starts while the previous one
/// is still moving.
pub fn stagger(progress: f32, index: usize, count: usize, overlap: f32, order: StaggerOrder) -> f32 {
    // The ends are exact whatever the rounding inside.
    if count <= 1 || !(progress > 0.0) || progress >= 1.0 {
        return if progress >= 1.0 { 1.0 } else if progress > 0.0 { progress } else { 0.0 };
    }
    let slot = match order {
        StaggerOrder::Forward => index as f32,
        StaggerOrder::Backward => (count - 1 - index) as f32,
        StaggerOrder::Centre => (index as f32 - (count - 1) as f32 * 0.5).abs() * 2.0,
        StaggerOrder::Random(seed) => {
            // Rank of this unit's hash among all units: a stable shuffle.
            let h = |i: usize| hash1(i as i32, seed);
            let mine = h(index);
            (0..count).filter(|&j| h(j) < mine || (h(j) == mine && j < index)).count() as f32
        }
    };
    let slots = (count - 1) as f32;
    let overlap = overlap.clamp(0.0, 1.0);
    // Each unit lasts `d`; starts are spaced `d * (1 - overlap)` apart and
    // the last one ends exactly at 1.
    let d = 1.0 / (1.0 + slots * (1.0 - overlap));
    let start = slot.min(slots) * d * (1.0 - overlap);
    ((progress - start) / d).clamp(0.0, 1.0)
}

/// Poses for every unit of a staggered text motion.
pub fn text_poses(
    motion: &MotionDef,
    phase: Phase,
    progress: f32,
    params: &ParamSet,
    count: usize,
    overlap: f32,
    order: StaggerOrder,
) -> Vec<Pose> {
    (0..count).map(|i| motion.pose(phase, stagger(progress, i, count, overlap, order), params)).collect()
}

/// Draws a full-frame layer with a pose: blur, then clip, then scale,
/// rotation and offset about the frame centre, then opacity. Areas the
/// layer no longer covers become transparent. The renderer has its own
/// GPU path; this is the reference and is used for previews.
pub fn apply_pose(img: &Image, pose: &Pose) -> Image {
    if pose.is_rest(0.0) {
        return img.clone();
    }
    let (w, h) = (img.width as f32, img.height as f32);
    let blurred;
    let src = if pose.blur > 0.0 {
        let r = pose.blur * 0.01 * h * 0.5;
        blurred = gaussian_blur(img, r, r, Edge::Transparent);
        &blurred
    } else {
        img
    };
    let (sn, cs) = (-pose.rotation.to_radians()).sin_cos();
    // Flat pictures (text units) show a 3D turn as foreshortening and depth
    // as shrinking towards the vanishing point.
    let far = 1.0 / (1.0 + pose.depth / 1.389).max(0.05);
    let (sx, sy) = (pose.scale[0] * pose.turn[1].to_radians().cos() * far, pose.scale[1] * pose.turn[0].to_radians().cos() * far);
    let (ox, oy) = (pose.offset[0] * w, pose.offset[1] * h);
    let [cl, ct, cr, cb] = pose.clip;
    let opacity = pose.opacity.clamp(0.0, 1.0);
    Image::from_fn(img.width, img.height, |x, y| {
        if sx.abs() < 1e-6 || sy.abs() < 1e-6 || opacity <= 0.0 {
            return [0.0; 4];
        }
        let (dx, dy) = (x as f32 + 0.5 - w * 0.5 - ox, y as f32 + 0.5 - h * 0.5 - oy);
        let (rx, ry) = (dx * cs - dy * sn, dx * sn + dy * cs);
        let (lx, ly) = (rx / sx + w * 0.5, ry / sy + h * 0.5);
        let (u, v) = (lx / w, ly / h);
        if u < cl || v < ct || u > 1.0 - cr || v > 1.0 - cb {
            return [0.0; 4];
        }
        scale_px(src.sample(lx, ly, Edge::Transparent), opacity)
    })
}

/// Draws a layer once per pose and averages the results: motion blur from
/// [`MotionDef::shutter_poses`]. Identical poses are drawn only once.
pub fn apply_poses(img: &Image, poses: &[Pose]) -> Image {
    let Some(first) = poses.first() else { return img.clone() };
    if poses.iter().all(|p| p == first) {
        return apply_pose(img, first);
    }
    let mut acc = Image::new(img.width, img.height);
    for pose in poses {
        let layer = apply_pose(img, pose);
        for (a, b) in acc.pixels.iter_mut().zip(&layer.pixels) {
            for c in 0..4 {
                a[c] += b[c];
            }
        }
    }
    let k = 1.0 / poses.len() as f32;
    acc.pixels.iter_mut().for_each(|px| *px = scale_px(*px, k));
    acc
}
