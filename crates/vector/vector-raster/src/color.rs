// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Colours. Vector art is specified and blended in sRGB-encoded values, as
//! illustration files are; [`Color`] is straight (not premultiplied) alpha
//! and the renderer works premultiplied internally.

use serde::{Deserialize, Serialize};

/// An sRGB colour with straight alpha, every channel in `0..=1`. In JSON a
/// colour is `"#rrggbb"` or `"#rrggbbaa"`; `{"r", "g", "b", "a"}` and
/// `[r, g, b]` / `[r, g, b, a]` (0..1) are read too.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Serialize for Color {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Color, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Hex(String),
            Obj { r: f32, g: f32, b: f32, #[serde(default = "one")] a: f32 },
            Arr(Vec<f32>),
        }
        fn one() -> f32 {
            1.0
        }
        match Repr::deserialize(d)? {
            Repr::Hex(h) => Color::from_hex(&h).or_else(|| named(&h)).ok_or_else(|| serde::de::Error::custom(format!("not a colour: {h}"))),
            Repr::Obj { r, g, b, a } => Ok(Color::rgba(r, g, b, a)),
            Repr::Arr(v) if v.len() == 3 || v.len() == 4 => Ok(Color::rgba(v[0], v[1], v[2], *v.get(3).unwrap_or(&1.0))),
            Repr::Arr(_) => Err(serde::de::Error::custom("a colour array has 3 or 4 numbers")),
        }
    }
}

/// A few colour names (CSS basics) for hand-written documents.
pub fn named(s: &str) -> Option<Color> {
    let hex = match s.trim().to_ascii_lowercase().as_str() {
        "black" => "#000000",
        "white" => "#ffffff",
        "red" => "#ff0000",
        "green" => "#008000",
        "lime" => "#00ff00",
        "blue" => "#0000ff",
        "yellow" => "#ffff00",
        "cyan" | "aqua" => "#00ffff",
        "magenta" | "fuchsia" => "#ff00ff",
        "orange" => "#ffa500",
        "purple" => "#800080",
        "pink" => "#ffc0cb",
        "brown" => "#a52a2a",
        "gray" | "grey" => "#808080",
        "navy" => "#000080",
        "teal" => "#008080",
        "gold" => "#ffd700",
        "tomato" => "#ff6347",
        "coral" => "#ff7f50",
        "salmon" => "#fa8072",
        "crimson" => "#dc143c",
        "maroon" => "#800000",
        "olive" => "#808000",
        "silver" => "#c0c0c0",
        "indigo" => "#4b0082",
        "violet" => "#ee82ee",
        "plum" => "#dda0dd",
        "orchid" => "#da70d6",
        "lavender" => "#e6e6fa",
        "turquoise" => "#40e0d0",
        "skyblue" | "sky blue" => "#87ceeb",
        "steelblue" | "steel blue" => "#4682b4",
        "royalblue" | "royal blue" => "#4169e1",
        "midnightblue" | "midnight blue" => "#191970",
        "slategray" | "slategrey" | "slate gray" => "#708090",
        "darkgray" | "darkgrey" | "dark gray" => "#a9a9a9",
        "lightgray" | "lightgrey" | "light gray" => "#d3d3d3",
        "beige" => "#f5f5dc",
        "ivory" => "#fffff0",
        "khaki" => "#f0e68c",
        "tan" => "#d2b48c",
        "chocolate" => "#d2691e",
        "sienna" => "#a0522d",
        "peru" => "#cd853f",
        "forestgreen" | "forest green" => "#228b22",
        "seagreen" | "sea green" => "#2e8b57",
        "limegreen" | "lime green" => "#32cd32",
        "mint" | "mintcream" => "#f5fffa",
        "hotpink" | "hot pink" => "#ff69b4",
        "deeppink" | "deep pink" => "#ff1493",
        "firebrick" => "#b22222",
        "darkred" | "dark red" => "#8b0000",
        "darkblue" | "dark blue" => "#00008b",
        "darkgreen" | "dark green" => "#006400",
        "goldenrod" => "#daa520",
        "wheat" => "#f5deb3",
        "snow" => "#fffafa",
        "charcoal" => "#36454f",
        "transparent" | "none" => "#00000000",
        _ => return None,
    };
    Color::from_hex(hex)
}

impl Default for Color {
    fn default() -> Self {
        Color::BLACK
    }
}

impl Color {
    pub const BLACK: Color = Color::rgb(0.0, 0.0, 0.0);
    pub const WHITE: Color = Color::rgb(1.0, 1.0, 1.0);
    pub const TRANSPARENT: Color = Color::rgba(0.0, 0.0, 0.0, 0.0);

    pub const fn rgb(r: f32, g: f32, b: f32) -> Color {
        Color { r, g, b, a: 1.0 }
    }

    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Color {
        Color { r, g, b, a }
    }

    pub fn from_rgba8(r: u8, g: u8, b: u8, a: u8) -> Color {
        Color::rgba(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, a as f32 / 255.0)
    }

    /// `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa` (the `#` is optional).
    pub fn from_hex(s: &str) -> Option<Color> {
        let h = s.trim().trim_start_matches('#');
        let nib = |c: u8| (c as char).to_digit(16).map(|v| v as u8);
        let b = h.as_bytes();
        let pair = |i: usize| Some(nib(b[i])? * 16 + nib(b[i + 1])?);
        match b.len() {
            3 | 4 => {
                let v: Option<Vec<u8>> = b.iter().map(|&c| nib(c).map(|v| v * 17)).collect();
                let v = v?;
                Some(Color::from_rgba8(v[0], v[1], v[2], if v.len() == 4 { v[3] } else { 255 }))
            }
            6 => Some(Color::from_rgba8(pair(0)?, pair(2)?, pair(4)?, 255)),
            8 => Some(Color::from_rgba8(pair(0)?, pair(2)?, pair(4)?, pair(6)?)),
            _ => None,
        }
    }

    /// `#rrggbb`, or `#rrggbbaa` when not opaque.
    pub fn to_hex(self) -> String {
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        if q(self.a) == 255 {
            format!("#{:02x}{:02x}{:02x}", q(self.r), q(self.g), q(self.b))
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", q(self.r), q(self.g), q(self.b), q(self.a))
        }
    }

    pub fn with_alpha(self, a: f32) -> Color {
        Color { a, ..self }
    }

    /// Premultiplied `[r, g, b, a]`.
    pub fn premul(self) -> [f32; 4] {
        let a = self.a.clamp(0.0, 1.0);
        [self.r * a, self.g * a, self.b * a, a]
    }

    pub fn from_premul(p: [f32; 4]) -> Color {
        if p[3] <= 1e-7 {
            return Color::TRANSPARENT;
        }
        Color::rgba(p[0] / p[3], p[1] / p[3], p[2] / p[3], p[3])
    }

    /// Linear-light components (alpha unchanged).
    pub fn to_linear(self) -> [f32; 4] {
        [srgb_to_linear(self.r), srgb_to_linear(self.g), srgb_to_linear(self.b), self.a]
    }

    pub fn from_linear(l: [f32; 4]) -> Color {
        Color::rgba(linear_to_srgb(l[0]), linear_to_srgb(l[1]), linear_to_srgb(l[2]), l[3])
    }

    /// Oklab `[L, a, b, alpha]`.
    pub fn to_oklab(self) -> [f32; 4] {
        let [r, g, b, a] = self.to_linear();
        let l = 0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_995 * b;
        let m = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
        let s = 0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b;
        let (l, m, s) = (l.cbrt(), m.cbrt(), s.cbrt());
        [
            0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
            1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
            0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
            a,
        ]
    }

    pub fn from_oklab(o: [f32; 4]) -> Color {
        let l = o[0] + 0.396_337_78 * o[1] + 0.215_803_76 * o[2];
        let m = o[0] - 0.105_561_346 * o[1] - 0.063_854_17 * o[2];
        let s = o[0] - 0.089_484_18 * o[1] - 1.291_485_5 * o[2];
        let (l, m, s) = (l * l * l, m * m * m, s * s * s);
        Color::from_linear([
            4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s,
            -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s,
            -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s,
            o[3],
        ])
    }

    /// Relative luminance of the sRGB values (Rec. 709 weights on the
    /// encoded values, as luminance masks use).
    pub fn luminance(self) -> f32 {
        0.2125 * self.r + 0.7154 * self.g + 0.0721 * self.b
    }

    /// Hue (degrees), saturation and lightness.
    pub fn to_hsl(self) -> [f32; 3] {
        let (r, g, b) = (self.r, self.g, self.b);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let l = (max + min) / 2.0;
        let d = max - min;
        if d < 1e-6 {
            return [0.0, 0.0, l];
        }
        let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
        let h = if max == r {
            (g - b) / d + if g < b { 6.0 } else { 0.0 }
        } else if max == g {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        };
        [h * 60.0, s, l]
    }

    pub fn from_hsl(h: f32, s: f32, l: f32, a: f32) -> Color {
        let h = h.rem_euclid(360.0) / 360.0;
        if s <= 0.0 {
            return Color::rgba(l, l, l, a);
        }
        let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
        let p = 2.0 * l - q;
        let f = |mut t: f32| {
            t = t.rem_euclid(1.0);
            if t < 1.0 / 6.0 {
                p + (q - p) * 6.0 * t
            } else if t < 0.5 {
                q
            } else if t < 2.0 / 3.0 {
                p + (q - p) * (2.0 / 3.0 - t) * 6.0
            } else {
                p
            }
        };
        Color::rgba(f(h + 1.0 / 3.0), f(h), f(h - 1.0 / 3.0), a)
    }
}

pub fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

pub fn linear_to_srgb(v: f32) -> f32 {
    let v = v.max(0.0);
    if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips() {
        let c = Color::from_hex("#ff8000").unwrap();
        assert_eq!(c.to_hex(), "#ff8000");
        assert_eq!(Color::from_hex("f80").unwrap().to_hex(), "#ff8800");
        assert_eq!(Color::from_hex("#00000080").unwrap().to_hex(), "#00000080");
        assert!(Color::from_hex("#12").is_none());
    }

    #[test]
    fn oklab_and_hsl_round_trip() {
        let c = Color::rgb(0.2, 0.6, 0.9);
        let back = Color::from_oklab(c.to_oklab());
        assert!((back.r - c.r).abs() < 1e-4 && (back.g - c.g).abs() < 1e-4 && (back.b - c.b).abs() < 1e-4);
        let [h, s, l] = c.to_hsl();
        let back = Color::from_hsl(h, s, l, 1.0);
        assert!((back.r - c.r).abs() < 1e-5 && (back.b - c.b).abs() < 1e-5);
    }
}
