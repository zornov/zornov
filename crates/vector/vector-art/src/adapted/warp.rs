//! Adapted from VectorCraft (<https://github.com/storytold/vectorcraft>, revision 8bf090b):
//! `crates/doc/src/live.rs` (warp maps, non-linear path mapping, Coons boundaries) and
//! `crates/effects/src/warp.rs`. Copyright (c) 2026 ArtCraft Team and the VectorCraft
//! contributors; MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE` in this folder).
//! Changes: typed parameters instead of JSON values, the style enum is Studio's.

use std::f64::consts::PI;

use studio_vector_geom::{Anchor, BezPath, CubicBez, PathData, Point, Rect, SubPath};

use crate::doc::WarpKind;

/// The warp map on normalised coordinates: `b` = bend (-1..1), `dh`/`dv` = distortion (-1..1);
/// `aspect` is the box's half-height over its half-width along the warp (Arc bends in real
/// proportions).
pub fn warp_point(style: WarpKind, b: f64, dh: f64, dv: f64, aspect: f64, x: f64, y: f64) -> (f64, f64) {
    use WarpKind::*;
    let t = (y + 1.0) / 2.0; // 0 at top, 1 at bottom
    let par = 1.0 - x * x; // parabola: 1 at centre, 0 at sides
    let (mut x2, mut y2) = match style {
        Arc => {
            if b.abs() < 1e-6 {
                (x, y)
            } else {
                // Bend the centre line into a circular arc of equal length (sweep = |b|·180°), in
                // the box's real proportions: the edges are concentric arcs, and one that would
                // pass the centre stops there instead of folding over.
                let k = if aspect.is_finite() && aspect > 0.0 { aspect } else { 1.0 };
                let sweep = b.abs() * PI;
                let r0 = 2.0 / sweep;
                let yy = if b > 0.0 { y } else { -y } * k;
                let a = x / r0;
                let r = (r0 - yy).max(0.0);
                let (s, c) = a.sin_cos();
                let (nx, ny) = (r * s, (r0 - r * c) / k);
                (nx, if b > 0.0 { ny } else { -ny })
            }
        }
        ArcLower => (x, y + b * par * t),
        ArcUpper => (x, y - b * par * (1.0 - t)),
        Arch => (x, y - b * par),
        Bulge => (x, y + b * par * y),
        ShellLower => (x * (1.0 - 0.5 * b * (1.0 - t)), y + b * par * t),
        ShellUpper => (x * (1.0 - 0.5 * b * t), y - b * par * (1.0 - t)),
        Flag => (x, y + 0.5 * b * (PI * x).sin()),
        Wave => (x + 0.1 * b * (PI * y).sin(), y + 0.3 * b * (2.0 * PI * x).sin() * (0.5 + 0.5 * t)),
        Fish => (x, y * (1.0 + 0.5 * b * (0.75 * PI * (x + 1.0)).sin())),
        Rise => (x, y - b * ((x + 1.0) / 2.0).powi(2) * 2.0 + b),
        Fisheye => {
            let r2 = x * x + y * y;
            let s = if r2 < 1.0 { 1.0 + 0.5 * b * (1.0 - r2) } else { 1.0 };
            (x * s, y * s)
        }
        Inflate => (x * (1.0 + 0.5 * b * (1.0 - y * y)), y * (1.0 + 0.5 * b * par)),
        Squeeze => (x * (1.0 - 0.5 * b * (1.0 - y * y)), y * (1.0 + 0.3 * b * par)),
        Twist => {
            let r = (x * x + y * y).sqrt();
            let a = -b * PI * 0.5 * (1.0 - r / std::f64::consts::SQRT_2).max(0.0);
            let (s, c) = a.sin_cos();
            (x * c - y * s, x * s + y * c)
        }
    };
    // Perspective-like distortion: horizontal narrows one side, vertical one end.
    if dh != 0.0 {
        y2 *= (1.0 + dh * x2 * 0.5).max(0.0);
    }
    if dv != 0.0 {
        x2 *= (1.0 + dv * y2 * 0.5).max(0.0);
    }
    (x2, y2)
}

/// Applies a warp style to `path` within box `b`.
pub fn warp(path: &PathData, b: Rect, style: WarpKind, bend: f64, dh: f64, dv: f64, vertical: bool, max: usize) -> PathData {
    let bend = bend.clamp(-1.0, 1.0);
    let (dh, dv) = (dh.clamp(-1.0, 1.0), dv.clamp(-1.0, 1.0));
    let c = b.center();
    let hw = (b.width() / 2.0).max(1e-9);
    let hh = (b.height() / 2.0).max(1e-9);
    let f = |q: Point| {
        let (x, y) = ((q.x - c.x) / hw, (q.y - c.y) / hh);
        let (x2, y2) = if vertical {
            let (a, b2) = warp_point(style, bend, dh, dv, hw / hh, y, x);
            (b2, a)
        } else {
            warp_point(style, bend, dh, dv, hh / hw, x, y)
        };
        Point::new(c.x + x2 * hw, c.y + y2 * hh)
    };
    let piece = b.width().hypot(b.height()) / 24.0;
    map_nonlinear_within(path, piece, max, f)
}

/// Segment as a cubic; straight lines get handles at 1/3 and 2/3 so a non-linear map bends them.
pub fn seg_cubic(sp: &SubPath, i: usize) -> CubicBez {
    let c = sp.segment(i);
    if sp.segment_is_line(i) { CubicBez::new(c.p0, c.p0.lerp(c.p3, 1.0 / 3.0), c.p0.lerp(c.p3, 2.0 / 3.0), c.p3) } else { c }
}

fn poly_len(c: &CubicBez) -> f64 {
    c.p0.distance(c.p1) + c.p1.distance(c.p2) + c.p2.distance(c.p3)
}

fn cumulative_lengths(pts: &[Point]) -> Vec<f64> {
    let mut total = 0.0;
    let mut cum = Vec::with_capacity(pts.len());
    cum.push(0.0);
    for w in pts.windows(2) {
        total += w[0].distance(w[1]);
        cum.push(total);
    }
    cum
}

/// Most pieces [`map_nonlinear_within`] splits one segment into.
const MAX_PIECES: usize = 64;

/// How many pieces of about `piece` a segment of length `len` is split into: one to `most`.
pub fn pieces(len: f64, piece: f64, most: usize) -> usize {
    ((len / piece).ceil() as usize).clamp(1, most.max(1))
}

/// The piece length to split segments of lengths `lens` with so that the pieces past each
/// segment's first number at most `room`.
pub fn fit_piece(lens: &[f64], piece: f64, most: usize, room: usize) -> f64 {
    let added = lens.iter().map(|&len| pieces(len, piece, most) - 1).fold(0, usize::saturating_add);
    if added <= room {
        return piece;
    }
    if room == 0 {
        return f64::INFINITY;
    }
    piece.max(lens.iter().filter(|len| !len.is_nan()).sum::<f64>() / room as f64)
}

/// Maps every point of `path` through the non-linear function `f`: segments are split into
/// pieces no longer than about `max_piece` and their control points mapped (an O(h²)
/// approximation of the image curve), with at most `max` anchors in the result.
pub fn map_nonlinear_within(path: &PathData, max_piece: f64, max: usize, f: impl Fn(Point) -> Point) -> PathData {
    let mut max_piece = max_piece.max(1e-3);
    let room = max.saturating_sub(path.anchor_count());
    let segs = path.subpaths.iter().map(SubPath::segment_count).fold(0, usize::saturating_add);
    if segs.saturating_mul(MAX_PIECES - 1) > room {
        let lens: Vec<f64> = path.subpaths.iter().flat_map(|sp| (0..sp.segment_count()).map(|i| poly_len(&seg_cubic(sp, i)))).collect();
        max_piece = fit_piece(&lens, max_piece, MAX_PIECES, room);
    }
    let mut subs = Vec::with_capacity(path.subpaths.len());
    for sp in &path.subpaths {
        let n = sp.anchors.len();
        if n == 0 {
            continue;
        }
        let segs = sp.segment_count();
        let mut res: Vec<Anchor> = Vec::new();
        let mut pending_in: Option<Point> = None;
        for i in 0..n {
            let a = &sp.anchors[i];
            let h_in = pending_in.take().unwrap_or_else(|| f(a.h_in));
            res.push(Anchor { p: f(a.p), h_in, h_out: f(a.h_out), kind: a.kind });
            if i < segs {
                let c = seg_cubic(sp, i);
                let k = pieces(poly_len(&c), max_piece, MAX_PIECES);
                for j in 0..k {
                    let sub = kurbo::ParamCurve::subsegment(&c, (j as f64 / k as f64)..((j + 1) as f64 / k as f64));
                    if let Some(last) = res.last_mut() {
                        last.h_out = f(sub.p1);
                    }
                    if j + 1 < k {
                        res.push(Anchor { p: f(sub.p3), h_in: f(sub.p2), h_out: f(sub.p3), kind: Default::default() });
                    } else {
                        pending_in = Some(f(sub.p2));
                    }
                }
            }
        }
        if let Some(h) = pending_in
            && sp.closed
            && let Some(first) = res.first_mut()
        {
            first.h_in = h;
        }
        let anchors = res.into_iter().map(|a| Anchor::with_handles(a.p, a.h_in, a.h_out)).collect();
        subs.push(SubPath::new(anchors, sp.closed));
    }
    PathData::new(subs)
}

fn flatten_subpath(sp: &SubPath, tol: f64) -> Vec<Point> {
    let mut bp = BezPath::new();
    sp.to_bezpath_into(&mut bp);
    let mut pts: Vec<Point> = Vec::new();
    kurbo::flatten(bp.iter(), tol, |el| match el {
        kurbo::PathEl::MoveTo(p) | kurbo::PathEl::LineTo(p) if pts.last().is_none_or(|q| q.distance(p) > 1e-9) => pts.push(p),
        _ => {}
    });
    pts
}

fn resample(poly: &[Point], m: usize) -> Vec<Point> {
    if poly.is_empty() {
        return vec![Point::ZERO; m];
    }
    if poly.len() == 1 {
        return vec![poly[0]; m];
    }
    let cum = cumulative_lengths(poly);
    let total = cum.last().copied().unwrap_or(0.0);
    let mut out = Vec::with_capacity(m);
    let mut i = 0;
    for k in 0..m {
        let s = if m > 1 { total * k as f64 / (m - 1) as f64 } else { 0.0 };
        while i + 2 < cum.len() && cum[i + 1] < s {
            i += 1;
        }
        let seg = (cum[i + 1] - cum[i]).max(1e-12);
        out.push(poly[i].lerp(poly[i + 1], ((s - cum[i]) / seg).clamp(0.0, 1.0)));
    }
    out
}

const SIDE: usize = 65;

/// Four boundary curves (top TL→TR, right TR→BR, bottom BL→BR, left TL→BL), each resampled,
/// from the outline of a closed path split at the points nearest its bounding box corners.
#[derive(Clone, Debug)]
pub struct Coons {
    top: Vec<Point>,
    right: Vec<Point>,
    bottom: Vec<Point>,
    left: Vec<Point>,
}

impl Coons {
    pub fn rect(r: Rect) -> Self {
        let line = |a: Point, b: Point| (0..SIDE).map(|i| a.lerp(b, i as f64 / (SIDE - 1) as f64)).collect::<Vec<_>>();
        let (tl, tr, br, bl) = (Point::new(r.x0, r.y0), Point::new(r.x1, r.y0), Point::new(r.x1, r.y1), Point::new(r.x0, r.y1));
        Self { top: line(tl, tr), right: line(tr, br), bottom: line(bl, br), left: line(tl, bl) }
    }

    pub fn from_path(path: &PathData) -> Option<Self> {
        let sp = path.subpaths.iter().filter(|s| s.anchors.len() >= 2).max_by(|a, b| a.area().abs().total_cmp(&b.area().abs()))?;
        let r = path.bounds()?;
        let tol = (r.width() + r.height()).max(1e-6) / 2000.0;
        let mut poly = flatten_subpath(sp, tol);
        if poly.len() > 1
            && let (Some(a), Some(b)) = (poly.first(), poly.last())
            && a.distance(*b) < 1e-9
        {
            poly.pop();
        }
        if poly.len() < 3 {
            return Some(Self::rect(r));
        }
        let area: f64 = (0..poly.len())
            .map(|i| {
                let (p, q) = (poly[i], poly[(i + 1) % poly.len()]);
                p.x * q.y - q.x * p.y
            })
            .sum();
        if area < 0.0 {
            poly.reverse();
        }
        let nearest = |c: Point| (0..poly.len()).min_by(|i, j| poly[*i].distance_squared(c).total_cmp(&poly[*j].distance_squared(c))).unwrap_or(0);
        let tl = nearest(Point::new(r.x0, r.y0));
        let tr = nearest(Point::new(r.x1, r.y0));
        let br = nearest(Point::new(r.x1, r.y1));
        let bl = nearest(Point::new(r.x0, r.y1));
        let n = poly.len();
        let arc = |i: usize, j: usize| -> Vec<Point> {
            let mut v = vec![poly[i]];
            let mut k = i;
            while k != j {
                k = (k + 1) % n;
                v.push(poly[k]);
            }
            v
        };
        let top = resample(&arc(tl, tr), SIDE);
        let right = resample(&arc(tr, br), SIDE);
        let mut bottom = resample(&arc(br, bl), SIDE);
        bottom.reverse();
        let mut left = resample(&arc(bl, tl), SIDE);
        left.reverse();
        Some(Self { top, right, bottom, left })
    }

    fn sample(side: &[Point], u: f64) -> Point {
        let s = u * (side.len() - 1) as f64;
        let i = (s.floor().max(0.0) as usize).min(side.len() - 2);
        side[i].lerp(side[i + 1], s - i as f64)
    }

    /// The surface point at (u, v) ∈ [0,1]².
    pub fn eval(&self, u: f64, v: f64) -> Point {
        let (t, b, l, r) = (Self::sample(&self.top, u), Self::sample(&self.bottom, u), Self::sample(&self.left, v), Self::sample(&self.right, v));
        let (p00, p10, p01, p11) = (self.top[0], self.top[SIDE - 1], self.bottom[0], self.bottom[SIDE - 1]);
        let c = (1.0 - v) * t.to_vec2() + v * b.to_vec2() + (1.0 - u) * l.to_vec2() + u * r.to_vec2()
            - ((1.0 - u) * (1.0 - v) * p00.to_vec2() + u * (1.0 - v) * p10.to_vec2() + (1.0 - u) * v * p01.to_vec2() + u * v * p11.to_vec2());
        c.to_point()
    }
}
