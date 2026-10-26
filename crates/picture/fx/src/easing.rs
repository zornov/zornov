// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Easing curves mapping progress 0..1 to eased progress. All curves pass
//! through (0, 0) and (1, 1); "back" and "elastic" overshoot in between.

use std::f32::consts::PI;

/// A named easing curve, or a custom cubic Bézier like CSS's
/// `cubic-bezier(x1, y1, x2, y2)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Easing {
    Linear,
    SineIn,
    SineOut,
    SineInOut,
    QuadIn,
    QuadOut,
    QuadInOut,
    CubicIn,
    CubicOut,
    CubicInOut,
    QuintIn,
    QuintOut,
    QuintInOut,
    ExpoIn,
    ExpoOut,
    ExpoInOut,
    BackIn,
    BackOut,
    BackInOut,
    ElasticOut,
    BounceOut,
    /// Control points (x1, y1, x2, y2); x values are clamped to 0..1.
    Bezier([f32; 4]),
}

/// Labels for the named curves, in [`Easing::NAMED`] order, for choice
/// parameters.
pub const EASING_NAMES: &[&str] = &[
    "Linear",
    "Sine in",
    "Sine out",
    "Sine in-out",
    "Quad in",
    "Quad out",
    "Quad in-out",
    "Cubic in",
    "Cubic out",
    "Cubic in-out",
    "Quint in",
    "Quint out",
    "Quint in-out",
    "Expo in",
    "Expo out",
    "Expo in-out",
    "Back in",
    "Back out",
    "Back in-out",
    "Elastic out",
    "Bounce out",
];

impl Easing {
    pub const NAMED: [Easing; 21] = [
        Easing::Linear,
        Easing::SineIn,
        Easing::SineOut,
        Easing::SineInOut,
        Easing::QuadIn,
        Easing::QuadOut,
        Easing::QuadInOut,
        Easing::CubicIn,
        Easing::CubicOut,
        Easing::CubicInOut,
        Easing::QuintIn,
        Easing::QuintOut,
        Easing::QuintInOut,
        Easing::ExpoIn,
        Easing::ExpoOut,
        Easing::ExpoInOut,
        Easing::BackIn,
        Easing::BackOut,
        Easing::BackInOut,
        Easing::ElasticOut,
        Easing::BounceOut,
    ];

    /// Named curve by its index in [`EASING_NAMES`] (out-of-range gives linear).
    pub fn from_index(i: usize) -> Easing {
        Self::NAMED.get(i).copied().unwrap_or(Easing::Linear)
    }

    /// Index in [`EASING_NAMES`] (custom Béziers have none).
    pub fn index(self) -> Option<usize> {
        Self::NAMED.iter().position(|&e| e == self)
    }

    /// The curve mirrored in time (`1 - f(1 - t)`). Elastic and bounce have
    /// no mirrored counterpart in the list and return themselves.
    pub fn reversed(self) -> Easing {
        use Easing::*;
        match self {
            SineIn => SineOut,
            SineOut => SineIn,
            QuadIn => QuadOut,
            QuadOut => QuadIn,
            CubicIn => CubicOut,
            CubicOut => CubicIn,
            QuintIn => QuintOut,
            QuintOut => QuintIn,
            ExpoIn => ExpoOut,
            ExpoOut => ExpoIn,
            BackIn => BackOut,
            BackOut => BackIn,
            Bezier([x1, y1, x2, y2]) => Bezier([1.0 - x2, 1.0 - y2, 1.0 - x1, 1.0 - y1]),
            other => other,
        }
    }

    /// Eased value for progress `t` (clamped to 0..1).
    pub fn apply(self, t: f32) -> f32 {
        let t = if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) };
        let inout = |f: fn(f32) -> f32, t: f32| if t < 0.5 { f(2.0 * t) * 0.5 } else { 1.0 - f(2.0 - 2.0 * t) * 0.5 };
        let out = |f: fn(f32) -> f32, t: f32| 1.0 - f(1.0 - t);
        match self {
            Easing::Linear => t,
            Easing::SineIn => sine_in(t),
            Easing::SineOut => out(sine_in, t),
            Easing::SineInOut => inout(sine_in, t),
            Easing::QuadIn => t * t,
            Easing::QuadOut => out(|t| t * t, t),
            Easing::QuadInOut => inout(|t| t * t, t),
            Easing::CubicIn => t * t * t,
            Easing::CubicOut => out(|t| t * t * t, t),
            Easing::CubicInOut => inout(|t| t * t * t, t),
            Easing::QuintIn => t.powi(5),
            Easing::QuintOut => out(|t| t.powi(5), t),
            Easing::QuintInOut => inout(|t| t.powi(5), t),
            Easing::ExpoIn => expo_in(t),
            Easing::ExpoOut => out(expo_in, t),
            Easing::ExpoInOut => inout(expo_in, t),
            Easing::BackIn => back_in(t),
            Easing::BackOut => out(back_in, t),
            Easing::BackInOut => inout(back_in, t),
            Easing::ElasticOut => {
                if t <= 0.0 || t >= 1.0 {
                    t
                } else {
                    // Decaying oscillation settling on 1.
                    1.0 + 2f32.powf(-10.0 * t) * ((t * 10.0 - 0.75) * (2.0 * PI / 3.0)).sin()
                }
            }
            Easing::BounceOut => bounce_out(t),
            Easing::Bezier(c) => cubic_bezier(c, t),
        }
    }
}

fn sine_in(t: f32) -> f32 {
    1.0 - (t * PI * 0.5).cos()
}

fn expo_in(t: f32) -> f32 {
    // Shifted and rescaled so it starts exactly at zero instead of 2^-10.
    let floor = 2f32.powi(-10);
    (2f32.powf(10.0 * t - 10.0) - floor) / (1.0 - floor)
}

fn back_in(t: f32) -> f32 {
    // Overshoot constant chosen for about 10 % undershoot.
    let s = 1.70158;
    t * t * ((s + 1.0) * t - s)
}

fn bounce_out(t: f32) -> f32 {
    let n = 7.5625;
    let d = 2.75;
    if t < 1.0 / d {
        n * t * t
    } else if t < 2.0 / d {
        let t = t - 1.5 / d;
        n * t * t + 0.75
    } else if t < 2.5 / d {
        let t = t - 2.25 / d;
        n * t * t + 0.9375
    } else {
        let t = t - 2.625 / d;
        n * t * t + 0.984375
    }
}

/// Evaluates a CSS-style cubic Bézier timing curve at `x`: finds the curve
/// parameter whose x matches (Newton steps, falling back to bisection) and
/// returns its y.
pub fn cubic_bezier(c: [f32; 4], x: f32) -> f32 {
    let (x1, y1, x2, y2) = (c[0].clamp(0.0, 1.0), c[1], c[2].clamp(0.0, 1.0), c[3]);
    let bez = |a: f32, b: f32, s: f32| {
        let m = 1.0 - s;
        3.0 * m * m * s * a + 3.0 * m * s * s * b + s * s * s
    };
    let dbez = |a: f32, b: f32, s: f32| {
        let m = 1.0 - s;
        3.0 * m * m * a + 6.0 * m * s * (b - a) + 3.0 * s * s * (1.0 - b)
    };
    let mut s = x;
    for _ in 0..8 {
        let err = bez(x1, x2, s) - x;
        if err.abs() < 1e-6 {
            return bez(y1, y2, s);
        }
        let d = dbez(x1, x2, s);
        if d.abs() < 1e-6 {
            break;
        }
        s = (s - err / d).clamp(0.0, 1.0);
    }
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    s = x;
    for _ in 0..40 {
        if bez(x1, x2, s) < x {
            lo = s;
        } else {
            hi = s;
        }
        s = 0.5 * (lo + hi);
    }
    bez(y1, y2, s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints() {
        for &e in Easing::NAMED.iter().chain([Easing::Bezier([0.25, 0.1, 0.25, 1.0])].iter()) {
            assert!(e.apply(0.0).abs() < 1e-4, "{e:?}");
            assert!((e.apply(1.0) - 1.0).abs() < 1e-4, "{e:?}");
            assert!(e.apply(0.37).is_finite());
        }
        assert_eq!(EASING_NAMES.len(), Easing::NAMED.len());
    }

    #[test]
    fn reversed_mirrors() {
        for &e in Easing::NAMED.iter().filter(|e| !matches!(e, Easing::ElasticOut | Easing::BounceOut)) {
            for i in 0..=10 {
                let t = i as f32 / 10.0;
                let r = 1.0 - e.reversed().apply(1.0 - t);
                assert!((r - e.apply(t)).abs() < 1e-3, "{e:?} at {t}");
            }
        }
    }

    #[test]
    fn bezier_matches_linear() {
        for i in 0..=10 {
            let t = i as f32 / 10.0;
            assert!((cubic_bezier([1.0 / 3.0, 1.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0], t) - t).abs() < 1e-4);
        }
    }
}
