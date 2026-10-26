// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! PDF out and in.
//!
//! Export writes one page per artboard, all vector: fills and strokes with
//! their joins, caps and dashes, linear and radial gradients as smooth
//! shadings (repeating ones stitched), images, clipping, group opacity and
//! blend modes as transparency groups, and opacity masks as soft masks.
//! Only what PDF cannot express (blurs, shadows and glows, conic, freeform
//! and mesh gradients, patterns) is drawn as a high-resolution image.
//!
//! Import reads the vector content of each page: paths, fills, strokes,
//! colours, opacity, clipping, form XObjects and images. Text on a page is
//! not converted (fonts in PDFs are often subset and unnamed).

use std::collections::HashMap;
use std::fmt::Write;

use kurbo::{Affine, BezPath, PathEl, Point, Rect, Shape};
use studio_vector_geom::PathData;
use studio_vector_raster::{BlendMode, Cap, Color, Extend, GradientKind, Item as SI, Join, Paint, Scene, StrokeStyle};

use crate::appearance::{Appearance, Fill, Item, Stroke};
use crate::build::{self, BuildOptions};
use crate::doc::{Document, ImageNode, Node, NodeKind};
use crate::FillRule;

// ================================================================== export

fn n(v: f64) -> String {
    let r = (v * 10000.0).round() / 10000.0;
    if r == r.trunc() { format!("{}", r as i64) } else { format!("{r}") }
}

struct Obj {
    body: Vec<u8>,
}

#[derive(Default)]
struct Resources {
    ext: Vec<(String, u32)>,
    shading: Vec<(String, u32)>,
    xobject: Vec<(String, u32)>,
}

impl Resources {
    fn dict(&self) -> String {
        let part = |name: &str, list: &[(String, u32)]| {
            if list.is_empty() {
                String::new()
            } else {
                let mut s = format!("/{name} <<");
                for (k, id) in list {
                    let _ = write!(s, " /{k} {id} 0 R");
                }
                s.push_str(" >>");
                s
            }
        };
        format!("<< {} {} {} >>", part("ExtGState", &self.ext), part("Shading", &self.shading), part("XObject", &self.xobject))
    }
}

struct Writer {
    objs: Vec<Obj>,
    /// Pixels per document unit for the parts drawn as images.
    raster_scale: f64,
}

impl Writer {
    fn reserve(&mut self) -> u32 {
        self.objs.push(Obj { body: Vec::new() });
        self.objs.len() as u32
    }

    fn set(&mut self, id: u32, body: Vec<u8>) {
        self.objs[id as usize - 1].body = body;
    }

    fn add(&mut self, body: impl Into<Vec<u8>>) -> u32 {
        let id = self.reserve();
        self.set(id, body.into());
        id
    }

    fn stream(&mut self, dict: &str, data: &[u8], compress: bool) -> u32 {
        let (data, filter) = if compress { (miniz_oxide::deflate::compress_to_vec_zlib(data, 6), " /Filter /FlateDecode") } else { (data.to_vec(), "") };
        let mut b = format!("<< {dict}{filter} /Length {} >>\nstream\n", data.len()).into_bytes();
        b.extend_from_slice(&data);
        b.extend_from_slice(b"\nendstream");
        self.add(b)
    }

    fn ext_state(&mut self, res: &mut Resources, fill_a: f32, stroke_a: f32, blend: BlendMode, smask: Option<u32>) -> String {
        let bm = match blend {
            BlendMode::Normal | BlendMode::Plus => "Normal",
            BlendMode::Multiply => "Multiply",
            BlendMode::Screen => "Screen",
            BlendMode::Overlay => "Overlay",
            BlendMode::Darken => "Darken",
            BlendMode::Lighten => "Lighten",
            BlendMode::ColorDodge => "ColorDodge",
            BlendMode::ColorBurn => "ColorBurn",
            BlendMode::HardLight => "HardLight",
            BlendMode::SoftLight => "SoftLight",
            BlendMode::Difference => "Difference",
            BlendMode::Exclusion => "Exclusion",
            BlendMode::Hue => "Hue",
            BlendMode::Saturation => "Saturation",
            BlendMode::Color => "Color",
            BlendMode::Luminosity => "Luminosity",
        };
        let sm = smask.map(|s| format!(" /SMask {s} 0 R")).unwrap_or_default();
        let id = self.add(format!("<< /Type /ExtGState /ca {} /CA {} /BM /{bm}{sm} >>", n(fill_a as f64), n(stroke_a as f64)));
        let name = format!("G{}", res.ext.len());
        res.ext.push((name.clone(), id));
        name
    }

    /// A stitched function of the gradient's stops (sRGB, alpha dropped:
    /// stop alpha goes to a soft mask only when it varies).
    fn function(&mut self, stops: &[studio_vector_raster::Stop], repeat: Option<(Extend, i32)>) -> u32 {
        let c = |s: &studio_vector_raster::Stop| format!("[{} {} {}]", n(s.color.r as f64), n(s.color.g as f64), n(s.color.b as f64));
        let mut parts = Vec::new();
        let mut bounds = Vec::new();
        let mut encode = Vec::new();
        let st: Vec<_> = if stops.len() == 1 { vec![stops[0], stops[0]] } else { stops.to_vec() };
        for w in st.windows(2) {
            parts.push(self.add(format!("<< /FunctionType 2 /Domain [0 1] /C0 {} /C1 {} /N 1 >>", c(&w[0]), c(&w[1]))));
        }
        for w in st.windows(2).skip(1) {
            bounds.push(n(w[0].offset as f64));
        }
        for _ in &parts {
            encode.push("0 1".to_string());
        }
        let single = |s: &mut Writer, parts: &[u32]| {
            let fs: Vec<String> = parts.iter().map(|p| format!("{p} 0 R")).collect();
            s.add(format!("<< /FunctionType 3 /Domain [0 1] /Functions [{}] /Bounds [{}] /Encode [{}] >>", fs.join(" "), bounds.join(" "), encode.join(" ")))
        };
        let one = single(self, &parts);
        let Some((ext, k)) = repeat else { return one };
        // Repeating gradients: copies over [-k, k + 1], reflected ones flipped.
        let copies: Vec<String> = (0..2 * k + 1).map(|_| format!("{one} 0 R")).collect();
        let b: Vec<String> = (-k + 1..=k).map(|i| n(i as f64)).collect();
        let e: Vec<String> = (-k..=k).map(|i| if ext == Extend::Reflect && i.rem_euclid(2) == 1 { "1 0".to_string() } else { "0 1".to_string() }).collect();
        self.add(format!("<< /FunctionType 3 /Domain [{} {}] /Functions [{}] /Bounds [{}] /Encode [{}] >>", -k, k + 1, copies.join(" "), b.join(" "), e.join(" ")))
    }

    /// A shading for a linear or radial gradient, or `None` for kinds PDF lacks.
    fn shading(&mut self, g: &studio_vector_raster::Gradient) -> Option<u32> {
        let rep = matches!(g.extend, Extend::Repeat | Extend::Reflect);
        match g.kind {
            GradientKind::Linear { start, end } => {
                let k = 16;
                let (p0, p1) = if rep { (start - (end - start) * k as f64, end + (end - start) * k as f64) } else { (start, end) };
                let f = self.function(&g.stops, rep.then_some((g.extend, k)));
                let dom = if rep { format!(" /Domain [{} {}]", -k, k + 1) } else { String::new() };
                Some(self.add(format!("<< /ShadingType 2 /ColorSpace /DeviceRGB /Coords [{} {} {} {}] /Function {f} 0 R /Extend [true true]{dom} >>", n(p0.x), n(p0.y), n(p1.x), n(p1.y))))
            }
            GradientKind::Radial { c0, r0, c1, r1 } if !rep => {
                let f = self.function(&g.stops, None);
                Some(self.add(format!("<< /ShadingType 3 /ColorSpace /DeviceRGB /Coords [{} {} {} {} {} {}] /Function {f} 0 R /Extend [true true] >>", n(c0.x), n(c0.y), n(r0), n(c1.x), n(c1.y), n(r1))))
            }
            _ => None,
        }
    }

    fn image(&mut self, img: &studio_vector_raster::Image) -> u32 {
        let px = studio_vector_raster::Pixmap { width: img.width, height: img.height, data: img.data.to_vec() }.to_straight();
        let mut rgb = Vec::with_capacity(px.len() / 4 * 3);
        let mut alpha = Vec::with_capacity(px.len() / 4);
        for p in px.chunks_exact(4) {
            rgb.extend_from_slice(&p[..3]);
            alpha.push(p[3]);
        }
        let opaque = alpha.iter().all(|a| *a == 255);
        let smask = (!opaque).then(|| self.stream(&format!("/Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceGray /BitsPerComponent 8", img.width, img.height), &alpha, true));
        let sm = smask.map(|s| format!(" /SMask {s} 0 R")).unwrap_or_default();
        self.stream(&format!("/Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceRGB /BitsPerComponent 8{sm}", img.width, img.height), &rgb, true)
    }

    fn xobject(&mut self, res: &mut Resources, id: u32) -> String {
        let name = format!("X{}", res.xobject.len());
        res.xobject.push((name.clone(), id));
        name
    }

    /// Draws `scene` (items in document space; `base` maps it to the page) as image content.
    fn raster(&mut self, scene: &Scene, out: &mut String, res: &mut Resources) {
        let Some(b) = scene.bounds() else { return };
        let mut s = self.raster_scale;
        let px = b.width() * b.height() * s * s;
        if px > 16e6 {
            s *= (16e6 / px).sqrt();
        }
        let (w, h) = ((b.width() * s).ceil().max(1.0) as u32, (b.height() * s).ceil().max(1.0) as u32);
        let mut o = studio_vector_raster::RenderOptions::new(w, h).with_transform(Affine::scale(s) * Affine::translate((-b.x0, -b.y0)));
        o.dither = true;
        let pm = studio_vector_raster::render(scene, &o);
        let Some(img) = pm.to_image() else { return };
        let id = self.image(&img);
        let name = self.xobject(res, id);
        // Images fill the unit square, bottom row first; documents are y-down.
        let (bw, bh) = (w as f64 / s, h as f64 / s);
        let _ = writeln!(out, "q {} 0 0 {} {} {} cm /{name} Do Q", n(bw), n(-bh), n(b.x0), n(b.y0 + bh));
    }

    fn path_ops(out: &mut String, m: Affine, p: &BezPath) {
        for el in p.elements() {
            let _ = match *el {
                PathEl::MoveTo(a) => {
                    let a = m * a;
                    writeln!(out, "{} {} m", n(a.x), n(a.y))
                }
                PathEl::LineTo(a) => {
                    let a = m * a;
                    writeln!(out, "{} {} l", n(a.x), n(a.y))
                }
                PathEl::QuadTo(c, b) => {
                    // Callers raise quadratics to cubics first ([`cubic`]); this is a fallback.
                    let (c, b) = (m * c, m * b);
                    writeln!(out, "{} {} {} {} {} {} c", n(c.x), n(c.y), n(c.x), n(c.y), n(b.x), n(b.y))
                }
                PathEl::CurveTo(a, b, c) => {
                    let (a, b, c) = (m * a, m * b, m * c);
                    writeln!(out, "{} {} {} {} {} {} c", n(a.x), n(a.y), n(b.x), n(b.y), n(c.x), n(c.y))
                }
                PathEl::ClosePath => writeln!(out, "h"),
            };
        }
    }

    fn items(&mut self, scene: &Scene, out: &mut String, res: &mut Resources) {
        for it in &scene.items {
            match it {
                SI::Fill { path, transform, rule, paint, blend, opacity } => {
                    // Quadratic segments become cubics for PDF.
                    let path = cubic(path);
                    self.fill(&path, *transform, *rule, paint, *blend, *opacity, out, res, it);
                }
                SI::Stroke { path, transform, style, paint, blend, opacity } => {
                    let path = cubic(path);
                    let simple = style.profile.is_empty() && style.align == studio_vector_raster::StrokeAlign::Center && !style.non_scaling && matches!(paint, Paint::Solid { .. });
                    if simple {
                        let Paint::Solid { color } = paint else { continue };
                        let gs = self.ext_state(res, 1.0, color.a * opacity, *blend, None);
                        let _ = writeln!(out, "q /{gs} gs {} {} {} RG", n(color.r as f64), n(color.g as f64), n(color.b as f64));
                        stroke_params(out, style);
                        let c = transform.as_coeffs();
                        let _ = writeln!(out, "{} {} {} {} {} {} cm", n(c[0]), n(c[1]), n(c[2]), n(c[3]), n(c[4]), n(c[5]));
                        Self::path_ops(out, Affine::IDENTITY, &path);
                        out.push_str("S Q\n");
                    } else {
                        // The stroke's outline, filled with its paint.
                        let o = cubic(&studio_vector_raster::stroke_outline(&path, style, 0.02));
                        self.fill(&o, *transform, FillRule::NonZero, paint, *blend, *opacity, out, res, it);
                    }
                }
                SI::Layer { layer, scene: inner } => {
                    if !layer.filters.is_empty() {
                        let mut s = Scene::new();
                        s.items.push(it.clone());
                        self.raster(&s, out, res);
                        continue;
                    }
                    out.push_str("q\n");
                    if let Some(c) = &layer.clip {
                        Self::path_ops(out, c.transform, &cubic(&c.path));
                        out.push_str(if c.rule == FillRule::EvenOdd { "W* n\n" } else { "W n\n" });
                    }
                    let plain = layer.opacity >= 1.0 && layer.blend == BlendMode::Normal && layer.mask.is_none();
                    if plain {
                        self.items(inner, out, res);
                    } else {
                        // A transparency group drawn with the layer's opacity, blend and mask.
                        let bbox = inner.bounds().unwrap_or(Rect::ZERO).inflate(1.0, 1.0);
                        let form = self.form(inner, bbox, false);
                        let smask = layer.mask.as_ref().map(|m| {
                            let g = self.form(&m.scene, bbox, true);
                            let kind = if m.kind == studio_vector_raster::MaskKind::Alpha { "Alpha" } else { "Luminosity" };
                            let tr = if m.invert { let f = self.add("<< /FunctionType 2 /Domain [0 1] /C0 [1] /C1 [0] /N 1 >>"); format!(" /TR {f} 0 R") } else { String::new() };
                            self.add(format!("<< /Type /Mask /S /{kind} /G {g} 0 R{tr} >>"))
                        });
                        let gs = self.ext_state(res, layer.opacity, layer.opacity, layer.blend, smask);
                        let name = self.xobject(res, form);
                        let _ = writeln!(out, "/{gs} gs /{name} Do");
                    }
                    out.push_str("Q\n");
                }
            }
        }
    }

    /// A form XObject (transparency group) of `scene` over `bbox`.
    fn form(&mut self, scene: &Scene, bbox: Rect, luminosity: bool) -> u32 {
        let mut res = Resources::default();
        let mut body = String::new();
        if luminosity {
            // Outside the mask's art the mask is black (hidden).
            let _ = writeln!(body, "0 g {} {} {} {} re f", n(bbox.x0), n(bbox.y0), n(bbox.width()), n(bbox.height()));
        }
        self.items(scene, &mut body, &mut res);
        let cs = " /CS /DeviceRGB";
        self.stream(
            &format!("/Type /XObject /Subtype /Form /BBox [{} {} {} {}] /Group << /S /Transparency{cs} /I true >> /Resources {}", n(bbox.x0), n(bbox.y0), n(bbox.x1), n(bbox.y1), res.dict()),
            body.as_bytes(),
            true,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn fill(&mut self, path: &BezPath, transform: Affine, rule: FillRule, paint: &Paint, blend: BlendMode, opacity: f32, out: &mut String, res: &mut Resources, item: &SI) {
        let op = if rule == FillRule::EvenOdd { "f*" } else { "f" };
        let clip_op = if rule == FillRule::EvenOdd { "W* n" } else { "W n" };
        let c = transform.as_coeffs();
        let cm = format!("{} {} {} {} {} {} cm", n(c[0]), n(c[1]), n(c[2]), n(c[3]), n(c[4]), n(c[5]));
        match paint {
            Paint::Solid { color } => {
                let gs = self.ext_state(res, color.a * opacity, 1.0, blend, None);
                let _ = writeln!(out, "q /{gs} gs {} {} {} rg {cm}", n(color.r as f64), n(color.g as f64), n(color.b as f64));
                Self::path_ops(out, Affine::IDENTITY, path);
                let _ = writeln!(out, "{op} Q");
            }
            Paint::Gradient(g) if g.stops.iter().all(|s| (s.color.a - g.stops[0].color.a).abs() < 1e-3) => match self.shading(g) {
                Some(sh) => {
                    let name = format!("S{}", res.shading.len());
                    res.shading.push((name.clone(), sh));
                    let gs = self.ext_state(res, g.stops[0].color.a * opacity, 1.0, blend, None);
                    let gc = g.transform.as_coeffs();
                    let _ = writeln!(out, "q /{gs} gs {cm}");
                    Self::path_ops(out, Affine::IDENTITY, path);
                    let _ = writeln!(out, "{clip_op} {} {} {} {} {} {} cm /{name} sh Q", n(gc[0]), n(gc[1]), n(gc[2]), n(gc[3]), n(gc[4]), n(gc[5]));
                }
                None => {
                    let mut s = Scene::new();
                    s.items.push(item.clone());
                    self.raster(&s, out, res);
                }
            },
            Paint::Image(ip) if ip.extend == Extend::None || ip.extend == Extend::Pad => {
                let id = self.image(&ip.image);
                let name = self.xobject(res, id);
                let gs = self.ext_state(res, ip.opacity * opacity, 1.0, blend, None);
                let ic = (Affine::scale_non_uniform(ip.image.width as f64, ip.image.height as f64)).as_coeffs();
                let it = ip.transform.as_coeffs();
                let _ = writeln!(out, "q /{gs} gs {cm}");
                Self::path_ops(out, Affine::IDENTITY, path);
                let _ = writeln!(out, "{clip_op} {} {} {} {} {} {} cm {} 0 0 {} 0 {} cm /{name} Do Q", n(it[0]), n(it[1]), n(it[2]), n(it[3]), n(it[4]), n(it[5]), n(ic[0]), n(-ic[3]), n(ic[3]));
            }
            _ => {
                let mut s = Scene::new();
                s.items.push(item.clone());
                self.raster(&s, out, res);
            }
        }
    }
}

fn stroke_params(out: &mut String, s: &StrokeStyle) {
    let j = match s.join {
        Join::Miter => 0,
        Join::Round => 1,
        Join::Bevel => 2,
    };
    let c = match s.start_cap {
        Cap::Butt => 0,
        Cap::Round => 1,
        Cap::Square => 2,
    };
    let dash = if s.dashes.is_empty() { "[] 0 d".to_string() } else { format!("[{}] {} d", s.dashes.iter().map(|v| n(*v)).collect::<Vec<_>>().join(" "), n(s.dash_offset)) };
    let _ = writeln!(out, "{} w {j} j {c} J {} M {dash}", n(s.width), n(s.miter_limit.max(1.0)));
}

/// The path with quadratic segments raised to cubics.
fn cubic(p: &BezPath) -> BezPath {
    let mut out = BezPath::new();
    let mut last = Point::ZERO;
    for el in p.elements() {
        match *el {
            PathEl::MoveTo(a) => {
                out.move_to(a);
                last = a;
            }
            PathEl::LineTo(a) => {
                out.line_to(a);
                last = a;
            }
            PathEl::QuadTo(c, b) => {
                let c1 = last + (c - last) * (2.0 / 3.0);
                let c2 = b + (c - b) * (2.0 / 3.0);
                out.curve_to(c1, c2, b);
                last = b;
            }
            PathEl::CurveTo(a, b, c) => {
                out.curve_to(a, b, c);
                last = c;
            }
            PathEl::ClosePath => out.close_path(),
        }
    }
    out
}

/// The document as a PDF, one page per artboard (or only `artboard`).
pub fn export(doc: &Document, artboard: Option<&str>, opts: &BuildOptions) -> Vec<u8> {
    let mut w = Writer { objs: Vec::new(), raster_scale: 300.0 / 72.0 };
    let catalog = w.reserve();
    let pages = w.reserve();
    let boards: Vec<_> = match artboard {
        Some(k) => doc.artboard(Some(k)).into_iter().cloned().collect(),
        None => doc.artboards.clone(),
    };
    let mut kids = Vec::new();
    for ab in &boards {
        let (scene, rect) = build::artboard_scene(doc, Some(ab), opts);
        let mut res = Resources::default();
        let mut body = String::new();
        // Document space (y down, artboard at its place) to the page (y up).
        let _ = writeln!(body, "1 0 0 -1 {} {} cm", n(-rect.x0), n(rect.y1));
        w.items(&scene, &mut body, &mut res);
        let content = w.stream("", body.as_bytes(), true);
        let page = w.add(format!(
            "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 {} {}] /Resources {} /Contents {content} 0 R /Group << /S /Transparency /CS /DeviceRGB >> >>",
            n(rect.width()),
            n(rect.height()),
            res.dict()
        ));
        kids.push(page);
    }
    let k: Vec<String> = kids.iter().map(|k| format!("{k} 0 R")).collect();
    w.set(pages, format!("<< /Type /Pages /Kids [{}] /Count {} >>", k.join(" "), kids.len()).into_bytes());
    w.set(catalog, format!("<< /Type /Catalog /Pages {pages} 0 R >>").into_bytes());
    let info = w.add(format!("<< /Producer (Zornov) /Title ({}) >>", doc.name.replace(['(', ')', '\\'], "")));
    let mut out = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in w.objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(&o.body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", w.objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root {catalog} 0 R /Info {info} 0 R >>\nstartxref\n{xref}\n%%EOF\n", w.objs.len() + 1).as_bytes());
    out
}

// ================================================================== import

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Obj2 {
    Null,
    Bool(bool),
    Num(f64),
    Name(String),
    Str(Vec<u8>),
    Arr(Vec<Obj2>),
    Dict(HashMap<String, Obj2>),
    Ref(u32),
    Stream(HashMap<String, Obj2>, Vec<u8>),
    /// An operator (in content streams).
    Op(String),
}

impl Obj2 {
    fn num(&self) -> Option<f64> {
        match self {
            Obj2::Num(v) => Some(*v),
            _ => None,
        }
    }

    fn name(&self) -> Option<&str> {
        match self {
            Obj2::Name(s) => Some(s),
            _ => None,
        }
    }

    fn dict(&self) -> Option<&HashMap<String, Obj2>> {
        match self {
            Obj2::Dict(d) | Obj2::Stream(d, _) => Some(d),
            _ => None,
        }
    }
}

pub(crate) struct Lexer<'a> {
    pub s: &'a [u8],
    pub i: usize,
    /// PostScript: `[`, `]`, `<<` and `>>` are operators, not literals.
    pub ps: bool,
}

fn is_delim(c: u8) -> bool {
    matches!(c, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

impl<'a> Lexer<'a> {
    pub fn new(s: &'a [u8]) -> Lexer<'a> {
        Lexer { s, i: 0, ps: false }
    }

    pub fn postscript(s: &'a [u8]) -> Lexer<'a> {
        Lexer { s, i: 0, ps: true }
    }

    fn skip_ws(&mut self) {
        while self.i < self.s.len() {
            let c = self.s[self.i];
            if c.is_ascii_whitespace() || c == 0 {
                self.i += 1;
            } else if c == b'%' {
                while self.i < self.s.len() && self.s[self.i] != b'\n' && self.s[self.i] != b'\r' {
                    self.i += 1;
                }
            } else {
                break;
            }
        }
    }

    fn word(&mut self) -> &'a [u8] {
        let st = self.i;
        while self.i < self.s.len() && !self.s[self.i].is_ascii_whitespace() && !is_delim(self.s[self.i]) {
            self.i += 1;
        }
        &self.s[st..self.i]
    }

    /// The next object (or operator); `None` at the end.
    pub(crate) fn next(&mut self) -> Option<Obj2> {
        self.skip_ws();
        let c = *self.s.get(self.i)?;
        Some(match c {
            b'/' => {
                self.i += 1;
                let w = self.word();
                // #xx escapes in names.
                let mut name = String::new();
                let mut k = 0;
                while k < w.len() {
                    if w[k] == b'#'
                        && k + 2 < w.len()
                        && let Ok(v) = u8::from_str_radix(std::str::from_utf8(&w[k + 1..k + 3]).unwrap_or(""), 16)
                    {
                        name.push(v as char);
                        k += 3;
                        continue;
                    }
                    name.push(w[k] as char);
                    k += 1;
                }
                Obj2::Name(name)
            }
            b'(' => {
                self.i += 1;
                let mut depth = 1;
                let mut out = Vec::new();
                while self.i < self.s.len() {
                    let c = self.s[self.i];
                    self.i += 1;
                    match c {
                        b'\\' => {
                            let e = *self.s.get(self.i).unwrap_or(&b'\\');
                            self.i += 1;
                            out.push(match e {
                                b'n' => b'\n',
                                b'r' => b'\r',
                                b't' => b'\t',
                                b'b' => 8,
                                b'f' => 12,
                                b'0'..=b'7' => {
                                    let mut v = (e - b'0') as u32;
                                    for _ in 0..2 {
                                        match self.s.get(self.i) {
                                            Some(d @ b'0'..=b'7') => {
                                                v = v * 8 + (d - b'0') as u32;
                                                self.i += 1;
                                            }
                                            _ => break,
                                        }
                                    }
                                    v as u8
                                }
                                other => other,
                            });
                        }
                        b'(' => {
                            depth += 1;
                            out.push(c);
                        }
                        b')' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                            out.push(c);
                        }
                        _ => out.push(c),
                    }
                }
                Obj2::Str(out)
            }
            b'<' if self.ps && self.s.get(self.i + 1) == Some(&b'<') => {
                self.i += 2;
                Obj2::Op("<<".into())
            }
            b'>' if self.ps && self.s.get(self.i + 1) == Some(&b'>') => {
                self.i += 2;
                Obj2::Op(">>".into())
            }
            b'[' | b']' if self.ps => {
                self.i += 1;
                Obj2::Op(String::from(c as char))
            }
            b'<' if self.s.get(self.i + 1) == Some(&b'<') => {
                self.i += 2;
                let mut d = HashMap::new();
                loop {
                    self.skip_ws();
                    if self.s.get(self.i) == Some(&b'>') && self.s.get(self.i + 1) == Some(&b'>') {
                        self.i += 2;
                        break;
                    }
                    let Some(Obj2::Name(k)) = self.next() else { break };
                    let v = self.value()?;
                    d.insert(k, v);
                }
                Obj2::Dict(d)
            }
            b'<' => {
                self.i += 1;
                let st = self.i;
                while self.i < self.s.len() && self.s[self.i] != b'>' {
                    self.i += 1;
                }
                let hex: Vec<u8> = self.s[st..self.i].iter().copied().filter(|c| c.is_ascii_hexdigit()).collect();
                self.i += 1;
                let mut out = Vec::new();
                for p in hex.chunks(2) {
                    let s = if p.len() == 2 { [p[0], p[1]] } else { [p[0], b'0'] };
                    out.push(u8::from_str_radix(std::str::from_utf8(&s).unwrap_or("0"), 16).unwrap_or(0));
                }
                Obj2::Str(out)
            }
            b'[' => {
                self.i += 1;
                let mut a = Vec::new();
                loop {
                    self.skip_ws();
                    if self.s.get(self.i) == Some(&b']') {
                        self.i += 1;
                        break;
                    }
                    match self.value() {
                        Some(v) => a.push(v),
                        None => break,
                    }
                }
                Obj2::Arr(a)
            }
            b']' | b'>' | b'}' | b')' => {
                self.i += 1;
                Obj2::Op(String::from(c as char))
            }
            b'{' => {
                self.i += 1;
                Obj2::Op("{".into())
            }
            _ => {
                let w = self.word();
                if w.is_empty() {
                    self.i += 1;
                    return Some(Obj2::Op(String::new()));
                }
                let s = String::from_utf8_lossy(w).to_string();
                if let Ok(v) = s.parse::<f64>() {
                    Obj2::Num(v)
                } else {
                    match s.as_str() {
                        "true" => Obj2::Bool(true),
                        "false" => Obj2::Bool(false),
                        "null" => Obj2::Null,
                        _ => Obj2::Op(s),
                    }
                }
            }
        })
    }

    /// An object, with `n g R` references folded.
    fn value(&mut self) -> Option<Obj2> {
        let v = self.next()?;
        if let Obj2::Num(a) = v {
            let save = self.i;
            if let Some(Obj2::Num(_)) = self.next() {
                if let Some(Obj2::Op(r)) = self.next()
                    && r == "R"
                {
                    return Some(Obj2::Ref(a as u32));
                }
            }
            self.i = save;
        }
        Some(v)
    }
}

fn inflate(data: &[u8]) -> Vec<u8> {
    miniz_oxide::inflate::decompress_to_vec_zlib(data).or_else(|_| miniz_oxide::inflate::decompress_to_vec(data)).unwrap_or_default()
}

struct Pdf {
    objs: HashMap<u32, Obj2>,
}

impl Pdf {
    fn parse(data: &[u8]) -> Pdf {
        let mut objs = HashMap::new();
        // Every "n g obj" in the file (robust to broken cross-reference tables).
        let mut i = 0;
        while let Some(p) = find(data, b" obj", i) {
            // Back up over "n g".
            let mut s = p;
            let mut fields = 0;
            while s > 0 && fields < 2 {
                s -= 1;
                while s > 0 && data[s].is_ascii_whitespace() {
                    s -= 1;
                }
                while s > 0 && data[s - 1].is_ascii_digit() {
                    s -= 1;
                }
                fields += 1;
            }
            let head = String::from_utf8_lossy(&data[s..p]).to_string();
            let mut parts = head.split_whitespace();
            let id = parts.next().and_then(|v| v.parse::<u32>().ok());
            let mut lx = Lexer::new(data);
            lx.i = p + 4;
            if let (Some(id), Some(v)) = (id, lx.value()) {
                let obj = if let Obj2::Dict(d) = &v {
                    lx.skip_ws();
                    if data[lx.i..].starts_with(b"stream") {
                        let mut st = lx.i + 6;
                        if data.get(st) == Some(&b'\r') {
                            st += 1;
                        }
                        if data.get(st) == Some(&b'\n') {
                            st += 1;
                        }
                        // The stated length when "endstream" follows it, else a search.
                        let len = d.get("Length").and_then(Obj2::num).map(|v| v as usize);
                        let fits = |l: usize| {
                            let mut e = st + l;
                            while e < data.len() && data[e].is_ascii_whitespace() {
                                e += 1;
                            }
                            st + l <= data.len() && data[e.min(data.len())..].starts_with(b"endstream")
                        };
                        let end = match len {
                            Some(l) if fits(l) => st + l,
                            _ => find(data, b"endstream", st).unwrap_or(data.len()),
                        };
                        Obj2::Stream(d.clone(), data[st..end.min(data.len())].to_vec())
                    } else {
                        v
                    }
                } else {
                    v
                };
                objs.insert(id, obj);
            }
            i = p + 4;
        }
        let mut pdf = Pdf { objs };
        pdf.expand_object_streams();
        pdf
    }

    fn expand_object_streams(&mut self) {
        let streams: Vec<(u32, Obj2)> = self.objs.iter().filter(|(_, o)| matches!(o, Obj2::Stream(d, _) if d.get("Type").and_then(Obj2::name) == Some("ObjStm"))).map(|(k, v)| (*k, v.clone())).collect();
        for (_, s) in streams {
            let Obj2::Stream(d, _) = &s else { continue };
            let data = self.stream_data(&s);
            let n = d.get("N").and_then(Obj2::num).unwrap_or(0.0) as usize;
            let first = d.get("First").and_then(Obj2::num).unwrap_or(0.0) as usize;
            let mut lx = Lexer::new(&data);
            let mut pairs = Vec::new();
            for _ in 0..n {
                let (Some(Obj2::Num(id)), Some(Obj2::Num(off))) = (lx.next(), lx.next()) else { break };
                pairs.push((id as u32, off as usize));
            }
            for (id, off) in pairs {
                let mut lx = Lexer::new(&data);
                lx.i = first + off;
                if let Some(v) = lx.value() {
                    self.objs.entry(id).or_insert(v);
                }
            }
        }
    }

    fn get<'b>(&'b self, o: &'b Obj2) -> &'b Obj2 {
        match o {
            Obj2::Ref(r) => self.objs.get(r).unwrap_or(&Obj2::Null),
            _ => o,
        }
    }

    fn stream_data(&self, o: &Obj2) -> Vec<u8> {
        let Obj2::Stream(d, data) = self.get(o) else { return Vec::new() };
        let filters: Vec<String> = match d.get("Filter").map(|f| self.get(f)) {
            Some(Obj2::Name(n)) => vec![n.clone()],
            Some(Obj2::Arr(a)) => a.iter().filter_map(|x| x.name().map(String::from)).collect(),
            _ => Vec::new(),
        };
        let mut out = data.clone();
        for f in filters {
            out = match f.as_str() {
                "FlateDecode" | "Fl" => inflate(&out),
                _ => return out,
            };
        }
        out
    }

    fn pages(&self) -> Vec<&HashMap<String, Obj2>> {
        let root = self.objs.values().find(|o| o.dict().and_then(|d| d.get("Type")).and_then(Obj2::name) == Some("Catalog"));
        let mut out = Vec::new();
        fn walk<'b>(pdf: &'b Pdf, node: &'b Obj2, out: &mut Vec<&'b HashMap<String, Obj2>>, depth: usize) {
            let Some(d) = pdf.get(node).dict() else { return };
            if depth > 64 {
                return;
            }
            match d.get("Type").and_then(Obj2::name) {
                Some("Pages") => {
                    if let Some(Obj2::Arr(kids)) = d.get("Kids").map(|k| pdf.get(k)) {
                        for k in kids {
                            walk(pdf, k, out, depth + 1);
                        }
                    }
                }
                _ => out.push(d),
            }
        }
        if let Some(pages) = root.and_then(|r| r.dict()).and_then(|d| d.get("Pages")) {
            walk(self, pages, &mut out, 0);
        }
        out
    }
}

fn find(h: &[u8], n: &[u8], from: usize) -> Option<usize> {
    if from >= h.len() {
        return None;
    }
    h[from..].windows(n.len()).position(|w| w == n).map(|p| p + from)
}

#[derive(Clone)]
struct GState {
    ctm: Affine,
    fill: Color,
    stroke: Color,
    width: f64,
    join: Join,
    cap: Cap,
    miter: f64,
    dashes: Vec<f64>,
    dash_offset: f64,
    fill_alpha: f32,
    stroke_alpha: f32,
    blend: BlendMode,
    /// Depth of the clip groups this state opened.
    clips: usize,
}

struct Interp<'p> {
    pdf: &'p Pdf,
    stack: Vec<GState>,
    gs: GState,
    path: BezPath,
    pending_clip: Option<(BezPath, FillRule)>,
    /// Open groups (clip groups), innermost last; their children accumulate.
    groups: Vec<Vec<Node>>,
    doc: &'p mut Document,
    depth: usize,
}

fn cmyk(c: f64, m: f64, y: f64, k: f64) -> Color {
    Color::rgb(((1.0 - c) * (1.0 - k)) as f32, ((1.0 - m) * (1.0 - k)) as f32, ((1.0 - y) * (1.0 - k)) as f32)
}

impl Interp<'_> {
    fn emit(&mut self, n: Node) {
        if let Some(g) = self.groups.last_mut() {
            g.push(n);
        }
    }

    fn open_clip(&mut self, clip: BezPath, rule: FillRule) {
        let id = self.doc.new_id();
        let mut c = Node::new(id, NodeKind::Path { path: PathData::from_bezpath(&clip), rule });
        c.name = "Clip".into();
        self.groups.push(vec![c]);
        self.gs.clips += 1;
    }

    fn close_clips(&mut self, to: usize) {
        while self.gs.clips > to {
            self.gs.clips -= 1;
            let kids = self.groups.pop().unwrap_or_default();
            let id = self.doc.new_id();
            let g = Node::new(id, NodeKind::Group { children: kids, clip: true, layer: false });
            self.emit(g);
        }
    }

    fn paint(&mut self, fill: Option<FillRule>, stroke: bool) {
        let path = std::mem::take(&mut self.path);
        if !path.elements().is_empty() && (fill.is_some() || stroke) {
            let mut ap = Appearance::default();
            if fill.is_some() {
                let mut f = Fill::new(self.gs.fill);
                f.opacity = self.gs.fill_alpha;
                f.blend = self.gs.blend;
                ap.items.push(Item::Fill(f));
            }
            if stroke {
                let k = self.gs.ctm.determinant().abs().sqrt();
                let mut s = Stroke::new(self.gs.stroke, self.gs.width.max(0.0) * k);
                s.style.join = self.gs.join;
                s.style.start_cap = self.gs.cap;
                s.style.end_cap = self.gs.cap;
                s.style.miter_limit = self.gs.miter;
                s.style.dashes = self.gs.dashes.iter().map(|d| d * k).collect();
                s.style.dash_offset = self.gs.dash_offset * k;
                s.opacity = self.gs.stroke_alpha;
                s.blend = self.gs.blend;
                if s.style.width <= 0.0 {
                    s.style.width = 0.5;
                    s.style.non_scaling = true;
                }
                ap.items.push(Item::Stroke(s));
            }
            let id = self.doc.new_id();
            let mut n = Node::new(id, NodeKind::Path { path: PathData::from_bezpath(&path), rule: fill.unwrap_or(FillRule::NonZero) });
            n.appearance = ap;
            self.emit(n);
        }
        if let Some((clip, rule)) = self.pending_clip.take() {
            self.open_clip(clip, rule);
        }
    }

    fn run(&mut self, content: &[u8], resources: Option<&HashMap<String, Obj2>>) {
        if self.depth > 16 {
            return;
        }
        let mut lx = Lexer::new(content);
        let mut ops: Vec<Obj2> = Vec::new();
        let mut cur = Point::ZERO;
        while let Some(tok) = lx.next() {
            let Obj2::Op(op) = tok else {
                ops.push(tok);
                continue;
            };
            let nums: Vec<f64> = ops.iter().filter_map(Obj2::num).collect();
            let a = |i: usize| nums.get(i).copied().unwrap_or(0.0);
            let m = self.gs.ctm;
            match op.as_str() {
                "q" => self.stack.push(self.gs.clone()),
                "Q" => {
                    if let Some(s) = self.stack.pop() {
                        let to = s.clips;
                        self.close_clips(to);
                        let clips = self.gs.clips;
                        self.gs = s;
                        self.gs.clips = clips;
                    }
                }
                "cm" if nums.len() >= 6 => self.gs.ctm = m * Affine::new([a(0), a(1), a(2), a(3), a(4), a(5)]),
                "w" => self.gs.width = a(0),
                "J" => self.gs.cap = [Cap::Butt, Cap::Round, Cap::Square][(a(0) as usize).min(2)],
                "j" => self.gs.join = [Join::Miter, Join::Round, Join::Bevel][(a(0) as usize).min(2)],
                "M" => self.gs.miter = a(0),
                "d" => {
                    if let Some(Obj2::Arr(d)) = ops.first() {
                        self.gs.dashes = d.iter().filter_map(Obj2::num).collect();
                    }
                    self.gs.dash_offset = nums.last().copied().unwrap_or(0.0);
                }
                "m" => {
                    cur = Point::new(a(0), a(1));
                    self.path.move_to(m * cur);
                }
                "l" => {
                    cur = Point::new(a(0), a(1));
                    self.path.line_to(m * cur);
                }
                "c" => {
                    let (p1, p2, p3) = (Point::new(a(0), a(1)), Point::new(a(2), a(3)), Point::new(a(4), a(5)));
                    self.path.curve_to(m * p1, m * p2, m * p3);
                    cur = p3;
                }
                "v" => {
                    let (p2, p3) = (Point::new(a(0), a(1)), Point::new(a(2), a(3)));
                    self.path.curve_to(m * cur, m * p2, m * p3);
                    cur = p3;
                }
                "y" => {
                    let (p1, p3) = (Point::new(a(0), a(1)), Point::new(a(2), a(3)));
                    self.path.curve_to(m * p1, m * p3, m * p3);
                    cur = p3;
                }
                "h" => self.path.close_path(),
                "re" => {
                    let r = Rect::new(a(0), a(1), a(0) + a(2), a(1) + a(3));
                    self.path.extend(m * r.to_path(0.0));
                }
                "S" => self.paint(None, true),
                "s" => {
                    self.path.close_path();
                    self.paint(None, true);
                }
                "f" | "F" => self.paint(Some(FillRule::NonZero), false),
                "f*" => self.paint(Some(FillRule::EvenOdd), false),
                "B" => self.paint(Some(FillRule::NonZero), true),
                "B*" => self.paint(Some(FillRule::EvenOdd), true),
                "b" => {
                    self.path.close_path();
                    self.paint(Some(FillRule::NonZero), true);
                }
                "b*" => {
                    self.path.close_path();
                    self.paint(Some(FillRule::EvenOdd), true);
                }
                "n" => self.paint(None, false),
                "W" => self.pending_clip = Some((self.path.clone(), FillRule::NonZero)),
                "W*" => self.pending_clip = Some((self.path.clone(), FillRule::EvenOdd)),
                "g" => self.gs.fill = Color::rgb(a(0) as f32, a(0) as f32, a(0) as f32),
                "G" => self.gs.stroke = Color::rgb(a(0) as f32, a(0) as f32, a(0) as f32),
                "rg" => self.gs.fill = Color::rgb(a(0) as f32, a(1) as f32, a(2) as f32),
                "RG" => self.gs.stroke = Color::rgb(a(0) as f32, a(1) as f32, a(2) as f32),
                "k" => self.gs.fill = cmyk(a(0), a(1), a(2), a(3)),
                "K" => self.gs.stroke = cmyk(a(0), a(1), a(2), a(3)),
                "sc" | "scn" | "SC" | "SCN" => {
                    let c = match nums.len() {
                        1 => Color::rgb(a(0) as f32, a(0) as f32, a(0) as f32),
                        3 => Color::rgb(a(0) as f32, a(1) as f32, a(2) as f32),
                        4 => cmyk(a(0), a(1), a(2), a(3)),
                        _ => Color::rgb(0.5, 0.5, 0.5),
                    };
                    if op.starts_with('s') {
                        self.gs.fill = c;
                    } else {
                        self.gs.stroke = c;
                    }
                }
                "gs" => {
                    if let (Some(Obj2::Name(k)), Some(res)) = (ops.first(), resources)
                        && let Some(Obj2::Dict(ext)) = res.get("ExtGState").map(|e| self.pdf.get(e))
                        && let Some(d) = ext.get(k).map(|v| self.pdf.get(v)).and_then(Obj2::dict)
                    {
                        if let Some(v) = d.get("ca").and_then(Obj2::num) {
                            self.gs.fill_alpha = v as f32;
                        }
                        if let Some(v) = d.get("CA").and_then(Obj2::num) {
                            self.gs.stroke_alpha = v as f32;
                        }
                        if let Some(v) = d.get("LW").and_then(Obj2::num) {
                            self.gs.width = v;
                        }
                        if let Some(bm) = d.get("BM").and_then(|b| match self.pdf.get(b) {
                            Obj2::Name(n) => Some(n.clone()),
                            Obj2::Arr(a) => a.first().and_then(Obj2::name).map(String::from),
                            _ => None,
                        }) {
                            self.gs.blend = BlendMode::parse(&bm).unwrap_or(BlendMode::Normal);
                        }
                    }
                }
                "sh" => {
                    if let (Some(Obj2::Name(k)), Some(res)) = (ops.first(), resources)
                        && let Some(Obj2::Dict(shd)) = res.get("Shading").map(|e| self.pdf.get(e))
                        && let Some(sh) = shd.get(k).map(|v| self.pdf.get(v).clone())
                        && let Some(paint) = self.shading_paint(&sh)
                    {
                        // A shading paints everywhere the clip lets it.
                        let id = self.doc.new_id();
                        let mut n = Node::new(id, NodeKind::Path { path: PathData::from_bezpath(&Rect::new(-1e5, -1e5, 1e5, 1e5).to_path(0.0)), rule: FillRule::NonZero });
                        let mut f = Fill::new(paint);
                        f.opacity = self.gs.fill_alpha;
                        n.appearance.items.push(Item::Fill(f));
                        self.emit(n);
                    }
                }
                "Do" => {
                    if let (Some(Obj2::Name(k)), Some(res)) = (ops.first(), resources)
                        && let Some(Obj2::Dict(xo)) = res.get("XObject").map(|e| self.pdf.get(e))
                        && let Some(x) = xo.get(k)
                    {
                        let x = self.pdf.get(x).clone();
                        self.xobject(&x);
                    }
                }
                _ => {}
            }
            ops.clear();
        }
    }

    /// A gradient paint from an axial or radial shading, in document space.
    fn shading_paint(&self, sh: &Obj2) -> Option<Paint> {
        let d = sh.dict()?;
        let kind = d.get("ShadingType").and_then(Obj2::num)? as u32;
        let coords: Vec<f64> = match d.get("Coords").map(|c| self.pdf.get(c)) {
            Some(Obj2::Arr(a)) => a.iter().filter_map(Obj2::num).collect(),
            _ => return None,
        };
        let comps = |a: Option<&Obj2>| -> Option<Color> {
            let v: Vec<f64> = match a.map(|x| self.pdf.get(x)) {
                Some(Obj2::Arr(a)) => a.iter().filter_map(Obj2::num).collect(),
                _ => return None,
            };
            Some(match v.len() {
                1 => Color::rgb(v[0] as f32, v[0] as f32, v[0] as f32),
                4 => cmyk(v[0], v[1], v[2], v[3]),
                3 => Color::rgb(v[0] as f32, v[1] as f32, v[2] as f32),
                _ => return None,
            })
        };
        let mut stops: Vec<studio_vector_raster::Stop> = Vec::new();
        let f = self.pdf.get(d.get("Function")?);
        let fd = f.dict()?;
        match fd.get("FunctionType").and_then(Obj2::num)? as u32 {
            2 => {
                stops.push(studio_vector_raster::Stop::new(0.0, comps(fd.get("C0")).unwrap_or(Color::BLACK)));
                stops.push(studio_vector_raster::Stop::new(1.0, comps(fd.get("C1")).unwrap_or(Color::WHITE)));
            }
            3 => {
                let Some(Obj2::Arr(funcs)) = fd.get("Functions").map(|x| self.pdf.get(x)) else { return None };
                let bounds: Vec<f64> = match fd.get("Bounds").map(|x| self.pdf.get(x)) {
                    Some(Obj2::Arr(a)) => a.iter().filter_map(Obj2::num).collect(),
                    _ => Vec::new(),
                };
                let mut edges = vec![0.0];
                edges.extend(bounds);
                edges.push(1.0);
                for (i, sub) in funcs.iter().enumerate() {
                    let sd = self.pdf.get(sub).dict()?;
                    let (a, b) = (edges.get(i).copied().unwrap_or(0.0), edges.get(i + 1).copied().unwrap_or(1.0));
                    if i == 0 {
                        stops.push(studio_vector_raster::Stop::new(a as f32, comps(sd.get("C0")).unwrap_or(Color::BLACK)));
                    }
                    stops.push(studio_vector_raster::Stop::new(b as f32, comps(sd.get("C1")).unwrap_or(Color::WHITE)));
                }
            }
            _ => return None,
        }
        let g = match (kind, coords.as_slice()) {
            (2, [x0, y0, x1, y1]) => studio_vector_raster::Gradient::linear((*x0, *y0), (*x1, *y1), stops),
            (3, [x0, y0, r0, x1, y1, r1]) => studio_vector_raster::Gradient::new(GradientKind::Radial { c0: Point::new(*x0, *y0), r0: *r0, c1: Point::new(*x1, *y1), r1: *r1 }, stops),
            _ => return None,
        };
        Some(Paint::gradient(g.with_transform(self.gs.ctm)))
    }

    fn xobject(&mut self, x: &Obj2) {
        let Obj2::Stream(d, _) = x else { return };
        match d.get("Subtype").and_then(Obj2::name) {
            Some("Form") => {
                let data = self.pdf.stream_data(x);
                let saved = self.gs.clone();
                // A see-through or blended form is a group of its own.
                let grouped = self.gs.fill_alpha < 0.999 || self.gs.blend != BlendMode::Normal;
                if grouped {
                    self.groups.push(Vec::new());
                }
                self.gs.fill_alpha = 1.0;
                self.gs.stroke_alpha = 1.0;
                self.gs.blend = BlendMode::Normal;
                if let Some(Obj2::Arr(mx)) = d.get("Matrix") {
                    let v: Vec<f64> = mx.iter().filter_map(Obj2::num).collect();
                    if v.len() == 6 {
                        self.gs.ctm = self.gs.ctm * Affine::new([v[0], v[1], v[2], v[3], v[4], v[5]]);
                    }
                }
                let res = d.get("Resources").map(|r| self.pdf.get(r)).and_then(Obj2::dict).cloned();
                self.stack.push(saved.clone());
                self.depth += 1;
                self.run(&data, res.as_ref());
                self.depth -= 1;
                self.stack.pop();
                let to = saved.clips;
                self.close_clips(to);
                if grouped {
                    let kids = self.groups.pop().unwrap_or_default();
                    let id = self.doc.new_id();
                    let mut g = Node::new(id, NodeKind::Group { children: kids, clip: false, layer: false });
                    g.opacity = saved.fill_alpha;
                    g.blend = saved.blend;
                    self.gs = saved;
                    self.emit(g);
                } else {
                    self.gs = saved;
                }
            }
            Some("Image") => {
                let w = d.get("Width").and_then(Obj2::num).unwrap_or(0.0) as u32;
                let h = d.get("Height").and_then(Obj2::num).unwrap_or(0.0) as u32;
                let filter = d.get("Filter").and_then(|f| match self.pdf.get(f) {
                    Obj2::Name(n) => Some(n.clone()),
                    Obj2::Arr(a) => a.last().and_then(Obj2::name).map(String::from),
                    _ => None,
                });
                let img = match filter.as_deref() {
                    Some("DCTDecode") => {
                        let Obj2::Stream(_, raw) = x else { return };
                        super::decode_image(raw)
                    }
                    _ => {
                        let raw = self.pdf.stream_data(x);
                        let cs = d.get("ColorSpace").and_then(Obj2::name).unwrap_or("DeviceRGB");
                        let bpc = d.get("BitsPerComponent").and_then(Obj2::num).unwrap_or(8.0);
                        if bpc != 8.0 {
                            None
                        } else {
                            let ch = if cs == "DeviceGray" { 1 } else if cs == "DeviceCMYK" { 4 } else { 3 };
                            let mut px = Vec::with_capacity((w * h * 4) as usize);
                            for p in raw.chunks_exact(ch).take((w * h) as usize) {
                                match ch {
                                    1 => px.extend_from_slice(&[p[0], p[0], p[0], 255]),
                                    4 => {
                                        let c = cmyk(p[0] as f64 / 255.0, p[1] as f64 / 255.0, p[2] as f64 / 255.0, p[3] as f64 / 255.0);
                                        px.extend_from_slice(&[(c.r * 255.0) as u8, (c.g * 255.0) as u8, (c.b * 255.0) as u8, 255]);
                                    }
                                    _ => px.extend_from_slice(&[p[0], p[1], p[2], 255]),
                                }
                            }
                            studio_vector_raster::Image::from_straight(w, h, px)
                        }
                    }
                };
                let Some(img) = img else { return };
                // The image fills the unit square, its first row at the top (v = 1).
                let id = self.doc.new_id();
                let mut n = Node::new(id, NodeKind::Image(ImageNode { image: Some(img), href: None, width: 1.0, height: 1.0 }));
                n.transform = self.gs.ctm * Affine::new([1.0, 0.0, 0.0, -1.0, 0.0, 1.0]);
                self.emit(n);
            }
            _ => {}
        }
    }
}

/// Reads the vector content of a PDF: each page becomes an artboard (side
/// by side) with its art in one layer.
pub fn import(data: &[u8]) -> Result<Document, String> {
    if !data.starts_with(b"%PDF") && find(data, b"%PDF", 0).is_none_or(|p| p > 1024) {
        return Err("not a PDF".into());
    }
    let pdf = Pdf::parse(data);
    let pages = pdf.pages();
    if pages.is_empty() {
        return Err("the PDF has no pages this reader understands (it may be encrypted)".into());
    }
    let mut doc = Document::new(1.0, 1.0);
    doc.artboards.clear();
    let layer = doc.layers[0].id;
    let mut x = 0.0;
    for (pi, page) in pages.iter().enumerate().take(200) {
        let mb: Vec<f64> = match page.get("MediaBox").map(|m| pdf.get(m)) {
            Some(Obj2::Arr(a)) => a.iter().filter_map(|v| pdf.get(v).num()).collect(),
            _ => vec![0.0, 0.0, 612.0, 792.0],
        };
        let (w, h) = if mb.len() == 4 { (mb[2] - mb[0], mb[3] - mb[1]) } else { (612.0, 792.0) };
        let id = doc.new_id();
        doc.artboards.push(crate::doc::Artboard { id, name: format!("Page {}", pi + 1), rect: Rect::new(x, 0.0, x + w, h), background: Some(Color::WHITE) });
        let contents: Vec<u8> = match page.get("Contents").map(|c| pdf.get(c)) {
            Some(Obj2::Arr(a)) => a.iter().flat_map(|c| {
                let mut v = pdf.stream_data(c);
                v.push(b'\n');
                v
            }).collect(),
            Some(c @ Obj2::Stream(..)) => pdf.stream_data(c),
            _ => Vec::new(),
        };
        let res = page.get("Resources").map(|r| pdf.get(r)).and_then(Obj2::dict).cloned();
        let base = Affine::new([1.0, 0.0, 0.0, -1.0, x - mb.first().copied().unwrap_or(0.0), h + mb.get(1).copied().unwrap_or(0.0)]);
        let gs = GState { ctm: base, fill: Color::BLACK, stroke: Color::BLACK, width: 1.0, join: Join::Miter, cap: Cap::Butt, miter: 10.0, dashes: Vec::new(), dash_offset: 0.0, fill_alpha: 1.0, stroke_alpha: 1.0, blend: BlendMode::Normal, clips: 0 };
        let mut it = Interp { pdf: &pdf, stack: Vec::new(), gs, path: BezPath::new(), pending_clip: None, groups: vec![Vec::new()], doc: &mut doc, depth: 0 };
        it.run(&contents, res.as_ref());
        it.close_clips(0);
        let nodes = it.groups.pop().unwrap_or_default();
        let gid = doc.new_id();
        let mut g = Node::new(gid, NodeKind::Group { children: nodes, clip: false, layer: false });
        g.name = format!("Page {}", pi + 1);
        if let Some(c) = doc.find_mut(layer).and_then(Node::children_mut) {
            c.push(g);
        }
        x += w + 40.0;
    }
    doc.fix_ids();
    Ok(doc)
}
