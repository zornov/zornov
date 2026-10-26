// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Per-unit animation: reveal (entrance/exit) presets and loop motions
//! expressed as [`UnitPose`]s, with stagger across characters, words or
//! lines. Templates and the caption renderer use these; `fx` can call
//! them for its text stagger too.

use serde::{Deserialize, Serialize};

use crate::geom::Ease;
use crate::paint::UnitPose;
use crate::rgba::Rgba;
use crate::style::TextUnit;

/// Entrance / exit motion shapes. Named for what the viewer sees.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevealKind {
    #[default]
    Fade,
    /// Rises from below while fading in.
    Rise,
    /// Drops from above.
    Drop,
    /// Scales up from nothing with a little overshoot.
    Pop,
    /// Slides in from the left.
    SlideLeft,
    /// Slides in from the right.
    SlideRight,
    /// Characters appear one by one with no motion (typewriter).
    Type,
    /// Spins in while scaling.
    Spin,
    /// Falls past the baseline and bounces back.
    Bounce,
    /// Scales down from large while fading.
    Zoom,
    /// Flips (horizontal squash) into place.
    Flip,
    /// Rises with an elastic settle.
    Spring,
}

/// Which way the stagger runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Order {
    #[default]
    Forward,
    Backward,
    Centre,
    Random,
}

/// A reveal: kind, unit, stagger and easing. `overlap` 0 runs units one
/// after another, 1 runs them all together.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reveal {
    pub kind: RevealKind,
    #[serde(default)]
    pub unit: TextUnit,
    #[serde(default = "default_overlap")]
    pub overlap: f32,
    #[serde(default)]
    pub order: Order,
    #[serde(default)]
    pub ease: Ease,
    /// Travel distance for moving kinds, as a fraction of the unit's size
    /// (1 = one unit height).
    #[serde(default = "one")]
    pub distance: f32,
}

fn default_overlap() -> f32 {
    0.7
}

fn one() -> f32 {
    1.0
}

impl Default for Reveal {
    fn default() -> Self {
        Reveal { kind: RevealKind::Fade, unit: TextUnit::Character, overlap: 0.7, order: Order::Forward, ease: Ease::Out, distance: 1.0 }
    }
}

impl Reveal {
    pub fn new(kind: RevealKind, unit: TextUnit) -> Reveal {
        let ease = match kind {
            RevealKind::Pop | RevealKind::Spring => Ease::Overshoot,
            RevealKind::Bounce => Ease::Bounce,
            RevealKind::Type => Ease::Hold,
            _ => Ease::Out,
        };
        Reveal { kind, unit, ease, ..Default::default() }
    }

    pub fn overlap(mut self, o: f32) -> Reveal {
        self.overlap = o.clamp(0.0, 1.0);
        self
    }

    pub fn order(mut self, o: Order) -> Reveal {
        self.order = o;
        self
    }

    pub fn ease(mut self, e: Ease) -> Reveal {
        self.ease = e;
        self
    }

    pub fn distance(mut self, d: f32) -> Reveal {
        self.distance = d;
        self
    }

    /// Pose of unit `index` of `count` when the whole reveal is at
    /// `progress` (0 = hidden, 1 = fully shown). `size_px` scales the
    /// motion. For an exit, call with `1 - progress`.
    pub fn pose(&self, progress: f32, index: usize, count: usize, size_px: f32) -> UnitPose {
        let local = stagger(progress, index, count, self.overlap, self.order);
        let t = self.ease.apply(local);
        let d = size_px * self.distance;
        let fade = local.clamp(0.0, 1.0);
        match self.kind {
            RevealKind::Fade => UnitPose::REST.opacity(fade),
            RevealKind::Rise => UnitPose::REST.offset(0.0, (1.0 - t) * d).opacity(fade),
            RevealKind::Drop => UnitPose::REST.offset(0.0, -(1.0 - t) * d).opacity(fade),
            RevealKind::Pop => UnitPose::REST.scale(t.max(0.0)).opacity(fade.min(1.0)),
            RevealKind::SlideLeft => UnitPose::REST.offset(-(1.0 - t) * d * 1.5, 0.0).opacity(fade),
            RevealKind::SlideRight => UnitPose::REST.offset((1.0 - t) * d * 1.5, 0.0).opacity(fade),
            RevealKind::Type => UnitPose::REST.opacity(if local > 0.0 { 1.0 } else { 0.0 }),
            RevealKind::Spin => UnitPose::REST.rotation((1.0 - t) * 90.0).scale(t.max(0.0)).opacity(fade),
            RevealKind::Bounce => UnitPose::REST.offset(0.0, -(1.0 - t) * d * 1.2).opacity(fade.min(1.0)),
            RevealKind::Zoom => UnitPose::REST.scale(1.0 + (1.0 - t) * 1.5).opacity(fade),
            RevealKind::Flip => UnitPose { scale: [t.max(0.02), 1.0], opacity: fade, ..UnitPose::REST },
            RevealKind::Spring => UnitPose::REST.offset(0.0, (1.0 - t) * d).opacity(fade.min(1.0)),
        }
    }
}

/// Local progress of unit `index` of `count` for overall `progress`.
/// Same shape as `fx`'s text stagger so the two agree: starts are spaced
/// `d·(1 − overlap)` apart and the last unit ends at 1.
pub fn stagger(progress: f32, index: usize, count: usize, overlap: f32, order: Order) -> f32 {
    if count <= 1 || progress <= 0.0 || progress >= 1.0 {
        return progress.clamp(0.0, 1.0);
    }
    let slots = (count - 1) as f32;
    let slot = match order {
        Order::Forward => index as f32,
        Order::Backward => (count - 1 - index) as f32,
        Order::Centre => (index as f32 - slots * 0.5).abs() * 2.0,
        Order::Random => {
            let h = |i: usize| hash(i as u32);
            let mine = h(index);
            (0..count).filter(|&j| h(j) < mine || (h(j) == mine && j < index)).count() as f32
        }
    };
    let overlap = overlap.clamp(0.0, 1.0);
    let d = 1.0 / (1.0 + slots * (1.0 - overlap));
    let start = slot.min(slots) * d * (1.0 - overlap);
    ((progress - start) / d).clamp(0.0, 1.0)
}

fn hash(i: u32) -> u32 {
    let mut x = i.wrapping_mul(0x9E3779B1) ^ 0x85EBCA6B;
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B3C6D);
    x ^= x >> 12;
    x
}

/// Continuous motions that run while the text is on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopKind {
    /// Characters bob up and down in sequence.
    Wave,
    /// Whole units breathe in scale.
    Pulse,
    /// Small random jitter, like a nervous sticker.
    Jitter,
    /// Gentle left-right sway.
    Sway,
    /// A bright colour sweeps across the units.
    Shimmer,
    /// Units tilt back and forth.
    Wobble,
}

/// A loop motion: `speed` in cycles per second, `amount` in fractions of
/// the font size (or degrees for rotations, scaled by 15°).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LoopMotion {
    pub kind: LoopKind,
    #[serde(default)]
    pub unit: TextUnit,
    #[serde(default = "one")]
    pub speed: f32,
    #[serde(default = "half")]
    pub amount: f32,
    /// Colour for `Shimmer`.
    #[serde(default)]
    pub colour: Option<Rgba>,
}

fn half() -> f32 {
    0.5
}

impl LoopMotion {
    pub fn new(kind: LoopKind, unit: TextUnit) -> LoopMotion {
        LoopMotion { kind, unit, speed: 1.0, amount: 0.5, colour: None }
    }

    pub fn speed(mut self, s: f32) -> LoopMotion {
        self.speed = s;
        self
    }

    pub fn amount(mut self, a: f32) -> LoopMotion {
        self.amount = a;
        self
    }

    pub fn colour(mut self, c: Rgba) -> LoopMotion {
        self.colour = Some(c);
        self
    }

    /// Pose of unit `index` at `time` seconds.
    pub fn pose(&self, time: f32, index: usize, count: usize, size_px: f32) -> UnitPose {
        let tau = std::f32::consts::TAU;
        let phase = time * self.speed * tau;
        let a = self.amount;
        let i = index as f32;
        match self.kind {
            LoopKind::Wave => UnitPose::REST.offset(0.0, (phase - i * 0.6).sin() * a * size_px * 0.3),
            LoopKind::Pulse => UnitPose::REST.scale(1.0 + (phase - i * 0.3).sin() * a * 0.15),
            LoopKind::Jitter => {
                let frame = (time * 12.0 * self.speed.max(0.1)).floor() as u32;
                let h1 = hash(frame.wrapping_mul(31).wrapping_add(index as u32)) as f32 / u32::MAX as f32 - 0.5;
                let h2 = hash(frame.wrapping_mul(17).wrapping_add(index as u32 + 1000)) as f32 / u32::MAX as f32 - 0.5;
                UnitPose::REST.offset(h1 * a * size_px * 0.12, h2 * a * size_px * 0.12).rotation(h1 * a * 6.0)
            }
            LoopKind::Sway => UnitPose::REST.offset((phase * 0.5).sin() * a * size_px * 0.2, 0.0).rotation((phase * 0.5).cos() * a * 3.0),
            LoopKind::Shimmer => {
                let n = count.max(1) as f32;
                let pos = (time * self.speed).rem_euclid(1.0) * (n + 4.0) - 2.0;
                let dist = (i - pos).abs();
                let k = (1.0 - dist / 2.5).clamp(0.0, 1.0);
                let col = self.colour.unwrap_or(Rgba::WHITE);
                if k > 0.0 { UnitPose::REST.colour(col.alpha(1.0)).opacity(1.0).scale(1.0 + k * a * 0.1).colour(Rgba::new(col.r, col.g, col.b, k)) } else { UnitPose::REST }
            }
            LoopKind::Wobble => UnitPose::REST.rotation((phase - i * 0.4).sin() * a * 15.0),
        }
    }
}

/// A custom per-unit motion: keys over each unit's own progress (0 at its
/// start, 1 at its end), staggered across the units. Offsets are in
/// fractions of the font size, rotation in degrees, scale and opacity as
/// factors. This is how new text animations are made, not picked.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UnitMotion {
    #[serde(default)]
    pub unit: TextUnit,
    /// Seconds for the whole thing, from `start` seconds into the clip.
    pub duration: f32,
    #[serde(default)]
    pub start: f32,
    #[serde(default = "default_overlap")]
    pub overlap: f32,
    #[serde(default)]
    pub order: Order,
    #[serde(default)]
    pub ease: Ease,
    #[serde(default)]
    pub x: Vec<[f32; 2]>,
    #[serde(default)]
    pub y: Vec<[f32; 2]>,
    #[serde(default)]
    pub scale: Vec<[f32; 2]>,
    #[serde(default)]
    pub scale_x: Vec<[f32; 2]>,
    #[serde(default)]
    pub scale_y: Vec<[f32; 2]>,
    #[serde(default)]
    pub rotation: Vec<[f32; 2]>,
    #[serde(default)]
    pub opacity: Vec<[f32; 2]>,
}

/// The value of piecewise keys `[progress, value]` at `p` (eased).
fn keyed(keys: &[[f32; 2]], p: f32, ease: Ease, rest: f32) -> f32 {
    match keys {
        [] => rest,
        [only] => only[1],
        _ => {
            if p <= keys[0][0] {
                return keys[0][1];
            }
            for w in keys.windows(2) {
                if p <= w[1][0] {
                    let span = (w[1][0] - w[0][0]).max(1e-6);
                    let u = ease.apply((p - w[0][0]) / span);
                    return w[0][1] + (w[1][1] - w[0][1]) * u;
                }
            }
            keys[keys.len() - 1][1]
        }
    }
}

impl UnitMotion {
    pub fn pose(&self, time: f32, index: usize, count: usize, size_px: f32) -> UnitPose {
        let progress = ((time - self.start) / self.duration.max(1e-3)).clamp(0.0, 1.0);
        let p = stagger(progress, index, count, self.overlap, self.order);
        let s = keyed(&self.scale, p, self.ease, 1.0);
        UnitPose {
            offset: [keyed(&self.x, p, self.ease, 0.0) * size_px, keyed(&self.y, p, self.ease, 0.0) * size_px],
            scale: [s * keyed(&self.scale_x, p, self.ease, 1.0), s * keyed(&self.scale_y, p, self.ease, 1.0)],
            rotation: keyed(&self.rotation, p, self.ease, 0.0),
            opacity: keyed(&self.opacity, p, self.ease, 1.0).clamp(0.0, 1.0),
            colour: None,
        }
    }
}

/// Everything a title does over its time on screen: an entrance, an exit,
/// a loop while it is up, and any number of custom unit motions. Each part
/// works on its own unit (characters, words or lines).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TextMotion {
    /// Entrance and how long it takes (seconds).
    #[serde(default)]
    pub reveal: Option<Reveal>,
    #[serde(default = "default_reveal_dur")]
    pub reveal_duration: f32,
    /// Exit (played backwards into the end of the clip) and its length.
    #[serde(default)]
    pub exit: Option<Reveal>,
    #[serde(default = "default_reveal_dur")]
    pub exit_duration: f32,
    #[serde(default, rename = "loop")]
    pub looping: Option<LoopMotion>,
    #[serde(default)]
    pub custom: Vec<UnitMotion>,
    /// Range-style animators: values applied by how much a
    /// selector picks each unit.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub animators: Vec<crate::animator::Animator>,
}

fn default_reveal_dur() -> f32 {
    0.6
}

fn combine(a: UnitPose, b: UnitPose) -> UnitPose {
    UnitPose {
        offset: [a.offset[0] + b.offset[0], a.offset[1] + b.offset[1]],
        scale: [a.scale[0] * b.scale[0], a.scale[1] * b.scale[1]],
        rotation: a.rotation + b.rotation,
        opacity: a.opacity * b.opacity,
        colour: b.colour.or(a.colour),
    }
}

impl TextMotion {
    /// Whether any part works on `unit`.
    pub fn uses(&self, unit: TextUnit) -> bool {
        self.reveal.as_ref().is_some_and(|r| r.unit == unit) || self.exit.as_ref().is_some_and(|r| r.unit == unit) || self.looping.as_ref().is_some_and(|l| l.unit == unit) || self.custom.iter().any(|c| c.unit == unit) || self.animators.iter().any(|a| a.unit == unit)
    }

    /// The unit whose animators blur, if any do (blur works on one kind of
    /// unit: the first animator with a blur decides).
    pub fn blur_unit(&self) -> Option<TextUnit> {
        self.animators.iter().find(|a| a.blur.is_some_and(|b| b != 0.0)).map(|a| a.unit)
    }

    /// Blur of unit `index` of `count` (of the blur unit) in picture
    /// pixels; `px` is picture pixels per canvas pixel.
    pub fn blur(&self, time: f32, index: usize, count: usize, px: f32) -> f32 {
        let Some(unit) = self.blur_unit() else { return 0.0 };
        self.animators.iter().filter(|a| a.unit == unit).map(|a| a.blur(time, index, count, px)).sum()
    }

    /// The pose of unit `index` of `count` (of kind `unit`) at `time`
    /// seconds into a clip lasting `clip_dur`.
    pub fn pose(&self, unit: TextUnit, time: f32, clip_dur: f32, index: usize, count: usize, size_px: f32) -> UnitPose {
        self.pose_px(unit, time, clip_dur, index, count, size_px, 1.0)
    }

    /// [`TextMotion::pose`] where animators' pixel values are scaled by
    /// `px` (picture pixels per canvas pixel).
    #[allow(clippy::too_many_arguments)]
    pub fn pose_px(&self, unit: TextUnit, time: f32, clip_dur: f32, index: usize, count: usize, size_px: f32, px: f32) -> UnitPose {
        let mut pose = UnitPose::REST;
        if let Some(r) = self.reveal.as_ref().filter(|r| r.unit == unit) {
            let p = (time / self.reveal_duration.max(1e-3)).clamp(0.0, 1.0);
            pose = combine(pose, r.pose(p, index, count, size_px));
        }
        if let Some(r) = self.exit.as_ref().filter(|r| r.unit == unit) {
            let left = clip_dur - time;
            if left < self.exit_duration {
                let p = (left / self.exit_duration.max(1e-3)).clamp(0.0, 1.0);
                pose = combine(pose, r.pose(p, index, count, size_px));
            }
        }
        if let Some(l) = self.looping.as_ref().filter(|l| l.unit == unit) {
            pose = combine(pose, l.pose(time, index, count, size_px));
        }
        for c in self.custom.iter().filter(|c| c.unit == unit) {
            pose = combine(pose, c.pose(time, index, count, size_px));
        }
        for a in self.animators.iter().filter(|a| a.unit == unit) {
            pose = combine(pose, a.pose(time, index, count, px));
        }
        pose
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stagger_ends_are_exact() {
        for order in [Order::Forward, Order::Backward, Order::Centre, Order::Random] {
            for i in 0..7 {
                assert_eq!(stagger(0.0, i, 7, 0.5, order), 0.0);
                assert_eq!(stagger(1.0, i, 7, 0.5, order), 1.0);
            }
        }
        // Forward: the first unit leads.
        assert!(stagger(0.3, 0, 5, 0.3, Order::Forward) > stagger(0.3, 4, 5, 0.3, Order::Forward));
        assert!(stagger(0.3, 4, 5, 0.3, Order::Backward) > stagger(0.3, 0, 5, 0.3, Order::Backward));
    }

    #[test]
    fn reveals_rest_at_one_and_hide_at_zero() {
        for kind in [RevealKind::Fade, RevealKind::Rise, RevealKind::Drop, RevealKind::Pop, RevealKind::SlideLeft, RevealKind::SlideRight, RevealKind::Type, RevealKind::Spin, RevealKind::Bounce, RevealKind::Zoom, RevealKind::Flip, RevealKind::Spring] {
            let r = Reveal::new(kind, TextUnit::Character);
            let end = r.pose(1.0, 2, 5, 40.0);
            assert!((end.opacity - 1.0).abs() < 1e-4, "{kind:?} end opacity {}", end.opacity);
            assert!((end.offset[0]).abs() < 1e-2 && end.offset[1].abs() < 1e-2, "{kind:?} end offset");
            assert!((end.scale[0] - 1.0).abs() < 1e-3 && (end.scale[1] - 1.0).abs() < 1e-3, "{kind:?} end scale {:?}", end.scale);
            let start = r.pose(0.0, 2, 5, 40.0);
            assert!(start.opacity < 1e-4, "{kind:?} start opacity");
        }
    }

    #[test]
    fn loops_are_bounded() {
        let m = LoopMotion::new(LoopKind::Wave, TextUnit::Character);
        for t in 0..50 {
            let p = m.pose(t as f32 * 0.1, 3, 10, 40.0);
            assert!(p.offset[1].abs() <= 40.0 * 0.15 + 1e-4);
        }
    }
}
