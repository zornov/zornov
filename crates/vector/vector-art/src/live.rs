// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Live constructions, recomputed whenever they are drawn: vector effects
//! on outlines, repeats, blends between shapes, envelopes and the
//! perspective grid.

use kurbo::{Affine, Point, Rect};
use studio_vector_geom::{Homography, PathData};
use studio_vector_raster::{Color, Paint};

use crate::adapted::{distort, warp};
use crate::appearance::{Effect, Item};
use crate::doc::{BlendSpec, Envelope, Node, NodeKind, Perspective, Plane, Repeat};

/// Most anchors a live effect makes of one path.
pub const MAX_ANCHORS: usize = 200_000;

fn to_geom_join(j: studio_vector_raster::Join) -> studio_vector_pathops::Join {
    match j {
        studio_vector_raster::Join::Miter => studio_vector_pathops::Join::Miter,
        studio_vector_raster::Join::Round => studio_vector_pathops::Join::Round,
        studio_vector_raster::Join::Bevel => studio_vector_pathops::Join::Bevel,
    }
}

/// Applies the vector effects in `effects` (raster ones are skipped) to `path`.
pub fn apply_effects(path: &PathData, effects: &[Effect]) -> PathData {
    let mut p = path.clone();
    for e in effects {
        let b = p.bounds().unwrap_or(Rect::ZERO);
        p = match e {
            Effect::Offset { amount, join } => studio_vector_pathops::offset_path(&p, *amount, to_geom_join(*join), 4.0),
            Effect::Roughen { size, detail, smooth, seed } => distort::roughen(&p, *size, *detail, *smooth, *seed, MAX_ANCHORS),
            Effect::PuckerBloat { amount } => distort::pucker_bloat(&p, b, *amount),
            Effect::ZigZag { size, ridges, smooth } => distort::zig_zag(&p, *size, *ridges, *smooth, MAX_ANCHORS),
            Effect::Twist { angle } => distort::twist(&p, b, *angle, MAX_ANCHORS),
            Effect::RoundCorners { radius } => distort::round_corners(&p, *radius),
            Effect::Transform { copies, dx, dy, rotate, scale } => {
                let c = b.center();
                let m = Affine::translate((*dx, *dy)) * Affine::translate(c.to_vec2()) * Affine::rotate(rotate.to_radians()) * Affine::scale(*scale) * Affine::translate(-c.to_vec2());
                let mut out = p.clone();
                let mut cur = p.clone();
                for _ in 0..(*copies).min(1000) {
                    cur = cur.transformed(m);
                    if out.anchor_count() + cur.anchor_count() > MAX_ANCHORS {
                        break;
                    }
                    out.subpaths.extend(cur.subpaths.iter().cloned());
                }
                if *copies == 0 { p.transformed(m) } else { out }
            }
            Effect::Warp { style, bend, horizontal, vertical } => warp::warp(&p, b, *style, *bend, *horizontal, *vertical, false, MAX_ANCHORS),
            _ => p,
        };
    }
    p
}

/// The copies a repeat makes: (transform applied to the children, in the
/// repeat's space).
pub fn repeat_transforms(r: &Repeat, bounds: Rect) -> Vec<Affine> {
    match r {
        Repeat::Radial { count, center, radius, rotate } => {
            let n = (*count).clamp(1, 1000);
            let c = center.unwrap_or_else(|| Point::new(bounds.center().x, bounds.y1 + radius.max(0.0) + bounds.height()));
            (0..n)
                .map(|i| {
                    let a = i as f64 / n as f64 * std::f64::consts::TAU;
                    let push = Affine::translate((0.0, -*radius));
                    if *rotate {
                        Affine::translate(c.to_vec2()) * Affine::rotate(a) * Affine::translate(-c.to_vec2()) * push
                    } else {
                        // Upright copies on the circle through the art's centre.
                        let art = bounds.center() - c;
                        let rot = (Affine::rotate(a) * art.to_point()).to_vec2();
                        Affine::translate(rot - art) * push
                    }
                })
                .collect()
        }
        Repeat::Grid { rows, cols, gap, offset_rows } => {
            let (w, h) = (bounds.width() + gap.0, bounds.height() + gap.1);
            let mut out = Vec::new();
            for r in 0..(*rows).clamp(1, 500) {
                for c in 0..(*cols).clamp(1, 500) {
                    let shift = if *offset_rows && r % 2 == 1 { w / 2.0 } else { 0.0 };
                    out.push(Affine::translate((c as f64 * w + shift, r as f64 * h)));
                }
            }
            out
        }
        Repeat::Mirror { at, angle } => {
            let c = at.unwrap_or(Point::new(bounds.x1, bounds.center().y));
            let a = angle.to_radians();
            // Reflection across the line through c at angle a from vertical.
            let axis = Affine::translate(c.to_vec2()) * Affine::rotate(a) * Affine::scale_non_uniform(-1.0, 1.0) * Affine::rotate(-a) * Affine::translate(-c.to_vec2());
            vec![Affine::IDENTITY, axis]
        }
    }
}

fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    Color::rgba(a.r + (b.r - a.r) * t, a.g + (b.g - a.g) * t, a.b + (b.b - a.b) * t, a.a + (b.a - a.a) * t)
}

/// Steps between two leaf nodes: geometry morphs, solid fills and strokes
/// and their widths mix, opacity mixes. The two end nodes are included.
pub fn blend_steps(a: &Node, b: &Node, spec: &BlendSpec, geom: &dyn Fn(&Node) -> Option<PathData>) -> Vec<Node> {
    let (Some(pa), Some(pb)) = (geom(a), geom(b)) else { return vec![a.clone(), b.clone()] };
    let pa = pa.transformed(a.transform);
    let pb = pb.transformed(b.transform);
    let steps = if spec.steps == 0 {
        // Smooth colour: enough steps that neighbours differ by under 1%.
        let fa = a.appearance.top_fill().and_then(|f| match &f.paint {
            Paint::Solid { color } => Some(*color),
            _ => None,
        });
        let fb = b.appearance.top_fill().and_then(|f| match &f.paint {
            Paint::Solid { color } => Some(*color),
            _ => None,
        });
        let dc = match (fa, fb) {
            (Some(x), Some(y)) => ((x.r - y.r).abs().max((x.g - y.g).abs()).max((x.b - y.b).abs()) * 255.0) as u32,
            _ => 0,
        };
        let dist = pa.bounds().zip(pb.bounds()).map(|(x, y)| x.center().distance(y.center())).unwrap_or(0.0);
        dc.max((dist / 2.0) as u32).clamp(1, 256)
    } else {
        spec.steps.min(1000)
    };
    let spine = spec.spine.as_ref().map(|s| s.to_bezpath());
    let mut out = Vec::with_capacity(steps as usize + 2);
    for k in 0..=steps + 1 {
        let t = k as f64 / (steps + 1) as f64;
        let mut path = crate::anim::morph(&pa, &pb, t);
        if let Some(sp) = &spine {
            // Steps follow the spine instead of the straight line between the ends.
            use kurbo::{ParamCurve, ParamCurveArclen};
            let segs: Vec<kurbo::PathSeg> = sp.segments().collect();
            let lens: Vec<f64> = segs.iter().map(|s| s.arclen(1e-3)).collect();
            let total: f64 = lens.iter().sum();
            let mut s = t * total;
            let mut pos = None;
            for (seg, l) in segs.iter().zip(&lens) {
                if s <= *l {
                    pos = Some(seg.eval(seg.inv_arclen(s, 1e-4)));
                    break;
                }
                s -= l;
            }
            let pos = pos.or_else(|| segs.last().map(|s| s.eval(1.0)));
            if let (Some(pos), Some(c)) = (pos, path.bounds().map(|r| r.center())) {
                path.transform(Affine::translate(pos - c));
            }
        }
        let mut n = if t < 0.5 { a.clone() } else { b.clone() };
        n.transform = Affine::IDENTITY;
        n.kind = NodeKind::Path { path, rule: Default::default() };
        n.opacity = a.opacity + (b.opacity - a.opacity) * t as f32;
        // Mix the appearance items pairwise where both are solid.
        let mut items = Vec::new();
        for (ia, ib) in a.appearance.items.iter().zip(b.appearance.items.iter()) {
            items.push(match (ia, ib) {
                (Item::Fill(fa), Item::Fill(fb)) => {
                    let mut f = fa.clone();
                    if let (Paint::Solid { color: ca }, Paint::Solid { color: cb }) = (&fa.paint, &fb.paint) {
                        f.paint = Paint::solid(lerp_color(*ca, *cb, t as f32));
                    }
                    f.opacity = fa.opacity + (fb.opacity - fa.opacity) * t as f32;
                    Item::Fill(f)
                }
                (Item::Stroke(sa), Item::Stroke(sb)) => {
                    let mut s = sa.clone();
                    if let (Paint::Solid { color: ca }, Paint::Solid { color: cb }) = (&sa.paint, &sb.paint) {
                        s.paint = Paint::solid(lerp_color(*ca, *cb, t as f32));
                    }
                    s.style.width = sa.style.width + (sb.style.width - sa.style.width) * t;
                    Item::Stroke(s)
                }
                _ => if t < 0.5 { ia.clone() } else { ib.clone() },
            });
        }
        if items.is_empty() {
            items = n.appearance.items.clone();
        }
        n.appearance.items = items;
        out.push(n);
    }
    out
}

/// The map an envelope applies, for art whose bounds are `b`.
pub fn envelope_map(env: &Envelope, b: Rect) -> Box<dyn Fn(&PathData) -> PathData + '_> {
    match env {
        Envelope::Warp { style, bend, horizontal, vertical, vertical_axis } => {
            let (s, bd, h, v, va) = (*style, *bend, *horizontal, *vertical, *vertical_axis);
            Box::new(move |p| warp::warp(p, b, s, bd, h, v, va, MAX_ANCHORS))
        }
        Envelope::Free { corners } => {
            let c = *corners;
            Box::new(move |p| distort::free_distort(p, b, c, MAX_ANCHORS))
        }
        Envelope::Mesh { rows, cols, points } => {
            let (rows, cols) = ((*rows).max(1) as usize, (*cols).max(1) as usize);
            let pts = points.clone();
            Box::new(move |p| {
                if pts.len() != (rows + 1) * (cols + 1) {
                    return p.clone();
                }
                let f = |q: Point| {
                    let u = ((q.x - b.x0) / b.width().max(1e-9)).clamp(-0.5, 1.5) * cols as f64;
                    let v = ((q.y - b.y0) / b.height().max(1e-9)).clamp(-0.5, 1.5) * rows as f64;
                    let (i, j) = ((u.floor() as isize).clamp(0, cols as isize - 1) as usize, (v.floor() as isize).clamp(0, rows as isize - 1) as usize);
                    let (fu, fv) = (u - i as f64, v - j as f64);
                    let at = |i: usize, j: usize| pts[j * (cols + 1) + i];
                    let top = at(i, j).lerp(at(i + 1, j), fu);
                    let bot = at(i, j + 1).lerp(at(i + 1, j + 1), fu);
                    top.lerp(bot, fv)
                };
                warp::map_nonlinear_within(p, b.width().hypot(b.height()) / (8.0 * rows.max(cols) as f64), MAX_ANCHORS, f)
            })
        }
        Envelope::Shape { shape } => {
            let coons = warp::Coons::from_path(shape).unwrap_or_else(|| warp::Coons::rect(b));
            Box::new(move |p| {
                let f = |q: Point| coons.eval((q.x - b.x0) / b.width().max(1e-9), (q.y - b.y0) / b.height().max(1e-9));
                warp::map_nonlinear_within(p, b.width().hypot(b.height()) / 32.0, MAX_ANCHORS, f)
            })
        }
    }
}

/// The homography taking the unit square (0,0) (1,0) (1,1) (0,1) to `q`.
pub fn square_to_quad(q: [Point; 4]) -> Option<Homography> {
    let (x0, y0, x1, y1, x2, y2, x3, y3) = (q[0].x, q[0].y, q[1].x, q[1].y, q[2].x, q[2].y, q[3].x, q[3].y);
    let dx1 = x1 - x2;
    let dx2 = x3 - x2;
    let dy1 = y1 - y2;
    let dy2 = y3 - y2;
    let sx = x0 - x1 + x2 - x3;
    let sy = y0 - y1 + y2 - y3;
    let den = dx1 * dy2 - dx2 * dy1;
    if den.abs() < 1e-15 {
        return None;
    }
    let g = (sx * dy2 - dx2 * sy) / den;
    let h = (dx1 * sy - sx * dy1) / den;
    let a = x1 - x0 + g * x1;
    let b = x3 - x0 + h * x3;
    let c = x0;
    let d = y1 - y0 + g * y1;
    let e = y3 - y0 + h * y3;
    let f = y0;
    // Row-major [a b c; d e f; g h 1].
    Homography::from_array([a, b, c, d, e, f, g, h, 1.0])
}

fn intersect(p: Point, r: Point, q: Point, s: Point) -> Option<Point> {
    let (d1, d2) = (r - p, s - q);
    let den = d1.cross(d2);
    if den.abs() < 1e-12 {
        return None;
    }
    let t = (q - p).cross(d2) / den;
    Some(p + d1 * t)
}

/// The homography of a perspective plane: grid units (1 = one cell) on the
/// plane to document points. Walls run from the origin towards their
/// vanishing point, `v` upwards; the floor's `u` runs right, `v` left.
pub fn plane_homography(p: &Perspective, plane: Plane) -> Option<Homography> {
    let o = p.origin;
    let step = |vp: Point| {
        let d = vp.distance(o).max(1e-9);
        o + (vp - o) * (p.cell / (p.cell + d))
    };
    let up = |q: Point| Point::new(q.x, q.y - p.cell);
    let quad = match plane {
        Plane::Right | Plane::Left => {
            let vp = if plane == Plane::Right { p.right_vp } else { p.left_vp };
            let a = step(vp);
            let top0 = up(o);
            // The far top corner: on the line from the near top towards the vanishing point,
            // straight above the far bottom corner.
            let top1 = intersect(top0, vp, a, Point::new(a.x, a.y - 1.0))?;
            [Point::new(o.x, o.y), a, top1, top0].map(|q| q)
        }
        Plane::Floor => {
            let r = step(p.right_vp);
            let l = step(p.left_vp);
            let far = intersect(r, p.left_vp, l, p.right_vp)?;
            [o, r, far, l]
        }
    };
    // Walls: u along the ground, v up (y down in documents, so v = -y).
    let h = square_to_quad(quad)?;
    Some(h)
}

/// Maps art in plane units (y down, so up the wall is negative y) onto the plane.
pub fn perspective_map(p: &Perspective, plane: Plane, path: &PathData) -> PathData {
    let Some(h) = plane_homography(p, plane) else { return path.clone() };
    let f = |q: Point| {
        let u = Point::new(q.x / p.cell, -q.y / p.cell);
        h.apply(u).unwrap_or(q)
    };
    let b = path.bounds().unwrap_or(Rect::ZERO);
    warp::map_nonlinear_within(path, (b.width().hypot(b.height()) / 16.0).max(1.0), MAX_ANCHORS, f)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shapes::LiveShape;

    #[test]
    fn square_to_quad_maps_corners() {
        let q = [Point::new(10.0, 10.0), Point::new(110.0, 20.0), Point::new(100.0, 120.0), Point::new(0.0, 90.0)];
        let h = square_to_quad(q).unwrap();
        for (u, want) in [(Point::new(0.0, 0.0), q[0]), (Point::new(1.0, 0.0), q[1]), (Point::new(1.0, 1.0), q[2]), (Point::new(0.0, 1.0), q[3])] {
            let p = h.apply(u).unwrap();
            assert!(p.distance(want) < 1e-9, "{u:?} -> {p:?}, want {want:?}");
        }
    }

    #[test]
    fn effects_change_outlines() {
        let r = LiveShape::Rect { rect: Rect::new(0.0, 0.0, 100.0, 100.0), radii: [0.0; 4], corners: None }.to_path();
        let grown = apply_effects(&r, &[Effect::Offset { amount: 10.0, join: Default::default() }]);
        let b = grown.bounds().unwrap();
        assert!((b.width() - 120.0).abs() < 0.5, "{b:?}");
        let zz = apply_effects(&r, &[Effect::ZigZag { size: 5.0, ridges: 4.0, smooth: false }]);
        assert!(zz.anchor_count() > 16);
        let round = apply_effects(&r, &[Effect::RoundCorners { radius: 10.0 }]);
        assert_eq!(round.anchor_count(), 8);
        let copies = repeat_transforms(&Repeat::Radial { count: 6, center: Some(Point::ZERO), radius: 0.0, rotate: true }, Rect::new(-5.0, -50.0, 5.0, -40.0));
        assert_eq!(copies.len(), 6);
    }
}
