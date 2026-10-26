// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What a fill or stroke is painted with: a colour, a gradient (linear,
//! radial, conic, freeform or mesh), an image or a repeating pattern.

use std::sync::{Arc, OnceLock};

use kurbo::{Affine, Point, Rect};
use serde::{Deserialize, Serialize};

use crate::color::Color;
use crate::scene::Scene;

/// In JSON a paint is `{"type": "solid", "color": …}`, `{"type": "gradient",
/// "kind": "linear", …}` and so on, or just a colour (`"#ff8800"`) for a solid.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", from = "PaintRepr")]
pub enum Paint {
    Solid { color: Color },
    Gradient(Arc<Gradient>),
    Freeform(Arc<FreeformGradient>),
    Mesh(Arc<MeshGradient>),
    Image(Arc<ImagePaint>),
    Pattern(Arc<Pattern>),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum PaintRepr {
    Color(Color),
    Tagged(TaggedPaint),
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum TaggedPaint {
    Solid { color: Color },
    Gradient(Arc<Gradient>),
    Freeform(Arc<FreeformGradient>),
    Mesh(Arc<MeshGradient>),
    Image(Arc<ImagePaint>),
    Pattern(Arc<Pattern>),
}

impl From<PaintRepr> for Paint {
    fn from(r: PaintRepr) -> Paint {
        match r {
            PaintRepr::Color(color) => Paint::Solid { color },
            PaintRepr::Tagged(t) => match t {
                TaggedPaint::Solid { color } => Paint::Solid { color },
                TaggedPaint::Gradient(g) => Paint::Gradient(g),
                TaggedPaint::Freeform(f) => Paint::Freeform(f),
                TaggedPaint::Mesh(m) => Paint::Mesh(m),
                TaggedPaint::Image(i) => Paint::Image(i),
                TaggedPaint::Pattern(p) => Paint::Pattern(p),
            },
        }
    }
}

impl Paint {
    pub fn solid(color: Color) -> Paint {
        Paint::Solid { color }
    }

    pub fn gradient(g: Gradient) -> Paint {
        Paint::Gradient(Arc::new(g))
    }

    /// Whether the paint is fully transparent everywhere (so a fill with it
    /// draws nothing and can be skipped).
    pub fn is_invisible(&self) -> bool {
        match self {
            Paint::Solid { color } => color.a <= 0.0,
            Paint::Gradient(g) => g.stops.iter().all(|s| s.color.a <= 0.0),
            Paint::Freeform(f) => f.points.iter().all(|p| p.color.a <= 0.0),
            Paint::Image(i) => i.opacity <= 0.0,
            _ => false,
        }
    }
}

impl From<Color> for Paint {
    fn from(color: Color) -> Paint {
        Paint::Solid { color }
    }
}

/// How a gradient or image continues past its ends.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Extend {
    /// The end colours continue (images: the edge pixels).
    #[default]
    Pad,
    Repeat,
    Reflect,
    /// Transparent outside (images only; gradients treat it as `Pad`).
    None,
}

/// The colour space gradient stops are interpolated in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Interpolation {
    /// sRGB-encoded values (what illustration files expect).
    #[default]
    Srgb,
    /// Linear light: brighter, physically mixed midpoints.
    Linear,
    /// Oklab: perceptually even, no grey dip between complementary colours.
    Oklab,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stop {
    /// Position along the gradient, `0..=1`.
    pub offset: f32,
    pub color: Color,
    /// Where between this stop and the next the colours are half mixed, as
    /// a fraction of the gap (`0.5` = evenly).
    #[serde(default = "half")]
    pub midpoint: f32,
}

fn half() -> f32 {
    0.5
}

impl Stop {
    pub fn new(offset: f32, color: Color) -> Stop {
        Stop { offset, color, midpoint: 0.5 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GradientKind {
    /// Along the line from `start` (offset 0) to `end` (offset 1).
    Linear { start: Point, end: Point },
    /// Two-point conical: from the circle (`c0`, `r0`) to (`c1`, `r1`). A
    /// plain radial gradient has `c0 == c1` and `r0 == 0`; moving `c0`
    /// gives a focal point.
    Radial { c0: Point, r0: f64, c1: Point, r1: f64 },
    /// Around `center`, offset 0 at `angle` (radians, clockwise on screen
    /// from 3 o'clock) through a full turn.
    Conic { center: Point, angle: f64 },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Gradient {
    #[serde(flatten)]
    pub kind: GradientKind,
    pub stops: Vec<Stop>,
    #[serde(default)]
    pub extend: Extend,
    #[serde(default)]
    pub interpolation: Interpolation,
    /// Gradient space to the space of the shape it paints.
    #[serde(default = "identity", skip_serializing_if = "is_identity")]
    pub transform: Affine,
    #[serde(skip)]
    lut: OnceLock<Arc<Lut>>,
}

pub(crate) fn identity() -> Affine {
    Affine::IDENTITY
}

pub(crate) fn is_identity(a: &Affine) -> bool {
    *a == Affine::IDENTITY
}

impl Gradient {
    pub fn new(kind: GradientKind, stops: Vec<Stop>) -> Gradient {
        Gradient {
            kind,
            stops,
            extend: Extend::Pad,
            interpolation: Interpolation::Srgb,
            transform: Affine::IDENTITY,
            lut: OnceLock::new(),
        }
    }

    pub fn linear(start: impl Into<Point>, end: impl Into<Point>, stops: Vec<Stop>) -> Gradient {
        Gradient::new(GradientKind::Linear { start: start.into(), end: end.into() }, stops)
    }

    pub fn radial(center: impl Into<Point>, radius: f64, stops: Vec<Stop>) -> Gradient {
        let c = center.into();
        Gradient::new(GradientKind::Radial { c0: c, r0: 0.0, c1: c, r1: radius }, stops)
    }

    pub fn conic(center: impl Into<Point>, angle: f64, stops: Vec<Stop>) -> Gradient {
        Gradient::new(GradientKind::Conic { center: center.into(), angle }, stops)
    }

    pub fn with_extend(mut self, e: Extend) -> Gradient {
        self.extend = e;
        self
    }

    pub fn with_interpolation(mut self, i: Interpolation) -> Gradient {
        self.interpolation = i;
        self.lut = OnceLock::new();
        self
    }

    pub fn with_transform(mut self, t: Affine) -> Gradient {
        self.transform = t;
        self
    }

    /// The colour table the renderers sample, built once.
    pub(crate) fn lut(&self) -> Arc<Lut> {
        self.lut.get_or_init(|| Arc::new(Lut::build(&self.stops, self.interpolation))).clone()
    }

    /// The colour at offset `t` (after extension), straight alpha.
    pub fn color_at(&self, t: f32) -> Color {
        Color::from_premul(self.lut().sample(t))
    }
}

/// Entries in a gradient colour table; samples are interpolated between them.
pub(crate) const LUT_SIZE: usize = 512;

/// A gradient's colours at `LUT_SIZE` even offsets, premultiplied.
#[derive(Debug, PartialEq)]
pub(crate) struct Lut(pub Vec<[f32; 4]>);

impl Lut {
    fn build(stops: &[Stop], interp: Interpolation) -> Lut {
        let mut stops: Vec<Stop> = stops.iter().copied().filter(|s| s.offset.is_finite()).collect();
        // Offsets never go backwards (a stop before its predecessor sits on it).
        let mut last = 0.0f32;
        for s in &mut stops {
            s.offset = s.offset.clamp(0.0, 1.0).max(last);
            last = s.offset;
        }
        if stops.is_empty() {
            return Lut(vec![[0.0; 4]; LUT_SIZE]);
        }
        let to_space = |c: Color| -> [f32; 4] {
            match interp {
                Interpolation::Srgb => c.premul(),
                Interpolation::Linear => {
                    let l = c.to_linear();
                    [l[0] * l[3], l[1] * l[3], l[2] * l[3], l[3]]
                }
                Interpolation::Oklab => c.to_oklab(),
            }
        };
        let from_space = |v: [f32; 4]| -> [f32; 4] {
            match interp {
                Interpolation::Srgb => v,
                Interpolation::Linear => {
                    if v[3] <= 1e-7 {
                        return [0.0; 4];
                    }
                    let c = Color::from_linear([v[0] / v[3], v[1] / v[3], v[2] / v[3], v[3]]);
                    c.premul()
                }
                Interpolation::Oklab => Color::from_oklab(v).premul(),
            }
        };
        let space: Vec<[f32; 4]> = stops.iter().map(|s| to_space(s.color)).collect();
        let mut out = Vec::with_capacity(LUT_SIZE);
        for i in 0..LUT_SIZE {
            let t = i as f32 / (LUT_SIZE - 1) as f32;
            let v = if t <= stops[0].offset {
                space[0]
            } else if t >= stops[stops.len() - 1].offset {
                space[stops.len() - 1]
            } else {
                let k = stops.windows(2).position(|w| t >= w[0].offset && t <= w[1].offset).unwrap_or(0);
                let (a, b) = (stops[k], stops[k + 1]);
                let gap = (b.offset - a.offset).max(1e-9);
                let mut u = (t - a.offset) / gap;
                // A midpoint other than 0.5 bends the mix so it is half way at
                // `midpoint` (a power curve through (m, 0.5)).
                let m = a.midpoint.clamp(0.01, 0.99);
                if (m - 0.5).abs() > 1e-4 {
                    u = u.powf((0.5f32).ln() / m.ln());
                }
                let (p, q) = (space[k], space[k + 1]);
                [p[0] + (q[0] - p[0]) * u, p[1] + (q[1] - p[1]) * u, p[2] + (q[2] - p[2]) * u, p[3] + (q[3] - p[3]) * u]
            };
            out.push(from_space(v));
        }
        Lut(out)
    }

    /// The colour at `t` in `0..=1`, interpolated between entries.
    pub(crate) fn sample(&self, t: f32) -> [f32; 4] {
        let x = t.clamp(0.0, 1.0) * (LUT_SIZE - 1) as f32;
        let i = (x.floor() as usize).min(LUT_SIZE - 2);
        let f = x - i as f32;
        let (a, b) = (self.0[i], self.0[i + 1]);
        [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f, a[2] + (b[2] - a[2]) * f, a[3] + (b[3] - a[3]) * f]
    }
}

/// Colour points blended smoothly across a shape (a "freeform" gradient):
/// each point pulls the colour towards its own, more strongly nearby.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FreeformGradient {
    pub points: Vec<FreeformPoint>,
    /// Gradient space to the shape's space.
    #[serde(default = "identity", skip_serializing_if = "is_identity")]
    pub transform: Affine,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct FreeformPoint {
    pub at: Point,
    pub color: Color,
    /// How far the colour reaches, as a weight multiplier (1 = normal).
    #[serde(default = "onef")]
    pub spread: f32,
}

fn onef() -> f32 {
    1.0
}

/// A Coons-patch mesh gradient: each patch is bounded by four cubic curves
/// and blends the colours at its four corners across its area.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MeshGradient {
    pub patches: Vec<CoonsPatch>,
    #[serde(default = "identity", skip_serializing_if = "is_identity")]
    pub transform: Affine,
}

/// Twelve control points going round the patch clockwise from the top-left
/// corner (`points[0]`, `[3]`, `[6]`, `[9]` are the corners) and the
/// colours at those corners.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CoonsPatch {
    pub points: [Point; 12],
    pub colors: [Color; 4],
}

impl CoonsPatch {
    /// A patch filling `r` with straight edges.
    pub fn rect(r: Rect, colors: [Color; 4]) -> CoonsPatch {
        let l = |a: Point, b: Point, t: f64| a.lerp(b, t);
        let (p0, p1, p2, p3) = (Point::new(r.x0, r.y0), Point::new(r.x1, r.y0), Point::new(r.x1, r.y1), Point::new(r.x0, r.y1));
        CoonsPatch {
            points: [
                p0,
                l(p0, p1, 1.0 / 3.0),
                l(p0, p1, 2.0 / 3.0),
                p1,
                l(p1, p2, 1.0 / 3.0),
                l(p1, p2, 2.0 / 3.0),
                p2,
                l(p2, p3, 1.0 / 3.0),
                l(p2, p3, 2.0 / 3.0),
                p3,
                l(p3, p0, 1.0 / 3.0),
                l(p3, p0, 2.0 / 3.0),
            ],
            colors,
        }
    }

    /// The point at (`u`, `v`) in `0..=1` (u along the top edge, v down the
    /// left edge): the Coons blend of the four boundary curves.
    pub fn eval(&self, u: f64, v: f64) -> Point {
        let p = &self.points;
        let cubic = |a: Point, b: Point, c: Point, d: Point, t: f64| {
            let mt = 1.0 - t;
            (a.to_vec2() * (mt * mt * mt) + b.to_vec2() * (3.0 * mt * mt * t) + c.to_vec2() * (3.0 * mt * t * t) + d.to_vec2() * (t * t * t)).to_point()
        };
        let top = cubic(p[0], p[1], p[2], p[3], u);
        let bottom = cubic(p[9], p[8], p[7], p[6], u);
        let left = cubic(p[0], p[11], p[10], p[9], v);
        let right = cubic(p[3], p[4], p[5], p[6], v);
        let (c00, c10, c11, c01) = (p[0].to_vec2(), p[3].to_vec2(), p[6].to_vec2(), p[9].to_vec2());
        let ruled_v = top.to_vec2() * (1.0 - v) + bottom.to_vec2() * v;
        let ruled_u = left.to_vec2() * (1.0 - u) + right.to_vec2() * u;
        let bilinear = c00 * ((1.0 - u) * (1.0 - v)) + c10 * (u * (1.0 - v)) + c11 * (u * v) + c01 * ((1.0 - u) * v);
        (ruled_v + ruled_u - bilinear).to_point()
    }

    pub fn color(&self, u: f32, v: f32) -> [f32; 4] {
        let c = self.colors.map(|c| c.premul());
        let mut out = [0.0; 4];
        for (i, o) in out.iter_mut().enumerate() {
            *o = c[0][i] * (1.0 - u) * (1.0 - v) + c[1][i] * u * (1.0 - v) + c[2][i] * u * v + c[3][i] * (1.0 - u) * v;
        }
        out
    }
}

/// How an image is sampled between and across its pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageQuality {
    /// Hard pixels (pixel art).
    Nearest,
    /// Bilinear, with mipmaps when shrinking.
    #[default]
    Bilinear,
    /// Bicubic (Catmull-Rom) when enlarging, mipmaps when shrinking.
    Bicubic,
}

/// An RGBA8 image, premultiplied, rows top to bottom.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    #[serde(with = "bytes_b64")]
    pub data: Arc<Vec<u8>>,
    #[serde(skip)]
    pub(crate) mips: Arc<OnceLock<Vec<Mip>>>,
}

impl PartialEq for Image {
    fn eq(&self, o: &Image) -> bool {
        self.width == o.width && self.height == o.height && self.data == o.data
    }
}

#[derive(Debug)]
pub(crate) struct Mip {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl Image {
    /// From premultiplied RGBA8 pixels. `None` if the length is wrong.
    pub fn from_premul(width: u32, height: u32, data: Vec<u8>) -> Option<Image> {
        (data.len() == width as usize * height as usize * 4 && width > 0 && height > 0).then(|| Image {
            width,
            height,
            data: Arc::new(data),
            mips: Arc::new(OnceLock::new()),
        })
    }

    /// From straight-alpha RGBA8 pixels (what PNG decoders give).
    pub fn from_straight(width: u32, height: u32, mut data: Vec<u8>) -> Option<Image> {
        for px in data.chunks_exact_mut(4) {
            let a = px[3] as u32;
            if a < 255 {
                for c in &mut px[..3] {
                    *c = ((*c as u32 * a + 127) / 255) as u8;
                }
            }
        }
        Image::from_premul(width, height, data)
    }

    /// Halved levels down to 1×1 (box filtered), built on first use.
    pub(crate) fn mips(&self) -> &[Mip] {
        self.mips.get_or_init(|| {
            let mut out: Vec<Mip> = Vec::new();
            let (mut w, mut h) = (self.width, self.height);
            while w > 1 || h > 1 {
                let (src, sw, sh): (&[u8], u32, u32) = match out.last() {
                    Some(m) => (&m.data, m.width, m.height),
                    None => (&self.data, w, h),
                };
                let (nw, nh) = ((sw / 2).max(1), (sh / 2).max(1));
                let mut data = vec![0u8; nw as usize * nh as usize * 4];
                for y in 0..nh {
                    for x in 0..nw {
                        for c in 0..4 {
                            let mut sum = 0u32;
                            let mut n = 0u32;
                            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                                let (sx, sy) = ((x * 2 + dx).min(sw - 1), (y * 2 + dy).min(sh - 1));
                                sum += src[(sy as usize * sw as usize + sx as usize) * 4 + c] as u32;
                                n += 1;
                            }
                            data[(y as usize * nw as usize + x as usize) * 4 + c] = ((sum + n / 2) / n) as u8;
                        }
                    }
                }
                out.push(Mip { width: nw, height: nh, data });
                w = nw;
                h = nh;
            }
            out
        })
    }
}

mod bytes_b64 {
    //! Image pixels in JSON as base64, so scenes can be saved and logged.
    use std::sync::Arc;

    use serde::{Deserialize, Deserializer, Serializer};

    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn serialize<S: Serializer>(v: &Arc<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
        let mut out = String::with_capacity(v.len().div_ceil(3) * 4);
        for c in v.chunks(3) {
            let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
            out.push(A[(n >> 18) as usize & 63] as char);
            out.push(A[(n >> 12) as usize & 63] as char);
            out.push(if c.len() > 1 { A[(n >> 6) as usize & 63] as char } else { '=' });
            out.push(if c.len() > 2 { A[n as usize & 63] as char } else { '=' });
        }
        s.serialize_str(&out)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Arc<Vec<u8>>, D::Error> {
        let s = String::deserialize(d)?;
        let val = |c: u8| A.iter().position(|&a| a == c).map(|p| p as u32);
        let mut out = Vec::with_capacity(s.len() / 4 * 3);
        let b: Vec<u8> = s.bytes().filter(|c| !c.is_ascii_whitespace()).collect();
        for q in b.chunks(4) {
            let mut n = 0u32;
            let mut k: usize = 0;
            for &c in q {
                if c == b'=' {
                    break;
                }
                n |= val(c).ok_or_else(|| serde::de::Error::custom("bad base64"))? << (18 - 6 * k);
                k += 1;
            }
            for i in 0..k.saturating_sub(1) {
                out.push((n >> (16 - 8 * i)) as u8);
            }
        }
        Ok(Arc::new(out))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImagePaint {
    pub image: Image,
    /// Image pixel space to the shape's space (an image drawn at its own
    /// size at the origin is the identity).
    #[serde(default = "identity", skip_serializing_if = "is_identity")]
    pub transform: Affine,
    #[serde(default)]
    pub extend: Extend,
    #[serde(default)]
    pub quality: ImageQuality,
    #[serde(default = "onef")]
    pub opacity: f32,
}

/// A tile of vector art repeated across a shape.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pattern {
    pub tile: Scene,
    /// The repeating cell, in the tile's own space.
    pub rect: Rect,
    /// Pattern space to the shape's space.
    #[serde(default = "identity", skip_serializing_if = "is_identity")]
    pub transform: Affine,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lut_hits_stops_and_midpoints() {
        let g = Gradient::linear((0.0, 0.0), (1.0, 0.0), vec![Stop::new(0.0, Color::BLACK), Stop::new(1.0, Color::WHITE)]);
        assert!((g.color_at(0.5).r - 0.5).abs() < 0.01);
        assert_eq!(g.color_at(0.0).r, 0.0);
        assert!((g.color_at(1.0).r - 1.0).abs() < 1e-6);
        let mut s = vec![Stop::new(0.0, Color::BLACK), Stop::new(1.0, Color::WHITE)];
        s[0].midpoint = 0.25;
        let g = Gradient::linear((0.0, 0.0), (1.0, 0.0), s);
        assert!((g.color_at(0.25).r - 0.5).abs() < 0.01, "{}", g.color_at(0.25).r);
    }

    #[test]
    fn oklab_avoids_grey_dip() {
        let stops = vec![Stop::new(0.0, Color::rgb(0.0, 0.0, 1.0)), Stop::new(1.0, Color::rgb(1.0, 1.0, 0.0))];
        let srgb = Gradient::linear((0.0, 0.0), (1.0, 0.0), stops.clone()).color_at(0.5);
        let ok = Gradient::linear((0.0, 0.0), (1.0, 0.0), stops).with_interpolation(Interpolation::Oklab).color_at(0.5);
        assert!(ok.luminance() > srgb.luminance());
    }

    #[test]
    fn image_json_round_trip() {
        let img = Image::from_straight(2, 1, vec![255, 0, 0, 255, 0, 255, 0, 128]).unwrap();
        let s = serde_json::to_string(&img).unwrap();
        let back: Image = serde_json::from_str(&s).unwrap();
        assert_eq!(back, img);
        assert_eq!(img.mips().len(), 1);
    }
}
