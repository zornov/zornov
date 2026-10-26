// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Test support: SVG documents turned into scenes (through usvg) so the
//! same art can be drawn by this rasterizer and by an independent one
//! (resvg) and compared, plus image statistics and PNG output.
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;

use studio_vector_raster::kurbo::{Affine, BezPath, Point};
use studio_vector_raster::*;

pub fn affine(t: usvg::Transform) -> Affine {
    Affine::new([t.sx as f64, t.ky as f64, t.kx as f64, t.sy as f64, t.tx as f64, t.ty as f64])
}

pub fn bez(p: &usvg::tiny_skia_path::Path) -> BezPath {
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

fn color(c: usvg::Color, o: f32) -> Color {
    Color::rgba(c.red as f32 / 255.0, c.green as f32 / 255.0, c.blue as f32 / 255.0, o)
}

fn stops(s: &[usvg::Stop]) -> Vec<Stop> {
    s.iter().map(|s| Stop::new(s.offset().get(), color(s.color(), s.opacity().get()))).collect()
}

fn spread(s: usvg::SpreadMethod) -> Extend {
    match s {
        usvg::SpreadMethod::Pad => Extend::Pad,
        usvg::SpreadMethod::Reflect => Extend::Reflect,
        usvg::SpreadMethod::Repeat => Extend::Repeat,
    }
}

fn paint(p: &usvg::Paint, opacity: f32) -> Option<Paint> {
    Some(match p {
        usvg::Paint::Color(c) => Paint::solid(color(*c, opacity)),
        usvg::Paint::LinearGradient(g) => {
            let mut st = stops(g.stops());
            st.iter_mut().for_each(|s| s.color.a *= opacity);
            Paint::gradient(Gradient::linear((g.x1() as f64, g.y1() as f64), (g.x2() as f64, g.y2() as f64), st).with_extend(spread(g.spread_method())).with_transform(affine(g.transform())))
        }
        usvg::Paint::RadialGradient(g) => {
            let mut st = stops(g.stops());
            st.iter_mut().for_each(|s| s.color.a *= opacity);
            let kind = GradientKind::Radial {
                c0: Point::new(g.fx() as f64, g.fy() as f64),
                r0: g.fr().get() as f64,
                c1: Point::new(g.cx() as f64, g.cy() as f64),
                r1: g.r().get() as f64,
            };
            Paint::gradient(Gradient::new(kind, st).with_extend(spread(g.spread_method())).with_transform(affine(g.transform())))
        }
        usvg::Paint::Pattern(p) => {
            let r = p.rect();
            let rect = studio_vector_raster::kurbo::Rect::new(r.left() as f64, r.top() as f64, r.right() as f64, r.bottom() as f64);
            let mut tile = group_scene(p.root(), Affine::IDENTITY);
            if opacity < 1.0 {
                let mut s = Scene::new();
                s.layer(Layer::new().with_opacity(opacity), tile);
                tile = s;
            }
            Paint::Pattern(Arc::new(Pattern { tile, rect, transform: affine(p.transform()) }))
        }
    })
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

fn path(p: &usvg::Path, out: &mut Scene, t: Affine) {
    if !p.is_visible() {
        return;
    }
    let b = Arc::new(bez(p.data()));
    if let Some(f) = p.fill() {
        if let Some(pt) = paint(f.paint(), f.opacity().get()) {
            let rule = if f.rule() == usvg::FillRule::EvenOdd { FillRule::EvenOdd } else { FillRule::NonZero };
            out.fill(b.clone(), t, rule, pt);
        }
    }
    if let Some(s) = p.stroke() {
        if let Some(pt) = paint(s.paint(), s.opacity().get()) {
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
            out.stroke(b, t, st, pt);
        }
    }
}

fn group_scene(g: &usvg::Group, t: Affine) -> Scene {
    let mut inner = Scene::new();
    for n in g.children() {
        match n {
            usvg::Node::Path(p) => path(p, &mut inner, t),
            usvg::Node::Group(sub) => group(sub, &mut inner, t),
            _ => {}
        }
    }
    inner
}

fn group(g: &usvg::Group, out: &mut Scene, parent: Affine) {
    let t = parent * affine(g.transform());
    let inner = group_scene(g, t);
    let mut layer = Layer::new().with_blend(blend(g.blend_mode())).with_opacity(g.opacity().get());
    if let Some(c) = g.clip_path() {
        // Clip paths can hold several shapes: their union is an alpha mask.
        let ct = t * affine(c.transform());
        let mut m = Scene::new();
        for n in c.root().children() {
            if let usvg::Node::Path(p) = n {
                let rule = p.fill().map(|f| f.rule()).unwrap_or(usvg::FillRule::NonZero);
                let rule = if rule == usvg::FillRule::EvenOdd { FillRule::EvenOdd } else { FillRule::NonZero };
                m.fill(bez(p.data()), ct, rule, Paint::solid(Color::WHITE));
            }
        }
        layer.mask = Some(Box::new(Mask { scene: m, kind: MaskKind::Alpha, invert: false }));
    }
    if let Some(mk) = g.mask() {
        let ms = group_scene(mk.root(), t);
        let r = mk.rect();
        let rect = studio_vector_raster::kurbo::Rect::new(r.left() as f64, r.top() as f64, r.right() as f64, r.bottom() as f64);
        let mut clipped = Scene::new();
        clipped.layer(Layer::new().with_clip(studio_vector_raster::kurbo::Shape::to_path(&rect, 0.0), t, FillRule::NonZero).with_blend(BlendMode::Normal), ms);
        let kind = if mk.kind() == usvg::MaskType::Alpha { MaskKind::Alpha } else { MaskKind::Luminance };
        // A clip-path and a mask together: the mask nests inside.
        if layer.mask.is_some() {
            let inner_layer = Layer::new().with_mask(Mask { scene: clipped, kind, invert: false });
            let mut s = Scene::new();
            s.layer(inner_layer, inner);
            out.layer(layer, s);
            return;
        }
        layer.mask = Some(Box::new(Mask { scene: clipped, kind, invert: false }));
    }
    for f in g.filters() {
        for p in f.primitives() {
            if let usvg::filter::Kind::GaussianBlur(b) = p.kind() {
                layer.filters.push(Filter::Blur { sigma_x: b.std_dev_x().get() as f64, sigma_y: b.std_dev_y().get() as f64 });
                layer.filter_space = t;
            }
        }
    }
    out.layer(layer, inner);
}

/// The SVG as a scene in its own user space, and its pixel size.
pub fn svg_scene(svg: &str) -> (Scene, u32, u32) {
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default()).expect("svg parses");
    let s = tree.size();
    let mut out = Scene::new();
    for n in tree.root().children() {
        match n {
            usvg::Node::Path(p) => path(p, &mut out, Affine::IDENTITY),
            usvg::Node::Group(g) => group(g, &mut out, Affine::IDENTITY),
            _ => {}
        }
    }
    (out, s.width().ceil() as u32, s.height().ceil() as u32)
}

/// The independent reference: resvg's rendering, premultiplied RGBA8.
pub fn resvg_render(svg: &str, scale: f32) -> Pixmap {
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default()).expect("svg parses");
    let s = tree.size();
    let (w, h) = ((s.width() * scale).ceil() as u32, (s.height() * scale).ceil() as u32);
    let mut pm = resvg::tiny_skia::Pixmap::new(w, h).expect("pixmap");
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pm.as_mut());
    Pixmap { width: w, height: h, data: pm.data().to_vec() }
}

#[derive(Debug)]
pub struct Diff {
    pub max: u8,
    /// Mean absolute difference over every channel, in 1/255 steps.
    pub mean: f64,
    /// Share of pixels with a channel more than 8 levels off.
    pub off: f64,
}

pub fn diff(a: &Pixmap, b: &Pixmap) -> Diff {
    assert_eq!((a.width, a.height), (b.width, b.height));
    let mut max = 0u8;
    let mut sum = 0u64;
    let mut off = 0usize;
    for (pa, pb) in a.data.chunks_exact(4).zip(b.data.chunks_exact(4)) {
        let mut worst = 0u8;
        for c in 0..4 {
            let d = pa[c].abs_diff(pb[c]);
            sum += d as u64;
            worst = worst.max(d);
        }
        max = max.max(worst);
        if worst > 8 {
            off += 1;
        }
    }
    let n = a.data.len() / 4;
    Diff { max, mean: sum as f64 / (n * 4) as f64, off: off as f64 / n as f64 }
}

pub fn out_dir() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("vector-raster");
    std::fs::create_dir_all(&d).ok();
    d
}

pub fn write_png(path: &std::path::Path, p: &Pixmap) {
    let f = std::fs::File::create(path).expect("create png");
    let mut e = png::Encoder::new(std::io::BufWriter::new(f), p.width, p.height);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().expect("png header").write_image_data(&p.to_straight()).expect("png data");
}

pub fn read_png(path: &std::path::Path) -> Option<Pixmap> {
    let d = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).ok()?));
    let mut r = d.read_info().ok()?;
    let mut buf = vec![0; r.output_buffer_size()];
    let info = r.next_frame(&mut buf).ok()?;
    if info.color_type != png::ColorType::Rgba {
        return None;
    }
    let img = Image::from_straight(info.width, info.height, buf[..info.buffer_size()].to_vec())?;
    Some(Pixmap { width: info.width, height: info.height, data: img.data.to_vec() })
}
