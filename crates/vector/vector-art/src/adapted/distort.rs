//! Adapted from VectorCraft (<https://github.com/storytold/vectorcraft>, revision 8bf090b):
//! `crates/effects/src/distort.rs`, `crates/effects/src/stylize.rs` (round corners) and
//! `crates/effects/src/util.rs`. Copyright (c) 2026 ArtCraft Team and the VectorCraft
//! contributors; MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE` in this folder).
//! Changes: typed parameters instead of JSON values; sizes are absolute document units.

use kurbo::{ParamCurve, ParamCurveArclen};
use studio_vector_geom::corners;
use studio_vector_geom::shapes::KAPPA;
use studio_vector_geom::{Anchor, CubicBez, PathData, Point, Rect, SubPath, Vec2};

use super::warp::{fit_piece, map_nonlinear_within, pieces, seg_cubic};

fn diag(b: Rect) -> f64 {
    b.width().hypot(b.height()).max(1e-6)
}

/// Deterministic hash noise in [-1, 1] for (seed, a, b, c).
pub fn noise(seed: u64, a: u64, b: u64, c: u64) -> f64 {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ a.wrapping_mul(0xBF58_476D_1CE4_E5B9) ^ b.wrapping_mul(0x94D0_49BB_1331_11EB) ^ c;
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 31;
    (x >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
}

/// Free distort: bilinear map of the bounding box onto four new corners (TL, TR, BR, BL).
pub fn free_distort(path: &PathData, b: Rect, c: [Point; 4], max: usize) -> PathData {
    let (w, h) = (b.width().max(1e-9), b.height().max(1e-9));
    let f = |q: Point| {
        let u = (q.x - b.x0) / w;
        let v = (q.y - b.y0) / h;
        let top = c[0].lerp(c[1], u);
        let bot = c[3].lerp(c[2], u);
        top.lerp(bot, v)
    };
    map_nonlinear_within(path, diag(b) / 16.0, max, f)
}

/// Pucker & bloat: anchors move toward (bloat) or away from (pucker) the centre, handles the
/// opposite way; `amount` -200..200 percent.
pub fn pucker_bloat(path: &PathData, b: Rect, amount: f64) -> PathData {
    let a = amount.clamp(-200.0, 200.0) / 100.0;
    if a == 0.0 {
        return path.clone();
    }
    let c = b.center();
    let mut res = path.clone();
    for (si, sp) in res.subpaths.iter_mut().enumerate() {
        let n = sp.anchors.len();
        let segs = sp.segment_count();
        let orig = &path.subpaths[si];
        for i in 0..segs {
            let cub = seg_cubic(orig, i);
            let j = (i + 1) % n;
            sp.anchors[i].h_out = cub.p1;
            sp.anchors[j].h_in = cub.p2;
        }
        for an in &mut sp.anchors {
            let p = an.p;
            *an = Anchor::with_handles(p.lerp(c, a * 0.5), an.h_in.lerp(c, -a * 0.5), an.h_out.lerp(c, -a * 0.5));
        }
    }
    res
}

const MAX_ROUGHEN_POINTS: usize = 2000;

/// Roughen: resample at `detail` points per 100 units and jitter each by up to `size`.
pub fn roughen(path: &PathData, size: f64, detail: f64, smooth: bool, seed: u64, max: usize) -> PathData {
    let amp = size.clamp(0.0, 1e4);
    let detail = detail.clamp(0.0, 1000.0);
    let spacing = if detail > 0.0 { 100.0 / detail } else { f64::INFINITY };
    let lens: Vec<f64> = path.subpaths.iter().flat_map(|sp| (0..sp.segment_count()).map(|i| sp.segment(i).arclen(0.1))).collect();
    let spacing = fit_piece(&lens, spacing, MAX_ROUGHEN_POINTS, max.saturating_sub(path.anchor_count()));
    let mut lens = lens.iter();
    let mut subs = vec![];
    for (si, sp) in path.subpaths.iter().enumerate() {
        let mut pts = vec![];
        for (i, &len) in (0..sp.segment_count()).zip(&mut lens) {
            let c = sp.segment(i);
            let k = pieces(len, spacing, MAX_ROUGHEN_POINTS);
            for j in 0..k {
                pts.push(c.eval(j as f64 / k as f64));
            }
        }
        if !sp.closed
            && let Some(l) = sp.anchors.last()
        {
            pts.push(l.p);
        }
        if pts.is_empty() {
            pts.extend(sp.anchors.iter().map(|a| a.p));
        }
        for (k, q) in pts.iter_mut().enumerate() {
            q.x += amp * noise(seed, si as u64, k as u64, 1);
            q.y += amp * noise(seed, si as u64, k as u64, 2);
        }
        subs.push(if smooth { catmull_rom(&pts, sp.closed, 1.0) } else { SubPath::polyline(&pts, sp.closed) });
    }
    PathData::new(subs)
}

/// Twist: rotation strongest at the centre, fading to nothing at the bounding circle.
pub fn twist(path: &PathData, b: Rect, angle: f64, max: usize) -> PathData {
    let ang = angle.clamp(-3600.0, 3600.0).to_radians();
    if ang == 0.0 {
        return path.clone();
    }
    let c = b.center();
    let r = diag(b) / 2.0;
    let f = move |q: Point| {
        let d = q - c;
        let t = (1.0 - d.hypot() / r).max(0.0);
        let a = -ang * t;
        let (s, co) = a.sin_cos();
        c + Vec2::new(d.x * co - d.y * s, d.x * s + d.y * co)
    };
    let piece = (r / 8.0).min(r / (1.0 + ang.abs() * 2.0));
    map_nonlinear_within(path, piece, max, f)
}

const ZIGZAG_ARCLEN_ACCURACY: f64 = 1e-4;

struct Ridge {
    p: Point,
    dir: Vec2,
    gap_in: f64,
    gap_out: f64,
}

/// Unit normal (left of travel direction in y-down coordinates) of cubic `c` at `t`.
pub fn normal_at(c: &CubicBez, t: f64) -> Vec2 {
    let eps = 1e-4;
    let a = c.eval((t - eps).max(0.0));
    let b = c.eval((t + eps).min(1.0));
    let mut d = b - a;
    if d.hypot() < 1e-12 {
        d = c.p3 - c.p0;
    }
    let len = d.hypot();
    if len < 1e-12 { Vec2::ZERO } else { Vec2::new(d.y / len, -d.x / len) }
}

/// Zig zag: `ridges` extra points per segment, evenly spaced by length and alternately displaced
/// along the normal by `size`; smooth points make a wave.
pub fn zig_zag(path: &PathData, size: f64, ridges: f64, smooth: bool, max: usize) -> PathData {
    let amp = size.clamp(-1e4, 1e4);
    let segs = path.subpaths.iter().map(SubPath::segment_count).fold(0, usize::saturating_add);
    let ends: usize = path.subpaths.iter().filter(|sp| !sp.closed).map(|sp| if sp.segment_count() > 0 { 1 } else { sp.anchors.len() }).sum();
    let fit = (max.saturating_sub(ends) / segs.max(1)).saturating_sub(1);
    let ridges = (ridges.clamp(0.0, 100.0) as usize).min(fit);
    let mut subs = vec![];
    for sp in &path.subpaths {
        let segs = sp.segment_count();
        let gap = |i: usize| sp.segment(i).arclen(ZIGZAG_ARCLEN_ACCURACY) / (ridges + 1) as f64;
        let mut pts: Vec<Ridge> = vec![];
        let mut k = 0usize;
        for i in 0..segs {
            let c = sp.segment(i);
            let len = c.arclen(ZIGZAG_ARCLEN_ACCURACY);
            let step = len / (ridges + 1) as f64;
            for j in 0..=ridges {
                let s = j as f64 / (ridges + 1) as f64;
                let t = if j > 0 && len > 1e-9 { c.inv_arclen(s * len, ZIGZAG_ARCLEN_ACCURACY).clamp(0.0, 1.0) } else { s };
                let mut gap_in = step;
                let nrm = if j == 0 {
                    let prev = if i > 0 {
                        Some(i - 1)
                    } else if sp.closed {
                        Some(segs - 1)
                    } else {
                        None
                    };
                    gap_in = prev.map_or(0.0, gap);
                    let n1 = normal_at(&c, 0.0);
                    match prev {
                        Some(pi) => {
                            let s = n1 + normal_at(&sp.segment(pi), 1.0);
                            if s.hypot() > 1e-9 { s / s.hypot() } else { n1 }
                        }
                        None => n1,
                    }
                } else {
                    normal_at(&c, t)
                };
                let sign = if k.is_multiple_of(2) { 1.0 } else { -1.0 };
                pts.push(Ridge { p: c.eval(t) + nrm * (amp * sign), dir: Vec2::new(-nrm.y, nrm.x), gap_in, gap_out: step });
                k += 1;
            }
        }
        if !sp.closed {
            if segs > 0 {
                let c = sp.segment(segs - 1);
                let sign = if k.is_multiple_of(2) { 1.0 } else { -1.0 };
                let nrm = normal_at(&c, 1.0);
                pts.push(Ridge { p: c.p3 + nrm * (amp * sign), dir: Vec2::new(-nrm.y, nrm.x), gap_in: gap(segs - 1), gap_out: 0.0 });
            } else {
                pts.extend(sp.anchors.iter().map(|a| Ridge { p: a.p, dir: Vec2::ZERO, gap_in: 0.0, gap_out: 0.0 }));
            }
        }
        subs.push(if smooth {
            let anchors = pts.iter().map(|r| Anchor::with_handles(r.p, r.p - r.dir * (r.gap_in / 3.0), r.p + r.dir * (r.gap_out / 3.0))).collect();
            SubPath::new(anchors, sp.closed)
        } else {
            SubPath::polyline(&pts.iter().map(|r| r.p).collect::<Vec<_>>(), sp.closed)
        });
    }
    PathData::new(subs)
}

/// Smooth subpath through `pts` (Catmull-Rom tangents, kept at least half the longer chord where
/// the path turns back sharply so it makes a round hairpin instead of a cusp).
pub fn catmull_rom(pts: &[Point], closed: bool, tension: f64) -> SubPath {
    let n = pts.len();
    if n < 3 {
        return SubPath::polyline(pts, closed);
    }
    let anchors = (0..n)
        .map(|i| {
            let prev = if i == 0 { if closed { pts[n - 1] } else { pts[0] } } else { pts[i - 1] };
            let next = if i == n - 1 { if closed { pts[0] } else { pts[n - 1] } } else { pts[i + 1] };
            let t: Vec2 = turn_tangent(prev, pts[i], next) * (tension / 6.0);
            Anchor::with_handles(pts[i], pts[i] - t, pts[i] + t)
        })
        .collect();
    SubPath::new(anchors, closed)
}

fn turn_tangent(prev: Point, p: Point, next: Point) -> Vec2 {
    let t = next - prev;
    let (a, b) = (p - prev, next - p);
    let floor = 0.5 * a.hypot().max(b.hypot());
    if t.hypot() >= floor {
        return t;
    }
    let unit = |v: Vec2| {
        let l = v.hypot();
        if l > 1e-12 { v / l } else { Vec2::ZERO }
    };
    let (u, v) = (unit(a), unit(b));
    let mut d = unit(u + v);
    if d == Vec2::ZERO {
        d = Vec2::new(-u.y, u.x);
    }
    if d == Vec2::ZERO { t } else { d * floor }
}

/// Round corners: each corner between straight sides becomes a quarter-circle-like arc meeting
/// each side `radius` from the corner; corners with handles get a round cut of the same reach.
pub fn round_corners(path: &PathData, radius: f64) -> PathData {
    if radius <= 0.0 {
        return path.clone();
    }
    let mut subs = vec![];
    let mut with_handle = vec![];
    for sp in &path.subpaths {
        let n = sp.anchors.len();
        if n < 3 && (n != 2 || sp.closed) {
            with_handle.extend(sp.anchors.iter().map(|a| a.has_in() || a.has_out()));
            subs.push(sp.clone());
            continue;
        }
        let mut out: Vec<Anchor> = vec![];
        for i in 0..n {
            let a = sp.anchors[i];
            let interior = sp.closed || (i > 0 && i + 1 < n);
            if !interior || a.has_in() || a.has_out() {
                with_handle.push(a.has_in() || a.has_out());
                out.push(a);
                continue;
            }
            let prev = &sp.anchors[(i + n - 1) % n];
            let next = &sp.anchors[(i + 1) % n];
            let tp = if prev.has_out() { prev.h_out } else { prev.p };
            let tn = if next.has_in() { next.h_in } else { next.p };
            let (vp, vn) = (tp - a.p, tn - a.p);
            let (lp, ln) = (vp.hypot(), vn.hypot());
            if lp < 1e-9 || ln < 1e-9 {
                with_handle.push(false);
                out.push(a);
                continue;
            }
            let (up, un) = (vp / lp, vn / ln);
            if up.dot(un) < -0.9999 {
                with_handle.push(false);
                out.push(a);
                continue;
            }
            let d = radius.min(prev.p.distance(a.p) / 2.0).min(next.p.distance(a.p) / 2.0);
            let pa = a.p + up * d;
            let pb = a.p + un * d;
            let k = d * KAPPA;
            with_handle.extend([false, false]);
            out.push(Anchor::with_handles(pa, pa, pa - up * k));
            out.push(Anchor::with_handles(pb, pb - un * k, pb));
        }
        subs.push(SubPath::new(out, sp.closed));
    }
    let rounded = PathData::new(subs);
    let mut radii = vec![0.0; with_handle.len()];
    for c in corners::path_corners(&rounded) {
        if with_handle.get(c.index) == Some(&true)
            && let Some(r) = radii.get_mut(c.index)
        {
            *r = radius * c.sin() / (1.0 + c.cos());
        }
    }
    if radii.iter().all(|r| *r == 0.0) {
        return rounded;
    }
    let (mut out, sources) = corners::cut_corners(&rounded, &radii, &[]);
    for (sp, from) in out.subpaths.iter_mut().zip(&sources) {
        if sp.closed && sp.anchors.len() > 1 && from.first() == from.last() {
            sp.anchors.rotate_right(1);
        }
    }
    out
}
