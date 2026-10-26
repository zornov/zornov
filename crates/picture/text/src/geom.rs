// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Small geometry helpers shared by layout, painting and scenes.

use serde::{Deserialize, Serialize};

/// An axis-aligned rectangle in pixels (or units; the caller knows).
#[derive(Clone, Copy, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    /// The empty rectangle that [`Rect::union`] treats as "nothing yet".
    pub const EMPTY: Rect = Rect { x: f32::INFINITY, y: f32::INFINITY, w: f32::NEG_INFINITY, h: f32::NEG_INFINITY };

    pub fn is_empty(&self) -> bool {
        !(self.w > 0.0 && self.h > 0.0)
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn centre(&self) -> [f32; 2] {
        [self.x + self.w * 0.5, self.y + self.h * 0.5]
    }

    pub fn from_min_max(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
        Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
    }

    pub fn union(&self, o: &Rect) -> Rect {
        if self.is_empty() && !self.x.is_finite() {
            return *o;
        }
        if o.is_empty() && !o.x.is_finite() {
            return *self;
        }
        Rect::from_min_max(self.x.min(o.x), self.y.min(o.y), self.right().max(o.right()), self.bottom().max(o.bottom()))
    }

    pub fn include_point(&self, x: f32, y: f32) -> Rect {
        self.union(&Rect::new(x, y, 0.0, 0.0))
    }

    /// Grown by `d` on every side.
    pub fn inflate(&self, d: f32) -> Rect {
        Rect { x: self.x - d, y: self.y - d, w: self.w + 2.0 * d, h: self.h + 2.0 * d }
    }

    pub fn translate(&self, dx: f32, dy: f32) -> Rect {
        Rect { x: self.x + dx, y: self.y + dy, ..*self }
    }

    /// The smallest integer-aligned rectangle containing this one.
    pub fn round_out(&self) -> Rect {
        let x0 = self.x.floor();
        let y0 = self.y.floor();
        Rect::from_min_max(x0, y0, self.right().ceil(), self.bottom().ceil())
    }
}

/// A 2-D affine transform `[a c e; b d f]` (column-major like zeno/SVG).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub e: f32,
    pub f: f32,
}

impl Affine {
    pub const IDENTITY: Affine = Affine { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 };

    pub fn translate(x: f32, y: f32) -> Affine {
        Affine { e: x, f: y, ..Affine::IDENTITY }
    }

    pub fn scale(sx: f32, sy: f32) -> Affine {
        Affine { a: sx, d: sy, ..Affine::IDENTITY }
    }

    /// Rotation in degrees, clockwise on a y-down canvas.
    pub fn rotate(degrees: f32) -> Affine {
        let (s, c) = degrees.to_radians().sin_cos();
        Affine { a: c, b: s, c: -s, d: c, e: 0.0, f: 0.0 }
    }

    pub fn skew_x(degrees: f32) -> Affine {
        Affine { c: degrees.to_radians().tan(), ..Affine::IDENTITY }
    }

    /// `self` applied after `o` (so `a.then(b)` maps a point through `a`, then `b`).
    pub fn then(&self, o: &Affine) -> Affine {
        o.pre(self)
    }

    /// `self` applied after `o`: result(p) = self(o(p)).
    pub fn pre(&self, o: &Affine) -> Affine {
        Affine {
            a: self.a * o.a + self.c * o.b,
            b: self.b * o.a + self.d * o.b,
            c: self.a * o.c + self.c * o.d,
            d: self.b * o.c + self.d * o.d,
            e: self.a * o.e + self.c * o.f + self.e,
            f: self.b * o.e + self.d * o.f + self.f,
        }
    }

    /// Scale, rotate and translate about a pivot: the usual "unit pose".
    pub fn about(pivot: [f32; 2], scale: [f32; 2], rotation_deg: f32, offset: [f32; 2]) -> Affine {
        Affine::translate(-pivot[0], -pivot[1])
            .then(&Affine::scale(scale[0], scale[1]))
            .then(&Affine::rotate(rotation_deg))
            .then(&Affine::translate(pivot[0] + offset[0], pivot[1] + offset[1]))
    }

    pub fn apply(&self, p: [f32; 2]) -> [f32; 2] {
        [self.a * p[0] + self.c * p[1] + self.e, self.b * p[0] + self.d * p[1] + self.f]
    }

    pub fn is_identity(&self) -> bool {
        *self == Affine::IDENTITY
    }

    /// True when the transform is a pure translation.
    pub fn is_translation(&self) -> bool {
        self.a == 1.0 && self.b == 0.0 && self.c == 0.0 && self.d == 1.0
    }

    pub fn to_zeno(&self) -> zeno::Transform {
        zeno::Transform::new(self.a, self.b, self.c, self.d, self.e, self.f)
    }

    /// Bounds of a rectangle after this transform.
    pub fn bounds(&self, r: &Rect) -> Rect {
        let mut out = Rect::EMPTY;
        for p in [[r.x, r.y], [r.right(), r.y], [r.x, r.bottom()], [r.right(), r.bottom()]] {
            let q = self.apply(p);
            out = out.include_point(q[0], q[1]);
        }
        out
    }
}

/// Easing curves for animation (our own naming; shapes are the standard
/// cubic/back/elastic/bounce families).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ease {
    Linear,
    #[default]
    Smooth,
    In,
    Out,
    InOut,
    /// Overshoots a little past the end, like a settling spring.
    Overshoot,
    /// Pulls back slightly before leaving.
    Anticipate,
    Elastic,
    Bounce,
    /// Steps straight to the end value half way through.
    Hold,
}

impl Ease {
    /// Maps linear progress 0..1 to eased progress (may exceed 0..1 for
    /// overshoots).
    pub fn apply(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Ease::Linear => t,
            Ease::Smooth => t * t * (3.0 - 2.0 * t),
            Ease::In => t * t * t,
            Ease::Out => 1.0 - (1.0 - t).powi(3),
            Ease::InOut => {
                if t < 0.5 { 4.0 * t * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(3) / 2.0 }
            }
            Ease::Overshoot => {
                let c1 = 1.70158;
                let c3 = c1 + 1.0;
                1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
            }
            Ease::Anticipate => {
                let c1 = 1.70158;
                let c3 = c1 + 1.0;
                c3 * t * t * t - c1 * t * t
            }
            Ease::Elastic => {
                if t <= 0.0 {
                    0.0
                } else if t >= 1.0 {
                    1.0
                } else {
                    let c4 = (2.0 * std::f32::consts::PI) / 3.0;
                    2f32.powf(-10.0 * t) * ((t * 10.0 - 0.75) * c4).sin() + 1.0
                }
            }
            Ease::Bounce => {
                let (n1, d1) = (7.5625, 2.75);
                let mut t = t;
                if t < 1.0 / d1 {
                    n1 * t * t
                } else if t < 2.0 / d1 {
                    t -= 1.5 / d1;
                    n1 * t * t + 0.75
                } else if t < 2.5 / d1 {
                    t -= 2.25 / d1;
                    n1 * t * t + 0.9375
                } else {
                    t -= 2.625 / d1;
                    n1 * t * t + 0.984375
                }
            }
            Ease::Hold => {
                if t < 0.5 { 0.0 } else { 1.0 }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eases_hit_both_ends() {
        for e in [Ease::Linear, Ease::Smooth, Ease::In, Ease::Out, Ease::InOut, Ease::Overshoot, Ease::Anticipate, Ease::Elastic, Ease::Bounce, Ease::Hold] {
            assert!((e.apply(0.0)).abs() < 1e-5, "{e:?} start");
            assert!((e.apply(1.0) - 1.0).abs() < 1e-4, "{e:?} end");
        }
    }

    #[test]
    fn affine_about_pivot_keeps_pivot() {
        let t = Affine::about([10.0, 20.0], [2.0, 3.0], 45.0, [0.0, 0.0]);
        let p = t.apply([10.0, 20.0]);
        assert!((p[0] - 10.0).abs() < 1e-4 && (p[1] - 20.0).abs() < 1e-4);
    }

    #[test]
    fn affine_composition_order() {
        let t = Affine::scale(2.0, 2.0).then(&Affine::translate(1.0, 0.0));
        assert_eq!(t.apply([1.0, 1.0]), [3.0, 2.0]);
    }

    #[test]
    fn rect_union_and_empty() {
        let r = Rect::EMPTY.union(&Rect::new(1.0, 1.0, 2.0, 2.0)).union(&Rect::new(0.0, 5.0, 1.0, 1.0));
        assert_eq!(r, Rect::new(0.0, 1.0, 3.0, 5.0));
    }
}
