// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What to draw: a tree of fills, strokes and layers. A [`Scene`] is built
//! in user space; the renderers map it to pixels through
//! [`RenderOptions::transform`](crate::RenderOptions::transform), so the
//! same scene is crisp at any resolution.

use std::sync::Arc;

use kurbo::{Affine, BezPath, Rect};
use serde::{Deserialize, Serialize};

use crate::color::Color;
use crate::paint::{Paint, identity, is_identity};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FillRule {
    #[default]
    NonZero,
    EvenOdd,
}

/// How a fill, stroke or layer mixes with what is under it (the W3C
/// compositing modes, plus additive `Plus`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlendMode {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    ColorDodge,
    ColorBurn,
    HardLight,
    SoftLight,
    Difference,
    Exclusion,
    Hue,
    Saturation,
    Color,
    Luminosity,
    Plus,
}

impl BlendMode {
    pub const ALL: [BlendMode; 17] = [
        BlendMode::Normal,
        BlendMode::Multiply,
        BlendMode::Screen,
        BlendMode::Overlay,
        BlendMode::Darken,
        BlendMode::Lighten,
        BlendMode::ColorDodge,
        BlendMode::ColorBurn,
        BlendMode::HardLight,
        BlendMode::SoftLight,
        BlendMode::Difference,
        BlendMode::Exclusion,
        BlendMode::Hue,
        BlendMode::Saturation,
        BlendMode::Color,
        BlendMode::Luminosity,
        BlendMode::Plus,
    ];

    pub fn name(self) -> &'static str {
        match self {
            BlendMode::Normal => "normal",
            BlendMode::Multiply => "multiply",
            BlendMode::Screen => "screen",
            BlendMode::Overlay => "overlay",
            BlendMode::Darken => "darken",
            BlendMode::Lighten => "lighten",
            BlendMode::ColorDodge => "color_dodge",
            BlendMode::ColorBurn => "color_burn",
            BlendMode::HardLight => "hard_light",
            BlendMode::SoftLight => "soft_light",
            BlendMode::Difference => "difference",
            BlendMode::Exclusion => "exclusion",
            BlendMode::Hue => "hue",
            BlendMode::Saturation => "saturation",
            BlendMode::Color => "color",
            BlendMode::Luminosity => "luminosity",
            BlendMode::Plus => "plus",
        }
    }

    /// From a name in any common spelling (`color-dodge`, `ColorDodge`,
    /// `colour_dodge`, …).
    pub fn parse(s: &str) -> Option<BlendMode> {
        let k: String = s.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase().replace("colour", "color");
        BlendMode::ALL.into_iter().find(|m| m.name().replace('_', "") == k).or(match k.as_str() {
            "add" | "lighter" | "linear_dodge" | "lineardodge" => Some(BlendMode::Plus),
            _ => None,
        })
    }

    #[cfg_attr(not(feature = "gpu"), allow(dead_code))]
    pub(crate) fn index(self) -> u32 {
        BlendMode::ALL.iter().position(|&m| m == self).unwrap_or(0) as u32
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Join {
    #[default]
    Miter,
    Round,
    Bevel,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cap {
    #[default]
    Butt,
    Round,
    Square,
}

/// Where a stroke sits relative to a closed path's outline.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrokeAlign {
    #[default]
    Center,
    Inside,
    Outside,
}

/// One point of a variable-width profile: at `at` (0 = start of the path,
/// 1 = end, by length) the stroke is `width` times its nominal width.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WidthPoint {
    pub at: f64,
    pub width: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StrokeStyle {
    pub width: f64,
    #[serde(default)]
    pub join: Join,
    #[serde(default = "four")]
    pub miter_limit: f64,
    #[serde(default)]
    pub start_cap: Cap,
    #[serde(default)]
    pub end_cap: Cap,
    /// Dash and gap lengths, alternating; empty for a solid stroke.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dashes: Vec<f64>,
    #[serde(default)]
    pub dash_offset: f64,
    /// Variable width along the path; empty for an even stroke.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profile: Vec<WidthPoint>,
    #[serde(default)]
    pub align: StrokeAlign,
    /// The stroke keeps its width in device pixels however the art is
    /// scaled (a hairline at any zoom).
    #[serde(default)]
    pub non_scaling: bool,
}

fn four() -> f64 {
    4.0
}

impl StrokeStyle {
    pub fn new(width: f64) -> StrokeStyle {
        StrokeStyle {
            width,
            join: Join::Miter,
            miter_limit: 4.0,
            start_cap: Cap::Butt,
            end_cap: Cap::Butt,
            dashes: Vec::new(),
            dash_offset: 0.0,
            profile: Vec::new(),
            align: StrokeAlign::Center,
            non_scaling: false,
        }
    }

    pub fn with_join(mut self, j: Join) -> StrokeStyle {
        self.join = j;
        self
    }

    pub fn with_caps(mut self, c: Cap) -> StrokeStyle {
        self.start_cap = c;
        self.end_cap = c;
        self
    }

    pub fn with_dashes(mut self, d: Vec<f64>, offset: f64) -> StrokeStyle {
        self.dashes = d;
        self.dash_offset = offset;
        self
    }

    pub fn with_profile(mut self, p: Vec<WidthPoint>) -> StrokeStyle {
        self.profile = p;
        self
    }

    /// Nominal width times the profile at `t` (by length, `0..=1`).
    pub fn width_at(&self, t: f64) -> f64 {
        if self.profile.is_empty() {
            return self.width;
        }
        let p = &self.profile;
        if t <= p[0].at {
            return self.width * p[0].width;
        }
        for w in p.windows(2) {
            if t <= w[1].at {
                let u = ((t - w[0].at) / (w[1].at - w[0].at).max(1e-12)).clamp(0.0, 1.0);
                // Smooth (cosine) between profile points, like width tools draw.
                let s = 0.5 - 0.5 * (u * std::f64::consts::PI).cos();
                return self.width * (w[0].width + (w[1].width - w[0].width) * s);
            }
        }
        self.width * p[p.len() - 1].width
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Clip {
    pub path: Arc<BezPath>,
    #[serde(default = "identity", skip_serializing_if = "is_identity")]
    pub transform: Affine,
    #[serde(default)]
    pub rule: FillRule,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaskKind {
    /// The mask's brightness (times its alpha) decides what shows.
    #[default]
    Luminance,
    /// The mask's alpha decides what shows.
    Alpha,
}

/// An opacity mask: a scene whose luminance or alpha scales the layer.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Mask {
    pub scene: Scene,
    #[serde(default)]
    pub kind: MaskKind,
    #[serde(default)]
    pub invert: bool,
}

/// A raster effect on a layer. Distances are in the layer's user space
/// and scale with it, so effects look the same at every resolution.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Filter {
    /// Gaussian blur; `sigma_x` and `sigma_y` are the standard deviations.
    Blur { sigma_x: f64, sigma_y: f64 },
    /// A blurred, offset, tinted copy of the layer's alpha drawn under it.
    DropShadow { dx: f64, dy: f64, sigma: f64, color: Color, #[serde(default)] spread: f64 },
    /// Shadow cast inside the layer's shape by its edges.
    InnerShadow { dx: f64, dy: f64, sigma: f64, color: Color },
    /// A blurred, tinted halo around the layer, drawn under it.
    OuterGlow { sigma: f64, color: Color, #[serde(default)] spread: f64 },
    /// A tinted glow inside the layer's edges.
    InnerGlow { sigma: f64, color: Color },
    /// Multiplies colour and alpha (`opacity`) without isolation tricks.
    Opacity { amount: f32 },
}

impl Filter {
    pub fn blur(sigma: f64) -> Filter {
        Filter::Blur { sigma_x: sigma, sigma_y: sigma }
    }

    /// How far the effect reaches outside the layer's own pixels, in user
    /// units (3 deviations covers 99.7% of a Gaussian).
    pub fn reach(&self) -> f64 {
        match self {
            Filter::Blur { sigma_x, sigma_y } => 3.0 * sigma_x.max(*sigma_y),
            Filter::DropShadow { dx, dy, sigma, spread, .. } => 3.0 * sigma + dx.abs().max(dy.abs()) + spread.max(0.0),
            Filter::OuterGlow { sigma, spread, .. } => 3.0 * sigma + spread.max(0.0),
            _ => 0.0,
        }
    }
}

/// A group drawn on its own and then combined with what is under it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Layer {
    #[serde(default)]
    pub blend: BlendMode,
    #[serde(default = "onef")]
    pub opacity: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<Clip>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<Box<Mask>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filters: Vec<Filter>,
    /// The space `filters` distances are measured in, relative to the
    /// scene (usually the group's own transform).
    #[serde(default = "identity", skip_serializing_if = "is_identity")]
    pub filter_space: Affine,
}

fn onef() -> f32 {
    1.0
}

impl Layer {
    pub fn new() -> Layer {
        Layer { opacity: 1.0, ..Default::default() }
    }

    pub fn with_blend(mut self, b: BlendMode) -> Layer {
        self.blend = b;
        self
    }

    pub fn with_opacity(mut self, o: f32) -> Layer {
        self.opacity = o;
        self
    }

    pub fn with_clip(mut self, path: impl Into<Arc<BezPath>>, transform: Affine, rule: FillRule) -> Layer {
        self.clip = Some(Clip { path: path.into(), transform, rule });
        self
    }

    pub fn with_mask(mut self, m: Mask) -> Layer {
        self.mask = Some(Box::new(m));
        self
    }

    pub fn with_filter(mut self, f: Filter) -> Layer {
        self.filters.push(f);
        self
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Item {
    Fill {
        path: Arc<BezPath>,
        #[serde(default = "identity", skip_serializing_if = "is_identity")]
        transform: Affine,
        #[serde(default)]
        rule: FillRule,
        paint: Paint,
        #[serde(default)]
        blend: BlendMode,
        #[serde(default = "onef")]
        opacity: f32,
    },
    Stroke {
        path: Arc<BezPath>,
        #[serde(default = "identity", skip_serializing_if = "is_identity")]
        transform: Affine,
        style: StrokeStyle,
        paint: Paint,
        #[serde(default)]
        blend: BlendMode,
        #[serde(default = "onef")]
        opacity: f32,
    },
    Layer {
        layer: Layer,
        scene: Scene,
    },
}

/// A list of things to draw, bottom first.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Scene {
    pub items: Vec<Item>,
}

impl Scene {
    pub fn new() -> Scene {
        Scene::default()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn fill(&mut self, path: impl Into<Arc<BezPath>>, transform: Affine, rule: FillRule, paint: impl Into<Paint>) -> &mut Scene {
        self.items.push(Item::Fill {
            path: path.into(),
            transform,
            rule,
            paint: paint.into(),
            blend: BlendMode::Normal,
            opacity: 1.0,
        });
        self
    }

    pub fn stroke(&mut self, path: impl Into<Arc<BezPath>>, transform: Affine, style: StrokeStyle, paint: impl Into<Paint>) -> &mut Scene {
        self.items.push(Item::Stroke {
            path: path.into(),
            transform,
            style,
            paint: paint.into(),
            blend: BlendMode::Normal,
            opacity: 1.0,
        });
        self
    }

    /// Sets the blend mode and opacity of the item just added.
    pub fn blended(&mut self, mode: BlendMode, alpha: f32) -> &mut Scene {
        if let Some(Item::Fill { blend, opacity, .. } | Item::Stroke { blend, opacity, .. }) = self.items.last_mut() {
            *blend = mode;
            *opacity = alpha;
        }
        self
    }

    pub fn layer(&mut self, layer: Layer, scene: Scene) -> &mut Scene {
        self.items.push(Item::Layer { layer, scene });
        self
    }

    /// Bounds of the visible geometry in scene space (strokes padded by
    /// half their width and miters; effects not included).
    pub fn bounds(&self) -> Option<Rect> {
        let mut out: Option<Rect> = None;
        let mut add = |r: Rect| out = Some(out.map_or(r, |o| o.union(r)));
        for it in &self.items {
            match it {
                Item::Fill { path, transform, .. } => {
                    if !path.elements().is_empty() {
                        add(transform.transform_rect_bbox(kurbo::Shape::bounding_box(&**path)));
                    }
                }
                Item::Stroke { path, transform, style, .. } => {
                    if !path.elements().is_empty() {
                        let pad = style.width * style.miter_limit.max(1.0) * 0.5 * style.profile.iter().map(|p| p.width).fold(1.0, f64::max);
                        add(transform.transform_rect_bbox(kurbo::Shape::bounding_box(&**path).inflate(pad, pad)));
                    }
                }
                Item::Layer { layer, scene } => {
                    if let Some(mut r) = scene.bounds() {
                        if let Some(c) = &layer.clip {
                            match r.intersect(c.transform.transform_rect_bbox(kurbo::Shape::bounding_box(&*c.path))) {
                                i if i.width() > 0.0 && i.height() > 0.0 => r = i,
                                _ => continue,
                            }
                        }
                        let reach: f64 = layer.filters.iter().map(Filter::reach).sum();
                        if reach > 0.0 {
                            let s = layer.filter_space.as_coeffs();
                            let k = (s[0] * s[3] - s[1] * s[2]).abs().sqrt().max(1e-9);
                            r = r.inflate(reach * k, reach * k);
                        }
                        add(r);
                    }
                }
            }
        }
        out
    }
}

/// A solid paint, for brevity in builders and tests.
pub fn solid(hex: &str) -> Paint {
    Paint::solid(Color::from_hex(hex).unwrap_or(Color::BLACK))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blend_names_parse() {
        for m in BlendMode::ALL {
            assert_eq!(BlendMode::parse(m.name()), Some(m));
        }
        assert_eq!(BlendMode::parse("Color-Dodge"), Some(BlendMode::ColorDodge));
        assert_eq!(BlendMode::parse("colour"), Some(BlendMode::Color));
        assert_eq!(BlendMode::parse("add"), Some(BlendMode::Plus));
    }

    #[test]
    fn profile_is_smooth_and_clamped() {
        let s = StrokeStyle::new(10.0).with_profile(vec![WidthPoint { at: 0.0, width: 0.0 }, WidthPoint { at: 1.0, width: 1.0 }]);
        assert_eq!(s.width_at(0.0), 0.0);
        assert!((s.width_at(0.5) - 5.0).abs() < 1e-9);
        assert_eq!(s.width_at(2.0), 10.0);
    }

    #[test]
    fn scene_json_round_trip() {
        let mut s = Scene::new();
        s.fill(kurbo::Shape::to_path(&Rect::new(0.0, 0.0, 10.0, 10.0), 0.1), Affine::IDENTITY, FillRule::EvenOdd, solid("#ff0000"));
        let j = serde_json::to_string(&s).unwrap();
        let back: Scene = serde_json::from_str(&j).unwrap();
        assert_eq!(back.items.len(), 1);
        assert_eq!(back.bounds(), Some(Rect::new(0.0, 0.0, 10.0, 10.0)));
    }
}
