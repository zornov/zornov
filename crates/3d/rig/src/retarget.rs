// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Moving a motion from one rig onto another of other proportions (a
//! capture of a tall actor onto a short cartoon, one character's walk onto
//! another, a 3D capture onto a 2D cut-out): per frame, each target bone
//! points the way its source bone points (corrected for rest poses that
//! differ), and the body's travel is scaled by leg length so the feet
//! keep up.
//!
//! Both sides are given as bone segments (`(head, tail)`) in their own
//! space; each rig turns the directions back into its own rotations.

use crate::v::{self, P};

/// How directions carry over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Each target bone turns from its own rest as the source turns from
    /// its rest (rests that differ, a T-pose capture onto an A-pose
    /// character, come out right).
    Relative,
    /// Each target bone points exactly where the source bone points
    /// (rests that already match; the pose looks the same).
    Absolute,
}

/// For each target bone, where it should point (world direction, one
/// long), from the source bone paired with it (`pairs`: `(target,
/// source)`); `None` for target bones with no partner.
pub fn directions<const N: usize>(src_rest: &[(P<N>, P<N>)], src_now: &[(P<N>, P<N>)], dst_rest: &[(P<N>, P<N>)], pairs: &[(usize, usize)], mode: Mode) -> Vec<Option<P<N>>> {
    let mut out = vec![None; dst_rest.len()];
    for &(d, s) in pairs {
        let (Some(sr), Some(sn), Some(dr)) = (src_rest.get(s), src_now.get(s), dst_rest.get(d)) else { continue };
        let now = v::norm(v::sub(sn.1, sn.0));
        if v::len(now) < 0.5 {
            continue;
        }
        out[d] = Some(match mode {
            Mode::Absolute => now,
            Mode::Relative => v::norm(v::turn(v::sub(dr.1, dr.0), v::sub(sr.1, sr.0), now)),
        });
    }
    out
}

/// How much to scale the source's travel for the target: the ratio of
/// their leg lengths (hip to ground), or 1 when either has none.
pub fn travel_scale(src_leg: f64, dst_leg: f64) -> f64 {
    if src_leg > 1e-9 && dst_leg > 1e-9 { dst_leg / src_leg } else { 1.0 }
}

/// The target's root per frame: where its rest root is, plus the source
/// root's travel from its rest, scaled by `k`.
pub fn root_motion<const N: usize>(src_root: &[P<N>], src_rest_root: P<N>, dst_rest_root: P<N>, k: f64) -> Vec<P<N>> {
    src_root.iter().map(|r| v::add(dst_rest_root, v::scale(v::sub(*r, src_rest_root), k))).collect()
}

/// A 3D point seen in a 2D picture: the side view keeps (z or x, y), the
/// front view (x, y). `side_axis_z` picks the depth axis a side view
/// shows across the picture; `flip` mirrors it (a character facing the
/// other way).
pub fn project(p: P<3>, side: bool, flip: bool) -> P<2> {
    let across = if side { -p[2] } else { p[0] };
    [if flip { -across } else { across }, p[1]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_directions_correct_for_different_rests() {
        // Source arm rests straight out (T-pose) and swings down 90°.
        let src_rest = [([0.0, 0.0], [1.0, 0.0])];
        let src_now = [([0.0, 0.0], [0.0, 1.0])];
        // Target arm rests 45° down (A-pose).
        let s2 = std::f64::consts::FRAC_1_SQRT_2;
        let dst_rest = [([0.0, 0.0], [s2, s2])];
        let rel = directions(&src_rest, &src_now, &dst_rest, &[(0, 0)], Mode::Relative);
        let d = rel[0].unwrap();
        // Turned 90° from its own rest: 135° down.
        assert!(v::dist(d, [-s2, s2]) < 1e-9, "{d:?}");
        let abs = directions(&src_rest, &src_now, &dst_rest, &[(0, 0)], Mode::Absolute);
        assert!(v::dist(abs[0].unwrap(), [0.0, 1.0]) < 1e-9);
        assert_eq!(directions(&src_rest, &src_now, &[([0.0; 2], [1.0, 0.0]); 2], &[(0, 0)], Mode::Absolute)[1], None);
    }

    #[test]
    fn travel_scales_with_leg_length() {
        let k = travel_scale(1.0, 0.5);
        let r = root_motion(&[[0.0, 1.0, 0.0], [2.0, 1.0, 0.0]], [0.0, 1.0, 0.0], [5.0, 0.5, 0.0], k);
        assert_eq!(r, vec![[5.0, 0.5, 0.0], [6.0, 0.5, 0.0]]);
        assert_eq!(project([1.0, 2.0, 3.0], true, false), [-3.0, 2.0]);
        assert_eq!(project([1.0, 2.0, 3.0], false, true), [-1.0, 2.0]);
    }
}
