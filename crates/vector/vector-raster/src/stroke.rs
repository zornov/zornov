// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Strokes become fills: the outline a stroke covers, built in the path's
//! own space so that a non-uniform transform shears the stroke as it should.

use kurbo::{BezPath, PathEl, Point, Vec2};

use crate::scene::{Cap, Join, StrokeStyle};

/// The area `style` covers along `path`, filled with the non-zero rule.
/// `tolerance` is the largest distance error allowed, in path units.
pub fn stroke_outline(path: &BezPath, style: &StrokeStyle, tolerance: f64) -> BezPath {
    if !(style.width > 0.0) || !style.width.is_finite() {
        return BezPath::new();
    }
    let tolerance = tolerance.max(1e-6);
    let dashes: Vec<f64> = if style.dashes.iter().all(|d| d.is_finite() && *d >= 0.0) && style.dashes.iter().sum::<f64>() > 1e-9 {
        // An odd count repeats once more, as SVG does.
        if style.dashes.len() % 2 == 1 { style.dashes.iter().chain(style.dashes.iter()).copied().collect() } else { style.dashes.clone() }
    } else {
        Vec::new()
    };
    if style.profile.is_empty() {
        let k = kurbo::Stroke::new(style.width)
            .with_join(match style.join {
                Join::Miter => kurbo::Join::Miter,
                Join::Round => kurbo::Join::Round,
                Join::Bevel => kurbo::Join::Bevel,
            })
            .with_miter_limit(style.miter_limit.max(1.0))
            .with_start_cap(cap(style.start_cap))
            .with_end_cap(cap(style.end_cap))
            .with_dashes(style.dash_offset, dashes.iter());
        return kurbo::stroke(path.iter(), &k, &kurbo::StrokeOpts::default(), tolerance);
    }
    variable_width(path, style, &dashes, tolerance)
}

fn cap(c: Cap) -> kurbo::Cap {
    match c {
        Cap::Butt => kurbo::Cap::Butt,
        Cap::Round => kurbo::Cap::Round,
        Cap::Square => kurbo::Cap::Square,
    }
}

/// One flattened subpath: points, whether it closes, and the cumulative
/// length at each point.
struct Poly {
    pts: Vec<Point>,
    closed: bool,
}

fn polylines(path: &BezPath, tolerance: f64) -> Vec<Poly> {
    let mut out: Vec<Poly> = Vec::new();
    let mut cur: Vec<Point> = Vec::new();
    let flush = |cur: &mut Vec<Point>, closed: bool, out: &mut Vec<Poly>| {
        cur.dedup_by(|a, b| a.distance(*b) < 1e-12);
        if closed && cur.len() > 2 && cur[0].distance(cur[cur.len() - 1]) < 1e-12 {
            cur.pop();
        }
        if cur.len() >= 2 || (cur.len() == 1 && !closed) {
            out.push(Poly { pts: std::mem::take(cur), closed });
        }
        cur.clear();
    };
    kurbo::flatten(path.iter(), tolerance, |el| match el {
        PathEl::MoveTo(p) => {
            flush(&mut cur, false, &mut out);
            cur.push(p);
        }
        PathEl::LineTo(p) => cur.push(p),
        PathEl::ClosePath => flush(&mut cur, true, &mut out),
        _ => {}
    });
    flush(&mut cur, false, &mut out);
    out
}

/// Pieces of a variable-width stroke, each wound the same way so the
/// non-zero rule unites them: a quad per segment plus joins and caps.
fn variable_width(path: &BezPath, style: &StrokeStyle, dashes: &[f64], tolerance: f64) -> BezPath {
    let polys = polylines(path, tolerance * 0.5);
    let total: f64 = polys.iter().map(|p| poly_len(p)).sum();
    if total <= 0.0 {
        return BezPath::new();
    }
    let mut out = BezPath::new();
    let mut base = 0.0;
    for poly in &polys {
        let len = poly_len(poly);
        // Runs of the polyline that are drawn (dashes), as length ranges.
        let runs = dash_runs(len, base, dashes, style.dash_offset);
        let whole = runs.len() == 1 && runs[0].0 <= 0.0 && runs[0].1 >= len;
        for (a, b) in runs {
            // Long straight runs are sampled finely enough for the profile's curve.
            let pts = densify(sub_poly(poly, a, b), (total / 256.0).max(tolerance * 4.0));
            if pts.is_empty() {
                continue;
            }
            let closed = poly.closed && whole;
            let ts: Vec<f64> = {
                let mut s = a;
                let mut v = vec![(base + s) / total];
                for w in pts.windows(2) {
                    s += w[0].distance(w[1]);
                    v.push((base + s) / total);
                }
                v
            };
            let r: Vec<f64> = ts.iter().map(|&t| 0.5 * style.width_at(t).max(0.0)).collect();
            emit_pieces(&mut out, &pts, &r, closed, style, tolerance);
        }
        base += len;
    }
    out
}

fn poly_len(p: &Poly) -> f64 {
    let mut l: f64 = p.pts.windows(2).map(|w| w[0].distance(w[1])).sum();
    if p.closed && p.pts.len() > 1 {
        l += p.pts[p.pts.len() - 1].distance(p.pts[0]);
    }
    l
}

/// The drawn length ranges of a subpath of length `len` starting `base`
/// into the whole path's dash sequence.
fn dash_runs(len: f64, base: f64, dashes: &[f64], offset: f64) -> Vec<(f64, f64)> {
    if dashes.is_empty() {
        return vec![(0.0, len)];
    }
    let period: f64 = dashes.iter().sum();
    let mut out = Vec::new();
    // Phase of position 0 of this subpath within the pattern. Like SVG,
    // each subpath restarts the pattern.
    let _ = base;
    let mut pos = -(offset.rem_euclid(period));
    let mut i = 0;
    while pos < len {
        let d = dashes[i % dashes.len()];
        if i % 2 == 0 {
            let (a, b) = (pos.max(0.0), (pos + d).min(len));
            if b > a {
                out.push((a, b));
            }
        }
        pos += d;
        i += 1;
        if i > 1_000_000 {
            break;
        }
    }
    out
}

/// The points of `poly` between lengths `a` and `b` (closed polylines
/// include the closing segment).
fn sub_poly(poly: &Poly, a: f64, b: f64) -> Vec<Point> {
    let mut pts = poly.pts.clone();
    if poly.closed {
        pts.push(pts[0]);
    }
    if pts.len() == 1 {
        return pts;
    }
    let mut out = Vec::new();
    let mut s = 0.0;
    for w in pts.windows(2) {
        let l = w[0].distance(w[1]);
        let (s0, s1) = (s, s + l);
        if s1 >= a && s0 <= b && l > 0.0 {
            let ta = ((a - s0) / l).clamp(0.0, 1.0);
            let tb = ((b - s0) / l).clamp(0.0, 1.0);
            let pa = w[0].lerp(w[1], ta);
            if out.last().is_none_or(|q: &Point| q.distance(pa) > 1e-12) {
                out.push(pa);
            }
            out.push(w[0].lerp(w[1], tb));
        }
        s = s1;
    }
    if poly.closed && a <= 0.0 && out.len() > 1 && out[0].distance(out[out.len() - 1]) < 1e-9 {
        out.pop();
    }
    out
}

/// Splits segments longer than `step` into equal parts.
fn densify(pts: Vec<Point>, step: f64) -> Vec<Point> {
    if pts.len() < 2 || !(step > 0.0) {
        return pts;
    }
    let mut out = vec![pts[0]];
    for w in pts.windows(2) {
        let n = ((w[0].distance(w[1]) / step).ceil() as usize).clamp(1, 10_000);
        for k in 1..=n {
            out.push(w[0].lerp(w[1], k as f64 / n as f64));
        }
    }
    out
}

fn push_poly(out: &mut BezPath, pts: &[Point]) {
    if pts.len() < 3 {
        return;
    }
    let area: f64 = (0..pts.len()).map(|i| pts[i].to_vec2().cross(pts[(i + 1) % pts.len()].to_vec2())).sum();
    if area.abs() < 1e-14 {
        return;
    }
    let it: Box<dyn Iterator<Item = &Point>> = if area > 0.0 { Box::new(pts.iter()) } else { Box::new(pts.iter().rev()) };
    let mut first = true;
    for &p in it {
        if first {
            out.move_to(p);
            first = false;
        } else {
            out.line_to(p);
        }
    }
    out.close_path();
}

fn circle(out: &mut BezPath, c: Point, r: f64, tolerance: f64) {
    if r <= 0.0 {
        return;
    }
    let n = segments_for_arc(r, std::f64::consts::TAU, tolerance);
    let pts: Vec<Point> = (0..n).map(|i| c + Vec2::from_angle(i as f64 / n as f64 * std::f64::consts::TAU) * r).collect();
    push_poly(out, &pts);
}

fn segments_for_arc(r: f64, sweep: f64, tolerance: f64) -> usize {
    // Chord error r(1 - cos(θ/2)) ≤ tolerance.
    let step = 2.0 * (1.0 - (tolerance / r).min(1.0)).acos();
    ((sweep / step.max(1e-3)).ceil() as usize).clamp(8, 4096)
}

fn normal(a: Point, b: Point) -> Vec2 {
    let d = b - a;
    let l = d.hypot();
    if l < 1e-15 { Vec2::ZERO } else { Vec2::new(-d.y / l, d.x / l) }
}

fn emit_pieces(out: &mut BezPath, pts: &[Point], r: &[f64], closed: bool, style: &StrokeStyle, tol: f64) {
    let n = pts.len();
    if n == 1 {
        // A zero-length dash or subpath: round caps draw a dot, square caps a square.
        match style.start_cap {
            Cap::Round => circle(out, pts[0], r[0], tol),
            Cap::Square => {
                let p = pts[0];
                let q = r[0];
                push_poly(out, &[p + Vec2::new(-q, -q), p + Vec2::new(q, -q), p + Vec2::new(q, q), p + Vec2::new(-q, q)]);
            }
            Cap::Butt => {}
        }
        return;
    }
    let seg_count = if closed { n } else { n - 1 };
    for i in 0..seg_count {
        let j = (i + 1) % n;
        let (a, b) = (pts[i], pts[j]);
        let nv = normal(a, b);
        if nv == Vec2::ZERO || (r[i] <= 0.0 && r[j] <= 0.0) {
            continue;
        }
        push_poly(out, &[a + nv * r[i], b + nv * r[j], b - nv * r[j], a - nv * r[i]]);
    }
    // Joins at interior vertices (and all vertices when closed).
    let (first, last) = if closed { (0, n) } else { (1, n - 1) };
    for i in first..last {
        let prev = pts[(i + n - 1) % n];
        let next = pts[(i + 1) % n];
        let p = pts[i];
        let (n0, n1) = (normal(prev, p), normal(p, next));
        if n0 == Vec2::ZERO || n1 == Vec2::ZERO || r[i] <= 0.0 {
            continue;
        }
        let turn = (p - prev).cross(next - p);
        if turn.abs() < 1e-14 && n0.dot(n1) > 0.0 {
            continue;
        }
        // The outer side of the turn is opposite to its direction.
        let s = if turn > 0.0 { -1.0 } else { 1.0 };
        let (o0, o1) = (p + n0 * (r[i] * s), p + n1 * (r[i] * s));
        let cos = n0.dot(n1).clamp(-1.0, 1.0);
        let sharp = cos < 0.9999;
        match style.join {
            Join::Round if sharp => circle(out, p, r[i], tol),
            Join::Miter if sharp => {
                // Miter length ratio 1 / sin(θ/2) where θ is the angle between the segments.
                let half = ((1.0 + cos) * 0.5).sqrt();
                let ratio = if half > 1e-9 { 1.0 / half } else { f64::INFINITY };
                if ratio <= style.miter_limit.max(1.0) {
                    let bis = (n0 + n1).normalize() * s;
                    push_poly(out, &[p, o0, p + bis * (r[i] * ratio), o1]);
                } else {
                    push_poly(out, &[p, o0, o1]);
                }
            }
            _ => push_poly(out, &[p, o0, o1]),
        }
    }
    if !closed {
        let ends = [(pts[0], pts[1], r[0], style.start_cap), (pts[n - 1], pts[n - 2], r[n - 1], style.end_cap)];
        for (p, toward, rad, c) in ends {
            match c {
                Cap::Butt => {}
                Cap::Round => circle(out, p, rad, tol),
                Cap::Square => {
                    let d = (p - toward).normalize();
                    let nv = Vec2::new(-d.y, d.x);
                    push_poly(out, &[p + nv * rad, p + nv * rad + d * rad, p - nv * rad + d * rad, p - nv * rad]);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::WidthPoint;
    use kurbo::Shape;

    fn area(p: &BezPath) -> f64 {
        p.area()
    }

    #[test]
    fn plain_stroke_area_matches_width_times_length() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((100.0, 0.0));
        let o = stroke_outline(&p, &StrokeStyle::new(10.0), 0.01);
        assert!((area(&o).abs() - 1000.0).abs() < 1e-6, "{}", area(&o));
    }

    #[test]
    fn dashed_stroke_draws_half() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((100.0, 0.0));
        let o = stroke_outline(&p, &StrokeStyle::new(2.0).with_dashes(vec![10.0, 10.0], 0.0), 0.01);
        assert!((area(&o).abs() - 100.0).abs() < 1e-6, "{}", area(&o));
    }

    #[test]
    fn tapered_stroke_is_a_triangle() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((100.0, 0.0));
        let style = StrokeStyle::new(10.0).with_profile(vec![WidthPoint { at: 0.0, width: 1.0 }, WidthPoint { at: 1.0, width: 1.0 }]);
        let o = stroke_outline(&p, &style, 0.01);
        // Even profile, butt caps: the same 100 × 10 rectangle.
        assert!((area(&o).abs() - 1000.0).abs() < 1e-6, "{}", area(&o));
        assert!(o.winding(Point::new(50.0, 4.0)) != 0);
        assert!(o.winding(Point::new(50.0, 6.0)) == 0);
    }
}
