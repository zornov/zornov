// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Inverse kinematics on joint positions: where each joint of a chain
//! goes so its end reaches a goal. Callers turn the solved positions back
//! into bone rotations (and keep their joint limits there).
//!
//! - [`two_bone`]: an arm or a leg, exact (law of cosines), the middle
//!   joint bending toward a pole.
//! - [`fabrik`]: chains of any length (tails, spines, tentacles, fingers),
//!   with an optional pole the middle joints lean toward.
//! - [`ccd`]: cyclic coordinate descent, for chains whose callers clamp
//!   each joint as it goes.
//! - [`Reach`]: soft IK (the limb eases into straight instead of snapping
//!   at full reach) and stretch (the chain grows, up to a limit, to cover
//!   what is left).
//! - [`pole_for`]: the pole that keeps an FK pose's bend when switching
//!   to IK (snap IK to FK).

use crate::v::{self, P};

/// How a chain behaves near and past full reach.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reach {
    /// The longest the chain may grow, as a factor of its length (1 =
    /// rigid bones; 1.5 = half again as long).
    pub stretch: f64,
    /// Soft IK: the last fraction of the chain's reach (0..0.5) over which
    /// the end slows into the straight limb, so a knee or elbow never
    /// pops straight.
    pub soft: f64,
}

impl Default for Reach {
    fn default() -> Reach {
        Reach { stretch: 1.0, soft: 0.0 }
    }
}

/// Where a chain `len` long actually reaches toward a goal `d` away:
/// `(distance reached, stretch factor)`. Rigid and hard, it is `min(d,
/// len)`; soft, it slows over the last stretch of the reach; stretchy, it
/// grows (up to `stretch`) to cover the rest.
pub fn reach_distance(d: f64, len: f64, r: Reach) -> (f64, f64) {
    if len <= 1e-12 {
        return (0.0, 1.0);
    }
    let soft = r.soft.clamp(0.0, 0.5) * len;
    let reach = if soft > 1e-12 && d > len - soft { len - soft + soft * (1.0 - (-(d - (len - soft)) / soft).exp()) } else { d.min(len) };
    let k = if r.stretch > 1.0 && d > reach + 1e-12 { (d / reach.max(1e-12)).clamp(1.0, r.stretch) } else { 1.0 };
    (reach * k, k)
}

/// A solved two-bone chain.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TwoBone<const N: usize> {
    /// The middle joint (elbow, knee).
    pub mid: P<N>,
    /// The end (wrist, ankle).
    pub end: P<N>,
    /// How much both bones grew (1 = not at all).
    pub stretch: f64,
    /// Whether the end is on the goal.
    pub reached: bool,
}

/// Two bones from `root`, `l1` and `l2` long, reaching for `goal`, the
/// middle joint bending toward `bend` (a direction; only its part across
/// the root-to-goal line counts: pass `pole - root` for a pole target).
pub fn two_bone<const N: usize>(root: P<N>, l1: f64, l2: f64, goal: P<N>, bend: P<N>, r: Reach) -> TwoBone<N> {
    let to = v::sub(goal, root);
    let d = v::len(to);
    let dir = if d > 1e-12 {
        v::scale(to, 1.0 / d)
    } else {
        // The goal on the root: fold along whatever is square to the bend.
        let p = v::perpendicular(bend);
        if v::len(p) > 0.5 { p } else { v::perpendicular([1.0; N]) }
    };
    let (reach, k) = reach_distance(d, l1 + l2, r);
    let (a, b) = (l1 * k, l2 * k);
    let dd = reach.clamp((a - b).abs() + 1e-9, (a + b).max(1e-9));
    let mut side = v::norm(v::across(bend, dir));
    if v::len(side) < 0.5 {
        side = v::perpendicular(dir);
    }
    let x = (a * a + dd * dd - b * b) / (2.0 * dd);
    let h = (a * a - x * x).max(0.0).sqrt();
    let mid = v::add(root, v::add(v::scale(dir, x), v::scale(side, h)));
    let end = v::add(root, v::scale(dir, dd));
    let reached = v::dist(end, goal) <= 1e-6 * (l1 + l2).max(1.0);
    TwoBone { mid, end, stretch: k, reached }
}

/// What a longer chain's solve came to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Solved {
    pub stretch: f64,
    pub reached: bool,
}

/// FABRIK: moves `joints` (root first; the root stays) so the last one
/// reaches `goal`, each bone keeping its length (times the stretch
/// [`Reach`] allows). With `pole`, the middle joints lean toward it.
pub fn fabrik<const N: usize>(joints: &mut [P<N>], goal: P<N>, pole: Option<P<N>>, r: Reach, tolerance: f64, iterations: usize) -> Solved {
    let n = joints.len();
    if n < 2 {
        return Solved { stretch: 1.0, reached: n == 1 && v::dist(joints[0], goal) <= tolerance };
    }
    let mut lens: Vec<f64> = joints.windows(2).map(|w| v::dist(w[0], w[1])).collect();
    let total: f64 = lens.iter().sum();
    let root = joints[0];
    let d = v::dist(root, goal);
    let (reach, k) = reach_distance(d, total, r);
    for l in &mut lens {
        *l *= k;
    }
    let dir = v::norm(v::sub(goal, root));
    let aim = if d > 1e-12 { v::add(root, v::scale(dir, reach)) } else { goal };
    if reach >= total * k - 1e-9 && d > 1e-12 {
        // Out of reach: a straight line at the goal.
        let mut at = 0.0;
        for i in 1..n {
            at += lens[i - 1];
            joints[i] = v::add(root, v::scale(dir, at));
        }
        return Solved { stretch: k, reached: v::dist(joints[n - 1], goal) <= tolerance };
    }
    // A straight chain has no bend to keep: start it bent toward the pole
    // (or any way) so it can fold.
    let hint = pole.unwrap_or_else(|| v::add(root, v::perpendicular(v::sub(goal, root))));
    lean(joints, hint, total * 1e-3);
    for _ in 0..iterations.max(1) {
        joints[n - 1] = aim;
        for i in (0..n - 1).rev() {
            let dir = v::norm(v::sub(joints[i], joints[i + 1]));
            let dir = if v::len(dir) < 0.5 { v::perpendicular(v::sub(aim, root)) } else { dir };
            joints[i] = v::add(joints[i + 1], v::scale(dir, lens[i]));
        }
        joints[0] = root;
        for i in 1..n {
            let dir = v::norm(v::sub(joints[i], joints[i - 1]));
            let dir = if v::len(dir) < 0.5 { v::norm(v::sub(aim, root)) } else { dir };
            joints[i] = v::add(joints[i - 1], v::scale(dir, lens[i - 1]));
        }
        if v::dist(joints[n - 1], aim) <= tolerance {
            break;
        }
    }
    if let Some(p) = pole {
        lean(joints, p, 0.0);
    }
    Solved { stretch: k, reached: v::dist(joints[n - 1], goal) <= tolerance.max(1e-9) }
}

/// Turns each middle joint about the line through its neighbours toward
/// `pole`: its distance to both neighbours stays, so the chain still
/// reaches. A joint on the line is pushed out by `nudge` (so it can fold).
fn lean<const N: usize>(joints: &mut [P<N>], pole: P<N>, nudge: f64) {
    for j in 1..joints.len().saturating_sub(1) {
        let (a, b) = (joints[j - 1], joints[j + 1]);
        let axis = v::norm(v::sub(b, a));
        if v::len(axis) < 0.5 {
            continue;
        }
        let off = v::sub(joints[j], a);
        let along = v::dot(off, axis);
        let perp = v::across(off, axis);
        let hint = v::norm(v::across(v::sub(pole, a), axis));
        if v::len(hint) < 0.5 {
            continue;
        }
        let r = v::len(perp).max(nudge);
        joints[j] = v::add(a, v::add(v::scale(axis, along), v::scale(hint, r)));
    }
}

/// Cyclic coordinate descent on positions: each joint from the end back,
/// in turn, swings what hangs from it to point the end at `goal`;
/// `after` runs after each swing (to clamp a joint to its limits).
pub fn ccd<const N: usize>(joints: &mut [P<N>], goal: P<N>, tolerance: f64, iterations: usize, mut after: impl FnMut(usize, &mut [P<N>])) -> Solved {
    let n = joints.len();
    if n < 2 {
        return Solved { stretch: 1.0, reached: false };
    }
    for _ in 0..iterations.max(1) {
        for i in (0..n - 1).rev() {
            let end = joints[n - 1];
            if v::dist(end, goal) <= tolerance {
                return Solved { stretch: 1.0, reached: true };
            }
            let (from, to) = (v::sub(end, joints[i]), v::sub(goal, joints[i]));
            let at = joints[i];
            for j in joints.iter_mut().skip(i + 1) {
                *j = v::add(at, v::turn(v::sub(*j, at), from, to));
            }
            after(i, joints);
        }
    }
    Solved { stretch: 1.0, reached: v::dist(joints[n - 1], goal) <= tolerance }
}

/// Where a pole goes to keep the bend of the chain `root`-`mid`-`end` as
/// it is (switching FK to IK without a jump): in front of the middle
/// joint, `distance` out. A straight chain bends along `fallback`.
pub fn pole_for<const N: usize>(root: P<N>, mid: P<N>, end: P<N>, distance: f64, fallback: P<N>) -> P<N> {
    let axis = v::norm(v::sub(end, root));
    let mut out = v::norm(v::across(v::sub(mid, root), axis));
    if v::len(out) < 0.5 {
        out = v::norm(v::across(fallback, axis));
    }
    if v::len(out) < 0.5 {
        out = v::perpendicular(axis);
    }
    v::add(mid, v::scale(out, distance))
}

/// A direction kept within `max` radians of `rest` (a ball joint's cone
/// limit): past it, the nearest direction on the cone.
pub fn cone_limit<const N: usize>(dir: P<N>, rest: P<N>, max: f64) -> P<N> {
    let a = v::angle(dir, rest);
    if a <= max || a < 1e-12 {
        return dir;
    }
    let l = v::len(dir);
    // Turn the rest direction toward `dir` by `max`.
    let (r, d) = (v::norm(rest), v::norm(dir));
    let w = v::norm(v::across(d, r));
    let w = if v::len(w) < 0.5 { v::perpendicular(r) } else { w };
    v::scale(v::add(v::scale(r, max.cos()), v::scale(w, max.sin())), l)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close<const N: usize>(a: P<N>, b: P<N>, tol: f64) -> bool {
        v::dist(a, b) <= tol
    }

    #[test]
    fn two_bones_reach_a_reachable_goal_and_bend_toward_the_pole() {
        let root = [0.0, 0.0];
        // Pole above (-y): the elbow goes up.
        let s = two_bone(root, 1.0, 1.0, [1.5, 0.0], [0.0, -1.0], Reach::default());
        assert!(s.reached && close(s.end, [1.5, 0.0], 1e-9), "{s:?}");
        assert!(s.mid[1] < -0.1, "{s:?}");
        assert!((v::dist(root, s.mid) - 1.0).abs() < 1e-9 && (v::dist(s.mid, s.end) - 1.0).abs() < 1e-9);
        // The pole below: the other way.
        let t = two_bone(root, 1.0, 1.0, [1.5, 0.0], [0.0, 1.0], Reach::default());
        assert!(t.mid[1] > 0.1);
        // 3D: the knee toward a pole in front (+z).
        let k = two_bone([0.0, 0.0, 0.0], 1.0, 1.0, [0.0, -1.6, 0.0], [0.0, 0.0, 1.0], Reach::default());
        assert!(k.reached && k.mid[2] > 0.5 && k.mid[0].abs() < 1e-9, "{k:?}");
    }

    #[test]
    fn unreachable_goals_get_a_straight_limb_pointing_at_them_or_a_stretch() {
        let s = two_bone([0.0, 0.0], 1.0, 1.0, [3.0, 4.0], [0.0, -1.0], Reach::default());
        assert!(!s.reached);
        assert!(close(s.end, [1.2, 1.6], 1e-6), "straight at the goal: {s:?}");
        assert!((v::dist([0.0, 0.0], s.mid) - 1.0).abs() < 1e-6);
        // Stretch up to 3×: reaches 5 away with 2 of bone.
        let st = two_bone([0.0, 0.0], 1.0, 1.0, [3.0, 4.0], [0.0, -1.0], Reach { stretch: 3.0, soft: 0.0 });
        assert!(st.reached && (st.stretch - 2.5).abs() < 1e-9, "{st:?}");
        // A stretch limit stops short.
        let lim = two_bone([0.0, 0.0], 1.0, 1.0, [3.0, 4.0], [0.0, -1.0], Reach { stretch: 1.5, soft: 0.0 });
        assert!(!lim.reached && (v::len(lim.end) - 3.0).abs() < 1e-9);
    }

    #[test]
    fn soft_ik_eases_into_straight_and_stretch_makes_up_the_rest() {
        let r = Reach { stretch: 1.0, soft: 0.2 };
        // Well inside the soft zone's start: exact.
        assert_eq!(reach_distance(1.0, 2.0, r).0, 1.0);
        // Near full reach: short of the goal, smoothly.
        let (a, _) = reach_distance(1.9, 2.0, r);
        let (b, _) = reach_distance(2.0, 2.0, r);
        let (c, _) = reach_distance(5.0, 2.0, r);
        assert!(a < 1.9 && b < 2.0 && a < b && b < c && c <= 2.0, "{a} {b} {c}");
        // The reach never jumps: tiny steps in, tiny steps out.
        let mut last = 0.0;
        for i in 0..400 {
            let (d, _) = reach_distance(i as f64 * 0.01, 2.0, r);
            assert!(d - last < 0.0101 && d >= last - 1e-12);
            last = d;
        }
        // Soft and stretchy: the end lands on the goal again.
        let (d, k) = reach_distance(1.95, 2.0, Reach { stretch: 2.0, soft: 0.2 });
        assert!((d - 1.95).abs() < 1e-9 && k > 1.0);
    }

    #[test]
    fn fabrik_reaches_with_long_chains_keeps_lengths_and_honours_a_pole() {
        let mut j: Vec<P<2>> = (0..5).map(|i| [i as f64, 0.0]).collect();
        let s = fabrik(&mut j, [2.0, 2.0], Some([0.0, 3.0]), Reach::default(), 1e-6, 64);
        assert!(s.reached, "{j:?}");
        for w in j.windows(2) {
            assert!((v::dist(w[0], w[1]) - 1.0).abs() < 1e-6);
        }
        assert_eq!(j[0], [0.0, 0.0]);
        // Out of reach: a straight line toward the goal.
        let mut j: Vec<P<3>> = (0..4).map(|i| [0.0, -(i as f64), 0.0]).collect();
        let s = fabrik(&mut j, [0.0, -10.0, 0.0], None, Reach::default(), 1e-6, 16);
        assert!(!s.reached && close(j[3], [0.0, -3.0, 0.0], 1e-9));
        // Stretchy: gets there.
        let s = fabrik(&mut j, [0.0, -4.0, 0.0], None, Reach { stretch: 2.0, soft: 0.0 }, 1e-6, 16);
        assert!(s.reached && (s.stretch - 4.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn ccd_reaches_and_lets_the_caller_limit_each_joint() {
        let mut j: Vec<P<2>> = (0..4).map(|i| [i as f64, 0.0]).collect();
        assert!(ccd(&mut j, [1.0, 2.0], 1e-4, 64, |_, _| {}).reached);
        // A clamp that keeps everything on the x axis's upper side.
        let mut j: Vec<P<2>> = (0..4).map(|i| [i as f64, 0.0]).collect();
        let s = ccd(&mut j, [1.0, -2.0], 1e-4, 16, |_, js| {
            for p in js.iter_mut() {
                p[1] = p[1].max(0.0);
            }
        });
        assert!(!s.reached && j.iter().all(|p| p[1] >= 0.0));
    }

    #[test]
    fn the_matching_pole_keeps_an_fk_bend_and_cones_limit_swing() {
        let (root, mid, end) = ([0.0, 0.0, 0.0], [0.5, -1.0, 0.3], [0.0, -2.0, 0.0]);
        let pole = pole_for(root, mid, end, 1.0, [0.0, 0.0, 1.0]);
        // Solving IK on the FK end with that pole gives the FK elbow back.
        let s = two_bone(root, v::dist(root, mid), v::dist(mid, end), end, v::sub(pole, root), Reach::default());
        assert!(close(s.mid, mid, 1e-6), "{s:?}");
        let d = cone_limit([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], 30f64.to_radians());
        assert!((v::angle(d, [0.0, 1.0, 0.0]) - 30f64.to_radians()).abs() < 1e-9);
        assert_eq!(cone_limit([0.1, 1.0, 0.0], [0.0, 1.0, 0.0], 1.0), [0.1, 1.0, 0.0]);
    }
}
