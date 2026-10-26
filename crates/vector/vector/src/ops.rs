// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shapes and what changes them: the primitives (rectangle, ellipse,
//! polygon, star) and the path operators of a shape layer (merge,
//! offset, trim, wiggle, zig-zag, round corners, pucker & bloat, twist,
//! morph).

use crate::path::{Cubic, M, P, Path, Sub, V, apply, dist, smooth};
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::single::SingleFloatOverlay;
use i_overlay::mesh::float::outline::offset::OutlineOffset;
use i_overlay::mesh::float::style::{LineJoin, OutlineStyle};

/// The tangent length that makes a quarter circle from a cubic.
pub const KAPPA: f64 = 0.552_284_749_8;

/// A rectangle centred on `c`, `size` wide and high, corners rounded by
/// `r`. Starts at the top right going clockwise (as is usual, which
/// matters to trim paths).
pub fn rect(c: P, size: P, r: f64) -> Sub {
    let (hw, hh) = (size[0].abs() / 2.0, size[1].abs() / 2.0);
    let r = r.clamp(0.0, hw.min(hh));
    let (l, t, rr, b) = (c[0] - hw, c[1] - hh, c[0] + hw, c[1] + hh);
    if r <= 0.0 {
        return Sub::polyline(&[[rr, t], [rr, b], [l, b], [l, t]], true);
    }
    let k = KAPPA * r;
    let v = vec![
        V { p: [rr, t + r], i: [rr, t + r - k], o: [rr, t + r] },
        V { p: [rr, b - r], i: [rr, b - r], o: [rr, b - r + k] },
        V { p: [rr - r, b], i: [rr - r + k, b], o: [rr - r, b] },
        V { p: [l + r, b], i: [l + r, b], o: [l + r - k, b] },
        V { p: [l, b - r], i: [l, b - r + k], o: [l, b - r] },
        V { p: [l, t + r], i: [l, t + r], o: [l, t + r - k] },
        V { p: [l + r, t], i: [l + r - k, t], o: [l + r, t] },
        V { p: [rr - r, t], i: [rr - r, t], o: [rr - r + k, t] },
    ];
    Sub { v, closed: true }
}

/// An ellipse centred on `c`, starting at the top, clockwise.
pub fn ellipse(c: P, size: P) -> Sub {
    let (rx, ry) = (size[0].abs() / 2.0, size[1].abs() / 2.0);
    let (kx, ky) = (KAPPA * rx, KAPPA * ry);
    let v = vec![
        V { p: [c[0], c[1] - ry], i: [c[0] - kx, c[1] - ry], o: [c[0] + kx, c[1] - ry] },
        V { p: [c[0] + rx, c[1]], i: [c[0] + rx, c[1] - ky], o: [c[0] + rx, c[1] + ky] },
        V { p: [c[0], c[1] + ry], i: [c[0] + kx, c[1] + ry], o: [c[0] - kx, c[1] + ry] },
        V { p: [c[0] - rx, c[1]], i: [c[0] - rx, c[1] + ky], o: [c[0] - rx, c[1] - ky] },
    ];
    Sub { v, closed: true }
}

/// A star (`inner` set) or regular polygon with `points` points, the
/// first straight up (turned by `rotation` degrees). `roundness` (0..1)
/// bends the edges as outer / inner roundness do.
pub fn star(c: P, points: f64, outer: f64, inner: Option<f64>, rotation: f64, outer_round: f64, inner_round: f64) -> Sub {
    let n = points.round().clamp(3.0, 200.0) as usize;
    let verts = if inner.is_some() { 2 * n } else { n };
    let step = std::f64::consts::TAU / verts as f64;
    let start = rotation.to_radians() - std::f64::consts::FRAC_PI_2;
    let v = (0..verts)
        .map(|k| {
            let is_inner = inner.is_some() && k % 2 == 1;
            let r = if is_inner { inner.unwrap_or(0.0) } else { outer };
            let round = if is_inner { inner_round } else { outer_round };
            let a = start + step * k as f64;
            let p = [c[0] + r * a.cos(), c[1] + r * a.sin()];
            // Handles along the tangent of the circle through the vertex,
            // a quarter of the edge long at full roundness.
            let edge = 2.0 * r * (step / 2.0).sin();
            let h = round * edge * 0.5 * KAPPA;
            let t = [-a.sin(), a.cos()];
            V { p, i: [p[0] - t[0] * h, p[1] - t[1] * h], o: [p[0] + t[0] * h, p[1] + t[1] * h] }
        })
        .collect();
    Sub { v, closed: true }
}

/// Flattened outlines for the polygon engine.
fn polygons(paths: &[Path], tol: f64) -> Vec<Vec<P>> {
    paths.iter().flat_map(|p| p.subs.iter()).filter(|s| s.v.len() > 1).map(|s| s.flatten(tol)).filter(|c| c.len() >= 3).collect()
}

fn from_contours(shapes: Vec<Vec<Vec<P>>>) -> Path {
    Path { subs: shapes.into_iter().flatten().filter(|c| c.len() >= 3).map(|c| Sub::polyline(&c, true)).collect() }
}

/// How shapes combine in a merge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Merge {
    /// All of them as one outline.
    Union,
    /// The first with the others cut out of it.
    Subtract,
    /// Only where they all overlap.
    Intersect,
    /// Where an odd number overlap.
    Exclude,
}

/// Merges paths (each path is one shape; its own sub-paths combine by
/// `even_odd` or non-zero).
pub fn merge(paths: &[Path], mode: Merge, tol: f64) -> Path {
    let shapes: Vec<Vec<Vec<P>>> = paths.iter().map(|p| polygons(std::slice::from_ref(p), tol)).filter(|s| !s.is_empty()).collect();
    let Some(first) = shapes.first() else { return Path::default() };
    let fill = FillRule::NonZero;
    // Each shape cleaned on its own first, so self-overlaps count once.
    let mut acc: Vec<Vec<Vec<P>>> = first.overlay(&Vec::<Vec<P>>::new(), OverlayRule::Subject, fill);
    for s in &shapes[1..] {
        let rule = match mode {
            Merge::Union => OverlayRule::Union,
            Merge::Subtract => OverlayRule::Difference,
            Merge::Intersect => OverlayRule::Intersect,
            Merge::Exclude => OverlayRule::Xor,
        };
        let subj: Vec<Vec<P>> = acc.into_iter().flatten().collect();
        acc = subj.overlay(s, rule, fill);
    }
    from_contours(acc)
}

/// Grows (`amount` > 0) or shrinks every closed outline by `amount`.
pub fn offset(paths: &[Path], amount: f64, join: &str, miter: f64, tol: f64) -> Path {
    if amount.abs() < 1e-9 {
        return Path { subs: paths.iter().flat_map(|p| p.subs.clone()).collect() };
    }
    let mut subs = Vec::new();
    for p in paths {
        // Only closed outlines have an inside to grow from; open ones pass.
        let closed: Vec<Vec<P>> = p.subs.iter().filter(|s| s.closed && s.v.len() > 1).map(|s| s.flatten(tol)).filter(|c| c.len() >= 3).collect();
        subs.extend(p.subs.iter().filter(|s| !s.closed).cloned());
        if closed.is_empty() {
            continue;
        }
        // The engine wants outer outlines counter-clockwise in its axes
        // (y up): clean to one shape first, which also orients them.
        let shapes: Vec<Vec<Vec<P>>> = closed.overlay(&Vec::<Vec<P>>::new(), OverlayRule::Subject, FillRule::NonZero);
        let j = match join {
            "round" => LineJoin::Round(0.1),
            "bevel" => LineJoin::Bevel,
            _ => LineJoin::Miter((std::f64::consts::PI / miter.max(1.0)).clamp(0.05, 3.0)),
        };
        let style = OutlineStyle::new(amount).line_join(j);
        let out = shapes.outline(&style);
        subs.extend(from_contours(out).subs);
    }
    Path { subs }
}

/// Trim paths: the part between `start` and `end` (fractions 0..1), slid
/// round by `offset` (fractions, 1 a whole turn). `together`: each path
/// trimmed alike; otherwise all of them as one long line.
pub fn trim(paths: &[Path], start: f64, end: f64, offset: f64, together: bool) -> Vec<Path> {
    let (mut a, mut b) = (start.clamp(0.0, 1.0), end.clamp(0.0, 1.0));
    if a > b {
        std::mem::swap(&mut a, &mut b);
    }
    if b - a >= 1.0 - 1e-9 && offset.rem_euclid(1.0) == 0.0 {
        return paths.to_vec();
    }
    if b - a <= 1e-9 {
        return paths.iter().map(|_| Path::default()).collect();
    }
    let o = offset.rem_euclid(1.0);
    let (a, b) = (a + o, b + o);
    // One run over `total`, cut between a·total and b·total, wrapping.
    let cut_run = |subs: &[&Sub], total: f64| -> Vec<(usize, Sub)> {
        let mut out = Vec::new();
        let ranges: Vec<(f64, f64)> = if b <= 1.0 { vec![(a, b)] } else if a >= 1.0 { vec![(a - 1.0, b - 1.0)] } else { vec![(a, 1.0), (0.0, b - 1.0)] };
        let mut at = 0.0;
        for (k, s) in subs.iter().enumerate() {
            let l = s.length();
            for (ra, rb) in &ranges {
                let (lo, hi) = ((ra * total - at).max(0.0), (rb * total - at).min(l));
                if hi > lo + 1e-9 {
                    for piece in s.cut(lo, hi) {
                        out.push((k, piece));
                    }
                }
            }
            at += l;
        }
        out
    };
    let join_wrap = |pieces: Vec<(usize, Sub)>, sub: &Sub| -> Vec<Sub> {
        // A closed path cut across its start comes back as its tail then
        // its head: one continuous line, so caps and dashes don't break.
        if sub.closed && pieces.len() == 2 && b > 1.0 && a < 1.0 {
            let (head, tail) = (&pieces[1].1, &pieces[0].1);
            let mut segs = tail.segments();
            segs.extend(head.segments());
            return vec![Sub::from_segments(&segs, false)];
        }
        pieces.into_iter().map(|p| p.1).collect()
    };
    if together {
        paths
            .iter()
            .map(|p| {
                Path {
                    subs: p
                        .subs
                        .iter()
                        .flat_map(|s| {
                            let l = s.length();
                            join_wrap(cut_run(&[s], l), s)
                        })
                        .collect(),
                }
            })
            .collect()
    } else {
        let all: Vec<(usize, &Sub)> = paths.iter().enumerate().flat_map(|(k, p)| p.subs.iter().map(move |s| (k, s))).collect();
        let subs: Vec<&Sub> = all.iter().map(|(_, s)| *s).collect();
        let total: f64 = subs.iter().map(|s| s.length()).sum();
        let mut out: Vec<Path> = paths.iter().map(|_| Path::default()).collect();
        for (k, piece) in cut_run(&subs, total) {
            out[all[k].0].subs.push(piece);
        }
        out
    }
}

/// Points spaced about `step` apart round a sub-path, with their unit
/// normals (pointing left of travel).
fn sample(s: &Sub, step: f64) -> Vec<(P, P)> {
    let segs = s.segments();
    let mut out = Vec::new();
    for c in &segs {
        let n = (c.length() / step.max(0.5)).ceil().max(1.0) as usize;
        for k in 0..n {
            let t = c.t_at_length(k as f64 / n as f64);
            let p = c.at(t);
            let d = c.tangent(t);
            let l = (d[0] * d[0] + d[1] * d[1]).sqrt().max(1e-12);
            out.push((p, [d[1] / l, -d[0] / l]));
        }
    }
    if !s.closed
        && let Some(c) = segs.last()
    {
        let d = c.tangent(1.0);
        let l = (d[0] * d[0] + d[1] * d[1]).sqrt().max(1e-12);
        out.push((c.3, [d[1] / l, -d[0] / l]));
    }
    out
}

/// Wiggle paths: the outline wobbles. `size` px of movement, `detail`
/// points per 100 px of outline, `per_second` wobbles a second,
/// `correlation` (0..1) how much neighbours move together.
pub fn wiggle(paths: &[Path], size: f64, detail: f64, per_second: f64, correlation: f64, seed: u32, t: f64, smooth_points: bool) -> Vec<Path> {
    let step = 100.0 / detail.clamp(0.2, 50.0);
    let spread = (1.0 - correlation.clamp(0.0, 1.0)) * 3.0 + 0.05;
    paths
        .iter()
        .map(|p| Path {
            subs: p
                .subs
                .iter()
                .enumerate()
                .map(|(si, s)| {
                    let pts: Vec<P> = sample(s, step)
                        .iter()
                        .enumerate()
                        .map(|(k, (q, n))| {
                            let ph = k as f64 * spread;
                            let sd = seed.wrapping_add(si as u32 * 977);
                            let a = studio_model::modifier::noise(t * per_second + ph, sd);
                            let b = studio_model::modifier::noise(t * per_second + ph + 37.7, sd.wrapping_add(1));
                            [q[0] + n[0] * a * size + -n[1] * b * size * 0.5, q[1] + n[1] * a * size + n[0] * b * size * 0.5]
                        })
                        .collect();
                    if smooth_points { smooth(&pts, s.closed, 1.0) } else { Sub::polyline(&pts, s.closed) }
                })
                .collect(),
        })
        .collect()
}

/// Zig-zag: `ridges` peaks along each sub-path, `size` px out and in.
pub fn zigzag(paths: &[Path], size: f64, ridges: f64, smooth_points: bool) -> Vec<Path> {
    paths
        .iter()
        .map(|p| Path {
            subs: p
                .subs
                .iter()
                .map(|s| {
                    let l = s.length();
                    let n = (ridges.round().max(1.0) * 2.0) as usize;
                    let pts: Vec<P> = sample(s, l / n as f64)
                        .iter()
                        .enumerate()
                        .map(|(k, (q, nr))| {
                            let s2 = if k % 2 == 0 { 1.0 } else { -1.0 };
                            [q[0] + nr[0] * size * s2, q[1] + nr[1] * size * s2]
                        })
                        .collect();
                    if smooth_points { smooth(&pts, s.closed, 1.0) } else { Sub::polyline(&pts, s.closed) }
                })
                .collect(),
        })
        .collect()
}

/// Rounds every sharp corner by `radius`.
pub fn round_corners(paths: &[Path], radius: f64) -> Vec<Path> {
    if radius <= 0.0 {
        return paths.to_vec();
    }
    paths
        .iter()
        .map(|p| Path {
            subs: p
                .subs
                .iter()
                .map(|s| {
                    let n = s.v.len();
                    if n < 3 {
                        return s.clone();
                    }
                    let mut v = Vec::with_capacity(n * 2);
                    for k in 0..n {
                        let cur = s.v[k];
                        let ends = !s.closed && (k == 0 || k == n - 1);
                        if !cur.is_corner() || ends {
                            v.push(cur);
                            continue;
                        }
                        let prev = s.v[(k + n - 1) % n].p;
                        let next = s.v[(k + 1) % n].p;
                        let (d0, d1) = (dist(prev, cur.p), dist(cur.p, next));
                        if d0 < 1e-9 || d1 < 1e-9 {
                            v.push(cur);
                            continue;
                        }
                        // Almost straight: nothing to round.
                        let u0 = [(cur.p[0] - prev[0]) / d0, (cur.p[1] - prev[1]) / d0];
                        let u1 = [(next[0] - cur.p[0]) / d1, (next[1] - cur.p[1]) / d1];
                        let turn = (u0[0] * u1[0] + u0[1] * u1[1]).clamp(-1.0, 1.0).acos();
                        if turn < 0.05 {
                            v.push(cur);
                            continue;
                        }
                        // Distance back along each edge so the arc has the
                        // radius, held to half of each edge.
                        let cut = (radius * (turn / 2.0).tan()).min(d0 / 2.0).min(d1 / 2.0);
                        let a = [cur.p[0] - u0[0] * cut, cur.p[1] - u0[1] * cut];
                        let b = [cur.p[0] + u1[0] * cut, cur.p[1] + u1[1] * cut];
                        // The cubic handle of a circular arc through the turn.
                        let h = 4.0 / 3.0 * (turn / 4.0).tan() * cut / (turn / 2.0).tan().max(1e-9);
                        v.push(V { p: a, i: a, o: [a[0] + u0[0] * h, a[1] + u0[1] * h] });
                        v.push(V { p: b, i: [b[0] - u1[0] * h, b[1] - u1[1] * h], o: b });
                    }
                    Sub { v, closed: s.closed }
                })
                .collect(),
        })
        .collect()
}

/// Pucker (negative) and bloat (positive), -1..1: vertices go towards the
/// centre and the edges bulge out, or the other way.
pub fn pucker_bloat(paths: &[Path], amount: f64) -> Vec<Path> {
    paths
        .iter()
        .map(|p| {
            let all: Vec<P> = p.subs.iter().flat_map(|s| s.v.iter().map(|v| v.p)).collect();
            let n = all.len().max(1) as f64;
            let c = all.iter().fold([0.0, 0.0], |a, q| [a[0] + q[0] / n, a[1] + q[1] / n]);
            let to = |q: P, k: f64| [q[0] + (c[0] - q[0]) * k, q[1] + (c[1] - q[1]) * k];
            Path { subs: p.subs.iter().map(|s| Sub { v: s.v.iter().map(|v| V { p: to(v.p, amount), i: to(v.i, -amount), o: to(v.o, -amount) }).collect(), closed: s.closed }).collect() }
        })
        .collect()
}

/// Twists the outline about its centre: `angle` degrees at the edge, less
/// towards the middle.
pub fn twist(paths: &[Path], angle: f64, centre: P) -> Vec<Path> {
    paths
        .iter()
        .map(|p| {
            let reach = p.subs.iter().flat_map(|s| s.v.iter()).map(|v| dist(v.p, centre)).fold(1e-9, f64::max);
            let subs = p
                .subs
                .iter()
                .map(|s| {
                    // Denser points so the twist curves.
                    let pts: Vec<P> = sample(s, (s.length() / 96.0).max(1.0))
                        .into_iter()
                        .map(|(q, _)| {
                            let r = dist(q, centre) / reach;
                            let (sn, cs) = (angle.to_radians() * r).sin_cos();
                            let d = [q[0] - centre[0], q[1] - centre[1]];
                            [centre[0] + d[0] * cs - d[1] * sn, centre[1] + d[0] * sn + d[1] * cs]
                        })
                        .collect();
                    smooth(&pts, s.closed, 1.0)
                })
                .collect();
            Path { subs }
        })
        .collect()
}

/// `n` points evenly round a sub-path.
fn resample(s: &Sub, n: usize) -> Vec<P> {
    let segs = s.segments();
    let lens: Vec<f64> = segs.iter().map(Cubic::length).collect();
    let total: f64 = lens.iter().sum();
    if total <= 0.0 || segs.is_empty() {
        return vec![s.v.first().map(|v| v.p).unwrap_or([0.0, 0.0]); n];
    }
    let span = if s.closed { n as f64 } else { (n - 1).max(1) as f64 };
    (0..n)
        .map(|k| {
            let want = total * k as f64 / span;
            let mut at = 0.0;
            for (c, l) in segs.iter().zip(&lens) {
                if want <= at + l || std::ptr::eq(c, segs.last().unwrap_or(c)) {
                    let t = c.t_at_length(((want - at) / l.max(1e-12)).clamp(0.0, 1.0));
                    return c.at(t);
                }
                at += l;
            }
            segs[segs.len() - 1].3
        })
        .collect()
}

/// Morphs `from` into `to` by `amount` (0..1; an eased amount may overshoot)
/// with `studio-morph`'s correspondence: subpaths paired by place and size
/// (extras grow from a point), outlines resampled to equal counts with
/// corners kept, winding matched and the start point that moves least.
pub fn morph(from: &[Path], to: &[Path], amount: f64) -> Vec<Path> {
    let has_to = to.iter().any(|p| !p.subs.is_empty());
    if amount == 0.0 || !has_to {
        return from.to_vec();
    }
    if amount == 1.0 {
        return to.to_vec();
    }
    let contours = |ps: &[Path]| -> Vec<studio_morph::Contour> { ps.iter().flat_map(|p| p.subs.iter()).map(|s| studio_morph::Contour::new(s.flatten(0.25), s.closed)).collect() };
    let m = studio_morph::Morph::new(&contours(from), &contours(to), studio_morph::Options::default());
    vec![Path { subs: m.at(amount).into_iter().map(|c| Sub::polyline(&c.pts, c.closed)).collect() }]
}

/// Every vertex of every path through `m`.
pub fn transform_all(paths: &[Path], m: &M) -> Vec<Path> {
    paths.iter().map(|p| p.transform(m)).collect()
}

/// The centre of the bounds of `paths`.
pub fn centre_of(paths: &[Path]) -> P {
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    for p in paths {
        if let Some((a, b)) = p.bounds() {
            lo = [lo[0].min(a[0]), lo[1].min(a[1])];
            hi = [hi[0].max(b[0]), hi[1].max(b[1])];
        }
    }
    if lo[0].is_finite() { [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0] } else { [0.0, 0.0] }
}

/// A point on the path through `m` (for tests and hit checks).
pub fn point_on(p: &Path, m: &M, frac: f64) -> Option<P> {
    let s = p.subs.first()?;
    let pts = resample(s, 257);
    Some(apply(m, pts[((frac.clamp(0.0, 1.0)) * 256.0).round() as usize]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area_of(p: &Path) -> f64 {
        p.subs.iter().map(|s| s.area()).sum::<f64>().abs()
    }

    #[test]
    fn primitives_have_the_right_size() {
        let r = rect([0.0, 0.0], [200.0, 100.0], 0.0);
        assert!((r.length() - 600.0).abs() < 1e-9);
        let rr = rect([0.0, 0.0], [200.0, 100.0], 20.0);
        // Four straight runs minus the corners plus a circle of radius 20.
        let want = 2.0 * (160.0 + 60.0) + std::f64::consts::TAU * 20.0;
        assert!((rr.length() - want).abs() < 0.5, "{} vs {want}", rr.length());
        let e = ellipse([0.0, 0.0], [200.0, 200.0]);
        assert!((e.length() - std::f64::consts::PI * 200.0).abs() < 0.5);
        let s = star([0.0, 0.0], 5.0, 100.0, Some(40.0), 0.0, 0.0, 0.0);
        assert_eq!(s.v.len(), 10);
        assert!((s.v[0].p[1] + 100.0).abs() < 1e-9, "first point straight up");
    }

    #[test]
    fn merges_add_cut_and_overlap() {
        let a = Path::new(vec![rect([0.0, 0.0], [100.0, 100.0], 0.0)]);
        let b = Path::new(vec![rect([50.0, 0.0], [100.0, 100.0], 0.0)]);
        let u = merge(&[a.clone(), b.clone()], Merge::Union, 0.25);
        assert!((area_of(&u) - 15_000.0).abs() < 1.0, "{}", area_of(&u));
        let s = merge(&[a.clone(), b.clone()], Merge::Subtract, 0.25);
        assert!((area_of(&s) - 5_000.0).abs() < 1.0);
        let i = merge(&[a.clone(), b.clone()], Merge::Intersect, 0.25);
        assert!((area_of(&i) - 5_000.0).abs() < 1.0);
        let x = merge(&[a, b], Merge::Exclude, 0.25);
        assert!((area_of(&x) - 10_000.0).abs() < 1.0);
    }

    #[test]
    fn offsets_grow_and_shrink() {
        let a = Path::new(vec![rect([0.0, 0.0], [100.0, 100.0], 0.0)]);
        let g = offset(std::slice::from_ref(&a), 10.0, "miter", 4.0, 0.25);
        assert!((area_of(&g) - 120.0 * 120.0).abs() < 2.0, "{}", area_of(&g));
        let s = offset(&[a], -10.0, "miter", 4.0, 0.25);
        assert!((area_of(&s) - 80.0 * 80.0).abs() < 2.0, "{}", area_of(&s));
    }

    #[test]
    fn trims_take_the_right_stretch() {
        let a = Path::new(vec![rect([0.0, 0.0], [100.0, 100.0], 0.0)]);
        let half = trim(std::slice::from_ref(&a), 0.0, 0.5, 0.0, true);
        assert!((half[0].length() - 200.0).abs() < 1e-6);
        // Slid round past the start: still one line, as long.
        let wrap = trim(std::slice::from_ref(&a), 0.0, 0.5, 0.75, true);
        assert_eq!(wrap[0].subs.len(), 1);
        assert!((wrap[0].length() - 200.0).abs() < 1e-6);
        // Two paths as one line: the first is drawn, the second not yet.
        let b = Path::new(vec![rect([300.0, 0.0], [100.0, 100.0], 0.0)]);
        let seq = trim(&[a.clone(), b], 0.0, 0.5, 0.0, false);
        assert!((seq[0].length() - 400.0).abs() < 1e-6 && seq[1].is_empty());
        assert!(trim(&[a], 0.3, 0.3, 0.0, true)[0].is_empty());
    }

    #[test]
    fn modifiers_keep_shapes_sensible() {
        let a = Path::new(vec![rect([0.0, 0.0], [200.0, 200.0], 0.0)]);
        let r = round_corners(std::slice::from_ref(&a), 30.0);
        assert_eq!(r[0].subs[0].v.len(), 8);
        assert!(area_of(&r[0]) < 40_000.0 && area_of(&r[0]) > 38_000.0, "{}", area_of(&r[0]));
        let z = zigzag(std::slice::from_ref(&a), 10.0, 20.0, false);
        assert!(z[0].length() > a.length() * 1.2);
        let w0 = wiggle(std::slice::from_ref(&a), 8.0, 4.0, 2.0, 0.5, 1, 0.0, true);
        let w1 = wiggle(std::slice::from_ref(&a), 8.0, 4.0, 2.0, 0.5, 1, 0.3, true);
        assert_ne!(w0, w1, "it moves over time");
        let b = pucker_bloat(std::slice::from_ref(&a), 0.3);
        assert!(b[0].subs[0].v[0].p[0].abs() < 100.0);
        let t = twist(std::slice::from_ref(&a), 90.0, [0.0, 0.0]);
        assert!(!t[0].is_empty());
    }

    #[test]
    fn morph_goes_from_one_shape_to_the_other() {
        let sq = Path::new(vec![rect([0.0, 0.0], [200.0, 200.0], 0.0)]);
        let ci = Path::new(vec![ellipse([0.0, 0.0], [200.0, 200.0])]);
        let half = morph(std::slice::from_ref(&sq), std::slice::from_ref(&ci), 0.5);
        let a = area_of(&half[0]);
        assert!(a < 40_000.0 && a > std::f64::consts::PI * 10_000.0, "{a}");
        assert_eq!(morph(std::slice::from_ref(&sq), std::slice::from_ref(&ci), 0.0), vec![sq.clone()]);
        assert_eq!(morph(std::slice::from_ref(&sq), std::slice::from_ref(&ci), 1.0), vec![ci]);
    }
}
