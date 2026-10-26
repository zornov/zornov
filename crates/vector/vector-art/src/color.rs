// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Swatches (global colours that recolour every object using them) and
//! colour harmonies built in a perceptual space (Oklab lightness and
//! chroma kept, hue turned), so harmony colours look equally strong.

use serde::{Deserialize, Serialize};
use studio_vector_raster::{Color, Paint};

use crate::appearance::Item;
use crate::doc::{Document, Node};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Swatch {
    pub id: String,
    #[serde(default)]
    pub name: String,
    pub color: Color,
    /// A global swatch: fills and strokes linked to it follow its colour.
    #[serde(default)]
    pub global: bool,
    /// A group name, to keep palettes together.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub group: String,
}

/// A colour harmony rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Harmony {
    Complementary,
    SplitComplementary,
    Analogous,
    Triadic,
    Tetradic,
    Square,
    Monochromatic,
    Shades,
    Tints,
}

impl Harmony {
    pub fn parse(s: &str) -> Option<Harmony> {
        let k = s.to_ascii_lowercase().replace(['-', ' '], "_");
        Some(match k.as_str() {
            "complementary" | "complement" => Harmony::Complementary,
            "split_complementary" | "split" => Harmony::SplitComplementary,
            "analogous" => Harmony::Analogous,
            "triadic" | "triad" => Harmony::Triadic,
            "tetradic" | "rectangle" => Harmony::Tetradic,
            "square" => Harmony::Square,
            "monochromatic" | "mono" => Harmony::Monochromatic,
            "shades" => Harmony::Shades,
            "tints" => Harmony::Tints,
            _ => return None,
        })
    }
}

/// Turns a colour's hue by `deg` in Oklab (lightness and chroma kept).
pub fn rotate_hue(c: Color, deg: f64) -> Color {
    let [l, a, b, al] = c.to_oklab();
    let (s, co) = (deg.to_radians() as f32).sin_cos();
    let out = Color::from_oklab([l, a * co - b * s, a * s + b * co, al]);
    clamp(out)
}

fn clamp(c: Color) -> Color {
    Color::rgba(c.r.clamp(0.0, 1.0), c.g.clamp(0.0, 1.0), c.b.clamp(0.0, 1.0), c.a.clamp(0.0, 1.0))
}

fn with_lightness(c: Color, l: f32) -> Color {
    let [_, a, b, al] = c.to_oklab();
    clamp(Color::from_oklab([l.clamp(0.0, 1.0), a, b, al]))
}

/// The colours of `rule` starting from `base` (which comes first).
pub fn harmony(base: Color, rule: Harmony, count: usize) -> Vec<Color> {
    let count = count.clamp(2, 12);
    match rule {
        Harmony::Complementary => vec![base, rotate_hue(base, 180.0)],
        Harmony::SplitComplementary => vec![base, rotate_hue(base, 150.0), rotate_hue(base, 210.0)],
        Harmony::Analogous => (0..count).map(|i| rotate_hue(base, (i as f64 - (count as f64 - 1.0) / 2.0) * 30.0)).collect(),
        Harmony::Triadic => vec![base, rotate_hue(base, 120.0), rotate_hue(base, 240.0)],
        Harmony::Tetradic => vec![base, rotate_hue(base, 60.0), rotate_hue(base, 180.0), rotate_hue(base, 240.0)],
        Harmony::Square => vec![base, rotate_hue(base, 90.0), rotate_hue(base, 180.0), rotate_hue(base, 270.0)],
        Harmony::Monochromatic => {
            let l = base.to_oklab()[0];
            (0..count).map(|i| with_lightness(base, (l - 0.3 + 0.6 * i as f32 / (count - 1) as f32).clamp(0.15, 0.95))).collect()
        }
        Harmony::Shades => (0..count).map(|i| mix(base, Color::BLACK, i as f32 / count as f32)).collect(),
        Harmony::Tints => (0..count).map(|i| mix(base, Color::WHITE, i as f32 / count as f32)).collect(),
    }
}

/// `a` mixed towards `b` by `t` in Oklab.
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    let (p, q) = (a.to_oklab(), b.to_oklab());
    clamp(Color::from_oklab([0, 1, 2, 3].map(|i| p[i] + (q[i] - p[i]) * t)))
}

/// WCAG contrast ratio between two opaque colours (1..21).
pub fn contrast(a: Color, b: Color) -> f64 {
    let lum = |c: Color| {
        let l = c.to_linear();
        0.2126 * l[0] as f64 + 0.7152 * l[1] as f64 + 0.0722 * l[2] as f64
    };
    let (x, y) = (lum(a), lum(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// The swatch's colour at `tint` (1 = full, 0 = white), as global swatch
/// tints work.
pub fn tinted(c: Color, tint: f32) -> Color {
    let t = tint.clamp(0.0, 1.0);
    Color::rgba(1.0 - (1.0 - c.r) * t, 1.0 - (1.0 - c.g) * t, 1.0 - (1.0 - c.b) * t, c.a)
}

impl Document {
    pub fn swatch(&self, key: &str) -> Option<&Swatch> {
        self.swatches.iter().find(|s| s.id == key || s.name == key)
    }

    /// Adds (or replaces, by id) a swatch.
    pub fn set_swatch(&mut self, s: Swatch) {
        if let Some(old) = self.swatches.iter_mut().find(|o| o.id == s.id) {
            *old = s;
        } else {
            self.swatches.push(s);
        }
    }

    /// Every colour used by the art, with how many fills and strokes use it.
    pub fn colours_used(&self) -> Vec<(Color, usize)> {
        let mut out: Vec<(Color, usize)> = Vec::new();
        let mut add = |c: Color| {
            let key = c.to_hex();
            if let Some(e) = out.iter_mut().find(|(o, _)| o.to_hex() == key) {
                e.1 += 1;
            } else {
                out.push((c, 1));
            }
        };
        self.walk(&mut |n: &Node| {
            for it in &n.appearance.items {
                let p = match it {
                    Item::Fill(f) => &f.paint,
                    Item::Stroke(s) => &s.paint,
                };
                match p {
                    Paint::Solid { color } => add(*color),
                    Paint::Gradient(g) => g.stops.iter().for_each(|s| add(s.color)),
                    _ => {}
                }
            }
        });
        out.sort_by(|a, b| b.1.cmp(&a.1));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complement_turns_half_way_round() {
        let base = Color::from_hex("#d94f30").unwrap();
        let c = harmony(base, Harmony::Complementary, 2);
        assert_eq!(c.len(), 2);
        // Opposite hue: for a colour whose complement is in gamut, turning
        // twice comes back.
        let soft = Color::from_hex("#9a7b6c").unwrap();
        let back = rotate_hue(rotate_hue(soft, 180.0), 180.0);
        let base_for_back = soft;
        assert!((back.r - base_for_back.r).abs() < 0.02 && (back.g - base_for_back.g).abs() < 0.02 && (back.b - base_for_back.b).abs() < 0.02, "{back:?}");
        let [h0, _, _] = base.to_hsl();
        let [h1, _, _] = c[1].to_hsl();
        let d = ((h1 - h0).rem_euclid(360.0) - 180.0).abs();
        assert!(d < 40.0, "hues {h0} {h1}");
    }

    #[test]
    fn contrast_of_black_on_white_is_21() {
        assert!((contrast(Color::BLACK, Color::WHITE) - 21.0).abs() < 1e-6);
        assert_eq!(harmony(Color::WHITE, Harmony::Tints, 4).len(), 4);
        assert_eq!(tinted(Color::BLACK, 0.5).to_hex(), "#808080");
    }
}
