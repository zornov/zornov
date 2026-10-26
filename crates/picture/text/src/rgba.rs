// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Colours as creators type them: sRGB, straight alpha, 0..1 per channel.
//!
//! Everything the painter composites is linear light and premultiplied
//! (the `studio_colour::Image` convention), so colours are converted once
//! at paint time with [`Rgba::linear_premul`].

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// An sRGB colour with straight (not premultiplied) alpha.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Rgba {
    pub const WHITE: Rgba = Rgba::new(1.0, 1.0, 1.0, 1.0);
    pub const BLACK: Rgba = Rgba::new(0.0, 0.0, 0.0, 1.0);
    pub const TRANSPARENT: Rgba = Rgba::new(0.0, 0.0, 0.0, 0.0);

    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Rgba {
        Rgba { r, g, b, a }
    }

    /// Opaque colour from 8-bit channels.
    pub const fn rgb8(r: u8, g: u8, b: u8) -> Rgba {
        Rgba { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a: 1.0 }
    }

    /// Parses `#RGB`, `#RRGGBB` or `#RRGGBBAA` (the `#` is optional).
    /// Anything else gives opaque magenta so a mistake is visible.
    pub fn hex(s: &str) -> Rgba {
        let s = s.trim().trim_start_matches('#');
        let v = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
        match s.len() {
            3 => {
                let c = |i: usize| u8::from_str_radix(&s[i..i + 1], 16).ok().map(|x| x * 17);
                match (c(0), c(1), c(2)) {
                    (Some(r), Some(g), Some(b)) => Rgba::rgb8(r, g, b),
                    _ => Rgba::rgb8(255, 0, 255),
                }
            }
            6 => match (v(0), v(2), v(4)) {
                (Some(r), Some(g), Some(b)) => Rgba::rgb8(r, g, b),
                _ => Rgba::rgb8(255, 0, 255),
            },
            8 => match (v(0), v(2), v(4), v(6)) {
                (Some(r), Some(g), Some(b), Some(a)) => Rgba::rgb8(r, g, b).alpha(a as f32 / 255.0),
                _ => Rgba::rgb8(255, 0, 255),
            },
            _ => Rgba::rgb8(255, 0, 255),
        }
    }

    /// `#RRGGBB` or `#RRGGBBAA` when alpha is not 1.
    pub fn to_hex(&self) -> String {
        let q = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
        if (self.a - 1.0).abs() < 1e-6 {
            format!("#{:02X}{:02X}{:02X}", q(self.r), q(self.g), q(self.b))
        } else {
            format!("#{:02X}{:02X}{:02X}{:02X}", q(self.r), q(self.g), q(self.b), q(self.a))
        }
    }

    /// The same colour with another alpha.
    pub const fn alpha(self, a: f32) -> Rgba {
        Rgba { a, ..self }
    }

    /// Alpha multiplied by `k`.
    pub fn fade(self, k: f32) -> Rgba {
        Rgba { a: self.a * k, ..self }
    }

    /// Linear interpolation in sRGB (what gradient pickers expect).
    pub fn lerp(self, o: Rgba, t: f32) -> Rgba {
        Rgba {
            r: self.r + (o.r - self.r) * t,
            g: self.g + (o.g - self.g) * t,
            b: self.b + (o.b - self.b) * t,
            a: self.a + (o.a - self.a) * t,
        }
    }

    /// Linear-light, premultiplied, as the painter composites it.
    pub fn linear_premul(self) -> [f32; 4] {
        let a = self.a.clamp(0.0, 1.0);
        [srgb_to_linear(self.r) * a, srgb_to_linear(self.g) * a, srgb_to_linear(self.b) * a, a]
    }

    /// Perceived lightness 0..1 (Rec. 709 luma of the linear colour).
    pub fn luma(self) -> f32 {
        0.2126 * srgb_to_linear(self.r) + 0.7152 * srgb_to_linear(self.g) + 0.0722 * srgb_to_linear(self.b)
    }

    /// Hue shift in degrees, keeping saturation and value.
    pub fn shift_hue(self, degrees: f32) -> Rgba {
        let (h, s, v) = rgb_to_hsv(self.r, self.g, self.b);
        let (r, g, b) = hsv_to_rgb((h + degrees).rem_euclid(360.0), s, v);
        Rgba { r, g, b, a: self.a }
    }

    /// From hue (degrees), saturation and value, all opaque.
    pub fn hsv(h: f32, s: f32, v: f32) -> Rgba {
        let (r, g, b) = hsv_to_rgb(h.rem_euclid(360.0), s, v);
        Rgba { r, g, b, a: 1.0 }
    }
}

/// IEC 61966-2-1 decoding.
pub fn srgb_to_linear(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// IEC 61966-2-1 encoding.
pub fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

fn rgb_to_hsv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d < 1e-6 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let s = if max < 1e-6 { 0.0 } else { d / max };
    (h, s, max)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match (h / 60.0) as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    (r + m, g + m, b + m)
}

impl Serialize for Rgba {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Rgba {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(Rgba::hex(&s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip() {
        let c = Rgba::hex("#1E90FF");
        assert_eq!(c.to_hex(), "#1E90FF");
        assert_eq!(Rgba::hex("#1E90FF80").to_hex(), "#1E90FF80");
        assert_eq!(Rgba::hex("fff"), Rgba::WHITE);
        assert_eq!(Rgba::hex("nope"), Rgba::rgb8(255, 0, 255));
    }

    #[test]
    fn srgb_transfer_round_trip() {
        for i in 0..=100 {
            let c = i as f32 / 100.0;
            assert!((linear_to_srgb(srgb_to_linear(c)) - c).abs() < 1e-5);
        }
        assert!((srgb_to_linear(0.5) - 0.2140).abs() < 1e-3);
    }

    #[test]
    fn hue_shift_cycles() {
        let c = Rgba::rgb8(255, 0, 0).shift_hue(120.0);
        assert!((c.g - 1.0).abs() < 1e-5 && c.r < 1e-5);
        let back = c.shift_hue(240.0);
        assert!((back.r - 1.0).abs() < 1e-5);
    }
}
