// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! From document to rasterizer scene. Live shapes, type, symbols,
//! repeats, blends, envelopes and perspective are worked out here; every
//! fill and stroke of every appearance stack becomes a scene item, and
//! opacity, blend modes, clipping groups, opacity masks and raster effects
//! become layers. Coordinates stay in document space; the renderer maps
//! them to pixels, so art is exact at any resolution.

use std::path::Path;
use std::sync::Arc;

use kurbo::{Affine, BezPath, Point, Rect, Shape};
use studio_vector_geom::PathData;
use studio_vector_raster::{BlendMode, Color, Extend, Filter, ImagePaint, ImageQuality, Layer, Mask, MaskKind, Paint, RenderOptions, Scene};

use crate::appearance::{Appearance, Effect, Item};
use crate::color::tinted;
use crate::doc::{Artboard, Document, Node, NodeKind};
use crate::live;
use crate::text::{self, CharProps};
use crate::FillRule;

/// Options for turning a document into a scene.
#[derive(Clone, Debug, Default)]
pub struct BuildOptions {
    /// Where linked images are looked up.
    pub base_dir: Option<std::path::PathBuf>,
    /// Draw hidden nodes too (for outlines and thumbnails).
    pub show_hidden: bool,
}

/// One piece of a node's own geometry: an outline in the node's space and,
/// for type runs with their own colour, that colour.
#[derive(Clone, Debug)]
pub struct Piece {
    pub path: PathData,
    pub color: Option<Color>,
}

pub(crate) fn bez_to_path(b: &BezPath) -> PathData {
    PathData::from_bezpath(b)
}

/// Character properties of a run with its named style and the node's
/// paragraph style beneath it.
fn run_props(doc: &Document, t: &crate::text::TextNode) -> (CharProps, crate::text::ParaProps, Vec<CharProps>) {
    let para_style = t.para_style.as_deref().and_then(|k| doc.para_styles.iter().find(|s| s.id == k || s.name == k));
    let para = t.para.over(&para_style.map(|s| s.props.clone()).unwrap_or_default());
    let para_char = para_style.and_then(|s| s.char_style.as_deref()).and_then(|k| doc.char_styles.iter().find(|c| c.id == k || c.name == k)).map(|c| c.props.clone()).unwrap_or_default();
    let base = t.char.over(&para_char);
    let runs = t
        .runs
        .iter()
        .map(|r| {
            let named = r.style.as_deref().and_then(|k| doc.char_styles.iter().find(|c| c.id == k || c.name == k)).map(|c| c.props.clone()).unwrap_or_default();
            r.props.over(&named)
        })
        .collect();
    (base, para, runs)
}

/// Outlines of the nodes area type should flow around (siblings above it
/// with `wrap` set), in the text node's space.
fn wrap_obstacles(siblings: &[Node], index: usize, inv: Affine) -> Vec<BezPath> {
    siblings
        .iter()
        .skip(index + 1)
        .filter_map(|n| {
            let off = n.wrap?;
            let b = node_bounds(n)?;
            Some(inv * b.inflate(off, off).to_path(0.1))
        })
        .collect()
}

/// A node's own geometry (before its transform), for leaves.
pub fn geometry(doc: &Document, n: &Node) -> Vec<Piece> {
    geometry_in(doc, n, &[])
}

fn geometry_in(doc: &Document, n: &Node, wrap: &[BezPath]) -> Vec<Piece> {
    match &n.kind {
        NodeKind::Path { path, .. } => vec![Piece { path: path.clone(), color: None }],
        NodeKind::Shape { shape } => vec![Piece { path: shape.to_path(), color: None }],
        NodeKind::Text(t) => {
            let (base, para, runs) = run_props(doc, t);
            text::layout(t, &base, &para, &runs, wrap).parts.into_iter().map(|(c, b)| Piece { path: bez_to_path(&b), color: c }).collect()
        }
        NodeKind::Image(im) => vec![Piece { path: crate::shapes::LiveShape::Rect { rect: Rect::new(0.0, 0.0, im.width, im.height), radii: [0.0; 4], corners: None }.to_path(), color: None }],
        _ => Vec::new(),
    }
}

/// Bounds of a node in its parent's space without a document (symbols and
/// type, which need one, give `None` or the type's box).
pub fn node_bounds(n: &Node) -> Option<Rect> {
    let own = match &n.kind {
        NodeKind::Path { path, .. } => path.bounds(),
        NodeKind::Shape { shape } => shape.to_path().bounds(),
        NodeKind::Image(im) => Some(Rect::new(0.0, 0.0, im.width, im.height)),
        NodeKind::Text(t) => match &t.kind {
            crate::text::TextKind::Area { rect, .. } => Some(*rect),
            crate::text::TextKind::OnPath { path, .. } => path.bounds(),
            crate::text::TextKind::Point { at } => {
                let size = t.char.size.unwrap_or(24.0);
                let w = t.plain().chars().count() as f64 * size * 0.55;
                Some(Rect::new(at.x, at.y - size * 0.8, at.x + w, at.y + size * 0.2))
            }
        },
        _ => n.children().and_then(|c| c.iter().filter_map(node_bounds).reduce(|a, b| a.union(b))),
    }?;
    Some(n.transform.transform_rect_bbox(own))
}

/// Bounds of a node in document space, with symbols and type resolved.
pub fn world_bounds(doc: &Document, id: crate::doc::NodeId) -> Option<Rect> {
    let n = doc.find(id)?;
    let parent = doc.parent_of(id).and_then(|p| doc.world_transform(p)).unwrap_or(Affine::IDENTITY);
    let mut s = Scene::new();
    let b = Builder { doc, opts: BuildOptions { show_hidden: true, ..Default::default() } };
    b.node(n, parent, &mut s, &[], 0);
    s.bounds()
}

struct Builder<'a> {
    doc: &'a Document,
    opts: BuildOptions,
}

impl Builder<'_> {
    fn appearance<'n>(&'n self, n: &'n Node) -> std::borrow::Cow<'n, Appearance> {
        if let Some(st) = n.style.as_deref().and_then(|k| self.doc.styles.iter().find(|s| s.id == k || s.name == k)) {
            return std::borrow::Cow::Borrowed(&st.appearance);
        }
        if n.appearance.items.is_empty() && matches!(n.kind, NodeKind::Text(_)) {
            // Type with no appearance is black, as new type is.
            let mut a = n.appearance.clone();
            a.items.push(Item::Fill(crate::appearance::Fill::new(Paint::solid(Color::BLACK))));
            return std::borrow::Cow::Owned(a);
        }
        std::borrow::Cow::Borrowed(&n.appearance)
    }

    fn paint(&self, paint: &Paint, swatch: Option<&str>, tint: f32) -> Paint {
        if let Some(s) = swatch.and_then(|k| self.doc.swatch(k)) {
            return Paint::solid(tinted(s.color, tint));
        }
        paint.clone()
    }

    fn scene(&self, nodes: &[Node], m: Affine, out: &mut Scene, depth: usize) {
        for (i, n) in nodes.iter().enumerate() {
            self.node(n, m, out, nodes, i);
            let _ = depth;
        }
    }

    /// Adds `n` (drawn through `parent`) to `out`. `siblings` and `index`
    /// locate it for type wrap.
    fn node(&self, n: &Node, parent: Affine, out: &mut Scene, siblings: &[Node], index: usize) {
        if !n.visible && !self.opts.show_hidden {
            return;
        }
        let m = parent * n.transform;
        let ap = self.appearance(n);
        let raster: Vec<&Effect> = ap.effects.iter().filter(|e| e.is_raster()).collect();
        let needs_layer = n.opacity < 1.0 || n.blend != BlendMode::Normal || n.mask.is_some() || !raster.is_empty();
        let mut body = Scene::new();
        let target: &mut Scene = if needs_layer { &mut body } else { out };
        match &n.kind {
            NodeKind::Group { children, clip, .. } => {
                if *clip && !children.is_empty() {
                    // The first child's outline clips the others.
                    let clip_path = self.outline(&children[0], m);
                    let mut inner = Scene::new();
                    self.scene(&children[1..], m, &mut inner, 0);
                    if let Some(cp) = clip_path {
                        target.layer(Layer::new().with_clip(cp, Affine::IDENTITY, studio_vector_raster::FillRule::NonZero), inner);
                    }
                } else {
                    self.scene(children, m, target, 0);
                }
            }
            NodeKind::Instance { symbol, overrides } => {
                if let Some(sym) = self.doc.symbol(symbol) {
                    let mut art = sym.art.clone();
                    if !overrides.is_empty()
                        && let Ok(mut v) = serde_json::to_value(&art)
                    {
                        for (ptr, val) in overrides {
                            if let Some(slot) = v.pointer_mut(ptr) {
                                *slot = val.clone();
                            }
                        }
                        if let Ok(a) = serde_json::from_value(v) {
                            art = a;
                        }
                    }
                    self.scene(&art, m, target, 0);
                }
            }
            NodeKind::Repeat { repeat, children } => {
                let b = children.iter().filter_map(node_bounds).reduce(|a, b| a.union(b)).unwrap_or(Rect::ZERO);
                for t in live::repeat_transforms(repeat, b) {
                    self.scene(children, m * t, target, 0);
                }
            }
            NodeKind::Blend { blend, children } => {
                if children.len() >= 2 {
                    let geom = |k: &Node| -> Option<PathData> {
                        let ps = geometry(self.doc, k);
                        (!ps.is_empty()).then(|| PathData::new(ps.into_iter().flat_map(|p| p.path.subpaths).collect()))
                    };
                    let steps = live::blend_steps(&children[0], &children[children.len() - 1], blend, &geom);
                    self.scene(&steps, m, target, 0);
                } else {
                    self.scene(children, m, target, 0);
                }
            }
            NodeKind::Envelope { envelope, children } => {
                let leaves = self.leaves(children, Affine::IDENTITY);
                let b = leaves.iter().filter_map(|l| l.0.path.bounds()).reduce(|a, b| a.union(b)).unwrap_or(Rect::ZERO);
                let map = live::envelope_map(envelope, b);
                for (piece, node) in leaves {
                    let warped = map(&piece.path);
                    self.leaf_items(&node, &[Piece { path: warped, color: piece.color }], m, target);
                }
            }
            NodeKind::Perspective { plane, children } => {
                let grid = self.doc.perspective.unwrap_or_default();
                for (piece, node) in self.leaves(children, Affine::IDENTITY) {
                    let mapped = live::perspective_map(&grid, *plane, &piece.path);
                    self.leaf_items(&node, &[Piece { path: mapped, color: piece.color }], m, target);
                }
            }
            NodeKind::Image(im) => {
                if let Some(img) = self.image(im) {
                    let (w, h) = (img.width as f64, img.height as f64);
                    let ip = ImagePaint {
                        image: img,
                        transform: Affine::scale_non_uniform(im.width / w, im.height / h),
                        extend: Extend::None,
                        quality: ImageQuality::Bicubic,
                        opacity: 1.0,
                    };
                    target.fill(Rect::new(0.0, 0.0, im.width, im.height).to_path(0.0), m, studio_vector_raster::FillRule::NonZero, Paint::Image(Arc::new(ip)));
                }
                // Images can carry strokes (frames) too.
                let pieces = geometry(self.doc, n);
                let mut frame = n.clone();
                frame.appearance.items.retain(|i| matches!(i, Item::Stroke(_)));
                self.leaf_items(&frame, &pieces, m, target);
            }
            NodeKind::Text(_) => {
                let pieces = geometry_in(self.doc, n, &wrap_obstacles(siblings, index, n.transform.inverse()));
                self.leaf_items(n, &pieces, m, target);
            }
            _ => {
                let pieces = geometry(self.doc, n);
                self.leaf_items(n, &pieces, m, target);
            }
        }
        if needs_layer {
            let mut layer = Layer::new().with_opacity(n.opacity).with_blend(n.blend);
            layer.filter_space = m;
            for e in raster {
                layer.filters.push(match e {
                    Effect::Blur { radius } => Filter::blur(radius / 3.0),
                    Effect::DropShadow { dx, dy, blur, color, spread } => Filter::DropShadow { dx: *dx, dy: *dy, sigma: blur / 3.0, color: *color, spread: *spread },
                    Effect::InnerShadow { dx, dy, blur, color } => Filter::InnerShadow { dx: *dx, dy: *dy, sigma: blur / 3.0, color: *color },
                    Effect::OuterGlow { blur, color, spread } => Filter::OuterGlow { sigma: blur / 3.0, color: *color, spread: *spread },
                    Effect::InnerGlow { blur, color } => Filter::InnerGlow { sigma: blur / 3.0, color: *color },
                    _ => continue,
                });
            }
            if let Some(mask) = &n.mask {
                let mut ms = Scene::new();
                self.scene(&mask.art, m, &mut ms, 0);
                layer.mask = Some(Box::new(Mask { scene: ms, kind: if mask.alpha { MaskKind::Alpha } else { MaskKind::Luminance }, invert: mask.invert }));
            }
            out.layer(layer, body);
        }
    }

    /// Every leaf under `nodes` with its geometry moved into the space the
    /// nodes are in (for envelopes and perspective, which bend everything).
    fn leaves(&self, nodes: &[Node], m: Affine) -> Vec<(Piece, Node)> {
        let mut out = Vec::new();
        for n in nodes {
            if !n.visible && !self.opts.show_hidden {
                continue;
            }
            let mm = m * n.transform;
            match &n.kind {
                NodeKind::Group { children, .. } | NodeKind::Repeat { children, .. } | NodeKind::Blend { children, .. } | NodeKind::Envelope { children, .. } | NodeKind::Perspective { children, .. } => {
                    out.extend(self.leaves(children, mm));
                }
                NodeKind::Instance { symbol, .. } => {
                    if let Some(sym) = self.doc.symbol(symbol) {
                        out.extend(self.leaves(&sym.art, mm));
                    }
                }
                _ => {
                    let mut flat = n.clone();
                    flat.transform = Affine::IDENTITY;
                    for p in geometry(self.doc, n) {
                        out.push((Piece { path: p.path.transformed(mm), color: p.color }, flat.clone()));
                    }
                }
            }
        }
        out
    }

    /// The node's outline in scene space (for clipping), all its pieces together.
    fn outline(&self, n: &Node, parent: Affine) -> Option<BezPath> {
        let m = parent * n.transform;
        let mut out = BezPath::new();
        for (p, _) in self.leaves(std::slice::from_ref(n), Affine::IDENTITY) {
            out.extend(parent * p.path.to_bezpath());
        }
        let _ = m;
        (!out.elements().is_empty()).then_some(out)
    }

    /// The fills and strokes of a leaf's appearance over `pieces`.
    fn leaf_items(&self, n: &Node, pieces: &[Piece], m: Affine, out: &mut Scene) {
        if pieces.is_empty() {
            return;
        }
        let ap = self.appearance(n);
        let rule = match &n.kind {
            NodeKind::Path { rule, .. } => *rule,
            _ => FillRule::NonZero,
        };
        let vector: Vec<Effect> = ap.effects.iter().filter(|e| !e.is_raster()).cloned().collect();
        let geom: Vec<Piece> = pieces.iter().map(|p| Piece { path: if vector.is_empty() { p.path.clone() } else { live::apply_effects(&p.path, &vector) }, color: p.color }).collect();
        for item in &ap.items {
            match item {
                Item::Fill(f) if f.visible => {
                    for p in &geom {
                        let path = if f.effects.is_empty() { p.path.clone() } else { live::apply_effects(&p.path, &f.effects) };
                        let paint = match p.color {
                            Some(c) => Paint::solid(c),
                            None => self.paint(&f.paint, f.swatch.as_deref(), f.tint),
                        };
                        out.fill(path.to_bezpath(), m, rule, paint).blended(f.blend, f.opacity);
                    }
                }
                Item::Stroke(s) if s.visible => {
                    for p in &geom {
                        let path = if s.effects.is_empty() { p.path.clone() } else { live::apply_effects(&p.path, &s.effects) };
                        out.stroke(path.to_bezpath(), m, s.style.clone(), self.paint(&s.paint, s.swatch.as_deref(), s.tint)).blended(s.blend, s.opacity);
                    }
                }
                _ => {}
            }
        }
    }

    fn image(&self, im: &crate::doc::ImageNode) -> Option<studio_vector_raster::Image> {
        if let Some(i) = &im.image {
            return Some(i.clone());
        }
        let href = im.href.as_ref()?;
        let path = match &self.opts.base_dir {
            Some(d) if !Path::new(href).is_absolute() => d.join(href),
            _ => href.into(),
        };
        crate::interop::load_image(&path)
    }
}

/// The whole document as a scene, in document coordinates.
pub fn scene(doc: &Document, opts: &BuildOptions) -> Scene {
    let b = Builder { doc, opts: opts.clone() };
    let mut s = Scene::new();
    b.scene(&doc.layers, Affine::IDENTITY, &mut s, 0);
    s
}

/// The scene of one artboard: its background and the art (still in
/// document coordinates), and the artboard's rectangle.
pub fn artboard_scene(doc: &Document, artboard: Option<&Artboard>, opts: &BuildOptions) -> (Scene, Rect) {
    let rect = artboard.map(|a| a.rect).or_else(|| doc.artboards.first().map(|a| a.rect)).unwrap_or(Rect::new(0.0, 0.0, 1920.0, 1080.0));
    let mut s = Scene::new();
    if let Some(bg) = artboard.or(doc.artboards.first()).and_then(|a| a.background) {
        s.fill(rect.to_path(0.0), Affine::IDENTITY, studio_vector_raster::FillRule::NonZero, Paint::solid(bg));
    }
    let art = scene(doc, opts);
    s.items.extend(art.items);
    (s, rect)
}

/// Render options that draw `rect` (document units) into `width` × `height` pixels.
pub fn fit_options(rect: Rect, width: u32, height: u32) -> RenderOptions {
    RenderOptions::new(width, height).fit(rect)
}

/// Draws an artboard at `width` × `height` on the CPU.
pub fn render_artboard(doc: &Document, artboard: Option<&str>, width: u32, height: u32, opts: &BuildOptions) -> studio_vector_raster::Pixmap {
    let ab = doc.artboard(artboard);
    let (s, rect) = artboard_scene(doc, ab, opts);
    studio_vector_raster::render(&s, &fit_options(rect, width, height))
}

/// The point in document space under pixel (`x`, `y`) of a render made
/// with [`fit_options`].
pub fn unproject(rect: Rect, width: u32, height: u32, x: f64, y: f64) -> Point {
    fit_options(rect, width, height).transform.inverse() * Point::new(x, y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::appearance::{Appearance, Fill};
    use crate::doc::{NodeKind, Repeat};
    use crate::shapes::LiveShape;

    #[test]
    fn a_red_circle_renders_red() {
        let mut d = Document::new(100.0, 100.0);
        let mut n = Node::new(0, NodeKind::Shape { shape: LiveShape::Ellipse { rect: Rect::new(25.0, 25.0, 75.0, 75.0) } });
        n.appearance = Appearance::fill(Color::rgb(1.0, 0.0, 0.0));
        d.add(None, n);
        let p = render_artboard(&d, None, 200, 200, &BuildOptions::default());
        assert_eq!(p.pixel(100, 100), [255, 0, 0, 255]);
        assert_eq!(p.pixel(5, 5), [255, 255, 255, 255]);
    }

    #[test]
    fn swatches_styles_repeats_and_clips() {
        let mut d = Document::new(100.0, 100.0);
        d.set_swatch(crate::color::Swatch { id: "brand".into(), name: "Brand".into(), color: Color::rgb(0.0, 0.0, 1.0), global: true, group: String::new() });
        let mut f = Fill::new(Color::BLACK);
        f.swatch = Some("brand".into());
        let mut n = Node::new(0, NodeKind::Shape { shape: LiveShape::Rect { rect: Rect::new(0.0, 0.0, 10.0, 10.0), radii: [0.0; 4], corners: None } });
        n.appearance.items.push(Item::Fill(f));
        let rep = Node::new(0, NodeKind::Repeat { repeat: Repeat::Grid { rows: 2, cols: 3, gap: (5.0, 5.0), offset_rows: false }, children: vec![n] });
        d.add(None, rep);
        let p = render_artboard(&d, None, 100, 100, &BuildOptions::default());
        // Copies at x = 0, 15, 30 and y = 0, 15, all blue.
        assert_eq!(p.pixel(35, 20), [0, 0, 255, 255]);
        assert_eq!(p.pixel(12, 5), [255, 255, 255, 255]);
    }
}
