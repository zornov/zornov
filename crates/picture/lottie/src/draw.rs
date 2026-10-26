// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Drawing a composition at a frame.
//!
//! Layers are listed top first and drawn bottom up, each into its own
//! picture (so masks, mattes, opacity and blend modes apply to the layer
//! as a whole) composited onto the one below. In a shape layer a fill or
//! stroke paints every path listed before it in its group (nested groups
//! included), and what comes later in a list is drawn beneath what comes
//! earlier, as in the format's description of shape stacking.

use crate::model::{Content, Item, Layer, apply, mul};
use crate::value::Bez;
use crate::{Animation, Comp};
use tiny_skia::{BlendMode, Color, FillRule, GradientStop, LineCap, LineJoin, LinearGradient, Mask, MaskType, Paint, PathBuilder, Pixmap, PixmapPaint, Point, RadialGradient, Shader, SpreadMode, Stroke, StrokeDash, Transform};

/// Precomps within precomps, at most.
const MAX_DEPTH: usize = 16;

fn ts(m: [f64; 6]) -> Transform {
    Transform::from_row(m[0] as f32, m[1] as f32, m[2] as f32, m[3] as f32, m[4] as f32, m[5] as f32)
}

/// The layer's own time.
fn local(l: &Layer, frame: f64) -> f64 {
    (frame - l.start) / l.stretch
}

/// Layer `i`'s matrix with its parents', into the composition.
fn world(c: &Comp, i: usize, frame: f64) -> [f64; 6] {
    let mut l = &c.layers[i];
    let mut m = l.tf.matrix(local(l, frame));
    for _ in 0..64 {
        let Some(p) = l.parent.and_then(|p| c.layers.iter().find(|x| x.index == Some(p))) else { break };
        m = mul(p.tf.matrix(local(p, frame)), m);
        l = p;
    }
    m
}

fn blend(bm: u8) -> BlendMode {
    match bm {
        1 => BlendMode::Multiply,
        2 => BlendMode::Screen,
        3 => BlendMode::Overlay,
        4 => BlendMode::Darken,
        5 => BlendMode::Lighten,
        6 => BlendMode::ColorDodge,
        7 => BlendMode::ColorBurn,
        8 => BlendMode::HardLight,
        9 => BlendMode::SoftLight,
        10 => BlendMode::Difference,
        11 => BlendMode::Exclusion,
        12 => BlendMode::Hue,
        13 => BlendMode::Saturation,
        14 => BlendMode::Color,
        15 => BlendMode::Luminosity,
        16 => BlendMode::Plus,
        _ => BlendMode::SourceOver,
    }
}

/// Draws composition `c` at `frame` onto `target` through `base`.
pub fn comp(anim: &Animation, c: &Comp, frame: f64, target: &mut Pixmap, base: [f64; 6], depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    let shown = |l: &Layer| !l.hidden && frame >= l.in_frame && frame < l.out_frame;
    for i in (0..c.layers.len()).rev() {
        let l = &c.layers[i];
        if l.is_matte || !shown(l) {
            continue;
        }
        let Some(mut px) = layer(anim, c, i, frame, target.width(), target.height(), base, depth) else { continue };
        if l.matte != 0 {
            let m = l.matte_parent.and_then(|p| c.layers.iter().position(|x| x.index == Some(p))).or_else(|| i.checked_sub(1));
            let inverted = l.matte == 2 || l.matte == 4;
            let matte = m.filter(|&m| shown(&c.layers[m])).and_then(|m| layer(anim, c, m, frame, target.width(), target.height(), base, depth));
            match matte {
                Some(mp) => {
                    let mut mask = Mask::from_pixmap(mp.as_ref(), if l.matte >= 3 { MaskType::Luminance } else { MaskType::Alpha });
                    if inverted {
                        mask.data_mut().iter_mut().for_each(|a| *a = 255 - *a);
                    }
                    px.apply_mask(&mask);
                }
                // No matte showing: an alpha matte hides all, an inverted one nothing.
                None if !inverted => continue,
                None => {}
            }
        }
        target.draw_pixmap(0, 0, px.as_ref(), &PixmapPaint { opacity: 1.0, blend_mode: blend(l.blend), quality: tiny_skia::FilterQuality::Nearest }, Transform::identity(), None);
    }
}

/// Layer `i` drawn alone into a target-sized picture (masks and opacity
/// applied), or `None` when it draws nothing.
#[allow(clippy::too_many_arguments)]
fn layer(anim: &Animation, c: &Comp, i: usize, frame: f64, w: u32, h: u32, base: [f64; 6], depth: usize) -> Option<Pixmap> {
    let l = &c.layers[i];
    let t = local(l, frame);
    let m = mul(base, world(c, i, frame));
    let mut px = Pixmap::new(w, h)?;
    match &l.content {
        Content::Null | Content::Nothing => return None,
        Content::Solid { colour, width, height } => {
            let rect = tiny_skia::Rect::from_xywh(0.0, 0.0, *width as f32, *height as f32)?;
            px.fill_rect(rect, &paint(Shader::SolidColor(rgba(colour, 1.0))), ts(m), None);
        }
        Content::Image(id) => {
            let img = anim.images.get(id)?;
            px.draw_pixmap(0, 0, img.as_ref().as_ref(), &PixmapPaint { opacity: 1.0, blend_mode: BlendMode::SourceOver, quality: tiny_skia::FilterQuality::Bicubic }, ts(m), None);
        }
        Content::Precomp { id, width, height, remap } => {
            let child = anim.comps.get(id)?;
            let ct = match remap {
                Some(r) => r.num(t, 0.0) * anim.fps,
                None => t,
            };
            comp(anim, child, ct, &mut px, m, depth + 1);
            // A precomp shows only inside its own frame.
            if *width > 0.0 && *height > 0.0 {
                let mut clip = Mask::new(w, h)?;
                let rect = PathBuilder::from_rect(tiny_skia::Rect::from_xywh(0.0, 0.0, *width as f32, *height as f32)?);
                clip.fill_path(&rect, FillRule::Winding, true, ts(m));
                px.apply_mask(&clip);
            }
        }
        Content::Shapes(items) => {
            let mut store = Vec::new();
            let (_, draws) = group(items, t, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], 1.0, &mut store);
            run(&draws, &store, &mut px, ts(m));
        }
    }
    if !l.masks.is_empty() {
        if let Some(mask) = masks(l, t, w, h, ts(m)) {
            px.apply_mask(&mask);
        }
    }
    let o = l.tf.opacity(t);
    if o < 1.0 {
        let k = o as f32;
        px.data_mut().iter_mut().for_each(|b| *b = (*b as f32 * k + 0.5) as u8);
    }
    Some(px)
}

fn masks(l: &Layer, t: f64, w: u32, h: u32, xf: Transform) -> Option<Mask> {
    let mut acc: Option<Vec<f32>> = None;
    for mk in &l.masks {
        if mk.mode == 'n' {
            continue;
        }
        let mut one = Mask::new(w, h)?;
        if let Some(path) = path(&[sub_of(&mk.path.at(t), &[1.0, 0.0, 0.0, 1.0, 0.0, 0.0])]) {
            one.fill_path(&path, FillRule::Winding, true, xf);
        }
        let k = (mk.opacity.num(t, 100.0) / 100.0).clamp(0.0, 1.0) as f32;
        let m: Vec<f32> = one.data().iter().map(|a| { let v = *a as f32 / 255.0 * k; if mk.inverted { 1.0 - v } else { v } }).collect();
        let full = || vec![1.0f32; m.len()];
        acc = Some(match mk.mode {
            's' => acc.unwrap_or_else(full).iter().zip(&m).map(|(a, b)| a * (1.0 - b)).collect(),
            'i' => acc.unwrap_or_else(full).iter().zip(&m).map(|(a, b)| a * b).collect(),
            'l' => match acc { Some(a) => a.iter().zip(&m).map(|(a, b)| a.max(*b)).collect(), None => m },
            'd' => match acc { Some(a) => a.iter().zip(&m).map(|(a, b)| a.min(*b)).collect(), None => m },
            'f' => match acc { Some(a) => a.iter().zip(&m).map(|(a, b)| (a - b).abs()).collect(), None => m },
            _ => match acc { Some(a) => a.iter().zip(&m).map(|(a, b)| a + b - a * b).collect(), None => m },
        });
    }
    let acc = acc?;
    let mut mask = Mask::new(w, h)?;
    for (d, a) in mask.data_mut().iter_mut().zip(acc) {
        *d = (a.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    }
    Some(mask)
}

fn rgba(c: &[f64], alpha: f64) -> Color {
    // Some writers use 0..255.
    let k = if c.iter().take(3).any(|v| *v > 1.0) { 1.0 / 255.0 } else { 1.0 };
    let ch = |i: usize| (c.get(i).copied().unwrap_or(0.0) * k).clamp(0.0, 1.0) as f32;
    Color::from_rgba(ch(0), ch(1), ch(2), alpha.clamp(0.0, 1.0) as f32 * c.get(3).map_or(1.0, |a| (a * k).clamp(0.0, 1.0) as f32)).unwrap_or(Color::BLACK)
}

fn paint(shader: Shader<'static>) -> Paint<'static> {
    Paint { shader, anti_alias: true, ..Paint::default() }
}

/// A sub-path with absolute control points, in layer space.
#[derive(Clone, Debug, PartialEq)]
struct Sub {
    v: Vec<[f64; 2]>,
    /// In-tangent control point of each vertex.
    i: Vec<[f64; 2]>,
    /// Out-tangent control point.
    o: Vec<[f64; 2]>,
    closed: bool,
}

fn sub_of(b: &Bez, m: &[f64; 6]) -> Sub {
    let n = b.v.len();
    let abs = |k: usize, t: &[[f64; 2]]| apply(m, [b.v[k][0] + t[k][0], b.v[k][1] + t[k][1]]);
    Sub { v: (0..n).map(|k| apply(m, b.v[k])).collect(), i: (0..n).map(|k| abs(k, &b.i)).collect(), o: (0..n).map(|k| abs(k, &b.o)).collect(), closed: b.closed }
}

/// Kappa: the tangent length that makes a quarter circle from a cubic.
const K: f64 = 0.552_284_749_8;

fn ellipse(c: [f64; 2], s: [f64; 2]) -> Bez {
    let (rx, ry) = (s[0] / 2.0, s[1] / 2.0);
    Bez {
        closed: true,
        v: vec![[c[0], c[1] - ry], [c[0] + rx, c[1]], [c[0], c[1] + ry], [c[0] - rx, c[1]]],
        i: vec![[-K * rx, 0.0], [0.0, -K * ry], [K * rx, 0.0], [0.0, K * ry]],
        o: vec![[K * rx, 0.0], [0.0, K * ry], [-K * rx, 0.0], [0.0, -K * ry]],
    }
}

fn rect(c: [f64; 2], s: [f64; 2], r: f64) -> Bez {
    let (hw, hh) = (s[0] / 2.0, s[1] / 2.0);
    let r = r.clamp(0.0, hw.min(hh));
    let (l, t, rr, b) = (c[0] - hw, c[1] - hh, c[0] + hw, c[1] + hh);
    if r <= 0.0 {
        return Bez { closed: true, v: vec![[rr, t], [rr, b], [l, b], [l, t]], i: vec![[0.0; 2]; 4], o: vec![[0.0; 2]; 4] };
    }
    let k = K * r;
    // Clockwise from the top of the right edge, two vertices per corner
    // (where lottie-web starts it: it matters to trim paths).
    Bez {
        closed: true,
        v: vec![[rr, t + r], [rr, b - r], [rr - r, b], [l + r, b], [l, b - r], [l, t + r], [l + r, t], [rr - r, t]],
        i: vec![[0.0, -k], [0.0, 0.0], [k, 0.0], [0.0, 0.0], [0.0, k], [0.0, 0.0], [-k, 0.0], [0.0, 0.0]],
        o: vec![[0.0, 0.0], [0.0, k], [0.0, 0.0], [-k, 0.0], [0.0, 0.0], [0.0, -k], [0.0, 0.0], [k, 0.0]],
    }
}

fn star(c: [f64; 2], points: f64, rot: f64, outer: f64, inner: Option<f64>) -> Bez {
    let n = points.round().clamp(3.0, 100.0) as usize;
    let verts = if inner.is_some() { 2 * n } else { n };
    let step = std::f64::consts::TAU / verts as f64;
    let start = rot.to_radians() - std::f64::consts::FRAC_PI_2;
    let v: Vec<[f64; 2]> = (0..verts)
        .map(|k| {
            let r = if inner.is_some() && k % 2 == 1 { inner.unwrap_or(0.0) } else { outer };
            let a = start + step * k as f64;
            [c[0] + r * a.cos(), c[1] + r * a.sin()]
        })
        .collect();
    Bez { closed: true, i: vec![[0.0; 2]; v.len()], o: vec![[0.0; 2]; v.len()], v }
}

fn path(subs: &[Sub]) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    for s in subs {
        let n = s.v.len();
        if n == 0 {
            continue;
        }
        pb.move_to(s.v[0][0] as f32, s.v[0][1] as f32);
        let seg = |pb: &mut PathBuilder, a: usize, b: usize| pb.cubic_to(s.o[a][0] as f32, s.o[a][1] as f32, s.i[b][0] as f32, s.i[b][1] as f32, s.v[b][0] as f32, s.v[b][1] as f32);
        for k in 0..n - 1 {
            seg(&mut pb, k, k + 1);
        }
        if s.closed {
            seg(&mut pb, n - 1, 0);
            pb.close();
        }
    }
    pb.finish()
}

/// What a group draws, in list order (drawn last to first). Styles name
/// the paths they paint; a trim listed anywhere after a path changes that
/// path for every style, before or after the trim (as
/// the players that follow it do), so paths live in one store per layer
/// and are read only when drawing.
enum Draw {
    Group(Vec<Draw>),
    Fill { paths: Vec<usize>, shader: Shader<'static>, even_odd: bool },
    Stroke { paths: Vec<usize>, shader: Shader<'static>, stroke: Stroke },
}

fn collect(store: &[Vec<Sub>], ids: &[usize]) -> Vec<Sub> {
    ids.iter().flat_map(|&i| store[i].iter().cloned()).collect()
}

fn run(draws: &[Draw], store: &[Vec<Sub>], px: &mut Pixmap, xf: Transform) {
    for d in draws.iter().rev() {
        match d {
            Draw::Group(inner) => run(inner, store, px, xf),
            Draw::Fill { paths, shader, even_odd } => {
                if let Some(p) = path(&collect(store, paths)) {
                    px.fill_path(&p, &paint(shader.clone()), if *even_odd { FillRule::EvenOdd } else { FillRule::Winding }, xf, None);
                }
            }
            Draw::Stroke { paths, shader, stroke } => {
                if let Some(p) = path(&collect(store, paths)) {
                    px.stroke_path(&p, &paint(shader.clone()), stroke, xf, None);
                }
            }
        }
    }
}

fn gradient(g: &crate::model::Gradient, t: f64, m: &[f64; 6], alpha: f64) -> Option<Shader<'static>> {
    let vals = g.stops.values.at(t);
    let n = g.stops.count.max(1);
    if vals.len() < n * 4 {
        return None;
    }
    let alphas: Vec<(f64, f64)> = vals[n * 4..].chunks_exact(2).map(|p| (p[0], p[1])).collect();
    let alpha_at = |pos: f64| -> f64 {
        if alphas.is_empty() {
            return 1.0;
        }
        let k = alphas.iter().position(|(p, _)| *p >= pos).unwrap_or(alphas.len() - 1);
        if k == 0 || alphas[k].0 <= pos {
            return alphas[k].1;
        }
        let (a, b) = (alphas[k - 1], alphas[k]);
        a.1 + (b.1 - a.1) * (pos - a.0) / (b.0 - a.0).max(1e-9)
    };
    let stops: Vec<GradientStop> = vals[..n * 4].chunks_exact(4).map(|s| GradientStop::new(s[0].clamp(0.0, 1.0) as f32, rgba(&[s[1], s[2], s[3], alpha_at(s[0])], alpha))).collect();
    let (s, e) = (apply(m, g.start.pair(t, [0.0, 0.0])), apply(m, g.end.pair(t, [0.0, 0.0])));
    let (sp, ep) = (Point::from_xy(s[0] as f32, s[1] as f32), Point::from_xy(e[0] as f32, e[1] as f32));
    if g.radial {
        let r = ((e[0] - s[0]).hypot(e[1] - s[1]) as f32).max(0.01);
        RadialGradient::new(sp, sp, r, stops, SpreadMode::Pad, Transform::identity())
    } else {
        LinearGradient::new(sp, ep, stops, SpreadMode::Pad, Transform::identity())
    }
}

fn stroke_of(width: f64, cap: u8, join: u8, miter: f64, dash: Option<StrokeDash>) -> Stroke {
    Stroke {
        width: width.max(0.0) as f32,
        line_cap: match cap {
            1 => LineCap::Butt,
            3 => LineCap::Square,
            _ => LineCap::Round,
        },
        line_join: match join {
            1 => LineJoin::Miter,
            3 => LineJoin::Bevel,
            _ => LineJoin::Round,
        },
        miter_limit: miter.max(1.0) as f32,
        dash,
    }
}

/// How much `m` scales lengths.
fn length_scale(m: &[f64; 6]) -> f64 {
    (m[0] * m[3] - m[1] * m[2]).abs().sqrt()
}

/// A group's paths (ids in `store`, for the styles and trims after it)
/// and draws.
fn group(items: &[Item], t: f64, gm: [f64; 6], opacity: f64, store: &mut Vec<Vec<Sub>>) -> (Vec<usize>, Vec<Draw>) {
    let mut ids: Vec<usize> = Vec::new();
    let mut draws = Vec::new();
    let mut add = |store: &mut Vec<Vec<Sub>>, s: Sub, ids: &mut Vec<usize>| {
        store.push(vec![s]);
        ids.push(store.len() - 1);
    };
    for it in items {
        match it {
            Item::Group { items, tf, hidden } => {
                if *hidden {
                    continue;
                }
                let (inner, d) = group(items, t, mul(gm, tf.matrix(t)), opacity * tf.opacity(t), store);
                ids.extend(inner);
                draws.push(Draw::Group(d));
            }
            Item::Rect { position, size, roundness } => add(store, sub_of(&rect(position.pair(t, [0.0, 0.0]), size.pair(t, [0.0, 0.0]), roundness.num(t, 0.0)), &gm), &mut ids),
            Item::Ellipse { position, size } => add(store, sub_of(&ellipse(position.pair(t, [0.0, 0.0]), size.pair(t, [0.0, 0.0])), &gm), &mut ids),
            Item::Star { polygon, position, points, rotation, outer, inner } => add(store, sub_of(&star(position.pair(t, [0.0, 0.0]), points.num(t, 5.0), rotation.num(t, 0.0), outer.num(t, 0.0), if *polygon { None } else { Some(inner.num(t, 0.0)) }), &gm), &mut ids),
            Item::Path(p) => add(store, sub_of(&p.at(t), &gm), &mut ids),
            Item::Fill { colour, opacity: o, even_odd } => {
                let a = opacity * o.num(t, 100.0) / 100.0;
                draws.push(Draw::Fill { paths: ids.clone(), shader: Shader::SolidColor(rgba(&colour.at(t), a)), even_odd: *even_odd });
            }
            Item::GradientFill { g, opacity: o, even_odd } => {
                if let Some(shader) = gradient(g, t, &gm, opacity * o.num(t, 100.0) / 100.0) {
                    draws.push(Draw::Fill { paths: ids.clone(), shader, even_odd: *even_odd });
                }
            }
            Item::Stroke { colour, opacity: o, width, cap, join, miter, dash } => {
                let k = length_scale(&gm);
                let a = opacity * o.num(t, 100.0) / 100.0;
                let mut lens: Vec<f32> = dash.iter().filter(|(n, _)| *n != 'o').map(|(_, v)| (v.num(t, 0.0) * k).max(0.0) as f32).collect();
                if lens.len() % 2 == 1 {
                    lens.extend(lens.clone());
                }
                let offset = dash.iter().find(|(n, _)| *n == 'o').map_or(0.0, |(_, v)| (v.num(t, 0.0) * k) as f32);
                let d = if lens.len() >= 2 && lens.iter().any(|l| *l > 0.0) { StrokeDash::new(lens, offset) } else { None };
                draws.push(Draw::Stroke { paths: ids.clone(), shader: Shader::SolidColor(rgba(&colour.at(t), a)), stroke: stroke_of(width.num(t, 1.0) * k, *cap, *join, *miter, d) });
            }
            Item::GradientStroke { g, opacity: o, width, cap, join, miter } => {
                if let Some(shader) = gradient(g, t, &gm, opacity * o.num(t, 100.0) / 100.0) {
                    draws.push(Draw::Stroke { paths: ids.clone(), shader, stroke: stroke_of(width.num(t, 1.0) * length_scale(&gm), *cap, *join, *miter, None) });
                }
            }
            Item::Trim { start, end, offset, together } => {
                let before: Vec<Vec<Sub>> = ids.iter().map(|&i| store[i].clone()).collect();
                let after = trim(&before, start.num(t, 0.0) / 100.0, end.num(t, 100.0) / 100.0, offset.num(t, 0.0) / 360.0, *together);
                for (&i, subs) in ids.iter().zip(after) {
                    store[i] = subs;
                }
            }
            Item::Transform(_) => {}
        }
    }
    (ids, draws)
}

/// A sub-path as a polyline.
fn flatten(s: &Sub) -> Vec<[f64; 2]> {
    let n = s.v.len();
    if n == 0 {
        return Vec::new();
    }
    let mut out = vec![s.v[0]];
    let segs = if s.closed { n } else { n - 1 };
    for k in 0..segs {
        let (a, b) = (k, (k + 1) % n);
        let (p0, p1, p2, p3) = (s.v[a], s.o[a], s.i[b], s.v[b]);
        for j in 1..=24 {
            let u = j as f64 / 24.0;
            let m = 1.0 - u;
            let c = |d: usize| m * m * m * p0[d] + 3.0 * m * m * u * p1[d] + 3.0 * m * u * u * p2[d] + u * u * u * p3[d];
            out.push([c(0), c(1)]);
        }
    }
    out
}

fn line(pts: Vec<[f64; 2]>) -> Sub {
    Sub { i: pts.clone(), o: pts.clone(), v: pts, closed: false }
}

/// The part of a polyline between lengths `a` and `b`.
fn cut(pts: &[[f64; 2]], cum: &[f64], a: f64, b: f64) -> Vec<[f64; 2]> {
    let at = |d: f64| -> [f64; 2] {
        let k = cum.partition_point(|c| *c < d).clamp(1, pts.len() - 1);
        let (l0, l1) = (cum[k - 1], cum[k]);
        let u = if l1 > l0 { (d - l0) / (l1 - l0) } else { 0.0 };
        [pts[k - 1][0] + (pts[k][0] - pts[k - 1][0]) * u, pts[k - 1][1] + (pts[k][1] - pts[k - 1][1]) * u]
    };
    let mut out = vec![at(a)];
    out.extend(pts.iter().zip(cum).filter(|(_, c)| **c > a && **c < b).map(|(p, _)| *p));
    out.push(at(b));
    out
}

/// Trim paths: keep `start..end` (fractions, shifted by `offset`) of each
/// sub-path (`together`), or of all of them end to end; the result keeps
/// each piece with the path it came from.
fn trim(paths: &[Vec<Sub>], start: f64, end: f64, offset: f64, together: bool) -> Vec<Vec<Sub>> {
    let (mut s, mut e) = (start.clamp(0.0, 1.0), end.clamp(0.0, 1.0));
    if s > e {
        std::mem::swap(&mut s, &mut e);
    }
    if e - s >= 1.0 - 1e-9 {
        return paths.to_vec();
    }
    let mut out: Vec<Vec<Sub>> = vec![Vec::new(); paths.len()];
    if e - s <= 1e-9 {
        return out;
    }
    let (s, e) = (s + offset.rem_euclid(1.0), e + offset.rem_euclid(1.0));
    // Every sub-path as a polyline with its running length, and its owner.
    let lines: Vec<(usize, Vec<[f64; 2]>, Vec<f64>)> = paths
        .iter()
        .enumerate()
        .flat_map(|(o, subs)| subs.iter().map(move |sub| (o, flatten(sub))))
        .filter(|(_, l)| l.len() > 1)
        .map(|(o, pts)| {
            let mut cum = vec![0.0];
            for w in pts.windows(2) {
                cum.push(cum.last().copied().unwrap_or(0.0) + (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]));
            }
            (o, pts, cum)
        })
        .collect();
    // The kept span may wrap past the end.
    let spans = [(s, e.min(1.0)), (0.0, (e - 1.0).max(0.0))];
    if together {
        for (o, pts, cum) in &lines {
            let total = *cum.last().unwrap_or(&0.0);
            for (a, b) in spans {
                if b - a > 1e-9 && total > 0.0 {
                    out[*o].push(line(cut(pts, cum, a * total, b * total)));
                }
            }
        }
    } else {
        let total: f64 = lines.iter().map(|(_, _, c)| *c.last().unwrap_or(&0.0)).sum();
        let mut at = 0.0;
        for (o, pts, cum) in &lines {
            let len = *cum.last().unwrap_or(&0.0);
            let (a0, a1) = (at / total.max(1e-9), (at + len) / total.max(1e-9));
            at += len;
            for (x, y) in spans {
                let (lo, hi) = (x.max(a0), y.min(a1));
                if hi > lo && len > 0.0 {
                    out[*o].push(line(cut(pts, cum, (lo - a0) * total, (hi - a0) * total)));
                }
            }
        }
    }
    out
}
