// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! SVG in and out. Import reads any SVG (through usvg, which resolves CSS,
//! `use`, units and inheritance) into editable groups, paths, gradients,
//! clipping groups, masks, blurs and images. Export writes the document
//! as plain SVG 1.1 with gradients, clip paths, masks, blend modes and
//! filters; live constructions are written as the art they make, type as
//! outlines (with the text kept in a `<title>` for accessibility).

use std::fmt::Write;
use std::sync::Arc;

use kurbo::{Affine, BezPath, PathEl, Point, Rect};
use studio_vector_geom::PathData;
use studio_vector_raster::{BlendMode, Cap, Color, Extend, Gradient, GradientKind, Join, Paint, Stop, StrokeStyle};

use crate::appearance::{Appearance, Effect, Fill, Item, Stroke};
use crate::build::{self, BuildOptions};
use crate::doc::{Document, ImageNode, Node, NodeKind, OpacityMask};
use crate::FillRule;

// ------------------------------------------------------------------ import

fn affine(t: usvg::Transform) -> Affine {
    Affine::new([t.sx as f64, t.ky as f64, t.kx as f64, t.sy as f64, t.tx as f64, t.ty as f64])
}

fn bez(p: &usvg::tiny_skia_path::Path) -> BezPath {
    use usvg::tiny_skia_path::PathSegment as S;
    let pt = |p: usvg::tiny_skia_path::Point| Point::new(p.x as f64, p.y as f64);
    let mut b = BezPath::new();
    for s in p.segments() {
        match s {
            S::MoveTo(p) => b.move_to(pt(p)),
            S::LineTo(p) => b.line_to(pt(p)),
            S::QuadTo(c, p) => b.quad_to(pt(c), pt(p)),
            S::CubicTo(c1, c2, p) => b.curve_to(pt(c1), pt(c2), pt(p)),
            S::Close => b.close_path(),
        }
    }
    b
}

fn ucolor(c: usvg::Color, o: f32) -> Color {
    Color::rgba(c.red as f32 / 255.0, c.green as f32 / 255.0, c.blue as f32 / 255.0, o)
}

fn stops(s: &[usvg::Stop], opacity: f32) -> Vec<Stop> {
    s.iter().map(|s| Stop::new(s.offset().get(), ucolor(s.color(), s.opacity().get() * opacity))).collect()
}

fn spread(s: usvg::SpreadMethod) -> Extend {
    match s {
        usvg::SpreadMethod::Pad => Extend::Pad,
        usvg::SpreadMethod::Reflect => Extend::Reflect,
        usvg::SpreadMethod::Repeat => Extend::Repeat,
    }
}

struct Importer<'a> {
    doc: &'a mut Document,
}

impl Importer<'_> {
    fn paint(&mut self, p: &usvg::Paint, opacity: f32) -> Paint {
        match p {
            usvg::Paint::Color(c) => Paint::solid(ucolor(*c, opacity)),
            usvg::Paint::LinearGradient(g) => Paint::gradient(
                Gradient::linear((g.x1() as f64, g.y1() as f64), (g.x2() as f64, g.y2() as f64), stops(g.stops(), opacity)).with_extend(spread(g.spread_method())).with_transform(affine(g.transform())),
            ),
            usvg::Paint::RadialGradient(g) => {
                let kind = GradientKind::Radial { c0: Point::new(g.fx() as f64, g.fy() as f64), r0: g.fr().get() as f64, c1: Point::new(g.cx() as f64, g.cy() as f64), r1: g.r().get() as f64 };
                Paint::gradient(Gradient::new(kind, stops(g.stops(), opacity)).with_extend(spread(g.spread_method())).with_transform(affine(g.transform())))
            }
            usvg::Paint::Pattern(pat) => {
                // The tile's art drawn through the raster pattern paint.
                let r = pat.rect();
                let rect = Rect::new(r.left() as f64, r.top() as f64, r.right() as f64, r.bottom() as f64);
                let mut tmp = Document::new(rect.width(), rect.height());
                tmp.layers.clear();
                let mut imp = Importer { doc: &mut tmp };
                let nodes = imp.children(pat.root());
                tmp.layers = vec![Node::new(1, NodeKind::Group { children: nodes, clip: false, layer: true })];
                tmp.fix_ids();
                let tile = build::scene(&tmp, &BuildOptions::default());
                Paint::Pattern(Arc::new(studio_vector_raster::Pattern { tile, rect, transform: affine(pat.transform()) }))
            }
        }
    }

    fn path(&mut self, p: &usvg::Path) -> Option<Node> {
        let mut ap = Appearance::default();
        let mut rule = FillRule::NonZero;
        let mut fill_item = None;
        if let Some(f) = p.fill() {
            rule = if f.rule() == usvg::FillRule::EvenOdd { FillRule::EvenOdd } else { FillRule::NonZero };
            fill_item = Some(Item::Fill(Fill::new(self.paint(f.paint(), f.opacity().get()))));
        }
        let mut stroke_item = None;
        if let Some(s) = p.stroke() {
            let mut st = StrokeStyle::new(s.width().get() as f64);
            st.join = match s.linejoin() {
                usvg::LineJoin::Miter | usvg::LineJoin::MiterClip => Join::Miter,
                usvg::LineJoin::Round => Join::Round,
                usvg::LineJoin::Bevel => Join::Bevel,
            };
            let cap = match s.linecap() {
                usvg::LineCap::Butt => Cap::Butt,
                usvg::LineCap::Round => Cap::Round,
                usvg::LineCap::Square => Cap::Square,
            };
            st.start_cap = cap;
            st.end_cap = cap;
            st.miter_limit = s.miterlimit().get() as f64;
            if let Some(d) = s.dasharray() {
                st.dashes = d.iter().map(|v| *v as f64).collect();
                st.dash_offset = s.dashoffset() as f64;
            }
            let mut sk = Stroke::new(self.paint(s.paint(), s.opacity().get()), st.width);
            sk.style = st;
            stroke_item = Some(Item::Stroke(sk));
        }
        match p.paint_order() {
            usvg::PaintOrder::FillAndStroke => ap.items.extend(fill_item.into_iter().chain(stroke_item)),
            usvg::PaintOrder::StrokeAndFill => ap.items.extend(stroke_item.into_iter().chain(fill_item)),
        }
        let mut n = Node::new(self.doc.new_id(), NodeKind::Path { path: PathData::from_bezpath(&bez(p.data())), rule });
        n.name = p.id().to_string();
        n.visible = p.is_visible();
        n.appearance = ap;
        Some(n)
    }

    fn image(&mut self, im: &usvg::Image) -> Option<Node> {
        let size = im.size();
        let data = match im.kind() {
            usvg::ImageKind::PNG(d) | usvg::ImageKind::JPEG(d) | usvg::ImageKind::GIF(d) | usvg::ImageKind::WEBP(d) => super::decode_image(d)?,
            usvg::ImageKind::SVG(tree) => {
                // A nested SVG becomes a group of its art.
                let mut g = Node::new(self.doc.new_id(), NodeKind::Group { children: Vec::new(), clip: false, layer: false });
                let kids = self.children(tree.root());
                if let Some(c) = g.children_mut() {
                    *c = kids;
                }
                return Some(g);
            }
        };
        let mut n = Node::new(self.doc.new_id(), NodeKind::Image(ImageNode { image: Some(data), href: None, width: size.width() as f64, height: size.height() as f64 }));
        n.name = im.id().to_string();
        Some(n)
    }

    fn children(&mut self, g: &usvg::Group) -> Vec<Node> {
        let mut out = Vec::new();
        for n in g.children() {
            let node = match n {
                usvg::Node::Path(p) => self.path(p),
                usvg::Node::Group(sub) => self.group(sub),
                usvg::Node::Image(im) => self.image(im),
                usvg::Node::Text(t) => {
                    let mut g = self.group(t.flattened());
                    if let Some(g) = &mut g {
                        g.name = format!("Type: {}", t.chunks().iter().map(|c| c.text()).collect::<Vec<_>>().join(" "));
                    }
                    g
                }
            };
            out.extend(node);
        }
        out
    }

    fn group(&mut self, g: &usvg::Group) -> Option<Node> {
        let mut kids = self.children(g);
        let mut clip = false;
        if let Some(cp) = g.clip_path() {
            // A clipping group: the clip's shapes, united, first.
            let mut shape = BezPath::new();
            let ct = affine(cp.transform());
            for n in cp.root().children() {
                if let usvg::Node::Path(p) = n {
                    shape.extend(ct * bez(p.data()));
                }
            }
            let mut clip_node = Node::new(self.doc.new_id(), NodeKind::Path { path: PathData::from_bezpath(&shape), rule: FillRule::NonZero });
            clip_node.name = "Clip".into();
            kids.insert(0, clip_node);
            clip = true;
        }
        let mut n = Node::new(self.doc.new_id(), NodeKind::Group { children: kids, clip, layer: false });
        n.name = g.id().to_string();
        n.transform = affine(g.transform());
        n.opacity = g.opacity().get();
        n.blend = blend(g.blend_mode());
        if let Some(m) = g.mask() {
            let art = self.children(m.root());
            n.mask = Some(Box::new(OpacityMask { art, alpha: m.kind() == usvg::MaskType::Alpha, invert: false }));
        }
        for f in g.filters() {
            for p in f.primitives() {
                match p.kind() {
                    usvg::filter::Kind::GaussianBlur(b) => n.appearance.effects.push(Effect::Blur { radius: 3.0 * b.std_dev_x().get().max(b.std_dev_y().get()) as f64 }),
                    usvg::filter::Kind::DropShadow(d) => n.appearance.effects.push(Effect::DropShadow {
                        dx: d.dx() as f64,
                        dy: d.dy() as f64,
                        blur: 3.0 * d.std_dev_x().get() as f64,
                        color: ucolor(d.color(), d.opacity().get()),
                        spread: 0.0,
                    }),
                    _ => {}
                }
            }
        }
        Some(n)
    }
}

fn blend(b: usvg::BlendMode) -> BlendMode {
    use usvg::BlendMode as B;
    match b {
        B::Normal => BlendMode::Normal,
        B::Multiply => BlendMode::Multiply,
        B::Screen => BlendMode::Screen,
        B::Overlay => BlendMode::Overlay,
        B::Darken => BlendMode::Darken,
        B::Lighten => BlendMode::Lighten,
        B::ColorDodge => BlendMode::ColorDodge,
        B::ColorBurn => BlendMode::ColorBurn,
        B::HardLight => BlendMode::HardLight,
        B::SoftLight => BlendMode::SoftLight,
        B::Difference => BlendMode::Difference,
        B::Exclusion => BlendMode::Exclusion,
        B::Hue => BlendMode::Hue,
        B::Saturation => BlendMode::Saturation,
        B::Color => BlendMode::Color,
        B::Luminosity => BlendMode::Luminosity,
    }
}

/// Reads an SVG into a new document (one artboard the size of the SVG).
pub fn import(svg: &str) -> Result<Document, String> {
    let mut opt = usvg::Options::default();
    opt.fontdb_mut().load_system_fonts();
    let tree = usvg::Tree::from_str(svg, &opt).map_err(|e| format!("not an SVG: {e}"))?;
    let size = tree.size();
    let mut doc = Document::new(size.width() as f64, size.height() as f64);
    let layer = doc.layers[0].id;
    let nodes = {
        let mut imp = Importer { doc: &mut doc };
        imp.children(tree.root())
    };
    if let Some(c) = doc.find_mut(layer).and_then(Node::children_mut) {
        *c = nodes;
    }
    doc.fix_ids();
    Ok(doc)
}

/// Reads an SVG and adds its art to `doc` as one group; returns its id.
pub fn place(doc: &mut Document, svg: &str, parent: Option<u64>) -> Result<u64, String> {
    let other = import(svg)?;
    let mut kids: Vec<Node> = other.layers.into_iter().flat_map(|l| match l.kind {
        NodeKind::Group { children, .. } => children,
        _ => vec![l],
    }).collect();
    doc.reid(&mut kids);
    let mut g = Node::new(0, NodeKind::Group { children: kids, clip: false, layer: false });
    g.name = "Placed SVG".into();
    Ok(doc.add(parent, g))
}

// ------------------------------------------------------------------ export

fn num(v: f64) -> String {
    let r = (v * 1000.0).round() / 1000.0;
    if r == r.trunc() { format!("{}", r as i64) } else { format!("{r}") }
}

/// SVG path data for a path.
pub fn path_d(b: &BezPath) -> String {
    let mut s = String::new();
    let p = |pt: Point| format!("{} {}", num(pt.x), num(pt.y));
    for el in b.elements() {
        let _ = match el {
            PathEl::MoveTo(a) => write!(s, "M{}", p(*a)),
            PathEl::LineTo(a) => write!(s, "L{}", p(*a)),
            PathEl::QuadTo(a, b) => write!(s, "Q{} {}", p(*a), p(*b)),
            PathEl::CurveTo(a, b, c) => write!(s, "C{} {} {}", p(*a), p(*b), p(*c)),
            PathEl::ClosePath => write!(s, "Z"),
        };
    }
    s
}

fn matrix(a: Affine) -> String {
    let c = a.as_coeffs();
    format!("matrix({} {} {} {} {} {})", num(c[0]), num(c[1]), num(c[2]), num(c[3]), num(c[4]), num(c[5]))
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn css_blend(b: BlendMode) -> &'static str {
    match b {
        BlendMode::Normal => "normal",
        BlendMode::Multiply => "multiply",
        BlendMode::Screen => "screen",
        BlendMode::Overlay => "overlay",
        BlendMode::Darken => "darken",
        BlendMode::Lighten => "lighten",
        BlendMode::ColorDodge => "color-dodge",
        BlendMode::ColorBurn => "color-burn",
        BlendMode::HardLight => "hard-light",
        BlendMode::SoftLight => "soft-light",
        BlendMode::Difference => "difference",
        BlendMode::Exclusion => "exclusion",
        BlendMode::Hue => "hue",
        BlendMode::Saturation => "saturation",
        BlendMode::Color => "color",
        BlendMode::Luminosity => "luminosity",
        BlendMode::Plus => "plus-lighter",
    }
}

struct Exporter<'a> {
    defs: String,
    next: usize,
    doc: &'a Document,
}

impl Exporter<'_> {
    fn id(&mut self, prefix: &str) -> String {
        self.next += 1;
        format!("{prefix}{}", self.next)
    }

    fn color_attr(c: Color) -> (String, Option<f32>) {
        let opaque = Color::rgba(c.r, c.g, c.b, 1.0).to_hex();
        (opaque, (c.a < 0.999).then_some(c.a))
    }

    /// `fill="…"` (or stroke) and its opacity for a paint, adding defs.
    fn paint_attrs(&mut self, attr: &str, p: &Paint, opacity: f32) -> String {
        match p {
            Paint::Solid { color } => {
                let (hex, a) = Self::color_attr(*color);
                let a = a.unwrap_or(1.0) * opacity;
                if a < 0.999 { format!(r#" {attr}="{hex}" {attr}-opacity="{}""#, num(a as f64)) } else { format!(r#" {attr}="{hex}""#) }
            }
            Paint::Gradient(g) => {
                let id = self.id("g");
                let st: String = g
                    .stops
                    .iter()
                    .map(|s| {
                        let (hex, a) = Self::color_attr(s.color);
                        let op = a.map(|a| format!(r#" stop-opacity="{}""#, num(a as f64))).unwrap_or_default();
                        format!(r#"<stop offset="{}" stop-color="{hex}"{op}/>"#, num(s.offset as f64))
                    })
                    .collect();
                let spread = match g.extend {
                    Extend::Repeat => r#" spreadMethod="repeat""#,
                    Extend::Reflect => r#" spreadMethod="reflect""#,
                    _ => "",
                };
                let tr = if g.transform == Affine::IDENTITY { String::new() } else { format!(r#" gradientTransform="{}""#, matrix(g.transform)) };
                match g.kind {
                    GradientKind::Linear { start, end } => {
                        let _ = write!(self.defs, r#"<linearGradient id="{id}" gradientUnits="userSpaceOnUse" x1="{}" y1="{}" x2="{}" y2="{}"{spread}{tr}>{st}</linearGradient>"#, num(start.x), num(start.y), num(end.x), num(end.y));
                    }
                    GradientKind::Radial { c0, r0, c1, r1 } => {
                        let _ = write!(
                            self.defs,
                            r#"<radialGradient id="{id}" gradientUnits="userSpaceOnUse" cx="{}" cy="{}" r="{}" fx="{}" fy="{}" fr="{}"{spread}{tr}>{st}</radialGradient>"#,
                            num(c1.x),
                            num(c1.y),
                            num(r1),
                            num(c0.x),
                            num(c0.y),
                            num(r0)
                        );
                    }
                    GradientKind::Conic { .. } => {
                        // SVG has no conic gradient: its average colour stands in.
                        let mid = g.color_at(0.5);
                        return self.paint_attrs(attr, &Paint::solid(mid), opacity);
                    }
                }
                let op = if opacity < 0.999 { format!(r#" {attr}-opacity="{}""#, num(opacity as f64)) } else { String::new() };
                format!(r#" {attr}="url(#{id})"{op}"#)
            }
            // Paints SVG cannot express keep their average look as a colour.
            Paint::Freeform(f) => {
                let n = f.points.len().max(1) as f32;
                let c = f.points.iter().fold([0.0f32; 4], |a, p| [a[0] + p.color.r / n, a[1] + p.color.g / n, a[2] + p.color.b / n, a[3] + p.color.a / n]);
                self.paint_attrs(attr, &Paint::solid(Color::rgba(c[0], c[1], c[2], c[3])), opacity)
            }
            _ => format!(r##" {attr}="#808080""##),
        }
    }

    fn effects_filter(&mut self, effects: &[Effect]) -> Option<String> {
        let raster: Vec<&Effect> = effects.iter().filter(|e| e.is_raster()).collect();
        if raster.is_empty() {
            return None;
        }
        let id = self.id("f");
        let mut body = String::new();
        for e in raster {
            let _ = match e {
                Effect::Blur { radius } => write!(body, r#"<feGaussianBlur stdDeviation="{}"/>"#, num(radius / 3.0)),
                Effect::DropShadow { dx, dy, blur, color, .. } => {
                    let (hex, a) = Self::color_attr(*color);
                    write!(body, r#"<feDropShadow dx="{}" dy="{}" stdDeviation="{}" flood-color="{hex}" flood-opacity="{}"/>"#, num(*dx), num(*dy), num(blur / 3.0), num(a.unwrap_or(1.0) as f64))
                }
                Effect::OuterGlow { blur, color, .. } => {
                    let (hex, a) = Self::color_attr(*color);
                    write!(body, r#"<feDropShadow dx="0" dy="0" stdDeviation="{}" flood-color="{hex}" flood-opacity="{}"/>"#, num(blur / 3.0), num(a.unwrap_or(1.0) as f64))
                }
                _ => Ok(()),
            };
        }
        let _ = write!(self.defs, r#"<filter id="{id}" x="-50%" y="-50%" width="200%" height="200%">{body}</filter>"#);
        Some(id)
    }

    fn node(&mut self, n: &Node, out: &mut String) {
        if !n.visible {
            return;
        }
        let mut attrs = String::new();
        if !n.name.is_empty() {
            let _ = write!(attrs, r#" id="{}""#, esc(&n.name.replace(' ', "_")));
        }
        if n.transform != Affine::IDENTITY {
            let _ = write!(attrs, r#" transform="{}""#, matrix(n.transform));
        }
        if n.opacity < 1.0 {
            let _ = write!(attrs, r#" opacity="{}""#, num(n.opacity as f64));
        }
        if n.blend != BlendMode::Normal {
            let _ = write!(attrs, r#" style="mix-blend-mode:{}""#, css_blend(n.blend));
        }
        let ap = n.style.as_deref().and_then(|k| self.doc.styles.iter().find(|s| s.id == k || s.name == k)).map(|s| s.appearance.clone()).unwrap_or_else(|| n.appearance.clone());
        if let Some(f) = self.effects_filter(&ap.effects) {
            let _ = write!(attrs, r#" filter="url(#{f})""#);
        }
        if let Some(m) = &n.mask {
            let id = self.id("m");
            let mut inner = String::new();
            for k in &m.art {
                self.node(k, &mut inner);
            }
            let kind = if m.alpha { r#" mask-type="alpha""# } else { "" };
            let _ = write!(self.defs, r#"<mask id="{id}"{kind}>{inner}</mask>"#);
            let _ = write!(attrs, r#" mask="url(#{id})""#);
        }
        match &n.kind {
            NodeKind::Group { children, clip, .. } => {
                if *clip && !children.is_empty() {
                    let id = self.id("c");
                    let mut cp = String::new();
                    for (piece, _) in leaf_paths(self.doc, &children[0]) {
                        let _ = write!(cp, r#"<path d="{}"/>"#, path_d(&piece));
                    }
                    let _ = write!(self.defs, r#"<clipPath id="{id}" clipPathUnits="userSpaceOnUse">{cp}</clipPath>"#);
                    let _ = write!(out, r#"<g{attrs}><g clip-path="url(#{id})">"#);
                    for k in &children[1..] {
                        self.node(k, out);
                    }
                    out.push_str("</g></g>");
                } else {
                    let _ = write!(out, "<g{attrs}>");
                    for k in children {
                        self.node(k, out);
                    }
                    out.push_str("</g>");
                }
            }
            NodeKind::Image(im) => {
                let _ = write!(out, "<g{attrs}>");
                if let Some(img) = &im.image {
                    let png = super::encode_png(img);
                    let _ = write!(out, r#"<image width="{}" height="{}" preserveAspectRatio="none" href="data:image/png;base64,{}"/>"#, num(im.width), num(im.height), super::base64(&png));
                } else if let Some(h) = &im.href {
                    let _ = write!(out, r#"<image width="{}" height="{}" preserveAspectRatio="none" href="{}"/>"#, num(im.width), num(im.height), esc(h));
                }
                out.push_str("</g>");
            }
            NodeKind::Path { .. } | NodeKind::Shape { .. } | NodeKind::Text(_) => {
                let pieces = build::geometry(self.doc, n);
                let rule = match &n.kind {
                    NodeKind::Path { rule: FillRule::EvenOdd, .. } => r#" fill-rule="evenodd""#,
                    _ => "",
                };
                let _ = write!(out, "<g{attrs}>");
                if let NodeKind::Text(t) = &n.kind {
                    let _ = write!(out, "<title>{}</title>", esc(&t.plain()));
                }
                let vector: Vec<Effect> = ap.effects.iter().filter(|e| !e.is_raster()).cloned().collect();
                let ap_items = if ap.items.is_empty() && matches!(n.kind, NodeKind::Text(_)) { vec![Item::Fill(Fill::new(Color::BLACK))] } else { ap.items.clone() };
                for item in &ap_items {
                    for piece in &pieces {
                        let mut g = crate::live::apply_effects(&piece.path, &vector);
                        match item {
                            Item::Fill(f) if f.visible => {
                                g = crate::live::apply_effects(&g, &f.effects);
                                let paint = piece.color.map(Paint::solid).unwrap_or_else(|| self.resolve(&f.paint, f.swatch.as_deref(), f.tint));
                                let pa = self.paint_attrs("fill", &paint, f.opacity);
                                let bl = if f.blend != BlendMode::Normal { format!(r#" style="mix-blend-mode:{}""#, css_blend(f.blend)) } else { String::new() };
                                let _ = write!(out, r#"<path d="{}"{pa}{rule}{bl}/>"#, path_d(&g.to_bezpath()));
                            }
                            Item::Stroke(s) if s.visible => {
                                g = crate::live::apply_effects(&g, &s.effects);
                                let paint = self.resolve(&s.paint, s.swatch.as_deref(), s.tint);
                                let pa = self.paint_attrs("stroke", &paint, s.opacity);
                                let st = &s.style;
                                if !st.profile.is_empty() {
                                    // Variable width: the stroke's outline, filled.
                                    let o = studio_vector_raster::stroke_outline(&g.to_bezpath(), st, 0.05);
                                    let fa = self.paint_attrs("fill", &paint, s.opacity);
                                    let _ = write!(out, r#"<path d="{}"{fa}/>"#, path_d(&o));
                                    continue;
                                }
                                let join = match st.join {
                                    Join::Miter => "miter",
                                    Join::Round => "round",
                                    Join::Bevel => "bevel",
                                };
                                let cap = match st.start_cap {
                                    Cap::Butt => "butt",
                                    Cap::Round => "round",
                                    Cap::Square => "square",
                                };
                                let mut extra = format!(r#" stroke-width="{}" stroke-linejoin="{join}" stroke-linecap="{cap}" stroke-miterlimit="{}""#, num(st.width), num(st.miter_limit));
                                if !st.dashes.is_empty() {
                                    let d: Vec<String> = st.dashes.iter().map(|v| num(*v)).collect();
                                    let _ = write!(extra, r#" stroke-dasharray="{}" stroke-dashoffset="{}""#, d.join(" "), num(st.dash_offset));
                                }
                                if st.non_scaling {
                                    extra.push_str(r#" vector-effect="non-scaling-stroke""#);
                                }
                                let _ = write!(out, r#"<path d="{}" fill="none"{pa}{extra}/>"#, path_d(&g.to_bezpath()));
                            }
                            _ => {}
                        }
                    }
                }
                out.push_str("</g>");
            }
            _ => {
                // Live constructions: the art they make, as paths.
                let mut s = studio_vector_raster::Scene::new();
                let single = Document { layers: vec![{
                    let mut k = n.clone();
                    k.transform = Affine::IDENTITY;
                    k.opacity = 1.0;
                    k.blend = BlendMode::Normal;
                    k.mask = None;
                    k
                }], ..self.doc.clone() };
                let built = build::scene(&single, &BuildOptions::default());
                s.items.extend(built.items);
                let _ = write!(out, "<g{attrs}>");
                self.scene_items(&s, out);
                out.push_str("</g>");
            }
        }
    }

    fn resolve(&self, p: &Paint, swatch: Option<&str>, tint: f32) -> Paint {
        match swatch.and_then(|k| self.doc.swatch(k)) {
            Some(s) => Paint::solid(crate::color::tinted(s.color, tint)),
            None => p.clone(),
        }
    }

    /// Writes already-built scene items (used for live constructions).
    fn scene_items(&mut self, s: &studio_vector_raster::Scene, out: &mut String) {
        use studio_vector_raster::Item as SI;
        for it in &s.items {
            match it {
                SI::Fill { path, transform, rule, paint, opacity, .. } => {
                    let pa = self.paint_attrs("fill", paint, *opacity);
                    let r = if *rule == FillRule::EvenOdd { r#" fill-rule="evenodd""# } else { "" };
                    let _ = write!(out, r#"<path d="{}"{pa}{r}/>"#, path_d(&(*transform * (**path).clone())));
                }
                SI::Stroke { path, transform, style, paint, opacity, .. } => {
                    let o = studio_vector_raster::stroke_outline(path, style, 0.05);
                    let pa = self.paint_attrs("fill", paint, *opacity);
                    let _ = write!(out, r#"<path d="{}"{pa}/>"#, path_d(&(*transform * o)));
                }
                SI::Layer { layer, scene } => {
                    let op = if layer.opacity < 1.0 { format!(r#" opacity="{}""#, num(layer.opacity as f64)) } else { String::new() };
                    let _ = write!(out, "<g{op}>");
                    self.scene_items(scene, out);
                    out.push_str("</g>");
                }
            }
        }
    }
}

/// The outlines of a node's leaves in its parent's space (for clip paths).
fn leaf_paths(doc: &Document, n: &Node) -> Vec<(BezPath, ())> {
    build::geometry(doc, n).into_iter().map(|p| (n.transform * p.path.to_bezpath(), ())).collect()
}

/// The document's artboard as SVG.
pub fn export(doc: &Document, artboard: Option<&str>) -> String {
    let ab = doc.artboard(artboard).cloned();
    let rect = ab.as_ref().map(|a| a.rect).unwrap_or(Rect::new(0.0, 0.0, 1920.0, 1080.0));
    let mut e = Exporter { defs: String::new(), next: 0, doc };
    let mut body = String::new();
    if let Some(bg) = ab.as_ref().and_then(|a| a.background) {
        let (hex, a) = Exporter::color_attr(bg);
        let op = a.map(|a| format!(r#" fill-opacity="{}""#, num(a as f64))).unwrap_or_default();
        let _ = write!(body, r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{hex}"{op}/>"#, num(rect.x0), num(rect.y0), num(rect.width()), num(rect.height()));
    }
    for l in &doc.layers {
        e.node(l, &mut body);
    }
    let defs = if e.defs.is_empty() { String::new() } else { format!("<defs>{}</defs>", e.defs) };
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="{}" height="{}" viewBox="{} {} {} {}">{defs}{body}</svg>"#,
        num(rect.width()),
        num(rect.height()),
        num(rect.x0),
        num(rect.y0),
        num(rect.width()),
        num(rect.height())
    )
}
