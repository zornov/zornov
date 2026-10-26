// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Morphing one outline into another.
//!
//! A shape is a list of contours (outlines and holes), each a dense
//! polyline. [`Morph::new`] finds which point of one corresponds to which
//! point of the other, and [`Morph::at`] gives the shape in between:
//!
//! 1. **Contours pair up** by position and size (an assignment that keeps
//!    the total mismatch least), so the outline of an `O` becomes the
//!    outline of a `D` and its hole the `D`'s hole. A contour with no
//!    partner grows from (or shrinks into) a point: its own centre.
//! 2. **Each pair is resampled** by arc length to the same number of
//!    points, enough for the longer outline to stay smooth, with corners
//!    of either outline kept as sample points so squares stay square and
//!    triangles keep their tips.
//! 3. **Winding is matched**: an outline drawn the other way round is
//!    reversed, so nothing turns inside out half way.
//! 4. **The best start point** is the cyclic shift that makes the two
//!    outlines, each about its own centre, closest: shapes neither twist
//!    nor cross themselves on the way. Open paths may be reversed instead.
//!
//! Every point then travels in a straight line; callers ease the amount.

pub type P = [f64; 2];

/// One outline or hole: points in order, closed back to the first or not.
#[derive(Clone, Debug, PartialEq)]
pub struct Contour {
    pub pts: Vec<P>,
    pub closed: bool,
}

impl Contour {
    pub fn new(pts: Vec<P>, closed: bool) -> Contour {
        Contour { pts, closed }
    }

    /// Signed area (positive clockwise on screen, y down); 0 when open.
    pub fn area(&self) -> f64 {
        if !self.closed {
            return 0.0;
        }
        area(&self.pts)
    }

    pub fn length(&self) -> f64 {
        length(&self.pts, self.closed)
    }

    pub fn centroid(&self) -> P {
        centroid(&self.pts)
    }
}

/// How finely the outlines are matched.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Points per contour; 0 picks enough for the longer outline (about
    /// one per 1/400 of the art's size, 32 to 512).
    pub points: usize,
    /// Turns sharper than this (degrees) are corners, kept as samples.
    pub corner_angle: f64,
}

impl Default for Options {
    fn default() -> Options {
        Options { points: 0, corner_angle: 30.0 }
    }
}

/// Two shapes with their points put in correspondence.
#[derive(Clone, Debug, PartialEq)]
pub struct Morph {
    /// For each contour pair: the start and end points (same count) and
    /// whether the in-between is closed.
    pairs: Vec<(Vec<P>, Vec<P>, bool)>,
}

fn sub(a: P, b: P) -> P {
    [a[0] - b[0], a[1] - b[1]]
}

fn dist2(a: P, b: P) -> f64 {
    let d = sub(a, b);
    d[0] * d[0] + d[1] * d[1]
}

fn lerp(a: P, b: P, t: f64) -> P {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

fn area(p: &[P]) -> f64 {
    let n = p.len();
    if n < 3 {
        return 0.0;
    }
    (0..n).map(|i| p[i][0] * p[(i + 1) % n][1] - p[(i + 1) % n][0] * p[i][1]).sum::<f64>() * 0.5
}

fn length(p: &[P], closed: bool) -> f64 {
    let mut l: f64 = p.windows(2).map(|w| dist2(w[0], w[1]).sqrt()).sum();
    if closed && p.len() > 1 {
        l += dist2(p[p.len() - 1], p[0]).sqrt();
    }
    l
}

fn centroid(p: &[P]) -> P {
    if p.is_empty() {
        return [0.0, 0.0];
    }
    // Area-weighted for closed outlines, point average otherwise.
    let a = area(p);
    if a.abs() > 1e-12 {
        let n = p.len();
        let (mut cx, mut cy) = (0.0, 0.0);
        for i in 0..n {
            let (q, r) = (p[i], p[(i + 1) % n]);
            let c = q[0] * r[1] - r[0] * q[1];
            cx += (q[0] + r[0]) * c;
            cy += (q[1] + r[1]) * c;
        }
        return [cx / (6.0 * a), cy / (6.0 * a)];
    }
    let n = p.len() as f64;
    let s = p.iter().fold([0.0, 0.0], |acc, q| [acc[0] + q[0], acc[1] + q[1]]);
    [s[0] / n, s[1] / n]
}

/// The polyline without repeated points (and without a closing duplicate).
fn clean(c: &Contour) -> Contour {
    let mut pts: Vec<P> = Vec::with_capacity(c.pts.len());
    for &p in &c.pts {
        if p[0].is_finite() && p[1].is_finite() && pts.last().is_none_or(|q| dist2(*q, p) > 1e-18) {
            pts.push(p);
        }
    }
    if c.closed && pts.len() > 1 && dist2(pts[0], pts[pts.len() - 1]) <= 1e-18 {
        pts.pop();
    }
    Contour { pts, closed: c.closed }
}

/// Arc-length positions (0..1) of the corners of a polyline.
fn corners(p: &[P], closed: bool, angle: f64) -> Vec<f64> {
    let n = p.len();
    if n < 3 {
        return Vec::new();
    }
    let total = length(p, closed).max(1e-12);
    let limit = angle.to_radians().cos();
    let mut out = Vec::new();
    let mut s = 0.0;
    for i in 0..n {
        if i > 0 {
            s += dist2(p[i - 1], p[i]).sqrt();
        }
        if !closed && (i == 0 || i == n - 1) {
            continue;
        }
        let (a, b, c) = (p[(i + n - 1) % n], p[i], p[(i + 1) % n]);
        let (u, v) = (sub(b, a), sub(c, b));
        let (lu, lv) = ((u[0] * u[0] + u[1] * u[1]).sqrt(), (v[0] * v[0] + v[1] * v[1]).sqrt());
        if lu < 1e-12 || lv < 1e-12 {
            continue;
        }
        // Short segments either side are a curve's flattening, not a corner.
        let cos = (u[0] * v[0] + u[1] * v[1]) / (lu * lv);
        if cos < limit {
            out.push(s / total);
        }
    }
    out
}

/// `n` points spaced by arc length, starting at the first point, with the
/// positions in `pins` (0..1 of the length) hit exactly.
fn resample(p: &[P], closed: bool, n: usize, pins: &[f64]) -> Vec<P> {
    if p.is_empty() {
        return vec![[0.0, 0.0]; n];
    }
    if p.len() == 1 {
        return vec![p[0]; n];
    }
    let mut pts = p.to_vec();
    if closed {
        pts.push(p[0]);
    }
    let mut cum = vec![0.0];
    for w in pts.windows(2) {
        let l = cum[cum.len() - 1] + dist2(w[0], w[1]).sqrt();
        cum.push(l);
    }
    let total = cum[cum.len() - 1].max(1e-12);
    // Sample positions: even, then the nearest of each pinned position
    // moved onto it (each sample moves at most once).
    let steps = if closed { n } else { n.saturating_sub(1).max(1) };
    let mut at: Vec<f64> = (0..n).map(|k| k as f64 / steps as f64).collect();
    let mut taken = vec![false; n];
    for &pin in pins {
        let k = ((pin * steps as f64).round() as usize).min(n - 1);
        let best = [k, k.saturating_sub(1), (k + 1).min(n - 1)].into_iter().filter(|j| !taken[*j]).min_by(|x, y| (at[*x] - pin).abs().total_cmp(&(at[*y] - pin).abs()));
        if let Some(j) = best {
            at[j] = pin;
            taken[j] = true;
        }
    }
    let mut out = Vec::with_capacity(n);
    let mut i = 0;
    for &u in &at {
        let s = u * total;
        while i + 2 < cum.len() && cum[i + 1] < s {
            i += 1;
        }
        // Positions are nearly in order; step back when a pin moved one earlier.
        while i > 0 && cum[i] > s {
            i -= 1;
        }
        let seg = (cum[i + 1] - cum[i]).max(1e-12);
        out.push(lerp(pts[i], pts[i + 1], ((s - cum[i]) / seg).clamp(0.0, 1.0)));
    }
    out
}

/// The cyclic shift of `b` that best matches `a`, both about their centres.
fn best_shift(a: &[P], b: &[P]) -> usize {
    let n = a.len();
    if n == 0 || b.len() != n {
        return 0;
    }
    let (ca, cb) = (centroid(a), centroid(b));
    let ra: Vec<P> = a.iter().map(|p| sub(*p, ca)).collect();
    let rb: Vec<P> = b.iter().map(|p| sub(*p, cb)).collect();
    // Coarse search on every few points, then refined around the best.
    let cost = |s: usize, step: usize| (0..n).step_by(step).map(|i| dist2(ra[i], rb[(i + s) % n])).sum::<f64>();
    let coarse = (n / 128).max(1);
    let best = (0..n).step_by(coarse).min_by(|x, y| cost(*x, coarse.max(2) / 2).total_cmp(&cost(*y, coarse.max(2) / 2))).unwrap_or(0);
    let lo = best as isize - coarse as isize;
    (lo..=best as isize + coarse as isize).map(|s| s.rem_euclid(n as isize) as usize).min_by(|x, y| cost(*x, 1).total_cmp(&cost(*y, 1))).unwrap_or(best)
}

/// The assignment of rows to columns with the least total cost (Hungarian
/// method); `cost` is `rows × cols` with `rows <= cols`.
fn assign(cost: &[Vec<f64>]) -> Vec<usize> {
    let n = cost.len();
    let m = cost.first().map_or(0, Vec::len);
    if n == 0 {
        return Vec::new();
    }
    let inf = f64::INFINITY;
    let (mut u, mut v) = (vec![0.0; n + 1], vec![0.0; m + 1]);
    let (mut p, mut way) = (vec![0usize; m + 1], vec![0usize; m + 1]);
    for i in 1..=n {
        p[0] = i;
        let mut j0 = 0;
        let mut minv = vec![inf; m + 1];
        let mut used = vec![false; m + 1];
        loop {
            used[j0] = true;
            let i0 = p[j0];
            let mut delta = inf;
            let mut j1 = 0;
            for j in 1..=m {
                if !used[j] {
                    let cur = cost[i0 - 1][j - 1] - u[i0] - v[j];
                    if cur < minv[j] {
                        minv[j] = cur;
                        way[j] = j0;
                    }
                    if minv[j] < delta {
                        delta = minv[j];
                        j1 = j;
                    }
                }
            }
            for j in 0..=m {
                if used[j] {
                    u[p[j]] += delta;
                    v[j] -= delta;
                } else {
                    minv[j] -= delta;
                }
            }
            j0 = j1;
            if p[j0] == 0 {
                break;
            }
        }
        loop {
            let j1 = way[j0];
            p[j0] = p[j1];
            j0 = j1;
            if j0 == 0 {
                break;
            }
        }
    }
    let mut out = vec![0; n];
    for j in 1..=m {
        if p[j] > 0 {
            out[p[j] - 1] = j - 1;
        }
    }
    out
}

fn size_of(c: &Contour) -> f64 {
    let a = c.area().abs();
    if a > 1e-9 { a.sqrt() } else { c.length() / 4.0 }
}

impl Morph {
    /// Puts `from` and `to` in correspondence.
    pub fn new(from: &[Contour], to: &[Contour], opts: Options) -> Morph {
        let a: Vec<Contour> = from.iter().map(clean).filter(|c| !c.pts.is_empty()).collect();
        let b: Vec<Contour> = to.iter().map(clean).filter(|c| !c.pts.is_empty()).collect();
        // The art's size sets how fine the matching is.
        let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
        for c in a.iter().chain(&b) {
            for p in &c.pts {
                lo = [lo[0].min(p[0]), lo[1].min(p[1])];
                hi = [hi[0].max(p[0]), hi[1].max(p[1])];
            }
        }
        let diag = if lo[0] <= hi[0] { dist2(lo, hi).sqrt().max(1e-9) } else { 1.0 };
        // Pair contours: rows are the side with fewer.
        let swap = a.len() > b.len();
        let (rows, cols) = if swap { (&b, &a) } else { (&a, &b) };
        let cost: Vec<Vec<f64>> = rows
            .iter()
            .map(|r| {
                cols.iter()
                    .map(|c| {
                        let d = dist2(r.centroid(), c.centroid()).sqrt() / diag;
                        let (sr, sc) = (size_of(r).max(1e-9), size_of(c).max(1e-9));
                        // Holes pair with holes, outlines with outlines.
                        let sign = if r.area() * c.area() < 0.0 && r.closed && c.closed { 0.5 } else { 0.0 };
                        d + 0.5 * (sr / sc).ln().abs() + sign
                    })
                    .collect()
            })
            .collect();
        let pick = assign(&cost);
        let mut partner: Vec<Option<usize>> = vec![None; cols.len()];
        for (r, &c) in pick.iter().enumerate() {
            partner[c] = Some(r);
        }
        let mut pairs = Vec::new();
        for (ci, col) in cols.iter().enumerate() {
            let (ca, cb): (Contour, Contour) = match partner[ci] {
                Some(ri) => (rows[ri].clone(), col.clone()),
                // No partner: from (or to) a point at its own centre.
                None => (Contour::new(vec![col.centroid()], col.closed), col.clone()),
            };
            let (pa, pb) = if swap { (cb, ca) } else { (ca, cb) };
            pairs.push(pair(&pa, &pb, diag, opts));
        }
        Morph { pairs }
    }

    /// The shape at `t` (0 = `from`, 1 = `to`; values outside overshoot).
    pub fn at(&self, t: f64) -> Vec<Contour> {
        self.pairs.iter().map(|(a, b, closed)| Contour::new(a.iter().zip(b).map(|(p, q)| lerp(*p, *q, t)).collect(), *closed)).collect()
    }

    /// The two ends with corresponding points: the same contour and point
    /// counts in the same order, ready for point-by-point keyframes.
    pub fn ends(&self) -> (Vec<Contour>, Vec<Contour>) {
        (self.pairs.iter().map(|(a, _, c)| Contour::new(a.clone(), *c)).collect(), self.pairs.iter().map(|(_, b, c)| Contour::new(b.clone(), *c)).collect())
    }

    /// Points per contour pair.
    pub fn counts(&self) -> Vec<usize> {
        self.pairs.iter().map(|p| p.0.len()).collect()
    }
}

fn pair(a: &Contour, b: &Contour, diag: f64, opts: Options) -> (Vec<P>, Vec<P>, bool) {
    let closed = a.closed || b.closed;
    let longest = a.length().max(b.length());
    let n = if opts.points > 0 { opts.points.max(3) } else { ((longest / (diag / 400.0)).ceil() as usize).clamp(32, 512) };
    // A point (a contour growing from nothing) needs no shaping.
    let degenerate = |c: &Contour| c.pts.len() == 1 || c.length() < 1e-9;
    let pa_src = a.pts.clone();
    let mut pb_src = b.pts.clone();
    if closed && !degenerate(a) && !degenerate(b) && area(&pa_src) * area(&pb_src) < 0.0 {
        pb_src.reverse();
    }
    if !closed && !degenerate(a) && !degenerate(b) {
        // Open paths: run the same way (the ends closest together).
        let (a0, a1) = (pa_src[0], pa_src[pa_src.len() - 1]);
        let (b0, b1) = (pb_src[0], pb_src[pb_src.len() - 1]);
        if dist2(a0, b1) + dist2(a1, b0) < dist2(a0, b0) + dist2(a1, b1) {
            pb_src.reverse();
        }
    }
    let ka = if degenerate(a) { Vec::new() } else { corners(&pa_src, closed, opts.corner_angle) };
    let kb = if degenerate(b) { Vec::new() } else { corners(&pb_src, closed, opts.corner_angle) };
    let pa = resample(&pa_src, closed, n, &ka);
    if !closed || degenerate(a) || degenerate(b) {
        let pb = resample(&pb_src, closed, n, &kb);
        return (pa, pb, closed);
    }
    // Align b's start to a, then resample b again from that start so its
    // corners are hit exactly where they now fall.
    let rough = resample(&pb_src, closed, n, &kb);
    let s = best_shift(&pa, &rough);
    let start = rough[s];
    // Rotate the source outline to begin at `start`.
    let (mut best_i, mut best_d) = (0, f64::MAX);
    let m = pb_src.len();
    for i in 0..m {
        let (p, q) = (pb_src[i], pb_src[(i + 1) % m]);
        let d = sub(q, p);
        let l2 = (d[0] * d[0] + d[1] * d[1]).max(1e-18);
        let t = (((start[0] - p[0]) * d[0] + (start[1] - p[1]) * d[1]) / l2).clamp(0.0, 1.0);
        let dd = dist2(lerp(p, q, t), start);
        if dd < best_d {
            best_d = dd;
            best_i = i;
        }
    }
    let mut rotated = vec![start];
    for k in 1..=m {
        let q = pb_src[(best_i + k) % m];
        if dist2(q, start) > 1e-18 {
            rotated.push(q);
        }
    }
    let kb = corners(&rotated, closed, opts.corner_angle);
    let mut pb = resample(&rotated, closed, n, &kb);
    // The rotated start may itself be a corner of b: keep the alignment.
    let s2 = best_shift(&pa, &pb);
    pb.rotate_left(s2);
    (pa, pb, closed)
}

/// Easing curves for the amount.
pub fn ease(name: &str, t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    match name {
        "linear" => t,
        "ease_in" | "in" => t * t * t,
        "ease_out" | "out" => 1.0 - (1.0 - t).powi(3),
        "hold" => 0.0,
        "spring" => 1.0 - (-6.0 * t).exp() * (12.0 * t).cos(),
        _ => {
            if t < 0.5 {
                4.0 * t * t * t
            } else {
                1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
            }
        }
    }
}

/// A regular polygon (or a circle with many sides) for tests and callers.
pub fn regular(c: P, r: f64, sides: usize, rot_deg: f64) -> Contour {
    let pts = (0..sides)
        .map(|i| {
            let a = (rot_deg - 90.0).to_radians() + i as f64 * std::f64::consts::TAU / sides as f64;
            [c[0] + r * a.cos(), c[1] + r * a.sin()]
        })
        .collect();
    Contour::new(pts, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on_triangle(p: P, tri: &Contour) -> f64 {
        // Distance from p to the triangle's outline.
        let n = tri.pts.len();
        (0..n)
            .map(|i| {
                let (a, b) = (tri.pts[i], tri.pts[(i + 1) % n]);
                let d = sub(b, a);
                let t = (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / (d[0] * d[0] + d[1] * d[1])).clamp(0.0, 1.0);
                dist2(lerp(a, b, t), p).sqrt()
            })
            .fold(f64::MAX, f64::min)
    }

    #[test]
    fn circle_becomes_a_triangle_with_sharp_tips() {
        let circle = regular([0.0, 0.0], 100.0, 256, 0.0);
        let tri = regular([0.0, 0.0], 100.0, 3, 0.0);
        let m = Morph::new(&[circle.clone()], &[tri.clone()], Options::default());
        let end = &m.at(1.0)[0];
        // Every point lies on the triangle, and its three tips are hit exactly.
        assert!(end.pts.iter().all(|p| on_triangle(*p, &tri) < 1e-6));
        for tip in &tri.pts {
            assert!(end.pts.iter().any(|p| dist2(*p, *tip) < 1e-12), "tip {tip:?} kept");
        }
        // The start is the circle, the middle a fair in-between.
        let start = &m.at(0.0)[0];
        assert!(start.pts.iter().all(|p| (dist2(*p, [0.0, 0.0]).sqrt() - 100.0).abs() < 0.1));
        let mid = &m.at(0.5)[0];
        let (ac, at, am) = (circle.area().abs(), tri.area().abs(), mid.area().abs());
        assert!(am < ac && am > at, "{at} < {am} < {ac}");
    }

    #[test]
    fn start_points_align_so_nothing_twists() {
        // The same square with its points listed from another corner: the
        // in-between is that square all the way.
        let sq = Contour::new(vec![[0.0, 0.0], [100.0, 0.0], [100.0, 100.0], [0.0, 100.0]], true);
        let shifted = Contour::new(vec![[100.0, 100.0], [0.0, 100.0], [0.0, 0.0], [100.0, 0.0]], true);
        let m = Morph::new(&[sq.clone()], &[shifted], Options { points: 64, ..Default::default() });
        let (a, b) = m.ends();
        let worst = a[0].pts.iter().zip(&b[0].pts).map(|(p, q)| dist2(*p, *q)).fold(0.0, f64::max);
        assert!(worst < 1e-9, "points travel {worst}");
    }

    #[test]
    fn opposite_winding_is_matched() {
        let a = regular([0.0, 0.0], 100.0, 4, 45.0);
        let mut b = regular([10.0, 0.0], 100.0, 4, 45.0);
        b.pts.reverse();
        let m = Morph::new(&[a.clone()], &[b], Options::default());
        let mid = &m.at(0.5)[0];
        assert!(mid.area().abs() > 0.9 * a.area().abs(), "the shape does not turn inside out: {}", mid.area());
        assert!(mid.area() * a.area() > 0.0);
    }

    #[test]
    fn subpaths_pair_by_place_and_size_and_extras_grow_from_a_point() {
        // An O (outline and hole) into a disc: the outline becomes the disc,
        // the hole shrinks to its centre.
        let outer = regular([0.0, 0.0], 100.0, 64, 0.0);
        let mut hole = regular([0.0, 0.0], 40.0, 64, 0.0);
        hole.pts.reverse();
        let disc = regular([0.0, 0.0], 90.0, 64, 0.0);
        let m = Morph::new(&[hole.clone(), outer.clone()], &[disc.clone()], Options::default());
        let end = m.at(1.0);
        assert_eq!(end.len(), 2);
        let sizes: Vec<f64> = end.iter().map(|c| c.area().abs()).collect();
        assert!(sizes.iter().any(|a| (a - disc.area().abs()).abs() < disc.area().abs() * 0.02), "{sizes:?}");
        assert!(sizes.iter().any(|a| *a < 1e-6), "the hole has shrunk to a point: {sizes:?}");
        // Two circles listed in the other order still pair by position.
        let l = regular([-200.0, 0.0], 50.0, 32, 0.0);
        let r = regular([200.0, 0.0], 50.0, 32, 0.0);
        let r2 = regular([210.0, 0.0], 60.0, 32, 0.0);
        let l2 = regular([-210.0, 0.0], 60.0, 32, 0.0);
        let m = Morph::new(&[l, r], &[r2, l2], Options::default());
        let (a, b) = m.ends();
        for (x, y) in a.iter().zip(&b) {
            assert!((x.centroid()[0] - y.centroid()[0]).abs() < 20.0, "{:?} → {:?}", x.centroid(), y.centroid());
        }
    }

    #[test]
    fn open_paths_run_the_same_way() {
        let a = Contour::new(vec![[0.0, 0.0], [100.0, 0.0]], false);
        let b = Contour::new(vec![[100.0, 10.0], [50.0, 60.0], [0.0, 10.0]], false);
        let m = Morph::new(&[a], &[b], Options { points: 16, ..Default::default() });
        let (s, e) = m.ends();
        assert!(dist2(s[0].pts[0], e[0].pts[0]) < 200.0, "{:?} {:?}", s[0].pts[0], e[0].pts[0]);
        assert_eq!(s[0].pts.len(), 16);
        assert!(!m.at(0.5)[0].closed);
    }

    #[test]
    fn assignment_finds_the_least_total() {
        let cost = vec![vec![4.0, 1.0, 3.0], vec![2.0, 0.0, 5.0], vec![3.0, 2.0, 2.0]];
        assert_eq!(assign(&cost), vec![1, 0, 2]);
        assert!((ease("ease_in_out", 0.5) - 0.5).abs() < 1e-12);
    }
}
