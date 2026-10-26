// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The step both renderers share: a [`Scene`] becomes device-space line
//! segments, resolved paints and a tree of isolated groups. Layers with
//! effects, opacity masks, patterns and mesh gradients become *prepasses*
//! (images drawn first and then used as paints), so the main pass only
//! ever fills, blends and clips.

use std::sync::Arc;

use kurbo::{Affine, BezPath, CubicBez, ParamCurveExtrema, PathEl, Point, QuadBez, Shape};

use crate::color::Color;
use crate::paint::{Extend, GradientKind, Image, ImageQuality, Lut, Paint};
use crate::scene::{BlendMode, Clip, FillRule, Filter, Item, Layer, MaskKind, Scene, StrokeAlign, StrokeStyle};
use crate::stroke::stroke_outline;
use crate::RenderOptions;

/// A pixel rectangle, `x0..x1` × `y0..y1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) struct IRect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl IRect {
    pub fn new(x0: i32, y0: i32, x1: i32, y1: i32) -> IRect {
        IRect { x0, y0, x1, y1 }
    }

    pub fn wh(w: u32, h: u32) -> IRect {
        IRect::new(0, 0, w as i32, h as i32)
    }

    pub fn is_empty(&self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }

    pub fn width(&self) -> i32 {
        (self.x1 - self.x0).max(0)
    }

    pub fn height(&self) -> i32 {
        (self.y1 - self.y0).max(0)
    }

    pub fn intersect(&self, o: &IRect) -> IRect {
        IRect::new(self.x0.max(o.x0), self.y0.max(o.y0), self.x1.min(o.x1), self.y1.min(o.y1))
    }

    pub fn union(&self, o: &IRect) -> IRect {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        IRect::new(self.x0.min(o.x0), self.y0.min(o.y0), self.x1.max(o.x1), self.y1.max(o.y1))
    }

    pub fn inflate(&self, d: i32) -> IRect {
        IRect::new(self.x0 - d, self.y0 - d, self.x1 + d, self.y1 + d)
    }
}

/// A fill's geometry: device-space line segments `[x0, y0, x1, y1]`
/// (horizontal ones dropped), their pixel bounds and the fill rule.
#[derive(Clone, Debug)]
pub(crate) struct Lines {
    pub segs: Vec<[f32; 4]>,
    pub bbox: IRect,
    pub rule: FillRule,
    /// Covers everywhere the path does not (an "outside" clip).
    pub invert: bool,
}

/// Where an image paint's pixels come from.
#[derive(Clone, Debug)]
pub(crate) enum ImgSrc {
    User(Image),
    /// A prepass image (index into `Prepared::prepasses`).
    Prepass(usize),
}

/// A paint resolved to device space. Gradient and image paints carry the
/// map from device pixels to their own space.
#[derive(Clone, Debug)]
pub(crate) enum PPaint {
    /// Premultiplied colour.
    Solid([f32; 4]),
    /// `t = x` of `inv * p`.
    Linear { inv: [f32; 6], lut: Arc<Lut>, extend: Extend },
    Radial { inv: [f32; 6], lut: Arc<Lut>, extend: Extend, c0: [f32; 2], r0: f32, c1: [f32; 2], r1: f32 },
    /// `t = atan2(y, x) / 2π` of `inv * p`.
    Conic { inv: [f32; 6], lut: Arc<Lut>, extend: Extend },
    /// Device-space points, premultiplied colours and weights.
    Freeform { pts: Vec<([f32; 2], [f32; 4], f32)> },
    Image { src: ImgSrc, inv: [f32; 6], extend: Extend, quality: ImageQuality, opacity: f32, lod: f32 },
}

#[derive(Clone, Debug)]
pub(crate) struct PDraw {
    pub lines: Lines,
    pub paint: PPaint,
    pub blend: BlendMode,
    pub opacity: f32,
}

#[derive(Clone, Debug)]
pub(crate) struct PMask {
    pub prepass: usize,
    pub kind: MaskKind,
    pub invert: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct PGroup {
    pub blend: BlendMode,
    pub opacity: f32,
    pub clip: Option<Lines>,
    pub mask: Option<PMask>,
    pub children: Vec<PNode>,
    /// Pixels the group can touch.
    pub bbox: IRect,
}

impl PGroup {
    fn root(bbox: IRect) -> PGroup {
        PGroup { blend: BlendMode::Normal, opacity: 1.0, clip: None, mask: None, children: Vec::new(), bbox }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum PNode {
    Draw(PDraw),
    Group(PGroup),
}

/// Effects in device pixels.
#[derive(Clone, Debug)]
pub(crate) enum PFilter {
    Blur { sx: f32, sy: f32 },
    DropShadow { dx: f32, dy: f32, sigma: f32, color: [f32; 4], spread: f32 },
    InnerShadow { dx: f32, dy: f32, sigma: f32, color: [f32; 4] },
    OuterGlow { sigma: f32, color: [f32; 4], spread: f32 },
    InnerGlow { sigma: f32, color: [f32; 4] },
    Opacity { amount: f32 },
}

/// An image drawn before the main pass. Its pixels cover `region` of its
/// own coordinate space (device pixels for layers and masks, tile pixels
/// for patterns).
#[derive(Clone, Debug)]
pub(crate) enum Prepass {
    Scene { region: IRect, root: PGroup, filters: Vec<PFilter> },
    /// Pixels made while preparing (mesh gradients), premultiplied.
    Ready { region: IRect, pixels: Arc<Vec<[f32; 4]>> },
}

impl Prepass {
    pub fn region(&self) -> IRect {
        match self {
            Prepass::Scene { region, .. } | Prepass::Ready { region, .. } => *region,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Prepared {
    pub root: PGroup,
    pub prepasses: Vec<Prepass>,
    pub region: IRect,
    pub background: Option<[f32; 4]>,
}

/// Largest side of a pattern tile image.
const MAX_TILE: f64 = 4096.0;

pub(crate) fn prepare(scene: &Scene, opts: &RenderOptions) -> Prepared {
    let region = IRect::wh(opts.width, opts.height);
    let mut p = Preparer { tolerance: opts.tolerance.clamp(0.001, 2.0), prepasses: Vec::new() };
    let mut root = PGroup::root(region);
    p.scene(scene, opts.transform, region, &mut root.children);
    Prepared { root, prepasses: p.prepasses, region, background: opts.background.map(Color::premul) }
}

struct Preparer {
    tolerance: f64,
    prepasses: Vec<Prepass>,
}

fn coeffs(a: Affine) -> [f32; 6] {
    a.as_coeffs().map(|v| v as f32)
}

fn max_scale(m: Affine) -> f64 {
    let c = m.as_coeffs();
    let (sx, sy) = ((c[0] * c[0] + c[1] * c[1]).sqrt(), (c[2] * c[2] + c[3] * c[3]).sqrt());
    sx.max(sy).max(1e-12)
}

fn finite(a: Affine) -> bool {
    a.as_coeffs().iter().all(|v| v.is_finite()) && a.determinant().abs() > 1e-18
}

impl Preparer {
    fn scene(&mut self, scene: &Scene, m: Affine, region: IRect, out: &mut Vec<PNode>) {
        for item in &scene.items {
            match item {
                Item::Fill { path, transform, rule, paint, blend, opacity } => {
                    if paint.is_invisible() || *opacity <= 0.0 {
                        continue;
                    }
                    let mm = m * *transform;
                    if !finite(mm) {
                        continue;
                    }
                    let lines = self.lines(path, mm, *rule, region);
                    if let Some(lines) = lines {
                        self.draw(lines, paint, mm, *blend, *opacity, out);
                    }
                }
                Item::Stroke { path, transform, style, paint, blend, opacity } => {
                    if paint.is_invisible() || *opacity <= 0.0 {
                        continue;
                    }
                    let mm = m * *transform;
                    if !finite(mm) {
                        continue;
                    }
                    self.stroke(path, mm, style, paint, *blend, *opacity, region, out);
                }
                Item::Layer { layer, scene } => self.layer(layer, scene, m, region, out),
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn stroke(&mut self, path: &BezPath, mm: Affine, style: &StrokeStyle, paint: &Paint, blend: BlendMode, opacity: f32, region: IRect, out: &mut Vec<PNode>) {
        let closed = path.elements().iter().any(|e| matches!(e, PathEl::ClosePath));
        let mut st = style.clone();
        let aligned = closed && st.align != StrokeAlign::Center;
        if aligned {
            st.width *= 2.0;
        }
        let (outline, outline_m) = if style.non_scaling {
            let dev = mm * path.clone();
            (stroke_outline(&dev, &st, self.tolerance * 0.5), Affine::IDENTITY)
        } else {
            (stroke_outline(path, &st, self.tolerance * 0.5 / max_scale(mm)), mm)
        };
        let Some(lines) = self.lines(&outline, outline_m, FillRule::NonZero, region) else { return };
        if !aligned {
            self.draw(lines, paint, mm, blend, opacity, out);
            return;
        }
        // Inside / outside strokes: twice as wide, clipped by the path's own
        // area (or everything but it).
        let Some(mut clip) = self.lines(path, mm, FillRule::NonZero, region.inflate(1)) else {
            if style.align == StrokeAlign::Outside {
                self.draw(lines, paint, mm, blend, opacity, out);
            }
            return;
        };
        clip.invert = style.align == StrokeAlign::Outside;
        let bbox = lines.bbox;
        let mut kids = Vec::new();
        self.draw(lines, paint, mm, BlendMode::Normal, 1.0, &mut kids);
        out.push(PNode::Group(PGroup { blend, opacity, clip: Some(clip), mask: None, children: kids, bbox }));
    }

    fn draw(&mut self, lines: Lines, paint: &Paint, mm: Affine, blend: BlendMode, opacity: f32, out: &mut Vec<PNode>) {
        if let Some(paint) = self.paint(paint, mm, lines.bbox) {
            out.push(PNode::Draw(PDraw { lines, paint, blend, opacity: opacity.clamp(0.0, 1.0) }));
        }
    }

    fn layer(&mut self, layer: &Layer, scene: &Scene, m: Affine, region: IRect, out: &mut Vec<PNode>) {
        if layer.opacity <= 0.0 {
            return;
        }
        // The pixels the layer's own art may cover, then what its clip allows.
        let mut clip_lines = None;
        let mut limit = region;
        if let Some(Clip { path, transform, rule }) = &layer.clip {
            let cm = m * *transform;
            if !finite(cm) {
                return;
            }
            match self.lines(path, cm, *rule, region) {
                Some(l) => {
                    limit = limit.intersect(&l.bbox);
                    clip_lines = Some(l);
                }
                None => return,
            }
        }
        let filters: Vec<PFilter> = self.filters(&layer.filters, m * layer.filter_space);
        let reach = filters.iter().map(filter_reach).sum::<f32>().ceil() as i32;
        let mut kids = Vec::new();
        if filters.is_empty() {
            self.scene(scene, m, limit, &mut kids);
        } else {
            // Art outside the canvas can blur or cast shadows into it.
            let area = limit.inflate(reach);
            let mut inner = Vec::new();
            self.scene(scene, m, area, &mut inner);
            let content = bbox_of(&inner);
            if content.is_empty() {
                return;
            }
            let pre_region = content.inflate(reach).intersect(&area);
            if pre_region.is_empty() {
                return;
            }
            let mut root = PGroup::root(pre_region);
            root.children = inner;
            self.prepasses.push(Prepass::Scene { region: pre_region, root, filters });
            let idx = self.prepasses.len() - 1;
            let shown = pre_region.intersect(&limit);
            if shown.is_empty() {
                return;
            }
            kids.push(PNode::Draw(PDraw {
                lines: rect_lines(shown),
                paint: PPaint::Image {
                    src: ImgSrc::Prepass(idx),
                    inv: coeffs(Affine::translate((-pre_region.x0 as f64, -pre_region.y0 as f64))),
                    extend: Extend::None,
                    quality: ImageQuality::Nearest,
                    opacity: 1.0,
                    lod: 1.0,
                },
                blend: BlendMode::Normal,
                opacity: 1.0,
            }));
        }
        let bbox = bbox_of(&kids).intersect(&limit);
        if bbox.is_empty() {
            return;
        }
        let mask = layer.mask.as_ref().and_then(|mk| {
            let mut mkids = Vec::new();
            self.scene(&mk.scene, m, bbox, &mut mkids);
            let mut root = PGroup::root(bbox);
            root.children = mkids;
            self.prepasses.push(Prepass::Scene { region: bbox, root, filters: Vec::new() });
            Some(PMask { prepass: self.prepasses.len() - 1, kind: mk.kind, invert: mk.invert })
        });
        let plain = layer.blend == BlendMode::Normal && layer.opacity >= 1.0 && clip_lines.is_none() && mask.is_none();
        if plain {
            out.extend(kids);
            return;
        }
        out.push(PNode::Group(PGroup { blend: layer.blend, opacity: layer.opacity.min(1.0), clip: clip_lines, mask, children: kids, bbox }));
    }

    fn filters(&self, fs: &[Filter], m: Affine) -> Vec<PFilter> {
        let c = m.as_coeffs();
        let (sx, sy) = ((c[0] * c[0] + c[1] * c[1]).sqrt(), (c[2] * c[2] + c[3] * c[3]).sqrt());
        let s = (sx * sy).sqrt();
        let off = |dx: f64, dy: f64| {
            let c = m.as_coeffs();
            let v = kurbo::Vec2::new(c[0] * dx + c[2] * dy, c[1] * dx + c[3] * dy);
            (v.x as f32, v.y as f32)
        };
        let col = |c: &Color| c.premul();
        fs.iter()
            .filter_map(|f| {
                Some(match f {
                    Filter::Blur { sigma_x, sigma_y } => PFilter::Blur { sx: (sigma_x * sx) as f32, sy: (sigma_y * sy) as f32 },
                    Filter::DropShadow { dx, dy, sigma, color, spread } => {
                        let (dx, dy) = off(*dx, *dy);
                        PFilter::DropShadow { dx, dy, sigma: (sigma * s) as f32, color: col(color), spread: (spread * s) as f32 }
                    }
                    Filter::InnerShadow { dx, dy, sigma, color } => {
                        let (dx, dy) = off(*dx, *dy);
                        PFilter::InnerShadow { dx, dy, sigma: (sigma * s) as f32, color: col(color) }
                    }
                    Filter::OuterGlow { sigma, color, spread } => PFilter::OuterGlow { sigma: (sigma * s) as f32, color: col(color), spread: (spread * s) as f32 },
                    Filter::InnerGlow { sigma, color } => PFilter::InnerGlow { sigma: (sigma * s) as f32, color: col(color) },
                    Filter::Opacity { amount } => PFilter::Opacity { amount: amount.clamp(0.0, 1.0) },
                })
            })
            .filter(|f| match f {
                PFilter::Blur { sx, sy } => sx.is_finite() && sy.is_finite(),
                _ => true,
            })
            .collect()
    }

    /// Resolves a paint for a shape drawn through `mm`. `None` when it
    /// cannot show anything.
    fn paint(&mut self, paint: &Paint, mm: Affine, bbox: IRect) -> Option<PPaint> {
        Some(match paint {
            Paint::Solid { color } => PPaint::Solid(color.premul()),
            Paint::Gradient(g) => {
                let gm = mm * g.transform;
                if !finite(gm) || g.stops.is_empty() {
                    // A degenerate gradient shows its last colour, as SVG does.
                    return g.stops.last().map(|s| PPaint::Solid(s.color.premul()));
                }
                let lut = g.lut();
                match g.kind {
                    GradientKind::Linear { start, end } => {
                        let d = end - start;
                        let l2 = d.hypot2();
                        if l2 < 1e-18 {
                            return g.stops.last().map(|s| PPaint::Solid(s.color.premul()));
                        }
                        // Gradient space -> unit space with start at 0 and end at (1, 0).
                        let to_unit = Affine::new([d.x / l2, -d.y / l2, d.y / l2, d.x / l2, 0.0, 0.0]) * Affine::translate(-start.to_vec2());
                        PPaint::Linear { inv: coeffs(to_unit * gm.inverse()), lut, extend: g.extend }
                    }
                    GradientKind::Radial { c0, r0, c1, r1 } => {
                        if r0 < 0.0 || r1 < 0.0 || (r0 == r1 && c0 == c1) {
                            return g.stops.last().map(|s| PPaint::Solid(s.color.premul()));
                        }
                        PPaint::Radial {
                            inv: coeffs(gm.inverse()),
                            lut,
                            extend: g.extend,
                            c0: [c0.x as f32, c0.y as f32],
                            r0: r0 as f32,
                            c1: [c1.x as f32, c1.y as f32],
                            r1: r1 as f32,
                        }
                    }
                    GradientKind::Conic { center, angle } => {
                        let to_local = Affine::rotate(-angle) * Affine::translate(-center.to_vec2());
                        PPaint::Conic { inv: coeffs(to_local * gm.inverse()), lut, extend: g.extend }
                    }
                }
            }
            Paint::Freeform(f) => {
                let fm = mm * f.transform;
                let pts: Vec<([f32; 2], [f32; 4], f32)> = f
                    .points
                    .iter()
                    .take(64)
                    .map(|p| {
                        let q = fm * p.at;
                        ([q.x as f32, q.y as f32], p.color.premul(), p.spread.max(0.01))
                    })
                    .collect();
                match pts.len() {
                    0 => return None,
                    1 => PPaint::Solid(pts[0].1),
                    _ => PPaint::Freeform { pts },
                }
            }
            Paint::Mesh(mesh) => {
                let region = bbox;
                if region.is_empty() || mesh.patches.is_empty() {
                    return None;
                }
                let pixels = crate::mesh::rasterize(mesh, mm * mesh.transform, region);
                self.prepasses.push(Prepass::Ready { region, pixels: Arc::new(pixels) });
                PPaint::Image {
                    src: ImgSrc::Prepass(self.prepasses.len() - 1),
                    inv: coeffs(Affine::translate((-region.x0 as f64, -region.y0 as f64))),
                    extend: Extend::None,
                    quality: ImageQuality::Nearest,
                    opacity: 1.0,
                    lod: 1.0,
                }
            }
            Paint::Image(ip) => {
                let im = mm * ip.transform;
                if !finite(im) || ip.opacity <= 0.0 {
                    return None;
                }
                let inv = im.inverse();
                let lod = (inv.determinant().abs().sqrt()) as f32;
                PPaint::Image { src: ImgSrc::User(ip.image.clone()), inv: coeffs(inv), extend: ip.extend, quality: ip.quality, opacity: ip.opacity.min(1.0), lod }
            }
            Paint::Pattern(pat) => {
                let pm = mm * pat.transform;
                let r = pat.rect;
                if !finite(pm) || !(r.width() > 0.0 && r.height() > 0.0) {
                    return None;
                }
                let c = pm.as_coeffs();
                let (sx, sy) = ((c[0] * c[0] + c[1] * c[1]).sqrt(), (c[2] * c[2] + c[3] * c[3]).sqrt());
                let w = (r.width() * sx).ceil().clamp(1.0, MAX_TILE);
                let h = (r.height() * sy).ceil().clamp(1.0, MAX_TILE);
                let to_img = Affine::scale_non_uniform(w / r.width(), h / r.height()) * Affine::translate((-r.x0, -r.y0));
                let region = IRect::wh(w as u32, h as u32);
                let mut root = PGroup::root(region);
                self.scene(&pat.tile, to_img, region, &mut root.children);
                self.prepasses.push(Prepass::Scene { region, root, filters: Vec::new() });
                let img_to_dev = pm * to_img.inverse();
                PPaint::Image {
                    src: ImgSrc::Prepass(self.prepasses.len() - 1),
                    inv: coeffs(img_to_dev.inverse()),
                    extend: Extend::Repeat,
                    quality: ImageQuality::Bilinear,
                    opacity: 1.0,
                    lod: 1.0,
                }
            }
        })
    }

    /// Flattens `path` through `m` into device segments, keeping only what
    /// can affect `region`. `None` if nothing can show.
    fn lines(&self, path: &BezPath, m: Affine, rule: FillRule, region: IRect) -> Option<Lines> {
        let (segs, reaches_right) = flatten(path, m, self.tolerance, region);
        if segs.is_empty() {
            return None;
        }
        let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        for s in &segs {
            b[0] = b[0].min(s[0].min(s[2]));
            b[1] = b[1].min(s[1].min(s[3]));
            b[2] = b[2].max(s[0].max(s[2]));
            b[3] = b[3].max(s[1].max(s[3]));
        }
        // Edges dropped right of the region still bound coverage that reaches it.
        let x1 = if reaches_right { region.x1 } else { b[2].ceil() as i32 };
        let bbox = IRect::new(b[0].floor() as i32, b[1].floor() as i32, x1, b[3].ceil() as i32).intersect(&region);
        if bbox.is_empty() {
            return None;
        }
        Some(Lines { segs, bbox, rule, invert: false })
    }
}

fn filter_reach(f: &PFilter) -> f32 {
    match f {
        PFilter::Blur { sx, sy } => 3.0 * sx.max(*sy),
        PFilter::DropShadow { dx, dy, sigma, spread, .. } => 3.0 * sigma + dx.abs().max(dy.abs()) + spread.max(0.0),
        PFilter::OuterGlow { sigma, spread, .. } => 3.0 * sigma + spread.max(0.0),
        _ => 0.0,
    }
}

pub(crate) fn bbox_of(nodes: &[PNode]) -> IRect {
    nodes.iter().fold(IRect::default(), |acc, n| {
        acc.union(&match n {
            PNode::Draw(d) => d.lines.bbox,
            PNode::Group(g) => g.bbox,
        })
    })
}

pub(crate) fn rect_lines(r: IRect) -> Lines {
    let (x0, y0, x1, y1) = (r.x0 as f32, r.y0 as f32, r.x1 as f32, r.y1 as f32);
    Lines { segs: vec![[x0, y0, x0, y1], [x1, y1, x1, y0]], bbox: r, rule: FillRule::NonZero, invert: false }
}

/// Device-space segments of `path` through `m`. Subpaths close
/// implicitly (fills always do). Curves that cannot reach `region`'s pixels
/// are dropped, and curves wholly to its left (which still count for the
/// winding of pixels to their right) become y-monotone chords.
pub(crate) fn flatten(path: &BezPath, m: Affine, tolerance: f64, region: IRect) -> (Vec<[f32; 4]>, bool) {
    let right = std::cell::Cell::new(false);
    let (rx0, ry0, rx1, ry1) = (region.x0 as f64 - 1.0, region.y0 as f64 - 1.0, region.x1 as f64 + 1.0, region.y1 as f64 + 1.0);
    let mut out: Vec<[f32; 4]> = Vec::new();
    let push = |a: Point, b: Point, out: &mut Vec<[f32; 4]>| {
        if a.y == b.y || !(a.x.is_finite() && a.y.is_finite() && b.x.is_finite() && b.y.is_finite()) {
            return;
        }
        if a.y.max(b.y) <= ry0 || a.y.min(b.y) >= ry1 {
            return;
        }
        if a.x.min(b.x) >= rx1 {
            right.set(true);
            return;
        }
        out.push([a.x as f32, a.y as f32, b.x as f32, b.y as f32]);
    };
    // Classifies a curve by its control box: 0 = may show, 1 = cannot
    // affect the region, 2 = wholly left of it.
    let class = |pts: &[Point]| -> u8 {
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for p in pts {
            x0 = x0.min(p.x);
            y0 = y0.min(p.y);
            x1 = x1.max(p.x);
            y1 = y1.max(p.y);
        }
        if y1 <= ry0 || y0 >= ry1 {
            1
        } else if x0 >= rx1 {
            right.set(true);
            1
        } else if x1 <= rx0 {
            2
        } else {
            0
        }
    };
    let mut start = Point::ZERO;
    let mut cur = Point::ZERO;
    let mut open = false;
    let flat = |seg: &[PathEl], out: &mut Vec<[f32; 4]>, from: Point| {
        let mut last = from;
        let mut mini = BezPath::new();
        mini.move_to(from);
        for e in seg {
            mini.push(*e);
        }
        kurbo::flatten(mini.iter(), tolerance, |el| {
            if let PathEl::LineTo(p) = el {
                push(last, p, out);
                last = p;
            }
        });
    };
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                if open && cur != start {
                    push(m * cur, m * start, &mut out);
                }
                start = p;
                cur = p;
                open = true;
            }
            PathEl::LineTo(p) => {
                push(m * cur, m * p, &mut out);
                cur = p;
            }
            PathEl::QuadTo(c, p) => {
                let pts = [m * cur, m * c, m * p];
                match class(&pts) {
                    0 => flat(&[PathEl::QuadTo(pts[1], pts[2])], &mut out, pts[0]),
                    2 => {
                        let q = QuadBez::new(pts[0], pts[1], pts[2]);
                        monotone_chords(&q.extrema(), |t| kurbo::ParamCurve::eval(&q, t), pts[0], pts[2], &mut |a, b| push(a, b, &mut out));
                    }
                    _ => {}
                }
                cur = p;
            }
            PathEl::CurveTo(c1, c2, p) => {
                let pts = [m * cur, m * c1, m * c2, m * p];
                match class(&pts) {
                    0 => flat(&[PathEl::CurveTo(pts[1], pts[2], pts[3])], &mut out, pts[0]),
                    2 => {
                        let c = CubicBez::new(pts[0], pts[1], pts[2], pts[3]);
                        monotone_chords(&c.extrema(), |t| kurbo::ParamCurve::eval(&c, t), pts[0], pts[3], &mut |a, b| push(a, b, &mut out));
                    }
                    _ => {}
                }
                cur = p;
            }
            PathEl::ClosePath => {
                if cur != start {
                    push(m * cur, m * start, &mut out);
                }
                cur = start;
            }
        }
    }
    if open && cur != start {
        push(m * cur, m * start, &mut out);
    }
    (out, right.get())
}

fn monotone_chords(ts: &[f64], eval: impl Fn(f64) -> Point, a: Point, b: Point, push: &mut impl FnMut(Point, Point)) {
    let mut last = a;
    for &t in ts {
        let p = eval(t);
        push(last, p);
        last = p;
    }
    push(last, b);
}

/// The area a path covers (for tests and tools): the filled area under its
/// fill rule, measured by rasterizing at `scale` pixels per unit.
pub fn covered_area(path: &BezPath, rule: FillRule) -> f64 {
    let b = path.bounding_box();
    if !(b.width() > 0.0 && b.height() > 0.0) {
        return 0.0;
    }
    let s = 512.0 / b.width().max(b.height());
    let m = Affine::scale(s) * Affine::translate((-b.x0, -b.y0));
    let w = (b.width() * s).ceil() as u32 + 1;
    let h = (b.height() * s).ceil() as u32 + 1;
    let (segs, _) = flatten(path, m, 0.01, IRect::wh(w, h));
    let lines = Lines { segs, bbox: IRect::wh(w, h), rule, invert: false };
    let cov = crate::cpu::coverage_full(&lines, IRect::wh(w, h));
    cov.iter().map(|&c| c as f64).sum::<f64>() / (s * s)
}
