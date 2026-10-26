// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Planted feet that do not slide: contacts found in a motion (a foot low
//! and nearly still for a few frames) and, per frame, where an IK target
//! holds the foot during each contact, eased in and out so the leg never
//! pops. The motion is sampled in world space (the canvas, the scene):
//! a character that travels moves its feet in the world while they are
//! still on the ground, and that is what pinning removes.

use crate::v::{self, P};

/// Frames `start..=end` during which something is planted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Contact {
    pub start: usize,
    pub end: usize,
}

/// What counts as planted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rules {
    /// How far above the ground (or the lowest the foot gets) still
    /// touches it.
    pub height: f64,
    /// The most it may move in a frame (world units).
    pub speed: f64,
    /// The fewest frames a contact lasts.
    pub min_frames: usize,
}

/// Contacts of a foot whose tip is at `tips[i]` on frame `i`: frames
/// where it is within `rules.height` of `ground` (or of the lowest it
/// gets) along `up`, moving less than `rules.speed`.
pub fn contacts<const N: usize>(tips: &[P<N>], up: P<N>, ground: Option<f64>, rules: Rules) -> Vec<Contact> {
    if tips.is_empty() {
        return Vec::new();
    }
    let up = v::norm(up);
    let h: Vec<f64> = tips.iter().map(|t| v::dot(*t, up)).collect();
    let floor = ground.unwrap_or_else(|| h.iter().copied().fold(f64::INFINITY, f64::min));
    let speed = |i: usize| {
        let back = if i > 0 { v::dist(tips[i], tips[i - 1]) } else { f64::INFINITY };
        let fwd = if i + 1 < tips.len() { v::dist(tips[i], tips[i + 1]) } else { f64::INFINITY };
        let s = back.min(fwd);
        if s.is_finite() { s } else { 0.0 }
    };
    let planted: Vec<bool> = (0..tips.len()).map(|i| h[i] - floor <= rules.height && speed(i) <= rules.speed).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < planted.len() {
        if planted[i] {
            let s = i;
            while i + 1 < planted.len() && planted[i + 1] {
                i += 1;
            }
            if i + 1 - s >= rules.min_frames.max(1) {
                out.push(Contact { start: s, end: i });
            }
        }
        i += 1;
    }
    out
}

/// Per frame, where the foot's IK target goes and how much the IK holds
/// it (0..1): during a contact, the place it landed (on the ground when
/// `ground` is given), fully; `ease` frames either side, blending in and
/// out; elsewhere, the foot as animated, not held.
pub fn pinned<const N: usize>(tips: &[P<N>], contacts: &[Contact], ease: usize, up: P<N>, ground: Option<f64>) -> Vec<(P<N>, f64)> {
    let up = v::norm(up);
    let mut out: Vec<(P<N>, f64)> = tips.iter().map(|t| (*t, 0.0)).collect();
    for c in contacts {
        let Some(&first) = tips.get(c.start) else { continue };
        let anchor = match ground {
            Some(g) => v::add(first, v::scale(up, g - v::dot(first, up))),
            None => first,
        };
        let lo = c.start.saturating_sub(ease);
        let hi = (c.end + ease).min(tips.len().saturating_sub(1));
        for (i, slot) in out.iter_mut().enumerate().take(hi + 1).skip(lo) {
            let w = if i < c.start {
                v::smooth(0.0, 1.0, 1.0 - (c.start - i) as f64 / (ease + 1) as f64)
            } else if i > c.end {
                v::smooth(0.0, 1.0, 1.0 - (i - c.end) as f64 / (ease + 1) as f64)
            } else {
                1.0
            };
            if w > slot.1 {
                *slot = (anchor, w);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A foot that lands at frame 5, slides along with the body (the
    /// mistake pinning fixes) until frame 14, lifts and lands again.
    fn steps() -> Vec<P<2>> {
        (0..30)
            .map(|i| {
                let x = i as f64 * 0.01;
                let lift = if (5..=14).contains(&i) || (20..=27).contains(&i) { 0.0 } else { 0.2 };
                [x, -lift]
            })
            .collect()
    }

    #[test]
    fn contacts_are_found_and_pinned_feet_stay_put() {
        // y down: up is -y.
        let tips = steps();
        let rules = Rules { height: 0.01, speed: 0.02, min_frames: 3 };
        let c = contacts(&tips, [0.0, -1.0], Some(0.0), rules);
        assert_eq!(c, vec![Contact { start: 5, end: 14 }, Contact { start: 20, end: 27 }]);
        let p = pinned(&tips, &c, 2, [0.0, -1.0], Some(0.0));
        // Held where it landed for the whole contact.
        for (i, (t, w)) in p.iter().enumerate().take(15).skip(5) {
            assert_eq!(*w, 1.0, "frame {i}");
            assert!(v::dist(*t, [0.05, 0.0]) < 1e-12);
        }
        // Eased either side; free well away.
        assert!(p[4].1 > 0.0 && p[4].1 < 1.0 && p[3].1 < p[4].1);
        assert_eq!(p[0].1, 0.0);
        assert!(p[16].1 > 0.0 && p[16].1 < 1.0);
        // Too short to count.
        assert!(contacts(&tips, [0.0, -1.0], Some(0.0), Rules { min_frames: 11, ..rules }).is_empty());
    }
}
