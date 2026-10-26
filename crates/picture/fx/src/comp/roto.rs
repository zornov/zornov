// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Roto: animated Bézier shapes rasterised into a matte.
//!
//! A `roto` node's data is `{"shapes": [...]}`; each shape has `points`
//! (`[x, y]` corners or `[x, y, in_x, in_y, out_x, out_y]` with handles
//! relative to the point, in pixels of the comp's picture), optional
//! `keys` (`[{"t": seconds, "points": [...]}]`, same point count, eased
//! between), `feather` (pixels), `mode` (`add`, `subtract`, `intersect`),
//! `opacity`, `invert`, and `from`/`to` seconds it exists between.

use std::sync::Arc;

use serde_json::Value as Json;

use super::nodes::{BASE, MATTE_OUT, NodeCategory, NodeDef, main_or_blank, matte_output};
use super::{Node, NodeCtx, Plate, geo};
use crate::params::{ParamDef, ParamSet};

pub static NODES: &[NodeDef] = &[NodeDef {
    id: "roto",
    name: "Roto",
    category: NodeCategory::Matte,
    description: "Animated Bézier shapes as a matte: isolate a subject, hold out an object, make a garbage matte.",
    params: &[ParamDef::float("feather", "Feather", 0.0, 500.0, 0.0), ParamDef::bool("invert", "Invert", false), ParamDef::choice("output", "Output", MATTE_OUT, 1)],
    data: "shapes: [{points: [[x,y] or [x,y,inx,iny,outx,outy]], keys: [{t, points}], feather, mode: add|subtract|intersect, opacity, invert, from, to}] in picture pixels",
    run: roto,
    animated: roto_animated,
    ..BASE
}];

fn roto_animated(n: &Node) -> bool {
    shapes(&n.data).iter().any(|s| s.keys.len() > 1 || s.from.is_some() || s.to.is_some())
}

/// A control point: position and the two handles (relative).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cv {
    pub p: [f64; 2],
    pub h_in: [f64; 2],
    pub h_out: [f64; 2],
}

/// One roto shape.
#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    pub points: Vec<Cv>,
    pub keys: Vec<(f64, Vec<Cv>)>,
    pub feather: f64,
    pub mode: u8,
    pub opacity: f64,
    pub invert: bool,
    pub from: Option<f64>,
    pub to: Option<f64>,
}

fn cvs(v: &Json) -> Vec<Cv> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|p| {
                    let p = p.as_array()?;
                    let g = |i: usize| p.get(i).and_then(Json::as_f64).unwrap_or(0.0);
                    (p.len() >= 2).then(|| Cv { p: [g(0), g(1)], h_in: [g(2), g(3)], h_out: [g(4), g(5)] })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The shapes in a roto node's data.
pub fn shapes(data: &Json) -> Vec<Shape> {
    data.get("shapes")
        .and_then(Json::as_array)
        .map(|a| {
            a.iter()
                .map(|s| {
                    let mut keys: Vec<(f64, Vec<Cv>)> = s.get("keys").and_then(Json::as_array).map(|k| k.iter().filter_map(|k| Some((k.get("t")?.as_f64()?, cvs(k.get("points")?)))).collect()).unwrap_or_default();
                    keys.sort_by(|a, b| a.0.total_cmp(&b.0));
                    Shape {
                        points: s.get("points").map(cvs).unwrap_or_default(),
                        keys,
                        feather: s.get("feather").and_then(Json::as_f64).unwrap_or(0.0),
                        mode: match s.get("mode").and_then(Json::as_str) {
                            Some("subtract") => 1,
                            Some("intersect") => 2,
                            _ => 0,
                        },
                        opacity: s.get("opacity").and_then(Json::as_f64).unwrap_or(1.0),
                        invert: s.get("invert").and_then(Json::as_bool).unwrap_or(false),
                        from: s.get("from").and_then(Json::as_f64),
                        to: s.get("to").and_then(Json::as_f64),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

impl Shape {
    /// The control points at `t` seconds.
    pub fn at(&self, t: f64) -> Vec<Cv> {
        if self.keys.is_empty() {
            return self.points.clone();
        }
        let i = self.keys.iter().position(|k| k.0 > t);
        let (a, b) = match i {
            Some(0) => return self.keys[0].1.clone(),
            None => return self.keys[self.keys.len() - 1].1.clone(),
            Some(i) => (&self.keys[i - 1], &self.keys[i]),
        };
        if a.1.len() != b.1.len() {
            return a.1.clone();
        }
        let u = ((t - a.0) / (b.0 - a.0).max(1e-9)).clamp(0.0, 1.0);
        let s = u * u * (3.0 - 2.0 * u);
        let l = |x: [f64; 2], y: [f64; 2]| [x[0] + (y[0] - x[0]) * s, x[1] + (y[1] - x[1]) * s];
        a.1.iter().zip(&b.1).map(|(p, q)| Cv { p: l(p.p, q.p), h_in: l(p.h_in, q.h_in), h_out: l(p.h_out, q.h_out) }).collect()
    }
}

/// The outline of closed control points as a polygon (curves flattened).
pub fn flatten(cv: &[Cv]) -> Vec<[f64; 2]> {
    let n = cv.len();
    let mut out = Vec::with_capacity(n * 8);
    for i in 0..n {
        let (a, b) = (cv[i], cv[(i + 1) % n]);
        let p0 = a.p;
        let p1 = [a.p[0] + a.h_out[0], a.p[1] + a.h_out[1]];
        let p2 = [b.p[0] + b.h_in[0], b.p[1] + b.h_in[1]];
        let p3 = b.p;
        let straight = a.h_out == [0.0, 0.0] && b.h_in == [0.0, 0.0];
        let len = ((p3[0] - p0[0]).powi(2) + (p3[1] - p0[1]).powi(2)).sqrt();
        let steps = if straight { 1 } else { (len / 4.0).ceil().clamp(4.0, 64.0) as usize };
        for k in 0..steps {
            let t = k as f64 / steps as f64;
            let m = 1.0 - t;
            out.push(std::array::from_fn(|c| m * m * m * p0[c] + 3.0 * m * m * t * p1[c] + 3.0 * m * t * t * p2[c] + t * t * t * p3[c]));
        }
    }
    out
}

/// Anti-aliased coverage (0..1) of a polygon, non-zero winding, with 4×4
/// supersampling per pixel.
pub fn rasterise(poly: &[[f64; 2]], w: usize, h: usize) -> Vec<f32> {
    let mut cov = vec![0.0f32; w * h];
    if poly.len() < 3 || w == 0 || h == 0 {
        return cov;
    }
    const SS: usize = 4;
    let n = poly.len();
    let ymin = poly.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min).floor().max(0.0) as usize;
    let ymax = (poly.iter().map(|p| p[1]).fold(f64::NEG_INFINITY, f64::max).ceil().max(0.0) as usize).min(h);
    let mut xs: Vec<(f64, i32)> = Vec::new();
    let mut row = vec![0.0f32; w];
    for y in ymin..ymax {
        row.iter_mut().for_each(|v| *v = 0.0);
        for sy in 0..SS {
            let fy = y as f64 + (sy as f64 + 0.5) / SS as f64;
            xs.clear();
            for i in 0..n {
                let (a, b) = (poly[i], poly[(i + 1) % n]);
                if (a[1] <= fy) != (b[1] <= fy) {
                    let x = a[0] + (fy - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
                    xs.push((x, if b[1] > a[1] { 1 } else { -1 }));
                }
            }
            xs.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut wind = 0;
            for k in 0..xs.len() {
                let before = wind;
                wind += xs[k].1;
                if before == 0 && wind != 0 {
                    // Span start; find where the winding returns to zero.
                    let x0 = xs[k].0;
                    let mut ww = wind;
                    let mut x1 = x0;
                    for e in xs.iter().skip(k + 1) {
                        ww += e.1;
                        if ww == 0 {
                            x1 = e.0;
                            break;
                        }
                    }
                    span(&mut row, x0, x1, 1.0 / SS as f32);
                }
            }
        }
        cov[y * w..(y + 1) * w].copy_from_slice(&row);
    }
    cov.iter_mut().for_each(|v| *v = v.clamp(0.0, 1.0));
    cov
}

/// Adds `k` × the horizontal coverage of `[x0, x1)` to a row.
fn span(row: &mut [f32], x0: f64, x1: f64, k: f32) {
    let w = row.len() as f64;
    let (x0, x1) = (x0.clamp(0.0, w), x1.clamp(0.0, w));
    if x1 <= x0 {
        return;
    }
    let (i0, i1) = (x0.floor() as usize, (x1.ceil() as usize).min(row.len()));
    for (i, v) in row.iter_mut().enumerate().take(i1).skip(i0) {
        let l = (x1.min(i as f64 + 1.0) - x0.max(i as f64)).max(0.0);
        *v += l as f32 * k;
    }
}

/// The matte of a roto node's shapes at `t`.
pub fn roto_matte(data: &Json, t: f64, w: usize, h: usize, extra_feather: f64) -> Vec<f32> {
    let mut m = vec![0.0f32; w * h];
    for s in shapes(data) {
        if s.from.is_some_and(|f| t < f) || s.to.is_some_and(|e| t >= e) {
            continue;
        }
        let mut c = rasterise(&flatten(&s.at(t)), w, h);
        let f = s.feather + extra_feather;
        if f > 0.0 {
            c = geo::blur_plane(&c, w, h, (f * 0.5) as f32);
        }
        let op = s.opacity.clamp(0.0, 1.0) as f32;
        for (dst, &v) in m.iter_mut().zip(&c) {
            let v = if s.invert { 1.0 - v } else { v } * op;
            *dst = match s.mode {
                1 => *dst * (1.0 - v),
                2 => *dst * v,
                _ => dst.max(v),
            };
        }
    }
    m
}

fn roto(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], p: &ParamSet) -> Result<Plate, String> {
    let src = main_or_blank(ctx, inputs);
    let mut m = roto_matte(&ctx.node.data, ctx.time, ctx.width, ctx.height, p.f("feather") as f64);
    if p.b("invert") {
        m.iter_mut().for_each(|v| *v = 1.0 - *v);
    }
    // With nothing plugged in, show the matte itself.
    let out = if inputs.first().is_some_and(Option::is_some) { p.choice("output") } else { 1 };
    Ok(matte_output(&src, &m, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_square_covers_its_area_with_soft_edges() {
        let poly = [[10.25, 10.0], [30.25, 10.0], [30.25, 30.0], [10.25, 30.0]];
        let c = rasterise(&poly, 40, 40);
        let area: f32 = c.iter().sum();
        assert!((area - 400.0).abs() < 1.0, "{area}");
        assert!((c[20 * 40 + 10] - 0.75).abs() < 0.01);
        assert_eq!(c[5 * 40 + 5], 0.0);
        assert_eq!(c[20 * 40 + 20], 1.0);
    }

    #[test]
    fn shapes_ease_between_keys() {
        let data = serde_json::json!({"shapes": [{"keys": [
            {"t": 0.0, "points": [[0,0],[10,0],[10,10],[0,10]]},
            {"t": 1.0, "points": [[20,0],[30,0],[30,10],[20,10]]}
        ]}]});
        let s = &shapes(&data)[0];
        assert_eq!(s.at(0.5)[0].p, [10.0, 0.0]);
        assert_eq!(s.at(2.0)[0].p, [20.0, 0.0]);
    }
}
