//! Test helpers vendored from VectorCraft (<https://github.com/storytold/vectorcraft>, crates/dev/testkit
//! src/geom.rs and the path strategies of src/strategies.rs, revision 8bf090b); MIT OR Apache-2.0.
#![allow(dead_code)]

pub mod geom {
//! Geometry assertions.

use kurbo::{ParamCurveNearest, Shape};
use studio_vector_geom::{BezPath, ParamCurve, PathData, Point, Rect};

pub fn approx(a: f64, b: f64, eps: f64) -> bool {
    (a - b).abs() <= eps
}

/// Relative closeness with an absolute floor of 1.
pub fn rel_close(a: f64, b: f64, rel: f64) -> bool {
    (a - b).abs() <= rel * a.abs().max(b.abs()).max(1.0)
}

#[track_caller]
pub fn assert_approx(a: f64, b: f64, eps: f64) {
    assert!(approx(a, b, eps), "{a} != {b} (±{eps})");
}

#[track_caller]
pub fn assert_rect_approx(a: Rect, b: Rect, eps: f64) {
    assert!(approx(a.x0, b.x0, eps) && approx(a.y0, b.y0, eps) && approx(a.x1, b.x1, eps) && approx(a.y1, b.y1, eps), "{a:?} != {b:?} (±{eps})");
}

/// Does `outer` contain `inner` (with slack `eps`)?
pub fn rect_contains(outer: Rect, inner: Rect, eps: f64) -> bool {
    inner.x0 >= outer.x0 - eps && inner.y0 >= outer.y0 - eps && inner.x1 <= outer.x1 + eps && inner.y1 <= outer.y1 + eps
}

/// `n` evenly spaced parameter samples on every segment (endpoints included).
pub fn sample(bp: &BezPath, n: usize) -> Vec<Point> {
    let mut v = vec![];
    for seg in bp.segments() {
        for i in 0..=n {
            v.push(seg.eval(i as f64 / n as f64));
        }
    }
    v
}

/// Sample a `PathData`.
pub fn sample_path(p: &PathData, n: usize) -> Vec<Point> {
    sample(&p.to_bezpath(), n)
}

/// Distance from `p` to the nearest point of `bp`'s outline.
pub fn distance_to(bp: &BezPath, p: Point) -> f64 {
    bp.segments().map(|s| s.nearest(p, 1e-9).distance_sq).fold(f64::INFINITY, f64::min).sqrt()
}

/// One-sided Hausdorff distance: the largest distance from a sample of `a` to the outline of `b`.
pub fn directed_hausdorff(a: &BezPath, b: &BezPath, n: usize) -> f64 {
    sample(a, n).into_iter().map(|p| distance_to(b, p)).fold(0.0, f64::max)
}

/// Symmetric Hausdorff distance (sampled).
pub fn hausdorff(a: &BezPath, b: &BezPath, n: usize) -> f64 {
    directed_hausdorff(a, b, n).max(directed_hausdorff(b, a, n))
}

/// Assert that two paths trace the same outline within `tol`.
#[track_caller]
pub fn assert_paths_close(a: &PathData, b: &PathData, tol: f64) {
    let d = hausdorff(&a.to_bezpath(), &b.to_bezpath(), 16);
    assert!(d <= tol, "paths differ by {d} > {tol}");
}

/// Shoelace area of a polygon (absolute).
pub fn polygon_area(pts: &[Point]) -> f64 {
    let n = pts.len();
    (0..n).map(|i| pts[i].to_vec2().cross(pts[(i + 1) % n].to_vec2())).sum::<f64>().abs() / 2.0
}

/// Signed area enclosed by a path (kurbo's Green's theorem area, all subpaths).
pub fn signed_area(p: &PathData) -> f64 {
    p.to_bezpath().area()
}

/// Is every point finite?
pub fn all_finite(p: &PathData) -> bool {
    p.subpaths.iter().flat_map(|s| &s.anchors).all(|a| [a.p, a.h_in, a.h_out].iter().all(|q| q.x.is_finite() && q.y.is_finite()))
}

/// A grid of probe points over `r` (`n`×`n`, cell centres).
pub fn grid(r: Rect, n: usize) -> Vec<Point> {
    let mut v = vec![];
    for i in 0..n {
        for j in 0..n {
            v.push(Point::new(r.x0 + (i as f64 + 0.5) / n as f64 * r.width(), r.y0 + (j as f64 + 0.5) / n as f64 * r.height()));
        }
    }
    v
}
}

pub mod strategies {
use proptest::prelude::*;
use studio_vector_geom::{Anchor, PathData, Point, Rect, SubPath};


// ---------------------------------------------------------------- geometry

pub fn arb_point(lo: f64, hi: f64) -> impl Strategy<Value = Point> {
    (lo..hi, lo..hi).prop_map(|(x, y)| Point::new(x, y))
}

/// A non-degenerate rectangle inside `[0, 200]²`.
pub fn arb_rect() -> impl Strategy<Value = Rect> {
    (0.0..150.0f64, 0.0..150.0f64, 2.0..80.0f64, 2.0..80.0f64).prop_map(|(x, y, w, h)| Rect::new(x, y, x + w, y + h))
}

/// A simple (non self-intersecting) star-shaped polygon: points at increasing angles around a
/// centre with random radii. Every angular step stays below half a turn: a wider one puts the
/// centre outside and can make the polygon cross itself.
pub fn arb_star_polygon() -> impl Strategy<Value = Vec<Point>> {
    let steps = prop::collection::vec((0.2f64..1.0, 5.0f64..40.0), 3..12).prop_filter("an angular step of half a turn or more", |v| {
        let total: f64 = v.iter().map(|(a, _)| a).sum();
        v.iter().all(|(a, _)| 2.0 * a < total)
    });
    (arb_point(40.0, 160.0), steps).prop_map(|(c, v)| {
        let total: f64 = v.iter().map(|(a, _)| a).sum();
        let mut ang = 0.0;
        v.iter()
            .map(|&(a, r)| {
                ang += a / total * std::f64::consts::TAU;
                Point::new(c.x + r * ang.cos(), c.y + r * ang.sin())
            })
            .collect()
    })
}

/// A convex polygon (regular polygon with jittered radius kept convex by using a single radius per
/// polygon and jittered angles only).
pub fn arb_convex_polygon() -> impl Strategy<Value = Vec<Point>> {
    (arb_point(40.0, 160.0), 5.0f64..40.0, prop::collection::vec(0.3f64..1.0, 3..10)).prop_map(|(c, r, v)| {
        let total: f64 = v.iter().sum();
        let mut ang = 0.0;
        v.iter()
            .map(|a| {
                ang += a / total * std::f64::consts::TAU;
                Point::new(c.x + r * ang.cos(), c.y + r * ang.sin())
            })
            .collect()
    })
}

pub fn polygon_path(pts: &[Point]) -> PathData {
    PathData::single(SubPath::polyline(pts, true))
}

/// An axis-aligned ellipse as a path.
pub fn arb_ellipse() -> impl Strategy<Value = PathData> {
    arb_rect().prop_map(studio_vector_geom::shapes::ellipse)
}

/// Rectangles, ellipses and star-shaped polygons.
pub fn arb_closed_shape() -> impl Strategy<Value = PathData> {
    prop_oneof![arb_rect().prop_map(studio_vector_geom::shapes::rectangle), arb_ellipse(), arb_star_polygon().prop_map(|p| polygon_path(&p)),]
}

/// A random cubic Bézier path (open or closed, possibly several subpaths, arbitrary handles).
pub fn arb_path_data() -> impl Strategy<Value = PathData> {
    let anchor = (arb_point(0.0, 200.0), arb_point(-30.0, 30.0), arb_point(-30.0, 30.0), any::<bool>())
        .prop_map(|(p, i, o, corner)| if corner { Anchor::corner(p) } else { Anchor::with_handles(p, p + i.to_vec2(), p + o.to_vec2()) });
    let sub = (prop::collection::vec(anchor, 2..8), any::<bool>()).prop_map(|(a, closed)| SubPath::new(a, closed));
    prop::collection::vec(sub, 1..3).prop_map(PathData::new)
}

/// A smooth open curve through points on a gently varying function (for simplify tests): densely
/// sampled polyline-like cubic chain with C1 handles.
pub fn arb_smooth_curve() -> impl Strategy<Value = PathData> {
    (prop::collection::vec(-20.0f64..20.0, 4..9), 10.0f64..30.0).prop_map(|(ys, step)| {
        // Catmull-Rom through (i*step, y_i), converted to cubic handles.
        let pts: Vec<Point> = ys.iter().enumerate().map(|(i, y)| Point::new(i as f64 * step, 100.0 + y)).collect();
        let n = pts.len();
        let tangent = |i: usize| {
            let a = pts[i.saturating_sub(1)];
            let b = pts[(i + 1).min(n - 1)];
            (b - a) / 6.0
        };
        let anchors = (0..n).map(|i| Anchor::with_handles(pts[i], pts[i] - tangent(i), pts[i] + tangent(i))).collect();
        PathData::single(SubPath::new(anchors, false))
    })
}
}
