// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The proportions of a standing figure: how tall the head is, how long the
//! legs and arms are, how big the hands and feet, for a baby, a child, an
//! adult, an elder, and anything from true to life to cartoon. The figure
//! builder ([`crate::character`]) shapes the body with them and the
//! armature fitter ([`crate::autorig`]) places its joints with the same
//! numbers, so bones always land where the body bends.
//!
//! Heights are fractions of the figure's height, measured down from the top
//! of the head (crown 0, soles 1). An adult in the standard fractions has
//! the chin at 0.13, the shoulders at 0.205, the hip joints at 0.53, the
//! knees at 0.74 and the ankles at 0.945.

/// Where the landmarks of a figure are.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Proportions {
    /// Crown to chin, as a fraction of the height (0.13 for an adult).
    pub head: f32,
    /// Hip joints (0.53 for an adult): the legs are everything below.
    pub hip: f32,
    /// Arm length against an adult's (1).
    pub arm: f32,
    /// Hand length against the standard (1).
    pub hand: f32,
    /// Foot length and size against the standard (1).
    pub foot: f32,
    /// Neck length and thickness against the standard (1).
    pub neck: f32,
    /// Limb thickness against the standard (1).
    pub limb: f32,
    /// 0 true to life .. 1 cartoon: rounder, softer, simpler.
    pub style: f32,
}

impl Default for Proportions {
    fn default() -> Proportions {
        Proportions { head: 0.13, hip: 0.53, arm: 1.0, hand: 1.0, foot: 1.0, neck: 1.0, limb: 1.0, style: 0.0 }
    }
}

impl Proportions {
    /// The proportions of a person of `age` years (0 means an adult),
    /// stylised by `style` (0 true to life .. 1 cartoon), with leg, arm,
    /// hand and neck lengths scaled by the multipliers (1 = as is).
    pub fn of(age: f32, style: f32, legs: f32, arms: f32, hands: f32, neck: f32) -> Proportions {
        let years = if age <= 0.0 { 30.0 } else { age.clamp(0.1, 110.0) };
        let style = style.clamp(0.0, 1.0);
        let young = (-years / 6.0).exp();
        let head = 0.13 + 0.12 * young + 0.12 * style;
        let leg = (0.47 - 0.12 * (-years / 7.0).exp() - 0.08 * style) * legs.clamp(0.7, 1.4);
        // The old shorten a little in the spine; nothing else changes here.
        let elder = ((years - 60.0) / 40.0).clamp(0.0, 1.0);
        Proportions {
            head: head.clamp(0.1, 0.4),
            hip: (1.0 - leg).clamp(0.4, 0.7),
            arm: (1.0 - 0.1 * (-years / 8.0).exp() - 0.12 * style - 0.02 * elder) * arms.clamp(0.7, 1.4),
            hand: (1.0 + 0.55 * style + 0.12 * young) * hands.clamp(0.6, 1.8),
            foot: 1.0 + 0.45 * style + 0.15 * young,
            neck: neck.clamp(0.5, 1.6) * (1.0 - 0.3 * style),
            limb: 1.0 + 0.1 * young + 0.28 * style,
            style,
        }
    }

    /// The height fraction of a landmark given as an adult's standard
    /// fraction: the head, torso and legs each stretched to fit.
    pub fn y(&self, f: f32) -> f32 {
        if f <= 0.13 {
            f * self.head / 0.13
        } else if f <= 0.53 {
            self.head + (f - 0.13) * (self.hip - self.head) / 0.40
        } else {
            self.hip + (f - 0.53) * (1.0 - self.hip) / 0.47
        }
    }

    /// How much taller the torso is than the standard (1).
    pub fn torso_k(&self) -> f32 {
        (self.hip - self.head) / 0.40
    }

    /// How much longer the legs are than the standard (1).
    pub fn leg_k(&self) -> f32 {
        (1.0 - self.hip) / 0.47
    }

    /// The head's size against the standard 0.13 of the height.
    pub fn head_k(&self) -> f32 {
        self.head / 0.13
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_adult_is_the_standard_and_a_child_has_a_bigger_head_and_shorter_legs() {
        let a = Proportions::of(0.0, 0.0, 1.0, 1.0, 1.0, 1.0);
        assert!((a.y(0.205) - 0.205).abs() < 0.01 && (a.y(0.74) - 0.74).abs() < 0.01, "{a:?}");
        let k = Proportions::of(5.0, 0.0, 1.0, 1.0, 1.0, 1.0);
        assert!(k.head > 0.17 && k.hip > 0.56 && k.arm < 0.97, "{k:?}");
        let toon = Proportions::of(0.0, 1.0, 1.0, 1.0, 1.0, 1.0);
        assert!(toon.head > 0.22 && toon.hand > 1.4, "{toon:?}");
        // Landmarks stay in order and inside the figure.
        for p in [a, k, toon] {
            let ys: Vec<f32> = [0.0, 0.13, 0.205, 0.4, 0.53, 0.74, 0.945, 1.0].iter().map(|&f| p.y(f)).collect();
            assert!(ys.windows(2).all(|w| w[0] < w[1]), "{ys:?}");
        }
    }
}
